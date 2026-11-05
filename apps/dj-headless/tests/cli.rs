//! End-to-end checks for the headless runner: exit codes, the WAV the engine produced, and
//! the exact sample at which a command took effect (A01 acceptance).

use std::path::{Path, PathBuf};
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

fn scenario_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/scenarios/play-stop.json")
        .to_path_buf()
}

/// Frames in a mono 16-bit PCM WAV, read from its own header.
fn wav_frames(path: &Path) -> usize {
    let bytes = std::fs::read(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    assert_eq!(&bytes[0..4], b"RIFF", "not a RIFF file");
    assert_eq!(&bytes[8..12], b"WAVE", "not a WAVE file");
    assert_eq!(
        &bytes[36..40],
        b"data",
        "expected the data chunk at offset 36"
    );
    let data = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]) as usize;
    assert_eq!(
        data + 44,
        bytes.len(),
        "the header must describe the payload"
    );
    data / 2
}

fn wav_samples(path: &Path) -> Vec<i16> {
    let bytes = std::fs::read(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    bytes[44..]
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect()
}

fn temp_wav(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("dj-headless-{}-{name}.wav", std::process::id()))
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
fn help_documents_both_modes_and_the_exit_codes() {
    let output = run(&["--help"]);
    assert!(output.status.success(), "--help must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for needle in ["sine", "scenario", "--frames", "Exit codes"] {
        assert!(stdout.contains(needle), "help must mention {needle}");
    }
}

#[test]
fn sine_renders_exactly_the_requested_number_of_frames() {
    let out = temp_wav("sine");
    let output = run(&[
        "sine",
        "--frames",
        "4800",
        "--out",
        out.to_str().unwrap_or_default(),
    ]);
    assert!(
        output.status.success(),
        "sine must exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        wav_frames(&out),
        4800,
        "the WAV must hold exactly 4800 frames"
    );

    let samples = wav_samples(&out);
    let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    assert!(
        peak > 30_000,
        "a full-scale tone was expected, peak was {peak}"
    );

    let report = String::from_utf8_lossy(&output.stdout);
    assert!(report.contains("\"frames\":4800"), "report was {report}");
}

#[test]
fn a_scenario_applies_play_and_stop_at_the_named_samples() {
    let out = temp_wav("scenario");
    let path = scenario_path();
    let output = run(&[
        "scenario",
        "--file",
        path.to_str().unwrap_or_default(),
        "--out",
        out.to_str().unwrap_or_default(),
    ]);
    assert!(
        output.status.success(),
        "scenario must exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(wav_frames(&out), 4800);

    let samples = wav_samples(&out);
    assert!(
        samples[..1000].iter().all(|s| *s == 0),
        "nothing may sound before the first command"
    );
    assert!(
        samples[1000..4000].iter().any(|s| *s != 0),
        "the transport is on between Play and Stop"
    );
    assert!(
        samples[4000..].iter().all(|s| *s == 0),
        "Stop must silence the deck exactly at its sample"
    );

    let report = String::from_utf8_lossy(&output.stdout);
    assert!(
        report.contains("\"applied_at_sample\":1000")
            && report.contains("\"applied_at_sample\":4000"),
        "the report must name both application samples; got {report}"
    );
}

#[test]
fn two_renders_of_one_scenario_are_byte_identical() {
    let first = temp_wav("det-a");
    let second = temp_wav("det-b");
    let path = scenario_path();
    for out in [&first, &second] {
        let output = run(&[
            "scenario",
            "--file",
            path.to_str().unwrap_or_default(),
            "--out",
            out.to_str().unwrap_or_default(),
        ]);
        assert!(output.status.success(), "render failed");
    }
    let a = std::fs::read(&first).unwrap_or_default();
    let b = std::fs::read(&second).unwrap_or_default();
    assert!(!a.is_empty(), "the render produced no bytes");
    assert_eq!(a, b, "the same scenario must render identical bytes");
}

#[test]
fn bad_usage_exits_with_two() {
    for args in [
        vec!["--frames"],
        vec!["sine", "--frames", "not-a-number", "--out", "x.wav"],
        vec!["sine", "--frames", "10"],
        vec!["scenario"],
        vec!["dance"],
    ] {
        let output = run(&args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?} should be a usage error; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn a_missing_scenario_file_exits_with_three() {
    let output = run(&["scenario", "--file", "/nonexistent/scenario.json"]);
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot read"), "stderr was {stderr:?}");
}
