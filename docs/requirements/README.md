# Требования и traceability

`traceability.csv` сгенерирован из ТЗ (`DJP-SRS-001`) на этапе A00: 136 строк, по одной на каждый
идентификатор `DJP-*`, кроме `DJP-SRS-001` и `DJP-TASK-000` (это идентификаторы документа и
шаблона задачи, а не требования).

## Колонки

`req_id, owner, priority, release, source, implementation_path, test_id, test_status, evidence_link, reviewer, date`

- `priority` / `release` заполнены там, где ТЗ их задаёт (`[R0, P0]` или таблица `P0 / R0`).
  Пустое значение означает «в ТЗ не назначено», а не «P3».
- `owner` — роль из плана §6.1, а не человек.
- `implementation_path` / `test_id` заполняются при старте задачи.
- `reviewer` — человек; агент не ставит `verified` самому себе.

## Статусы

`unverified → review → verified`; также `partial`, `blocked-external`, `hardware-not-verified`,
`rejected`, `deferred` (план §8).

- `unverified` — требование заведено, реализации/доказательств нет.
- `review` — реализация и свидетельства есть, ждёт человека-ревьюера.
- `verified` — ревьюер подтвердил; только после фактического прогона.
- `partial` / `hardware-not-verified` — рабочий статус матрицы паритета и S4; **не** закрывает
  требование.

## Гейт

`bash scripts/check-docs.sh` падает, если в ТЗ появился `DJP-*`, которого нет в CSV, или у строки нет
владельца. CI вызывает его в отдельной job без Nix.
