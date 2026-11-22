# Пакеты 0.1.0: что проверено и чем

Дата: 22.11. Артефакты: `dist/packages`.

| файл | размер | sha256 |
|---|---|---|
| `hauler-dj_0.1.0_amd64.deb` | 233 028 864 Б | `be5ee4b6b5226ff90936939b5b03ee4171599b52ddb482cd57daade8127d8d67` |
| `hauler-dj-0.1.0-1.x86_64.rpm` | 265 716 157 Б | `1129a699cf6e8b9b773871507b7cb2c937aea64901939355b8a5f647472ae416` |
| `hauler-dj-0.1.0-x86_64.AppImage` | 194 491 584 Б | `fb5152dd2d6732d21ea74a97f137a3e4730880fb5ebcf4f2fe08fd40c4ab9de1` |

## Свидетели

```
$ dpkg-deb -x hauler-dj_0.1.0_amd64.deb /tmp/deb2 && ls /tmp/deb2/usr/lib/hauler-dj/bin/
dj-controller  dj-engine  dj-ui

$ ld-linux-x86-64.so.2 --library-path <корень> <корень>/bin/dj-controller --status
dj-controller: controller support is off in the settings; nothing to do

$ QT_PLUGIN_PATH=<корень>/qtplugins QML2_IMPORT_PATH=<корень>/qml:<корень>/app-qml   ld-linux-x86-64.so.2 --library-path <корень> <корень>/bin/dj-ui --smoke
dj-ui: QML scene loaded, no Qt warnings

$ unsquashfs -o 193728 hauler-dj-0.1.0-x86_64.AppImage → тот же набор файлов, та же оболочка грузит сцену
```

## Чего не проверено и почему

1. **Установка в живую систему** (`dpkg -i`, `dnf install`): на этой машине ставить некуда, а «распаковалось» — не
   «установилось». Пути в пакете — обычные FHS, скриптов после установки нет.
2. **Запуск AppImage как файла**: его рантайм — ELF с `PT_INTERP=/lib64/ld-linux-x86-64.so.2`, которого на NixOS и на
   этой машине нет. Нужен `appimage-run` (NixOS) или обычный FHS-дистрибутив. Содержимое образа проверено.
