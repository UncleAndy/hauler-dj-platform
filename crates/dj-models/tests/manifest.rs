//! Манифест, выбор величины и проверка установленного файла.

#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use dj_models::{ModelEntry, ModelError, Store, parse_manifest, prefer, sha256_of};

fn manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models/model.toml")
}

fn entry(variant: &str) -> ModelEntry {
    ModelEntry {
        id: "beat_this".to_string(),
        variant: variant.to_string(),
        title: format!("Beat This! ({variant})"),
        version: "1.0.0".to_string(),
        format: "onnx".to_string(),
        bytes: 4,
        sha256: "00".repeat(32),
        artifact: "https://example.invalid/model.onnx".to_string(),
        source: "https://github.com/CPJKU/beat_this".to_string(),
        license: "MIT".to_string(),
        license_url: "https://example.invalid/license".to_string(),
        attribution: "Beat This! — CPJKU".to_string(),
    }
}

fn scratch(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("dj-models-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).expect("a scratch directory");
    base
}

#[test]
fn the_repository_manifest_is_valid_and_has_both_sizes() {
    let text = fs::read_to_string(manifest_path()).expect("models/model.toml");
    let manifest = parse_manifest(&text).expect("a valid manifest");
    assert_eq!(manifest.model.len(), 2, "две величины одной модели");
    let variants: Vec<&str> = manifest.model.iter().map(|m| m.variant.as_str()).collect();
    assert!(variants.contains(&"full") && variants.contains(&"small"));
    for model in &manifest.model {
        assert_eq!(model.license, "MIT");
        assert_eq!(model.format, "onnx");
        assert!(model.bytes > 1_000_000, "вес модели — мегабайты, не байты");
        assert_eq!(model.sha256.len(), 64);
    }
}

#[test]
fn a_manifest_without_models_is_refused() {
    let error = parse_manifest("version = 1\n").expect_err("ни одной записи — не манифест");
    assert!(matches!(error, ModelError::Manifest(_)), "{error}");
}

#[test]
fn a_manifest_with_a_broken_hash_is_refused() {
    let text = fs::read_to_string(manifest_path()).expect("models/model.toml");
    let broken = text.replace(
        "1e92d69463843b0cf553fd4e062b1e3e67c95731433ebe23c88350e28787b5bf",
        "zz92d69463843b0cf553fd4e062b1e3e67c95731433ebe23c88350e28787b5bf",
    );
    let error = parse_manifest(&broken).expect_err("не hex — не хэш");
    assert!(matches!(error, ModelError::Manifest(_)), "{error}");
}

#[test]
fn the_full_variant_wins_when_both_are_installed() {
    let small = entry("small");
    let full = entry("full");
    assert_eq!(
        prefer([&small, &full]).map(|m| m.variant.as_str()),
        Some("full")
    );
    assert_eq!(
        prefer([&full, &small]).map(|m| m.variant.as_str()),
        Some("full")
    );
    assert_eq!(prefer([&small]).map(|m| m.variant.as_str()), Some("small"));
    assert!(prefer([]).is_none(), "нет модели — классический детектор");
}

#[test]
fn the_store_lives_under_the_data_home_and_keeps_versions_apart() {
    let store = Store::new("/tmp/models-root");
    let model = entry("full");
    assert_eq!(
        store.dir(&model),
        PathBuf::from("/tmp/models-root/beat_this/full/1.0.0")
    );
    assert_eq!(
        store.path(&model),
        PathBuf::from("/tmp/models-root/beat_this/full/1.0.0/model.onnx")
    );
}

#[test]
fn a_verified_file_passes_and_a_wrong_size_fails() {
    let base = scratch("verify");
    let store = Store::new(&base);
    let mut model = entry("small");
    model.bytes = 4;
    let path = store.path(&model);
    fs::create_dir_all(path.parent().expect("a directory")).expect("the model directory");
    fs::write(&path, b"beat").expect("the artifact");
    model.sha256 = sha256_of(&path).expect("a hash");
    store.verify(&model).expect("the file matches the manifest");

    fs::write(&path, b"beats").expect("a longer artifact");
    let error = store
        .verify(&model)
        .expect_err("size must be checked first");
    assert!(matches!(error, ModelError::Size { .. }), "{error}");

    fs::write(&path, b"beat").expect("the artifact back");
    model.sha256 = "11".repeat(32);
    let error = store
        .verify(&model)
        .expect_err("a different hash must be refused");
    assert!(matches!(error, ModelError::Checksum { .. }), "{error}");

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn removing_a_model_that_is_not_there_is_not_an_error() {
    let base = scratch("remove");
    let store = Store::new(&base);
    store
        .remove(&entry("full"))
        .expect("nothing to remove is fine");
    let _ = fs::remove_dir_all(&base);
}
