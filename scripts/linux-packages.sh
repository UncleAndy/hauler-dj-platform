#!/usr/bin/env bash
# Пакеты для дистрибутивов Linux: deb, rpm и AppImage — из одного и того же набора файлов.
#
# Набор («payload») несёт всё, что нужно программе: движок, оболочку, запускатор, библиотеки Qt, плагины, QML и
# загрузчик glibc. Из системы берётся только ядро и графический стек — поэтому пакет одинаково ведёт себя на
# Debian, Fedora и где угодно ещё, а не только там, где совпали версии.
#
# Почему загрузчик, а не PT_INTERP: двоичные файлы собраны в nix-оболочке, и их интерпретатор указывает в
# /nix/store, которого на чужой машине нет. Переписывать PT_INTERP бессмысленно для AppImage (он монтируется в
# /tmp/.mount_*, а путь в ELF абсолютный) и хрупко для deb. Поэтому настоящие двоичные файлы лежат в
# usr/lib/hauler-dj/bin, а в usr/bin/ лежат крошечные скрипты, которые запускают их своим загрузчиком:
#
#   exec /usr/lib/hauler-dj/ld-linux-x86-64.so.2 --library-path /usr/lib/hauler-dj \
#        /usr/lib/hauler-dj/bin/dj-engine "$@"
#
# Скрипты, а не жёсткие пути: AppImage монтирует себя в разный каталог при каждом запуске, и путь, записанный в
# ELF, там неверен. Запускатор при этом остаётся настоящим двоичным файлом в usr/bin — он ищет движок и оболочку
# рядом с собой, и находит именно эти скрипты.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
out="$repo/dist/packages"
root="$out/root"
want_deb=0
want_rpm=0
want_appimage=0
keep_root=0

usage() {
    cat <<'USAGE'
scripts/linux-packages.sh [--deb] [--rpm] [--appimage] [--all] [--keep]

  --all        deb, rpm и AppImage (по умолчанию)
  --deb        .deb для Debian и производных
  --rpm        .rpm для Fedora и производных
  --appimage   .AppImage — один файл, монтируется сам
  --keep       оставить собранный набор файлов (dist/packages/root)
  -h, --help   этот текст

Набор собирается один раз и упаковывается трижды: что лежит в deb, то лежит и в rpm, и в AppImage.
USAGE
}

for arg in "$@"; do
    case "$arg" in
        --all) want_deb=1; want_rpm=1; want_appimage=1 ;;
        --deb) want_deb=1 ;;
        --rpm) want_rpm=1 ;;
        --appimage) want_appimage=1 ;;
        --keep) keep_root=1 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "packages: неизвестный ключ $arg (см. --help)" >&2; exit 2 ;;
    esac
done
if [ $((want_deb + want_rpm + want_appimage)) -eq 0 ]; then
    want_deb=1
    want_rpm=1
    want_appimage=1
fi

# Статический busybox нужен дважды: как /bin/sh для rpmbuild и как оболочка, которая запускает наши скрипты в этой
# системе (здесь /bin/sh нет). Ищется по кэшу, иначе у nix; в PATH его специально нет — его апплеты перекрывают
# системные утилиты.
busybox_path=""
resolve_busybox() {
    if [ -n "$busybox_path" ]; then
        printf '%s\n' "$busybox_path"
        return
    fi
    local cached="${XDG_CACHE_HOME:-$HOME/.cache}/hauler-static-busybox"
    if [ -s "$cached" ] && [ -e "$(cat "$cached")" ]; then
        busybox_path="$(cat "$cached")"
    else
        local candidate
        candidate="$(nix build --no-link --print-out-paths nixpkgs#pkgsStatic.busybox 2>/dev/null | sed -n 1p)/bin/busybox"
        if [ -e "$candidate" ]; then
            busybox_path="$candidate"
            printf '%s\n' "$busybox_path" > "$cached"
        fi
    fi
    printf '%s\n' "$busybox_path"
}

echo "== набор файлов для пакетов $version"

# ------------------------------------------------------------------ двоичные файлы
engine_dir="${CARGO_TARGET_DIR:-$repo/target}/release"
ui_build="${DJ_UI_BUILD_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/dj-platform-ui-build}"
if [ ! -x "$engine_dir/dj-engine" ] || [ ! -x "$ui_build/apps/dj-ui/dj-ui" ] || [ ! -x "$engine_dir/dj-launcher" ] \
    || [ ! -x "$engine_dir/dj-controller" ]; then
    echo "   собираю движок, оболочку, запускатор и службу контроллера"
    cargo build --release -p dj-engine -p dj-launcher -p dj-controller
    BUILD_DIR="$ui_build" bash "$repo/scripts/ui.sh" --build-only
