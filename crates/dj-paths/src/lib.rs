//! Where the application keeps its files, on every platform it runs on.
//!
//! The Linux rules are the ones that were here before, unchanged, down to the small differences between the
//! callers: `$XDG_CONFIG_HOME` is a config folder only when it is absolute, while `$XDG_CACHE_HOME` is taken as
//! it comes. Those differences are load-bearing — a database that follows the working directory is a database
//! found twice — so they are kept here, in one place, rather than rediscovered at each call site.
//!
//! On Windows the same roles are played by `%APPDATA%` (roaming: settings and the mirror) and
//! `%LOCALAPPDATA%` (this machine: caches), and the endpoint is a named pipe instead of a socket file.

use std::path::{Path, PathBuf};

/// Root of the user's data: `$XDG_DATA_HOME` when it is absolute, else `$HOME/.local/share`.
///
/// `None` when neither is known: the caller decides what to do, and the two callers disagree about the
/// fallback, which is why this returns an option rather than a guess.
#[cfg(unix)]
pub fn data_dir() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_DATA_HOME");
    data_dir_from(
        xdg.as_deref().map(Path::new),
        std::env::var_os("HOME").as_deref().map(Path::new),
    )
}

/// The collection the engine keeps when nothing else is chosen: `<data>/hauler-dj/collection`.
///
/// The shell asks the same question and must get the same answer — an import that lands where the list does not
/// look is a track the DJ never finds again.
pub fn collection_root() -> Option<PathBuf> {
    data_dir().map(|base| base.join("hauler-dj").join("collection"))
}

/// The same decision, from values instead of the environment.
pub fn data_dir_from(xdg: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(folder) = xdg.filter(|folder| folder.is_absolute()) {
        return Some(folder.to_path_buf());
    }
    home.map(|home| home.join(".local").join("share"))
}

/// Root of the user's data, taken as it comes: no absolute check.
///
/// The metadata mirror was written with this rule before the folder above existed, and a mirror that moved
/// for a relative `$XDG_DATA_HOME` would be a second opinion about where a file lives.
#[cfg(unix)]
pub fn data_dir_raw() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}

#[cfg(windows)]
pub fn data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(windows)]
pub fn data_dir_raw() -> Option<PathBuf> {
    data_dir()
}

/// Root of the user's settings: `$XDG_CONFIG_HOME` when it is absolute, else `$HOME/.config`.
#[cfg(unix)]
pub fn config_dir() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_CONFIG_HOME");
    config_dir_from(
        xdg.as_deref().map(Path::new),
        std::env::var_os("HOME").as_deref().map(Path::new),
    )
}

/// The same decision, from values instead of the environment.
pub fn config_dir_from(xdg: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(folder) = xdg.filter(|folder| folder.is_absolute()) {
        return Some(folder.to_path_buf());
    }
    home.map(|home| home.join(".config"))
}

/// Root of the user's settings, taken as it comes: the session file's rule.
#[cfg(unix)]
pub fn config_dir_raw() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

#[cfg(windows)]
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(windows)]
pub fn config_dir_raw() -> Option<PathBuf> {
    config_dir()
}

/// Root of the user's caches: `$XDG_CACHE_HOME` as it comes, else `$HOME/.cache`.
#[cfg(unix)]
pub fn cache_dir() -> Option<PathBuf> {
    std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|home| PathBuf::from(home).join(".cache"))
        })
}

#[cfg(windows)]
pub fn cache_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

/// Where the engine listens and the shell connects.
///
/// Linux: a socket file, `$XDG_RUNTIME_DIR/hauler-dj-engine.sock` when the runtime folder is there, else
/// `/tmp/hauler-dj-engine.sock` — the two names the shell has always used.
///
/// Windows: a named pipe, named after the user so two sessions on one machine do not meet. The shell hands this
/// string to `QLocalSocket`, which treats it as a pipe name; the engine adds the `\\.\pipe\` prefix itself.
#[cfg(unix)]
pub fn default_endpoint() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let runtime = PathBuf::from(runtime);
        if runtime.is_absolute() {
            return runtime.join("hauler-dj-engine.sock");
        }
    }
    PathBuf::from("/tmp/hauler-dj-engine.sock")
}

#[cfg(windows)]
pub fn default_endpoint() -> PathBuf {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "dj".to_string());
    PathBuf::from(format!("hauler-dj-engine-{user}"))
}

/// The full name of a pipe, `\\.\pipe\<name>`, from the endpoint as the caller spells it.
#[cfg(windows)]
pub fn pipe_name(endpoint: &Path) -> String {
    let text = endpoint.display().to_string();
    if text.starts_with(r"\\.\pipe\") {
        text
    } else {
        format!(r"\\.\pipe\{text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absolute_xdg_folder_wins_and_a_relative_one_is_not_a_folder() {
        let xdg = Path::new("/var/data");
        let home = Path::new("/home/dj");
        assert_eq!(
            data_dir_from(Some(xdg), Some(home)),
            Some(PathBuf::from("/var/data"))
        );
        assert_eq!(
            data_dir_from(Some(Path::new("relative")), Some(home)),
            Some(PathBuf::from("/home/dj/.local/share"))
        );
        assert_eq!(data_dir_from(None, None), None);
        assert_eq!(
            config_dir_from(Some(Path::new("rel")), Some(home)),
            Some(PathBuf::from("/home/dj/.config"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_default_endpoint_is_the_socket_file_the_shell_has_always_used() {
        let endpoint = default_endpoint();
        assert!(
            endpoint
                .display()
                .to_string()
                .ends_with("hauler-dj-engine.sock"),
            "{}",
            endpoint.display()
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_default_endpoint_is_a_pipe_named_after_the_user() {
        let endpoint = default_endpoint();
        let full = pipe_name(&endpoint);
        assert!(full.starts_with(r"\\.\pipe\hauler-dj-engine-"), "{full}");
    }
}
