//! Внешняя модель удара в движке: уровень темпа от модели, если она установлена.
//!
//! Манифест живёт в репозитории и вшит в бинарник: весов там нет, только описание (ADR-0012).
//! Модель ищется в хранилище пользователя, библиотека ONNX Runtime — по `ORT_DYLIB_PATH` или рядом
//! с исполняемым файлом. Нет модели, нет библиотеки, модель не загрузилась, `DJ_BEATMODEL=off` —
//! работает классический детектор, и это не ошибка, а штатный путь.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use dj_analysis::tempo::{LevelPrior, Tempo};
use dj_beatmodel::{BeatModel, DbnConfig, prior};
use dj_models::{ModelEntry, Store, parse_manifest, prefer};

/// Манифест моделей: только описание, весов в репозитории нет.
const MANIFEST: &str = include_str!("../../../models/model.toml");

/// Установленная модель: запись манифеста и путь к артефакту.
struct Installed {
    entry: ModelEntry,
    path: PathBuf,
}

/// Что нашлось в хранилище: `full` важнее `small`, отсутствие — тоже ответ.
fn installed() -> Option<Installed> {
    if std::env::var("DJ_BEATMODEL").is_ok_and(|value| value == "off") {
        return None;
    }
    let manifest = parse_manifest(MANIFEST).ok()?;
    let store = Store::new(Store::default_root());
    let present: Vec<&ModelEntry> = manifest
        .model
        .iter()
        .filter(|entry| store.is_installed(entry))
        .collect();
    let entry = prefer(present)?.clone();
    let path = store.path(&entry);
    Some(Installed { entry, path })
}

/// Где лежит библиотека ONNX Runtime.
fn library() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ORT_DYLIB_PATH") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let directory = std::env::current_exe().ok()?.parent()?.to_path_buf();
    for name in [
        "libonnxruntime.so",
        "libonnxruntime.dylib",
        "onnxruntime.dll",
    ] {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Модель на процесс: загрузка стоит треть секунды, а треков за сеанс много.
fn model() -> Option<&'static Mutex<BeatModel>> {
    static MODEL: OnceLock<Option<Mutex<BeatModel>>> = OnceLock::new();
    MODEL
        .get_or_init(|| {
            let installed = installed()?;
            let library = library()?;
            match BeatModel::load(&library, &installed.path) {
                Ok(model) => {
                    eprintln!(
                        "dj-engine: модель удара {} ({})",
                        installed.entry.variant, installed.entry.license
                    );
                    Some(Mutex::new(model))
                }
                Err(error) => {
                    eprintln!("dj-engine: {error}; работаем своим детектором");
                    None
                }
            }
        })
        .as_ref()
}

/// Каким путём считается сетка: для журнала и проверок.
pub fn source() -> &'static str {
    match installed() {
        Some(installed) if model().is_some() => match installed.entry.variant.as_str() {
            "full" => "модель full",
            "small" => "модель small",
            _ => "модель",
        },
        _ => "свой детектор",
    }
}

/// Сетка трека: уровень от модели, если она есть, иначе свой замер.
pub fn analyse(path: &Path) -> Option<Tempo> {
    let (mono, rate) = match dj_analysis::tempo::decode_mono(path) {
        Ok(decoded) => decoded,
        Err(error) => {
            eprintln!("dj-engine: {}: {error}", path.display());
            return None;
        }
    };
    let guess = model().and_then(|model| {
        let mut model = match model.lock() {
            Ok(model) => model,
            Err(poisoned) => poisoned.into_inner(),
        };
        match prior(&mut model, &mono, rate, &DbnConfig::default()) {
            Ok(guess) => guess,
            Err(error) => {
                eprintln!("dj-engine: {}: {error}", path.display());
                None
            }
        }
    });
    let prior = guess.map(|guess| LevelPrior {
        bpm: guess.bpm,
        tolerance: 0.02,
        anchor: Some(guess.anchor),
    });
    dj_analysis::tempo::detect_with_prior(&mono, rate, prior.as_ref())
}
