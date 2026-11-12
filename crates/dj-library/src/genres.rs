//! Жанры как список (DJP-MEDIA-004).
//!
//! Тег хранит одну строку, а коллекции нужен список: диджей фильтрует по жанру, и трек может
//! принадлежать нескольким. Правила разбора намеренно консервативные: что не разделилось — остаётся
//! одним жанром со своим странным написанием, потому что потерять жанр хуже, чем показать его не так.

/// Самое длинное имя, которое ещё может быть жанром: абзац в теге — не список жанров.
pub const MAX_NAME_CHARS: usize = 64;

/// Сколько жанров может нести один трек. Дальше тег — описание, а не перечисление.
pub const MAX_GENRES: usize = 16;

/// Один жанр, готовый для индекса: написание из тега и ключ, который делает два написания одним.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenreName {
    /// Имя так, как его написал тег. Побеждает первое написание, остальные — тот же жанр.
    pub name: String,
    /// `name` в нижнем регистре: тождество жанра, `house` == `House` == `HOUSE`.
    pub key: String,
}

/// Разобрать тег жанра на жанры.
///
/// `,` `;` и `/` перечисляют жанры; `&` — нет: «Drum & Bass» это один жанр. Пробелы обрезаются и
/// схлопываются, пустые куски отбрасываются, а жанр, уже встреченный в другом регистре, не двоится.
pub fn split(raw: &str) -> Vec<GenreName> {
    let mut genres: Vec<GenreName> = Vec::new();
    for piece in raw.split([',', ';', '/']) {
        let name = piece.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
            continue;
        }
        let key = name.to_lowercase();
        if genres.iter().any(|genre| genre.key == key) {
            continue;
        }
        genres.push(GenreName { name, key });
        if genres.len() == MAX_GENRES {
            break;
        }
    }
    genres
}
