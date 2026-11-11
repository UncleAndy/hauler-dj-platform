//! The tags a file carries, and the encodings they were written in.
//!
//! Reading is symphonia's job. Repairing is ours: a tag written in cp1251 and read as Latin-1, or in
//! UTF-8 and read as cp1252, is common enough in a real collection to be a rule rather than a
//! curiosity — see `docs/architecture/library.md`.

use std::fs::File;
use std::io;
use std::path::Path;

use symphonia::core::formats::FormatOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardTag};

/// The fields the collection needs. `count` is how many tags the file carried at all, which is how a
/// file with nothing in it is told from a file with an empty artist.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tags {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub genre: String,
    pub year: String,
    pub track: String,
    pub count: usize,
}

impl Tags {
    /// True when there is nothing to build a name from.
    pub fn is_empty(&self) -> bool {
        self.artist.trim().is_empty() && self.title.trim().is_empty()
    }
}

/// Read the container's tags. A file the reader cannot parse yields an empty set, not an error: the
/// import still has a name to build from the file name itself.
pub fn read(path: &Path) -> io::Result<Tags> {
    let file = File::open(path)?;
    let source = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(io::Error::other)?;
    let mut tags = Tags::default();
    if let Some(revision) = format.metadata().current() {
        for tag in &revision.media.tags {
            tags.count += 1;
            match &tag.std {
                Some(StandardTag::Artist(value)) => tags.artist = value.to_string(),
                Some(StandardTag::TrackTitle(value)) => tags.title = value.to_string(),
                Some(StandardTag::Album(value)) => tags.album = value.to_string(),
                Some(StandardTag::Genre(value)) => tags.genre = value.to_string(),
                Some(StandardTag::RecordingDate(value)) => tags.year = value.to_string(),
                Some(StandardTag::TrackNumber(number)) => tags.track = number.to_string(),
                _ => {}
            }
        }
    }
    Ok(tags)
}

/// What happened to a string on its way through [`repair`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing but ASCII: no encoding to get wrong.
    Ascii,
    /// Non-ASCII, but plausible text already.
    Clean,
    /// Decoded again from the encoding it was really written in.
    Repaired(&'static str),
    /// Bytes were already lost before we saw them.
    Lost,
}

impl Verdict {
    /// True when the text changed and the file is worth rewriting.
    pub fn changed(self) -> bool {
        matches!(self, Self::Repaired(_))
    }
}

/// Characters that broken bytes produce when they are read in the wrong encoding.
fn suspicious(character: char) -> bool {
    ('\u{00A0}'..='\u{017F}').contains(&character) || ('\u{2000}'..='\u{2030}').contains(&character)
}

/// How much of a string is made of those characters.
fn suspicious_ratio(text: &str) -> f64 {
    let total = text.chars().count();
    if total == 0 {
        return 0.0;
    }
    let bad = text
        .chars()
        .filter(|character| suspicious(*character))
        .count();
    bad as f64 / total as f64
}

/// How much a string looks like text someone meant to write.
fn text_score(text: &str) -> f64 {
    let mut good = 0.0_f64;
    let mut bad = 0.0_f64;
    for character in text.chars() {
        if character.is_ascii_alphanumeric() || ('\u{0400}'..='\u{04FF}').contains(&character) {
            good += 1.0;
        } else if character == ' ' || ".,'-&()/+:".contains(character) {
            good += 0.8;
        } else if character.is_control() || character == '\u{FFFD}' {
            bad += 3.0;
        } else if suspicious(character) {
            bad += 1.5;
        } else if character.is_alphabetic() {
            // Another alphabet: not evidence of damage, not evidence of health either.
            good += 0.5;
        } else {
            bad += 0.5;
        }
    }
    good / (good + bad).max(1.0)
}

/// True when no single word mixes Latin and Cyrillic letters.
///
/// The two repairs that matter — cp1251 read as Latin-1 and UTF-8 read as cp1252 — produce text in one
/// alphabet, word by word. A candidate that turns `Käärijä` into `Kддrijд` mixes them inside a word,
/// and that is the signature of a guess rather than of a repair.
pub fn single_script(text: &str) -> bool {
    for word in text.split_whitespace() {
        let mut latin = false;
        let mut cyrillic = false;
        for character in word.chars() {
            if character.is_ascii_alphabetic() {
                latin = true;
            } else if ('\u{0400}'..='\u{04FF}').contains(&character) {
                cyrillic = true;
            }
        }
        if latin && cyrillic {
            return false;
        }
    }
    true
}

