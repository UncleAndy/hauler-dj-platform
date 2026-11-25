//! The envelope the window already caches, read for the strip at the bottom of a deck's screen.
//!
//! The engine publishes a path, not the picture: a five-minute track is a few hundred kilobytes of extremes, and the
//! snapshot is published twenty times a second (DJP-MEDIA-003). The file is written by `dj-analysis::cache`; this
//! reader takes the two extremes of each column and drops the three band levels, which the screen has no room for. A
//! format change is therefore a two-place edit, and this side fails soft: no envelope, no strip.

use std::io::Read;

/// The magic at the head of an envelope file, as `dj-analysis::cache` writes it.
const MAGIC: &[u8; 8] = b"DJWAVE11";
/// Bytes per column on disk: two extremes as `i16` and three band levels the screen does not use.
const BUCKET_BYTES: usize = 7;

/// One track's envelope: the extremes of each column of the whole track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Envelope {
    /// Frames each column covers.
    pub frames_per_bucket: u32,
    /// The source sample rate, so a caller can turn columns into seconds.
    pub sample_rate: u32,
    /// Low then high per column, −1 to 1, left to right.
    pub columns: Vec<(f32, f32)>,
}

/// Read the envelope at `path`; `None` when the file is missing, of another format, or shorter than its own header
/// says. A track without an envelope is a screen without a strip, which is honest — it is not a track with no sound.
pub fn read(path: &str) -> Option<Envelope> {
    let raw = std::fs::read(path).ok()?;
    let mut cursor = std::io::Cursor::new(raw.as_slice());
    let mut magic = [0u8; 8];
    cursor.read_exact(&mut magic).ok()?;
    if &magic != MAGIC {
        return None;
    }
    // The fingerprint of the file the envelope describes: its size and modification time. The screen trusts the
    // engine's path here — the entry name already carries the resolution and the digest, and a stale path is a stale
    // path.
    let mut number = [0u8; 8];
    cursor.read_exact(&mut number).ok()?;
    let mut wide = [0u8; 16];
    cursor.read_exact(&mut wide).ok()?;
    cursor.read_exact(&mut number).ok()?;
    let sample_rate = u32::try_from(u64::from_le_bytes(number)).ok()?;
    cursor.read_exact(&mut number).ok()?;
    let frames_per_bucket = u32::try_from(u64::from_le_bytes(number)).ok()?;
    cursor.read_exact(&mut number).ok()?;
    let buckets = usize::try_from(u64::from_le_bytes(number)).ok()?;
    let mut columns = Vec::with_capacity(buckets);
    let mut column = [0u8; BUCKET_BYTES];
    for _ in 0..buckets {
        cursor.read_exact(&mut column).ok()?;
        let low = f32::from(i16::from_le_bytes([column[0], column[1]])) / 32_767.0;
        let high = f32::from(i16::from_le_bytes([column[2], column[3]])) / 32_767.0;
        columns.push((low, high));
    }
    Some(Envelope {
        frames_per_bucket,
        sample_rate,
        columns,
    })
}

/// `count` columns of the whole track, however many the envelope has.
///
/// Each column of the strip takes the extremes of the run it covers, not one bucket of it: sampling every n-th bucket
/// throws away the transient that makes a drop visible, and a strip without transients is a strip nobody reads.
#[must_use]
pub fn spread(columns: &[(f32, f32)], count: usize) -> Vec<(f32, f32)> {
    if columns.is_empty() || count == 0 {
        return Vec::new();
    }
    (0..count)
        .map(|index| {
            let from = index * columns.len() / count;
            let to = ((index + 1) * columns.len() / count)
                .max(from + 1)
                .min(columns.len());
            columns[from..to]
                .iter()
                .fold((0.0f32, 0.0f32), |(low, high), (a, b)| {
                    (low.min(*a), high.max(*b))
                })
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// A file in the layout `dj-analysis::cache` writes: the magic, the fingerprint, the rate, the column width, the
    /// count, then two extremes and three band levels per column.
    fn envelope_file(buckets: &[(i16, i16)]) -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend_from_slice(MAGIC);
        raw.extend_from_slice(&1234u64.to_le_bytes());
        raw.extend_from_slice(&5678u128.to_le_bytes());
        raw.extend_from_slice(&48_000u64.to_le_bytes());
        raw.extend_from_slice(&512u64.to_le_bytes());
        raw.extend_from_slice(&(buckets.len() as u64).to_le_bytes());
        for (low, high) in buckets {
            raw.extend_from_slice(&low.to_le_bytes());
            raw.extend_from_slice(&high.to_le_bytes());
            raw.extend_from_slice(&[1, 2, 3]);
        }
        raw
    }

    #[test]
    fn an_envelope_is_read_as_the_extremes_of_each_column() {
        let dir = std::env::temp_dir().join("dj-screen-envelope");
        std::fs::create_dir_all(&dir).expect("a folder for the envelope");
        let path = dir.join("track.wave");
        std::fs::write(
            &path,
            envelope_file(&[(-16_384, 16_384), (-32_767, 0), (0, 32_767)]),
        )
        .expect("the envelope file");
        let read = read(path.to_str().expect("a path")).expect("the envelope parses");
        assert_eq!(read.frames_per_bucket, 512);
        assert_eq!(read.sample_rate, 48_000);
        assert_eq!(read.columns.len(), 3);
        assert!(
            (read.columns[0].0 + 0.5).abs() < 0.001,
            "the low extreme is half"
        );
        assert!(
            (read.columns[0].1 - 0.5).abs() < 0.001,
            "the high extreme is half"
        );
        assert!(
            (read.columns[2].1 - 1.0).abs() < 0.001,
            "the loudest column is one"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn another_format_is_no_envelope_at_all() {
        let dir = std::env::temp_dir().join("dj-screen-envelope");
        std::fs::create_dir_all(&dir).expect("a folder for the envelope");
        let path = dir.join("wrong.wave");
        std::fs::write(&path, b"NOTAWAVE and then some").expect("the wrong file");
        assert!(
            read(path.to_str().expect("a path")).is_none(),
            "a wrong magic is refused"
        );
        assert!(
            read("/nowhere/at/all.wave").is_none(),
            "a missing file is refused"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_strip_keeps_the_transients_of_the_columns_it_covers() {
        // Eight columns onto four: each pair keeps the loudest of its two, so a transient in the second bucket is
        // still on the strip.
        let columns = [
            (0.1, 0.2),
            (0.0, 0.9),
            (0.3, 0.4),
            (0.2, 0.5),
            (0.05, 0.1),
            (0.0, 0.2),
            (0.4, 0.6),
            (0.1, 0.3),
        ];
        let strip = spread(&columns, 4);
        assert_eq!(strip.len(), 4);
        assert!(
            (strip[0].1 - 0.9).abs() < 1e-6,
            "the transient survives the cut"
        );
        assert!((strip[2].1 - 0.2).abs() < 1e-6);
        assert!(spread(&[], 4).is_empty(), "no envelope, no strip");
    }
}
