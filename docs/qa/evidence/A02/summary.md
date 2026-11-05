# A02 — декодер, индексированный seek, bounded PCM prefetch

- **Задача:** A02 (owner: media)
- **Дата:** 2026-11-05
- **Требования:** DJP-MEDIA-001, DJP-MEDIA-002
- **Решения:** ADR-0008 (политика PCM-кэша по форматам), ADR-0005 (частота = частота устройства)
- **Артефакты:** `measurements.json`, `test-results.txt`

## Что сделано (этап 1: декодирование)

| Файл | Содержание |
|---|---|
| `crates/dj-codec/src/info.rs` | `MediaFormat` (классификация формата → политика кэша), `MediaInfo` (частота, каналы, кадры, delay/padding), `MediaError` |
| `crates/dj-codec/src/decode.rs` | `StreamDecoder`: Symphonia 0.6, выход — interleaved stereo `f32`, gapless-обрезка включена явно, seek «точно к цели либо на кадр раньше с добивкой», счётчики `decoder_calls`/`seeks`/`corrupt_packets`/`discarded_frames` |
| `crates/dj-codec/src/prefetch.rs` | `Prefetcher`: поток-декодер + кольцо на атомиках (без `unsafe`, без аллокаций на чтении), seek внутри окна = только курсор |
| `apps/dj-media-bench` | `decode` / `load` / `seek` с JSON-выводом |
| `crates/dj-codec/tests/fixtures/audio` | FLAC, MP3 CBR, MP3 VBR, Ogg — из нашего собственного синуса (`scripts/gen-media-fixtures.sh`), WAV генерируется тестами |

## Acceptance

| Пункт | Чем проверено | Результат |
|---|---|---|
| WAV, FLAC, MP3 (CBR и VBR), Ogg Vorbis декодируются и делают seek | 10 тестов `tests/decode.rs` | ✅ |
| MP3 доставляется уже без задержки кодировщика | `mp3_arrives_with_the_encoder_delay_already_trimmed`: первые сэмплы — звук, расхождение с заявленным числом кадров < 2000 | ✅ (delay 1105, padding 191–875 из LAME-тега) |
| VBR даёт пригодную длительность | `a_vbr_mp3_still_reports_a_usable_duration` | ✅ |
| Lossless совпадает с исходным WAV | `flac_decodes_to_the_same_samples_as_the_wav_it_came_from` (расхождение < 1e-3) | ✅ |
| Seek в WAV — точно в сэмпл | `a_seek_in_wav_lands_on_the_exact_sample` (`landed == 44100`) | ✅ |
| Seek в MP3 — в пределах одного фрейма | `a_seek_in_mp3_lands_within_one_codec_frame` | ✅ |
| Audio callback читает только предзабуференный PCM | `tests/prefetch.rs`: кольцо отдаёт те же сэмплы, что декодер | ✅ |
| Seek внутри окна не трогает декодер | `a_seek_inside_the_window_never_calls_the_decoder`: `decoder_calls` и `seeks` не меняются | ✅ |
| EOF, мусор, обрезанный файл — определённый исход без паники | `garbage_is_an_error_and_not_a_panic`, `reading_past_the_end_returns_zero_rather_than_padding` | ✅ |

## Замеры (release, 60-секундный файл, 44,1 кГц stereo)

| Формат | Декодирование, ×realtime | `t_ready` (окно 2 с) | Seek за пределы окна, p99 |
|---|---|---|---|
| FLAC | 5 055× | 1,27 мс | 0,064 мс |
| MP3 CBR 320 | 2 294× | 2,26 мс | 0,276 мс |
| MP3 VBR | 3 029× | 1,08 мс | 0,094 мс |
| Ogg Vorbis | 2 426× | — | — |

Выводы, которые из этого следуют честно:

1. Бюджет `Ready ≤ 3 с` (ADR-0008) имеет гигантский запас: 5-минутный трек декодируется за ~0,13 с,
   значит PCM-кэш при загрузке укладывается в бюджет вместе с записью на диск.
2. Бюджет `seek ≤ 30 мс` для seek вне окна выполняется с запасом в два порядка.
3. **Числа не вынуждают PCM-кэш.** Он остаётся решением владельца по политике (детерминированное
   поведение формата, точный seek, независимость от состояния диска), а не следствием медленного
   декодера — это важно записать, чтобы потом никто не «оптимизировал» политику обратно.

## Оговорки

Замеры получены на этой машине с тёплым page cache и файлом 60 с. Холодный диск, файл на 100 МБ и
параллельная нагрузка дадут другие абсолютные числа; порядок величины (тысячи ×realtime, seek в
микросекундах) устойчив, но пороги проверяются на эталонном стенде в A11.
