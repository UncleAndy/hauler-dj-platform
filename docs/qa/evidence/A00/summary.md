# A00 — репозиторий и воспроизводимая среда

- **Задача:** A00 (owner: architect/devops), зависимости: нет
- **Требования:** DJP-ENV-001, DJP-ENV-003
- **Дата:** 2026-11-04
- **Окружение:** NixOS 26.05.20261007.2efa67f, ядро 6.18.55, агент `hermes` через SSH-bwrap
- **Артефакты:** `test-results.txt` (фактические команды и вывод), `limitations.md`

## Что создано

| Область | Файлы |
|---|---|
| Rust workspace | `Cargo.toml`, `Cargo.lock`, 13 крейтов в `crates/`, `apps/dj-headless`, `apps/dj-cli` |
| Линты/правила | `[workspace.lints]` (unsafe deny, unsafe-блоки задокументированы, unwrap/expect warn), ed. 2024, MSRV 1.95 |
| Среда | `flake.nix`, `flake.lock`, `rust-toolchain.toml`, `.envrc`, `.gitignore` |
| Qt/QML | `CMakeLists.txt`, `apps/dj-ui` (C++ + QML, двухдековая пустая сцена), `--smoke` режим |
| Гейты | `scripts/check.sh`, `check-rust.sh`, `check-docs.sh`, `ui.sh`, `.github/workflows/ci.yml` |
| Документы | `README.md`, `CONTRIBUTING.md`, `SECURITY.md`, `THIRD_PARTY.md`, `docs/**` (архитектура, ADR-001…003, traceability, QA, правила для агентов) |

## Фактическая проверка

| Проверка | Результат |
|---|---|
| `nix develop` | собирается, Rust 1.95.0, Qt 6.11.2, CMake 4.1.6, qmllint/qmlformat доступны |
| пин тулчейна | `rustc 1.95.0` совпадает с `rust-toolchain.toml` (проверяется скриптом) |
| `cargo fmt --all -- --check` | без диффа |
| `cargo clippy --workspace --all-targets -- -D warnings` | без предупреждений (15 пакетов) |
| `cargo test --workspace` | 6 тестов, 0 failed (3 `dj-headless`, 3 `dj-cli`) |
| сборка Qt/QML | `dj-ui` линкуется, `--smoke` под `QT_QPA_PLATFORM=offscreen` → «QML scene loaded, no Qt warnings», exit 0 |
| traceability | 136 требований `DJP-*`, у каждой строки есть владелец; гейт `check-docs.sh` зелёный |
| `bash scripts/check.sh` (из обычного shell) | exit 0 |
| готовность зависимостей A02/A05 | scratch-крейт с `cpal 0.16.0` (alsa-sys 0.3.1), `symphonia 0.5.5`, `rusqlite 0.32.1 (bundled)` собирается и запускается в этом shell |

## Отклонения от ТЗ/плана (осознанные)

1. **Лицензия.** В репозитории уже лежал `LICENSE` = MIT; ТЗ фиксирует `LICENSE: TBD` и запрещает
   назначать лицензию за владельца. Файл не менялся, факт зафиксирован в `docs/adr/0001-license.md`.
2. **Крейты.** Созданы только те, что нужны M0/R0 (13). `dj-fx`, `dj-time-stretch`, `dj-sync`,
   `dj-stems`, `dj-remix`, `dj-pattern`, `dj-controller-s4mk3`, `dj-recording`, `services/stem-worker`
   описаны в `docs/architecture/crate-map.md` как planned: пустые заглушки под R2-компоненты создали бы
   видимость структуры без ответственности (ТЗ §10).
3. **UI-мост не выбран.** `apps/dj-ui` — C++/QML без Rust-линковки: ADR-003 не принят, поэтому нет ни
   C ABI, ни IPC (ТЗ §15).
4. **Скрипты вызываются как `bash scripts/…`**: на NixOS нет `/usr/bin/env`, shebang оставлен для CI.
5. **`glibc.dev` убран из `buildInputs`** flake: он попадал в `NIX_CFLAGS_COMPILE` перед libstdc++ и
   ломал `#include_next <stdlib.h>` в C++ (`stdlib.h: No such file`). bindgen получает заголовки через
   `BINDGEN_EXTRA_CLANG_ARGS`. Причина записана комментарием в `flake.nix`.
6. **`traceability.csv` сгенерирован механически** из ТЗ: `priority`/`release` пусты там, где ТЗ их не
   задаёт; `owner` — роль из плана §6.1; у `DJP-LIC-*` приоритет в ТЗ не указан.

## Что дальше

A01 (контракты Command/State + RT-ядро + headless sine), A12 (ADR-001), затем A02/A05/A08/A11.
