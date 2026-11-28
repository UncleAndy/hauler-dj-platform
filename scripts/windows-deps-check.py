#!/usr/bin/env python3
"""Проверить, что дистрибутив Windows самодостаточен по DLL.

Windows ищет DLL рядом с exe, и «не обнаружена такая-то dll» — типичная ошибка первого запуска. Скрипт читает
таблицы импорта каждого PE-файла дистрибутива (objdump из mingw), собирает транзитивное замыкание зависимостей и
требует, чтобы каждая несистемная библиотека лежала в дистрибутиве.

Системные библиотеки Windows сюда не входят: они есть на любой машине. Остальное — наша забота.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
from pathlib import Path

# Библиотеки самой Windows: их поставляет система, в пакет они не входят.
SYSTEM_DLLS = {
    "advapi32.dll", "authz.dll", "bcrypt.dll", "bcryptprimitives.dll", "cfgmgr32.dll",
    "combase.dll", "comctl32.dll", "comdlg32.dll", "credui.dll", "crypt32.dll", "d3d9.dll",
    "d3d11.dll", "d3d12.dll", "dwmapi.dll", "dxgi.dll", "gdi32.dll", "gdi32full.dll", "hid.dll",
    "imm32.dll", "iphlpapi.dll", "kernel32.dll", "kernelbase.dll", "mpr.dll", "msvcrt.dll",
    "netapi32.dll", "ntdll.dll", "ole32.dll", "oleaut32.dll", "powrprof.dll", "propsys.dll",
    "psapi.dll", "rpcrt4.dll", "secur32.dll", "sechost.dll", "setupapi.dll", "shcore.dll",
    "shell32.dll", "shlwapi.dll", "user32.dll", "userenv.dll", "usp10.dll", "uxtheme.dll",
    "version.dll", "winhttp.dll", "wininet.dll", "winmm.dll", "winspool.drv", "ws2_32.dll",
    "mmdevapi.dll", "ncrypt.dll", "cryptbase.dll", "ksuser.dll", "dsound.dll", "wdmaud.drv",
    "wsock32.dll", "wtsapi32.dll", "dnsapi.dll", "dxcore.dll", "opengl32.dll", "glu32.dll",
    "avrt.dll", "mfplat.dll", "mf.dll", "mfreadwrite.dll", "dwrite.dll", "dcomp.dll",
    "d2d1.dll", "d3dcompiler_47.dll", "textshaping.dll", "wldap32.dll", "bluetoothapis.dll",
    # WinUSB: the user-mode half of the driver the controller's bulk endpoints (the screens) go through on Windows. It is
    # part of Windows — `dj-controller.exe` imports it through `nusb`, and without this line the packaging refused a
    # distribution for a library that is already on every machine (owner's build report, 25.11).
    "winusb.dll",
}

SYSTEM_PREFIXES = ("api-ms-win-", "ext-ms-", "ucrtbase")

IMPORT_RE = re.compile(r"DLL Name:\s*(\S+)", re.IGNORECASE)


def imports_of(objdump: str, path: Path) -> set[str]:
    result = subprocess.run([objdump, "-p", str(path)], capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise SystemExit(f"objdump не прочитал {path}: {result.stderr.strip()[:200]}")
    return {name.lower() for name in IMPORT_RE.findall(result.stdout)}


def is_system(name: str) -> bool:
    return name in SYSTEM_DLLS or name.startswith(SYSTEM_PREFIXES)


def main() -> int:
    parser = argparse.ArgumentParser(description="Проверить DLL-зависимости дистрибутива Windows")
    parser.add_argument("--dist", default="dist/windows")
    parser.add_argument("--objdump", default=None, help="objdump (по умолчанию из PATH)")
    parser.add_argument("--only-missing", action="store_true",
                        help="напечатать только имена недостающих библиотек (для сборочного цикла)")
    args = parser.parse_args()

    repo = Path(__file__).resolve().parent.parent
    dist = Path(args.dist)
    if not dist.is_absolute():
        dist = (repo / dist).resolve()
    if not dist.is_dir():
        raise SystemExit(f"нет каталога {dist}")

    objdump = args.objdump or shutil.which("x86_64-w64-mingw32-objdump") or shutil.which("objdump")
    if not objdump:
        raise SystemExit("нет objdump: он приходит из mingw (binutils)")

    present = {path.name.lower(): path for path in dist.rglob("*") if path.is_file()}
    pe_files = [p for p in dist.rglob("*") if p.is_file() and p.suffix.lower() in {".exe", ".dll"}]

    missing: dict[str, set[str]] = {}
    checked: set[str] = set()
    queue = [p for p in pe_files]
    while queue:
        current = queue.pop()
        key = current.name.lower()
        if key in checked:
            continue
        checked.add(key)
        for name in imports_of(objdump, current):
            if is_system(name):
                continue
            target = present.get(name)
            if target is None:
                missing.setdefault(name, set()).add(current.relative_to(dist).as_posix())
                continue
            queue.append(target)

    # Библиотеки, которых в пакете может не быть по делу: MSVC runtime ставится Redistributable-пакетом (Microsoft
    # разрешает и app-local, тогда их приносит `--ort` с ORT_CRT_DIR), а dbghelp.dll есть в любой Windows.
    allowed_missing = {"dbghelp.dll", "msvcp140.dll", "msvcp140_1.dll", "vcruntime140.dll",
                       "vcruntime140_1.dll"}
    # Что из этого действительно пропало — запоминаем, но объяснение печатаем только в отчёте для человека: в
    # режиме `--only-missing` этот поток читает цикл сборки, и любая фраза в нём принимается за имя библиотеки
    # (сборка печатала «не нашлось: DJ», «не нашлось: Windows»).
    explained = [name for name in sorted(allowed_missing) if name in missing]
    for name in explained:
        missing.pop(name, None)

    if args.only_missing:
        for name in sorted(missing):
            print(name)
        return 1 if missing else 0

    for name in explained:
        print(f"DJ Windows deps: {name} — приходит из Visual C++ Redistributable или из Windows")

    exes = sorted(p.name for p in pe_files if p.suffix.lower() == ".exe")
    if missing:
        print("не хватает библиотек в дистрибутиве:", file=sys.stderr)
        for name in sorted(missing):
            users = ", ".join(sorted(missing[name])[:4])
            print(f"  {name} — нужна для {users}", file=sys.stderr)
        return 1

    print(f"проверено: {len(exes)} exe ({', '.join(exes)}), {len(checked)} библиотек, зависимостей нет вне дистрибутива")
    return 0


if __name__ == "__main__":
    sys.exit(main())
