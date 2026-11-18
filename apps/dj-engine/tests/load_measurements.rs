//! What a load does not have to decode again: a grid and a loudness correction the record already holds.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use dj_library::{Store, TrackMeta};

/// The counters of measurements are per process, so two tests measuring at once would each see the other's
/// decodes. One test at a time keeps every number where its test put it.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn alone() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// A copy of the repository's own drum loop: a real recording with a beat in it, so a measurement has
/// something to find and a record has something to store.
fn track(folder: &Path) -> std::io::Result<PathBuf> {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/dj-codec/tests/fixtures/audio/drums-8s-48000.wav");
    let path = folder.join("Measured.wav");
    std::fs::copy(fixture, &path)?;
    Ok(path)
}

fn prepare(
    path: &Path,
    known: &dj_engine::KnownMeta,
) -> std::result::Result<dj_engine::PreparedTrack, String> {
    dj_engine::prepare_track_with_key(path, 48_000, 2.0, 0.7, None, known)
}

#[test]
fn a_stored_grid_saves_the_tempo_decode() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let _alone = alone();
    let temp = std::env::temp_dir().join(format!("dj-grid-saves-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let music = temp.join("music");
    let mirror = temp.join("mirror");
    std::fs::create_dir_all(&music)?;
    std::fs::create_dir_all(&mirror)?;
    let path = track(&music)?;
    let store = Store::new(&mirror);
    let (volume, root) = ("vol", music.clone());

    // The first load measures the beat and leaves the grid in the record, the way the loader does.
    let first = prepare(&path, &dj_engine::KnownMeta::default())?;
    let Some(grid) = first.grid else {
        return Err("the first load must measure a grid".into());
    };
    assert!(
        dj_engine::meta::write_beatgrid(&store, volume, &root, &path, &grid, false)?,
        "the grid must land in the record"
    );

    // The second load has the record's word for it: no decode, and the grid still arrives.
    let before = dj_engine::beatmodel::measurements();
    let second = prepare(
        &path,
        &dj_engine::KnownMeta {
            grid: true,
            ..dj_engine::KnownMeta::default()
        },
    )?;
    let after = dj_engine::beatmodel::measurements();
    assert_eq!(
        before, after,
        "the loader decoded the file for a tempo the record already holds"
    );

    let mut restored = second;
    dj_engine::meta::restore(&mut restored, &store, volume, &root);
    assert!(
        restored.grid.is_some(),
        "the deck lost the grid the record holds"
    );

    let _ = std::fs::remove_dir_all(&temp);
    Ok(())
}

#[test]
fn a_stored_gain_saves_the_loudness_decode() -> std::result::Result<(), Box<dyn std::error::Error>>
{
    let _alone = alone();
    let temp = std::env::temp_dir().join(format!("dj-gain-saves-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let music = temp.join("music");
    let mirror = temp.join("mirror");
    std::fs::create_dir_all(&music)?;
    std::fs::create_dir_all(&mirror)?;
    let path = track(&music)?;
    let store = Store::new(&mirror);
    let (volume, root) = ("vol", music.clone());

    // A record that already carries a correction — the value does not matter here, only that the file is
    // not decoded again to find it.
    let mut record = TrackMeta::new(dj_library::fingerprint::of_path(&path)?, 1);
    record.auto_gain_db = Some(-3.0);
    store.save(volume, &root, &path, &record)?;

    let before = dj_analysis::loudness::measurements();
    let prepared = prepare(
        &path,
        &dj_engine::KnownMeta {
            auto_gain_db: Some(-3.0),
            ..dj_engine::KnownMeta::default()
        },
    )?;
    let after = dj_analysis::loudness::measurements();
    assert_eq!(
        before, after,
        "the loader decoded the file for a loudness the record already holds"
    );
    let expected = dj_analysis::loudness::gain_for(-3.0, 0.7);
    assert!(
        (prepared.static_info.auto_gain - expected).abs() < 1e-5,
        "the stored correction was not applied: {} instead of {expected}",
        prepared.static_info.auto_gain
    );

    let _ = std::fs::remove_dir_all(&temp);
    Ok(())
}

#[test]
fn a_fresh_track_is_measured_and_the_answer_is_kept()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let _alone = alone();
    let temp = std::env::temp_dir().join(format!("dj-gain-kept-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let music = temp.join("music");
    let mirror = temp.join("mirror");
    std::fs::create_dir_all(&music)?;
    std::fs::create_dir_all(&mirror)?;
    let path = track(&music)?;
    let store = Store::new(&mirror);
    let (volume, root) = ("vol", music.clone());

    let before = dj_analysis::loudness::measurements();
    let prepared = prepare(&path, &dj_engine::KnownMeta::default())?;
    let after = dj_analysis::loudness::measurements();
    assert_eq!(after, before + 1, "a fresh track must be measured once");
    let Some(correction) = prepared.auto_gain_correction_db else {
        return Err("the measurement must report the correction it applied".into());
    };
    assert!(
        dj_engine::meta::write_auto_gain(&store, volume, &root, &path, correction, false)?,
        "the correction must land in the record beside the track"
    );
    let stored = store.load(volume, &root, &path)?;
    assert!(
        stored.meta.auto_gain_db.is_some(),
        "the record beside the track says nothing about the loudness"
    );

    let _ = std::fs::remove_dir_all(&temp);
    Ok(())
}
