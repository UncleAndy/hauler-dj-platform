# Architecture Decision Records

Формат: `NNNN-<тема>.md`, статусы `proposed → accepted / rejected / superseded`.
Решение по умолчанию — `proposed`: агент не выбирает за владельца продукта.

| ADR | Тема | Статус | Блокирует |
|---|---|---|---|
| [0001](0001-license.md) | Лицензия продукта и политика third-party | **accepted** (MIT OR Apache-2.0) | — (GPL-компоненты запрещены) |
| [0002](0002-audio-backend.md) | Основной аудиобэкенд | **accepted**: CPAL 0.18 + нативный PipeWire-бэкенд | — |
| [0003](0003-ui-bridge.md) | Мост QML ↔ Rust: in-process или IPC | **accepted**: отдельный UI-процесс | — |
| 0004 | Эталонная версия Traktor и допустимые отклонения | не начат | закрытие R0 |
| [0005](0005-audio-devices-and-routes.md) | Модель аудиоустройств и маршрутов | **accepted**: шины → route plan, идентичность, калибровка | — (`dj-audio-io`, A10) |
| [0006](0006-controllers.md) | Контроллеры: транспорт — драйвер — профиль — состояние экрана | **accepted**: статические драйверы, состояние+события, clean-room | A09, CTRL-005 |
| 0007 | Безопасность моторов S4 MK3 | не начат | включение моторов |
| [0008](0008-pcm-cache-policy.md) | Политика PCM-кэша по форматам | **accepted**: lossless — потоково, lossy — в PCM-кэш при загрузке | A02 |
| [0009](0009-gapless-loops.md) | Петля без провала на переходе | **proposed**: буфер петли до 30 с, длинные — как сейчас | A06, DJP-DECK-004 |
| [0010](0010-realtime-scheduling-and-xruns.md) | RT-приоритет, размер блока и счётчик xrun | **proposed**: RT — из упаковки, приложение проверяет и предупреждает | A05, A06, A11 |
| [0011](0011-ui-deck-compact-layout.md) | Компактная дека: раскладка, скрытие ряда под замком, глобальные Snap/Quant | **proposed** | — (DJP-DECK-002/005/006) |
