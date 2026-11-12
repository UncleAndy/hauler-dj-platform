# Коллекция в SQLite: жанры, связи и фильтр — план

**Цель:** данные коллекции живут в SQLite (`~/.local/share/hauler-dj/db/library.db`), жанры — отдельной
таблицей со связью «многие-ко-многим», жанры извлекаются из тегов при анализе, а список коллекции
фильтруется по одному или нескольким жанрам.

**Архитектура:** `.djmeta` остаётся источником правды и никуда не девается; база — производная от неё,
которую можно потерять и пересобрать обходом корней. Пишут базу те, кто пишет файлы: импорт и анализ.
Читает — браузер: список жанров с количеством и фильтр по ним. Список строк пока строится обходом, как
сегодня; переход списка на базу с постраничностью — следующий срез (см. «Что не входит»).

**Стек:** Rust, rusqlite 0.32 (`bundled`, системная libsqlite3 не нужна), Qt 6 QML для интерфейса.

---

## Что есть сегодня (факты, на которых стоит план)

- `crates/dj-library/src/index.rs` — модуль SQLite уже написан: `Index::open` (схема `tracks`,
  `PRAGMA user_version = 1`), `rebuild`, `upsert`, `count`, `by_fingerprint`, `search` (`LIKE '%…%'` с
  жёстким `ORDER BY artist, album, title`), `columns`. **Используется только тестами**
  (`crates/dj-library/tests/metadata.rs`): ни импорт, ни движок, ни браузер его не вызывают.
- `.djmeta` рядом с треком: `TrackMeta` (`crates/dj-library/src/meta.rs`), поле `tags: Option<FileTags>`.
  `FileTags` = `artist`, `album`, `title` — **жанра там нет**.
- Теги читает `crates/dj-library/src/tags.rs`: `Tags.genre: String` — одна строка из symphonia
  (`StandardTag::Genre`). Мультижанровый тег приходит строкой с разделителями.
- Импорт пишет теги в сайдкар: `crates/dj-library/src/import.rs:330`.
- Анализ пишет метаданные: `apps/dj-engine/src/meta.rs` — `beatgrid` (строка 215), `tags` (302),
  `key` (343).
- Список коллекции: `FileBrowser::openCollection` (`apps/dj-ui/src/BridgeClient.cpp:198`) → `collect`
  (266) — обход `QDirIterator` + разбор `.djmeta` на каждый файл; фильтр и сортировка в памяти
  (`setFilter` 355, `applyFilter` 364, `applySort`).
- Замер (`--collection-probe`, синтетика с сайдкарами): 100 000 строк — вход 1 178 мс (холодная) /
  1 169 мс (тёплая), сортировка по шапке 2 090 мс, фильтр 107 мс, снятие 28 мс. Всё на потоке
  интерфейса.
- Путь данных: `crates/dj-library/src/store.rs` уже выводит `$XDG_DATA_HOME/hauler-dj/…`; база встанет
  рядом: `$XDG_DATA_HOME/hauler-dj/db/library.db`.

---

## Решения (подтверждены владельцем 2026-11-12)

1. **Разделители жанров:** `,` `;` `/`. **`&` не разделяет** — «Drum & Bass» это один жанр. Пробелы
   вокруг обрезаются, внутренние схлопываются, пустые отбрасываются, дубликаты — по нормализованному
   ключу; максимум 16 жанров на трек, длина имени до 64 символов.
2. **Идентичность жанра — без учёта регистра:** `key = lower(name)` (Unicode), `name` хранит первое
   встреченное написание. «house», «House», «HOUSE» — один жанр с `name = "House"` (первое написание).
3. **Несколько жанров в фильтре = ИЛИ** (любой из выбранных). «И» оставляем на потом: у диджея выбор
   двух жанров почти всегда означает «или».
4. **Пересборка базы** — автоматически, если база пуста, а коллекция нет (в фоне, с прогрессом, тем же
   механизмом, что у импорта), плюс явная команда `--rebuild-index` для рук.
5. **`tracks.genre` остаётся** сырой строкой тега (совместимость и быстрый показ), жанры для фильтра
   живут в `genres`/`track_genres`.

---