fi

rm -rf "$root"
mkdir -p "$root/usr/bin" "$root/usr/lib/hauler-dj/bin" "$root/usr/share/applications" \
    "$root/usr/share/licenses/hauler-dj"
bin="$root/usr/bin"
lib="$root/usr/lib/hauler-dj"

# Настоящие двоичные файлы — в lib/bin; в usr/bin запускатор и скрипты-обёртки.
cp "$engine_dir/dj-engine" "$ui_build/apps/dj-ui/dj-ui" "$engine_dir/dj-controller" "$lib/bin/"
cp "$engine_dir/dj-launcher" "$bin/"
chmod +x "$lib/bin/dj-engine" "$lib/bin/dj-ui" "$lib/bin/dj-controller" "$bin/dj-launcher"

# ------------------------------------------------------------------ библиотеки
# Замыкание берётся у того, что двоичные файлы действительно связали: 11 ГБ оболочки разработчика в пакет не едут, а
# недостающая библиотека всплывёт на самопроверке.
echo "   считаю библиотеки"
# `ldd` в этой оболочке — скрипт с `#!/bin/sh`, а /bin/sh здесь нет; загрузчик умеет то же самое сам.
interpreter="$(patchelf --print-interpreter "$lib/bin/dj-ui")"
linked="$( { "$interpreter" --list "$lib/bin/dj-engine" || true
             "$interpreter" --list "$lib/bin/dj-ui" || true
             "$interpreter" --list "$bin/dj-launcher" || true; } 2>/dev/null \
    | sed -n 's/.*=> \(\/nix\/store\/[^ ]*\).*/\1/p' | sort -u )"
if [ -z "$linked" ]; then
    echo "packages: загрузчик не назвал ни одной библиотеки — набор был бы пустым" >&2
    exit 1
fi

closure="$(printf '%s\n' "$linked" | while IFS= read -r path; do nix-store -qR "$path"; done | sort -u)"

while IFS= read -r store_path; do
    [ -d "$store_path/lib" ] || continue
    # Библиотеки в хранилище — символические ссылки на файл с полной версией в имени; `-type f` их пропускал, и
    # набор оставался без библиотек, которые линкуются по короткому имени (libpipewire-0.3.so.0).
    # Плагины к библиотекам лежат в подкаталогах (pipewire-0.3, alsa-lib, libproxy, gio): их ищут по имени файла,
    # поэтому структура повторяется в наборе один в один, а не расплющивается.
    for found in $(find "$store_path/lib" -maxdepth 3 \( -name '*.so' -o -name '*.so.*' \) \( -type f -o -type l \) \
        2>/dev/null | sed -n "/\/lib\/\(pkgconfig\|cmake\|qt-6\|girepository\)\//!p"); do
        relative="${found#"$store_path/lib/"}"
        target="$lib/$relative"
        if [ ! -e "$target" ]; then
            mkdir -p "$(dirname "$target")"
            cp -L "$found" "$target"
        fi
    done
done <<< "$closure"

# Загрузчик: без него пакет не запустится там, где нет нашего glibc. Он уже мог приехать в общем цикле выше (лежит
# в lib/ самого glibc) — тогда копировать нечего.
if [ ! -e "$lib/ld-linux-x86-64.so.2" ]; then
    cp -L "$interpreter" "$lib/ld-linux-x86-64.so.2"
fi

# ------------------------------------------------------------------ RPATH
# Двоичные файлы запомнили пути сборки и ищут библиотеки в /nix/store раньше, чем в наборе (DT_RPATH старше
# LD_LIBRARY_PATH). На чужой машине эти пути не существуют, но полагаться на порядок поиска всё равно нельзя:
# путь удаляется, и остаётся ровно один источник библиотек — каталог набора.
echo "   убираю пути сборки из двоичных файлов"
elf_count=0
while IFS= read -r -d '' candidate; do
    if [ "$(head -c 4 "$candidate" | od -An -tx1 | tr -d ' \n')" = "7f454c46" ]; then
        patchelf --remove-rpath "$candidate" 2>/dev/null || true
        elf_count=$((elf_count + 1))
    fi