/// cp1251 as a table: the Cyrillic block sits at 0xC0, and two letters live outside it.
fn cp1251(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| match byte {
            0x00..=0x7F => byte as char,
            0xA8 => 'Ё',
            0xB8 => 'ё',
            0xC0..=0xFF => char::from_u32(0x0410 + u32::from(byte - 0xC0)).unwrap_or('?'),
            _ => '?',
        })
        .collect()
}

/// The bytes a Latin-1 reader would have produced for this text.
fn latin1_bytes(text: &str) -> Option<Vec<u8>> {
    text.chars()
        .map(|character| u8::try_from(u32::from(character)).ok())
        .collect()
}

/// The bytes a cp1252 reader would have produced, including the block cp1252 puts above 0x80.
fn cp1252_bytes(text: &str) -> Option<Vec<u8>> {
    const HIGH: [(char, u8); 27] = [
        ('\u{20AC}', 0x80),
        ('\u{201A}', 0x82),
        ('\u{0192}', 0x83),
        ('\u{201E}', 0x84),
        ('\u{2026}', 0x85),
        ('\u{2020}', 0x86),
        ('\u{2021}', 0x87),
        ('\u{02C6}', 0x88),
        ('\u{2030}', 0x89),
        ('\u{0160}', 0x8A),
        ('\u{2039}', 0x8B),
        ('\u{0152}', 0x8C),
        ('\u{017D}', 0x8E),
        ('\u{2018}', 0x91),
        ('\u{2019}', 0x92),
        ('\u{201C}', 0x93),
        ('\u{201D}', 0x94),
        ('\u{2022}', 0x95),
        ('\u{2013}', 0x96),
        ('\u{2014}', 0x97),
        ('\u{02DC}', 0x98),
        ('\u{2122}', 0x99),
        ('\u{0161}', 0x9A),
        ('\u{203A}', 0x9B),
        ('\u{0153}', 0x9C),
        ('\u{017E}', 0x9E),
        ('\u{0178}', 0x9F),
    ];
    let mut bytes = Vec::with_capacity(text.len());
    for character in text.chars() {
        if let Ok(byte) = u8::try_from(u32::from(character)) {
            bytes.push(byte);
            continue;
        }
        let (_, byte) = HIGH.iter().find(|(known, _)| *known == character)?;
        bytes.push(*byte);
    }
    Some(bytes)
}

/// Repair a tag string written in the wrong encoding.
///
/// Repair is attempted only when a good share of the string is made of the characters broken bytes
/// produce: one accented letter in a title is a title, not damage.
pub fn repair(text: &str) -> (String, Verdict) {
    if text.is_empty() {
        return (String::new(), Verdict::Clean);
    }
    if text.is_ascii() {
        return (text.to_owned(), Verdict::Ascii);
    }
    if text.contains('\u{FFFD}') {
        return (text.to_owned(), Verdict::Lost);
    }
    if suspicious_ratio(text) < 0.3 {
        return (text.to_owned(), Verdict::Clean);
    }
    let original = text_score(text);
    let mut best = (text.to_owned(), Verdict::Clean, original);
    let consider =
        |candidate: Option<String>, verdict: Verdict, best: &mut (String, Verdict, f64)| {
            let Some(candidate) = candidate else {
                return;
            };
            if suspicious_ratio(&candidate) > 0.0 || !single_script(&candidate) {
                return;
            }
            let score = text_score(&candidate);
            if score > best.2 + 0.05 {
                *best = (candidate, verdict, score);
            }
        };
    if let Some(bytes) = latin1_bytes(text) {
        consider(Some(cp1251(&bytes)), Verdict::Repaired("cp1251"), &mut best);
    }
    if let Some(bytes) = cp1252_bytes(text)
        && let Ok(candidate) = std::str::from_utf8(&bytes)
    {
        consider(
            Some(candidate.to_owned()),
            Verdict::Repaired("utf8"),
            &mut best,
        );
    }
    (best.0, best.1)
}