## Схема, версия 2

```sql
-- уже есть, не меняется
CREATE TABLE IF NOT EXISTS tracks (
    fingerprint TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    artist TEXT NOT NULL DEFAULT '',
    title TEXT NOT NULL DEFAULT '',
    album TEXT NOT NULL DEFAULT '',
    genre TEXT NOT NULL DEFAULT '',      -- сырая строка тега
    year INTEGER,
    bpm REAL,
    musical_key TEXT,
    rating INTEGER NOT NULL DEFAULT 0,
    duration_frames INTEGER,
    gain REAL NOT NULL DEFAULT 1.0,
    play_count INTEGER NOT NULL DEFAULT 0,
    last_played_ms INTEGER,
    analysis_lock INTEGER NOT NULL DEFAULT 0
);

-- новое
CREATE TABLE IF NOT EXISTS genres (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL,                  -- как написал тег, первое написание
    key  TEXT NOT NULL UNIQUE            -- lower(name), свёрнутые пробелы
);

CREATE TABLE IF NOT EXISTS track_genres (
    fingerprint TEXT NOT NULL REFERENCES tracks(fingerprint) ON DELETE CASCADE,
    genre_id    INTEGER NOT NULL REFERENCES genres(id) ON DELETE CASCADE,
    PRIMARY KEY (fingerprint, genre_id)
);

CREATE INDEX IF NOT EXISTS genres_key          ON genres(key);
CREATE INDEX IF NOT EXISTS track_genres_genre  ON track_genres(genre_id, fingerprint);
```

`PRAGMA user_version = 2`. Открытие базы: `journal_mode = WAL` (один писатель — движок, читатели — UI),
`synchronous = NORMAL`, `busy_timeout = 3000`, `foreign_keys = ON`.

---

## Задачи

### Задача 1. Схема v2 и миграция

**Файлы:** `crates/dj-library/src/index.rs` (`open`, `SCHEMA`) и модуль `#[cfg(test)] mod tests` в нём же:
тесту миграции нужен приватный `connection`, поэтому это модульный тест, а не интеграционный (иначе
`rusqlite` пришлось бы тащить в dev-dependencies ради одного файла).

**Шаг 1 — падающий тест.** База версии 1 с одной строкой в `tracks`; `Index::open` на ней; проверяем:
`user_version = 2`, таблицы `genres` и `track_genres` есть, старая строка на месте.

**Шаг 2 — прогон.** `cargo test -p dj-library --test index_schema` → FAIL (нет таблиц, версия 1).

**Шаг 3 — реализация.** В `open` после `CREATE TABLE tracks` — `CREATE TABLE IF NOT EXISTS genres …`,
`track_genres …`, индексы, `PRAGMA user_version = 2`; прагмы WAL/`busy_timeout`/`foreign_keys`.
Никакого `DROP`: миграция только добавляет.

**Шаг 4 — прогон.** PASS.

**Шаг 5 — коммит.** `feat(library): схема v2 — жанры и связь многие-ко-многим`.

### Задача 2. Разбор строки жанров

**Файлы:** `crates/dj-library/src/genres.rs` (новый), `crates/dj-library/src/lib.rs` (реэкспорт),
`crates/dj-library/tests/genres.rs` (новый).

**Шаг 1 — падающий тест.** Таблица случаев:
`"House, Techno"` → `["House", "Techno"]`; `"Drum & Bass"` → `["Drum & Bass"]`;
`"house; House ;HOUSE"` → `["house"]`; `"House/Deep House"` → `["House", "Deep House"]`;
`""` → `[]`; `" , ; "` → `[]`; строка из 40 жанров → не больше 16.

**Шаг 2 — прогон.** FAIL.

**Шаг 3 — реализация.**
```rust
pub struct GenreName { pub name: String, pub key: String }
pub fn split(raw: &str) -> Vec<GenreName>;
```
Правила из решения 1 и 2. Нормализация: `trim`, схлопывание пробелов, `to_lowercase()` для ключа.

**Шаг 4 — прогон.** PASS.

**Шаг 5 — коммит.** `feat(library): разбор строки жанров на список`.

### Задача 3. Запросы жанров в индексе

