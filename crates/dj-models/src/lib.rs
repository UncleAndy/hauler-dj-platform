//! Внешние модели удара: манифест, хранилище, проверка и загрузка.
//!
//! В репозитории весов нет и не будет (ADR-0012). Здесь только то, что нужно, чтобы скачать модель
//! по запросу пользователя, убедиться, что пришли именно те байты, и решить, какую модель брать,
//! если их несколько. Сетевой клиент живёт в этом крейте и только здесь: путь анализа без модели
//! его не касается.
//!
//! Решение о выборе — по доступности (ADR-0012, «Две величины одной модели»): есть `full` — работает
//! он, есть только `small` — он, нет ни одного — классический детектор.

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Одна запись манифеста: что скачать, чем проверить и что показать пользователю.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct ModelEntry {
    /// Идентификатор модели, одинаковый у всех её величин.
    pub id: String,
    /// Величина: `full` или `small`. По ней решается выбор и лежит каталог.
    pub variant: String,
    /// Человеческое название для экрана загрузки.
    pub title: String,
    /// Версия артефакта.
    pub version: String,
    /// Формат артефакта: пока только `onnx`.
    pub format: String,
    /// Размер артефакта в байтах, как его ждёт проверка.
    pub bytes: u64,
    /// SHA-256 артефакта, шестнадцатеричной строкой в нижнем регистре.
    pub sha256: String,
    /// Откуда скачивать.
    pub artifact: String,
    /// Первоисточник модели: по нему сверяют, что артефакт — производное именно его.
    pub source: String,
    /// Лицензия артефакта в нотации SPDX.
    pub license: String,
    /// Ссылка на текст лицензии.
    pub license_url: String,
    /// Атрибуция: кого называть.
    pub attribution: String,
}

/// Манифест целиком: версия схемы и записи.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Manifest {
    /// Версия схемы манифеста.
    pub version: u32,
    /// Тег релиза, в котором лежат артефакты.
    #[serde(default)]
    pub release_tag: Option<String>,
    /// Записи моделей.
    #[serde(default)]
    pub model: Vec<ModelEntry>,
}

/// Что пошло не так при разборе, проверке или загрузке.
#[derive(Debug)]
pub enum ModelError {
    /// Манифест не читается или в нём нет записей.
    Manifest(String),
    /// Файл не найден там, где его ждут.
    Missing(PathBuf),
    /// Размер не тот, что в манифесте.
    Size {
        /// Сколько байт в манифесте.
        expected: u64,
        /// Сколько байт на диске.
        actual: u64,
    },
    /// Хэш не тот, что в манифесте.
    Checksum {
        /// Что ждали.
        expected: String,
        /// Что получили.
        actual: String,
    },
    /// Сеть или диск.
    Io(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::Manifest(reason) => write!(formatter, "манифест: {reason}"),
            ModelError::Missing(path) => write!(formatter, "нет файла {}", path.display()),
            ModelError::Size { expected, actual } => {
                write!(formatter, "размер {actual} вместо {expected}")
            }
            ModelError::Checksum { expected, actual } => {
                write!(formatter, "sha256 {actual} вместо {expected}")
            }
            ModelError::Io(reason) => write!(formatter, "{reason}"),
        }
    }
}

impl std::error::Error for ModelError {}

impl From<io::Error> for ModelError {
    fn from(error: io::Error) -> Self {
        ModelError::Io(error.to_string())
    }
}

/// Разобрать манифест из текста.
pub fn parse_manifest(text: &str) -> Result<Manifest, ModelError> {
    let manifest: Manifest =
        toml::from_str(text).map_err(|error| ModelError::Manifest(error.to_string()))?;
    if manifest.model.is_empty() {
        return Err(ModelError::Manifest("ни одной записи".to_string()));
    }
    for entry in &manifest.model {
        if entry.sha256.len() != 64 || !entry.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ModelError::Manifest(format!(
                "у записи {} неверный sha256",
                entry.variant
            )));
        }
        if entry.bytes == 0 {
            return Err(ModelError::Manifest(format!(
                "у записи {} нулевой размер",
                entry.variant
            )));
        }
    }
    Ok(manifest)
}

