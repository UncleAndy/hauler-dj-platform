#!/usr/bin/env python3
"""Собрать msi-пакет Windows из готового дистрибутива.

Пакет собирается на Linux: `wixl` из msitools читает WiX-описание и пишет msi. Описание генерируется здесь,
потому что файлов в дистрибутиве больше полутора тысяч и руками их список не поддерживать.

Что попадает в пакет:

  * `Program Files\\Hauler DJ` — всё содержимое дистрибутива, кроме самого zip: он artefact сборки, а не
    часть установки;
  * ярлык в меню «Пуск» на `hauler-dj.exe` — запускатор поднимает движок и оболочку и сам консоли не
    показывает (`hauler-dj.bat` рядом остаётся диагностическим путём, с консолью);
  * запись в списке установленных программ, версия из `Cargo.toml`.

Идентификаторы компонентов берутся из пути файла (UUIDv5), поэтому повторная сборка той же версии даёт тот же
пакет, а `UpgradeCode` постоянен — по нему Windows понимает, что новая версия заменяет старую.

Запуск: `scripts/windows-msi.py --dist dist/windows [--version 0.1.0]`, `wixl` должен быть в PATH.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import uuid
from pathlib import Path

# Постоянный UpgradeCode продукта: он не меняется от версии к версии.
UPGRADE_NAMESPACE = uuid.UUID("6f2f1f5a-8d3c-5b1e-9d47-2f4a6c8e0b13")
UPGRADE_CODE = str(uuid.uuid5(UPGRADE_NAMESPACE, "hauler-dj-product")).upper()

PRODUCT_NAME = "Hauler DJ"
MANUFACTURER = "Hauler DJ"
FOLDER_NAME = "Hauler DJ"
SHORTCUT_NAME = "Hauler DJ"

# Упаковка дистрибутива и сам пакет — то, из чего собран msi, а не то, что ставится на машину.
SKIP_NAMES = {"hauler-dj-windows.zip"}
SKIP_SUFFIXES = {".zip", ".msi"}


def workspace_version(repo: Path) -> str:
    """Версия из Cargo.toml рабочего пространства — та же, что у движка и оболочки."""
    for line in (repo / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped.startswith("version") and "workspace" not in stripped:
            return stripped.split("=", 1)[1].strip().strip('"')
    raise SystemExit("не нашёл version в Cargo.toml")


def xml_escape(text: str) -> str:
    return (
        text.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
    )


def directory_id(rel: str) -> str:
    """Устойчивый идентификатор каталога: msi не любит длинные имена, а хеш от пути всегда один и тот же."""
    return "dir_" + uuid.uuid5(UPGRADE_NAMESPACE, "dir:" + rel).hex[:24]


def component_id(rel: str) -> str:
    return "cmp_" + uuid.uuid5(UPGRADE_NAMESPACE, "file:" + rel).hex[:24]


def component_guid(rel: str) -> str:
    return str(uuid.uuid5(UPGRADE_NAMESPACE, "guid:" + rel)).upper()


def collect(dist: Path) -> dict[str, list[str]]:
    """Разложить файлы дистрибутива по каталогам: относительный каталог -> имена файлов."""
    tree: dict[str, list[str]] = {}
    for path in sorted(dist.rglob("*")):
        if not path.is_file():
            continue
        if path.name in SKIP_NAMES or path.suffix.lower() in SKIP_SUFFIXES:
            continue
        rel = path.relative_to(dist).as_posix()
        parent = str(Path(rel).parent.as_posix())
        parent = "" if parent == "." else parent
        tree.setdefault(parent, []).append(rel)
    return tree


def render_directories(tree: dict[str, list[str]], dist: Path) -> str:
    """Дерево каталогов плюс компоненты: по компоненту на файл, чтобы ключом компонента был сам файл."""
    # Каталог без собственных файлов (`qml/Assets` над `qml/Assets/Downloader`) в `tree` не попадает, но в
    # пакете он нужен: иначе его подкаталоги повисают без родителя. Поэтому берутся все предки.
    every_dir: set[str] = set()
    for rel_dir in tree:
        current = rel_dir
        while current:
            every_dir.add(current)
            parent = str(Path(current).parent.as_posix())
            current = "" if parent == "." else parent
    children: dict[str, list[str]] = {}
    for rel_dir in sorted(every_dir):
        parent = str(Path(rel_dir).parent.as_posix())
        parent = "" if parent == "." else parent
        children.setdefault(parent, []).append(rel_dir)

    def walk(rel_dir: str) -> str:
        lines: list[str] = []
        for file_rel in sorted(tree.get(rel_dir, [])):
            source = (dist / file_rel).as_posix()
            name = Path(file_rel).name
            lines.append(f'          <Component Id="{component_id(file_rel)}" Guid="{{{component_guid(file_rel)}}}">')
            lines.append(
                f'            <File Id="{component_id(file_rel)}_f" Name="{xml_escape(name)}" '
                f'Source="{xml_escape(source)}" />'
            )
            lines.append("          </Component>")
        for child in sorted(children.get(rel_dir, [])):
            lines.append(f'          <Directory Id="{directory_id(child)}" Name="{xml_escape(Path(child).name)}">')
            lines.append(walk(child))
            lines.append("          </Directory>")
        return "\n".join(line for line in lines if line)

    return walk("")


# Контроллер, который связывает драйвер экранов: он же и команда, и файл в пакете.
CONTROLLER_EXE = "dj-controller.exe"


def render_screens_action(tree: dict[str, list[str]]) -> str:
    """Связывание драйвера экранов во время установки — или ничего, если контроллера в дистрибутиве нет.

    Драйвер WinUSB для интерфейса `4` ставится один раз и только из-под администратора; в msi установщик уже
    администратор, поэтому шаг выполняется здесь, а не остаётся командой, которую человек забудет (владелец: экраны
    так и не заработали, потому что `--screens-driver` никто не запускал, 28.11).

    Три атрибута — не украшение: `Execute="deferred"` + `Impersonate="no"` ставят действие в отложенный скрипт от
    SYSTEM (иначе `pnputil` получит отказ в правах), а `Return="ignore"` не даёт отказу драйвера отменить установку
    приложения: программа без экранов всё ещё программа, а установщик, падающий на драйвере, — нет.
    """
    if CONTROLLER_EXE not in tree.get("", []):
        return ""
    return (
        f'    <CustomAction Id="BindScreensDriver" FileKey="{component_id(CONTROLLER_EXE)}_f" '
        f'ExeCommand="--screens-driver --elevated" Execute="deferred" Impersonate="no" Return="ignore" />'
    )


def render_screens_step(tree: dict[str, list[str]]) -> str:
    """Строка последовательности: после того как файлы на месте, и только при первой установке."""
    if CONTROLLER_EXE not in tree.get("", []):
        return ""
    return '      <Custom Action="BindScreensDriver" After="InstallFiles">NOT Installed</Custom>'


def render_reboot_step(tree: dict[str, list[str]]) -> str:
    """Предупредить о перезагрузке после установки — стандартным действием Windows, без своего окна.

    Замена драйвера у **подключённого** устройства WinUSB доводит до конца только после рестарта (так ответила
    `UpdateDriverForPlugAndPlayDevices` на панели владельца, 28.11: привязка принята, `bRebootRequired`). Установщик
    не может знать заранее, попросит ли Windows рестарт, поэтому спрашивает всегда при первой установке —
    `ScheduleReboot` показывает родное окно «Требуется перезагрузка» с выбором «перезагрузить сейчас/позже».
    Кто не хочет — ставит с `REBOOT=ReallySuppress`.
    """
    if CONTROLLER_EXE not in tree.get("", []):
        return ""
    return '      <ScheduleReboot After="InstallFinalize">NOT Installed</ScheduleReboot>'


def render(dist: Path, version: str) -> str:
    tree = collect(dist)
    total = sum(len(files) for files in tree.values())
    if total == 0:
        raise SystemExit(f"в {dist} нет файлов: сначала соберите дистрибутив")
    product_code = str(uuid.uuid5(UPGRADE_NAMESPACE, f"product:{version}")).upper()
    refs = [
        f'      <ComponentRef Id="{component_id(file_rel)}" />'
        for rel_dir in sorted(tree)
        for file_rel in sorted(tree[rel_dir])
    ]
    heat = "\n".join(refs)
    components = render_directories(tree, dist)
    screens_action = render_screens_action(tree)
    screens_step = render_screens_step(tree)
    reboot_step = render_reboot_step(tree)

    return f"""<?xml version="1.0" encoding="utf-8"?>
