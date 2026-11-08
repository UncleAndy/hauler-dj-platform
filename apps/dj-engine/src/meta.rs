//! Reading and writing a track's own metadata from the engine side (DJP-DATA-006).
//!
//! Two moments matter. When a file is loaded, the cues and the loop it carried go back on the deck,
//! so a reload looks like a reload. When the user moves a cue or sets a loop, the change is sent to
//! this module from the graph and written here — never in the callback, which must not touch a file.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dj_library::{Cue, Store, TrackMeta, fingerprint};

use crate::{MetaEdit, PreparedTrack, StoredGrid};

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
            if let Some(grid) = loaded.meta.beatgrid.as_ref() {
                prepared.grid = Some(StoredGrid {
                    bpm: grid.bpm,
                    first_beat_frame: grid.first_beat_frame,
                });
                if let Some(track) = prepared.track.as_mut() {
                    track.bpm = Some(grid.bpm);
                    track.first_beat_frame = Some(grid.first_beat_frame);
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
pub fn write_beatgrid(
    store: &Store,
    volume: &str,
    root: &Path,
    path: &Path,
    grid: StoredGrid,
) -> Result<bool, String> {
    if !grid.bpm.is_finite() || grid.bpm <= 0.0 {
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
    if meta.analysis_lock || meta.beatgrid.is_some() {
        return Ok(false);
    }
    meta.beatgrid = Some(dj_library::BeatGrid {
        bpm: grid.bpm,
        first_beat_frame: grid.first_beat_frame,
        anchors: Vec::new(),
    });
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