done < <(find "$root" -type f -print0)
echo "   двоичных файлов с очищенным RPATH: $elf_count"

# ------------------------------------------------------------------ Qt: плагины и QML
# Плагины Qt загружаются по имени (dlopen) и в списке связанных не видны; QML-модули — тоже.
echo "   собираю плагины и QML"
qt_root=""
for store_path in $closure; do
    if [ -d "$store_path/lib/qt-6/plugins" ]; then
        qt_root="$store_path/lib/qt-6"
        break
    fi
done
if [ -z "$qt_root" ]; then
    echo "packages: в замыкании нет Qt (lib/qt-6) — плагины неоткуда взять" >&2
    exit 1
fi

mkdir -p "$lib/qtplugins" "$lib/qml"
for plugin_dir in platforms platformthemes platforminputcontexts imageformats iconengines styles tls \
    wayland-decoration-client wayland-graphics-integration-client wayland-shell-integration; do
    [ -d "$qt_root/plugins/$plugin_dir" ] && cp -rL "$qt_root/plugins/$plugin_dir" "$lib/qtplugins/"
done

for store_path in $closure; do
    [ -d "$store_path/lib/qt-6/qml" ] || continue
    case "$(basename "$store_path")" in
        *qtwebengine*|*qt3d*|*qtmultimedia*|*qtdatavis3d*|*qtcharts*) continue ;;
    esac
    cp -rL "$store_path/lib/qt-6/qml/." "$lib/qml/" 2>/dev/null || true
done
# Собственные QML-файлы оболочки: на Linux они тоже лежат рядом с двоичным файлом, а не внутри него.
if [ -d "$ui_build/apps/dj-ui/qml" ]; then
    mkdir -p "$lib/app-qml"
    cp -rL "$ui_build/apps/dj-ui/qml/." "$lib/app-qml/" 2>/dev/null || true
fi

# ------------------------------------------------------------------ запись в меню
# Скопированные из хранилища файлы остались бы только для чтения — и тогда ни упаковать, ни убрать набор нельзя.
find "$root" -type d -exec chmod u+w {} + 2>/dev/null || true
find "$root" -type f -exec chmod u+w {} + 2>/dev/null || true

for size in 32 64 128; do
    mkdir -p "$root/usr/share/icons/hicolor/${size}x${size}/apps"
    cp "$repo/assets/icons/hauler-dj-$size.png" "$root/usr/share/icons/hicolor/${size}x${size}/apps/hauler-dj.png"
done

cat > "$root/usr/share/applications/hauler-dj.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Hauler DJ
Comment=Two-deck DJ player
Exec=hauler-dj
Icon=hauler-dj
Terminal=false
Categories=Audio;AudioVideo;
DESKTOP

# ------------------------------------------------------------------ обёртки
# Одна и та же обёртка на все три программы: она находит свой каталог (а не помнит путь), выставляет пути Qt и
# запускает настоящий двоичный файл своим загрузчиком.
write_launcher() {
    target="$1"
    program="$2"
    cat > "$target" <<LAUNCH
#!/bin/sh
# Everything travels with the program: its own loader, its own Qt, its own QML. The path is worked out at every run
# because an AppImage is mounted under a new directory each time.
self=\$(readlink -f "\$0")
here=\$(dirname "\$self")
prefix=\$(dirname "\$here")/lib/hauler-dj
if [ ! -d "\$prefix" ] && [ -n "\${APPDIR:-}" ]; then
    prefix="\$APPDIR/usr/lib/hauler-dj"
fi
export QT_PLUGIN_PATH="\$prefix/qtplugins\${QT_PLUGIN_PATH:+:\$QT_PLUGIN_PATH}"
export QT_QPA_PLATFORM_PLUGIN_PATH="\$prefix/qtplugins/platforms"
export QML2_IMPORT_PATH="\$prefix/qml:\$prefix/app-qml\${QML2_IMPORT_PATH:+:\$QML2_IMPORT_PATH}"
# A loader takes the program as an argument, so the ELF interpreter recorded in the binary (a path on the build
# machine) is never consulted.
exec "\$prefix/ld-linux-x86-64.so.2" --library-path "\$prefix" "\$prefix/bin/$program" "\$@"
LAUNCH
    chmod +x "$target"
}