<!-- Сгенерировано scripts/windows-msi.py: файлов {total}, версия {version}. Руками не править. -->
<Wix xmlns="http://schemas.microsoft.com/wix/2006/wi">
  <Product Id="{{{product_code}}}" Name="{PRODUCT_NAME}" Language="1033" Version="{version}"
           Manufacturer="{MANUFACTURER}" UpgradeCode="{{{UPGRADE_CODE}}}">
    <Package Id="*" Keywords="Installer" Description="{PRODUCT_NAME} {version}"
             Manufacturer="{MANUFACTURER}" InstallerVersion="200" Compressed="yes"
             InstallScope="perMachine" />
    <Media Id="1" Cabinet="hauler-dj.cab" EmbedCab="yes" />

    <!-- Новая версия ставится поверх старой, а не рядом: UpgradeCode у них общий. -->
    <Upgrade Id="{{{UPGRADE_CODE}}}">
      <UpgradeVersion Minimum="0.0.0" Maximum="{version}" Property="HAULER_OLD_VERSION"
                      IncludeMinimum="yes" IncludeMaximum="no" />
    </Upgrade>
    <InstallExecuteSequence>
      <RemoveExistingProducts After="InstallValidate" />
{screens_step}
{reboot_step}
    </InstallExecuteSequence>
{screens_action}

    <Directory Id="TARGETDIR" Name="SourceDir">
      <Directory Id="ProgramFiles64Folder">
        <Directory Id="APPLICATIONFOLDER" Name="{FOLDER_NAME}">
{components}
        </Directory>
      </Directory>

      <Directory Id="ProgramMenuFolder">
        <Directory Id="ApplicationProgramsFolder" Name="{FOLDER_NAME}">
          <Component Id="StartMenuShortcut" Guid="{{{component_guid("shortcut:start-menu")}}}">
            <Shortcut Id="ApplicationStartMenuShortcut" Name="{SHORTCUT_NAME}"
                      Description="{PRODUCT_NAME} {version}"
                      Target="[APPLICATIONFOLDER]hauler-dj.exe" WorkingDirectory="APPLICATIONFOLDER" />
            <RemoveFolder Id="RemoveApplicationProgramsFolder" On="uninstall" />
            <RegistryValue Root="HKCU" Key="Software\\{FOLDER_NAME}" Name="installed"
                           Type="integer" Value="1" KeyPath="yes" />
          </Component>
        </Directory>
      </Directory>
    </Directory>

    <Feature Id="Main" Title="{PRODUCT_NAME}" Level="1" Display="expand">
      <ComponentRef Id="StartMenuShortcut" />
{heat}
    </Feature>
  </Product>
