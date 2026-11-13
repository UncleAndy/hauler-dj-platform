//! The SQLite index over portable metadata (DJP-DATA-004, DJP-DATA-006).
//!
//! Only what a library is searched and sorted by lives here. Cue points and grids stay in the
//! metadata files, so this database can be deleted and rebuilt by scanning without losing one bit of
//! what the user decided. Writes happen off the audio thread; the audio path never touches a file.

use std::path::{Path, PathBuf};

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use crate::Progress;
use crate::genres::{self, GenreName};
use crate::meta::TrackMeta;
use crate::sidecar;

/// A row of the index: the searchable face of one track.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRow {
    /// Fingerprint key: the same bytes on another machine land on the same row.
    pub fingerprint: String,
    /// Where the file is now.
    pub path: String,
    /// Tags, when the file carries them.
    pub artist: String,
    /// Title, falling back to the file name.
    pub title: String,
    /// Album.
    pub album: String,
    /// Genre.
    pub genre: String,
    /// Year, when known.
    pub year: Option<i64>,
    /// Tempo, when analysed.
    pub bpm: Option<f64>,
    /// Musical key, when analysed.
    pub musical_key: Option<String>,
    /// Rating 0..=5.
    pub rating: u8,
    /// Length in frames of the source file.
    pub duration_frames: Option<u64>,
    /// Native sample rate of the file: without it the frames are a number, not a length.
    pub sample_rate: Option<u32>,
    /// Channel gain from the metadata file.
    pub gain: f64,
    /// How many times the track was played.
    pub play_count: u64,
    /// When it was last played, milliseconds since the epoch.
    pub last_played_ms: Option<u64>,
    /// True when analysis must not overwrite the grid.
    pub analysis_lock: bool,
}