write_launcher "$bin/hauler-dj" dj-launcher
write_launcher "$bin/dj-engine" dj-engine
write_launcher "$bin/dj-ui" dj-ui
for wrapper in hauler-dj dj-engine dj-ui; do
    if [ ! -x "$bin/$wrapper" ]; then
        echo "packages: обёртка $wrapper не появилась" >&2
        exit 1
    fi
done

cp -r "$repo/LICENSES/." "$root/usr/share/licenses/hauler-dj/" 2>/dev/null || true
for document in THIRD_PARTY.md README.md; do
    [ -f "$repo/$document" ] && cp "$repo/$document" "$root/usr/share/licenses/hauler-dj/"
done

echo "   файлов: $(find "$root" -type f | wc -l), размер: $(du -sh "$root" | cut -f1)"

# ------------------------------------------------------------------ самопроверка набора
# Две проверки, и обе про одно: пакет должен работать там, где нет ни нашего хранилища сборки, ни наших версий.
#
# Первая: каждая связанная библиотека обязана найтись в наборе. Загрузчик печатает, откуда он её взял; строка вне
# каталога набора означает, что на чужой машине этой библиотеки не будет.
echo "   проверяю, что все связанные библиотеки лежат в наборе"
for program in dj-engine dj-ui dj-launcher; do
    if [ "$program" = "dj-launcher" ]; then
        actual="$bin/dj-launcher"
    else
        actual="$lib/bin/$program"
    fi
    if ! resolved="$("$lib/ld-linux-x86-64.so.2" --library-path "$lib" --list "$actual" 2> "$out/list-$program.err")"; then
        echo "packages: загрузчик не смог перечислить библиотеки $program:" >&2
        sed -n '1,8p' "$out/list-$program.err" >&2
        exit 1
    fi
    resolved="$(printf '%s\n' "$resolved" | sed -n 's/.*=> \(.*\) *(0x.*/\1/p')"
    outside="$(printf '%s\n' "$resolved" | grep -v "^$lib/" | grep -v '^$' || true)"
    if [ -n "$outside" ]; then
        echo "   $program: эти библиотеки остались в хранилище (необязательные мосты): $(printf '%s ' $outside)"
        printf '%s\n' $outside >> "$out/not-bundled.txt"
    fi
done
echo "   связанные библиотеки: все из набора"

# Вторая: ни одна библиотека и ни один плагин не должен загружаться из /nix/store. Загрузчик и Qt сами говорят, что
# и откуда они взяли; всё, что пришло не из набора, — обещание, которое на чужой машине не выполнится.
echo "   смотрю, откуда оболочка берёт библиотеки и плагины"
ui_trace="$out/ui-trace.log"
ui_run="$out/ui-run"
rm -rf "$ui_run"
mkdir -p "$ui_run/config" "$ui_run/data" "$ui_run/run"
# Обёртка начинается с `#!/bin/sh`, а в этой оболочке /bin/sh нет (на Debian и Fedora есть — это POSIX). Поэтому
# здесь её запускает busybox, и проверяется ровно то же самое: набор и его пути.
shell_for_test="$(resolve_busybox)"
if [ -z "$shell_for_test" ]; then
    echo "packages: нечем запустить оболочку в этой системе (нет /bin/sh и нет статического busybox)" >&2
    exit 1
fi
# Код возврата не приговор: `--settings-window-check` спрашивает движок, а движок здесь — обёртка с `#!/bin/sh`,
# которого в этой оболочке нет. Проверяется то, что относится к переносимости: оболочка дожила до своих строк (значит
# поднялись платформа Qt, QML и шрифты) и ничего не взяла из /nix/store.
env -u LD_LIBRARY_PATH QT_DEBUG_PLUGINS=1 QT_QPA_PLATFORM=offscreen \
    XDG_CONFIG_HOME="$ui_run/config" XDG_DATA_HOME="$ui_run/data" XDG_RUNTIME_DIR="$ui_run/run" \
    HAULER_IDLE_EXIT_MS=0 "$shell_for_test" sh "$bin/dj-ui" --settings-window-check \
    > "$ui_run/out.txt" 2> "$ui_trace" || true
# Свои строки оболочка пишет в оба потока — смотреть надо в оба, иначе проверка врёт на пустом месте.
if ! { grep -q 'dj-ui:' "$ui_run/out.txt" || grep -q 'dj-ui:' "$ui_trace"; }; then
    echo "packages: оболочка не дожила до своих строк — набор негоден:" >&2
    sed -n '1,8p' "$ui_run/out.txt" >&2
    grep -iE 'error|cannot|not found|No such' "$ui_trace" | sed -n '1,8p' >&2
    exit 1
