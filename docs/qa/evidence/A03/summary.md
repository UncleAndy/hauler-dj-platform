# A03 — две деки, transport, hotcue, loop, состояние для дисплеев

- **Задача:** A03 (owner: audio-core)
- **Дата:** 2026-11-05
- **Требования:** DJP-DECK-001 … 004; DJP-ARCH-004 (расширение `state-v1` по ADR-0006)
- **Артефакты:** `test-results.txt`, `two-deck-mix.wav`

## Что сделано

| Файл | Содержание |
|---|---|
| `crates/dj-domain/src/lib.rs` | `state-v1` расширен: `DeckLoad` (`Empty/Loading/Ready/Failed`), `TrackMeta`, `Levels`, поля `remaining_frames`, `position_ratio`, `track`, `loop_region`, `hotcues[8]`, `levels` — в порядке приоритета отображения (ADR-0006) |
| `crates/dj-deck/src/lib.rs` | `Deck`: transport, seek, loop с переносом на точном сэмпле, 8 hotcue, gain, уровни, снимок для дисплеев; `DeckSource` — граница, благодаря которой дека не декодирует |
| `apps/dj-headless` | режим `mix`: JSON-сценарий (деки, gain, команды по сэмплам) → стерео WAV, детерминированно |

## Acceptance

| Пункт | Чем проверено | Результат |
|---|---|---|
| Две деки рендерятся вместе, headless | `tests/mix.rs::two_decks_render_together_with_commands_on_their_samples`: 88 200 кадров, оба файла звучат, вторая половина громче (дека B вступает на середине) | ✅ |
| Loop на точном сэмпле | `deck::a_loop_wraps_at_the_exact_sample`: кадр 3000 = источник 1000, позиция после блока 2000, ровно один перенос | ✅ |
| 8 hotcue: установка, переход, удаление | `deck::eight_hotcues_are_independent`, `a_hotcue_jumps_to_the_stored_frame` | ✅ |
| Состояние для дисплеев | `deck::a_snapshot_carries_everything_a_display_needs`: remaining, ratio, hotcues[8], track, levels | ✅ |
| Формат `state-v1` не поехал молча | `dj-domain/tests/wire_format.rs` обновлён: golden-строка снимка содержит все новые поля — изменение протокола видно в диффе | ✅ |
| Ошибка загрузки — состояние, а не пустая дека | `deck::a_failed_load_is_a_state_not_a_silent_empty_deck` | ✅ |
| Пустое чтение — underrun, не паника | `deck::an_empty_read_is_an_underrun_not_a_panic` | ✅ |
| Детерминизм микса | `tests/mix.rs::the_same_mix_renders_identical_bytes` (два прогона — побайтово равны) | ✅ |
| Частота файла ≠ частоте рендера — отказ, а не ускорение | `tests/mix.rs::a_rate_mismatch_is_refused_instead_of_silently_sped_up` | ✅ |

Всего в воркспейсе: **103 теста, 0 упавших**.

## Семантика петли (зафиксирована кодом и тестом)

- Активная петля срабатывает, когда плейхед доходит до её конца, и повторяет `start..end`.
- **Установка петли не двигает плейхед.** Область целиком позади плейхеда игнорируется, пока seek не вернёт его внутрь — это важнее удобства: «включил loop» не должно перескакивать трек.
- Перенос происходит на точном сэмпле: чтение блока клампится на `region.end`, затем seek на `start`.

## Что появилось в контракте

`state-v1` теперь несёт всё, что просил ADR-0006 для экранов контроллера: состояние загрузки и прогресс, метаданные трека, remaining и позицию, loop, hotcues, уровни. Это сделано **до** A09 — иначе драйверу S4 пришлось бы ломать протокол.
