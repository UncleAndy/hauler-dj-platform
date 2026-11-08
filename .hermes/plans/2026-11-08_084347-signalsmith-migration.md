# План: перевод уже реализованных DSP-мест на алгоритмы Signalsmith

> **Для Hermes:** промежуточный план. Выполнять этапами, каждый этап — отдельный коммит, отдельный замер
> до и после, полный гейт после каждого. План не требует делать всё сразу: любой этап можно остановить,
> и система останется рабочей.

**Цель:** заменить наши самописные реализации там, где библиотека Signalsmith заведомо точнее —
лимитер мастера, ресемплер (переход частоты и темп без key lock), биквадный EQ и «убийство» полосы, —
и добавить баллистику метров. Контракты UI, формат снапшота и решения владельца (законы кривых, закон
ручки, целевой уровень автогейна) не меняются.

**Архитектура:** один вендоренный крейт `crates/dj-signalsmith-sys` по образцу `crates/dj-stretch-sys`
(заголовки в `vendor/`, C ABI в `src/wrapper.cpp`, безопасный Rust-фасад в `src/lib.rs`, сборка
системным `clang++` из `build.rs`). Каждая ступень добавляет в обёртку **только** то, что использует.

**Стек:** C++11 header-only Signalsmith (MIT; `githilbert-iir` — 0BSD), Rust 1.95, `bash scripts/check.sh`
как единственный гейт.

**Исходники:** `./tmp/signalsmith-{dsp,basics,linear,fft,pitchtrack,githilbert-iir,stretch}` — каталог
`tmp/` в `.gitignore` (строка 41), то есть это **не** зависимость, а копия для чтения; вендоринг = копия
нужных заголовков в крейт.

---

## Что переводим, а что нет

| Место у нас | Сейчас | Что даёт Signalsmith | Решение |
|---|---|---|---|
| Мастер-лимитер `crates/dj-mixer/src/lib.rs:407` (`Limiter`) | мгновенная атака, release 150 мс, **без lookahead**, латентность 0 | `basics/limiter.h`: lookahead (`attackMs`), `holdMs`, `releaseMs`, `linkChannels`, `latencySamples()` | **переводим** (этап 2) |
| Метры `crates/dj-mixer/src/lib.rs:54` (`Meter{peak,rms}`), `crates/dj-audio-io/src/lib.rs:98` (`peaks`) | только пик блока, без инерции и удержания | `dsp/envelopes.h`: `PeakHold`, `PeakDecayLinear`, `BoxFilter` | **переводим** (этап 1) |
| Ресемплер `crates/dj-deck/src/lib.rs` (`render_at_tempo_chunk`, ~1097–1160) | линейная интерполяция; тот же код переводит 44,1 → 48 | `dsp/delay.h`: `InterpolatorCubic`, `LagrangeN`, `KaiserSincN`; `dsp/rates.h`: `fillKaiserSinc` | **переводим** (этап 3) |
| EQ `crates/dj-dsp/src/biquad.rs`, `eq.rs` | биквад с своим расчётом (505 шагов × 3 полосы), shelf-«kill» течёт на 1 кГц (−32 дБ) | `dsp/filters.h`: `BiquadStatic` с `BiquadDesign::vicanek`; LR-кроссовер из двух буттервортовских биквадов | **переводим** (этап 4) |
| Кроссфейдер `crates/dj-mixer` (constant_power/linear/smooth) | наш закон, закреплён тестами и решением владельца | `dsp/curves.h`: `Linear`, `Cubic`, `Reciprocal` | **не переводим**, только сверяем форму (этап 5) |
| Автогейн `crates/dj-analysis/src/loudness.rs` | блоки 0,4 с, гейт 20 дБ, цель −12 dBFS RMS — решение владельца | `envelopes.h::BoxFilter` может считать RMS иначе | **не переводим**: цель и окно — продуктовое решение, не алгоритм |
| Key lock `crates/dj-deck` | уже Signalsmith Stretch 1.3.2 | — | сделано (DJP-DECK-008) |

