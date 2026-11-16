//! Библиотека ONNX Runtime рядом с бинарником движка.
//!
//! Модель удара (ADR-0012) грузится динамически: `ort` с `load-dynamic` берёт `libonnxruntime.so` из
//! `ORT_DYLIB_PATH` или из каталога исполняемого файла. Переменная окружения не переживает ни ярлык,
//! ни перезапуск сеанса, поэтому копия кладётся туда, где её ищет движок: в каталог сборки
//! (`<target>/<profile>/`), откуда cargo кладёт рядом и сам бинарник.
//!
//! Путь к библиотеке приходит из dev shell (`DJ_ORT_LIBRARY`). Нет переменной или файла — сборка
//! идёт как обычно, и движок просто остаётся без модели: она необязательна.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=DJ_ORT_LIBRARY");
    let Ok(library) = std::env::var("DJ_ORT_LIBRARY") else {
        return;
    };
    let source = PathBuf::from(&library);
    if !source.is_file() {
        println!(
            "cargo:warning=DJ_ORT_LIBRARY={library} does not exist: the engine will run without the beat model"
        );
        return;
    }
    let Ok(out) = std::env::var("OUT_DIR") else {
        return;
    };
    // `<target>/<profile>/build/<пакет>-<хэш>/out` → `<target>/<profile>`
    let Some(profile) = Path::new(&out).ancestors().nth(3) else {
        return;
    };
    let Some(name) = source.file_name() else {
        return;
    };
    let destination = profile.join(name);
    // Прошлая копия пришла из хранилища nix и только для чтения: `fs::copy` поверх неё падает на
    // правах, поэтому старое имя убирается. Неудача удаления — тоже повод попробовать копию.
    let _ = std::fs::remove_file(&destination);
    if std::fs::copy(&source, &destination).is_err() {
        println!(
            "cargo:warning=could not place {} next to the engine",
            name.to_string_lossy()
        );
    }
}
