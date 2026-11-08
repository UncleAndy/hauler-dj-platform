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
    // The bar is what the measurements support, not what would be nice: the comb's period refinement
    // brought the worst case from 27 ms down to 4 ms over this range, so 15 ms leaves room for a
    // different machine without hiding a regression.
    for bpm in [72.0f64, 90.0, 100.0, 120.0, 128.0, 140.0, 174.0] {
        for offset in [0usize, 1_234, 4_800, 11_111] {
            let samples = click_track(bpm, offset, 20.0);
            let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
            let error = off_the_grid(&tempo, offset as u64);
            assert!(
                error <= 15.0,
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

/// A track that opens with something that has no beat at all: sparse, uneven hits for `intro` seconds.
fn rubato_intro(seconds: f64) -> Vec<f32> {
    let frames = (RATE * seconds) as usize;
    let mut samples = vec![0.0f32; frames];
    // Deliberately uneven, and with no short cycle: a linear congruential sequence, so no window can
    // find a period the intro does not have.
    let mut position = 1_000usize;
    let mut state = 12_345u64;
    while position + 256 < frames {
        for i in 0..192 {
            let decay = 1.0 - i as f32 / 192.0;
            samples[position + i] = decay * 0.5 * if i % 2 == 0 { 1.0 } else { -1.0 };
        }
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let step = 6_000 + (state >> 33) as usize % 12_000;
        position += step;
    }
    samples
}

#[test]
fn an_intro_without_a_beat_does_not_hide_the_tempo() {
    // The report that started this test: a track whose intro cannot give a count was left without a
    // tempo, because one look at the whole file saw the intro and not the beat. Windows fix that — the
    // intro gets a vote, not the deciding one.
    let mut samples = rubato_intro(30.0);
    let music = click_track(128.0, 0, 60.0);
    let music_start = samples.len();
    samples.extend(music);
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo behind the intro");
    assert!(
        (tempo.bpm - 128.0).abs() < 0.5,
        "measured {} BPM instead of 128",
        tempo.bpm
    );
    // And the grid still lands on the clicks, not on the intro.
    let period = RATE * 60.0 / tempo.bpm;
    let position = (music_start as f64 - tempo.first_beat_frame as f64) / period;
    let error = (position - position.round()).abs() * period / RATE * 1000.0;
    assert!(error <= 30.0, "the grid is {error:.1} ms off the clicks");
}

#[test]
fn a_track_with_no_beat_anywhere_is_still_refused() {
    // Three minutes of rubato: the detector must keep saying "no", not pick the strongest of the noise.
    let samples = rubato_intro(180.0);
    assert!(dj_analysis::tempo::detect(&samples, RATE).is_none());
}

/// A track built on triplets: a beat click every 450 ms, and two quieter clicks between them, which is
/// what put a 400 BPM peak in the spectrum of a 133 BPM track.
fn triplet_track(bpm: f64, offset: usize, seconds: f64) -> Vec<f32> {
    let frames = (RATE * seconds) as usize;
    let mut samples = vec![0.0f32; frames];
    let beat = RATE * 60.0 / bpm;
    let mut position = offset as f64;
    while (position as usize) + 256 < frames {
        // The beat itself: loud.
        let start = position as usize;
        for i in 0..192 {
            let decay = 1.0 - i as f32 / 192.0;
            samples[start + i] = decay * 0.8 * if i % 2 == 0 { 1.0 } else { -1.0 };
        }
        // Two triplet subdivisions: quieter, so the beat is still the loudest thing on the grid.
        for third in 1..3 {
            let at = (position + beat * f64::from(third) / 3.0) as usize;
            if at + 128 < frames {
                for i in 0..96 {
                    let decay = 1.0 - i as f32 / 96.0;
                    samples[at + i] = decay * 0.25 * if i % 2 == 0 { 1.0 } else { -1.0 };
                }
            }
        }
        position += beat;
    }
    samples
}

#[test]
fn the_beat_is_not_mistaken_for_its_triplets() {
    // Measured on a real track: "Shadow Rhythm — Deep Resonance — Treblo" runs at 133,3 BPM with a
    // triplet arpeggio whose spectrum peaks at 400 BPM (three times the tempo). A detector that takes
    // the lowest strong peak, or the fastest grid that hits every onset, lands on a multiple; the beat
    // is the grid the *loud* hits land on.
    for bpm in [133.3f64, 100.0] {
        let samples = triplet_track(bpm, 4_000, 30.0);
        let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
        assert!(
            (tempo.bpm - bpm).abs() < 1.0,
            "measured {} BPM instead of {bpm}",
            tempo.bpm
        );
        let error = off_the_grid(&tempo, 4_000);
        assert!(error <= 20.0, "the grid is {error:.1} ms off the beat");
    }
}
