# Architecture Decision Records

Формат: `NNNN-<тема>.md`, статусы `proposed → accepted / rejected / superseded`.
Решение по умолчанию — `proposed`: агент не выбирает за владельца продукта.

| ADR | Тема | Статус | Блокирует |
|---|---|---|---|
| [0001](0001-license.md) | Лицензия продукта и политика third-party | **accepted** (MIT OR Apache-2.0) | — (GPL-компоненты запрещены) |
| [0002](0002-audio-backend.md) | Основной аудиобэкенд: CPAL или прямой PipeWire/JACK | **accepted**: прямой PipeWire | — |
| [0003](0003-ui-bridge.md) | Мост QML ↔ Rust: in-process или IPC | **accepted**: отдельный UI-процесс | — |
| 0004 | Эталонная версия Traktor и допустимые отклонения | не начат | закрытие R0 |
| 0007 | Безопасность моторов S4 MK3 | не начат | включение моторов |