</Wix>
"""


def main() -> int:
    parser = argparse.ArgumentParser(description="Собрать msi из дистрибутива Windows")
    parser.add_argument("--dist", default="dist/windows", help="каталог дистрибутива")
    parser.add_argument("--out", default=None, help="путь к msi (по умолчанию рядом с дистрибутивом)")
    parser.add_argument("--version", default=None, help="версия (по умолчанию из Cargo.toml)")
    args = parser.parse_args()

    repo = Path(__file__).resolve().parent.parent
    dist = Path(args.dist)
    if not dist.is_absolute():
        dist = (repo / dist).resolve()
    if not dist.is_dir():
        raise SystemExit(f"нет каталога {dist}: сначала соберите дистрибутив")

    version = args.version or workspace_version(repo)
    out = Path(args.out) if args.out else dist / f"hauler-dj-{version}-x64.msi"

    build_dir = repo / "build-windows"
    build_dir.mkdir(exist_ok=True)
    wxs = build_dir / "hauler-dj.wxs"
    wxs.write_text(render(dist, version), encoding="utf-8")

    wixl = shutil.which("wixl")
    if not wixl:
        raise SystemExit("нет wixl: он приходит из msitools (см. scripts/windows.sh --msi)")

    files = sum(len(files) for files in collect(dist).values())
    print(f"== msi: {files} файлов, версия {version}")

    # `wixl` opens the output file itself, and when it cannot — a package left by another user, a directory it may not
    # write — it prints a cascade of `db != NULL` assertions and **still exits 0**. The build then said "готово" and
    # left no package behind (owner's reports, 27.11, twice: the second build tried to overwrite the file the first
    # one had made under another account). So the package is written beside the target and moved into place, and what
    # came out is opened and counted before anything is called finished.
    staged = out.with_name(out.name + ".part")
    staged.unlink(missing_ok=True)
    result = subprocess.run([wixl, "-v", "-a", "x64", "-o", str(staged), str(wxs)],
                            capture_output=True, text=True, check=False)
    if result.returncode != 0 or not staged.is_file() or staged.stat().st_size == 0:
        noise = [line for line in (result.stderr or "").splitlines() if "CRITICAL" in line or "WARNING" in line]
        print("wixl не собрал пакет:", file=sys.stderr)
        for line in noise[:6]:
            print(f"  {line}", file=sys.stderr)
        if result.returncode != 0:
            print(f"  код {result.returncode}", file=sys.stderr)
        else:
            print(f"  код 0, но файла нет или он пуст: {staged}", file=sys.stderr)
        staged.unlink(missing_ok=True)
        raise SystemExit(1)
    # The tables are the proof: a database wixl failed to fill answers nothing, or answers fewer rows than the folder
    # holds. Counting here is what turns "wixl printed nothing" into "the package is real".
    msiinfo = shutil.which("msiinfo")
    if msiinfo:
        listing = subprocess.run([msiinfo, "export", str(staged), "File"],
                                 capture_output=True, text=True, check=False)
        rows = sum(1 for line in listing.stdout.splitlines() if line.startswith("cmp_"))
        if listing.returncode != 0 or rows != files:
            print(f"пакет не той полноты: {rows} строк в таблице File, в дистрибутиве {files}",
                  file=sys.stderr)
            staged.unlink(missing_ok=True)
            raise SystemExit(1)
    std_replace = staged.replace(out)
    print(f"готово: {std_replace}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
