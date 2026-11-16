//! Что движок говорит об установленной модели: с первой публикации, а не после команды.

use std::path::PathBuf;

fn store_root() -> PathBuf {
    std::env::temp_dir().join(format!("dj-model-status-{}", std::process::id()))
}

#[test]
fn an_installed_model_is_reported_before_any_command() {
    let root = store_root();
    let artifact = root.join("beat_this/small/1.0.0");
    std::fs::create_dir_all(&artifact).unwrap_or_else(|error| panic!("{error}"));
    // Хранилище проверяет размер, а не содержимое: файл нужной длины, без десяти мегабайт на диске.
    let file = std::fs::File::create(artifact.join("model.onnx"))
        .unwrap_or_else(|error| panic!("{error}"));
    file.set_len(10_555_598)
        .unwrap_or_else(|error| panic!("{error}"));
    drop(file);

    // Свидетель самого дефекта: без объявления при старте состояние пусто, с ним — величина есть.
    let cells = dj_engine::StateCells::new();
    assert!(
        cells.model().variant.is_empty(),
        "свежие ячейки обязаны молчать про модель"
    );
    dj_engine::beatmodel::seed_model(&cells, &root);
    assert_eq!(
        cells.model().variant,
        "small",
        "после объявления при старте движок обязан видеть установленную величину"
    );

    let status = dj_engine::beatmodel::installed_status_at(&root);
    assert_eq!(
        status.variant, "small",
        "движок не увидел установленную величину"
    );
    assert_eq!(status.license, "MIT");
    assert!(!status.busy);
    assert!(status.error.is_empty(), "{}", status.error);

    // Обе величины — берётся старшая: full важнее small.
    let full = root.join("beat_this/full/1.0.0");
    std::fs::create_dir_all(&full).unwrap_or_else(|error| panic!("{error}"));
    let file =
        std::fs::File::create(full.join("model.onnx")).unwrap_or_else(|error| panic!("{error}"));
    file.set_len(83_162_656)
        .unwrap_or_else(|error| panic!("{error}"));
    drop(file);
    assert_eq!(
        dj_engine::beatmodel::installed_status_at(&root).variant,
        "full"
    );

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_empty_store_says_nothing_about_a_model() {
    let root = store_root().join("empty");
    let status = dj_engine::beatmodel::installed_status_at(&root);
    assert!(status.variant.is_empty());
    assert!(
        status.is_empty(),
        "пустое хранилище должно давать пустое состояние"
    );
}