/// What the index can fail at.
#[derive(Debug)]
pub enum IndexError {
    /// SQLite refused.
    Sqlite(rusqlite::Error),
    /// The file could not be created or opened.
    Io(std::io::Error),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "sqlite: {error}"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for IndexError {}

impl From<rusqlite::Error> for IndexError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<std::io::Error> for IndexError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// The schema, version 2.
///
/// It only ever grows: a table added here is added to every existing database on the next open, and
/// nothing the DJ already has is dropped. The database is a cache over the metadata files, so a broken
/// one is rebuilt by scanning — but a cache that throws away a working row forces a rescan of the whole
/// collection, which is not something a version bump may do.
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS tracks (
    fingerprint TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    artist TEXT NOT NULL DEFAULT '',
    title TEXT NOT NULL DEFAULT '',
    album TEXT NOT NULL DEFAULT '',
    genre TEXT NOT NULL DEFAULT '',
    year INTEGER,
    bpm REAL,
    musical_key TEXT,
    rating INTEGER NOT NULL DEFAULT 0,
    duration_frames INTEGER,
    sample_rate INTEGER,
    gain REAL NOT NULL DEFAULT 1.0,
    play_count INTEGER NOT NULL DEFAULT 0,
    last_played_ms INTEGER,
    analysis_lock INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS genres (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    key  TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS track_genres (
    fingerprint TEXT NOT NULL REFERENCES tracks(fingerprint) ON DELETE CASCADE,
    genre_id    INTEGER NOT NULL REFERENCES genres(id) ON DELETE CASCADE,
    PRIMARY KEY (fingerprint, genre_id)
);
CREATE INDEX IF NOT EXISTS genres_key         ON genres(key);
CREATE INDEX IF NOT EXISTS track_genres_genre ON track_genres(genre_id, fingerprint);
PRAGMA user_version = 3;
";

/// The database file. One writer, opened off the audio path.
pub struct Index {
    connection: Connection,
}

impl Index {
    /// Open (or create) the index at `path`.
    pub fn open(path: &Path) -> Result<Self, IndexError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        // WAL keeps a reader from blocking the writer; the schema is versioned so a migration can
        // rebuild rather than guess.
        connection.pragma_update(None, "journal_mode", "WAL")?;
        // Two processes touch this file — the engine writes while the browser reads — so a lock is
        // waited for instead of failing, and a link row cannot be left half-written.
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "busy_timeout", 3000)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        // The version is read before the schema runs: the schema stamps the current one, and a
        // database from before a column existed would then look up to date.
        let previous: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        connection.execute_batch(SCHEMA)?;
        if (1..3).contains(&previous) {
            let mut statement = connection.prepare("PRAGMA table_info(tracks)")?;
            let columns: Vec<String> = statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            if !columns.iter().any(|name| name == "sample_rate") {
                connection.execute_batch("ALTER TABLE tracks ADD COLUMN sample_rate INTEGER")?;
            }
        }
        Ok(Self { connection })
    }

    /// The index in the user's data folder: `$XDG_DATA_HOME/hauler-dj/db/library.db`, or
    /// `~/.local/share/hauler-dj/db/library.db` when the variable is unset.
    ///
    /// The database is a cache over the metadata files, so a missing one is not an error to report:
    /// the caller opens it, writes what it has, and a rebuild fills the rest.
    pub fn open_default() -> Result<Self, IndexError> {
        let folder = data_folder()
            .ok_or_else(|| IndexError::Io(std::io::Error::other("no data folder for the index")))?;
        Self::open_in(&folder)
    }

    /// The index for a reader: opened read-only, with no schema and no migrations.
    ///
    /// `open` runs the schema and the older-column migration, which is a write. A reader that does this
    /// while the engine's loader is writing metadata takes the writer's lock away for a moment — and a
    /// load that cannot record its names drops the track it was loading. The deck then keeps the file it
    /// had, which looks like a card that ignores its buttons. A reader gets exactly these rights: if the
    /// file or the column is not there, the answer is "nothing to show", and the writer builds it.
    pub fn reader_default() -> Result<Self, IndexError> {
        let folder = data_folder()
            .ok_or_else(|| IndexError::Io(std::io::Error::other("no data folder for the index")))?;
        Self::reader_in(&folder)
    }

    /// The reader's door under a data folder: the same file as `open_in`, opened read-only.
    pub fn reader_in(folder: &Path) -> Result<Self, IndexError> {
        let path = folder.join("hauler-dj").join("db").join("library.db");
        if !path.exists() {
            return Err(IndexError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no index yet",
            )));
        }
        let connection =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.pragma_update(None, "busy_timeout", 3000)?;
        Ok(Self { connection })
    }

    /// The index under a data folder: `<folder>/hauler-dj/db/library.db`, with the folders made.
    ///
    /// Every caller — the engine writing, the browser reading, the rebuild — comes through here, so
    /// there is one path and one file. A caller that builds the path itself is a second truth.
    pub fn open_in(folder: &Path) -> Result<Self, IndexError> {
        Self::open(&folder.join("hauler-dj").join("db").join("library.db"))
    }

    /// Replace the whole index with these rows: the rebuild path after the database is deleted.
    pub fn rebuild(&mut self, rows: &[TrackRow]) -> Result<usize, IndexError> {
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM tracks", [])?;
        {
            let mut statement = transaction.prepare(INSERT)?;
            for row in rows {
                bind(&mut statement, row)?;
            }
        }
        transaction.commit()?;
        Ok(rows.len())
    }

    /// Make the index out of the `.djmeta` records under these roots.
    ///
    /// The records are the source of truth and the database is a cache, so a rebuild starts from
    /// nothing: rows whose records are gone must not survive. The audio is never opened — a record
    /// carries the fingerprint, the names and the genres already.
    pub fn rebuild_from(&mut self, roots: &[PathBuf]) -> Result<RebuildReport, IndexError> {
        self.rebuild_from_with(roots, &mut |_| {})
    }

    /// Fill the index from the records under these roots, keeping every row it already holds.
    ///
    /// A rebuild starts from nothing because the records are the truth; this one is for the other case —
    /// the list reads the database, the database was never built, and the folders do hold records. Rows
    /// that are already there stay, so filling one folder cannot erase another.
    pub fn fill_from(&mut self, roots: &[PathBuf]) -> Result<RebuildReport, IndexError> {
        let records = records_under(roots);
        let mut report = RebuildReport::default();
        let mut batch: Vec<(TrackRow, Vec<GenreName>)> = Vec::new();
        for record in records {
            match read_record(&record) {
                Some((row, genres)) => {
                    report.tracks += 1;
                    report.genres += genres.len();
                    batch.push((row, genres));
                }
                None => report.skipped += 1,
            }
            if batch.len() >= REBUILD_BATCH {
                // `false`: this is a fill, not a rebuild — nothing is deleted.
                self.write_batch(&mut batch, false)?;
            }
        }
        self.write_batch(&mut batch, false)?;
        Ok(report)
    }

    /// The same rebuild, reporting how far it has come: the same [`Progress`] the import uses, so the
    /// window that shows an import can show this too.
    pub fn rebuild_from_with(
        &mut self,
        roots: &[PathBuf],
        progress: &mut dyn FnMut(&Progress),
    ) -> Result<RebuildReport, IndexError> {
        let records = records_under(roots);
        let mut state = Progress {
            total: records.len(),
            running: true,
            ..Progress::default()
        };
        progress(&state);
        let mut report = RebuildReport::default();
        let mut batch: Vec<(TrackRow, Vec<GenreName>)> = Vec::new();
        let mut clear = true;
        for record in records {
            state.current = Some(record.clone());
            match read_record(&record) {
                Some((row, genres)) => {
                    report.tracks += 1;
                    report.genres += genres.len();
                    state.done = report.tracks;
                    batch.push((row, genres));
                }
                None => {
                    report.skipped += 1;
                    state.failed += 1;
                }
            }
            if batch.len() >= REBUILD_BATCH {
                self.write_batch(&mut batch, clear)?;
                clear = false;
                progress(&state);
            }
        }
        self.write_batch(&mut batch, clear)?;
        state.running = false;
        progress(&state);
        Ok(report)
    }

    /// Write one batch in one transaction. The first batch also empties the index.
    fn write_batch(
        &mut self,
        batch: &mut Vec<(TrackRow, Vec<GenreName>)>,
        clear: bool,
    ) -> Result<(), IndexError> {
        if batch.is_empty() && !clear {
            return Ok(());
        }
        let transaction = self.connection.transaction()?;
        if clear {
            transaction.execute("DELETE FROM tracks", [])?;
        }
        {
            let mut statement = transaction.prepare(INSERT)?;
            for (row, _) in batch.iter() {
                bind(&mut statement, row)?;
            }
        }
        for (row, genres) in batch.iter() {
            Self::write_genres(&transaction, &row.fingerprint, genres)?;
        }
        transaction.commit()?;
        batch.clear();
        Ok(())
    }

    /// Insert or replace one row, leaving the rest of the index alone.
    pub fn upsert(&mut self, row: &TrackRow) -> Result<(), IndexError> {
        let transaction = self.connection.transaction()?;
        {
            let mut statement = transaction.prepare(INSERT)?;
            bind(&mut statement, row)?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Replace the genres of one track.
    ///
    /// A tag is a statement about the track, not a growing list: setting the genres of a track that
    /// already has some replaces them, or a retagged track would keep both the old and the new genre.
    pub fn set_genres(
        &mut self,
        fingerprint: &str,
        genres: &[GenreName],
    ) -> Result<(), IndexError> {
        let transaction = self.connection.transaction()?;
        Self::write_genres(&transaction, fingerprint, genres)?;
        transaction.commit()?;
        Ok(())
    }

    /// A row and its genres in one transaction: a track is never listed without the genres its tag
    /// carried, and a genre is never left pointing at a row that was rolled back.
    pub fn upsert_with_genres(
        &mut self,
        row: &TrackRow,
        genres: &[GenreName],
    ) -> Result<(), IndexError> {
        let transaction = self.connection.transaction()?;
        {
            let mut statement = transaction.prepare(INSERT)?;
            bind(&mut statement, row)?;
        }
        Self::write_genres(&transaction, &row.fingerprint, genres)?;
        transaction.commit()?;
        Ok(())
    }

    /// The genres of one track, by name.
    pub fn genres_of(&self, fingerprint: &str) -> Result<Vec<String>, IndexError> {
        let mut statement = self.connection.prepare(
            "SELECT g.name FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
             WHERE tg.fingerprint = ?1 ORDER BY g.name COLLATE NOCASE",
        )?;
        let names = statement
            .query_map([fingerprint], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(names)
    }

    /// Every genre the collection carries, with the number of tracks, sorted by name.
    pub fn genre_list(&self) -> Result<Vec<(String, u64)>, IndexError> {
        let mut statement = self.connection.prepare(
            "SELECT g.name, COUNT(*) FROM genres g
             JOIN track_genres tg ON tg.genre_id = g.id
             GROUP BY g.id ORDER BY g.name COLLATE NOCASE",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?.max(0) as u64,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The genres carried under these roots, with how many tracks carry each.
    ///
    /// Empty roots is the whole index. This is what the genre boxes ask: the answer comes from the
    /// `genres` table and its links, never from counting the rows of one page.
    pub fn genre_list_in(
        &self,
        roots: &[std::path::PathBuf],
    ) -> Result<Vec<(String, u64)>, IndexError> {
        let mut args: Vec<Value> = Vec::new();
        let mut filter = String::new();
        if !roots.is_empty() {
            let mut marks = Vec::with_capacity(roots.len());
            for folder in roots {
                let prefix = format!("{}/", folder.to_string_lossy().trim_end_matches('/'));
                args.push(Value::Text(prefix));
                marks.push(format!("t.path LIKE ?{} || '%'", args.len()));
            }
            filter = format!(" WHERE ({})", marks.join(" OR "));
        }
        let sql = format!(
            "SELECT g.name, COUNT(*) FROM genres g
             JOIN track_genres tg ON tg.genre_id = g.id
             JOIN tracks t ON t.fingerprint = tg.fingerprint{filter}
             GROUP BY g.id ORDER BY g.name COLLATE NOCASE"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(args.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as u64,
            ))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Fingerprints of the tracks carrying any of these genres.
    ///
    /// An empty list selects nothing: «no genre chosen» is the whole collection, and a filter that
    /// returned every track for an empty list would make the caller's mistake look like a feature.
    pub fn tracks_with_genres(&self, wanted: &[String]) -> Result<Vec<String>, IndexError> {
        let mut keys: Vec<String> = Vec::new();
        for raw in wanted {
            let Some(genre) = genres::normalise(raw) else {
                continue;
            };
            if !keys.contains(&genre.key) {
                keys.push(genre.key);
            }
        }
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (1..=keys.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT DISTINCT tg.fingerprint FROM track_genres tg
             JOIN genres g ON g.id = tg.genre_id
             WHERE g.key IN ({placeholders}) ORDER BY tg.fingerprint"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let bound: Vec<&dyn rusqlite::ToSql> =
            keys.iter().map(|key| key as &dyn rusqlite::ToSql).collect();
        let rows = statement
            .query_map(bound.as_slice(), |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Write the links of one track: replace what was there, reuse genres that already exist.
    fn write_genres(
        transaction: &rusqlite::Transaction<'_>,
        fingerprint: &str,
        genres: &[GenreName],
    ) -> Result<(), IndexError> {
        transaction.execute(
            "DELETE FROM track_genres WHERE fingerprint = ?1",
            [fingerprint],
        )?;
        for genre in genres {
            // The first spelling of a genre wins: `key` is unique, so a later `House` does not
            // overwrite the name the collection already shows.
            transaction.execute(
                "INSERT INTO genres (name, key) VALUES (?1, ?2) ON CONFLICT(key) DO NOTHING",
                params![genre.name, genre.key],
            )?;
            let id: i64 = transaction.query_row(
                "SELECT id FROM genres WHERE key = ?1",
                [&genre.key],
                |row| row.get(0),
            )?;
            transaction.execute(
                "INSERT OR IGNORE INTO track_genres (fingerprint, genre_id) VALUES (?1, ?2)",
                params![fingerprint, id],
            )?;
        }
        Ok(())
    }

    /// How many tracks the index knows.
    pub fn count(&self) -> Result<u64, IndexError> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// Find a track by fingerprint, which survives renames and moves.
    pub fn by_fingerprint(&self, fingerprint: &str) -> Result<Option<TrackRow>, IndexError> {
        let row = self
            .connection
            .query_row(
                "SELECT fingerprint, path, artist, title, album, genre, year, bpm, musical_key,
                        rating, duration_frames, sample_rate, gain, play_count, last_played_ms, analysis_lock
                 FROM tracks WHERE fingerprint = ?1",
                [fingerprint],
                Self::row,
            )
            .optional()?;
        Ok(row)
    }

    /// Search titles, artists and albums.
    pub fn search(&self, needle: &str) -> Result<Vec<TrackRow>, IndexError> {
        let pattern = format!("%{needle}%");
        let mut statement = self.connection.prepare(
            "SELECT fingerprint, path, artist, title, album, genre, year, bpm, musical_key,
                    rating, duration_frames, sample_rate, gain, play_count, last_played_ms, analysis_lock
             FROM tracks
             WHERE title LIKE ?1 OR artist LIKE ?1 OR album LIKE ?1
             ORDER BY artist, album, title",
        )?;
        let rows = statement
            .query_map([pattern], Self::row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// One page of the collection: ordered, filtered and searched **in SQL**.
    ///
    /// This is the question the interface asks over the bridge. A page keeps the cost of opening the
    /// section the same on a hundred thousand rows as on a thousand, and the ordering is the database's
    /// job — a second sort in the caller is a second opinion about what the DJ clicked.
    pub fn page(&self, query: &PageQuery) -> Result<Page, IndexError> {
        let mut args: Vec<Value> = Vec::new();
        let mut filters: Vec<String> = Vec::new();

        // The index is one for the whole machine; the list is one for the roots the DJ opened. Without
        // this, a collection of forty records would show whatever else the index happens to hold.
        if !query.roots.is_empty() {
            let mut marks = Vec::with_capacity(query.roots.len());
            for folder in &query.roots {
                let prefix = format!("{}/", folder.to_string_lossy().trim_end_matches('/'));
                args.push(Value::Text(prefix));
                marks.push(format!("path LIKE ?{} || '%'", args.len()));
            }
            filters.push(format!("({})", marks.join(" OR ")));
        }

        if let Some(search) = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            args.push(Value::Text(format!("%{}%", search.to_lowercase())));
            let at = args.len();
            filters.push(format!(
                "(lower(artist) LIKE ?{at} OR lower(album) LIKE ?{at} OR lower(title) LIKE ?{at})"
            ));
        }

        let keys: Vec<String> = query
            .genres
            .iter()
            .filter_map(|raw| genres::normalise(raw))
            .map(|genre| genre.key)
            .collect();
        if !keys.is_empty() {
            let mut marks = Vec::with_capacity(keys.len());
            for key in keys {
                args.push(Value::Text(key));
                marks.push(format!("?{}", args.len()));
            }
            // One genre is enough: the chips are a union, not a demand that a track be both at once.
            filters.push(format!(
                "EXISTS (SELECT 1 FROM track_genres tg JOIN genres g ON g.id = tg.genre_id \
                 WHERE tg.fingerprint = tracks.fingerprint AND g.key IN ({}))",
                marks.join(", ")
            ));
        }

        let where_sql = if filters.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", filters.join(" AND "))
        };

        // The count is what the filter found, not what the page holds: the list needs to know how long
        // it is before it has been scrolled to the end.
        let total: i64 = self.connection.query_row(
            &format!("SELECT COUNT(*) FROM tracks{where_sql}"),
            params_from_iter(args.iter()),
            |row| row.get(0),
        )?;

        // A page of zero is not a page of nothing: it is a caller that did not choose a width.
        let limit = if query.limit == 0 { 100 } else { query.limit };
        args.push(Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
        let limit_at = args.len();
        args.push(Value::Integer(
            i64::try_from(query.offset).unwrap_or(i64::MAX),
        ));
        let offset_at = args.len();

        let sql = format!(
            "SELECT fingerprint, path, artist, title, album, genre, year, bpm, musical_key,
                    rating, duration_frames, sample_rate, gain, play_count, last_played_ms, analysis_lock
             FROM tracks{where_sql}{} LIMIT ?{limit_at} OFFSET ?{offset_at}",
            order_clause(&query.order)
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement
            .query_map(params_from_iter(args.iter()), Self::row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Page {
            rows,
            total: usize::try_from(total).unwrap_or(usize::MAX),
        })
    }

    /// Column names, so a test can prove that cue points do not live here.
    pub fn columns(&self) -> Result<Vec<String>, IndexError> {
        let mut statement = self.connection.prepare("PRAGMA table_info(tracks)")?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(names)
    }

    fn row(raw: &rusqlite::Row<'_>) -> rusqlite::Result<TrackRow> {
        Ok(TrackRow {
            fingerprint: raw.get(0)?,
            path: raw.get(1)?,
            artist: raw.get(2)?,
            title: raw.get(3)?,
            album: raw.get(4)?,
            genre: raw.get(5)?,
            year: raw.get(6)?,
            bpm: raw.get(7)?,
            musical_key: raw.get(8)?,
            rating: raw.get::<_, i64>(9)?.clamp(0, 255) as u8,
            duration_frames: raw
                .get::<_, Option<i64>>(10)?
                .map(|frames| frames.max(0) as u64),
            sample_rate: raw
                .get::<_, Option<i64>>(11)?
                .map(|rate| rate.clamp(0, i64::from(u32::MAX)) as u32),
            gain: raw.get(12)?,
            play_count: raw.get::<_, i64>(13)?.max(0) as u64,
            last_played_ms: raw.get::<_, Option<i64>>(14)?.map(|ms| ms.max(0) as u64),
            analysis_lock: raw.get(15)?,
        })
    }
}

/// One statement for both paths: a rebuild and a single row must write the same columns.
/// The user's data folder: `$XDG_DATA_HOME` when it is absolute, else `$HOME/.local/share`.
///
/// The environment is read here and nowhere else: a second reader of `XDG_DATA_HOME` is a second
/// opinion about where the database lives.
pub fn data_folder() -> Option<PathBuf> {
    data_folder_from(
        std::env::var_os("XDG_DATA_HOME").as_deref().map(Path::new),
        std::env::var_os("HOME").as_deref().map(Path::new),
    )
}

/// The same decision, taken from values instead of the environment.
///
/// A relative `XDG_DATA_HOME` is not a data folder: the spec says so, and a database that follows the
/// working directory is a database found twice.
pub fn data_folder_from(xdg: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(folder) = xdg.filter(|folder| folder.is_absolute()) {
        return Some(folder.to_path_buf());
    }
    home.map(|home| home.join(".local").join("share"))
}

/// A column the collection can be ordered by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderBy {
    Artist,
    Album,
    Title,
    Bpm,
    Key,
    Time,
}

/// One question about a page of the collection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageQuery {
    /// The sort keys in order, each with its direction. Empty means artist, album, title.
    pub order: Vec<(OrderBy, bool)>,
    /// Genres to narrow to: a track is kept when it carries any of them. Empty is the whole collection.
    pub genres: Vec<String>,
    /// What the DJ typed: artist, album or title.
    pub search: Option<String>,
    pub offset: usize,
    /// How many rows the page holds. Zero means a hundred — a caller that did not choose a width.
    pub limit: usize,
    /// The folders the list is opened on: a row outside them is not in this collection. Empty means
    /// every row, which is what a rebuild or a probe wants.
    pub roots: Vec<std::path::PathBuf>,
}

/// The answer: the rows of the page, and how many the filter found in all.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub rows: Vec<TrackRow>,
    pub total: usize,
}

/// The ORDER BY the sort keys ask for.
///
/// A row with nothing to say in the column goes last whichever way the sort runs: an empty BPM at the
/// top of a descending sort is not the fastest track, it is a missing number.
fn order_clause(order: &[(OrderBy, bool)]) -> String {
    if order.is_empty() {
        return " ORDER BY artist COLLATE NOCASE, album COLLATE NOCASE, title COLLATE NOCASE"
            .to_string();
    }
    let mut parts: Vec<String> = Vec::with_capacity(order.len() * 2);
    for (column, ascending) in order {
        let (empty, name) = match column {
            OrderBy::Artist => ("trim(artist) = ''", "artist COLLATE NOCASE"),
            OrderBy::Album => ("trim(album) = ''", "album COLLATE NOCASE"),
            OrderBy::Title => ("trim(title) = ''", "title COLLATE NOCASE"),
            OrderBy::Bpm => ("bpm IS NULL", "bpm"),
            OrderBy::Key => ("trim(musical_key) = ''", "musical_key COLLATE NOCASE"),
            OrderBy::Time => ("duration_frames IS NULL", "duration_frames"),
        };
        parts.push(format!("CASE WHEN {empty} THEN 1 ELSE 0 END"));
        parts.push(format!(
            "{name} {}",
            if *ascending { "ASC" } else { "DESC" }
        ));
    }
    format!(" ORDER BY {}", parts.join(", "))
}

/// What a rebuild found and wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RebuildReport {
    /// Rows written: one per readable record.
    pub tracks: usize,
    /// Genre links written, counting one per genre a track carries.
    pub genres: usize,
    /// Records that could not be read: a broken `.djmeta` costs its own row, not the rebuild.
    pub skipped: usize,
}

/// How many records one transaction holds: a batch keeps the write fast without holding a lock over
/// a whole collection.
const REBUILD_BATCH: usize = 500;

/// The index row for a record: what the browser sorts by and a search looks in.
///
/// The row is built from the record, not from the file: a name the DJ corrected is the name the list
/// shows. `genre` is the split names joined back together — the links, not this column, decide what
/// the track belongs to.
pub fn row_of(meta: &TrackMeta, path: &Path) -> TrackRow {
    let tags = meta.tags.clone().unwrap_or_default();
    TrackRow {
        fingerprint: meta.fingerprint.key(),
        path: path.to_string_lossy().into_owned(),
        artist: tags.artist,
        title: tags.title,
        album: tags.album,
        genre: tags.genres.join(", "),
        year: None,
        bpm: meta.beatgrid.as_ref().map(|grid| grid.bpm),
        musical_key: meta.key.clone(),
        rating: meta.rating,
        duration_frames: meta.fingerprint.duration_frames,
        sample_rate: meta.fingerprint.sample_rate,
        gain: meta.gain,
        play_count: 0,
        last_played_ms: None,
        analysis_lock: meta.analysis_lock,
    }
}

/// One record and its genres, or nothing when the record cannot be read.
fn read_record(record: &Path) -> Option<(TrackRow, Vec<GenreName>)> {
    let mut meta = sidecar::read(record).ok()?;
    // The track sits next to the record's directory: `.djmeta/<name>.json` describes `<dir>/<name>`.
    let name = record.file_stem()?;
    let dir = record.parent()?.parent()?;
    let track = dir.join(name);
    if meta.tags.is_none() {
        // A record written before tags were kept carries none, and the list would show file names and no
        // genres for the life of the collection — a rescan skips a record that exists. The file still has
        // them, so they are read here, once, on the way into the index.
        if let Ok(tags) = crate::tags::read(&track)
            && (!tags.is_empty() || !tags.genre.trim().is_empty())
        {
            meta.tags = Some(crate::meta::FileTags {
                artist: tags.artist,
                album: tags.album,
                title: tags.title,
                genres: genres::split(&tags.genre)
                    .into_iter()
                    .map(|genre| genre.name)
                    .collect(),
            });
        }
    }
    let genres: Vec<GenreName> = meta
        .tags
        .as_ref()
        .map(|tags| {
            tags.genres
                .iter()
                .filter_map(|name| genres::normalise(name))
                .collect()
        })
        .unwrap_or_default();
    Some((row_of(&meta, &track), genres))
}

/// Every audio file under these roots, without opening one.
///
/// The rescan needs the files themselves, not the records: a track that arrived without a `.djmeta`
/// is exactly the one it has to measure.
pub fn audio_files_under(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in roots {
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|name| name.to_str());
                    if name != Some(sidecar::DIRECTORY)
                        && name != Some(crate::import::DUPLICATES_DIR)
                    {
                        stack.push(path);
                    }
                    continue;
                }
                let audio = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .and_then(dj_codec::MediaFormat::from_extension)
                    .is_some();
                if audio {
                    found.push(path);
                }
            }
        }
    }
    found.sort();
    found
}

fn records_under(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in roots {
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = path.file_name().and_then(|name| name.to_str());
                if name == Some(sidecar::DIRECTORY) {
                    let Ok(records) = std::fs::read_dir(&path) else {
                        continue;
                    };
                    for record in records.flatten() {
                        let record = record.path();
                        if record
                            .extension()
                            .is_some_and(|extension| extension == "json")
                        {
                            found.push(record);
                        }
                    }
                } else if name != Some(crate::import::DUPLICATES_DIR) {
                    stack.push(path);
                }
            }
        }
    }
    found.sort();
    found
}

