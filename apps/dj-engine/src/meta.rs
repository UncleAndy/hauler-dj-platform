//! Reading and writing a track's own metadata from the engine side (DJP-DATA-006).
//!
//! Two moments matter. When a file is loaded, the cues and the loop it carried go back on the deck,
//! so a reload looks like a reload. When the user moves a cue or sets a loop, the change is sent to
//! this module from the graph and written here — never in the callback, which must not touch a file.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dj_domain::{BeatGrid, BeatSegment};
use dj_library::{Cue, Store, TrackMeta, fingerprint};

use crate::{MetaEdit, PreparedTrack};

/// How long the shutdown waits for the last edit to reach the disk before saying it gave up.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// The thread that turns deck edits into files (DJP-DATA-006).
///
/// The audio callback may not write a file, so edits are queued and written here, in order. Queued
/// means "not yet stored", and the process leaves as soon as the run loop stops — so the queue is
/// closed and the thread awaited on the way out. A cue the DJ set and the engine forgot is the one
/// failure this module exists to prevent.
pub struct MetaWriter {
    /// Our own sender: dropping it closes the queue, which is what ends the thread.
    queue: Option<Sender<MetaEdit>>,
    thread: Option<JoinHandle<()>>,
}

impl MetaWriter {
    /// Start the writer for a store, a volume and a music root.
    ///
    /// The returned sender goes to whoever publishes edits; the writer itself must outlive it and
    /// is dropped last, so the queue is closed after the graph is gone.
    pub fn spawn(store: Store, volume: String, root: PathBuf) -> (Self, Sender<MetaEdit>) {
        let (queue, receiver) = mpsc::channel::<MetaEdit>();
        let handoff = queue.clone();
        let thread = thread::Builder::new()
            .name("dj-meta".to_string())
            .spawn(move || {
                while let Ok(edit) = receiver.recv() {
                    if let Err(message) = write_edit(&store, &volume, &root, &edit) {
                        eprintln!("dj-engine: {message}");
                    }
                }
            })
            .ok();
        (
            Self {
                queue: Some(queue),
                thread,
            },
            handoff,
        )
    }
}

impl Drop for MetaWriter {
    fn drop(&mut self) {
        // Closing our side is what ends the queue: the thread finishes what is already in it.
        drop(self.queue.take());
        let Some(thread) = self.thread.take() else {
            return;
        };
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while !thread.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        if thread.is_finished() {
            let _ = thread.join();
        } else {
            // A writer still running means another sender lives somewhere we do not control.
            // Saying so beats hanging the exit or pretending the edit was stored.
            eprintln!("dj-engine: the metadata writer is still busy; the last edit may be lost");
        }
    }
}