**Файлы:** `crates/dj-library/src/index.rs`, `crates/dj-library/tests/genres.rs`.

**Шаг 1 — падающий тест.**
- `set_genres(fingerprint, &["House", "Techno"])` дважды → в `genres` две строки, в `track_genres` две;
- `set_genres(fingerprint, &["House"])` после этого → осталась одна связь (замена, не добавление);
- `genre_list()` → `[(name, count)]`, «House» с count 1;
- `tracks_with_genres(&["Techno", "Ambient"])` → объединение по ИЛИ;
- `tracks_with_genres(&[])` → пусто (не «все»);
- регистр: `tracks_with_genres(&["house"])` находит «House»;
- удаление строки `tracks` → связи уходят (cascade).

**Шаг 2 — прогон.** FAIL.

**Шаг 3 — реализация.** Методы в одной транзакции на трек:
`set_genres`, `genres_of`, `genre_list`, `tracks_with_genres`; `upsert` вызывает `set_genres`, если
передан список (или отдельный `upsert_with_genres`).

**Шаг 4 — прогон.** PASS.

**Шаг 5 — коммит.** `feat(library): запросы жанров и связи с треками`.

### Задача 4. Жанры в сайдкаре и в импорте

**Файлы:** `crates/dj-library/src/meta.rs` (`FileTags`), `crates/dj-library/src/import.rs:330`,
`crates/dj-library/tests/genres.rs`.

**Шаг 1 — падающий тест.** Файл с тегом `"House, Techno"` проходит импорт → в `.djmeta`
`tags.genres = ["House", "Techno"]`, в базе две связи; старый `.djmeta` без поля читается без ошибки
(`serde(default)`).

**Шаг 2 — прогон.** FAIL.

**Шаг 3 — реализация.** `FileTags { …, #[serde(default)] genres: Vec<String> }`; импорт пишет
`genres: split(tags.genre)`, затем `Index::upsert_with_genres`.

**Шаг 4 — прогон.** PASS.

**Шаг 5 — коммит.** `feat(library): жанры в .djmeta и в индексе при импорте`.

**Закрыто** `c01b560`. Что вышло иначе, чем в плане:

- импорт не открывает базу сам: `import::index_row(&Imported)` отдаёт строку и список жанров, а пишет
  их вызывающий (`run_import` с `Option<&mut Index>`). Библиотечная функция, которая сама лезет в
  глобальный путь базы, не может быть вызвана дважды одним тестом;
- дверь к базе (`Index::open_default()`) появилась здесь, а не в задаче 7: без неё импорту некуда
  писать. Задача 7 остаётся про чтение из интерфейса и `--rebuild-index`;
- тест на путь «тег контейнера → сайдкар» не написан: наш генератор WAV не пишет тег, который читатель
  возвращает (`tags::write` + `tags::read` не дают жанр для WAV), а настоящая коллекция в тесты не
  входит. Покрыто: строка индекса с жанрами, пустой список без тега, чтение старой записи.

### Задача 5. Жанры при анализе

**Файлы:** `apps/dj-engine/src/meta.rs` (запись `tags` — строка 302), `apps/dj-engine/tests/genres.rs`
(новый).

**Шаг 1 — падающий тест.** Анализ файла с тегом `"Deep House; Soulful"` → в `.djmeta` два жанра, в базе
две связи; повторный анализ с изменённым тегом заменяет связи, а не добавляет.

**Шаг 2 — прогон.** FAIL.

**Шаг 3 — реализация.** В месте, где анализ пишет `meta.tags`, писать `genres`; после `save` —
`upsert_with_genres` в ту же базу, что у импорта.

**Шаг 4 — прогон.** PASS.

**Шаг 5 — коммит.** `feat(engine): анализ извлекает жанры и пишет их в базу`.

**Закрыто.** Что вышло иначе, чем в плане:

- `write_names` получил `force` и `Option<&mut Index>`: без просьбы запись с именами не перечитывается
  (как и раньше), с просьбой — перечитывается и заменяет связи. Флаг нужен потому, что тег мог сменить
  другой редактор;
