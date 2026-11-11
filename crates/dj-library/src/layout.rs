//! Where a track lives inside the collection (DJP-MEDIA-004).
//!
//! The rules are the owner's: two levels of directories, each named by one significant character of
//! the artist and then of the title, upper case; the file itself is `<artist> - <title>.<ext>` in the
//! tags' own case. Separators never reach a directory name, so `AC/DC` cannot make a third level.

use std::path::{Path, PathBuf};

/// Letters and digits of any alphabet: what may name a directory.
fn significant(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars().filter(|character| character.is_alphanumeric())
}

/// One character in upper case, as a string: a character may grow (`ß` becomes `SS`).
fn upper(character: char) -> String {
    character.to_uppercase().collect()
}

/// The two directory levels: the artist's first two significant characters, then the title's.
///
/// A track whose tags carry neither letters nor digits lands under `#`.
pub fn prefix(artist: &str, title: &str) -> [String; 2] {
    let mut characters = significant(artist).chain(significant(title));
    let first = characters.next().map(upper);
    let second = characters.next().map(upper);
    [
        first.unwrap_or_else(|| "#".to_owned()),
        second.unwrap_or_else(|| "#".to_owned()),
    ]
}

/// What a file name may not contain, replaced rather than dropped so two names stay distinct.
fn file_safe(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character == '/' || character == '\\' || character.is_control() {
                '_'
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The file name: `<artist> - <title>.<ext>`, extension in lower case.
pub fn file_name(artist: &str, title: &str, extension: &str) -> String {
    let artist = file_safe(artist);
    let title = file_safe(title);
    let stem = match (artist.is_empty(), title.is_empty()) {
        (false, false) => format!("{artist} - {title}"),
        (false, true) => artist,
        (true, false) => title,
        (true, true) => "untitled".to_owned(),
    };
    // A title may end with a full stop (`Super Smash Bros.`); a file name may not, on Windows and in
    // most tools. The name is trimmed so the extension is the only dot at the end.
    let stem = stem.trim_end_matches(['.', ' ']).to_owned();
    let extension = extension.trim_start_matches('.').to_lowercase();
    if extension.is_empty() {
        stem
    } else {
        format!("{stem}.{extension}")
    }
}

/// The path of a track inside the collection, relative to its root.
pub fn collection_path(artist: &str, title: &str, extension: &str) -> PathBuf {
    let [first, second] = prefix(artist, title);
    Path::new(&first)
        .join(second)
        .join(file_name(artist, title, extension))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(artist: &str, title: &str, extension: &str) -> String {
        collection_path(artist, title, extension)
            .display()
            .to_string()
    }

    #[test]
    fn a_track_lands_two_levels_deep_from_its_own_characters() {
        assert_eq!(
            path("Abba", "Dancing Queen", "flac"),
            "A/B/Abba - Dancing Queen.flac"
        );
        assert_eq!(path("A", "Alone", "flac"), "A/A/A - Alone.flac");
        assert_eq!(
            path("12 Stones", "Broken", "mp3"),
            "1/2/12 Stones - Broken.mp3"
        );
        assert_eq!(
            path("Bbno$", "touch grass", "flac"),
            "B/B/Bbno$ - touch grass.flac"
        );
    }

    #[test]
    fn cyrillic_is_kept_as_it_is() {
        assert_eq!(path("Ёжик", "Туман", "mp3"), "Ё/Ж/Ёжик - Туман.mp3");
        assert_eq!(path("ДЕТИ RAVE", "АУФ", "flac"), "Д/Е/ДЕТИ RAVE - АУФ.flac");
    }

    #[test]
    fn separators_never_make_a_third_level() {
        assert_eq!(
            path("AC/DC", "Back In Black", "flac"),
            "A/C/AC_DC - Back In Black.flac"
        );
    }

    #[test]
    fn a_name_without_letters_or_digits_still_lands_somewhere() {
        assert_eq!(path("!!!", "???", "mp3"), "#/#/!!! - ???.mp3");
    }

    #[test]
    fn the_second_level_comes_from_the_title_when_the_artist_is_short() {
        assert_eq!(path("X", "9 Lives", "flac"), "X/9/X - 9 Lives.flac");
    }

    #[test]
    fn a_title_that_ends_with_a_full_stop_does_not_make_two_dots() {
        assert_eq!(
            path("Bbno$", "Super Smash Bros.", "flac"),
            "B/B/Bbno$ - Super Smash Bros.flac"
        );
        assert_eq!(
            path("Bbno$", "F.U.N.K.", "flac"),
            "B/B/Bbno$ - F.U.N.K.flac"
        );
    }

    #[test]
    fn the_extension_is_lower_case() {
        assert_eq!(
            path("Abba", "Dancing Queen", "FLAC"),
            "A/B/Abba - Dancing Queen.flac"
        );
    }
}
