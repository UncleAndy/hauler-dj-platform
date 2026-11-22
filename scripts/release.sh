#!/usr/bin/env bash
# Сборка релиза: пакеты для Linux и Windows, заметки к выпуску, черновик релиза на GitHub.
#
#   --linux    движок, оболочка и запускатор этой машины; tar.gz с .desktop, иконками и лицензиями;
#   --windows  scripts/windows.sh --all (zip и MSI, собранные mingw-цепочкой);
#   --notes    docs/release-notes-<версия>.md — из шаблона, если файла ещё нет;
#   --github   черновик релиза: gh release create --draft с файлами и контрольными суммами.
#
# Черновик, а не публикация: тег появляется, когда владелец нажмёт Publish. Скрипт не делает ни git push, ни git tag.
#
# Linux-артефакт собирается в этой оболочке, а значит несёт nix-овский интерпретатор ELF: он запускается на NixOS и не
# запускается на обычном дистрибутиве. Это не скрывается: скрипт читает интерпретатор и при /nix/store называет файл
# `-nixos-` и говорит об этом в заметках. Для других дистрибутивов нужна сборка в их контейнере.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

version=""
want_linux=0
want_windows=0
want_notes=0
want_github=0
dry_run=0

usage() {
    cat <<'USAGE'
scripts/release.sh [шаги] [--version X.Y.Z] [--dry-run]

  --all        всё: --linux --windows --notes --github (по умолчанию)
  --linux      пакеты для Linux: tar.gz, а также deb, rpm и AppImage
  --windows    пакет для Windows: zip и MSI через scripts/windows.sh --all
  --notes      создать docs/release-notes-<версия>.md из шаблона, если его нет
  --github     черновик релиза на GitHub (gh release create --draft)
  --version    версия; по умолчанию из Cargo.toml, и тогда обязана с ним совпасть
  --dry-run    собрать пакеты и суммы, но не отправлять на GitHub
  -h, --help   этот текст
USAGE
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --all) want_linux=1; want_windows=1; want_notes=1; want_github=1 ;;
        --linux) want_linux=1 ;;
        --windows) want_windows=1 ;;
        --notes) want_notes=1 ;;
        --github) want_github=1 ;;
        --dry-run) dry_run=1 ;;
        --version)
            if [ "$#" -lt 2 ]; then
                echo "release: --version без значения" >&2
                exit 2
            fi
            version="$2"
            shift
            ;;
        -h|--help) usage; exit 0 ;;
        *) echo "release: неизвестный ключ $1 (см. --help)" >&2; exit 2 ;;
    esac
    shift
done

if [ $((want_linux + want_windows + want_notes + want_github)) -eq 0 ]; then
    want_linux=1
    want_windows=1
    want_notes=1
    want_github=1
fi

cargo_version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
if [ -z "$version" ]; then
    version="$cargo_version"
fi
if [ "$version" != "$cargo_version" ]; then
    {
        echo "release: версия $version не совпадает с Cargo.toml ($cargo_version)."
        echo "         Тег с чужим номером увёз бы файлы, подписанные неправильной версией."
        echo "         Поднимите версию в Cargo.toml — или не указывайте --version."
    } >&2
    exit 2
fi

out="$repo/dist/release"
stem="hauler-dj-$version"
mkdir -p "$out"
echo "== релиз $version"

flavour="linux"
linux_artefact=""
linux_extra=()

