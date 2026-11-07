//! A file's fingerprint: how a track is recognised after it moves or is renamed.
//!
//! DJP-DATA-004 asks for renames to be noticed without trusting the path, and DJP-DATA-006 puts the
//! fingerprint into the metadata file itself. The hash is not a security boundary — it identifies a
//! file among a collection — so it reads the head and the tail rather than every byte: a full pass
//! over a library of 100 000 tracks is minutes, and two windows are enough to tell two files apart.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::meta::Fingerprint;

/// How many bytes at each end are hashed.
pub const WINDOW: u64 = 64 * 1024;

/// Fingerprint a file on disk.
pub fn of_path(path: &Path) -> io::Result<Fingerprint> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let head_len = WINDOW.min(size);
    let mut head = vec![0u8; usize::try_from(head_len).unwrap_or(0)];
    file.read_exact(&mut head)?;

    let tail_offset = size.saturating_sub(WINDOW);
    file.seek(SeekFrom::Start(tail_offset))?;
    let tail_len = WINDOW.min(size);
    let mut tail = vec![0u8; usize::try_from(tail_len).unwrap_or(0)];
    file.read_exact(&mut tail)?;

    Ok(Fingerprint {
        size,
        head: hash(&head),
        tail: hash(&tail),
        duration_frames: None,
        sample_rate: None,
    })
}

/// FNV-1a, 64 bit: small, dependency free, and stable across versions — the property that matters
/// for a value written into a file and read back by a later build.
fn hash(bytes: &[u8]) -> String {
    let mut value: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        value ^= u64::from(*byte);
        value = value.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{value:016x}")
}

#[cfg(test)]
// A failed expectation in a test is a failure message; the workspace forbids them in library code.
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn the_same_bytes_give_the_same_fingerprint() {
        let dir = std::env::temp_dir().join(format!("dj-fp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let one = dir.join("one.bin");
        let two = dir.join("two.bin");
        std::fs::write(&one, b"same bytes").expect("write");
        std::fs::write(&two, b"same bytes").expect("write");
        let a = of_path(&one).expect("fingerprint");
        let b = of_path(&two).expect("fingerprint");
        assert_eq!(a, b, "the path must not matter, the bytes must");

        let other = dir.join("other.bin");
        let mut file = std::fs::File::create(&other).expect("create");
        file.write_all(b"different").expect("write");
        assert_ne!(of_path(&other).expect("fingerprint"), a);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_larger_than_the_window_reads_both_ends() {
        let dir = std::env::temp_dir().join(format!("dj-fp-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("big.bin");
        let mut bytes = vec![7u8; usize::try_from(WINDOW * 3).expect("fits")];
        std::fs::write(&path, &bytes).expect("write");
        let first = of_path(&path).expect("fingerprint");
        // Change one byte in the middle: outside both windows, so the fingerprint must not move.
        let middle = usize::try_from(WINDOW).expect("fits");
        bytes[middle] = 9;
        std::fs::write(&path, &bytes).expect("write");
        assert_eq!(of_path(&path).expect("fingerprint"), first);

        // Change the last byte: inside the tail window, so it must.
        if let Some(last) = bytes.last_mut() {
            *last = 9;
        }
        std::fs::write(&path, &bytes).expect("write");
        assert_ne!(of_path(&path).expect("fingerprint"), first);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
