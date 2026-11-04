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
- **UI thread** — QML, сцена, рендеринг; не участвует в критическом пути звука.
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
| Контракты Command/State/Session | не зафиксированы — A01 |
| Аудиобэкенд, SQLite, декодеры | не начаты — A02/A05/A08 |

## Открытые решения

ADR-001 (лицензия), ADR-002 (backend), ADR-003 (UI-мост) — см. `docs/adr/`.
