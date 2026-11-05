//! Two decks rendered together, headlessly (A03 acceptance).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    match Command::new(env!("CARGO_BIN_EXE_dj-headless"))
        .args(args)
        .output()
    {
        Ok(output) => output,
        Err(err) => panic!("spawn: {err}"),
    }
}

fn write_tone(path: &Path, rate: u32, seconds: f64, frequency: f32) {
    let frames = (f64::from(rate) * seconds) as u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + frames * 4).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(frames * 4).to_le_bytes());
    for frame in 0..frames {
        let value = 0.5 * (std::f32::consts::TAU * frequency * frame as f32 / rate as f32).sin();
        let sample = (value * 32_000.0) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap_or_else(|err| panic!("write: {err}"));
}

fn wav_frames(path: &Path) -> usize {
    let bytes = std::fs::read(path).unwrap_or_else(|err| panic!("read: {err}"));
    assert_eq!(&bytes[0..4], b"RIFF");
    let data = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]) as usize;
    assert_eq!(data + 44, bytes.len(), "header must describe the payload");
    data / 4
}

fn build_scenario(dir: &Path) -> (PathBuf, PathBuf) {
    let a = dir.join("a.wav");
    let b = dir.join("b.wav");
    write_tone(&a, 44_100, 2.0, 440.0);
    write_tone(&b, 44_100, 2.0, 660.0);

    let scenario = dir.join("mix.json");
    let text = format!(
        r#"{{
  "sample_rate": 44100,
  "frames": 88200,
  "block_frames": 256,
  "decks": [
    {{ "id": "A", "file": "{}", "gain": 1.0 }},
    {{ "id": "B", "file": "{}", "gain": 0.5 }}
  ],
  "commands": [
    {{ "at": 0, "deck": "A", "action": "Play" }},
    {{ "at": 44100, "deck": "B", "action": "Play" }},
    {{ "at": 66150, "deck": "A", "action": "LoopOn", "start": 70000, "end": 71000 }},
    {{ "at": 70000, "deck": "B", "action": "Seek", "frame": 22050 }}
  ]
}}"#,
        a.display(),
        b.display()
    );
    std::fs::write(&scenario, text).unwrap_or_else(|err| panic!("write scenario: {err}"));
    (scenario, dir.join("mix.wav"))
}

#[test]
fn two_decks_render_together_with_commands_on_their_samples() {
    let dir = std::env::temp_dir().join(format!("dj-mix-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("mkdir: {err}"));
    let (scenario, out) = build_scenario(&dir);

    let output = run(&[
        "mix",
        "--file",
        scenario.to_str().unwrap_or_default(),
        "--out",
        out.to_str().unwrap_or_default(),
    ]);
    assert!(
        output.status.success(),
        "mix failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(wav_frames(&out), 88_200, "exact frame count");

    let report_text = String::from_utf8_lossy(&output.stdout).to_string();
    let report: serde_json::Value =
        serde_json::from_str(&report_text).unwrap_or_else(|err| panic!("bad report: {err}"));
    assert_eq!(
        report["applied"], 4,
        "every command must be applied: {report_text}"
    );
    let decks = report["decks"]
        .as_array()
        .unwrap_or_else(|| panic!("no decks in {report_text}"));
    let deck_a = decks
        .iter()
        .find(|deck| deck["deck"] == "A")
        .unwrap_or_else(|| panic!("deck A missing"));
    let deck_b = decks
        .iter()
        .find(|deck| deck["deck"] == "B")
        .unwrap_or_else(|| panic!("deck B missing"));
    assert!(
        deck_a["loop_wraps"].as_u64().unwrap_or(0) > 0,
        "the loop must have run at least one lap: {report_text}"
    );
    assert_eq!(deck_b["loop_wraps"].as_u64(), Some(0), "deck B has no loop");
    assert!(deck_a["position"].as_u64().unwrap_or(0) > 0);

    let bytes = std::fs::read(&out).unwrap_or_else(|err| panic!("read: {err}"));
    let samples: Vec<i16> = bytes[44..]
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let first_half = &samples[..44_100 * 2];
    let second_half = &samples[44_100 * 2..];
    assert!(
        first_half.iter().any(|sample| *sample != 0),
        "deck A must sound from the first sample"
    );
    let peak_first = first_half
        .iter()
        .map(|s| s.unsigned_abs())
        .max()
        .unwrap_or(0);
    let peak_second = second_half
        .iter()
        .map(|s| s.unsigned_abs())
        .max()
        .unwrap_or(0);
    assert!(
        peak_second > peak_first,
        "deck B joins at mid-track, so the second half must be louder ({peak_first} -> {peak_second})"
    );
}

#[test]
fn the_same_mix_renders_identical_bytes() {
    let dir = std::env::temp_dir().join(format!("dj-mix-det-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("mkdir: {err}"));
    let (scenario, _) = build_scenario(&dir);

    let mut renders = Vec::new();
    for name in ["one.wav", "two.wav"] {
        let out = dir.join(name);
        let output = run(&[
            "mix",
            "--file",
            scenario.to_str().unwrap_or_default(),
            "--out",
            out.to_str().unwrap_or_default(),
        ]);
        assert!(output.status.success(), "mix failed");
        renders.push(std::fs::read(&out).unwrap_or_default());
    }
    assert!(!renders[0].is_empty());
    assert_eq!(renders[0], renders[1], "a mix must be reproducible");
}

#[test]
fn a_rate_mismatch_is_refused_instead_of_silently_sped_up() {
    let dir = std::env::temp_dir().join(format!("dj-mix-rate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("mkdir: {err}"));
    let a = dir.join("48k.wav");
    write_tone(&a, 48_000, 1.0, 440.0);
    let scenario = dir.join("bad.json");
    std::fs::write(
        &scenario,
        format!(
            r#"{{ "sample_rate": 44100, "frames": 1000, "decks": [{{ "id": "A", "file": "{}" }}] }}"#,
            a.display()
        ),
    )
    .unwrap_or_else(|err| panic!("write: {err}"));

    let output = run(&["mix", "--file", scenario.to_str().unwrap_or_default()]);
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("resampling is A04"),
        "the message must say why: {stderr}"
    );
}
