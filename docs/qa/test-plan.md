# План тестирования

Источник: ТЗ §8, план §7. Здесь — что уже действует в каркасе и что появится с задачами.

## Действует сейчас

| Уровень | Команда | Гейт |
|---|---|---|
| Форматирование | `cargo fmt --all -- --check` | CI + `bash scripts/check.sh` |
| Линт | `cargo clippy --workspace --all-targets -- -D warnings` | CI |
| Unit/doctest | `cargo test --workspace` | CI |
| Пин тулчейна | сравнение `rustc --version` с `rust-toolchain.toml` | CI |
| Сборка UI + загрузка QML | `bash scripts/ui.sh --smoke` (offscreen, падение на любом Qt-warning) | CI |
| Покрытие требований | `bash scripts/check-docs.sh` | CI (job без Nix) |

## Появится с задачами

- **Golden offline audio** (A01/A11): детерминированный PCM, WAV + хеш, допуски для DSP.
- **Property/fuzz** (A02, A03): битые файлы, границы loop/seek, очереди команд.
- **RT instrumentation** (A01/A11): счётчики аллокаций и блокировок в callback.
- **UI workflow** (A06): снимки состояния, раскладки, клавиатура (qmltestrunner).
- **Hardware-in-loop** (A09/A10): только с физическим устройством, иначе `hardware-not-verified`.
- **Soak 2 ч** (M3): 4 деки + FX + запись на эталонном стенде.

## Правила

1. Порог вердикта задаётся до прогона; «зелёный» тест не ослабляется правкой проверки.
2. Любой аудиодефект превращается в воспроизводимый fixture + regression test.
3. Непустой артефакт обязателен: пустой лог/файл = замер не состоялся.
