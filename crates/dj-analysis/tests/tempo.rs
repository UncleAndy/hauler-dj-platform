//! What the detector must get right, on signals whose tempo and offset are known (B02).
//!
//! Every number in the module's constants was chosen against these signals; if a change makes one of
//! them fail, the change is what is wrong, or the constant needs a new measurement — not a wider
//! tolerance.

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

const RATE: f64 = 48_000.0;

/// A click track: a click every beat, the first one `offset` frames in.
fn click_track(bpm: f64, offset: usize, seconds: f64) -> Vec<f32> {
    let frames = (RATE * seconds) as usize;
    let mut samples = vec![0.0f32; frames];
    let period = RATE * 60.0 / bpm;
    let mut position = offset as f64;
    while (position as usize) + 256 < frames {
        // 4 ms of decaying alternating samples: an onset with a broad spectrum, no tone to confuse the
        // envelope with a melody.
        let start = position as usize;
        for i in 0..192 {
            let decay = 1.0 - i as f32 / 192.0;
            samples[start + i] = decay * if i % 2 == 0 { 0.8 } else { -0.8 };
        }
        position += period;
    }
    samples
}

#[test]
fn a_click_track_at_128_bpm_is_measured() {
    let samples = click_track(128.0, 4_800, 20.0);
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
    assert!(
        (tempo.bpm - 128.0).abs() < 0.5,
        "measured {} BPM instead of 128",
        tempo.bpm
    );
}

/// Distance from `frame` to the nearest beat of the grid, in milliseconds.
fn off_the_grid(tempo: &dj_analysis::tempo::Tempo, frame: u64) -> f64 {
    let period = RATE * 60.0 / tempo.bpm;
    let position = (frame as f64 - tempo.first_beat_frame as f64) / period;
    let nearest = position.round();
    (position - nearest).abs() * period / RATE * 1000.0
}

#[test]
fn the_grid_lands_on_the_clicks() {
    // The grid is periodic, so the property is not "the first beat equals the offset" but "some beat
    // falls on every click". Offsets that are not round milliseconds are the interesting ones: the grid
    // has to follow the audio, not the analysis block.
    // The bar is what the measurements support, not what would be nice: 1 … 27 ms over this range, so
    // anything up to 30 ms is within what the detector promises. A slower tempo is worse, which is why
    // the editor has a manual shift (DJP-SYNC-003).
    for bpm in [72.0f64, 90.0, 100.0, 120.0, 128.0, 140.0, 174.0] {
        for offset in [0usize, 1_234, 4_800, 11_111] {
            let samples = click_track(bpm, offset, 20.0);
            let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
            let error = off_the_grid(&tempo, offset as u64);
            assert!(
                error <= 30.0,
                "{bpm} BPM, clicks at {offset}: the nearest beat is {error:.1} ms away"
            );
        }
    }
}

#[test]
fn a_silent_intro_does_not_move_the_grid() {
    // Two seconds of silence, then 128 BPM. The grid runs through the silence — that is what a beat
    // grid does — and what must hold is that it lands on the clicks once the music starts.
    let mut samples = vec![0.0f32; 96_000];
    samples.extend(click_track(128.0, 0, 18.0));
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
    let error = off_the_grid(&tempo, 96_000);
    assert!(
        error <= 15.0,
        "the grid is {error:.1} ms off the first click after the silence"
    );
}

#[test]
fn an_unusual_tempo_is_measured_too() {
    let samples = click_track(174.0, 2_000, 20.0);
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
    assert!(
        (tempo.bpm - 174.0).abs() < 0.5,
        "measured {} BPM instead of 174",
        tempo.bpm
    );
}

#[test]
fn a_half_time_pattern_reports_the_tempo_or_its_half() {
    // A kick on beats one and three is a real pattern, and no measurement can tell it from a track
    // played at half speed: the UI's half/double control exists for exactly this. What the detector
    // must not do is invent a third number.
    let samples = click_track(64.0, 3_000, 24.0);
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
    let matches = [64.0, 128.0];
    assert!(
        matches
            .iter()
            .any(|wanted| (tempo.bpm - wanted).abs() < 0.5),
        "measured {} BPM, neither 64 nor 128",
        tempo.bpm
    );
}

#[test]
fn silence_has_no_tempo() {
    let samples = vec![0.0f32; 48_000 * 4];
    assert!(dj_analysis::tempo::detect(&samples, RATE).is_none());
}

#[test]
fn a_tone_has_no_tempo() {
    // A held note has onsets once, at the start; a beat needs a pattern.
    let samples: Vec<f32> = (0..48_000 * 6)
        .map(|index| {
            let phase = 2.0 * std::f64::consts::PI * 440.0 * f64::from(index) / RATE;
            (phase.sin() * 0.4) as f32
        })
        .collect();
    assert!(dj_analysis::tempo::detect(&samples, RATE).is_none());
}
