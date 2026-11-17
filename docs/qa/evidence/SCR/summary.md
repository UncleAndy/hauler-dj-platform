# SCR — рука на пластинке (скрэтч)

- **Требования:** DJP-SCR-001…005
- **Дата:** 2026-11-17
- **Ветки/коммиты:** Э1 `1e8c082`, сторож и кроссфейд `dcde0b5`/`bef4bc1`, звук `scratch_sound`, петля `f1bfb91`, опция щелчка `e7a88e5`
- **Свидетели:** `crates/dj-deck/tests/scratch.rs` (7), `crates/dj-deck/tests/scratch_sound.rs` (4), `crates/dj-deck/tests/loop_scratch.rs` (4), `crates/dj-deck/tests/tempo_consumption.rs` (2), `crates/dj-deck/tests/throughput_probe.rs` (1), `crates/dj-signalsmith-sys/tests/interpolation.rs` (3)

## Приёмка — по пунктам

| Пункт | Как проверено | Результат |
|---|---|---|
| Рука задаёт абсолютную скорость: касание останавливает мотор | `grab`, затем `a_still_hand_is_silence` и UI-проверка `--scratch-check`: «the hand stopped the motor, moved the material 45363 frames to the left, a still hand moved nothing, and letting go handed it back to the motor» | ✅ |
| Звук идёт за рукой в обе стороны | `the_hand_plays_the_material_it_moves_over`, `a_hand_moves_the_head_by_its_own_integral` (допуск ±1 кадр), `a_slow_hand_moves_the_head_at_its_own_speed` | ✅ |
| Отпускание возвращает мотор без щелчка и без seek | `releasing_the_record_gives_it_back_to_the_motor`, `the_hand_arrives_and_leaves_without_a_click` (0,0301 против мерки 0,0151) | ✅ |
| Рука, которая замолчала, не скрэтчит сама себя | `a_hand_that_stops_speaking_is_released`, `a_held_record_that_keeps_speaking_stays_still` | ✅ |
| Keylock уступает руке (глиссандо) | `key_lock_does_not_slow_the_hand` — при темпе 1,1 и руке на половине скорости голова проходит ровно просимое | ✅ |
| Разворот под рукой не щёлкает | `turning_the_record_round_under_the_hand_does_not_click` — 0,0151 | ✅ |
| Щелчок иглы — опция, по умолчанию выключена | `the_reversal_click_is_an_option_and_it_is_off_by_default` | ✅ |
| Петля держит руку | `the_hand_wraps_at_the_loop_and_does_not_leave_it`, `the_hand_wraps_without_a_hole_in_the_sound`, `a_short_loop_pages_under_the_hand` | ✅ |
| Обороты петли считает мотор, не рука | `scratch_does_not_count_loop_wraps` | ✅ |
| Дека берёт материал со скоростью своего темпа | `a_deck_consumes_the_file_at_its_tempo_with_keylock` / `…_without_keylock` (1,0256 → 1,0256 с/с) | ✅ |
| Пропускная способность не является дефектом деки | `a_deck_renders_a_second_of_material_in_far_less_than_a_second` — 13 мс на секунду материала | ✅ |

## Гейты

`gate76`, `gate77` (ветка), `gate78`–`gate86` (main) — каждый `all gates passed`; `--scratch-check`
зелёная в каждом полном гейте, `--pitch-check` 0,998…1,005.

## Не закрыто

- вход в штрейчер щёлкает сам по себе (0,4542 при keylock против 0,0151 без) — вне скрэтча, отдельная задача;
- перескок иглы не реализован: у команды `Hand` нет переносчика позиции, а `MAX_HAND_RATE = 4,0` скачка в 250 мс не даёт;
- переключателя щелчка в интерфейсе нет — ждёт решения о месте хранения и месте в раскладке.
