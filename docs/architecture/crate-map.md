# Карта крейтов и владения

Выход G0 требует зафиксировать владельца каждого базового крейта. Разделение — границы
ответственности, а не обязанность делать отдельный крейт на каждую функцию (ТЗ §10).

## Созданы в A00 (scaffold)

| Крейт | Владелец (роль) | Задача | Релиз | Требования |
|---|---|---|---|---|
| `dj-domain` | audio-core | A01 | R0 | DJP-ARCH-001, -002, -004; DJP-DATA-002 |
| `dj-audio-core` | audio-core | A01 | R0 | DJP-ARCH-001, -003, -005; DJP-NFR-001 |
| `dj-audio-io` | architect (audio-platform) | A05 | R0 | DJP-NFR-002, -003; DJP-ARCH-007 |
| `dj-codec` | media | A02 | R0 | DJP-MEDIA-001, -002 |
| `dj-deck` | audio-core | A03 | R0 | DJP-DECK-001…004 |
| `dj-dsp` | dsp | A04 | R0 | DJP-ARCH-006, -009; DJP-NFR-008 |
| `dj-mixer` | dsp | A04 | R0 | DJP-MIX-001; DJP-ARCH-006, -007 |
| `dj-analysis` | media | A07 | R0 | DJP-MEDIA-003 |
| `dj-library` | media | A08 | R0 | DJP-DATA-001, -003, -004, -005 |
| `dj-session` | media | A08 | R0 | DJP-ENV-006; DJP-SET-002, -003 |
| `dj-bridge` | ui | A06 | R0 | DJP-ARCH-004; DJP-UI-001, -002 |
| `dj-controller-core` | hardware | A09 | R0 | DJP-CTRL-001, -002, -004 |
| `dj-testkit` | qa | A11 | R0 | DJP-NFR-001 |

Приложения: `apps/dj-headless` (A01/A13, audio-core), `apps/dj-cli` (A08, media),
`apps/dj-ui` (A06, ui; Qt/C++ + QML, собирается CMake, вне cargo-воркспейса).

**Планируется вместе с A01/A06:** `apps/dj-engine` — процесс, владеющий аудиоустройством и живущий
независимо от UI (ADR-003). `dj-bridge` — протокол этого моста: серверная сторона в движке, клиентская
в UI; `dj-headless` поднимает тот же движок без сокета.

## Запланированы (создаются вместе с задачей)

| Крейт | Владелец | Когда |
|---|---|---|
| `dj-sync` | audio-core | M2 (DJP-SYNC-001…006) |
| `dj-time-stretch` | dsp | M2 (DJP-DECK-008) |
| `dj-fx` | dsp | M4 (DJP-FX-001…004) |
| `dj-stems` | ml | M5 (DJP-STEM-*) |
| `dj-remix` | ml | M5 (DJP-REMIX-*) |
| `dj-pattern` | ml | M5 |
| `dj-controller-s4mk3` | hardware | M3 (DJP-CTRL-003…007) |
| `dj-recording` | dsp | M2 (DJP-MIX-005) |
| `services/stem-worker` | ml | M5 |

## Правила зависимости

1. `dj-domain` не зависит ни от одного крейта репозитория; все остальные могут зависеть от него.
2. `dj-audio-core` не зависит от `dj-codec`, `dj-library`, `dj-bridge` и Qt: только контракты и
   заранее подготовленные буферы (DJP-ARCH-005).
3. Крейты GUI/бриджа не попадают в граф RT-потока. `apps/dj-ui` — единственный потребитель Qt.
4. Крейт другого владельца не редактируется без согласованного ADR (план §6.2).