- строка индекса строится из записи, а не из файла (`meta::row_of`): имя, поправленное DJ, — то имя,
  которое видит список. `genre` в строке — жанры, сшитые обратно через `", "`; принадлежность решают
  связи, а не эта колонка;
- дверь к базе на пути загрузки трека открывается один раз на загрузку (`Index::open_default`).

**Тесты** (`apps/dj-engine/tests/genres.rs`, 3): тег `"Deep House; Soulful"` даёт два жанра в
`.djmeta` и две связи в индексе; второй анализ с `"Techno"` заменяет связи; запись с именами без
просьбы не перечитывается.

**Ограничение, найденное по дороге:** `symphonia-format-riff` 0.6 не читает теги из RIFF вообще — ни
`INFO`, ни `id3`. Значит, у WAV-файлов жанры в индекс не попадут, пока теги не читаются другим
способом. Тесты жанров поэтому идут на FLAC из фикстур `crates/dj-codec/tests/fixtures/audio/`.
Подробности — `docs/qa/evidence/B08/genres.md`.

### Задача 6. Пересборка базы обходом корней

**Файлы:** `crates/dj-library/src/index.rs` (`rebuild_from`), `apps/dj-engine/src/main.rs`
(`--rebuild-index`), `crates/dj-library/tests/rebuild.rs` (новый).

**Шаг 1 — падающий тест.** Корень с тремя `.djmeta` (у двух есть жанры) → `rebuild_from` даёт 3 строки,
4 связи; повторный запуск не плодит дубликатов; битый `.djmeta` пропускается, остальные на месте.

**Шаг 2 — прогон.** FAIL.

**Шаг 3 — реализация.** Обход только `.djmeta` (аудио не открывается), `INSERT OR REPLACE` в одной
транзакции на 500 строк; прогресс — тем же `Progress`, что у импорта.

**Шаг 4 — прогон.** PASS.

**Шаг 5 — коммит.** `feat(library): пересборка индекса из .djmeta`.

### Задача 7. Одна дверь к базе

**Файлы:** `crates/dj-library/src/index.rs` (`Index::open_default`), `apps/dj-engine/src/meta.rs`,
`apps/dj-ui/src/BridgeClient.cpp`.

**Шаг 1 — падающий тест.** `Index::open_default()` при `XDG_DATA_HOME=/tmp/x` создаёт
`/tmp/x/hauler-dj/db/library.db` и каталог; второй вызов открывает ту же базу.

**Шаг 2 — прогон.** FAIL.

**Шаг 3 — реализация.** Путь из `$XDG_DATA_HOME` (fallback `~/.local/share`), каталог создаётся,
`journal_mode = WAL`.

**Шаг 4 — прогон.** PASS.

**Шаг 5 — коммит.** `feat(library): путь базы $XDG_DATA_HOME/hauler-dj/db/library.db`.

### Задача 8. Фильтр по жанрам в браузере

**Файлы:** `apps/dj-ui/src/BridgeClient.h/.cpp` (`genreList`, `setGenreFilter`, `applyFilter`),
`apps/dj-ui/qml/browser/BrowserPanel.qml` (полоса жанров в разделе Collection), `apps/dj-ui/src/main.cpp`
(`runGenreCheck`), `scripts/ui.sh` (`--genre-check`).

**Шаг 1 — падающий тест (UI).** `--genre-check` на синтетической коллекции: `genreList()` возвращает
`House` и `Techno` с количеством; `setGenreFilter(["Techno"])` оставляет только строки с этим жанром;
`setGenreFilter(["Techno","Ambient"])` — объединение; пустой список возвращает всё.

**Шаг 2 — прогон.** FAIL.

**Шаг 3 — реализация.** `FileBrowser` открывает базу только на чтение; `applyFilter` при непустом
жанровом фильтре берёт множество fingerprint-ов из `tracks_with_genres` и оставляет строки из `m_all`,
у которых fingerprint есть в множестве. В QML — чипы жанров над списком (мультивыбор, «сбросить»),
состояние — в `LayoutSettings`, как прочие настройки вида.

**Шаг 4 — прогон.** PASS.

**Шаг 5 — коммит.** `feat(ui): фильтр коллекции по жанрам`.

### Задача 9. Приёмка на 100 000

