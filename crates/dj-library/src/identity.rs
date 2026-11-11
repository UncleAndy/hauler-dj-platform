//! Is this the same recording, and is it the same edition (DJP-MEDIA-005)?
//!
//! Two questions, two answers. The fingerprint answers "same recording": it survives another codec,
//! another loudness and another length of the file. The tags answer "same edition": `(Dub Mix)` and
//! `(Extended Mix)` of one take agree in the fingerprint, and the collection wants both.
//!
//! Both rules were measured on the owner's collection — see `docs/qa/evidence/B08`.

use std::io;
use std::path::{Path, PathBuf};

use crate::meta::Fingerprint;

/// Share of the fingerprint that must agree before two files are the same recording.
pub const MIN_AGREEMENT: f64 = 0.90;
/// How far apart two copies may sit in the fingerprint, in its own frames (0,12 s each).
pub const MAX_LAG: i32 = 128;
/// Length of one fingerprint frame, measured: a lag of 16 items is 2,0 s of audio.
pub const FRAME_SECONDS: f64 = 0.1238;
/// How far the shift may be from the duration difference and still be the same recording.
pub const SHIFT_TOLERANCE_MS: u64 = 300;

/// Words that name an edition rather than a credit.
///
/// `(feat. …)` and `(prod. …)` credit people, `(Original Mix)` is what a bare title means; the rest
/// name a different thing being sold.
const EDITION_WORDS: [&str; 24] = [
    "dub",
    "extended",
    "radio",
    "club",
    "instrumental",
    "acoustic",
    "live",
    "remix",
    "remixed",
    "edit",
    "rework",
    "remaster",
    "remastered",
    "bassboost",
    "sped",
    "slowed",
    "vip",
    "bootleg",
    "mashup",
    "flip",
    "cover",
    "demo",
    "reprise",
    "orchestral",
];

/// What a track is recognised by: duration and content, not its name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    /// Duration in milliseconds, as the decoder counted it.
    pub duration_ms: u64,
    /// Native sample rate of the file: frames and milliseconds are only comparable through it.
    pub sample_rate: u32,
    /// Size in bytes, which with the duration gives the average bitrate.
    pub bytes: u64,
    /// Bits per sample when the container says; the decoder does not expose it yet.
    pub bits: Option<u32>,
    /// True for FLAC, WAV and the rest of the formats that keep every sample.
    pub lossless: bool,
    /// The title the tags claim, used for the edition question only.
    pub title: String,
    /// Chromaprint subfingerprints.
    pub chromaprint: Vec<u32>,
}

impl Identity {
    /// True when the content was never measured: two such tracks are never the same recording,
    /// because nothing says they are.
    pub fn unknown(&self) -> bool {
        self.chromaprint.is_empty()
    }

    /// Average bits per second over the file: honest for VBR, where the header lies.
    pub fn bitrate(&self) -> Option<u64> {
        if self.duration_ms == 0 {
            return None;
        }
        self.bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(self.duration_ms)
    }
}

/// The verdict of comparing a candidate with one track already in the collection.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Another recording, or nothing to compare.
    Different,
    /// Same recording, same edition: one of the two files is spare.
    Duplicate { agreement: f64, lag: i32 },
    /// Same recording, another edition: both belong in the collection.
    Version { agreement: f64 },
}

impl Verdict {
    /// True when the two files are the same recording, whatever the edition.
    pub fn same_recording(&self) -> bool {
        !matches!(self, Self::Different)
    }
}

/// The set of edition words a title mentions, matched by whole words.
///
/// Whole words, not substrings: `Alive` contains `live` as letters and is not a live recording.
pub fn edition_markers(title: &str) -> Vec<&'static str> {
    let lowered = title.to_lowercase();
    let mut found: Vec<&'static str> = Vec::new();
    for token in lowered.split(|character: char| !character.is_alphanumeric()) {
        if let Some(word) = EDITION_WORDS.iter().find(|word| **word == token)
            && !found.contains(word)
        {
            found.push(word);
        }
    }
    found.sort_unstable();
    found
}

/// True when the two titles name the same edition.
pub fn same_edition(first: &str, second: &str) -> bool {
    edition_markers(first) == edition_markers(second)
}

/// How far apart the two durations may be and still be one recording.
///
/// An encoder pads: mp3 rounds the length to whole frames and keeps its delay, so a lossy file against
/// a lossless one of the same take differs by a second or two without being another edition, while
/// radio and extended mixes differ by tens of seconds.
pub fn tolerance_ms(existing: &Identity, candidate: &Identity) -> u64 {
    let longer = existing.duration_ms.max(candidate.duration_ms);
    if existing.lossless == candidate.lossless {
        (longer / 200).max(1_500)
    } else {
        (longer / 100).max(3_000)
    }
}

