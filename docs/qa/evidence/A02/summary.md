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

## Что сделано (этап 2: PCM-кэш по ADR-0008)

| Файл | Содержание |
|---|---|
| `crates/dj-codec/src/cache.rs` | `PcmCache`: ключ = sha256 содержимого + частота + каналы + формат сэмпла + версия формата; запись атомарная (`.part` → rename); `CacheStorage::{Disk, Memory}`; `PcmSampleFormat::{I16, I24, F32}`; LRU с лимитом и пиннингом; при сканировании битые записи удаляются |
| `CachedReader` | чтение записи без декодера, точный seek по байтовому смещению |
| `playback_source()` | решение `Streaming` / `PcmCache` **только по формату файла** (никакой адаптации в потоке) |
| `Prefetcher::open_with_cache()` | один и тот же аудиопут для обеих стратегий: кольцо и нарезка окна не меняются |

Замеры сборки кэша (5-минутный MP3 320, release):

| Сценарий | Время | Размер записи |
|---|---|---|
| холодная сборка (`Built`) | **244 мс** | 52,9 МБ |
| повторное открытие (`Hit`) | 33,7 мс (почти всё — хэш 12 МБ файла) | — |

Бюджет `Ready ≤ 3 с` при этом выполняется с запасом ~12×, даже с учётом записи на диск.

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
| Lossy идут через кэш, lossless — потоково, решение по формату | `cache::the_policy_reads_the_format_only`, `a_lossless_file_is_not_cached` | ✅ |
| Кэш строится один раз и переиспользуется | `an_mp3_is_built_once_then_reused` (`Built` → `Hit`, одна запись) | ✅ |
| Запись отдаёт те же сэмплы | `the_entry_plays_the_same_audio_as_the_decoder` (в пределах квантования i16) | ✅ |
| Seek в кэше точен | `seeking_inside_the_entry_is_exact_and_cheap` (`landed == 22050`) | ✅ |
| Лимит вытесняет LRU, пиннинг защищает | `the_limit_evicts_the_least_recently_used_and_spares_pinned_entries` | ✅ |
| Битые записи не играют | `a_corrupt_entry_is_dropped_on_scan_instead_of_being_played` | ✅ |
| Memory-хранилище не пишет на диск | `memory_storage_never_touches_the_disk` | ✅ |
| Плеер из кэша не вызывает декодер | `a_prefetch_over_the_cache_plays_without_a_decoder` (`decoder_calls == 0`) | ✅ |

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

## Этап 3: кэш всего трека в движке (2026-11-10)

Политика по формату (этап 2) отвечала на вопрос «можно ли отдать файл из кэша». Вопрос владельца был
другой: **почему буферы маленькие, если ОЗУ не проблема** — трек должен лежать в памяти целиком.

| Файл | Содержание |
|---|---|
| `crates/dj-codec/src/cache.rs` | `CacheConfig::lossless`: lossless кэшируется только по политике (по умолчанию — нет); правило одно и в `playback_source`, и в `prepare` |
| `crates/dj-deck/src/lib.rs` | `CachedSource` (массив в памяти: чтение — `memcpy`, seek — присваивание), `CachedReaderSource` (запись кэша как источник), `Deck::swap_source` |
| `apps/dj-engine/src/lib.rs` | `cached_source()` — одна точка решения, где деке брать материал; `GraphLoad::CacheReady` — подмена источника на границе блока |
| `apps/dj-engine/src/main.rs` | кэш движка: **Memory + F32 + 1024 МБ + lossless**; поток ставится первым, запись — за ним |

### Почему F32 и почему без пиннинга

F32 обязателен: свидетель `a_deck_on_a_cache_entry_plays_the_same_frames_as_a_deck_on_the_stream`
требует побайтового равенства с потоком, а I16 теряет младшие биты 24-битного файла. Пиннинг не нужен:
читатель владеет `Arc` с данными (или открытым файлом), поэтому вытеснение записи из кэша не может
выдернуть материал из-под играющей деки.

### Замер на реальном треке

`tmp/12660184_Emotions_(Original Mix).wav`, 56 МБ, 303,6 с, 48 кГц:

| Событие | Время |
|---|---|
| дека готова играть (анализ сетки, громкости, волна) | **4 924 мс** |
| запись в кэше готова, дека перешла на память | ещё **5 377 мс** |

Порядок «поток первым, кэш за ним» выбран по этому замеру: кэш при загрузке стоил бы вдвое больше
времени до первого звука (10,3 с вместо 4,9 с), а так второй декод не слышен. Размер записи —
**116 589 376 байт (111,2 МиБ)** = 303,6 с × 48 000 × 2 × 4, ровно как считается; на четыре
десятиминутных трека при 48 кГц это **922 МБ**, бюджет 1024 МБ их вмещает.

### Свидетели

| Тест | Что доказывает |
|---|---|
| `a_deck_on_the_cache_plays_the_same_frames_as_a_deck_on_the_stream` (dj-deck) | запись в кэше — тот же материал, побайтово, 200 блоков |
| `swapping_to_the_cache_mid_play_keeps_the_next_frame` (dj-deck) | подмена посреди игры не сбрасывает DSP, следующий кадр тот же |
| `a_track_the_cache_holds_keeps_playing_after_the_file_is_gone` (dj-engine) | файл удалён — дека играет дальше 200 блоков; это и есть смысл кэша |
| `the_engine_asks_the_cache_only_when_the_policy_wants_lossless` (dj-engine) | без политики lossless не кэшируется, и отказ ничего не пишет |
| `a_lossless_file_is_cached_only_when_the_policy_asks` (dj-codec) | политика: по умолчанию отказ, с флагом — запись |

Оговорка: замер снят на тёплом page cache; холодный диск даст больше времени на первом чтении. Числа
`4,9 с` и `5,4 с` — про этот файл и эту машину, не про любой трек.
