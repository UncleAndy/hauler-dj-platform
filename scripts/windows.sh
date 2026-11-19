#!/usr/bin/env bash
# Сборка Windows-версии: движок и оболочка, готовые exe и папка дистрибутива.
#
# Хост — Linux (NixOS). Всё нужное берётся из nix и из официальных сборок Qt:
#   * движок — Rust, платформа x86_64-pc-windows-gnu, линкер mingw-w64 из nixpkgs;
#   * оболочка — Qt 6 под mingw (официальные двоичные файлы, aqtinstall) плюс mingw-w64 из nixpkgs;
#   * дистрибутив — два exe и рядом то, без чего они не запускаются.
#
# Linux-сборку этот скрипт не трогает: он только читает репозиторий и пишет в dist/windows.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dist="$repo/dist/windows"
target=x86_64-pc-windows-gnu
qt_version="${HAULER_QT_VERSION:-6.8.1}"
qt_root="${HAULER_QT_ROOT:-$repo/.cache/qt-windows}"

usage() {
    cat <<'USAGE'
scripts/windows.sh <команда>

  --engine     собрать dist/windows/dj-engine.exe
  --ui         собрать dist/windows/dj-ui.exe (Qt под mingw, скачивается один раз)
  --dist       положить рядом с exe всё нужное и собрать zip
  --msi        собрать установочный пакет (нужен готовый дистрибутив)
  --all        --engine --ui --dist --msi
  --clean      удалить dist/windows, кэш Qt и сборку UI

Переменные: HAULER_QT_VERSION (по умолчанию 6.8.1), HAULER_QT_ROOT, HAULER_ORT_DLL.
USAGE
}

engine() {
    echo "== движок: $target"
    mkdir -p "$dist"
    REPO="$repo" nix shell nixpkgs#rustup "nixpkgs#pkgsCross.mingwW64.stdenv.cc" \
        nixpkgs#gcc nixpkgs#binutils nixpkgs#lld nixpkgs#pkg-config --command bash -c '
        set -eu
        export CARGO_HOME="$REPO/.cache/cargo-rustup"
        export RUSTUP_HOME="$REPO/.cache/rustup"
        # Сборочные скрипты — программы для хоста, и rustc линкует их своим lld через папку gcc-ld внутри
        # тулчейна. В nix-шелле она пуста, поэтому ld.lld указывается на lld из nix.
        sysroot="$(rustup run stable rustc --print sysroot)"
        mkdir -p "$sysroot/lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld"
        ln -sf "$(command -v ld.lld)" "$sysroot/lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld/ld.lld"
        # chromaprint просит libpthread.a, которого mingw из nix не содержит: потоки там собственные, и символов
        # pthread никто не требует. Пустой архив под этим именем удовлетворяет линкер и живёт в кэше сборки.
        shim="$REPO/.cache/win-shims"
        mkdir -p "$shim"
        [ -f "$shim/libpthread.a" ] || x86_64-w64-mingw32-ar cr "$shim/libpthread.a"
        export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
        # The vendored C and C++ libraries (chromaprint, the stretcher, signalsmith) are built by `cc`/`cmake`
        # inside the build scripts: they must be told the target compiler too, or they build host objects.
        export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
        export CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++
        export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar
        export RANLIB_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ranlib
        export CMAKE_SYSTEM_NAME=Windows
        export CMAKE_C_COMPILER=x86_64-w64-mingw32-gcc
        export CMAKE_CXX_COMPILER=x86_64-w64-mingw32-g++
        export CMAKE_RC_COMPILER=x86_64-w64-mingw32-windres
        # mingw из nix собран с mcF-потоками: часть libstdc++ (gthr.h) лежит в mcfgthread, а его заголовки и
        # библиотека — в отдельных выходах замыкания компилятора, а не в путях поиска по умолчанию.
        mcfg_inc=""
        mcfg_lib=""
        for store_path in $(nix-store -qR "$(readlink -f "$(command -v x86_64-w64-mingw32-g++)")"); do
            if [ -f "$store_path/include/mcfgthread/gthr.h" ]; then mcfg_inc="$store_path/include"; fi
            if [ -f "$store_path/lib/libmcfgthread.a" ]; then mcfg_lib="$store_path/lib"; fi
        done
        if [ -z "$mcfg_inc" ] || [ -z "$mcfg_lib" ]; then
            echo "не нашёл mcfgthread в замыкании mingw: C++ под Windows не соберётся" >&2
            exit 1
        fi
        export CXXFLAGS_x86_64_pc_windows_gnu="-I$mcfg_inc -L$mcfg_lib"
        export RUSTFLAGS="-L native=$shim -L native=$mcfg_lib"
        rustup toolchain install stable --profile minimal --target x86_64-pc-windows-gnu --no-self-update
        cd "$REPO"
        rustup run stable cargo build --release --target x86_64-pc-windows-gnu -p dj-engine
    '
    cp "${CARGO_TARGET_DIR:-$repo/target}/$target/release/dj-engine.exe" "$dist/"
    echo "готово: $dist/dj-engine.exe"
}

