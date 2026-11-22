# Ревью требований: что запускать по шагам

`review` в трассировке значит «реализация и свидетельство есть, нужен человек». Ниже — ровно те команды, которые
закрывают этот пункт, в том порядке, в котором их удобно выполнять. Всё запускается из корня репозитория.

Общее правило: **сначала прогон, потом отметка**. Скрипт отметки не даст закрыть строку, которая не в `review`, и
печатает, что было до и что стало.

## Шаг 0. Посмотреть, что ждёт вердикта

```bash
bash scripts/verify-requirement.sh --list
```

Ожидается: `ждут вердикта: N` и список из `ID`, теста и свидетельства. Сейчас N = 43.

## Шаг 1. Один прогон, который покрывает почти всё

```bash
bash scripts/check.sh
```

Девять минут, в конце должно быть ровно:

```
check.sh: all gates passed
```

Этот прогон внутри себя делает `cargo test --workspace`, `scripts/ui.sh --smoke`, `scripts/ui.sh --engine-check`,
`launcher-check`, `curve-check`, `fx-check`, `controller-check` и `check-docs`. Поэтому 36 из 43 строк `review`
проверяются именно им — отдельные команды для них не нужны.

Если вместо этого увидите `FAILED` или код возврата не 0 — ничего не отмечать и прислать мне вывод.

## Шаг 2. Отметить то, что покрыто гейтом

Сначала посмотреть список — без флага `--yes` скрипт **ничего не меняет**:

```bash
bash scripts/verify-requirement.sh --batch-gate "Андрей Великоредчанин"
```

Проверить, что в списке только те строки, у которых `test_id` — `cargo test …`, `bash scripts/…` или `ui.sh …`.
Затем закрыть:

```bash
bash scripts/verify-requirement.sh --batch-gate "Андрей Великоредчанин" --yes
```

Ожидается: `отмечено verified: N, ревьюер Андрей Великоредчанин, ГГГГ-ММ-ДД`.

## Шаг 3. Восемь строк, которые подготовлены отдельно

Это те, где свидетельство свежее и проверяется одной командой. Что именно вы подтверждаете — в
`docs/requirements/review.md`. Коротко:

| требование | команда | что должно быть |
|---|---|---|
| DJP-DECK-009 | `bash scripts/ui.sh --engine-check` | `dj-ui: four decks play and mix — each one moves, each one is heard on its own…` |
| DJP-MIX-006 | `cargo test -p dj-mixer --test mixer` | `test result: ok. 15 passed` |
| DJP-NFR-008 | тот же прогон | тест `NaN fader must be ignored` в числе пятнадцати |
| DJP-GOAL-005 | `grep -n "Qt" docs/architecture/crate-map.md` | `apps/dj-ui` — единственный потребитель Qt, у `dj-audio-core` его нет |
| DJP-GOAL-006 | `bash scripts/check-docs.sh` | `traceability: every DJP-* requirement has an owned row, every row has 11 fields` |
| DJP-LIC-005 | `grep -in "qt" THIRD_PARTY.md` | строка про Qt с лицензиями LGPL-2.1+/LGPL-3.0/GPL-2.0+/GPL-3.0 |
| DJP-ENV-004 | `grep -rn "ureq" --include=*.rs crates apps` | только `crates/dj-models` — загрузка модели удара по команде; воспроизведению сеть не нужна |
| DJP-NFR-013 | `grep -rniE "sentry|analytics" --include=*.rs crates apps` | пусто; `telemetry` в коде — внутренние счётчики `CueTelemetry` |

После каждого прогона, если всё сошлось:

```bash
bash scripts/verify-requirement.sh DJP-MIX-006 "Андрей Великоредчанин"
bash scripts/verify-requirement.sh DJP-NFR-008 "Андрей Великоредчанин"
```

и так далее по списку.

## Шаг 4. Семь строк, где проверять надо глазами, а не командой

`DJP-LIC-001`, `DJP-LIC-002`, `DJP-LIC-003`, `DJP-LIC-004`, `DJP-LIC-006`, `DJP-ENV-001`, `DJP-ENV-003` — это
политики (лицензии, закреплённый тулчейн), а не поведение. Читать: `docs/adr/0001-license.md`, `THIRD_PARTY.md`,
`flake.nix`. Если согласны с написанным — отмечать тем же скриптом по одному.

## Шаг 5. Убедиться, что файл не сломан

```bash
bash scripts/check-docs.sh
```

Ожидается последняя строка: `traceability: every DJP-* requirement has an owned row, every row has 11 fields`.

## Если что-то не сошлось

Ничего не отмечать и оставить строку в `review` — это честный результат: напишите мне, что именно не совпало, и
разбор пойдёт отдельной задачей. Вернуть строку в `unverified` можно вручную в
`docs/requirements/traceability.csv` (поле `test_status`), убрав заодно имя ревьюера и дату.