---

## Этап 0. Крейт `dj-signalsmith-sys` (каркас)

**Зачем:** одна точка вендоринга и одна лицензионная запись на весь набор, вместо четырёх крейтов.

**Файлы:**
- Создать: `crates/dj-signalsmith-sys/Cargo.toml` (по образцу `crates/dj-stretch-sys/Cargo.toml`:
  `build = "build.rs"`, `links = "dj_signalsmith"`)
- Создать: `crates/dj-signalsmith-sys/build.rs` — копия логики выбора компилятора из
  `crates/dj-stretch-sys/build.rs:1-40` (системный `clang++`, без `cc`)
- Создать: `crates/dj-signalsmith-sys/vendor/` — `signalsmith-dsp/*.h`, `signalsmith-basics/include/**`,
  `signalsmith-basics/include/stfx/stfx-library.h`, лицензии обоих репозиториев
- Создать: `crates/dj-signalsmith-sys/src/{lib.rs,wrapper.cpp}`
- Изменить: `Cargo.toml` (workspace members), `THIRD_PARTY.md` (строки «в сборке» для двух компонентов)

**Проверка:**
- `cargo build -p dj-signalsmith-sys` — собирается, `vendor/` не тянет `../..` путей
- `bash scripts/check-docs.sh` — записи в `THIRD_PARTY.md` на месте, таблица валидна
- `git status` не показывает `tmp/` (он в `.gitignore`)

**Откат:** удалить крейт из workspace; ничего в тракте не менялось.

---

## Этап 1. Метры: баллистика (низкий риск, звук не меняется)

**Что есть:** `Meter { peak, rms }` считается по блоку и публикуется как есть; в UI значение прыгает
с частотой кадров, без удержания и спада.

**Что делаем:** `signalsmith::envelopes::PeakHold` (+ `PeakDecayLinear` для спада) в обёртке, состояние
метра на канал/шину; на выходе — те же поля `peak`/`rms` (UI и снапшот не меняются) плюс новые
`peak_hold_db`, `peak_decay_db`.

**Файлы:**
- Изменить: `crates/dj-signalsmith-sys/src/{wrapper.cpp,lib.rs}` — обёртка `PeakHold<float>`
- Изменить: `crates/dj-mixer/src/lib.rs` — `Meter` получает `hold`/`decay` (вычисляются из `PeakHold`),
  `Meters::master`/`cue`/`channels` заполняются через неё
- Изменить: `crates/dj-domain/src/lib.rs` — поля снапшота `*_peak_hold` (с `#[serde(default)]`, чтобы
  старые документы читались)
- Тест: `crates/dj-mixer/tests/mixer.rs`

**TDD-шаги:**
1. Тест `a_meter_holds_its_peak_and_decays_after_the_signal_stops`: подать 20 мс синуса 0,5, затем 200 мс
   тишины; ждать `hold ≈ 0,5` через 100 мс тишины и `hold < 0,5` через 200 мс. Сейчас падает (нет hold).
2. `cargo test -p dj-mixer --test mixer a_meter_holds_its_peak` → FAIL.
3. Реализовать `PeakHold` + поля снапшота.
4. Тот же тест → PASS; `cargo test -p dj-mixer --test mixer meters_report_peak_and_rms_of_the_channel`
   остаётся зелёным (старые поля не сломались).
5. Коммит: `feat(mixer): метры держат пик и спадают — баллистика из Signalsmith`.

**Критерий приёмки:** старые тесты метра без правок проходят; новый тест падает без обёртки; звук
байт-в-байт прежний (`a_block_through_the_graph_is_audible…` и golden рендеры не меняются).

**Откат:** убрать поля hold/decay, вернуть `Meter` как был; тракт не затрагивался.

---

## Этап 2. Лимитер мастера с lookahead (средний риск, меняет тракт)