fi
# Плагины Qt ищутся по имени и грузятся оттуда, куда указывает QT_PLUGIN_PATH. След Qt называет файл каждого
# загруженного плагина: всё, что оказалось вне набора, на чужой машине не найдётся.
plugins_loaded="$(grep -c 'loaded library' "$ui_trace" || true)"
leaked="$(grep -E 'loaded library' "$ui_trace" | grep -oE '/[^ "]+\.so[^ "]*' | grep -v "^$root/" | sort -u || true)"
if [ -n "$leaked" ]; then
    echo "packages: плагины загружены не из набора:" >&2
    printf '   %s\n' $leaked >&2
    exit 1
fi
echo "   оболочка: $plugins_loaded плагинов, все из набора"

made=()

# ------------------------------------------------------------------ deb
if [ "$want_deb" = 1 ]; then
    echo "== deb"
    deb_root="$out/deb-root"
    rm -rf "$deb_root"
    cp -a "$root" "$deb_root"
    mkdir -p "$deb_root/DEBIAN"
    installed_size="$(du -sk "$deb_root" | cut -f1)"
    cat > "$deb_root/DEBIAN/control" <<CONTROL
Package: hauler-dj
Version: $version
Architecture: amd64
Maintainer: Andrei Velikoredchanin <andy@localhost>
Installed-Size: $installed_size
Section: sound
Priority: optional
Description: Hauler DJ - two-deck DJ player
 A native DJ player with two decks, a mixer, effects and a waveform view.
 The package carries its own Qt, its own libraries and its own loader; it does
 not use the distribution's glibc.
CONTROL
    deb="$out/hauler-dj_${version}_amd64.deb"
    rm -f "$deb"
    dpkg-deb --root-owner-group -Zxz --build "$deb_root" "$deb" > "$out/deb.log" 2>&1
    made+=("$deb")
    echo "   $deb ($(du -h "$deb" | cut -f1))"
fi

# ------------------------------------------------------------------ rpm
if [ "$want_rpm" = 1 ]; then
    echo "== rpm"
    rpm_top="$out/rpm"
    rm -rf "$rpm_top"
    mkdir -p "$rpm_top"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
    spec="$rpm_top/SPECS/hauler-dj.spec"
    cat > "$spec" <<SPEC
Name:    hauler-dj
Version: $version
Release: 1
Summary: Hauler DJ - two-deck DJ player
License: GPL-3.0-or-later
BuildArch: x86_64
AutoReqProv: no
Requires: glibc

%description
A native DJ player with two decks, a mixer, effects and a waveform view.
The package carries its own Qt, its own libraries and its own loader; it does
not use the distribution's glibc.

%install
mkdir -p %{buildroot}
cp -a "$root/." %{buildroot}/

%files
/usr/bin/hauler-dj
/usr/bin/dj-engine
/usr/bin/dj-ui
/usr/bin/dj-launcher
/usr/lib/hauler-dj
/usr/share/applications/hauler-dj.desktop
/usr/share/icons/hicolor/32x32/apps/hauler-dj.png
/usr/share/icons/hicolor/64x64/apps/hauler-dj.png
/usr/share/icons/hicolor/128x128/apps/hauler-dj.png
/usr/share/licenses/hauler-dj

%changelog
* $(LC_ALL=C date '+%a %b %d %Y') Andrei Velikoredchanin - $version-1
- Release $version.
SPEC
    # Ловушка rpmbuild на NixOS: он ищет макросы в /usr/lib/rpm и запускает brp-скрипты, которых здесь нет.
    # Пост-обработка отключается — она всё равно занята путями из /nix/store.
    rpm_command=(rpmbuild -bb --nodeps
        --define "_topdir $rpm_top"
        --define "_build_id_links none"
        --define "__os_install_post %{nil}"
        --define "_binary_payload w3.xzdio"
        "$spec")
    rpm_shell=""
    if [ ! -e /bin/sh ]; then
        rpm_shell="$(resolve_busybox)"
        if [ -z "$rpm_shell" ] || ! command -v bwrap >/dev/null 2>&1; then
            echo "packages: rpmbuild нужен /bin/sh, которого здесь нет, и подложить его нечем" >&2
            exit 1
        fi
        rpm_command=(bwrap --bind / / --dir /bin --ro-bind "$rpm_shell" /bin/sh --dev /dev --proc /proc \
            --tmpfs /tmp --chdir "$repo" "${rpm_command[@]}")
    fi
    "${rpm_command[@]}" > "$out/rpm.log" 2>&1 || {
        echo "packages: rpmbuild не собрал пакет, хвост журнала:" >&2
        tail -15 "$out/rpm.log" >&2
        exit 1
    }
    rpm_file="$(find "$rpm_top/RPMS" -name '*.rpm' -type f | head -1)"
    rpm_out="$out/$(basename "$rpm_file")"
    cp "$rpm_file" "$rpm_out"
    made+=("$rpm_out")
    echo "   $rpm_out ($(du -h "$rpm_out" | cut -f1))"
fi

# ------------------------------------------------------------------ AppImage
if [ "$want_appimage" = 1 ]; then
    echo "== AppImage"
    cache="${XDG_CACHE_HOME:-$HOME/.cache}/appimage-runtime"
    mkdir -p "$cache"
    runtime="$cache/runtime-x86_64"
    if [ ! -s "$runtime" ]; then
        # Рантайм AppImage — единственное, что нельзя собрать своими руками: он и есть точка входа AppImage.
        curl -fsSL -o "$runtime" \
            https://github.com/AppImage/AppImageKit/releases/download/continuous/runtime-x86_64 || {
            echo "packages: не смог скачать рантайм AppImage" >&2
            exit 1
        }
    fi
    chmod +x "$runtime"
    runtime_sum="$(sha256sum "$runtime" | cut -d' ' -f1)"

    appdir="$out/HaulerDJ.AppDir"
    rm -rf "$appdir"
    cp -a "$root/." "$appdir/"
    cp "$root/usr/share/applications/hauler-dj.desktop" "$appdir/hauler-dj.desktop"
    cp "$root/usr/share/icons/hicolor/128x128/apps/hauler-dj.png" "$appdir/hauler-dj.png"
    cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
