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
# Куда складывать пакет. DJ_WIN_DIST перекрывает dist/windows: в общем дереве этот каталог может принадлежать
# другому пользователю, и тогда его не переписать — собранное можно сложить в свой.
dist="${DJ_WIN_DIST:-$repo/dist/windows}"
target=x86_64-pc-windows-gnu
qt_version="${HAULER_QT_VERSION:-6.8.1}"
qt_root="${HAULER_QT_ROOT:-$repo/.cache/qt-windows}"

usage() {
    cat <<'USAGE'
scripts/windows.sh <команда>

  --engine     собрать dist/windows/dj-engine.exe
  --ui         собрать dist/windows/dj-ui.exe (Qt под mingw, скачивается один раз)
  --launcher   собрать dist/windows/hauler-dj.exe (запускатор: движок и оболочка одним действием)
  --ort        положить onnxruntime.dll (MIT) и, если дан ORT_CRT_DIR, MSVC runtime рядом с бинарями
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

# Запускатор: один exe, который поднимает движок, ждёт его и открывает оболочку.
#
# В релизе он собирается без консоли (windows_subsystem = "windows"): двойной щелчок по нему — это приложение,
# а не чёрное окно. Диагностическим путём остаётся hauler-dj.bat.
launcher() {
    echo "== запускатор"
    mkdir -p "$dist"
    # Запускатор — чистый Rust без C++: нужны только линкер mingw и сам target, поэтому обёртка проще,
    # чем у движка, но rustup в dev shell не живёт: он приходит из nix shell.
    REPO="$repo" nix shell nixpkgs#rustup nixpkgs#llvm "nixpkgs#pkgsCross.mingwW64.stdenv.cc" nixpkgs#lld --command bash -c '
        export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
        # The controller HID layer is C (`hidapi`), so the compiler for the *target* has to be named: without these the
        # build script reaches for the host `gcc`, compiles Windows code against Linux headers and stops at
        # `guiddef.h: No such file or directory` (found 2026-11-28, the first cross build after HID moved to hidapi).
        export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
        export CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++
        export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar
        # The host half: the build scripts of every crate in the graph are host programs, and rustc links them with its
        # own lld through the `gcc-ld` folder inside the toolchain, which is empty in a nix shell. Without this the build
        # stops at the first build script with `posix_spawn failed` — and it stops before a single line of ours is
        # compiled, which is why it reads as "the Windows port broke" while nothing of it ran (found 2026-11-28, when the
        # launcher was built on a machine whose rustup had been refreshed).
        export CARGO_HOME="$REPO/.cache/cargo-rustup"
        export RUSTUP_HOME="$REPO/.cache/rustup"
        sysroot="$(rustup run stable rustc --print sysroot)"
        mkdir -p "$sysroot/lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld"
        ln -sf "$(command -v ld.lld)" "$sysroot/lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld/ld.lld"
        # chromaprint просит libpthread.a, которого mingw из nix не содержит: пустой архив под этим именем
        # лежит в кэше сборки, и тот же путь нужен запускатору — он тянет dj-paths, а через него конвейер.
        mkdir -p "$REPO/.cache/win-shims"
        [ -f "$REPO/.cache/win-shims/libpthread.a" ] || \
            x86_64-w64-mingw32-ar cr "$REPO/.cache/win-shims/libpthread.a"
        # The icon Explorer draws for the launcher: a resource object linked in as an argument. Rust has no
        # resource step of its own, and a crate for it would be one more download for one file.
        #
        # Two steps, because `windres` on a `.rc` starts a C preprocessor through `popen`, and popen needs
        # `/bin/sh`, which NixOS does not have: `llvm-rc -no-preprocess` turns the `.rc` into a `.res`, and
        # `windres` on a `.res` calls no preprocessor at all.
        mkdir -p "$REPO/.cache/win-icon"
        llvm-rc -no-preprocess /fo "$REPO/.cache/win-icon/app.res" "$REPO/assets/app.rc"
        x86_64-w64-mingw32-windres -O coff -i "$REPO/.cache/win-icon/app.res" \
            -o "$REPO/.cache/win-icon/app-icon.o"
        export RUSTFLAGS="-L native=$REPO/.cache/win-shims -C link-arg=$REPO/.cache/win-icon/app-icon.o"
        rustup toolchain install stable --profile minimal --target x86_64-pc-windows-gnu --no-self-update
        cd "$REPO"
        rustup run stable cargo build --release --target x86_64-pc-windows-gnu -p dj-launcher -p dj-controller
    '
    cp "${CARGO_TARGET_DIR:-$repo/target}/$target/release/dj-launcher.exe" "$dist/hauler-dj.exe"
    # The controller service stays optional, but it has to be there to be switched on: the launcher looks for it
    # beside itself and says so in the log when it is missing.
    cp "${CARGO_TARGET_DIR:-$repo/target}/$target/release/dj-controller.exe" "$dist/dj-controller.exe"
    echo "готово: $dist/hauler-dj.exe и $dist/dj-controller.exe"
}

# ONNX Runtime: без него внешняя модель удара не работает, и движок молча считает встроенным детектором —
# на части треков это другой уровень темпа (96 → 128 на «Любе», замер 2026-11-20). DLL берётся из
# официального пакета Microsoft (NuGet, MIT), версия и хеш закреплены: другая версия не загрузится, и откат
# на встроенный детектор будет тихим.
#
# `onnxruntime.dll` собран MSVC и требует MSVCP140.dll / MSVCP140_1.dll из Visual C++ Redistributable. Их
# кладём из каталога, который назван `--crt-dir` (app-local распространение разрешено Microsoft), а если
# каталога нет — печатаем, чего не хватает: пакет без них запустится не на всякой машине.
ort_version="1.22.0"
ort_dll_sha256="579b636403983254346a5c1d80bd28f1519cd1e284cd204f8d4ff41f8d711559"
crt_dlls="MSVCP140.dll MSVCP140_1.dll VCRUNTIME140.dll VCRUNTIME140_1.dll"

ort() {
    echo "== onnx runtime $ort_version"
    mkdir -p "$dist/LICENSES"
    cache="$repo/.cache/win-ort"
    mkdir -p "$cache"
    dll="$cache/onnxruntime.dll"
    if [ ! -f "$dll" ]; then
        nix shell nixpkgs#curl nixpkgs#unzip --command bash -c "
            set -e
            curl -sSL -o '$cache/ort.nupkg' \
                'https://www.nuget.org/api/v2/package/Microsoft.ML.OnnxRuntime/$ort_version'
            unzip -q -o '$cache/ort.nupkg' -d '$cache/nupkg'
            cp '$cache/nupkg/runtimes/win-x64/native/onnxruntime.dll' '$dll'
            cp '$cache/nupkg/LICENSE' '$cache/LICENSE'
            cp '$cache/nupkg/ThirdPartyNotices.txt' '$cache/ThirdPartyNotices.txt'
        "
    fi
    got=$(sha256sum "$dll" | cut -d' ' -f1)
    if [ "$got" != "$ort_dll_sha256" ]; then
        echo "onnxruntime.dll не тот: sha256 $got, ожидался $ort_dll_sha256" >&2
        exit 1
    fi
    cp "$dll" "$dist/"
    cp "$cache/LICENSE" "$dist/LICENSES/onnxruntime-MIT.txt"
    cp "$cache/ThirdPartyNotices.txt" "$dist/LICENSES/onnxruntime-third-party.txt"
    echo "готово: $dist/onnxruntime.dll"
    missing=""
    for name in $crt_dlls; do
        if [ -n "${ORT_CRT_DIR:-}" ] && [ -f "$ORT_CRT_DIR/$name" ]; then
            cp "$ORT_CRT_DIR/$name" "$dist/"
        else
            missing="$missing $name"
        fi
    done
    if [ -n "$missing" ]; then
        echo "внимание: в пакете нет$missing" >&2
        echo "  onnxruntime.dll без них не загрузится. Возьмите файлы из установленного Visual C++" >&2
        echo "  Redistributable (C:\\Windows\\System32) и повторите с ORT_CRT_DIR=<каталог>." >&2
    else
        echo "готово: MSVC runtime рядом с движком"
    fi
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
            # Уже обёрнутый двоичный файл лежит рядом как *.qt-bin. Обернуть его ещё раз значит построить цепочку
            # обёрток (qmlcachegen.qt-bin.qt-bin…), а внутри цепочки инструмент теряет свой rpath и падает на
            # Qt_6_PRIVATE_API. Обёртки создаются только для настоящих инструментов, не для *.qt-bin.
            case "$tool" in *.qt-bin|*.qt-bin.*) continue ;; esac
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
                    '# Пути к библиотекам, унаследованные от оболочки сборки, убираются: иначе инструмент 6.8.1 грузит' \
                    '# libQt6Qml 6.11 из dev shell и падает на приватном символе Qt_6_PRIVATE_API.' \
                    'unset LD_LIBRARY_PATH LD_PRELOAD QML2_IMPORT_PATH QML_IMPORT_PATH QT_PLUGIN_PATH' \
                    'unset QT_QPA_PLATFORM_PLUGIN_PATH QT_HOST_PATH_CMAKE_DIR' \
                    "QT_HOST_PATH='$host_prefix'" \
                    'export QT_HOST_PATH' \
                    "LD_LIBRARY_PATH='$host_prefix/lib'" \
                    'export LD_LIBRARY_PATH' \
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

# Каталог сборки оболочки, названный в одном месте. Перекрывается DJ_WIN_BUILD_DIR: если build-windows собран
# другим пользователем, чужие файлы не переписать — и спорить за них нечего, можно собрать в своём каталоге.
ui_build="${DJ_WIN_BUILD_DIR:-$repo/build-windows}"

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
    REPO="$repo" PREFIX="$prefix" HOST_PREFIX="$host_prefix" UI_BUILD="$ui_build" nix shell "nixpkgs#pkgsCross.mingwW64.stdenv.cc" \
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
        cmake -S . -B "$UI_BUILD" -G Ninja \
            -DCMAKE_TOOLCHAIN_FILE=cmake/toolchain-mingw64.cmake \
            -DCMAKE_BUILD_TYPE=Release \
            -DCMAKE_PREFIX_PATH="$PREFIX" \
            -DQT_HOST_PATH="$HOST_PREFIX" \
            -DQT_HOST_PATH_CMAKE_DIR="$HOST_PREFIX/lib/cmake" \
            -DCMAKE_FIND_ROOT_PATH="$PREFIX" \
            -DCMAKE_CXX_FLAGS="-I$mcfg_inc" \
            -DCMAKE_EXE_LINKER_FLAGS="-L$mcfg_lib" \
            -DCMAKE_SHARED_LINKER_FLAGS="-L$mcfg_lib"
        cmake --build "$UI_BUILD" --target dj-ui
    '
    mkdir -p "$dist"
    cp "$ui_build/apps/dj-ui/dj-ui.exe" "$dist/"
    echo "готово: $dist/dj-ui.exe"
}

