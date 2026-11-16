//! Внешняя модель удара в движке: уровень темпа от модели, если она установлена.
//!
//! Манифест живёт в репозитории и вшит в бинарник: весов там нет, только описание (ADR-0012).
//! Модель ищется в хранилище пользователя, библиотека ONNX Runtime — по `ORT_DYLIB_PATH` или рядом
//! с исполняемым файлом. Нет модели, нет библиотеки, модель не загрузилась, `DJ_BEATMODEL=off` —
//! работает классический детектор, и это не ошибка, а штатный путь.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

/// Состояние загрузки модели на процесс: загрузка стоит треть секунды, а треков за сеанс много.
///
/// `tried` не даёт загружать заново на каждом треке, если модель не загрузилась. После скачивания
/// или удаления состояние сбрасывается: следующая сетка перечитает хранилище.
#[derive(Default)]
struct Session {
    model: Option<BeatModel>,
    tried: bool,
}

static SESSION: Mutex<Session> = Mutex::new(Session {
    model: None,
    tried: false,
});

/// Забыть загруженную сессию: после скачивания или удаления модель другая.
fn invalidate() {
    let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
    session.model = None;
    session.tried = false;
}

/// Дать модели посчитать: загружает при первом обращении, `None` — модели нет или она не встала.
fn with_model<T>(work: impl FnOnce(&mut BeatModel) -> T) -> Option<T> {
    let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
    if !session.tried {
        session.tried = true;
        if let (Some(installed), Some(library)) = (installed(), library()) {
            match BeatModel::load(&library, &installed.path) {
                Ok(model) => {
                    eprintln!(
                        "dj-engine: beat model ready: {} ({})",
                        installed.entry.variant, installed.entry.license
                    );
                    session.model = Some(model);
                }
                Err(error) => eprintln!("dj-engine: {error}; using the built-in detector"),
            }
        }
    }
    session.model.as_mut().map(work)
}

/// Объявить установленную модель в состоянии движка: с первой публикации после старта.
///
/// Без этого снапшот молчит до первой команды модели, и экран после рестарта показывает «built-in»
/// поверх установленной модели.
pub fn seed_model(cells: &crate::StateCells, root: &Path) {
    cells.set_model(installed_status_at(root));
}

/// Сказать в журнал при старте, чем будет считаться сетка.
///
/// Без этой строки тихий откат неотличим от работающей модели: установленная модель без библиотеки
/// ONNX Runtime молчит, и «новые методы не помогли» ищут в детекторе, а не в отсутствующем файле.
pub fn announce() {
    let status = installed_status();
    if status.variant.is_empty() {
        eprintln!("dj-engine: no beat model installed — the grid comes from the built-in detector");
        return;
    }
    match library() {
        Some(path) => eprintln!(
            "dj-engine: beat model {} ({}) from {}",
            status.variant,
            status.license,
            path.display()
        ),
        None => eprintln!(
            "dj-engine: beat model {} is installed but libonnxruntime.so was not found — using the \
             built-in detector (put the library next to the engine or set ORT_DYLIB_PATH)",
            status.variant
        ),
    }
}

/// Что известно про модель прямо сейчас: для снапшота.
pub fn installed_status() -> dj_domain::ModelStatus {
    installed_status_at(&Store::default_root())
}

/// То же, но для явно названного хранилища: так это проверяется без переменных окружения.
pub fn installed_status_at(root: &Path) -> dj_domain::ModelStatus {
    let Ok(manifest) = parse_manifest(MANIFEST) else {
        return dj_domain::ModelStatus::default();
    };
    let store = Store::new(root);
    let present: Vec<&ModelEntry> = manifest
        .model
        .iter()
        .filter(|entry| store.is_installed(entry))
        .collect();
    match prefer(present) {
        Some(entry) => dj_domain::ModelStatus {
            variant: entry.variant.clone(),
            license: entry.license.clone(),
            busy: false,
            error: String::new(),
        },
        None => dj_domain::ModelStatus::default(),
    }
}

/// Скачать величину в хранилище пользователя.
pub fn download(variant: &str) -> Result<dj_domain::ModelStatus, String> {
    let manifest = parse_manifest(MANIFEST).map_err(|error| error.to_string())?;
    let entry = manifest
        .model
        .iter()
        .find(|entry| entry.variant == variant)
        .ok_or_else(|| format!("в манифесте нет величины {variant}"))?;
    let store = Store::new(Store::default_root());
    dj_models::install(entry, &store).map_err(|error| error.to_string())?;
    invalidate();
    Ok(installed_status())
}

/// Удалить величину из хранилища пользователя.
pub fn remove(variant: &str) -> Result<dj_domain::ModelStatus, String> {
    let manifest = parse_manifest(MANIFEST).map_err(|error| error.to_string())?;
    let entry = manifest
        .model
        .iter()
        .find(|entry| entry.variant == variant)
        .ok_or_else(|| format!("в манифесте нет величины {variant}"))?;
    let store = Store::new(Store::default_root());
    store.remove(entry).map_err(|error| error.to_string())?;
    invalidate();
    Ok(installed_status())
}

/// Каким путём считается сетка: для журнала и проверок.
pub fn source() -> &'static str {
    match installed() {
        Some(installed) if with_model(|_| ()).is_some() => match installed.entry.variant.as_str() {
            "full" => "beat model full",
            "small" => "beat model small",
            _ => "beat model",
        },
        _ => "built-in detector",
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
    let guess = with_model(
        |model| match prior(model, &mono, rate, &DbnConfig::default()) {
            Ok(guess) => guess,
            Err(error) => {
                eprintln!("dj-engine: {}: {error}", path.display());
                None
            }
        },
    )
    .flatten();
    let prior = guess.map(|guess| LevelPrior {
        bpm: guess.bpm,
        tolerance: 0.02,
        anchor: Some(guess.anchor),
    });
    dj_analysis::tempo::detect_with_prior(&mono, rate, prior.as_ref())
}
