# Обзор архитектуры

Источник: ТЗ §4. Здесь — рабочий конспект и текущее состояние каркаса, чтобы новый участник
(человек или агент) не начинал с чтения 78 КБ спецификации.

## Компоненты и процессы

```text
Qt/QML UI ──► bridge/gateway ──► bounded RT command queue ──► audio engine (RT thread)
     ▲                                                              │
     └──────────── read-only snapshot / metering ◄──────────────────┘
                        ▲                        audio device adapter (ALSA/PipeWire)
Controller (MIDI/HID) ──┘
Media workers (decode, analyse, prefetch) ──► cache ──► audio engine
Storage workers (SQLite, recording)   ML worker (stems, отдельный процесс)
```

- **RT thread** — только граф на фиксированных блоках и заранее выделенные буферы.
- **Engine process** (`apps/dj-engine`) — владеет аудиоустройством и живёт независимо от UI (ADR-003):
  падение или перезапуск UI не останавливает воспроизведение.
- **UI process** (`apps/dj-ui`) — QML, сцена, рендеринг; клиент движка, не участвует в критическом
  пути звука; общение только командами и снимками по версионированному локальному протоколу.
- **Controller service** (`apps/dj-controller`) — отдельный процесс: транспорт (HID/MIDI) → драйвер
  устройства → привязка → те же команды движку; сюда же рендер экранов устройства (ADR-0006). Не в
  аудио (USB I/O блокирует) и не в UI (перезапуск UI не должен терять контроллер).
- **Media/storage/ML workers** — всё, что может блокировать: диск, сеть, модели.

## Граф обработки (DJP-ARCH-006)

`source buffer → resampler → time-stretch/keylock → deck gain → (deck FX) → EQ/filter →
channel gain/fader/crossfader → master processing/limiter → routed outputs`

Pre/post-fader и send-маршрутизация задаются конфигурацией, а не зашиты в линию.

## Что уже есть в репозитории

| Слой | Состояние |
|---|---|
| Rust workspace (13 крейтов) | каркас: границы и владельцы зафиксированы, логики нет |
| Qt/QML оболочка | пустая двухдековая сцена, запускается под offscreen |
| flake.nix / rust-toolchain | закреплено: Rust 1.95.0, Qt 6.11.2, CMake 4.x |
| CI | fmt, clippy, тесты, сборка UI, покрытие traceability |
| Контракты Command/State | `command-v1`/`state-v1` в `dj-domain`, формат запинен golden-тестами (A01) |
| Контроллеры, дисплеи | модель решена (ADR-0006), кода нет — A09 |
| Модель аудиоустройств | решена (ADR-0005), `dj-audio-io` не написан |
| Аудиобэкенд, SQLite, декодеры | не начаты — A02/A05/A08 |

## Открытые решения

ADR-001 (лицензия), ADR-002 (backend), ADR-003 (UI-мост), ADR-0005 (аудиоустройства и маршруты),
ADR-0006 (контроллеры и экраны) — см. `docs/adr/`. Открыты: ADR-0004 (эталон Traktor) и ADR о
безопасности моторов S4.