build_linux() {
    echo "== пакет для этой системы"
    cargo build --release -p dj-engine -p dj-launcher -p dj-controller
    BUILD_DIR="${DJ_UI_BUILD_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/dj-platform-ui-build}" \
        bash "$repo/scripts/ui.sh" --build-only

    local engine_dir="${CARGO_TARGET_DIR:-$repo/target}/release"
    local ui_build="${DJ_UI_BUILD_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/dj-platform-ui-build}"
    local staging="$out/$stem-linux-x86_64"
    rm -rf "$staging"
    mkdir -p "$staging" "$staging/LICENSES"

    cp "$engine_dir/dj-engine" "$staging/"
    cp "$engine_dir/dj-launcher" "$staging/"
    cp "$engine_dir/dj-controller" "$staging/"
    cp "$ui_build/apps/dj-ui/dj-ui" "$staging/"

    # Запись в меню и иконки: без них «пакет» — это папка с бинарниками.
    local size
    for size in 32 64 128; do
        mkdir -p "$staging/share/icons/hicolor/${size}x${size}/apps"
        cp "$repo/assets/icons/hauler-dj-$size.png" "$staging/share/icons/hicolor/${size}x${size}/apps/hauler-dj.png"
    done
    mkdir -p "$staging/share/applications"
    cat > "$staging/share/applications/hauler-dj.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Hauler DJ
Comment=Two-deck DJ player
Exec=hauler-dj
Icon=hauler-dj
Terminal=false
Categories=Audio;AudioVideo;
DESKTOP

    cat > "$staging/hauler-dj" <<'LAUNCH'
#!/bin/sh
# Everything the launcher needs sits beside it; the launcher starts and stops the engine.
here=$(dirname "$(readlink -f "$0")")
exec "$here/dj-launcher" "$@"
LAUNCH
    chmod +x "$staging/hauler-dj"

    cp -r "$repo/LICENSES/." "$staging/LICENSES/" 2>/dev/null || true
    for document in THIRD_PARTY.md README.md; do
        [ -f "$repo/$document" ] && cp "$repo/$document" "$staging/"
    done

    # Интерпретатор ELF решает, честное ли имя у файла. patchelf читает его напрямую; readelf в этой оболочке —
    # обёртка clang, и запускать её отсюда не получается (как и обёртку gzip). Если не прочитал никто, честное имя —
    # `unknown`, а не «linux»: артефакт, о котором неизвестно, где он запустится, нельзя называть переносимым.
    local interpreter=""
    interpreter="$(patchelf --print-interpreter "$staging/dj-ui" 2>/dev/null || true)"
    if [ -z "$interpreter" ]; then
        interpreter="$(file -b "$staging/dj-ui" 2>/dev/null | sed -n 's/.*interpreter \([^,]*\),.*/\1/p' | head -1)"
    fi
    case "$interpreter" in
        /nix/store/*) flavour="nixos" ;;
        "") flavour="unknown" ;;
        *) flavour="linux" ;;
    esac

    cat > "$staging/INSTALL-Linux.txt" <<EOF
Hauler DJ $version — сборка для Linux.

Запуск: ./hauler-dj

Интерпретатор ELF у dj-ui: ${interpreter:-НЕ ПРОЧИТАН}.

Если он ведёт в /nix/store — сборка сделана на NixOS и на обычном дистрибутиве не запустится: Nix
переписывает интерпретатор ELF на путь в хранилище. Такой архив годится для NixOS; для другого
дистрибутива пакет надо собрать в его контейнере.

Нужны Qt 6.8+ (Core, Gui, Widgets, Quick, QuickControls2, Network) и PipeWire либо ALSA.
На NixOS короче: nix run github:UncleAndy/hauler-dj-platform
EOF

    if [ "$flavour" != "linux" ]; then
        local named="$out/$stem-$flavour-x86_64"
        rm -rf "$named"
        mv "$staging" "$named"
        staging="$named"
    fi

    linux_artefact="$out/$stem-$flavour-x86_64.tar.gz"
    rm -f "$linux_artefact"
    # Клубок `tar`+`gzip` через трубу, а не `tar -z`: в этой оболочке tar не может запустить обёртку gzip из PATH
    # («Нет такого файла или каталога»), а сама оболочка её запускает. Архив получается тот же.
    tar -cf - -C "$out" "$(basename "$staging")" | gzip > "$linux_artefact"
    (cd "$out" && sha256sum -- "$(basename "$linux_artefact")" > "$(basename "$linux_artefact").sha256")
    echo "   $linux_artefact"
    echo "   ${interpreter:-интерпретатор не прочитан}"

    # Пакеты для дистрибутивов: deb, rpm и AppImage из одного набора. Каждый несёт свои библиотеки, поэтому работает
    # не только там, где совпали версии.
    if [ "${HAULER_SKIP_DISTRO_PACKAGES:-0}" != "1" ]; then
        echo "   пакеты для дистрибутивов"
        if ! bash "$repo/scripts/linux-packages.sh" --all > "$out/packages.log" 2>&1; then
            echo "release: пакеты для дистрибутивов не собрались, хвост журнала:" >&2
            tail -12 "$out/packages.log" >&2
            exit 1
        fi
        for package in "$repo/dist/packages/hauler-dj_${version}_amd64.deb" \
            "$repo/dist/packages/hauler-dj-${version}-1.x86_64.rpm" \
            "$repo/dist/packages/hauler-dj-${version}-x86_64.AppImage"; do
            if [ -f "$package" ]; then
                linux_extra+=("$package")
            fi
        done
        echo "   пакетов для дистрибутивов: ${#linux_extra[@]}"
    fi
}

windows_artefacts=()

build_windows() {
    echo "== пакет для Windows"
    # Каталоги windows-сборки могут принадлежать другому пользователю — тогда их не переписать, и спорить за них
    # нечего: собираем в свои, а где лежит готовое, говорим вслух.
    # Признак чужих файлов — не имя файла, а неwritable: после чужой сборки в каталоге остаётся горстка файлов, к
    # которым нет доступа, и cmake падает на первом же configure_file.
    local stuck
    if [ -d "$repo/build-windows" ]; then
        stuck="$(find "$repo/build-windows" ! -writable -print -quit 2>/dev/null || true)"
        if [ -n "$stuck" ]; then
            export DJ_WIN_BUILD_DIR="${DJ_WIN_BUILD_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/dj-platform-win-build}"
            echo "   в build-windows есть чужие файлы ($stuck) — оболочку собираю в $DJ_WIN_BUILD_DIR"
        fi
    fi
    stuck="$(find "$repo/dist/windows" ! -writable -print -quit 2>/dev/null || true)"
    if [ -n "$stuck" ]; then
        export DJ_WIN_DIST="${DJ_WIN_DIST:-${XDG_CACHE_HOME:-$HOME/.cache}/dj-platform-win-dist}"
        echo "   в dist/windows есть чужие файлы ($stuck) — пакет собираю в $DJ_WIN_DIST"
    fi
    local win_dist="${DJ_WIN_DIST:-$repo/dist/windows}"

    bash "$repo/scripts/windows.sh" --all
    while IFS= read -r file; do
        windows_artefacts+=("$file")
    done < <(find "$win_dist" -maxdepth 1 -type f \( -name '*.zip' -o -name '*.msi' \) | sort)
    if [ "${#windows_artefacts[@]}" -eq 0 ]; then
        echo "release: windows.sh не оставил ни zip, ни MSI" >&2
        exit 1
    fi
    local file
    for file in "${windows_artefacts[@]}"; do
        echo "   $file"
    done
}

package_kind() {
    case "$1" in
        *.deb) echo "Debian, Ubuntu и родня" ;;
        *.rpm) echo "Fedora, openSUSE и родня" ;;
        *.AppImage) echo "один файл, любой дистрибутив" ;;
        *) echo "Linux x86_64" ;;
    esac
}

notes_file="$repo/docs/release-notes-$version.md"

write_notes() {
    if [ -f "$notes_file" ]; then
        echo "== заметки: $notes_file уже есть, не трогаю"
        return
    fi
    echo "== заметки: создаю $notes_file из шаблона"
    {
        echo "# Hauler DJ $version"
        echo
        echo "## What is in it"
        echo
        echo "- (fill in: what the DJ can now do — plain sentences, not a list of commits)"
        echo
        echo "## Files"
        echo
        if [ -n "$linux_artefact" ]; then
            if [ "$flavour" = "nixos" ]; then
                echo "- \`$(basename "$linux_artefact")\` — Linux x86_64, NixOS (the ELF interpreter is in /nix/store)"
            else
                echo "- \`$(basename "$linux_artefact")\` — Linux x86_64"
            fi
        fi
        local file
        for file in "${linux_extra[@]:-}"; do
            [ -n "$file" ] && echo "- \`$(basename "$file")\` — $(package_kind "$file")"
        done
        for file in "${windows_artefacts[@]:-}"; do
            [ -n "$file" ] && echo "- \`$(basename "$file")\` — Windows x86_64"
        done
        echo "- \`SHA256SUMS\` — checksums of every file"
        echo
        echo "## Known limits"
        echo
        echo "- The Windows build is unsigned: SmartScreen shows \"More info → Run anyway\"."
        echo "- (anything not verified on a live system belongs here)"
        echo
        echo "## Verified"
        echo
        echo "- \`bash scripts/check.sh\` — all gates."
    } > "$notes_file"
}

publish() {
    if [ ! -f "$notes_file" ]; then
        echo "release: нет файла заметок $notes_file — черновик без заметок не создаю" >&2
        exit 1
    fi
    # Суммы считаются по каталогу выгрузки уже после того, как все файлы на месте: сумма, посчитанная раньше, о новом
    # файле молчит, и проверка по ней проходит, ничего не проверив.
    local upload="$out/upload"
    rm -rf "$upload"
    mkdir -p "$upload"
    [ -n "$linux_artefact" ] && cp "$linux_artefact" "$linux_artefact.sha256" "$upload/"
    for package in "${linux_extra[@]:-}"; do
        [ -n "$package" ] && cp "$package" "$upload/"
    done
    local file
    for file in "${windows_artefacts[@]:-}"; do
        [ -n "$file" ] && cp "$file" "$upload/"
    done
    cp "$notes_file" "$upload/"
    (cd "$upload" && sha256sum -- * > SHA256SUMS)

    local gh_command=(
        gh release create "v$version"
        --draft
        --title "Hauler DJ $version"
        --notes-file "$notes_file"
        --target "$(git rev-parse --abbrev-ref HEAD)"
    )
    for file in "$upload"/*; do
        gh_command+=("$file")
    done

    if [ "$dry_run" = 1 ]; then
        echo "== черновик не отправлен (--dry-run). Команда была бы:"
        printf '   %q' "${gh_command[0]}"
        printf ' %q' "${gh_command[@]:1}"
        echo
        echo "   файлы: $(cd "$upload" && ls | tr '\n' ' ')"
        return
    fi
    if ! command -v gh >/dev/null 2>&1; then
        echo "release: gh не найден. Поставьте его (nix shell nixpkgs#gh) или запустите с --dry-run." >&2
        exit 2
    fi
    if ! gh auth status >/dev/null 2>&1; then
        echo "release: gh не авторизован (gh auth login). Без входа черновик создать некому." >&2
        exit 2
    fi
    echo "== черновик релиза v$version"
    "${gh_command[@]}"
    echo "   черновик создан; тег появится, когда вы нажмёте Publish"
}

[ "$want_linux" = 1 ] && build_linux
[ "$want_windows" = 1 ] && build_windows
[ "$want_notes" = 1 ] && write_notes
[ "$want_github" = 1 ] && publish

echo "== готово: $out"
ls -la "$out"
