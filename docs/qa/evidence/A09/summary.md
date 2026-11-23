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
4. Живой замер задержки «орган → команда» — на устройстве: путь уже пройден на записанных событиях.
5. Удержание cue-кнопки (постановка точки) и нажатие (прыжок) на контроллере не различимы без его протокола: в
   таблице они есть как `cue_set` и `cue`, а что именно присылает S4 — снимается на железе.

## Живой контроллер, 23.11

Владелец подключил S4 MK3 и выбрал его в окне. Ниже — что из этого вышло и что нашлось.

```
# до исправления: интерфейс выбран в окне, звука нет; звук появляется, только если S4 выбран системным устройством
dj-engine: output: the system's default device        <- выбор из окна не читался вовсе
# после исправления
dj-engine: output: device matching "Traktor Kontrol S4 MK3"
device-choice-check: the settings file chooses the output; silence stays silence; an explicit name wins
```

Два дефекта, найденные на живом устройстве:

1. **Пустое имя устройства побеждало настройки.** Запускатор передаёт движку `--device ""` (он не знает названий), а
   движок читал это как «системное устройство»; поле `audio.device` на пути запуска не читалось. Отсюда «метры идут,
   звука нет». Правило выбора теперь в `dj_config::chosen_output_device`, его проверяет
   `scripts/device-choice-check.sh`.
2. **Пустая игла выбирала первое устройство.** `resolve_device` искал по подстроке без защиты от пустой строки:
   пустая содержится в любом имени. Теперь пустое имя читается как системное, как это давно делает
   `output_channels`.

```
cue-hardware-check: the window keeps the two knobs until a controller takes them; the software bus goes to unity
```

На железе видно и то, чего не видно было из документов: громкость наушников и степень микса на S4 — физические ручки,
и положения они нам не сообщают. Пока контроллер включён, экранные `PHONES` и `MIX` заперты с подсказкой «крутить на
контроллере», а программная шина наушников стоит на единице и полном мониторинге — иначе в цепочке был бы второй
регулятор, съедающий децибелы незаметно.

Что это не закрывает: карту органов (репорты `1`/`2`), экраны, моторы и замер задержки «орган → команда» — они
по-прежнему требуют съёма с устройства и решений, перечисленных выше.
