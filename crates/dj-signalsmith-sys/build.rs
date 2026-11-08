//! Compile the vendored C++ DSP helpers into a static library.
//!
//! The build shells out to the system compiler instead of pulling the `cc` crate: the tree already
//! needs a C++ toolchain for the Qt shell, and one fewer build dependency is one fewer thing to
//! vendor. `CXX` wins when set, then `clang++` (what the dev shell provides), then `c++`.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let vendor = PathBuf::from("vendor");
    println!("cargo:rerun-if-changed=src/wrapper.cpp");
    println!("cargo:rerun-if-changed=vendor/signalsmith-dsp/envelopes.h");
    println!("cargo:rerun-if-changed=vendor/signalsmith-dsp/common.h");

    let compiler = std::env::var("CXX")
        .ok()
        .or_else(|| {
            ["clang++", "c++", "g++"]
                .into_iter()
                .find(|candidate| {
                    Command::new(candidate)
                        .arg("--version")
                        .output()
                        .is_ok_and(|out| out.status.success())
                })
                .map(str::to_string)
        })
        .expect("no C++ compiler found: set CXX, or put clang++ on PATH");

    let object = out_dir.join("wrapper.o");
    let status = Command::new(&compiler)
        .args(["-std=c++11", "-O2", "-fPIC", "-fvisibility=hidden", "-c"])
        .arg("src/wrapper.cpp")
        .arg("-I")
        .arg(&vendor)
        .arg("-o")
        .arg(&object)
        .status()
        .unwrap_or_else(|err| panic!("cannot run {compiler}: {err}"));
    assert!(status.success(), "{compiler} failed on src/wrapper.cpp");

    let archive = out_dir.join("libdj_signalsmith.a");
    let status = Command::new("ar")
        .arg("crs")
        .arg(&archive)
        .arg(&object)
        .status()
        .expect("cannot run ar");
    assert!(status.success(), "ar failed on {}", archive.display());

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=dj_signalsmith");
    // The C++ runtime: libstdc++ is what both g++ and clang++ use here.
    println!("cargo:rustc-link-lib=dylib=stdc++");
}
