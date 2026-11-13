//! A locked grid is a decision the DJ made: it must survive in every copy of the metadata and in the
//! collection, and it must be readable from the list without opening the track (DJP-SYNC-003).

use std::io::Write;
use std::path::{Path, PathBuf};

use dj_library::{Index, Store};

/// Half a second of a 440 Hz tone, written by hand: a real audio file and no test-only dependency.
fn tone(path: &Path) -> std::io::Result<()> {
    let rate = 44_100u32;
    let frames = rate / 2;
    let mut samples = Vec::with_capacity(frames as usize * 2);
    for i in 0..frames {
        let value =
            ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 12_000.0) as i16;
        samples.extend_from_slice(&value.to_le_bytes());
    }
    let mut file = std::fs::File::create(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + samples.len() as u32).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&rate.to_le_bytes())?;
    file.write_all(&(rate * 2).to_le_bytes())?;
    file.write_all(&2u16.to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&(samples.len() as u32).to_le_bytes())?;
    file.write_all(&samples)?;
    file.flush()
}

/// Everything the store keeps beside the music, as one string.
fn records(folder: &Path) -> String {
    std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .filter_map(std::result::Result::ok)
                .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
                .collect::<Vec<String>>()
                .join("\n")
        })
        .unwrap_or_default()
}

#[test]
fn the_lock_reaches_both_copies_of_the_metadata_and_the_collection()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let temp = std::env::temp_dir().join(format!("dj-lock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let music = temp.join("music");
    let mirror = temp.join("mirror");
    std::fs::create_dir_all(&music)?;
    std::fs::create_dir_all(&mirror)?;

    let track = music.join("Locked.wav");
    tone(&track)?;

    let store = Store::new(&mirror);
    let volume = "vol";
    let root = PathBuf::from(&music);

    // A fresh file: the lock is written into the record beside it and into the mirror.
    assert!(
        dj_engine::meta::set_analysis_lock(&store, volume, &root, &track, true)?,
        "the first lock must be a write, not a no-op"
    );
    let folder = music.join(".djmeta");
    assert!(folder.is_dir(), "the record must sit beside the track");
    let text = records(&folder);
    assert!(
        text.contains("\"analysis_lock\":true") || text.contains("\"analysis_lock\": true"),
        "the record beside the track says nothing about the lock: {text}"
    );
    let loaded = store.load(volume, &root, &track)?;
    assert!(loaded.meta.analysis_lock, "the mirror lost the lock");

    // The same flag in the collection: the list draws it beside the number.
    let database = temp.join("db");
    std::fs::create_dir_all(&database)?;
    let mut index = Index::open_in(&database)?;
    let row = dj_engine::meta::row_of(&loaded.meta, &track);
    assert!(
        row.analysis_lock,
        "the row built for the index lost the lock"
    );
    index.upsert(&row)?;
    let page = index.page(&dj_library::index::PageQuery {
        order: Vec::new(),
        genres: Vec::new(),
        search: None,
        offset: 0,
        limit: 10,
        roots: vec![music.clone()],
    })?;
    assert_eq!(page.rows.len(), 1, "the collection lost the track");
    assert!(page.rows[0].analysis_lock, "the collection lost the lock");

    // And it comes off again, in both places.
    assert!(
        dj_engine::meta::set_analysis_lock(&store, volume, &root, &track, false)?,
        "lifting the lock must be a write too"
    );
    let lifted = store.load(volume, &root, &track)?;
    assert!(!lifted.meta.analysis_lock, "the lock did not come off");
    let text = records(&folder);
    assert!(
        !text.contains("\"analysis_lock\":true") && !text.contains("\"analysis_lock\": true"),
        "an unlocked track must not claim a lock: {text}"
    );

    let _ = std::fs::remove_dir_all(&temp);
    Ok(())
}