/// Compare a candidate with a track already in the collection.
pub fn judge(existing: &Identity, candidate: &Identity) -> Verdict {
    if existing.unknown() || candidate.unknown() {
        return Verdict::Different;
    }
    let agreement =
        dj_fingerprint::agreement(&existing.chromaprint, &candidate.chromaprint, MAX_LAG);
    let same = agreement.same();
    if same < MIN_AGREEMENT {
        return Verdict::Different;
    }
    if !same_edition(&existing.title, &candidate.title) {
        return Verdict::Version { agreement: same };
    }
    let delta = existing.duration_ms.abs_diff(candidate.duration_ms);
    // A copy that was trimmed is shifted in the fingerprint by exactly the length it lost. That is the
    // same recording, not another version — a radio edit and an extended mix do not line up.
    let shifted = (f64::from(agreement.lag.unsigned_abs()) * FRAME_SECONDS * 1_000.0) as u64;
    let trimmed = delta.abs_diff(shifted) <= SHIFT_TOLERANCE_MS;
    if delta <= tolerance_ms(existing, candidate) || trimmed {
        Verdict::Duplicate {
            agreement: same,
            lag: agreement.lag,
        }
    } else {
        Verdict::Version { agreement: same }
    }
}

/// The best verdict among the tracks already known: the strongest agreement wins.
pub fn best_match<'a>(
    candidate: &Identity,
    known: impl IntoIterator<Item = (&'a Path, &'a Identity)>,
) -> Option<(&'a Path, Verdict)> {
    let mut best: Option<(&Path, Verdict)> = None;
    for (path, existing) in known {
        let verdict = judge(existing, candidate);
        if !verdict.same_recording() {
            continue;
        }
        let agreement = match verdict {
            Verdict::Duplicate { agreement, .. } | Verdict::Version { agreement } => agreement,
            Verdict::Different => continue,
        };
        let better = match &best {
            None => true,
            Some((_, current)) => {
                let current = match current {
                    Verdict::Duplicate { agreement, .. } | Verdict::Version { agreement } => {
                        *agreement
                    }
                    Verdict::Different => 0.0,
                };
                agreement > current
            }
        };
        if better {
            best = Some((path, verdict));
        }
    }
    best
}

/// How many subfingerprints two tracks must share before a full comparison is worth it.
pub const MIN_SHARED: u32 = 8;
/// How many candidates a full comparison looks at.
pub const CANDIDATES: usize = 3;