/// Fill a prepared track with what its file remembered.
///
/// A metadata file that cannot be read is not fatal: the track plays with no cues, and the reason
/// stays in the return value so the caller can report it instead of pretending nothing happened.
pub fn restore(
    prepared: &mut PreparedTrack,
    store: &Store,
    volume: &str,
    root: &Path,
) -> Option<String> {
    let path = prepared.path.clone()?;
    match store.load(volume, root, &path) {
        Ok(loaded) => {
            prepared.cues = loaded
                .meta
                .cues
                .iter()
                .map(|cue| (cue.slot, cue.frame))
                .collect();
            prepared.cue_meta = loaded
                .meta
                .cues
                .iter()
                .map(|cue| (cue.slot, cue.label.clone(), cue.color.clone()))
                .collect();
            // Loops come back on their pads, not as one active loop: which four bars were being played
            // when the file was closed is not something a track remembers.
            prepared.pad_loops = loaded
                .meta
                .loops
                .iter()
                .map(|stored| (stored.slot, stored.start, stored.end))
                .collect();
            // A locked grid is a decision too: the deck must say so the moment the track arrives, and
            // refuse both the measurement and the editor from that first publish.
            prepared.static_info.analysis_lock = loaded.meta.analysis_lock;
            if let Some(track) = prepared.track.as_mut() {
                track.analysis_lock = loaded.meta.analysis_lock;
            }
            // A stored grid outranks the measurement: either the DJ moved it or the analysis was
            // locked, and both are decisions the detector may not overwrite (DJP-SYNC-003).
            if let Some(stored) = loaded.meta.beatgrid.as_ref() {
                // The file states a tempo and pins the rest with anchors; the grid is what the rest of
                // the program speaks. A stored grid that cannot be a grid is ignored rather than trusted.
                let rate = f64::from(prepared.static_info.source_rate);
                let anchors: Vec<(u64, u64, Option<f64>)> = stored
                    .anchors
                    .iter()
                    .map(|anchor| (anchor.frame, anchor.beat, anchor.bpm))
                    .collect();
                let grid = BeatGrid::from_stored(
                    stored.bpm,
                    stored.first_beat_frame,
                    &anchors,
                    rate,
                    stored.beats_per_bar,
                );
                if let Some(grid) = grid {
                    prepared.grid = Some(grid.clone());
                    prepared.static_info.grid = Some(grid.clone());
                    if let Some(track) = prepared.track.as_mut() {
                        track.bpm = Some(grid.dominant_bpm(track.duration_frames));
                        track.first_beat_frame = Some(grid.first_beat_frame());
                        track.grid = Some(grid);
                    }
                    prepared.static_info.track = prepared.track.clone();
                } else if let Some(track) = prepared.track.as_mut() {
                    track.bpm = Some(stored.bpm);
                    track.first_beat_frame = Some(stored.first_beat_frame);
                    prepared.static_info.track = prepared.track.clone();
                }
            }
            // A key in the file is a decision — the DJ's ear, or another program's — and outranks the
            // measurement the loader has just made.
            if let Some(stored) = loaded.meta.key.clone() {
                // The deck speaks the circle of fifths whatever the file holds: a key written by an
                // older build is a name (`A# major`), and a name is translated rather than shown. A
                // value this build cannot read — another program's Camelot code, a hand note — is
                // shown as it stands, because guessing at it would be worse than repeating it.
                let shown = dj_analysis::key::Key::from_name(&stored)
                    .map(|key| key.traktor())
                    .unwrap_or(stored);
                if let Some(track) = prepared.track.as_mut() {
                    track.key = Some(shown);
                }
                prepared.static_info.track = prepared.track.clone();
            }
            loaded.conflict.map(|(loser, winner)| {
                format!(
                    "{}: metadata disagreed (revision {} vs {}), used {}",
                    path.display(),
                    loser.revision,
                    winner.revision,
                    winner.revision
                )
            })
        }
        Err(error) => Some(format!("{}: {error}", path.display())),
    }
}

