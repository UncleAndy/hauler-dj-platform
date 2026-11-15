//! Проверка загрузчика на настоящем релизе: манифест → нужная величина → скачивание → сверка.
//!
//! Запуск: `cargo run -p dj-models --example fetch -- models/model.toml small /tmp/models-check`

fn main() {
    let mut arguments = std::env::args().skip(1);
    let (Some(manifest_path), Some(variant), Some(root)) =
        (arguments.next(), arguments.next(), arguments.next())
    else {
        eprintln!("usage: fetch <manifest.toml> <variant> <store-root>");
        return;
    };
    let text = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("манифест {manifest_path}: {error}");
            return;
        }
    };
    let manifest = match dj_models::parse_manifest(&text) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!("{error}");
            return;
        }
    };
    let Some(entry) = manifest.model.iter().find(|model| model.variant == variant) else {
        eprintln!("в манифесте нет величины {variant}");
        return;
    };
    println!(
        "{} {} → {} ({} байт, {})",
        entry.title, entry.version, entry.artifact, entry.bytes, entry.license
    );
    let store = dj_models::Store::new(&root);
    match dj_models::install(entry, &store) {
        Ok(path) => {
            println!("установлено: {}", path.display());
            match store.verify(entry) {
                Ok(()) => println!("сверено: sha256 совпал"),
                Err(error) => eprintln!("сверка: {error}"),
            }
        }
        Err(error) => eprintln!("загрузка: {error}"),
    }
}