**Что есть:** `Limiter::process` (`crates/dj-mixer/src/lib.rs:429`) — мгновенная атака, release 150 мс,
без lookahead. Комментарий типа честно говорит, почему: без lookahead сглаживание пропустило бы
именно те сэмплы, ради которых лимитер стоит.

**Что делаем:** `signalsmith::basics::LimiterFloat` — lookahead (по умолчанию 20 мс, потолок 100),
hold, release, линковка каналов. Лимитер встаёт **после** мастера, как сейчас.

**Главное следствие (урок DJP-DECK-008):** ступень в тракте = задержка. `Mixer::latency()` сейчас
возвращает 0 (`crates/dj-mixer/src/lib.rs:157`); станет `attackMs` в кадрах. Её нужно:
- вернуть в `EngineGraph` (часы вывода), `DeckStatic`/снапшот — чтобы «прошло / осталось» и волна не
  разъезжались со звуком;
- учесть в `--transport-check` и тестах движка, которые читают `position()`.

**Файлы:**
- Изменить: `crates/dj-signalsmith-sys/src/{wrapper.cpp,lib.rs}` — `dj_limiter_*` (create/configure/
  process/reset/latency)
- Изменить: `crates/dj-mixer/src/lib.rs` — `Limiter` становится обёрткой; `Mixer::latency()` возвращает
  задержку; поля `limiter_gain`/`clipped_samples` сохраняются
- Изменить: `crates/dj-audio-core/src/engine.rs` — задержка в публикуемом состоянии
- Изменить: `apps/dj-engine/src/{lib.rs,main.rs}`, `crates/dj-domain/src/lib.rs` — поле задержки
- Тесты: `crates/dj-mixer/tests/mixer.rs`, `apps/dj-engine/tests/engine.rs`

**TDD-шаги:**
1. Тест `a_sample_above_full_scale_never_leaves_the_master_limiter` в `crates/dj-mixer/tests/mixer.rs`:
   вход 1,5 (синус), после лимитера ни один сэмпл не > 1,0 **и** первый же сэмпл перегруза уже подрезан
   (сейчас, без lookahead, первый сэмпл проходит на 1,0 → тест на «первый сэмпл» падает).
2. `cargo test -p dj-mixer --test mixer a_sample_above_full_scale_never_leaves` → FAIL.
3. Подключить `LimiterFloat`, `Mixer::latency()` = задержка лимитера.
4. Тест → PASS; `the_limiter_keeps_the_master_below_full_scale` → PASS.
5. Тест `the_mixer_reports_its_latency`: `mixer.latency()` > 0 и совпадает с `latencySamples()` обёртки.
6. Прогнать `cargo test -p dj-engine`; тесты, читающие `position()` без учёта задержки, получают
   `warm-up` (как сделано в `apps/dj-engine/tests/engine.rs` после DJP-DECK-008).
7. Коммит: `feat(mixer): мастер-лимитер с lookahead и его задержка в часах`.

**Критерий приёмки:** перегруз 1,5 → выход ≤ 1,0 без «проскочившего» первого сэмпла; задержка видна в
состоянии и возвращается часами; `bash scripts/check.sh` зелёный.

**Откат:** `Limiter` оставить прежним, `Mixer::latency()` = 0. Задержка не попала в формат — откат чистый.

---

## Этап 3. Ресемплер: качество вместо линейной интерполяции

**Что есть:** `render_at_tempo_chunk` (`crates/dj-deck/src/lib.rs:1097+`) тянет `need` кадров и
интерполирует линейно, `phase` переносит дробную часть. Тот же путь переводит 44,1-кГц файл в 48 кГц
(`rate_ratio`) и темпит деку при `set_keylock(false)`.

**Что делаем:** кубический интерполятор (`InterpolatorCubic`) как замена линейному — шаг цены мал;
Кайзер-синк (`InterpolatorKaiserSincN` / `rates::fillKaiserSinc`) — для **перехода частоты**, где
алиасинг слышен на 44,1-файлах.