deploy() {
    prefix="$qt_root/$qt_version/mingw_64"
    echo "== дистрибутив"
    mkdir -p "$dist"
    cp "$repo/assets/app.ico" "$dist/"
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
    #
    # Что именно нужно, решают сами бинарники: список имён, написанный рукой, стареет молча (тулчейн переименовывает
    # свой потоковый рантайм, добавляет libssp/libquadmath), а поиск «где-то в замыкании компилятора» зависит от того,
    # какой nixpkgs разрешился на этой машине. Так пакет дважды уехал без `libmcfgthread-2.dll` (отчёты владельца,
    # 25.11 и 27.11). Теперь набор берётся из таблиц импорта, а ищется он и в bin Qt, и в замыкании тулчейна.
    mingw_runtime_roots=$(nix shell "nixpkgs#pkgsCross.mingwW64.stdenv.cc" --command bash -c '
        for path in $(nix-store -qR "$(readlink -f "$(command -v x86_64-w64-mingw32-g++)")"); do
            for dir in bin lib x86_64-w64-mingw32/lib x86_64-w64-mingw32/bin; do
                [ -d "$path/$dir" ] && echo "$path/$dir"
            done
        done' 2>/dev/null || true)
    # The Windows-only halves of the screens, copied only when the tree carries them: `libwdi.dll` (loaded at run time,
    # so its absence is a message and never a failure to start) and a prepared driver package. `assets/windows/` is the
    # place a person drops them; nothing here invents a signed driver.
    if [ -f "$repo/assets/windows/libwdi.dll" ]; then
        cp "$repo/assets/windows/libwdi.dll" "$dist/"
        echo "  libwdi.dll: копия из assets/windows"
    fi
    if [ -d "$repo/assets/windows/driver" ]; then
        rm -rf "$dist/driver"
        cp -r "$repo/assets/windows/driver" "$dist/driver"
        echo "  driver/: пакет драйвера из assets/windows"
    fi
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
    cat > "$dist/hauler-dj.bat" <<'BAT'
@echo off
rem Maximum detail for now: the engine's own log file is where a fault on a machine we cannot see is found.
set HAULER_LOG=trace
rem The launcher is the one that knows how to start the pair: it waits for the engine's endpoint, starts the shell
rem and keeps the engine from outliving it. This script is the diagnostic way in — it keeps a console, so a
rem failure has somewhere to print.
"%~dp0hauler-dj.exe"
if errorlevel 1 (
    echo.
    echo The application did not start. Run dj-engine.exe from this folder: it prints the reason.
    echo Its log would be %%APPDATA%%\hauler-dj\logs\dj-engine.log
    pause
)
rem The shell is started by the launcher, not here.
BAT
    cat > "$dist/README.txt" <<'TXT'
Hauler DJ для Windows.

Запуск: hauler-dj.exe (движок и оболочка вместе) или hauler-dj.bat — то же, но с консолью для диагностики.

Темп и сетку считает внешняя модель удара Beat This! (MIT) через ONNX Runtime (MIT). Рядом лежат
onnxruntime.dll и, если сборка их положила, MSVCP140/VCRUNTIME140 из Visual C++ Redistributable:
без runtime движок молча считает встроенным детектором, и на части треков это другой уровень темпа.
Тексты лицензий — в LICENSES/.

Каталоги:
  %APPDATA%\hauler-dj        — настройки, раскладка, зеркало метаданных, модели
  %LOCALAPPDATA%\hauler-dj   — кэш формы волны и PCM

Связь: именованная труба \\.\pipe\hauler-dj-engine-<пользователь>.
onnxruntime.dll рядом необязательна: без неё работает встроенный детектор темпа.

Журналы (подробность — переменная HAULER_LOG, запускатор ставит trace):
  %APPDATA%\hauler-dj\logs\dj-engine.log
  %APPDATA%\hauler-dj\logs\dj-ui.log

Если движок не поднялся, запустите dj-engine.exe вручную из этой папки: он печатает причину в консоль и в журнал.
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
            found=$(find "$prefix/bin" -maxdepth 1 -iname "$name" 2>/dev/null | head -1)
            if [ -z "$found" ]; then
                # shellcheck disable=SC2086 # the roots are a list of paths, split on purpose
                found=$(find $mingw_runtime_roots -maxdepth 1 -iname "$name" 2>/dev/null | head -1)
            fi
            if [ -n "$found" ]; then
                cp "$found" "$dist/"
                copied=1
            else
                echo "не нашлось: $name" >&2
            fi
        done
        if [ "$copied" -eq 0 ]; then
            echo "нечего взять: $(echo "$missing" | tr '\n' ' ')" >&2
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
    # Пакет собирается только из доказанного дистрибутива: `msi` проверяет, что рядом с exe есть всё, что они просят,
    # а не только что exe на месте. Раньше сюда можно было подсунуть каталог без рантайма, и установщик увозил
    # неполный набор — тот самый «не хватает dll» на машине, где сборки уже нет (отчёт владельца, 27.11).
    DIST="$dist" REPO="$repo" nix shell "nixpkgs#pkgsCross.mingwW64.stdenv.cc" nixpkgs#python3 --command bash -c '
        set -eu
        cd "$REPO"
        python3 scripts/windows-deps-check.py --dist "$DIST"
    '
    tools="$(msi_tools)"
    echo "== пакет msi"
    REPO="$repo" DIST="$dist" MSITOOLS="$tools" nix shell nixpkgs#python3 --command bash -c '
        set -eu
        export PATH="$MSITOOLS/bin:$PATH"
        cd "$REPO"
        # Каталог тот же, что у --dist, а не всегда dist/windows: иначе msi собирается из одного каталога, а zip и
        # проверка смотрят в другой.
        python3 scripts/windows-msi.py --dist "$DIST"
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
    # Размер пакета — не признак содержимого: cab пакуется блоками, и разные сборки ложатся в один и тот же размер
    # с точностью до байта (проверено опытом: три разных пакета, один размер). Поэтому пакет называет свой хеш.
    echo "пакет: $(stat -c %s "$package") Б, sha256 $(sha256sum "$package" | cut -d' ' -f1)"
    ls -la "$package"
}

case "${1:---all}" in
    --engine) engine ;;
    --ui) ui ;;
    --launcher) launcher ;;
    --ort) ort ;;
    --dist) deploy ;;
    --msi) msi ;;
    --all) engine; ui; launcher; ort; deploy; msi ;;
    --clean) rm -rf "$dist" "$qt_root" "$ui_build" "$repo/.cache/msitools-path" ;;
    -h|--help) usage ;;
    *) usage; exit 2 ;;
esac
