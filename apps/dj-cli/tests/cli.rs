//! Scaffold contract of the collection CLI: help/version succeed, unimplemented
//! commands fail with exit code 2 rather than silently doing nothing.

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    match Command::new(env!("CARGO_BIN_EXE_dj-cli"))
        .args(args)
        .output()
    {
        Ok(output) => output,
        Err(err) => panic!("failed to spawn dj-cli: {err}"),
    }
}

#[test]
fn version_reports_package_version() {
    let output = run(&["--version"]);
    assert!(output.status.success(), "--version must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with("dj-cli "),
        "--version printed {stdout:?}"
    );
}

#[test]
fn help_exits_zero() {
    let output = run(&["--help"]);
    assert!(output.status.success(), "--help must exit 0");
}

#[test]
fn import_is_not_wired_yet_and_says_so() {
    let output = run(&["import", "/tmp"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "unimplemented command must exit 2"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not implemented"), "stderr was {stderr:?}");
}