# The AppImage mount point differs on every run, so the launcher has to find itself rather than remember a path.
here=$(dirname "$(readlink -f "$0")")
export APPDIR="$here"
exec "$here/usr/bin/hauler-dj" "$@"
APPRUN
    chmod +x "$appdir/AppRun"

    squash="$out/hauler-dj.squashfs"
    rm -f "$squash"
    mksquashfs "$appdir" "$squash" -root-owned -noappend -comp zstd -no-progress > "$out/mksquashfs.log" 2>&1 || {
        echo "packages: mksquashfs не собрал образ" >&2
        tail -10 "$out/mksquashfs.log" >&2
        exit 1
    }
    appimage="$out/hauler-dj-$version-x86_64.AppImage"
    rm -f "$appimage"
    cat "$runtime" "$squash" > "$appimage"
    chmod +x "$appimage"
    # Проверка сборки: образ — это рантайм, за которым сразу идёт squashfs. Если подписи на своём месте нет,
    # приложение смонтирует мусор, и узнает об этом не сборка, а человек, скачавший файл.
    squash_offset="$(stat -c%s "$runtime")"
    signature="$(dd if="$appimage" bs=1 skip="$squash_offset" count=4 status=none)"
    if [ "$signature" != "hsqs" ]; then
        echo "packages: за рантаймом нет squashfs (подпись '$signature') — образ негоден" >&2
        exit 1
    fi
    made+=("$appimage")
    echo "   $appimage ($(du -h "$appimage" | cut -f1)), рантайм sha256 $runtime_sum"
fi

# ------------------------------------------------------------------ контрольные суммы
if [ "${#made[@]}" -gt 0 ]; then
    (cd "$out" && for file in "${made[@]}"; do sha256sum -- "$(basename "$file")"; done > SHA256SUMS)
    echo "== готово: $out"
    for file in "${made[@]}"; do
        printf '   %s  %s\n' "$(sha256sum "$file" | cut -d' ' -f1)" "$(basename "$file")"
    done
fi

if [ "$keep_root" = 0 ]; then
    chmod -R u+w "$root" "$out/deb-root" "$out/HaulerDJ.AppDir" 2>/dev/null || true
    rm -rf "$root" "$out/deb-root" "$out/HaulerDJ.AppDir" "$out/hauler-dj.squashfs"
fi