/// What the collection already holds, indexed so a new file is compared with a handful of tracks
/// instead of all of them.
///
/// Comparing every pair costs a full agreement search per pair (frames × 257 shifts); with a hundred
/// thousand tracks that is not a search, it is a weekend. The index turns it into "which tracks share
/// subfingerprints with this one", and only those get the full comparison.
#[derive(Debug, Default)]
pub struct Catalog {
    entries: Vec<(PathBuf, Identity)>,
    by_hash: std::collections::HashMap<u32, Vec<usize>>,
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many tracks the catalogue knows.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Read the collection's own metadata files: every track that has a chromaprint joins the index.
    ///
    /// Files written before the print existed simply do not take part — they are unknown, not "not a
    /// duplicate", and the caller sees the same thing as for a track nobody measured.
    pub fn scan(root: &Path) -> Self {
        let mut catalog = Self::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(directory) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if !name.ends_with(".json")
                    || path
                        .parent()
                        .and_then(|parent| parent.file_name())
                        .and_then(|parent| parent.to_str())
                        != Some(crate::sidecar::DIRECTORY)
                {
                    continue;
                }
                let Ok(record) = crate::sidecar::read(&path) else {
                    continue;
                };
                let Some(text) = record.chromaprint else {
                    continue;
                };
                let Some(print) = crate::meta::decode_print(&text) else {
                    continue;
                };
                // `.djmeta/<имя файла>.json` → `<имя файла>` рядом с каталогом метаданных.
                let music_name = name.trim_end_matches(".json");
                let music = path
                    .parent()
                    .and_then(|parent| parent.parent())
                    .map(|parent| parent.join(music_name));
                let Some(music) = music else {
                    continue;
                };
                if !music.is_file() {
                    continue;
                }
                let duration_ms = record
                    .fingerprint
                    .sample_rate
                    .zip(record.fingerprint.duration_frames)
                    .map(|(rate, frames)| {
                        if rate == 0 {
                            0
                        } else {
                            frames.saturating_mul(1_000) / u64::from(rate)
                        }
                    })
                    .unwrap_or(0);
                let lossless = matches!(
                    music
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .and_then(dj_codec::MediaFormat::from_extension),
                    Some(format) if !format.is_lossy()
                );
                let bytes = std::fs::metadata(&music)
                    .map(|meta| meta.len())
                    .unwrap_or(0);
                catalog.add(
                    music,
                    Identity {
                        duration_ms,
                        sample_rate: record.fingerprint.sample_rate.unwrap_or(0),
                        bytes,
                        bits: None,
                        lossless,
                        title: String::new(),
                        chromaprint: print,
                    },
                );
            }
        }
        catalog
    }

    /// Remember a track. Its own subfingerprints join the index, so the next file can find it.
    pub fn add(&mut self, path: PathBuf, identity: Identity) {
        let index = self.entries.len();
        for hash in &identity.chromaprint {
            self.by_hash.entry(*hash).or_default().push(index);
        }
        self.entries.push((path, identity));
    }

    /// The tracks worth a full comparison, most shared subfingerprints first.
    fn candidates(&self, candidate: &Identity) -> Vec<usize> {
        let mut shared: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();
        for hash in &candidate.chromaprint {
            if let Some(entries) = self.by_hash.get(hash) {
                for &entry in entries {
                    *shared.entry(entry).or_default() += 1;
                }
            }
        }
        let mut ranked: Vec<(usize, u32)> = shared.into_iter().collect();
        ranked.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        ranked
            .into_iter()
            .filter(|(_, count)| *count >= MIN_SHARED)
            .take(CANDIDATES)
            .map(|(entry, _)| entry)
            .collect()
    }

    /// The best verdict among the candidates: the strongest agreement wins.
    pub fn find(&self, candidate: &Identity) -> Option<(&Path, &Identity, Verdict)> {
        let mut best: Option<(usize, Verdict)> = None;
        for entry in self.candidates(candidate) {
            let verdict = judge(&self.entries[entry].1, candidate);
            let agreement = match verdict {
                Verdict::Duplicate { agreement, .. } | Verdict::Version { agreement } => agreement,
                Verdict::Different => continue,
            };
            let better = match &best {
                None => true,
                Some((_, current)) => {
                    let current = match current {
                        Verdict::Duplicate { agreement, .. } | Verdict::Version { agreement } => {
                            *agreement
                        }
                        Verdict::Different => 0.0,
                    };
                    agreement > current
                }
            };
            if better {
                best = Some((entry, verdict));
            }
        }
        best.map(|(entry, verdict)| {
            let (path, identity) = &self.entries[entry];
            (path.as_path(), identity, verdict)
        })
    }
}

