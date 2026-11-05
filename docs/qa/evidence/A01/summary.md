# A01 — RT-контракт, очередь команд, детерминированный headless-рендер

- **Задача:** A01 (owner: audio-core)
- **Дата:** 2026-11-05
- **Требования:** DJP-ARCH-001, -002, -003, -005; DJP-NFR-001
- **Артефакты:** `test-results.txt`, `sine-440-4800.wav`, `play-stop-4800.wav`

## Acceptance из backlog — по пунктам

| Пункт | Как проверено | Результат |
|---|---|---|
| 440 Гц рендерится в детерминированный WAV с точным числом кадров | `dj-headless sine --frames 4800` → 9644 байта = 44 + 4800×2; два прогона побайтово равны (`two_renders_of_one_scenario_are_byte_identical`) | ✅ |
| Play и Stop применяются в назначенном сэмпле | `play-stop.json`: `Play` на 1000, `Stop` на 4000; в WAV сэмплы `[0,1000)` и `[4000,4800)` нулевые, между ними — сигнал; отчёт печатает `applied_at_sample` = 1000 и 4000 | ✅ |
| Инструментированный callback не аллоцирует и не берёт локов | `rt_contract::processing_a_block_does_not_allocate` (200 блоков внутри `RtScope` → 0 аллокаций) + `no_forbidden_primitives` (в `src` ядра нет `Mutex`/`RwLock`/`Condvar`/`OnceLock`/`thread::*`/`println!`/`std::fs`) | ✅ |

## Что появилось в коде

| Файл | Что это |
|---|---|
| `crates/dj-domain/src/lib.rs` | `command-v1`/`state-v1`: `DjCommand`, `ScheduledCommand`, `Ack`/`AckStatus`/`RejectReason`, `Snapshot`, `SourceFrame`/`FrameRange`, классификация critical/coalesced, serde |
| `crates/dj-audio-core/src/queue.rs` | фиксированная очередь: 32 критических слота + 12 coalesced (ключ × дека), без аллокаций после создания |
| `crates/dj-audio-core/src/engine.rs` | `process(&mut [f32])` — блок режется на границе команды, команда применяется точно в своём сэмпле |
| `crates/dj-audio-core/src/sine.rs` | 440 Гц генератор с непрерывной фазой; `set_frequency` для tempo-ratio |
| `crates/dj-audio-core/src/wav.rs` | PCM16 WAV, детерминированные байты |
| `crates/dj-audio-core/src/bench.rs` | замер времени каждого `process`, p50/p99/max |
| `crates/dj-testkit/src/rt.rs` | `CountingAllocator` + `RtScope` — единственное место с `unsafe`, вне workspace-линтов |
| `apps/dj-headless/` | `sine` и `scenario` (JSON), JSON-отчёт, exit-коды 0/2/3 |

## Проверки (все зелёные)

```text
dj-domain            5 юнит + 8 wire_format (формат протокола запинен побайтово)
dj-audio-core       18 юнит + 3 rt_contract + 1 no_forbidden_primitives
dj-testkit           1 тест
dj-headless          3 юнит (scenario) + 7 CLI
```

`cargo clippy --workspace --all-targets -- -D warnings` — чисто, `cargo fmt --check` — чисто.

Полный гейт: `BUILD_DIR=/tmp/dj-ui-build bash scripts/check.sh` → `check.sh: all gates passed`,
63 теста, 0 упавших. `BUILD_DIR` нужен потому, что каталог `build/` в этом общем checkout
принадлежит другому uid (см. README, раздел Quickstart).

## Время блока (офлайн, эта машина — НЕ эталонный стенд)

48 кГц, блок 256 кадров, бюджет периода 5333,3 мкс, 188 блоков:

| Метрика | Значение | Доля периода |
|---|---|---|
| p50 | 2,605 мкс | 0,05 % |
| p99 | 6,269 мкс | 0,12 % |
| max | 17,195 мкс | 0,32 % |

Это ответ на вопрос «сколько стоит наш callback сейчас», а не подтверждение DJP-NFR-003: тот
требует живого 2-часового прогона на эталонной машине (A11).

## Отклонения от плана

- Формат сценария — **JSON**, не YAML: в плане пример `tests/scenarios/two-deck.yaml`, но YAML
  потянул бы новую зависимость ради тестового артефакта, а `serde_json` уже в дереве через
  контракты. Имя и назначение файла сохранены, расширение другое.
- `Missing docs` включается не во всём workspace, а только в `dj-domain`: это протокол, где имя
  поля — часть контракта. Для DSP-внутренностей это шум, который мешает читать код.