/// Write the tags back into a file, keeping every field we did not touch.
///
/// Returns false when the container has no tag to write into, which is a fact about the file rather
/// than a failure: the collection still gets the track, under the name its tags earned.
pub fn write(path: &Path, tags: &Tags) -> io::Result<bool> {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::tag::{ItemKey, Tag, TagExt};

    let mut tagged = lofty::read_from_path(path).map_err(io::Error::other)?;
    if tagged.primary_tag().is_none() {
        let kind = tagged.primary_tag_type();
        tagged.insert_tag(Tag::new(kind));
    }
    let Some(tag) = tagged.primary_tag_mut() else {
        return Ok(false);
    };
    for (key, value) in [
        (ItemKey::TrackArtist, &tags.artist),
        (ItemKey::TrackTitle, &tags.title),
        (ItemKey::AlbumTitle, &tags.album),
        (ItemKey::Genre, &tags.genre),
        (ItemKey::RecordingDate, &tags.year),
        (ItemKey::TrackNumber, &tags.track),
    ] {
        if value.trim().is_empty() {
            tag.remove_key(&key);
        } else {
            tag.insert_text(key, value.clone());
        }
    }
    tag.save_to_path(path, WriteOptions::default())
        .map_err(io::Error::other)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes of a cp1252 file read as Latin-1, which is what a broken tag looks like.
    fn cp1252_decode(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|byte| char::from(*byte))
            .collect::<String>()
    }

    #[test]
    fn a_cp1251_artist_read_as_latin1_is_repaired() {
        // «Москва» in cp1251, read by a reader that assumed Latin-1.
        let broken = cp1252_decode(b"\xcc\xee\xf1\xea\xe2\xe0");
        let (fixed, verdict) = repair(&broken);
        assert_eq!(fixed, "Москва", "получилось «{fixed}»");
        assert_eq!(verdict, Verdict::Repaired("cp1251"));
    }

    #[test]
    fn utf8_read_as_cp1252_is_repaired() {
        // «Москва» in UTF-8, shown by a cp1252 reader: the `ÐœÐ¾ÑÐºÐ²Ð°` case.
        let broken = cp1252_decode("Москва".as_bytes());
        let (fixed, verdict) = repair(&broken);
        assert_eq!(fixed, "Москва", "получилось «{fixed}»");
        assert_eq!(verdict, Verdict::Repaired("utf8"));
    }

    #[test]
    fn clean_text_is_left_alone() {
        for text in [
            "Москва",
            "Abba",
            "Ёжик — Туман",
            "Track 12",
            "Édith Piaf",
            "Björk — Jóga",
            "Sigur Rós",
        ] {
            let (fixed, verdict) = repair(text);
            assert_eq!(fixed, text, "испортили «{text}»");
            assert!(matches!(verdict, Verdict::Clean | Verdict::Ascii), "{text}");
        }
    }

    #[test]
    fn lost_bytes_are_reported_rather_than_guessed() {
        let (fixed, verdict) = repair("Москва\u{FFFD}");
        assert_eq!(fixed, "Москва\u{FFFD}");
        assert_eq!(verdict, Verdict::Lost);
    }

    #[test]
    fn a_correct_name_with_accents_is_not_repaired_into_cyrillic() {
        // Käärijä is a Finnish artist. Read as Latin-1 his four `ä` are 0xE4, and 0xE4 in cp1251 is
        // `д`, so a repair that only asks "does this look like text?" turns the name into `Kддrijд` —
        // a word in two alphabets at once, which a repair never produces.
        let (artist, verdict) = repair("Käärijä");
        assert_eq!(artist, "Käärijä", "испортили имя: «{artist}»");
        assert_eq!(verdict, Verdict::Clean);
    }

    #[test]
    fn a_guess_that_mixes_alphabets_is_rejected() {
        assert!(!single_script("Kддrijд"));
        assert!(single_script("Москва"));
        assert!(single_script("Käärijä"));
        assert!(single_script("Steve Levi - Lost"));
        assert!(single_script("Ёжик — Туман"));
    }
}