/// Measure a file's identity: decode it, count its frames, take its chromaprint.
///
/// This is the expensive part of an import — the whole file is decoded — and the reason the batch runs
/// on several workers. `title` comes from the tags the caller already read.
pub fn of_file(path: &Path, title: &str) -> io::Result<Identity> {
    let mut decoder = dj_codec::StreamDecoder::open(path).map_err(io::Error::other)?;
    let rate = decoder.sample_rate();
    let channels = usize::from(decoder.info().channels.max(1));
    let lossless = !decoder.info().format.is_lossy();
    let mut buffer = vec![0.0_f32; 8_192 * 2];
    let mut samples: Vec<i16> = Vec::new();
    loop {
        let read = decoder
            .read_frames(&mut buffer)
            .map_err(|error| io::Error::other(error.to_string()))?;
        if read == 0 {
            break;
        }
        let count = (read * channels).min(buffer.len());
        for value in &buffer[..count] {
            samples.push((value.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16);
        }
    }
    let frames = samples.len() / channels;
    let duration_ms = if rate == 0 {
        0
    } else {
        u64::try_from(frames).unwrap_or(0) * 1_000 / u64::from(rate)
    };
    let chromaprint =
        dj_fingerprint::of_samples(&samples, rate, u16::try_from(channels).unwrap_or(1))
            .map_err(|error| io::Error::other(error.to_string()))?;
    let bytes = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    Ok(Identity {
        duration_ms,
        sample_rate: rate,
        bytes,
        bits: None,
        lossless,
        title: title.to_owned(),
        chromaprint,
    })
}

/// The file fingerprint of a track, for the metadata file (DJP-DATA-004).
pub fn file_fingerprint(path: &Path) -> io::Result<Fingerprint> {
    crate::fingerprint::of_path(path)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn identity(title: &str, duration_ms: u64, lossless: bool, print: Vec<u32>) -> Identity {
        Identity {
            duration_ms,
            sample_rate: 44_100,
            bytes: 0,
            bits: None,
            lossless,
            title: title.to_owned(),
            chromaprint: print,
        }
    }

    /// A print of `count` values where every value is `seed + index`: two such prints agree fully when
    /// the seeds match and not at all when they do not.
    fn print(seed: u32, count: usize) -> Vec<u32> {
        (0..count).map(|index| seed + index as u32).collect()
    }

    #[test]
    fn the_same_print_and_length_is_a_duplicate() {
        let first = identity("Touch Grass", 180_000, false, print(1_000, 400));
        let second = identity(
            "touch grass (feat. Yung Gravy)",
            180_020,
            false,
            print(1_000, 400),
        );
        assert!(matches!(judge(&first, &second), Verdict::Duplicate { .. }));
    }

    #[test]
    fn a_trimmed_copy_is_still_the_same_recording() {
        // Two seconds shorter, shifted by sixteen fingerprint frames — the measured case.
        let first = identity("Track", 182_000, false, print(7_000, 600));
        let mut shifted = vec![0_u32; 16];
        shifted.extend(print(7_000, 584));
        let second = identity("Track", 180_000, false, shifted);
        assert!(matches!(judge(&first, &second), Verdict::Duplicate { .. }));
    }

    #[test]
    fn another_edition_is_a_version_even_at_full_agreement() {
        let first = identity("Bones (Dub Mix)", 200_000, true, print(3_000, 500));
        let second = identity("Bones (Extended Mix)", 200_018, true, print(3_000, 500));
        assert!(matches!(judge(&first, &second), Verdict::Version { .. }));
    }

    #[test]
    fn a_credit_is_not_an_edition() {
        let first = identity("C’est La Vie", 200_000, true, print(5_000, 500));
        let second = identity(
            "C'est La Vie (feat. Rich Brian)",
            200_000,
            true,
            print(5_000, 500),
        );
        assert!(matches!(judge(&first, &second), Verdict::Duplicate { .. }));
    }

    #[test]
    fn a_lossy_copy_of_a_lossless_track_gets_room_for_encoder_padding() {
        let first = identity("Controla", 180_000, true, print(9_000, 500));
        let second = identity("CONTROLA", 181_689, false, print(9_000, 500));
        assert!(matches!(judge(&first, &second), Verdict::Duplicate { .. }));
    }

    #[test]
    fn strangers_and_unmeasured_tracks_are_never_duplicates() {
        let first = identity("One", 180_000, false, print(1_000, 400));
        let stranger = identity("Two", 180_000, false, print(400_000, 400));
        assert_eq!(judge(&first, &stranger), Verdict::Different);
        let unmeasured = identity("One", 180_000, false, Vec::new());
        assert_eq!(judge(&first, &unmeasured), Verdict::Different);
    }

    #[test]
    fn the_strongest_match_wins() {
        let candidate = identity("Track", 180_000, false, print(2_000, 400));
        // Слабое совпадение: каждый сороковой отпечаток испорчен — по-прежнему та же запись, но
        // заметно хуже сильной.
        let weak: Vec<u32> = print(2_000, 400)
            .iter()
            .enumerate()
            .map(|(index, value)| {
                if index % 40 == 0 {
                    value ^ 0xFFFF
                } else {
                    *value
                }
            })
            .collect();
        let weak = identity("Track", 180_000, false, weak);
        let strong = identity("Track", 180_000, false, print(2_000, 400));
        let known = [
            (Path::new("/a/weak.flac"), &weak),
            (Path::new("/a/strong.flac"), &strong),
        ];
        let (path, _) = best_match(&candidate, known).expect("совпадение");
        assert_eq!(path, Path::new("/a/strong.flac"));
    }

    #[test]
    fn the_catalogue_finds_the_same_track_among_strangers() {
        let mut catalog = Catalog::new();
        for index in 0..50_u32 {
            catalog.add(
                PathBuf::from(format!("/a/{index}.flac")),
                identity(
                    &format!("Track {index}"),
                    180_000,
                    false,
                    print(index * 10_000 + 500, 400),
                ),
            );
        }
        let candidate = identity("Track 7", 180_000, false, print(7 * 10_000 + 500, 400));
        let (path, _, verdict) = catalog.find(&candidate).expect("не нашли свой же трек");
        assert_eq!(path, Path::new("/a/7.flac"));
        assert!(matches!(verdict, Verdict::Duplicate { .. }));

        let stranger = identity("New", 180_000, false, print(900_000, 400));
        assert!(catalog.find(&stranger).is_none(), "чужой принят за свой");
    }

    #[test]
    fn a_track_added_to_the_catalogue_is_found_by_the_next_copy() {
        let mut catalog = Catalog::new();
        let first = identity("Track", 180_000, false, print(4_000, 400));
        catalog.add(PathBuf::from("/a/first.flac"), first.clone());
        assert_eq!(catalog.len(), 1);
        let second = identity("Track", 180_020, false, print(4_000, 400));
        let (path, _, _) = catalog.find(&second).expect("копия не найдена");
        assert_eq!(path, Path::new("/a/first.flac"));
    }
}
