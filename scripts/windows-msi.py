#!/usr/bin/env python3
"""Собрать msi-пакет Windows из готового дистрибутива.

Пакет собирается на Linux: `wixl` из msitools читает WiX-описание и пишет msi. Описание генерируется здесь,
потому что файлов в дистрибутиве больше полутора тысяч и руками их список не поддерживать.

Что попадает в пакет:

  * `Program Files\\Hauler DJ` — всё содержимое дистрибутива, кроме самого zip: он artefact сборки, а не
    часть установки;
  * ярлык в меню «Пуск» на `hauler-dj.bat` — он поднимает движок и оболочку в правильном порядке;
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
    </InstallExecuteSequence>

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
                      Target="[APPLICATIONFOLDER]hauler-dj.bat" WorkingDirectory="APPLICATIONFOLDER" />
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
    subprocess.run([wixl, "-v", "-a", "x64", "-o", str(out), str(wxs)], check=True)
    print(f"готово: {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
