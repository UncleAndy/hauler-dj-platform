#!/usr/bin/env bash
# MSVC runtime for the Windows distribution: the four DLLs ONNX Runtime needs beside it.
#
# `onnxruntime.dll` is built with MSVC, so it needs the Visual C++ runtime even though everything else in the package is
# mingw. Microsoft allows these files to travel app-locally — that is what the redistributable licence grants — and the
# only official source without an installer is the redistributable bundle itself: a Burn-style exe whose payload is a
# cabinet of cabinets. This command takes that apart and leaves the x64 files in the distribution, so "the release has
# no runtime" is not something a user discovers on his own machine.
#
#   scripts/windows-msvc-runtime.sh [--dist dist/windows] [--cache <dir>] [--check]
#
# Version and hash are pinned: a bundle that changed has to be looked at by a person, because a runtime that silently
# became another version is a support case nobody can reproduce.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dist="$repo/dist/windows"
cache="${XDG_CACHE_HOME:-$HOME/.cache}/hauler-msvc"
check_only=0

url="https://aka.ms/vs/17/release/vc_redist.x64.exe"
# 14.44.35211.0, the x64 bundle served on 2025-06-11.
pinned_sha="cc0ff0eb1dc3f5188ae6300faef32bf5beeba4bdd6e8e445a9184072096b713b"
dlls="MSVCP140.dll MSVCP140_1.dll VCRUNTIME140.dll VCRUNTIME140_1.dll"

while [ $# -gt 0 ]; do
    case "$1" in
        --dist) dist="$2"; shift 2 ;;
        --cache) cache="$2"; shift 2 ;;
        --check) check_only=1; shift ;;
        -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
        *) echo "неизвестный аргумент: $1" >&2; exit 2 ;;
    esac
done

missing=""
for name in $dlls; do
    [ -f "$dist/$name" ] || missing="$missing $name"
done
if [ -z "$missing" ]; then
    echo "MSVC runtime уже в $dist"
    exit 0
fi
if [ "$check_only" -eq 1 ]; then
    echo "в $dist нет:$missing" >&2
    exit 1
fi

mkdir -p "$cache" "$dist"
bundle="$cache/vc_redist.x64.exe"
if [ ! -f "$bundle" ]; then
    echo "== качаю $url"
    curl -fsSL -o "$bundle.part" "$url"
    mv "$bundle.part" "$bundle"
fi
actual_sha="$(sha256sum "$bundle" | cut -d' ' -f1)"
if [ "$actual_sha" != "$pinned_sha" ]; then
    echo "внимание: у скачанного файла другой хеш" >&2
    echo "  ждали:   $pinned_sha" >&2
    echo "  получили: $actual_sha" >&2
    echo "  Microsoft выпустил новый redistributable: посмотрите версию и обновите пин в этом скрипте" >&2
    exit 1
fi

echo "== разбираю bundle"
work="$cache/unpack"
rm -rf "$work"
mkdir -p "$work"

# The payload cabinet is found by its signature, not by an offset: offsets move with every build, signatures do not.
# The first cabinet in the file is the installer's own manifest; the last one carries the files.
offset="$(grep -abo MSCF "$bundle" | tail -1 | cut -d: -f1)"
dd if="$bundle" of="$work/payload.cab" bs=1 skip="$offset" status=none
nix shell nixpkgs#p7zip --command bash -c "
    set -e
    cd '$work'
    7z x -y -opayload payload.cab >/dev/null
    cd payload
    for member in a*; do
        if 7z l \"\$member\" 2>/dev/null | grep -qi 'msvcp140.dll_amd64'; then
            7z x -y -o../x64 \"\$member\" >/dev/null
            echo \"== x64 лежит в \$member\"
            break
        fi
    done
"

for pair in "MSVCP140.dll:msvcp140.dll_amd64" "MSVCP140_1.dll:msvcp140_1.dll_amd64" \
            "VCRUNTIME140.dll:vcruntime140.dll_amd64" "VCRUNTIME140_1.dll:vcruntime140_1.dll_amd64"; do
    target="${pair%%:*}"
    source="${pair##*:}"
    if [ ! -f "$work/x64/$source" ]; then
        echo "в bundle нет $source — разбор изменился, посмотрите глазами" >&2
        exit 1
    fi
    cp "$work/x64/$source" "$dist/$target"
    # An x86 file under an x64 name links with nothing and fails at start: the machine field is read, not trusted.
    # Offset of the PE header, then the machine word two bytes into it; 64 86 is 0x8664, x86-64, little-endian.
    pe_offset="$(od -An -tu4 -j 60 -N4 "$dist/$target" | tr -d ' ')"
    machine="$(od -An -tx1 -j $((pe_offset + 4)) -N2 "$dist/$target" | tr -d ' ')"
    if [ "$machine" != "6486" ]; then
        echo "$target не x86-64 ($machine) — в bundle другой расклад" >&2
        exit 1
    fi
done

mkdir -p "$dist/LICENSES"
cat >"$dist/LICENSES/msvc-runtime.txt" <<TXT
Microsoft Visual C++ Redistributable runtime files
=================================================

Files: MSVCP140.dll, MSVCP140_1.dll, VCRUNTIME140.dll, VCRUNTIME140_1.dll
Version: 14.44.35211.0 (2025-06-11)
Source: $url
sha256 of the bundle: $pinned_sha

These four files are Microsoft's redistributable runtime, taken from the official Visual C++ Redistributable bundle and
placed beside ours ("app-local deployment"), which is the use the redistributable licence grants. They are not modified.

They are here because onnxruntime.dll is built with MSVC while this package is built with mingw: without them the runtime
cannot be loaded, and the engine silently falls back to its own beat detector.
TXT

echo "готово: четыре файла в $dist и LICENSES/msvc-runtime.txt"