/// Хранилище моделей: `<корень>/<id>/<величина>/<версия>/model.onnx`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Хранилище в заданном каталоге.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Store { root: root.into() }
    }

    /// Хранилище по умолчанию: `$XDG_DATA_HOME/hauler-dj/models`.
    pub fn default_root() -> PathBuf {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .unwrap_or_else(|| PathBuf::from(".local/share"));
        base.join("hauler-dj").join("models")
    }

    /// Каталог записи.
    pub fn dir(&self, entry: &ModelEntry) -> PathBuf {
        self.root
            .join(&entry.id)
            .join(&entry.variant)
            .join(&entry.version)
    }

    /// Путь артефакта записи.
    pub fn path(&self, entry: &ModelEntry) -> PathBuf {
        self.dir(entry).join("model.onnx")
    }

    /// Лежит ли файл и совпадает ли размер.
    pub fn is_installed(&self, entry: &ModelEntry) -> bool {
        match fs::metadata(self.path(entry)) {
            Ok(metadata) => metadata.is_file() && metadata.len() == entry.bytes,
            Err(_) => false,
        }
    }

    /// Сверить установленный файл с манифестом по размеру и SHA-256.
    pub fn verify(&self, entry: &ModelEntry) -> Result<(), ModelError> {
        let path = self.path(entry);
        let metadata = fs::metadata(&path).map_err(|_| ModelError::Missing(path.clone()))?;
        if metadata.len() != entry.bytes {
            return Err(ModelError::Size {
                expected: entry.bytes,
                actual: metadata.len(),
            });
        }
        let actual = sha256_of(&path)?;
        if actual != entry.sha256.to_lowercase() {
            return Err(ModelError::Checksum {
                expected: entry.sha256.clone(),
                actual,
            });
        }
        Ok(())
    }

    /// Убрать установленный артефакт; отсутствие файла — не ошибка.
    pub fn remove(&self, entry: &ModelEntry) -> Result<(), ModelError> {
        let dir = self.dir(entry);
        match fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(ModelError::Io(error.to_string())),
        }
    }
}

/// SHA-256 файла, шестнадцатеричной строкой.
pub fn sha256_of(path: &Path) -> Result<String, ModelError> {
    let mut file = fs::File::open(path).map_err(|_| ModelError::Missing(path.to_path_buf()))?;
    let mut hasher = Sha256::new();
    let mut block = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut block)?;
        if read == 0 {
            break;
        }
        hasher.update(&block[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Какую модель брать, когда их несколько: `full` важнее `small`, `small` важнее ничего.
pub fn prefer<'a>(installed: impl IntoIterator<Item = &'a ModelEntry>) -> Option<&'a ModelEntry> {
    let mut best: Option<&ModelEntry> = None;
    for entry in installed {
        let better = match best {
            None => true,
            Some(current) => rank(entry) > rank(current),
        };
        if better {
            best = Some(entry);
        }
    }
    best
}

/// Вес величины: больше — предпочтительнее.
fn rank(entry: &ModelEntry) -> u8 {
    match entry.variant.as_str() {
        "full" => 2,
        "small" => 1,
        _ => 0,
    }
}

/// Скачать артефакт записи в хранилище и проверить его.
///
/// Файл пишется рядом (`.part`) и переименовывается только после сверки хэша: недокачанный или
/// подменённый артефакт не должен оказаться там, откуда его возьмут.
pub fn install(entry: &ModelEntry, store: &Store) -> Result<PathBuf, ModelError> {
    let dir = store.dir(entry);
    fs::create_dir_all(&dir)?;
    let final_path = store.path(entry);
    let part_path = final_path.with_extension("onnx.part");

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(900)))
        .build()
        .new_agent();
    let mut response = agent
        .get(&entry.artifact)
        .call()
        .map_err(|error| ModelError::Io(format!("{}: {error}", entry.artifact)))?;
    let mut reader = response.body_mut().as_reader();
    let mut file = fs::File::create(&part_path)?;
    let mut hasher = Sha256::new();
    let mut block = vec![0u8; 64 * 1024];
    let mut written = 0u64;
    loop {
        let read = reader.read(&mut block)?;
        if read == 0 {
            break;
        }
        hasher.update(&block[..read]);
        file.write_all(&block[..read])?;
        written += read as u64;
    }
    file.flush()?;
    drop(file);

    if written != entry.bytes {
        let _ = fs::remove_file(&part_path);
        return Err(ModelError::Size {
            expected: entry.bytes,
            actual: written,
        });
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != entry.sha256.to_lowercase() {
        let _ = fs::remove_file(&part_path);
        return Err(ModelError::Checksum {
            expected: entry.sha256.clone(),
            actual,
        });
    }
    fs::rename(&part_path, &final_path)?;
    Ok(final_path)
}
