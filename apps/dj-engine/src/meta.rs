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
            // A stored grid outranks the measurement: either the DJ moved it or the analysis was
            // locked, and both are decisions the detector may not overwrite (DJP-SYNC-003).
            if let Some(stored) = loaded.meta.beatgrid.as_ref() {
                // The file states a tempo and pins the rest with anchors; the grid is what the rest of
                // the program speaks. A stored grid that cannot be a grid is ignored rather than trusted.
                let rate = f64::from(prepared.static_info.source_rate);
                let anchors: Vec<(u64, u64)> = stored
                    .anchors
                    .iter()
                    .map(|anchor| (anchor.frame, anchor.beat))
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
                        track.bpm = Some(grid.bpm());
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
    // A grid written because a person asked for it is a decision: the automatic measurement must not
    // quietly replace it on the next load, and the flag is what says so.
    if force {
        meta.analysis_lock = true;
    }
    meta.beatgrid = Some(dj_library::BeatGrid {
        bpm: grid.bpm(),
        first_beat_frame: grid.first_beat_frame(),
        beats_per_bar: grid.beats_per_bar(),
        anchors: grid
            .stored_anchors()
            .into_iter()
            .map(|(frame, beat)| dj_library::Anchor { frame, beat })
            .collect(),
    });
    if existed {
        meta.bump(now_ms);
    }
    store
        .save(volume, root, path, &meta)
        .map(|_| true)
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
    if needs_grid && let Ok(Some(found)) = dj_analysis::tempo::analyse(path) {
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
    if needs_key
        && let Ok(Some(key)) = dj_analysis::key::analyse(path)
        && write_key(store, volume, root, path, &key.traktor(), force).is_ok()
    {
        measured = true;
    }
    measured
}

/// Fill the names a collection list reads, once per file.
///
/// An import from before the names travelled in the record has none, and a list that shows an empty
/// album forever is a list nobody uses. The tags are read once and kept beside the file.
pub fn write_names(store: &Store, volume: &str, root: &Path, path: &Path) -> bool {
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
    if meta.tags.is_some() {
        return false;
    }
    let Ok(tags) = dj_library::tags::read(path) else {
        return false;
    };
    if tags.artist.trim().is_empty() && tags.title.trim().is_empty() && tags.album.trim().is_empty()
    {
        return false;
    }
    meta.tags = Some(dj_library::meta::FileTags {
        artist: tags.artist,
        album: tags.album,
        title: tags.title,
        genres: dj_library::genres::split(&tags.genre)
            .into_iter()
            .map(|genre| genre.name)
            .collect(),
    });
    if existed {
        meta.bump(now_ms);
    }
    store.save(volume, root, path, &meta).is_ok()
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
        .map(|_| true)
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
