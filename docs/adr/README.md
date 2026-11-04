# Architecture Decision Records

Формат: `NNNN-<тема>.md`, статусы `proposed → accepted / rejected / superseded`.
Решение по умолчанию — `proposed`: агент не выбирает за владельца продукта.

| ADR | Тема | Статус | Блокирует |
|---|---|---|---|
| [0001](0001-license.md) | Лицензия продукта и политика third-party | proposed | любой GPL-dependent merge (A12) |
| [0002](0002-audio-backend.md) | Основной аудиобэкенд: CPAL или прямой PipeWire/JACK | proposed | завершение A05 |
| [0003](0003-ui-bridge.md) | Мост QML ↔ Rust: in-process или IPC | proposed | стабилизацию A06 |
| 0004 | Эталонная версия Traktor и допустимые отклонения | не начат | закрытие R0 |
| 0007 | Безопасность моторов S4 MK3 | не начат | включение моторов |
