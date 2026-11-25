//! The envelope the window already caches, and the window's own drawing rule for it.
//!
//! The engine publishes a path, not the picture: a five-minute track is a few hundred kilobytes of extremes, and the
//! snapshot is published twenty times a second (DJP-MEDIA-003). The file is written by `dj-analysis::cache`; this
//! reader takes the two extremes of each column and drops the three band levels, which the screen has no room for. A
//! format change is therefore a two-place edit, and this side fails soft: no envelope, no strip.
//!
//! `bars` is a copy of `apps/dj-ui/src/Waveform.cpp` — the picture on the monitor is the reference. Inventing a second
//! way to draw the same track leaves two strips that disagree, and the eye notices (owner's request, 25.11).

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
    // engine's path here — the entry name already carries the resolution and the digest.
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

/// The radii the window smooths its outline with — the same fixed set: the chosen radius depends on the zoom and never
/// on where the view sits, or the strip pumps as it scrolls.
const RADII: [usize; 6] = [1, 4, 8, 16, 32, 64];
/// The window's contrast: loud stays loud, quiet sinks.
const CONTRAST: f32 = 1.4;

/// The bars the window would draw for one window of the track.
///
/// `start` and `span` are fractions of the whole track — what the window passes to its own `bars()` — and every step is
/// copied from `apps/dj-ui/src/Waveform.cpp`:
///
/// 1. the peak of each column is `max(|min|, |max|)`;
/// 2. the same gaussian ramp over the same fixed radii, with the radius the window picks for this many columns per bar;
/// 3. a bar's own height is the **average** of the columns it covers (the window's note: averaging is the honest
///    downsample — the loudest of them is the luckiest column, and the outline jumps from bar to bar);
/// 4. that average is blended with the smoothed outline, by how wide the window is;
/// 5. and the contrast is applied last.
#[must_use]
pub fn bars(columns: &[(f32, f32)], count: usize, start: f32, span: f32) -> Vec<f32> {
    if columns.is_empty() || count == 0 || span <= 0.0 {
        return Vec::new();
    }
    let total = columns.len();
    let covered_from = ((start * total as f32).floor() as usize).min(total);
    let covered_to = (((start + span) * total as f32).ceil() as usize).min(total);
    let covered = covered_to.saturating_sub(covered_from);
    if covered == 0 {
        return Vec::new();
    }
    let columns_per_bar = covered as f32 / count as f32;
    let zoom = 1.0 / span;
    let heat = smoothing_heat(zoom);
    // How much of the ramp to mix in, by how wide the window is: full at the whole track, half at a deep zoom, and never
    // zero — at the deepest zoom a little blend is what keeps the strip from turning into a comb.
    let medium = (0.5 + 0.5 * (span - 0.05) / 0.85).clamp(0.5, 1.0);
    let weight = (medium + 0.25 * (heat - 1.0)).min(0.9);
    let radius = RADII[ramp_level(columns_per_bar, heat)];
    // The window's slice with a margin wide enough for the blur to see what lies just outside it: a ramp computed on a
    // truncated slice darkens the window's edges.
    let from = covered_from.saturating_sub(radius);
    let to = (covered_to + radius).min(total);
    let raw: Vec<f32> = columns[from..to]
        .iter()
        .map(|(low, high)| high.max(-low).clamp(0.0, 1.0))
        .collect();
    let ramp = gaussian(&raw, radius);
    let max_amplitude = raw.iter().copied().fold(0.0f32, f32::max);
    let ramp_loudest = ramp.iter().copied().fold(0.0f32, f32::max);
    let scale = if weight > 0.0 {
        (1.0 - weight) * max_amplitude + weight * ramp_loudest
    } else {
        max_amplitude
    }
    .max(1e-6);
    let base = covered_from - from;
    (0..count)
        .map(|bar| {
            let a = base + bar * covered / count;
            let b = (base + (bar + 1) * covered / count)
                .max(a + 1)
                .min(raw.len());
            let columns_in_bar = (b - a) as f32;
            let own = raw[a..b].iter().sum::<f32>() / columns_in_bar;
            let height = if weight > 0.0 {
                let smoothed = ramp[a..b].iter().sum::<f32>() / columns_in_bar;
                (1.0 - weight) * own + weight * smoothed
            } else {
                own / scale
            };
            height.clamp(0.0, 1.0).powf(CONTRAST)
        })
        .collect()
}