/// Store the measured grid, unless the file already carries one.
///
/// Returns whether a write happened. A stored grid is a decision — the DJ's manual shift, or an
/// analysis locked against re-measurement — so the detector never replaces one (DJP-MEDIA-006).
/// `force` is the DJ asking: the grid is replaced and the file is locked against the next automatic
/// measurement, because a hand edit outranks a detector.
pub fn write_beatgrid(
    store: &Store,
    volume: &str,
    root: &Path,
    path: &Path,
    grid: &BeatGrid,
    force: bool,
) -> Result<bool, String> {
    if !grid.bpm().is_finite() || grid.bpm() <= 0.0 {
        return Ok(false);
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default();
    let (mut meta, existed) = match store.load(volume, root, path) {
        Ok(loaded) => (loaded.meta, true),
        Err(_) => {
            let print = fingerprint::of_path(path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            (TrackMeta::new(print, now_ms), false)
        }
    };
    // A stored grid is a decision, unless the DJ asked for the measurement again: then the answer
    // replaces it, lock or no lock.
    if !force && (meta.analysis_lock || meta.beatgrid.is_some()) {
        return Ok(false);
    }
    // A stored grid is already the decision: the condition above keeps the automatic pass away from a
    // track that has one, so no flag is needed for that. The lock is the DJ's switch and stays his —
    // setting it here took the Measure button away the moment he used it once.
    // One number stands for the track: the stretch that plays longest, not the one the file happens to start with. A
    // list built from this row shows the tempo a DJ mixes with (owner's rule, 26.11).
    let dominant = grid.dominant_bpm(meta.fingerprint.duration_frames);
    meta.beatgrid = Some(dj_library::BeatGrid {
        bpm: dominant,
        first_beat_frame: grid.first_beat_frame(),
        beats_per_bar: grid.beats_per_bar(),
        anchors: grid
            .stored_anchors()
            .into_iter()
            .map(|(frame, beat, bpm)| dj_library::Anchor {
                frame,
                beat,
                bpm: Some(bpm),
            })
            .collect(),
    });
    if existed {
        meta.bump(now_ms);
    }
    store
        .save(volume, root, path, &meta)
        .map(|_| {
            refresh_row(path, &meta);
            // A grid the DJ asked for — an edit, or the Measure button — is what the list shows as a number and a
            // colour, and the window asks again (owner's report, 26.11). A measurement that came with an import is not
            // counted: it writes forty grids in a row, and a list replaced forty times is the list moving under the DJ's
            // hands (the selection check holds exactly that).
            if force {
                GRIDS_WRITTEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            true
        })
        .map_err(|error| error.to_string())
}

/// Raise or lower the analysis lock of one track, in both copies of its metadata.
///
/// A locked grid is a decision: the measurement must not replace it and the editor must not move it. The flag
/// travels with the file, so it is written where the rest of the decision lives — the record beside the track
/// and the mirror of it.
/// Which file a lock belongs to: the deck's own file first, then the path the client sent.
///
/// A client that reconnected to a running engine never loaded anything and carries an empty path; the
/// engine is the one that knows which file the deck plays. Writing beside `""` made the engine refuse
/// the lock with "No such file or directory" and the button looked dead (owner's report, 2026-11-19).
/// Whether the DJ has locked this track's numbers. The lock is a decision about the file, so it has to be read
/// where the file is measured — a batch that trusted the list would rewrite exactly what the lock keeps.
pub fn analysis_locked(store: &Store, volume: &str, root: &Path, path: &Path) -> bool {
    store
        .load(volume, root, path)
        .map(|loaded| loaded.meta.analysis_lock)
        .unwrap_or(false)
}

pub fn analysis_lock_target(deck_path: Option<&str>, command_path: &str) -> Option<PathBuf> {
    if let Some(path) = deck_path
        && !path.trim().is_empty()
    {
        return Some(PathBuf::from(path));
    }
    let named = command_path.trim();
    if named.is_empty() {
        None
    } else {
        Some(PathBuf::from(named))
    }
}

pub fn set_analysis_lock(
    store: &Store,
    volume: &str,
    root: &Path,
    path: &Path,
    enabled: bool,
) -> Result<bool, String> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default();
    let (mut meta, existed) = match store.load(volume, root, path) {
        Ok(loaded) => (loaded.meta, true),
        Err(_) => {
            let print = fingerprint::of_path(path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            (TrackMeta::new(print, now_ms), false)
        }
    };
    if existed && meta.analysis_lock == enabled {
        return Ok(false);
    }
    meta.analysis_lock = enabled;
    if existed {
        meta.bump(now_ms);
    }
    store
        .save(volume, root, path, &meta)
        .map(|_| {
            refresh_row(path, &meta);
            true
        })
        .map_err(|error| error.to_string())
}

/// Measure a file's tempo and key and keep them beside it (DJP-MEDIA-004).
///
/// Both measurements are full decodes, so this never runs on a client thread: the import worker calls
/// it for a file that arrived without metadata, and the analysis worker calls it when the DJ asks.
/// Without `force` only the missing half is measured — a stored grid or a key is a decision, and
/// measuring it again would cost a decode and could answer differently.
pub fn analyze_file(store: &Store, volume: &str, root: &Path, path: &Path, force: bool) -> bool {
    let known = store
        .load(volume, root, path)
        .ok()
        .map(|loaded| loaded.meta);
    let needs_grid = force || known.as_ref().is_none_or(|meta| meta.beatgrid.is_none());
    let needs_key = force || known.as_ref().is_none_or(|meta| meta.key.is_none());
    let mut measured = false;
    if needs_grid && let Some(found) = crate::beatmodel::analyse(path) {
        let segments: Vec<BeatSegment> = found
            .segments
            .iter()
            .map(|segment| BeatSegment {
                frame: segment.first_beat_frame,
                beat: segment.beat,
                bpm: segment.bpm,
            })
            .collect();
        if let Some(grid) =
            BeatGrid::from_segments(segments).map(|grid| grid.with_beats_per_bar(found.metre))
            && write_beatgrid(store, volume, root, path, &grid, force).is_ok()
        {
            measured = true;
        }
    }
    if needs_key {
        match dj_analysis::key::analyse(path) {
            Ok(Some(key)) => {
                if write_key(store, volume, root, path, &key.traktor(), force).is_ok() {
                    measured = true;
                }
            }
            Ok(None) => crate::log::info(format!("{}: no key found", path.display())),
            Err(error) => crate::log::error(format!("{}: key: {error}", path.display())),
        }
    }
    // The batch is over a track in a second and the list is what the DJ looks at: one line per file says what was
    // measured and, when nothing was, which step said so.
    let kept = store
        .load(volume, root, path)
        .ok()
        .map(|loaded| loaded.meta);
    crate::log::info(format!(
        "analysis of {}: measured {}, grid {}, key {}",
        path.display(),
        measured,
        kept.as_ref()
            .and_then(|meta| meta.beatgrid.as_ref())
            .map(|grid| format!("{:.2}", grid.bpm))
            .unwrap_or_else(|| "none".to_string()),
        kept.as_ref()
            .and_then(|meta| meta.key.clone())
            .unwrap_or_else(|| "none".to_string())
    ));
    measured
}

/// Fill the names a collection list reads, once per file.
///
/// An import from before the names travelled in the record has none, and a list that shows an empty
/// album forever is a list nobody uses. The tags are read once and kept beside the file.
pub fn write_names(
    store: &Store,
    volume: &str,
    root: &Path,
    path: &Path,
    force: bool,
    index: Option<&mut dj_library::Index>,
) -> bool {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default();
    let (mut meta, existed) = match store.load(volume, root, path) {
        Ok(loaded) => (loaded.meta, true),
        Err(_) => {
            let Ok(print) = fingerprint::of_path(path) else {
                return false;
            };
            (TrackMeta::new(print, now_ms), false)
        }
    };
    if meta.tags.is_some() && !force {
        return false;
    }
    let Ok(tags) = dj_library::tags::read(path) else {
        return false;
    };
    let genres = dj_library::genres::split(&tags.genre);
    if tags.artist.trim().is_empty()
        && tags.title.trim().is_empty()
        && tags.album.trim().is_empty()
        && genres.is_empty()
    {
        return false;
    }
    meta.tags = Some(dj_library::meta::FileTags {
        artist: tags.artist,
        album: tags.album,
        title: tags.title,
        genres: genres.iter().map(|genre| genre.name.clone()).collect(),
    });
    if existed {
        meta.bump(now_ms);
    }
    if store.save(volume, root, path, &meta).is_err() {
        return false;
    }
    if let Some(index) = index {
        let row = row_of(&meta, path);
        // The database is a cache over the record: a failed write is reported, and the collection
        // still has its names. The rebuild fills the row later.
        if let Err(error) = index.upsert_with_genres(&row, &genres) {
            eprintln!("dj-engine: index write failed for {}: {error}", row.path);
        }
    }
    true
}

/// The index row for a record: what the browser sorts by, and what a search looks in.
///
/// The row is built from the record, not from the file: a name the DJ corrected is the name the list
/// shows. `genre` is the raw tag joined back together — the links, not this column, decide what the
/// track belongs to.
/// Put the record that was just written into the collection, so the list shows it without a rescan.
///
/// The list is built from the index, and a record on disk that the index has not seen is a track whose
/// tempo, key and rating are all missing on screen. A write that changes what the row says calls this: a
/// grid, a key, a lock, the names.
/// How many stored grids have been written since this process started.
///
/// The window holds a list it asked for once. An edit of the grid or a measurement changes the tempo and the flag behind
/// that list, and without a word from the engine the list keeps saying what used to be true — the row of a track whose
/// grid was just edited went on showing the old tempo, because nothing told the window to ask again (owner's report,
/// 26.11). The counter is that word: it moves when a grid lands, and a client asks once per move.
///
/// A lock is deliberately not counted: it changes one flag of one row, the window patches that row where it stands, and a
/// page replaced under the DJ's press drags the list out from under him (the lock check holds exactly that).
pub static GRIDS_WRITTEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn refresh_row(path: &Path, meta: &TrackMeta) {
    if let Ok(mut index) = dj_library::Index::open_default() {
        let _ = index.upsert(&row_of(meta, path));
    }
}

pub fn row_of(meta: &TrackMeta, path: &Path) -> dj_library::TrackRow {
    let tags = meta.tags.clone().unwrap_or_default();
    dj_library::TrackRow {
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
        // The same reading of the record the library makes: a row refreshed after an edit must say what a row rebuilt
        // from the file says, and a hard `false` here made every edited grid look like one tempo (owner's report,
        // 26.11: «поменял битгрид… BPM показывается тем же цветом»).
        variable_bpm: meta
            .beatgrid
            .as_ref()
            .is_some_and(dj_library::meta::BeatGrid::is_variable),
    }
}

/// Store the measured key, unless the file already carries one.
///
/// A key already in the file is a decision — the DJ's ear, or another program's — and a detector does
/// not overrule it. `force` is the DJ asking for the measurement again.
pub fn write_key(
    store: &Store,
    volume: &str,
    root: &Path,
    path: &Path,
    key: &str,
    force: bool,
) -> Result<bool, String> {
    if key.trim().is_empty() {
        return Ok(false);
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default();
    let (mut meta, existed) = match store.load(volume, root, path) {
        Ok(loaded) => (loaded.meta, true),
        Err(_) => {
            let print = fingerprint::of_path(path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            (TrackMeta::new(print, now_ms), false)
        }
    };
    if !force && meta.key.is_some() {
        return Ok(false);
    }
    meta.key = Some(key.to_owned());
    if existed {
        meta.bump(now_ms);
    }
    store
        .save(volume, root, path, &meta)
        .map(|_| {
            refresh_row(path, &meta);
            true
        })
        .map_err(|error| error.to_string())
}

/// Store the loudness correction, unless the file already carries one.
///
/// The correction is a property of the master: measuring it again would cost a full decode to reach the same
/// number. `force` is the DJ asking for the measurement again — a remaster, or a record written by a build that
/// kept no correction at all.
pub fn write_auto_gain(
    store: &Store,
    volume: &str,
    root: &Path,
    path: &Path,
    correction_db: f32,
    force: bool,
) -> Result<bool, String> {
    if !correction_db.is_finite() {
        return Ok(false);
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default();
    let (mut meta, existed) = match store.load(volume, root, path) {
        Ok(loaded) => (loaded.meta, true),
        Err(_) => {
            let print = fingerprint::of_path(path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            (TrackMeta::new(print, now_ms), false)
        }
    };
    if !force && meta.auto_gain_db.is_some() {
        return Ok(false);
    }
    meta.auto_gain_db = Some(f64::from(correction_db));
    if existed {
        meta.bump(now_ms);
    }
    store
        .save(volume, root, path, &meta)
        .map(|_| {
            refresh_row(path, &meta);
            true
        })
        .map_err(|error| error.to_string())
}

/// Write the deck's cues and loop into the track's metadata file.
pub fn write_edit(store: &Store, volume: &str, root: &Path, edit: &MetaEdit) -> Result<(), String> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default();
    // Start from what is on disk: fields this build does not know must survive the write.
    let (mut meta, existed) = match store.load(volume, root, &edit.path) {
        Ok(loaded) => (loaded.meta, true),
        Err(_) => {
            let print = fingerprint::of_path(&edit.path)
                .map_err(|error| format!("{}: {error}", edit.path.display()))?;
            // A fresh record is already revision 1: the first write is not an edit of nothing.
            (TrackMeta::new(print, now_ms), false)
        }
    };
    meta.cues = edit
        .cues
        .iter()
        .map(|(slot, frame)| {
            let existing = meta.cues.iter().find(|cue| cue.slot == *slot);
            let asked = edit.cue_meta.iter().find(|(wanted, _, _)| wanted == slot);
            Cue {
                slot: *slot,
                frame: *frame,
                // What the deck asked for wins; otherwise the file's own record survives the rewrite.
                label: asked
                    .and_then(|(_, label, _)| label.clone())
                    .or_else(|| existing.and_then(|cue| cue.label.clone())),
                color: asked
                    .and_then(|(_, _, color)| color.clone())
                    .or_else(|| existing.and_then(|cue| cue.color.clone())),
            }
        })
        .collect();
    // Pads carry the loops now: the slot is the pad, so a stored four bars come back on the pad the DJ
    // put them on (DJP-DECK-004). The active loop is a runtime state and is not written here.
    meta.loops = edit
        .pad_loops
        .iter()
        .map(|(slot, start, end)| dj_library::Loop {
            slot: *slot,
            start: *start,
            end: *end,
        })
        .collect();
    if existed {
        meta.bump(now_ms);
    }
    store
        .save(volume, root, &edit.path, &meta)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::row_of;

    /// A record with two tempos, the way an edit leaves one: a tempo, and an anchor that states its own.
    fn print() -> dj_library::meta::Fingerprint {
        dj_library::meta::Fingerprint {
            size: 4096,
            head: "aaaa".to_owned(),
            tail: "bbbb".to_owned(),
            duration_frames: Some(9_600_000),
            sample_rate: Some(48_000),
        }
    }

    fn two_tempos() -> dj_library::TrackMeta {
        let mut meta = dj_library::TrackMeta::new(print(), 0);
        meta.beatgrid = Some(dj_library::BeatGrid {
            bpm: 128.0,
            first_beat_frame: 0,
            anchors: vec![dj_library::Anchor {
                frame: 4_800_000,
                beat: 256,
                bpm: Some(256.0),
            }],
            beats_per_bar: 4,
        });
        meta
    }

    #[test]
    fn a_row_refreshed_after_an_edit_says_the_track_is_more_than_one_tempo() {
        let row = row_of(&two_tempos(), std::path::Path::new("/music/a.flac"));
        assert!(
            row.variable_bpm,
            "the same record gives the library's row a raised flag; the engine's refresh must not answer otherwise"
        );
        assert_eq!(row.bpm, Some(128.0), "and the number is the stored one");
        let single = dj_library::TrackMeta::new(print(), 0);
        assert!(
            !row_of(&single, std::path::Path::new("/music/b.flac")).variable_bpm,
            "a track with no grid is not a variable one"
        );
    }
}