qt_mingw_tool() {
    # Официальные сборки Qt для Windows собраны mingw с потоками winpthreads, и Qt6*.dll требуют
    # libwinpthread-1.dll. Файл приходит только вместе с инструментом Qt — берём его оттуда, чтобы версия
    # совпадала с той, под которую собран Qt.
    tool_dir="$qt_root/Tools/mingw1310_64"
    if [ -f "$tool_dir/bin/libwinpthread-1.dll" ]; then
        echo "$tool_dir"
        return
    fi
    echo "== инструмент mingw для Qt (один раз)" >&2
    QT_ROOT="$qt_root" nix shell nixpkgs#python3 --command bash -c '
        set -eu
        [ -x "$QT_ROOT/venv/bin/aqt" ] || {
            python3 -m venv "$QT_ROOT/venv" >/dev/null
            "$QT_ROOT/venv/bin/pip" install --quiet aqtinstall
        }
        "$QT_ROOT/venv/bin/aqt" install-tool windows desktop tools_mingw1310 --outputdir "$QT_ROOT"
    '
    echo "$tool_dir"
}
qt_host_download() {
    host_root="${HAULER_QT_HOST_ROOT:-$repo/.cache/qt-host}"
    if [ -d "$host_root/$qt_version/gcc_64/lib/cmake" ]; then
        echo "$host_root/$qt_version/gcc_64"
        return
    fi
    echo "== Qt $qt_version для хоста: инструменты сборки (один раз)" >&2
    mkdir -p "$host_root"
    QT_ROOT="$host_root" QT_VERSION="$qt_version" nix shell nixpkgs#python3 --command bash -c '
        set -eu
        [ -x "$QT_ROOT/venv/bin/aqt" ] || {
            python3 -m venv "$QT_ROOT/venv" >/dev/null
            "$QT_ROOT/venv/bin/pip" install --quiet aqtinstall
        }
        "$QT_ROOT/venv/bin/aqt" install-qt linux desktop "$QT_VERSION" linux_gcc_64 \
            --outputdir "$QT_ROOT" --archives qtbase qtdeclarative qtshadertools qtsvg icu
    '
    echo "$host_root/$qt_version/gcc_64"
}