**Файлы:**
- Изменить: `crates/dj-signalsmith-sys/src/{wrapper.cpp,lib.rs}` — два интерполятора
- Изменить: `crates/dj-deck/src/lib.rs` — точки вызова интерполяции (два места: темп и `rate_ratio`)
- Тесты: `crates/dj-deck/tests/deck.rs`

**TDD-шаги:**
1. Тест `a_forty_four_one_kilohertz_tone_does_not_alias_on_a_forty_eight_kilohertz_device`: тон 15 кГц
   с 44,1 кГц, 0,5 с, измерить долю энергии вне полосы 15 кГц ± 100 Гц. Линейная интерполяция даёт
   заметный образ; порог записать **до** правки (ожидание: сейчас FAIL, после — PASS).
2. `cargo test -p dj-deck --test deck a_forty_four_one_kilohertz_tone_does_not_alias` → FAIL.
3. Подключить кубический интерполятор для темпа и Кайзер-синк для `rate_ratio`.
4. Тест → PASS; `a_stretched_tone_stays_clean` (0,9996+) и `the_pitch_stays_where_the_file_put_it`
   остаются зелёными; `a_deck_at_its_own_speed_renders_the_same_bytes…` (golden) — пересмотреть
   осознанно: при `rate_ratio == 1` путь обязан оставаться байт-в-байт.
5. Стоимость: `cargo test -p dj-deck --release --test deck the_block_fits_the_callback_budget` (или
   существующий аналог) — блок 256 кадров ≤ 30 % периода 5,33 мс.
6. Коммит: `feat(deck): ресемплер с кубической интерполяцией и Кайзер-синком на смене частоты`.

**Критерий приёмки:** алиасинг 44,1 → 48 ниже порога, замеренного заранее; чистота тона не хуже
0,9996; стоимость блока в бюджете; golden на `rate_ratio == 1` не изменился.

**Откат:** вернуть линейную интерполяцию — правка локальна в одном файле.

---

## Этап 4. EQ и настоящий isolator (средний риск, меняет звук)

**Что есть:** свой расчёт коэффициентов (`crates/dj-dsp/src/eq.rs`: `shelf_coefficients`,
`peak_coefficients`), таблица 505 шагов × 3 полосы по 0,25 дБ. «Kill» полосы — shelf −26 дБ, который
на 1 кГц ещё даёт −32 дБ (тест `the_floor_kills_its_band_but_bleeds_upwards` это фиксирует честно).

**Что делаем:**
- Расчёт коэффициентов — `BiquadDesign::vicanek` (лучше у Найквиста, чем наш билинейный), структура
  таблицы и шаг 0,25 дБ не меняются.
- Отдельная функция `isolator`: LR-кроссовер 4-го порядка (два буттервортовских биквада на полосу),
  который действительно убивает полосу: −56 дБ на 40 Гц при 0,00 дБ на 1 кГц (числа уже измерены в
  `docs/qa/evidence/A14/summary.md`).
- isolator — **новое поведение**, его включает владелец отдельным решением (изменение ощущения ручки).

**Файлы:**
- Изменить: `crates/dj-signalsmith-sys/src/{wrapper.cpp,lib.rs}` — `BiquadStatic` на все типы
- Изменить: `crates/dj-dsp/src/biquad.rs` — расчёт через обёртку, `process` остаётся наш (или тоже
  переводится, решается замером стоимости)
- Изменить: `crates/dj-dsp/src/eq.rs` — `shelf_coefficients`/`peak_coefficients` → обёртка
- Создать: `crates/dj-dsp/src/isolator.rs` (LR-кроссовер)
- Тесты: `crates/dj-dsp/tests/eq_tests.rs`

**TDD-шаги:**
1. Тест `the_reported_response_matches_a_measured_sine` уже есть — он и есть контроль перехода:
   после смены расчёта отклик на 1 кГц не должен уехать больше чем на 0,25 дБ.
