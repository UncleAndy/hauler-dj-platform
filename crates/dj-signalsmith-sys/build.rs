//! Compile the vendored C++ DSP helpers into a static library.
//!
//! The build shells out to the system compiler instead of pulling the `cc` crate: the tree already
//! needs a C++ toolchain for the Qt shell, and one fewer build dependency is one fewer thing to
//! vendor. `CXX` wins when set, then `clang++` (what the dev shell provides), then `c++`.

use std::path::PathBuf;
use std::process::Command;

/// The compiler for the platform being built, not for the one doing the building.
///
/// Cargo sets `CXX_<target>` for a cross build (`x86_64_pc_windows_gnu`, say) and leaves `CXX` for the host.
/// Reading only `CXX` puts host objects into a library for another platform, and all the linker can say about
/// that is that the symbols are missing.
fn target_var(name: &str) -> Option<String> {
    let target = std::env::var("TARGET").unwrap_or_default();
    let underscored = target.replace('-', "_");
    std::env::var(format!("{name}_{underscored}"))
        .ok()
        .or_else(|| std::env::var(name).ok())
}

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let vendor = PathBuf::from("vendor");
    println!("cargo:rerun-if-changed=src/wrapper.cpp");
    println!("cargo:rerun-if-changed=vendor/signalsmith-dsp/envelopes.h");
    println!("cargo:rerun-if-changed=vendor/signalsmith-dsp/delay.h");
    println!("cargo:rerun-if-changed=vendor/signalsmith-dsp/filters.h");
    println!("cargo:rerun-if-changed=vendor/signalsmith-dsp/fft.h");
    println!("cargo:rerun-if-changed=vendor/signalsmith-dsp/common.h");
    println!(
        "cargo:rerun-if-changed=vendor/signalsmith-basics/include/signalsmith-basics/limiter.h"
    );
    println!("cargo:rerun-if-changed=vendor/signalsmith-basics/include/stfx/stfx-library.h");

    let compiler = target_var("CXX")
        .filter(|value| !value.is_empty())
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
        // C++17, not the C++11 the headers ask for: the effect framework stores `std::atomic<bool>`
        // in its base class, and with the C++11/14 rules an implicit copy of that base is ill-formed.
        // C++17's guaranteed copy elision removes the copies, and the headers compile clean.
        .args(["-std=c++17", "-O2", "-fPIC", "-fvisibility=hidden", "-c"])
        // Where the platform keeps the thread part of its standard library is the platform's business.
        .args(target_var("CXXFLAGS").unwrap_or_default().split_whitespace())
        .arg("src/wrapper.cpp")
        .arg("-I")
        .arg(&vendor)
        .arg("-o")
        .arg(&object)
        .status()
        .unwrap_or_else(|err| panic!("cannot run {compiler}: {err}"));
    assert!(status.success(), "{compiler} failed on src/wrapper.cpp");

    let archive = out_dir.join("libdj_signalsmith.a");
    let archiver = target_var("AR").unwrap_or_else(|| "ar".to_string());
    let status = Command::new(&archiver)
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
