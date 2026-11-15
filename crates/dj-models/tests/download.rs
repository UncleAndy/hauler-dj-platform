//! Загрузка: файл пишется только после сверки, иначе его не существует.

#![allow(clippy::expect_used)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread;

use dj_models::{ModelEntry, ModelError, Store, install};

const BODY: &[u8] = b"ONNX-fake-weights-for-the-test";

fn serve_once(body: &'static [u8]) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let address = listener.local_addr().expect("the address");
    let handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body);
            let _ = stream.flush();
        }
    });
    (format!("http://{address}/model.onnx"), handle)
}

fn scratch(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("dj-models-dl-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).expect("a scratch directory");
    base
}

fn entry(url: &str, sha256: String) -> ModelEntry {
    ModelEntry {
        id: "beat_this".to_string(),
        variant: "small".to_string(),
        title: "Beat This! (small)".to_string(),
        version: "1.0.0".to_string(),
        format: "onnx".to_string(),
        bytes: BODY.len() as u64,
        sha256,
        artifact: url.to_string(),
        source: "https://github.com/CPJKU/beat_this".to_string(),
        license: "MIT".to_string(),
        license_url: "https://example.invalid/license".to_string(),
        attribution: "Beat This! — CPJKU".to_string(),
    }
}

fn hash_of(body: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(body);
    format!("{:x}", hasher.finalize())
}

#[test]
fn a_download_with_the_right_hash_lands_in_the_store() {
    let base = scratch("ok");
    let store = Store::new(&base);
    let (url, server) = serve_once(BODY);
    let model = entry(&url, hash_of(BODY));
    let path = install(&model, &store).expect("the artifact is installed");
    server.join().expect("the server thread");
    assert_eq!(std::fs::read(&path).expect("the artifact"), BODY);
    store.verify(&model).expect("the installed file matches");
    assert!(store.is_installed(&model));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_download_with_the_wrong_hash_leaves_nothing_behind() {
    let base = scratch("bad");
    let store = Store::new(&base);
    let (url, server) = serve_once(BODY);
    let model = entry(&url, "22".repeat(32));
    let error = install(&model, &store).expect_err("a different hash must be refused");
    server.join().expect("the server thread");
    assert!(matches!(error, ModelError::Checksum { .. }), "{error}");
    assert!(!store.path(&model).exists(), "the artifact must not appear");
    assert!(
        !store.path(&model).with_extension("onnx.part").exists(),
        "the partial file must be gone"
    );
    let _ = std::fs::remove_dir_all(&base);
}