2. Тест `an_isolator_removes_the_band_without_touching_the_neighbours`: 40 Гц ≤ −50 дБ, 1 кГц в
   пределах ±0,5 дБ. Сейчас функции нет → FAIL (не компилируется).
3. Реализовать `isolator.rs` на LR-кроссовере.
4. Тест → PASS; `the_floor_kills_its_band_but_bleeds_upwards` переписать под новую семантику (он
   описывал именно слабость shelf-«kill»), в комментарии — почему.
5. Коммит: `feat(dsp): расчёт биквадов Вицанека и настоящий isolator на LR-кроссовере`.

**Критерий приёмки:** таблица 505 × 0,25 дБ без изменений; отклик на 1 кГц в пределах 0,25 дБ;
isolator −50 дБ и лучше в своей полосе при 0 дБ у соседей; `--eq-check` в UI зелёный.

**Откат:** вернуть свои формулы; `isolator.rs` удалить (он ещё не в тракте, пока владелец не включил).

---

## Этап 5. Сверка законов (без замены)

**Что есть:** законы кроссфейдера `constant_power/linear/smooth` — контракт UI и решение владельца
(тест `every_crossfader_law_has_the_shape_it_promises`); закон ручки — равный угол на пиксель.

**Что делаем:** численно сверить наши кривые с `dsp/curves.h` (`Linear`, `Cubic`, `Reciprocal`) на сетке
0…1 и записать расхождение в `docs/qa/evidence/A14/summary.md`. Если расхождение > 0,5 дБ — это
**вопрос владельцу**, а не повод менять.

**Файлы:** `benchmarks/` (одноразовый скрипт), `docs/qa/evidence/A14/summary.md`.

**Критерий приёмки:** таблица расхождений в документации; ни одной правки в `dj-mixer`/UI.

---

## Проверка после каждого этапа

```bash
# точечно
cargo test -p dj-mixer --test mixer
cargo test -p dj-deck
cargo test -p dj-dsp --test eq_tests
# полный гейт (fmt, clippy, тесты, документация, сборка UI, headless-проверки)
BUILD_DIR=/tmp/even2-build bash scripts/check.sh
```

Гейт считается пройденным только при `check.sh: all gates passed` и `GATE_EXIT=0`.

Замеры «до» пишутся **до** правки: алиасинг 44,1 → 48, первый сэмпл перегруза, отклик EQ на 1 кГц,
`render_p99` блока 256 кадров (порог 30 % от 5333 мкс).

---

## Риски и открытые вопросы

1. **Задержка лимитера** — единственное место, где правка выходит за пределы своей ступени: нужно
   провести её через часы, снапшот и тесты. Урок DJP-DECK-008 уже оплачен, повторять не нужно.
2. **Стоимость Кайзер-синка** в колбэке: 16…32 отвода на кадр — на 48 кГц × 2 деки это может съесть
   бюджет. Решение по замеру: для темпа — кубический (дешевле), для смены частоты — Кайзер.
3. **`tmp/` в `.gitignore`** — вендоринг только копией; проверить, что в коммит не попал `tmp/`.
4. **Лицензии:** `basics`/`dsp`/`fft`/`pitchtrack` — MIT, `githilbert-iir` — 0BSD. Запись в
   `THIRD_PARTY.md` **до** включения (DJP-LIC-001).
5. **Нужны числа, а не ощущения:** каждый этап начинается с теста, который падает, и с порога,
   записанного до данных.
6. **Не трогаем:** законы кроссфейдера, закон вращения ручки, цель автогейна, формат `.djmeta`, key lock
   (уже Signalsmith).

## Порядок и остановка

Этапы независимы: 1 → 2 → 3 → 4 → 5. Если владелец остановит план после любого этапа, дальше работает
прежний код, а сделанное остаётся с тестами и документацией. Этап 2 (лимитер) — единственный, который
меняет задержку тракта; если менять часы сейчас не хочется, его можно отложить, сделав 1 → 3 → 4.
