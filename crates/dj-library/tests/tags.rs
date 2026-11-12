//! What the collection reads from a file: the names, the album, the genre.
//!
//! The reader that understands the audio stream (`symphonia`) has no tag support for RIFF at all —
//! `symphonia-format-riff` 0.6 does not look at `INFO` or `id3`. A WAV with tags is a file the
//! collection must still list by its names, so the writer's own library is asked as a second opinion.
#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};

use dj_library::tags::{self, Tags};

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("dj-tags-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch");
    root
}

/// A short tone with a moving pitch: a flat tone gives a degenerate fingerprint, and the import needs
/// something to chew on.
fn write_tone(path: &Path) {
    const PITCHES: [f64; 12] = [
        1.0, 1.122, 1.26, 1.335, 1.498, 1.682, 1.782, 2.0, 2.245, 2.52, 2.67, 2.997,
    ];
    let rate = 22_050_u32;
    let samples = rate;
    let mut data = Vec::with_capacity(samples as usize * 2);
    let mut phase = 0.0_f64;
    let mut noise = 0x2545_F491_4F6C_DD1D_u64;
    let mut step = 440.0_f64;
    for index in 0..samples {
        if index.is_multiple_of(rate / 4) {
            noise = noise
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            step = 440.0 * PITCHES[(noise >> 33) as usize % PITCHES.len()];
        }
        phase += std::f64::consts::TAU * step / f64::from(rate);
        data.extend_from_slice(&((phase.sin() * 12_000.0) as i16).to_le_bytes());
    }
    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&(36_u32 + data.len() as u32).to_le_bytes());
    file.extend_from_slice(b"WAVEfmt ");
    file.extend_from_slice(&16_u32.to_le_bytes());
    file.extend_from_slice(&1_u16.to_le_bytes());
    file.extend_from_slice(&1_u16.to_le_bytes());
    file.extend_from_slice(&rate.to_le_bytes());
    file.extend_from_slice(&(rate * 2).to_le_bytes());
    file.extend_from_slice(&2_u16.to_le_bytes());
    file.extend_from_slice(&16_u16.to_le_bytes());
    file.extend_from_slice(b"data");
    file.extend_from_slice(&(data.len() as u32).to_le_bytes());
    file.extend_from_slice(&data);
    std::fs::write(path, file).expect("wav");
}

fn wanted() -> Tags {
    Tags {
        artist: "Artist".to_owned(),
        title: "Track".to_owned(),
        album: "Album".to_owned(),
        genre: "Deep House; Soulful".to_owned(),
        year: "2021".to_owned(),
        track: "3".to_owned(),
        count: 0,
    }
}

#[test]
fn a_wav_gives_back_the_tags_it_was_given() {
    let dir = scratch("wav");
    let path = dir.join("tone.wav");
    write_tone(&path);
    assert!(
        tags::write(&path, &wanted()).expect("write the tags"),
        "the writer took the file"
    );

    let back = tags::read(&path).expect("read the tags");
    assert_eq!(back.artist, "Artist");
    assert_eq!(back.title, "Track");
    assert_eq!(back.album, "Album");
    assert_eq!(back.genre, "Deep House; Soulful");
    assert_eq!(back.year, "2021");
    assert_eq!(back.track, "3");
    assert!(
        back.count >= 6,
        "every name counts as a tag: {}",
        back.count
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_wav_without_tags_reads_as_an_empty_set() {
    // The other side: the second reader must not invent names for a file that has none.
    let dir = scratch("bare");
    let path = dir.join("tone.wav");
    write_tone(&path);
    let back = tags::read(&path).expect("read the tags");
    assert_eq!(back.count, 0);
    assert!(back.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_flac_still_reads_through_the_audio_reader() {
    // The fallback must not take over the formats that already work: a FLAC is read by the first
    // reader, and its names come back unchanged.
    let dir = scratch("flac");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/dj-codec/tests/fixtures/audio/tone-1s-44100.flac");
    let path = dir.join("Artist - Track.flac");
    std::fs::copy(&source, &path).expect("copy the fixture");
    assert!(tags::write(&path, &wanted()).expect("write the tags"));

    let back = tags::read(&path).expect("read the tags");
    assert_eq!(back.artist, "Artist");
    assert_eq!(back.genre, "Deep House; Soulful");
    std::fs::remove_dir_all(&dir).ok();
}
