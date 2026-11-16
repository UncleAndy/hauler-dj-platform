//! Доли внешней модели для одного файла: для разбора сетки на проблемном треке.
//!
//! Запуск: `ORT_DYLIB_PATH=… cargo run -p dj-engine --release --example model_beats -- <файл>`
//! Печатает по строке на долю: номер и секунды.

use std::path::Path;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: model_beats <file>");
        return;
    };
    let (mono, rate) = match dj_analysis::tempo::decode_mono(Path::new(&path)) {
        Ok(decoded) => decoded,
        Err(error) => {
            eprintln!("decode: {error}");
            return;
        }
    };
    let library = match std::env::var_os("ORT_DYLIB_PATH") {
        Some(value) => std::path::PathBuf::from(value),
        None => {
            eprintln!("ORT_DYLIB_PATH is not set");
            return;
        }
    };
    let model_path = match dj_engine::beatmodel::installed_artifact() {
        Some(path) => path,
        None => {
            eprintln!("no beat model installed");
            return;
        }
    };
    let mut model = match dj_beatmodel::BeatModel::load(&library, &model_path) {
        Ok(model) => model,
        Err(error) => {
            eprintln!("model: {error}");
            return;
        }
    };
    match dj_beatmodel::beats(&mut model, &mono, rate, &dj_beatmodel::DbnConfig::default()) {
        Ok(beats) => {
            for (index, beat) in beats.iter().enumerate() {
                println!("{index}\t{beat:.4}");
            }
        }
        Err(error) => eprintln!("beats: {error}"),
    }
}
