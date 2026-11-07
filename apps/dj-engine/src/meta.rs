//! Reading and writing a track's own metadata from the engine side (DJP-DATA-006).
//!
//! Two moments matter. When a file is loaded, the cues and the loop it carried go back on the deck,
//! so a reload looks like a reload. When the user moves a cue or sets a loop, the change is sent to
//! this module from the graph and written here — never in the callback, which must not touch a file.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use dj_library::{Cue, Store, TrackMeta, fingerprint};

use crate::{MetaEdit, PreparedTrack};

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
            prepared.loop_region = loaded
                .meta
                .loops
                .first()
                .map(|stored| dj_domain::FrameRange {
                    start: dj_domain::SourceFrame(stored.start),
                    end: dj_domain::SourceFrame(stored.end),
                });
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
    meta.loops = edit
        .loop_region
        .map(|(start, end)| {
            vec![dj_library::Loop {
                slot: 0,
                start,
                end,
            }]
        })
        .unwrap_or_default();
    if existed {
        meta.bump(now_ms);
    }
    store
        .save(volume, root, &edit.path, &meta)
        .map(|_| ())
        .map_err(|error| error.to_string())
}