# Инструменты Qt (moc, qmlcachegen, qmlimportscanner) из официальной Linux-сборки — обычные бинарники под
# FHS: они ждут /lib64/ld-linux-x86-64.so.2, которого на NixOS нет. Поэтому каждый такой файл заменяется
# обёрткой, запускающей настоящий инструмент внутри FHS-окружения с нужными библиотеками. Без этого CMake
# либо не может запустить инструмент, либо (для Windows-копий) не может его исполнить вовсе.
qt_host_wrap() {
    host_prefix="$1"
    fhs_file="$repo/.cache/qt-host/fhs-env"
    if [ ! -s "$fhs_file" ] || [ ! -x "$(cat "$fhs_file")/bin/qt-tools" ]; then
        echo "== FHS-окружение для инструментов Qt (один раз)"
        fhs_path=$(nix build --impure --no-link --print-out-paths --expr '
            with import (builtins.getFlake "nixpkgs") {};
            buildFHSEnv {
                name = "qt-tools";
                targetPkgs = p: with p; [
                    qt6.qtbase qt6.qtdeclarative zstd stdenv.cc.cc.lib zlib pcre2 double-conversion
                    freetype fontconfig libxkbcommon dbus icu harfbuzz libpng brotli expat glib libGL
                    xorg.libX11 xorg.libXext xorg.libXrender xorg.libXi xorg.libXtst xorg.libxcb
                    xcb-util-cursor libdrm mesa krb5 openssl gnutls sqlite libglvnd libinput
                    xorg.xcbutil xorg.xcbutilimage xorg.xcbutilkeysyms xorg.xcbutilrenderutil xorg.xcbutilwm
                ];
                runScript = "bash";
            }')
        printf '%s\n' "$fhs_path" > "$fhs_file"
    fi
    fhs_path="$(cat "$fhs_file")"
    [ -d "$host_prefix/.wrapped" ] && return 0
    mkdir -p "$host_prefix/.wrapped"
    bash_path="#!$(command -v bash)"
    echo "== обёртки вокруг инструментов Qt: $host_prefix"
    for dir in bin libexec; do
        [ -d "$host_prefix/$dir" ] || continue
        for tool in "$host_prefix/$dir"/*; do
            [ -f "$tool" ] || continue
            case "$tool" in *.so*|*.prl|*.cmake|*.json|*.txt|*.dat) continue ;; esac
            # Уже обёрнутый инструмент лежит рядом как *.qt-bin: обёртку переписываем, а не пропускаем, иначе
            # исправленная обёртка не доедет до уже подготовленного каталога.
            if [ ! -f "$tool.qt-bin" ]; then
                head -c 4 "$tool" | grep -q ELF || continue
            fi
            real="$tool.qt-bin"
            [ -f "$real" ] || mv "$tool" "$real"
            # shebang указывает на bash из nix: на NixOS /bin/sh нет.
            {
                printf '%s\n' "$bash_path" \
                    '# Инструмент Qt из официальной Linux-сборки: запускается внутри FHS-окружения.' \
                    "QT_HOST_PATH='$host_prefix'" \
                    'export QT_HOST_PATH' \
                    "exec '$fhs_path/bin/qt-tools' -c 'exec \"\$@\"' sh '$real' \"\$@\""
            } > "$tool"
            chmod +x "$tool"
        done
    done
}

qt_download() {
    if [ -d "$qt_root/$qt_version/mingw_64/lib/cmake" ]; then
        return
    fi
    echo "== Qt $qt_version под mingw: качаю в $qt_root (один раз)"
    mkdir -p "$qt_root"
    QT_ROOT="$qt_root" QT_VERSION="$qt_version" nix shell nixpkgs#python3 --command bash -c '
        set -eu
        python3 -m venv "$QT_ROOT/venv" >/dev/null
        "$QT_ROOT/venv/bin/pip" install --quiet aqtinstall
        "$QT_ROOT/venv/bin/aqt" install-qt windows desktop "$QT_VERSION" win64_mingw \
            --outputdir "$QT_ROOT" --archives qtbase qtdeclarative qtshadertools qtsvg icu
    '
}

ui() {
    qt_download
    host_prefix="$(qt_host_download | tail -1)"
    qt_host_wrap "$host_prefix"
    prefix="$qt_root/$qt_version/mingw_64"
    if [ ! -d "$prefix/lib/cmake" ]; then
        echo "не нашёл Qt в $prefix — смотри вывод aqt выше" >&2
        exit 1
    fi
    echo "== оболочка: Qt из $prefix"
    REPO="$repo" PREFIX="$prefix" HOST_PREFIX="$host_prefix" nix shell "nixpkgs#pkgsCross.mingwW64.stdenv.cc" \
        nixpkgs#cmake nixpkgs#ninja nixpkgs#gcc nixpkgs#binutils nixpkgs#lld nixpkgs#pkg-config --command bash -c '
        set -eu
        cd "$REPO"
        # mingw из nix добавляет в линковку -lmcfgthread, а библиотека лежит в отдельном выходе замыкания.
        mcfg_inc=""
        mcfg_lib=""
        for store_path in $(nix-store -qR "$(readlink -f "$(command -v x86_64-w64-mingw32-g++)")"); do
            if [ -f "$store_path/include/mcfgthread/gthr.h" ]; then mcfg_inc="$store_path/include"; fi
            if [ -f "$store_path/lib/libmcfgthread.a" ]; then mcfg_lib="$store_path/lib"; fi
        done
        if [ -z "$mcfg_lib" ]; then echo "нет mcfgthread: оболочка не соберётся" >&2; exit 1; fi
        # Инструменты Qt (qmlimportscanner и прочие) — программы хоста: Windows-копии здесь не запускаются.
        export QT_HOST_PATH="$HOST_PREFIX"
        cmake -S . -B build-windows -G Ninja \
            -DCMAKE_TOOLCHAIN_FILE=cmake/toolchain-mingw64.cmake \
            -DCMAKE_BUILD_TYPE=Release \
            -DCMAKE_PREFIX_PATH="$PREFIX" \
            -DQT_HOST_PATH="$HOST_PREFIX" \
            -DQT_HOST_PATH_CMAKE_DIR="$HOST_PREFIX/lib/cmake" \
            -DCMAKE_FIND_ROOT_PATH="$PREFIX" \
            -DCMAKE_CXX_FLAGS="-I$mcfg_inc" \
            -DCMAKE_EXE_LINKER_FLAGS="-L$mcfg_lib" \
            -DCMAKE_SHARED_LINKER_FLAGS="-L$mcfg_lib"
        cmake --build build-windows --target dj-ui
    '
    mkdir -p "$dist"
    cp "$repo/build-windows/apps/dj-ui/dj-ui.exe" "$dist/"
    echo "готово: $dist/dj-ui.exe"
}

deploy() {
    prefix="$qt_root/$qt_version/mingw_64"
    echo "== дистрибутив"
    mkdir -p "$dist"
    for dll in Qt6Core Qt6Gui Qt6Qml Qt6Quick Qt6QuickControls2 Qt6QuickLayouts Qt6QuickTemplates2 \
               Qt6Widgets Qt6Svg Qt6Network Qt6ShaderTools Qt6OpenGL Qt6QmlModels Qt6QmlMeta \
               Qt6QmlWorkerScript Qt6QmlCompiler Qt6Concurrent Qt6DBus Qt6PrintSupport; do
        find "$prefix/bin" -maxdepth 1 -iname "$dll.dll" -exec cp {} "$dist/" \; 2>/dev/null || true
    done
    for dir in platforms styles imageformats iconengines qml tls networkinformation generic; do
        if [ -d "$prefix/plugins/$dir" ]; then cp -r "$prefix/plugins/$dir" "$dist/"; fi
    done
    for dll in libstdc++-6 libgcc_s_seh-1 libwinpthread-1; do
        find "$prefix/bin" -maxdepth 1 -iname "$dll.dll" -exec cp {} "$dist/" \; 2>/dev/null || true
    done
    # Дерево QML-модулей: без него оболочка стартует и не находит ни одной страницы.
    if [ -d "$prefix/qml" ]; then rm -rf "$dist/qml"; cp -r "$prefix/qml" "$dist/qml"; fi
    # Трансляции Qt — по желанию; qt.conf уже указывает на локальный префикс.
    if [ -n "${HAULER_ORT_DLL:-}" ] && [ -f "${HAULER_ORT_DLL}" ]; then
        cp "$HAULER_ORT_DLL" "$dist/onnxruntime.dll"
    fi
    # C++-рантайм: два разных набора, и нужны оба. Наши exe собраны mingw из nix — его libstdc++ работает через
    # mcfgthread (libmcfgthread-2.dll). Qt собран mingw с winpthreads — Qt6*.dll требуют libwinpthread-1.dll.
    # Без первого не стартует наш код, без второго — Qt.
    DIST="$dist" nix shell "nixpkgs#pkgsCross.mingwW64.stdenv.cc" --command bash -c '
        set -eu
        for store_path in $(nix-store -qR "$(readlink -f "$(command -v x86_64-w64-mingw32-g++)")"); do
            for dll in libstdc++-6.dll libgcc_s_seh-1.dll libatomic-1.dll libmcfgthread-2.dll; do
                for dir in x86_64-w64-mingw32/lib bin lib; do
                    if [ -f "$store_path/$dir/$dll" ] && [ ! -f "$DIST/$dll" ]; then
                        cp "$store_path/$dir/$dll" "$DIST/"
                    fi
                done
            done
        done
    '
    winpthread_dir="$(qt_mingw_tool)"
    if [ -f "$winpthread_dir/bin/libwinpthread-1.dll" ]; then
        cp "$winpthread_dir/bin/libwinpthread-1.dll" "$dist/"
    else
        echo "нет libwinpthread-1.dll: оболочка на Windows не запустится" >&2
        exit 1
    fi
    cat > "$dist/qt.conf" <<'CONF'
[Paths]
Prefix = .
Plugins = .
Qml2Imports = qml
CONF
    cat > "$dist/engine-console.bat" <<'BAT'
@echo off
rem The engine's console goes to a file: a crash before its own log opens still leaves a trace.
if not exist "%APPDATA%\hauler-dj\logs" mkdir "%APPDATA%\hauler-dj\logs"
"%~dp0dj-engine.exe" %* >> "%APPDATA%\hauler-dj\logs\dj-engine-console.log" 2>&1
BAT
    cat > "$dist/hauler-dj.bat" <<'BAT'
@echo off
rem Maximum detail for now: the two log files are how a fault on a machine we cannot see is found.
set HAULER_LOG=trace
rem --device with an empty name asks for the system's own output: without it the engine plays a generated
rem tone into nothing, which looks exactly like "no sound but the meter is lit".
start "" /min "%~dp0engine-console.bat" --device ""
timeout /t 2 /nobreak >nul
start "" "%~dp0dj-ui.exe"
BAT
    cat > "$dist/README.txt" <<'TXT'
Hauler DJ для Windows.

Запуск: hauler-dj.bat (движок, затем оболочка) или dj-engine.exe и затем dj-ui.exe.

Каталоги:
  %APPDATA%\hauler-dj        — настройки, раскладка, зеркало метаданных, модели
  %LOCALAPPDATA%\hauler-dj   — кэш формы волны и PCM

Связь: именованная труба \\.\pipe\hauler-dj-engine-<пользователь>.
onnxruntime.dll рядом необязательна: без неё работает встроенный детектор темпа.
TXT
    # qml-плагины тянут куда больше модулей Qt, чем перечислено выше (диалоги, эффекты, частицы, формы,
    # векторные картинки, QtTest). Вместо длинного списка — цикл: проверка говорит, чего не хватает, файлы
    # берутся из bin Qt; повторяется, пока пропажи не кончатся.
    for _ in 1 2 3 4 5; do
        missing=$(DIST="$dist" REPO="$repo" nix shell "nixpkgs#pkgsCross.mingwW64.stdenv.cc" nixpkgs#python3 \
            --command bash -c 'cd "$REPO" && python3 scripts/windows-deps-check.py --dist "$DIST" --only-missing' \
            2>/dev/null || true)
        [ -z "$missing" ] && break
        copied=0
        for name in $missing; do
            found=$(find "$prefix/bin" -maxdepth 1 -iname "$name" | head -1)
            if [ -n "$found" ]; then
                cp "$found" "$dist/"
                copied=1
            fi
        done
        if [ "$copied" -eq 0 ]; then
            echo "модули не найдены в $prefix/bin: $(echo "$missing" | tr '\n' ' ')" >&2
            break
        fi
    done
    # Проверка: ни один exe и ни одна dll не должны требовать библиотеку, которой нет в дистрибутиве.
    DIST="$dist" REPO="$repo" nix shell "nixpkgs#pkgsCross.mingwW64.stdenv.cc" nixpkgs#python3 --command bash -c '
        set -eu
        cd "$REPO"
        python3 scripts/windows-deps-check.py --dist "$DIST"
    '
    ( cd "$dist" && rm -f hauler-dj-windows.zip && zip -q -r hauler-dj-windows.zip . -x '*.zip' -x '*.msi' )
    echo "готово: $dist/hauler-dj-windows.zip"
}

msi_tools() {
    # msitools собирается с `doCheck = false`: его собственный набор тестов падает в песочнице на msibuild, а
    # сам wixl при этом рабочий. Найденный путь кэшируется, чтобы не будить nix на каждой сборке пакета.
    cache="$repo/.cache/msitools-path"
    if [ -s "$cache" ] && [ -x "$(cat "$cache")/bin/wixl" ]; then
        cat "$cache"
        return
    fi
    mkdir -p "$repo/.cache"
    path=$(nix build --impure --no-link --print-out-paths --expr '
        with import (builtins.getFlake "nixpkgs") {};
        msitools.overrideAttrs (o: { doCheck = false; })' | tail -1)
    printf '%s\n' "$path" > "$cache"
    cat "$cache"
}

msi() {
    if [ ! -f "$dist/dj-ui.exe" ] || [ ! -f "$dist/dj-engine.exe" ]; then
        echo "нет готового дистрибутива в $dist: сначала --dist" >&2
        exit 1
    fi
    tools="$(msi_tools)"
    echo "== пакет msi"
    REPO="$repo" MSITOOLS="$tools" nix shell nixpkgs#python3 --command bash -c '
        set -eu
        export PATH="$MSITOOLS/bin:$PATH"
        cd "$REPO"
        python3 scripts/windows-msi.py --dist dist/windows
    '
    # Проверка: в пакете должно быть ровно столько файлов, сколько в дистрибутиве, без самого zip и без msi.
    package="$dist/hauler-dj-$(grep -m1 "^version" "$repo/Cargo.toml" | cut -d'"' -f2)-x64.msi"
    # Считаются только строки данных таблицы File: у выгрузки msitools впереди идут строка имён колонок и
    # строка типов, а ключи файлов начинаются с `cmp_`.
    in_package=$("$tools/bin/msiinfo" export "$package" File | grep -c '^cmp_')
    in_dist=$(find "$dist" -type f ! -name "*.zip" ! -name "*.msi" | wc -l)
    if [ "$in_package" -ne "$in_dist" ]; then
        echo "в пакете $in_package файлов, в дистрибутиве $in_dist" >&2
        exit 1
    fi
    echo "проверено: $in_package файлов, шаблон $("$tools/bin/msiinfo" suminfo "$package" | sed -n "s/^Template: //p")"
    ls -la "$package"
}

case "${1:---all}" in
    --engine) engine ;;
    --ui) ui ;;
    --dist) deploy ;;
    --msi) msi ;;
    --all) engine; ui; deploy; msi ;;
    --clean) rm -rf "$dist" "$qt_root" "$repo/build-windows" "$repo/.cache/msitools-path" ;;
    -h|--help) usage ;;
    *) usage; exit 2 ;;
esac