**Файлы:** `apps/dj-ui/src/main.cpp` (`--collection-probe` расширяется), `docs/qa/evidence/B08/`
(новый файл `genres.md`).

**Шаг 1.** Синтетическая база на 100 000 строк и 30 жанров; замер: `genre_list` и
`tracks_with_genres(["Techno"])` — по 20 прогонов, p95.

**Шаг 2.** Требование `AC-MEDIA`: фильтр p95 < 200 мс, список жанров p95 < 50 мс. Записать в evidence
числа и команду.

**Шаг 3 — коммит.** `test(ui): приёмка фильтра по жанрам на 100 000 строк`.

### Задача 10. Документация и трассируемость

**Файлы:** `docs/architecture/library.md` (раздел «База коллекции»), `docs/requirements/traceability.csv`
(строка на фильтр по жанрам), `docs/qa/evidence/B08/genres.md`.

Записать: путь базы, схему, правила разбора, что `.djmeta` главнее, что база производная и
пересобирается, семантику «ИЛИ», замеры.

---

## Что не входит в этот срез

- Переход самого списка на базу (`LIMIT/OFFSET`, `QAbstractListModel`). Сегодняшние 1 178 мс на 100 000
  при входе в раздел и 2 090 мс на сортировку остаются; это отдельная задача `MEDIA-007` с собственными
  критериями. Список при этом остаётся **непрерывным**, батчи — только предзагрузка (см. «Следующий
  срез»).
- Сортировка по `Key` в SQL (нужна вычисляемая колонка квинтового круга) и по `BPM` с точностью до
  сотых.
- Smart playlists, обложки, очередь импорта с паузой — тоже `MEDIA-007`.

---

## Риски

- **Движок пишет, интерфейс читает.** WAL + `busy_timeout`; UI открывает базу `SQLITE_OPEN_READ_ONLY`.
  Проверить на живом прогоне: импорт 500 файлов при открытом браузере не даёт `database is locked`.
- **Расхождение базы и `.djmeta`.** База производная: единственный способ починить — пересборка.
  Поэтому `.djmeta` пишется всегда, даже если запись в базу не удалась (ошибка базы не должна ломать
  импорт: пишем в лог, продолжаем).
- **Грязные теги.** «Electronic; House; Deep House» и «Dance/Pop» встречаются; правила разбора
  консервативные, неразобранное остаётся одним жанром как есть.
- **Регистр и Юникод.** `to_lowercase()` работает и для кириллицы; тест на «Техно»/«техно» обязателен.
- **Размер базы.** 100 000 строк и ~300 000 связей — единицы мегабайт; `VACUUM` не нужен.

## Ответы владельца (2026-11-12)

На все пять вопросов — «да»:

1. Разделители `,` `;` `/`; `&` не разделяет — «Drum & Bass» остаётся одним жанром.
2. Жанры сравниваются без учёта регистра; в списке показывается первое встреченное написание.
3. Несколько выбранных жанров — «или».
4. База пересобирается автоматически, если она пуста, а коллекция нет (в фоне, с прогрессом); команда
   `--rebuild-index` остаётся для рук.
5. В списке жанров — количества и сортировка по ним.

## Следующий срез: список на базе

Ограничение владельца: «Сам список в интерфейсе должен быть непрерывным. А уже внутри при прокрутке
можно использовать предзагрузку по страницам (вернее, батчами).»

То есть постраничность — не механика интерфейса с «страница 1 из N», а внутренняя: в окне остаётся одно
непрерывное полотно, а строки подгружаются батчами по мере прокрутки. Поиск и фильтр по жанру при этом
идут запросом в базу (`WHERE … ORDER BY … LIMIT ? OFFSET ?`), а не перебором всего `m_all`.

Критерии этого среза:

- вход в раздел и прокрутка не зависят от размера коллекции (100 000 строк — те же ощущения, что 1 000);
- сортировка по шапке — `ORDER BY` в базе, а не пересортировка всех строк в памяти;
- курсор и выделение не сбиваются при догрузке батча (строка под курсором остаётся той же строкой);
- память интерфейса не растёт с коллекцией: в модели живёт окно строк, а не вся коллекция.
