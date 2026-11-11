# Сторонние компоненты

Обязательная запись для каждой зависимости до её включения (DJP-LIC-001). Колонка «статус»
показывает, что реально присутствует в сборке, — список не является заявлением о поставке.

**Политика (ADR-001, accepted):** проект под двойной лицензией `MIT OR Apache-2.0`. Зависимости
под GPL/AGPL не включаются: они сделали бы комбинированную работу GPL. Компоненты под LGPL (Qt,
ALSA) допустимы только через динамическую линковку с уведомлениями и возможностью замены библиотеки.

## В сборке сейчас

| Компонент | Версия | Лицензия | Способ связи | Обязанности | Патентный риск | Статус |
|---|---|---|---|---|---|---|
| Qt 6 (`qtbase`, `qtdeclarative`, `qtwayland`) | 6.11.2 (nixpkgs) | набор по `meta.license`: LGPL-2.1-or-later, LGPL-3.0, GPL-2.0-or-later, GPL-3.0, GFDL-1.3 | динамическая линковка UI | уведомления, возможность замены библиотеки, состав модулей проверить до поставки (DJP-LIC-005) | нет данных | в сборке (dev shell + `apps/dj-ui`) |
| Chromaprint, порт на Rust (`chromaprint-next`, vendored) | 0.1.0 | MIT (© 2010–2016 Lukas Lalinsky — оригинал, © 2026 Attila Györffy — порт) | статическая линковка (Rust, `crates/dj-fingerprint`) | сохранить текст MIT (`crates/dj-fingerprint/vendor/chromaprint-next/LICENSE-MIT`) | нет данных | в сборке: отпечатки для поиска дублей в коллекции. Исходный ресемплер порта (`src/audio/resample.rs`, порт FFmpeg `av_resample`, LGPL-2.1-or-later) **удалён и заменён** своим полифазным Kaiser-sinc ресемплером, поэтому LGPL-кода в дереве нет и комбинированная работа остаётся MIT |
| PipeWire | 1.6.6 | MIT | динамическая линковка (аудиобэкенд) | сохранение уведомлений | нет данных | в dev shell |
| ALSA lib | системная (nixpkgs) | LGPL-2.1-or-later | динамическая линковка | уведомления | нет данных | в dev shell |
| OpenSSL | nixpkgs | Apache-2.0 | динамическая линковка | уведомления | нет данных | в dev shell |
| Rust toolchain (rustc, cargo, clippy, rustfmt) | 1.95.0 (nixpkgs) | MIT / Apache-2.0 | инструмент сборки | нет | нет | в dev shell |
| cpal | 0.18.2 (`pipewire` feature) | MIT | статическая линковка (Rust) | уведомление | нет данных | в сборке (`apps/dj-audio-probe`) |
| pipewire / pipewire-sys / libspa | 0.10.1 | MIT | статическая линковка (Rust-биндинги к libpipewire) | уведомление | нет данных | в сборке (через cpal PipeWire-бэкенд) |
| serde / serde_json | 1.0.x | MIT OR Apache-2.0 | статическая линковка (Rust) | уведомление | нет данных | в сборке (`apps/dj-audio-probe`) |
| Signalsmith Stretch (`signalsmith-stretch.h`, vendored) | 1.3.2 | MIT (© 2022 Geraint Luff / Signalsmith Audio Ltd.) | заголовочная C++11-библиотека, статическая линковка в `dj-stretch-sys` | сохранить текст MIT и уведомление (`crates/dj-stretch-sys/vendor/LICENSE.txt`) | в README заявлен патентно-чистый метод (ADC22 «Four Ways To Write A Pitch-Shifter»); собственных патентных оговорок нет | в сборке (key lock, DJP-DECK-008) |
| Signalsmith Linear (`approx.h`, `fft.h`, `linear.h`, `stft.h`, vendored) | 0.6.6 | MIT (© 2025 Signalsmith Audio) | заголовочная C++11-библиотека (FFT и STFT под Stretch), статическая линковка | сохранить текст MIT (`crates/dj-stretch-sys/vendor/signalsmith-linear/LICENSE.txt`) | нет данных | в сборке (транзитивно, только через Stretch) |
| Signalsmith DSP (`envelopes.h`, `delay.h`, `filters.h`, `curves.h`, `fft.h`, `windows.h`, `perf.h`, `common.h`, vendored) | 1.7.0 | MIT (© 2021 Geraint Luff / Signalsmith Audio Ltd.) | заголовочная C++11-библиотека, статическая линковка в `dj-signalsmith-sys` | сохранить текст MIT (`crates/dj-signalsmith-sys/vendor/signalsmith-dsp/LICENSE.txt`) | нет данных | в сборке: баллистика метров (`PeakDecayLinear`), ресемплер (`InterpolatorKaiserSincN`) и изолятор на кроссовере Линквица-Райли (`BiquadStatic`), закон ручек гейта и мастера (`CubicSegmentCurve` в оболочке); lookahead-лимитер приходит отсюда через `signalsmith-basics` |
| Signalsmith Basics (`include/signalsmith-basics/limiter.h`, `include/stfx/stfx-library.h`, vendored) | 2022–2025 (repo) | MIT (© Geraint Luff / Signalsmith Audio Ltd.) | заголовочная C++-библиотека, статическая линковка в `dj-signalsmith-sys` | сохранить текст MIT (`crates/dj-signalsmith-sys/vendor/signalsmith-basics/LICENSE.txt`) | нет данных | в сборке: лимитер мастера с lookahead (A04). Собирается как **C++17**: в базовом классе эффекта лежит `std::atomic<bool>`, и по правилам C++11/14 неявная копия этого базового класса некорректна |

