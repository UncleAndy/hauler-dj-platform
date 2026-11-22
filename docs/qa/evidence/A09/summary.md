# A09 — каркас драйвера контроллера: что сделано и чем это подтверждено

Задача A09 требует «события сняты с физического S4 MK3». Устройства на столе нет, поэтому закрытой она не
считается. Здесь — то, что сделано **без железа**, и то, что осталось.

## Что сделано

| Часть | Где | Чем подтверждено |
|---|---|---|
| Транспорт HID: перечисление узлов, чтение репортов | `crates/dj-hid` | `cargo test -p dj-hid`: разбор `uevent`, отсутствие узлов без ошибки, записанный транспорт отдаёт репорты по одному |
| События, биты репорта, счётчик джога с переполнением, слои | `crates/dj-controller-core` | `cargo test -p dj-controller-core`: 17 тестов, включая переход через `2^32` вперёд и назад |
| Драйвер S4 MK3: репорт джогов, кадр экрана, цвета LED, отказ моторов | `crates/dj-controller-s4mk3` | `cargo test -p dj-controller-s4mk3`: 12 тестов, включая точные байты шапки и подвала кадра |
| Служба: `--status`, `--list`, `--capture`, и первая строка — «выключено» | `apps/dj-controller` | `cargo test -p dj-controller`, `bash scripts/controller-check.sh` |
| Выключатель в окне «Settings» → «Controller» | `settings/SettingsWindow.qml` | `--settings-window-check`: «the controller is off until asked for, and the switch writes it: on, then off again» |
| Настройка в файле, по умолчанию выключена | `crates/dj-config` | `cargo test -p dj-config`: `[controller] enabled = false` в свежем файле, круговой обход через TOML |
| Лаунчер запускает службу только по настройке | `apps/dj-launcher` | `bash scripts/launcher-check.sh` (запуск без службы не меняется) + `controller-check.sh` |
| Таблица «орган → команда» и путь до движка | `crates/dj-controller-core/src/mapping.rs`, `apps/dj-controller --replay` | `cargo test -p dj-controller-core` (23 теста) + `controller-check.sh`: `deck_b.sync=1` → `Applied { at_sample: … }` от живого движка |

## Свидетели

```
dj-controller: controller support is off in the settings; nothing to do
dj-controller: controller support is on; 0 HID nodes, 0 of them Traktor Kontrol S4 MK3
dj-ui: the controller is off until asked for, and the switch writes it: on, then off again
controller-check: off by default, on when asked, off again; nothing is opened or sent while it is off
```

## Что осталось (и требует железа или решения)

1. Карта репортов `1` и `2`: снимается `--capture` по одному действию, каждое значение подтверждается дважды,
   после чего `public-source` в `protocol-notes.md` меняется на `observed`.
2. Дисплеи: нужен доступ к bulk (на Linux правило `udev` на `/dev/bus/usb`, на Windows WinUSB) и решение, что
   показывать.
3. Моторы: запрещены до отдельного ADR о безопасности.
4. Команда для cue-точки в контракте движка отсутствует — таблица привязок поэтому без `cue`; это работа движка,
   не маппинга.
5. Живой замер задержки «орган → команда» — на устройстве: путь уже пройден на записанных событиях.
