# A04 — микшер: каналы, кроссфейдер, master-лимитер, метрики, шина cue

- **Задача:** A04 (owner: dsp)
- **Дата:** 2026-11-05
- **Требования:** DJP-MIX-001, DJP-ARCH-006, DJP-ARCH-007; частично DJP-MIX-003/004
- **Артефакты:** `crossfade-demo.wav`, `crossfade-cue.wav`, `report.json`, `test-results.txt`

## Что сделано

| Файл | Содержание |
|---|---|
| `crates/dj-mixer/src/lib.rs` | `Mixer`: канальные стрипы (trim dB, фейдер, назначение на сторону кроссфейдера, cue), кроссфейдер с двумя законами (линейный и constant-power), master gain, пиковый лимитер без lookahead, метры peak/RMS, шина cue (PFL, pre-crossfader), счётчики клипов и нефинитных сэмплов |
| `apps/dj-headless mix` | деки идут через микшер, а не суммируются напрямую; сценарий задаёт `fader`/`assign`/`cue`/`crossfader`/`curve`, команды `SetFader`, `SetCrossfader`, `CueOn/Off`, `SetMasterGain`; `--cue-out` пишет шину наушников отдельным файлом |

## Acceptance

| Пункт | Чем проверено | Результат |
|---|---|---|
| Только предназначенные адресаты (routing fixtures) | `routing_puts_each_channel_only_where_it_belongs`: канал, закрытый кроссфейдером, не просачивается в master, но слышен в cue | ✅ |
| Крайние значения кроссфейдера | `the_constant_power_crossfader_isolates_the_extremes`: 0 → A=1.0/B=0, 1 → наоборот, 0.5 → −3 дБ (0.7071) | ✅ |
| NaN-защита параметров | `non_finite_parameters_keep_the_previous_value`: NaN-кроссфейдер, inf-trim и NaN-фейдер не портят состояние | ✅ |
| NaN-защита сигнала | `non_finite_samples_become_silence_and_are_counted`: NaN/Inf в блоке → тишина, счётчик = 2 | ✅ |
| Master не превышает полную шкалу | `the_limiter_keeps_the_master_below_full_scale` (два full-scale канала в центре: 1.41 на входе → ≤ 1.0), `clipping_is_reported_when_the_input_is_over_full_scale` | ✅ |
| Метры RMS/peak/clip | `meters_report_peak_and_rms_of_the_channel`, `clipping_is_reported_...` | ✅ |
| Микшер не аллоцирует на аудиопути | `tests/rt_contract.rs` (`CountingAllocator` + `RtScope`, 50 блоков → 0 аллокаций) | ✅ |
| Latency/tail объявлены | `the_mixer_declares_no_latency` (DJP-ARCH-009-подобный контракт `prepare`) | ✅ |

## Артефакт маршрутизации

Сценарий: дека A (440 Гц, 0.5) назначена на сторону A, дека B (660 Гц, 0.2) — на сторону B и в cue;
кроссфейдер стартует на 0, на середине уходит в 1.

| Файл | Пики по четвертям |
|---|---|
| `crossfade-demo.wav` (master) | 0.488, 0.488, **0.195**, 0.195 |
| `crossfade-cue.wav` (наушники) | 0.195, 0.195, 0.195, 0.195 |

Master играет A, затем только B; cue всё время содержит только B — то есть PFL-шина не зависит от
кроссфейдера. Это и есть «routing fixtures show only intended destinations» в измеримом виде.

## Решение, принятое по ходу

**Лимитер с мгновенной атакой.** Первая версия сглаживала атаку ~1 мс, и замер показал 1.41 на входе
→ **1.40 на выходе**: первые сэмплы проходили на unity-гейне. Без lookahead сглаженная атака
бессмысленна, поэтому атака мгновенная, а сглажен только release (150 мс). Порог 0.999 оставляет
запас на интер-сэмпловые пики. Записано в doc-комментарии типа, чтобы «улучшение» не вернуло баг.