/// How much stronger than the medium setting the smoothing is, as a curve in the zoom — the window's own anchors
/// (`smoothingHeat`): (15, 1.0), (60, 2.05), (200, 1.9), (400, 1.1) in log zoom.
fn smoothing_heat(zoom: f32) -> f32 {
    const ANCHORS: [(f32, f32); 4] = [(15.0, 1.0), (60.0, 2.05), (200.0, 1.9), (400.0, 1.1)];
    if zoom <= ANCHORS[0].0 {
        return ANCHORS[0].1;
    }
    let last = ANCHORS.len() - 1;
    if zoom >= ANCHORS[last].0 {
        return ANCHORS[last].1;
    }
    for pair in ANCHORS.windows(2) {
        let (from_zoom, from_heat) = pair[0];
        let (to_zoom, to_heat) = pair[1];
        if zoom <= to_zoom {
            let at = (zoom.ln() - from_zoom.ln()) / (to_zoom.ln() - from_zoom.ln());
            return from_heat + at * (to_heat - from_heat);
        }
    }
    ANCHORS[last].1
}

/// The ramp the window picks for this many columns per bar (`rampForZoom`): the first radius at least eight bars of
/// columns away, and the largest one when even that is small.
fn ramp_level(columns_per_bar: f32, heat: f32) -> usize {
    let wanted = (columns_per_bar * 8.0 * heat).max(1.0);
    RADII
        .iter()
        .position(|radius| *radius as f32 >= wanted)
        .unwrap_or(RADII.len() - 1)
}

/// A gaussian blur of `raw` with `radius`, the kernel the window uses: sigma is half the radius, and the weights are
/// normalised, so a flat passage keeps its level.
fn gaussian(raw: &[f32], radius: usize) -> Vec<f32> {
    let sigma = radius as f32 * 0.5;
    (0..raw.len())
        .map(|index| {
            let from = index.saturating_sub(radius);
            let to = (index + radius).min(raw.len() - 1);
            let mut sum = 0.0;
            let mut weight = 0.0;
            for (offset, value) in raw[from..=to].iter().enumerate() {
                let step = (from + offset) as f32 - index as f32;
                let kernel = (-0.5 * (step / sigma).powi(2)).exp();
                sum += value * kernel;
                weight += kernel;
            }
            if weight > 0.0 {
                sum / weight
            } else {
                raw[index]
            }
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
    fn a_loud_passage_stands_proud_of_a_quiet_one() {
        // The window's strip, copied: the loud half of a window is drawn taller than the quiet half, and one loud column
        // is a hump with shoulders rather than a needle.
        let mut columns = vec![(0.0f32, 0.05f32); 600];
        for column in &mut columns[..300] {
            *column = (0.0, 0.8);
        }
        let strip = bars(&columns, 30, 0.0, 1.0);
        assert_eq!(strip.len(), 30);
        let loud = strip[..15].iter().copied().fold(0.0f32, f32::max);
        let quiet = strip[15..].iter().copied().fold(0.0f32, f32::max);
        assert!(loud > 0.3, "the loud half is not drawn: {loud}");
        assert!(
            quiet < loud * 0.7,
            "the quiet half is as tall as the loud one"
        );
        assert!(bars(&[], 4, 0.0, 1.0).is_empty(), "no envelope, no strip");
        assert!(bars(&columns, 0, 0.0, 1.0).is_empty(), "no bars, no strip");
    }

    #[test]
    fn the_window_of_a_zoomed_view_is_the_slice_it_names() {
        // A window in the middle of the track shows the middle, not the start: the strip follows the play head.
        let mut columns = vec![(0.0f32, 0.05f32); 1000];
        for column in &mut columns[500..520] {
            *column = (0.0, 0.9);
        }
        let at_start = bars(&columns, 20, 0.0, 0.1);
        let at_middle = bars(&columns, 20, 0.45, 0.1);
        let loud_start = at_start.iter().copied().fold(0.0f32, f32::max);
        let loud_middle = at_middle.iter().copied().fold(0.0f32, f32::max);
        assert!(
            loud_middle > loud_start,
            "the window did not follow the sound: start {loud_start}, middle {loud_middle}"
        );
    }
}