## Запланированы (ещё не зависимость)

| Компонент | Лицензия (по ТЗ §17 / upstream) | Роль | Когда | Ограничение |
|---|---|---|---|---|
| Symphonia | MPL-2.0 | декодирование WAV/FLAC/MP3/Ogg Vorbis, `symphonia-core` 0.6 | A02 | **license review закрыт 2026-11-05:** MPL-2.0 — пофайловый копилефт, как зависимость совместима с MIT OR Apache-2.0 (обязанность возникла бы только при правке файлов Symphonia, чего мы не делаем); патенты MP3 истекли к 2017 году. MP3 разрешён в R0 (ADR-0008) |
| SQLite | Public Domain | коллекция | A08 | — |
| Tokio | MIT | фоновые задачи | по необходимости | не на RT-пути |
| Encdr | GPL-3.0-or-later | референс фактов о протоколе HID S4 MK3 | A09 | **исключён** решением ADR-001: только публичные сведения, код не копируется (DJP-LIC-003) |
| Mixxx | GPL-2.0-or-later | референс **фактов** о протоколе S4 MK3 (HID) | A09 | **код и маппинги не используются**: только факты со ссылкой на источник, clean-room-лог `docs/hardware/s4mk3/cleanroom-log.md` (DJP-LIC-003/006, ADR-0006) |
| Ableton Link | GPLv2+ / коммерческая | внешняя синхронизация | M6 | **исключён**; только при отдельной коммерческой лицензии и новом ADR (DJP-LIC-004) |

## Рассмотрено, лежит в `./tmp` (не подключено)

Набор Signalsmith, разобранный под наши части. Заголовочные C++11, без сборки; лицензии MIT, кроме
`githilbert-iir` (0BSD). Включать по правилу DJP-LIC-001: запись здесь, затем вендоринг в отдельный
`crates/dj-*-sys` с C ABI и Rust-фасадом, затем замер до интеграции. Подробный разбор с привязкой к
нашим файлам — `references/signalsmith-libs.md` в скилле `hauler-dj-platform`.

| Компонент | Версия | Лицензия | Куда просится |
|---|---|---|---|
| Signalsmith Basics: `limiter.h` | 2022–2025 (repo) | MIT | мастер-лимитер: lookahead, hold, release, линковка каналов (наш `dj-mixer` без lookahead) |
| Signalsmith Basics: `dynamics.h` | там же | MIT | гейт тишины деки, экспандер, мягкая динамика мастера |
| Signalsmith Basics: `analyser.h` | там же | MIT | спектр для UI (Bark-полосы, `spectrumHz`/`spectrumEnergy`), потокобезопасные метры |
| Signalsmith Basics: `reverb.h`, `chorus.h`, `freq-shifter.h`, `crunch.h` | там же | MIT | будущая FX-секция (beat FX); `freq-shifter` опирается на `githilbert-iir` |
| Signalsmith DSP (`filters.h`, `delay.h`, `rates.h`, `curves.h`, `spectral.h`, `windows.h` — `envelopes.h` уже в сборке) | 1.7.0 | MIT | биквады с расчётом Вицанека (EQ, настоящий isolator), интерполированные задержки (ресемплер темпа, скраб), Кайзер-синк (44,1 → 48), законы кривых |
| Signalsmith FFT | repo | MIT (в README) | самостоятельный FFT, если понадобится без Stretch |
| Signalsmith Pitch Tracker | 2026 (repo) | MIT | оценка тональности трека, детект вокала (монофонический — на миксе врёт) |
| Signalsmith IIR Hilbert (`githilbert-iir`) | 2024 | **0BSD** | сдвиг частоты, ширина стерео, фазовое вращение |

Stretch `1.3.2` и Linear `0.6.6` из этого же набора уже в сборке (см. выше) и дают больше, чем мы
используем: `setTransposeSemitones`/`setFormantSemitones`/`setFreqMap` — это будущий сдвиг тональности
деки без смены темпа.

## Запрещено

Фирменные иконки, графика, сэмплы, прошивки, proprietary-бинарники Native Instruments, а также
код и ресурсы Traktor/Mixxx (DJP-LIC-006). Для Mixxx уточнение из ADR-0006: факты о протоколе
(идентификаторы HID-коллекции, назначение байтов) — можно и нужно фиксировать с указанием
происхождения; файлы маппингов (`.xml`, `.js`) и любой код — нельзя, включая перевод на Rust. Тестовые аудиофайлы — только собственные или
разрешённые, с зафиксированным происхождением.
