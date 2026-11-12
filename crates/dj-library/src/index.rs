//! The SQLite index over portable metadata (DJP-DATA-004, DJP-DATA-006).
//!
//! Only what a library is searched and sorted by lives here. Cue points and grids stay in the
//! metadata files, so this database can be deleted and rebuilt by scanning without losing one bit of
//! what the user decided. Writes happen off the audio thread; the audio path never touches a file.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

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
PRAGMA user_version = 2;
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
        connection.execute_batch(SCHEMA)?;
        Ok(Self { connection })
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
                        rating, duration_frames, gain, play_count, last_played_ms, analysis_lock
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
                    rating, duration_frames, gain, play_count, last_played_ms, analysis_lock
             FROM tracks
             WHERE title LIKE ?1 OR artist LIKE ?1 OR album LIKE ?1
             ORDER BY artist, album, title",
        )?;
        let rows = statement
            .query_map([pattern], Self::row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
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
            gain: raw.get(11)?,
            play_count: raw.get::<_, i64>(12)?.max(0) as u64,
            last_played_ms: raw.get::<_, Option<i64>>(13)?.map(|ms| ms.max(0) as u64),
            analysis_lock: raw.get(14)?,
        })
    }
}

/// One statement for both paths: a rebuild and a single row must write the same columns.
const INSERT: &str = "INSERT OR REPLACE INTO tracks
     (fingerprint, path, artist, title, album, genre, year, bpm, musical_key, rating,
      duration_frames, gain, play_count, last_played_ms, analysis_lock)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)";

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
        assert_eq!(version, 2, "the schema must move to version 2");
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
        assert_eq!(version, 2);
        assert!(table_exists(&index, "track_genres"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
