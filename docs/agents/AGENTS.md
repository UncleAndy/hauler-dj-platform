# Правила работы ИИ-агентов

Источник: ТЗ §11, план §6. Правила обязательны для любого агента (Hermes, Claude Code, Codex и др.).

## Обязательное

1. Каждая задача привязана к существующим `DJP-*`, имеет входы, выходы, ограничения, тесты и
   признак `hardware_needed`.
2. Публичные интерфейсы, формат хранилища и схемы не меняются без принятого ADR и плана миграции.
3. Запрещено добавлять пакет, копировать код или ресурсы Traktor/NI/Mixxx, включать GPL-код без
   license review (DJP-LIC-001…006).
4. Один RT-critical крейт не редактируется одновременно двумя агентами: см. владельцев в
   `docs/architecture/crate-map.md`.
5. `unwrap`, `panic`, блокирующий лог и аллокация в audio callback — дефект, а не стиль.
6. Отчёт обязан соответствовать фактически выполненным командам. Аппаратный тест без устройства
   помечается `hardware-not-verified`, а не `passed`.
7. Merge в `main` для DSP, лицензий, файлового ввода и аппаратных команд требует человека.
8. Если данных нет — создаётся ADR/исследовательская задача со статусом `unknown`, а не выдуманная
   спецификация протокола.
9. Эталонные аудиофайлы (`crates/dj-codec/tests/fixtures/audio/`) не перезаписываются «чтобы тест
   прошёл». Их хэши лежат в `audio.sha256`; изменение файла — отдельный коммит с причиной и
   обновлённым манифестом, иначе гейт падает (`scripts/check-fixtures.sh`).
10. Строка в `traceability.csv` — обещание: `passing` без теста или команды и `passing`/`review` без
    существующего файла свидетельств валят `scripts/check-docs.sh`. `unverified` — законный статус,
    выдуманный `passing` — нет.

## Рабочий цикл

```bash
nix develop                      # окружение закреплено flake.lock
bash scripts/check.sh                 # фикстуры + fmt/clippy/tests + traceability + UI smoke
bash scripts/ui.sh --smoke            # только UI-гейт
```

1. Ветка `feat/<task>-<кратко>`, scope файлов — из задания.
2. Сначала контракт и тесты, затем реализация (contract-first).
3. `bash scripts/check.sh` зелёный **до** отчёта; ослаблять проверки запрещено.
4. Свидетельства — в `docs/qa/evidence/<task>/` в формате из `docs/qa/evidence/README.md`.
5. Обновить `docs/requirements/traceability.csv` (путь реализации, test_id, статус).
   Сценарий headless-прогона — JSON в `apps/dj-headless/tests/scenarios/`: пример из плана
   (`two-deck.yaml`) устарел, YAML-парсер ради тестового артефакта не добавляется.
6. Коммит — одна задача; push и merge делает человек.

## Шаблон задания

```yaml
id: A02
name: "Codec и PCM prefetch"
requirements: ["DJP-MEDIA-001", "DJP-MEDIA-002"]
release: R0
priority: P0
owner_role: media
inputs: ["docs/architecture/rt-contract.md"]
deliverables: ["crates/dj-codec/src/lib.rs", "crates/dj-codec/tests/"]
interfaces_to_preserve: ["Command Protocol v1"]
acceptance:
  - "cargo test -p dj-codec проходит"
  - "RT читает только предзаполненный ring"
  - "EOF и битый файл не приводят к панике"
blocked_by: []
hardware_needed: false
license_review_needed: true
status: proposed
```
