//! The scaffold contract of the headless runner: `--help`/`--version` succeed, and a
//! render request fails loudly with exit code 2 instead of pretending to render
//! (docs/agents/AGENTS.md: an unimplemented feature must not look implemented).

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    match Command::new(env!("CARGO_BIN_EXE_dj-headless"))
        .args(args)
        .output()
    {
        Ok(output) => output,
        Err(err) => panic!("failed to spawn dj-headless: {err}"),
    }
}

#[test]
fn version_reports_package_version() {
    let output = run(&["--version"]);
    assert!(output.status.success(), "--version must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with("dj-headless "),
        "--version printed {stdout:?}"
    );
}

#[test]
fn help_lists_the_planned_scenario_flags() {
    let output = run(&["--help"]);
    assert!(output.status.success(), "--help must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--scenario"),
        "help must mention --scenario"
    );
    assert!(
        stdout.contains("not implemented"),
        "help must be honest about the state"
    );
}

#[test]
fn a_render_request_fails_with_exit_code_two() {
    let output = run(&[
        "--scenario",
        "tests/scenarios/two-deck.yaml",
        "--out",
        "out.wav",
    ]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "an unimplemented render must exit 2, not 0"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not implemented"), "stderr was {stderr:?}");
}
