# Схемы протоколов

Каталог зарезервирован под контракты ТЗ §6.2. Файлы появятся в A01 (command/state) и A08 (session),
controller-profile — в A09.

| Файл | Протокол | Задача | Статус |
|---|---|---|---|
| `command-v1.json` | Command Protocol v1: команда, ID, source, epoch, priority, effective sample, ack | A01 | не создан |
| `state-v1.json` | State Snapshot v1: revision, sample clock, deck/mixer/FX/meters, device info | A01 | не создан |
| `session-v1.json` | Session Format v1: состояние между запусками — устройство, микшер, деки | A08 | создан |
| `djmeta-v1.json` | Track Metadata v1: fingerprint, cue в кадрах, beatgrid, петли, gain, key, заметки | A08 | создан |
| `controller-profile-v1.json` | Controller Profile Schema v1: capabilities, bindings, LED/screen, safety | A09 | не создан |

Правила: у каждой схемы версия в имени; несовместимая версия даёт явную ошибку; тесты
backward/forward compatibility обязательны; изменение `command-v1` без решения интегратора —
блокирующий PR (план §8).
