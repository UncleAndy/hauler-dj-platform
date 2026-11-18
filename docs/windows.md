# Windows: сборка и запуск

Хост сборки — Linux (NixOS). Windows-версия собирается здесь же: движок — Rust под mingw, оболочка — Qt 6
под mingw. Linux-сборка при этом не меняется.

## Команды

```
bash scripts/windows.sh --engine    # dist/windows/dj-engine.exe
bash scripts/windows.sh --ui        # dist/windows/dj-ui.exe (Qt скачивается один раз в .cache/qt-windows)
bash scripts/windows.sh --dist      # рядом с exe: Qt, плагины, mingw-runtime, qt.conf, hauler-dj.bat, zip
bash scripts/windows.sh --all       # всё подряд
```

Переменные: `HAULER_QT_VERSION` (по умолчанию 6.8.1), `HAULER_QT_ROOT` (кэш Qt), `HAULER_ORT_DLL` (путь к
`onnxruntime.dll`, если он нужен — без него движок считает темп встроенным детектором).

## Что внутри дистрибутива

`dj-engine.exe`, `dj-ui.exe`, библиотеки Qt и mingw-runtime, `platforms/`, `styles/`, `imageformats/`,
`qml/`, `qt.conf`, `hauler-dj.bat` (поднимает движок, затем оболочку), `README.txt`.

## Отличия от Linux-версии

- **Канал.** На Linux — сокет `$XDG_RUNTIME_DIR/hauler-dj-engine.sock` (иначе `/tmp/hauler-dj-engine.sock`).
  На Windows — именованная труба `\\.\pipe\hauler-dj-engine-<пользователь>`; оболочка передаёт
  `QLocalSocket` только имя, префикс добавляет движок.
- **Каталоги.** Настройки и зеркало метаданных — `%APPDATA%\hauler-dj`; кэши (форма волны, PCM) —
  `%LOCALAPPDATA%\hauler-dj`. Правила одинаковые в коде: `crates/dj-paths`.
- **Звук.** Linux — PipeWire (cpal с фичей `pipewire`); Windows — WASAPI по умолчанию у cpal.
- **Приоритет аудиопотока (MMCSS)** пока не выставлен: первая версия играет, но под нагрузкой возможны
  щелчки. Это отдельная работа, не часть порта.
- **Установщика нет:** дистрибутив — zip с папкой.

## Границы

Проверок под Windows нет: приёмка — сборка exe и запуск `--help`. Единственный гейт проекта — существующий
`scripts/check.sh` на Linux; он должен оставаться зелёным, потому что Linux-путь порт не меняет.

## Состояние сборки (2026-11-18)

Собирается всё:

```
bash scripts/windows.sh --all
```

- `dist/windows/dj-engine.exe` — PE32+, x86-64, около 21 МБ, консольный;
- `dist/windows/dj-ui.exe` — PE32+, x86-64, около 5 МБ, консольный (`WIN32_EXECUTABLE OFF` не менялся);
- `dist/windows/hauler-dj-windows.zip` — 124 МБ: оба exe, Qt-библиотеки, плагины (`platforms` с `qwindows.dll`),
  дерево `qml`, рантайм mingw (`libstdc++-6.dll`, `libgcc_s_seh-1.dll`, `libatomic-1.dll`), `qt.conf`,
  `hauler-dj.bat`, `README.txt`.

Linux собирается тем же деревом, что и раньше: `cargo build --workspace` и `scripts/ui.sh --build-only` проходят,
`CXXFLAGS` в dev shell не задан, поэтому C++-сборка крейтов идёт прежними флагами.

### Что пришлось решить в окружении хоста

1. **Сборочные скрипты — программы хоста.** rustc линкует их своим lld через папку `gcc-ld` внутри тулчейна; в
   nix-шелле она пуста. Скрипт кладёт туда ссылку на `ld.lld` из nix — ничего вне тулчейна не трогается.
2. **C++-крейты читали только `CXX`.** Для кросс-сборки cargo даёт `CXX_x86_64_pc_windows_gnu`, а `CXX` — это
   хост. Теперь оба `build.rs` сначала смотрят переменную цели; иначе в библиотеку для Windows попадали
   хостовые объекты, и линкер говорил только «символы не найдены».
3. **`libpthread.a` в mingw из nixpkgs нет** (потоки собственные). chromaprint просит это имя — пустой архив в
   кэше сборки удовлетворяет линкер.
4. **mingw в nix собран с mcF-потоками.** Часть `libstdc++` (`gthr.h`) живёт в `mcfgthread`, а заголовки и
   библиотека — в отдельных выходах замыкания компилятора. Скрипт находит их через `nix-store -qR` и передаёт
   в `CXXFLAGS_<target>` и `RUSTFLAGS`. Тот же путь нужен CMake (`-DCMAKE_CXX_FLAGS`, `-DCMAKE_EXE_LINKER_FLAGS`),
   иначе не проходит даже пробная компиляция: mingw добавляет `-lmcfgthread`.
5. **Инструменты Qt для хоста.** Официальная Linux-сборка Qt — обычные бинарники под FHS: они ждут
   `/lib64/ld-linux-x86-64.so.2`, которого на NixOS нет. `scripts/windows.sh` собирает FHS-окружение
   (`buildFHSEnv` с нужными библиотеками) и заменяет каждый инструмент в `bin/` и `libexec/` обёрткой, которая
   запускает настоящий файл внутри этого окружения. Windows-копии инструментов не запускаются на хосте вообще,
   поэтому Qt обязан брать хостовые.
6. **`QT_HOST_PATH` сам по себе не переключает инструменты.** Нужен ещё `QT_HOST_PATH_CMAKE_DIR` — иначе Qt
   берёт `qmlimportscanner.exe` из Windows-набора и падает с «permission denied».
7. **Рантайм mingw.** Qt из aqt собран другим mingw и своих `libstdc++-6.dll` не приносит; `--dist` берёт их из
   того же тулчейна, которым собран exe.

### Чего в песочнице проверить нельзя

Запуск на настоящей Windows: два процесса, именованная труба, WASAPI, чтение `%APPDATA%`. В дистрибутиве нет
`onnxruntime.dll` (Windows-сборка ORT не скачивалась) — движок работает на встроенном детекторе темпа.