const INSERT: &str = "INSERT OR REPLACE INTO tracks
     (fingerprint, path, artist, title, album, genre, year, bpm, musical_key, rating,
      duration_frames, sample_rate, gain, play_count, last_played_ms, analysis_lock)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)";

fn bind(statement: &mut rusqlite::Statement<'_>, row: &TrackRow) -> Result<(), IndexError> {
    statement.execute(params![
        row.fingerprint,
        row.path,
        row.artist,
        row.title,
        row.album,
        row.genre,
        row.year,
        row.bpm,
        row.musical_key,
        row.rating,
        row.duration_frames.map(|frames| frames as i64),
        row.sample_rate.map(i64::from),
        row.gain,
        row.play_count as i64,
        row.last_played_ms.map(|ms| ms as i64),
        row.analysis_lock,
    ])?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// A database as the previous build left it: the `tracks` table, one row, version 1.
    fn version_one(path: &Path) {
        let connection = Connection::open(path).expect("open");
        connection
            .execute_batch(
                "CREATE TABLE tracks (
                    fingerprint TEXT PRIMARY KEY,
                    path TEXT NOT NULL,
                    artist TEXT NOT NULL DEFAULT '',
                    title TEXT NOT NULL DEFAULT '',
                    album TEXT NOT NULL DEFAULT '',
                    genre TEXT NOT NULL DEFAULT '',
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
                INSERT INTO tracks (fingerprint, path, artist, title)
                    VALUES ('abc', '/music/a.flac', 'Artist', 'Title');
                PRAGMA user_version = 1;",
            )
            .expect("version one schema");
    }

    fn table_exists(index: &Index, name: &str) -> bool {
        index
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [name],
                |row| row.get::<_, i64>(0),
            )
            .expect("sqlite_master")
            == 1
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dj-index-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// The schema moves forward on open, and nothing the DJ already had is lost: the database is a
    /// cache over the metadata files, but a cache that throws away a working row is a cache that
    /// forces a rescan of the whole collection.
    #[test]
    fn a_version_one_index_gains_genres_and_keeps_its_rows() {
        let dir = scratch("schema");
        let path = dir.join("library.db");
        version_one(&path);

        let index = Index::open(&path).expect("open");
        let version: i64 = index
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user_version");
        assert_eq!(version, 3, "the schema must move to version 3");
        assert!(
            index
                .columns()
                .expect("columns")
                .iter()
                .any(|name| name == "sample_rate"),
            "an older database gains the rate column"
        );
        assert!(table_exists(&index, "genres"), "genres table");
        assert!(table_exists(&index, "track_genres"), "track_genres table");
        assert_eq!(
            index.count().expect("count"),
            1,
            "the row from version one survives"
        );
        let row = index.by_fingerprint("abc").expect("read").expect("row");
        assert_eq!(row.artist, "Artist");
        assert_eq!(row.title, "Title");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Opening twice is what every start does: the second open must not undo the first.
    #[test]
    fn opening_an_index_twice_is_harmless() {
        let dir = scratch("twice");
        let path = dir.join("library.db");
        {
            let index = Index::open(&path).expect("first open");
            assert_eq!(index.count().expect("count"), 0);
        }
        let index = Index::open(&path).expect("second open");
        let version: i64 = index
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user_version");
        assert_eq!(version, 3);
        assert!(table_exists(&index, "track_genres"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
