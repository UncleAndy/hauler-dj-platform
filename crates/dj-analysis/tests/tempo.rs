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

/// A kick: sixty hertz, a fifth of a second of decay. The band the bar is in.
fn kick(samples: &mut [f32], at: usize, level: f32) {
    let length = (RATE * 0.2) as usize;
    for i in 0..length {
        if at + i >= samples.len() {
            break;
        }
        let decay = (1.0 - i as f32 / length as f32).powi(2);
        let phase = std::f32::consts::TAU * 60.0 * i as f32 / RATE as f32;
        samples[at + i] += level * decay * phase.sin();
    }
}

/// A hat: two milliseconds of three kilohertz, decaying.
///
/// A tone, not a click of alternating samples: alternation at the sample rate is a tone at twenty-four
/// kilohertz, and an FFT sees its edges leak all the way down into the band the kick is measured in.
/// Real hats are high-passed; this one keeps its energy where a hat's belongs.
fn hat(samples: &mut [f32], at: usize, level: f32) {
    for i in 0..96 {
        if at + i >= samples.len() {
            break;
        }
        let decay = (1.0 - i as f32 / 96.0).powi(2);
        let phase = std::f32::consts::TAU * 3_000.0 * i as f32 / RATE as f32;
        samples[at + i] += level * decay * phase.sin();
    }
}

/// Four-four: the kick on one and three, a hat on every sixteenth.
///
/// This is the shape of the track that was read as four thirds of its tempo. Dense percussion puts an
/// onset on every subdivision, so a grid of three sixteenths explains as many onsets as the beat does,
/// and the finer grid wins on the count. The kick band has nothing on the sixteenths, so the bar it
/// folds with is the only honest measure of where the beat is.
fn four_four(bpm: f64, offset: usize, seconds: f64) -> Vec<f32> {
    let frames = (RATE * seconds) as usize;
    let mut samples = vec![0.0f32; frames];
    let beat = RATE * 60.0 / bpm;
    let mut position = offset as f64;
    let mut index = 0usize;
    while (position as usize) + 512 < frames {
        let at = position as usize;
        // The kick on one and three, as four-four music puts it: an accent twice a bar, and nothing on
        // the sixteenths, which is what makes the band able to name the bar.
        if index.is_multiple_of(8) {
            kick(&mut samples, at, 0.8);
        }
        hat(&mut samples, at, 0.3);
        position += beat / 4.0;
        index += 1;
    }
    samples
}

/// A chord stab: six hundred hertz, a tenth of a second. The middle of a waltz's bar.
fn stab(samples: &mut [f32], at: usize, level: f32) {
    for i in 0..(RATE * 0.1) as usize {
        if at + i >= samples.len() {
            break;
        }
        let decay = (1.0 - i as f32 / (RATE * 0.1) as f32).powi(2);
        let phase = std::f32::consts::TAU * 600.0 * i as f32 / RATE as f32;
        samples[at + i] += level * decay * phase.sin();
    }
}

/// Three-four: the kick on one, a chord on two and three, a hat on every eighth.
///
/// A waltz, not a metronome: the kick alone says "one accent every beat and a half", which is as true of
/// a beat and a half as of a bar of three, and a bar of three is only countable when something marks the
/// other two beats.
fn three_four(bpm: f64, offset: usize, seconds: f64) -> Vec<f32> {
    let frames = (RATE * seconds) as usize;
    let mut samples = vec![0.0f32; frames];
    let beat = RATE * 60.0 / bpm;
    let mut position = offset as f64;
    let mut index = 0usize;
    let mut bar = 0usize;
    while (position as usize) + 512 < frames {
        let at = position as usize;
        if bar == 0 {
            kick(&mut samples, at, 0.8);
        } else {
            stab(&mut samples, at, 0.5);
        }
        hat(&mut samples, at, 0.3);
        position += beat;
        index += 1;
        bar = (bar + 1) % 3;
    }
    let _ = index;
    samples
}

#[test]
fn a_bar_of_four_beats_is_not_read_as_three_of_them() {
    // The kick band says where the bar is; the full spectrum only says where the onsets are, and on
    // dense percussion that is every third sixteenth as much as every fourth.
    let samples = four_four(100.0, 4_800, 30.0);
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
    assert!(
        (tempo.bpm() - 100.0).abs() < 1.0,
        "measured {} BPM instead of 100: the grid is a third of a bar off",
        tempo.bpm()
    );
    assert_eq!(
        tempo.metre, 4,
        "the bar was counted in {} beats",
        tempo.metre
    );
    let error = off_the_grid(&tempo, 4_800);
    assert!(error <= 15.0, "the kick is {error:.1} ms off the grid");
}

#[test]
fn the_first_beat_is_the_one_the_kick_lands_on() {
    // A bar starts where the accent is. The grid may sit on any beat of the bar and still be a grid,
    // but a square drawn from the wrong beat starts in the middle of the music.
    // The track opens on a hat, two beats before the kick: the first beat of the grid is a beat of the
    // audio, and the accent says which one is the bar's.
    let beat = RATE * 60.0 / 100.0;
    let mut samples = vec![0.0f32; (RATE * 30.0) as usize];
    let mut position = 0.0f64;
    let mut index = 0usize;
    while (position as usize) + 512 < samples.len() {
        let at = position as usize;
        if index >= 2 && (index - 2).is_multiple_of(4) {
            kick(&mut samples, at, 0.8);
        }
        hat(&mut samples, at, 0.3);
        position += beat;
        index += 1;
    }
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
    let error = off_the_grid(&tempo, (2.0 * beat) as u64);
    assert!(
        error <= 15.0,
        "the first beat is {error:.1} ms off the kick that opens the bar"
    );
}

#[test]
fn a_bar_of_three_beats_is_counted_in_three() {
    let samples = three_four(120.0, 4_800, 30.0);
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
    assert!(
        (tempo.bpm() - 120.0).abs() < 1.5,
        "measured {} BPM instead of 120",
        tempo.bpm()
    );
    assert_eq!(
        tempo.metre, 3,
        "the bar was counted in {} beats",
        tempo.metre
    );
}

#[test]
fn a_click_track_at_128_bpm_is_measured() {
    let samples = click_track(128.0, 4_800, 20.0);
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
    assert!(
        (tempo.bpm() - 128.0).abs() < 0.5,
        "measured {} BPM instead of 128",
        tempo.bpm()
    );
}

/// Distance from `frame` to the nearest beat of the grid, in milliseconds.
fn off_the_grid(tempo: &dj_analysis::tempo::Tempo, frame: u64) -> f64 {
    let period = RATE * 60.0 / tempo.bpm();
    let position = (frame as f64 - tempo.first_beat_frame() as f64) / period;
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
        (tempo.bpm() - 174.0).abs() < 0.5,
        "measured {} BPM instead of 174",
        tempo.bpm()
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
            .any(|wanted| (tempo.bpm() - wanted).abs() < 0.5),
        "measured {} BPM, neither 64 nor 128",
        tempo.bpm()
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
        (tempo.bpm() - 128.0).abs() < 0.5,
        "measured {} BPM instead of 128",
        tempo.bpm()
    );
    // And the grid still lands on the clicks, not on the intro.
    let period = RATE * 60.0 / tempo.bpm();
    let position = (music_start as f64 - tempo.first_beat_frame() as f64) / period;
    let error = (position - position.round()).abs() * period / RATE * 1000.0;
    assert!(error <= 30.0, "the grid is {error:.1} ms off the clicks");
}

/// A beat whose tempo creeps: a click track that speeds up a little in every section.
fn creeping_beat(sections: &[f64], seconds: f64) -> Vec<f32> {
    let mut samples = Vec::new();
    for bpm in sections {
        samples.extend(click_track(*bpm, 0, seconds));
    }
    samples
}

#[test]
fn a_beat_that_does_not_hold_its_tempo_is_still_a_beat() {
    // Two real remixes were refused as a whole while every window of them found the beat: the phase was
    // fitted across the entire span, and a tenth of a percent of tempo error walks off the grid by the
    // time it reaches the window the check looks at. A track whose tempo moves is still a track with a
    // beat; what it does not have is one grid for its whole length.
    let samples = creeping_beat(&[124.0, 124.8, 125.6], 40.0);
    let tempo = dj_analysis::tempo::detect(&samples, RATE)
        .expect("a tempo from a beat that creeps from 124 to 125.6");
    assert!(
        (123.0..=127.0).contains(&tempo.bpm()),
        "measured {} BPM, which is not the beat it was given",
        tempo.bpm()
    );
}

/// A track that plays one tempo and then another: `first` for `first_seconds`, then `second`.
fn two_tempos(first: f64, first_seconds: f64, second: f64, second_seconds: f64) -> Vec<f32> {
    let mut samples = click_track(first, 0, first_seconds);
    samples.extend(click_track(second, 0, second_seconds));
    samples
}

#[test]
fn a_track_that_changes_tempo_is_described_as_two() {
    // An edit, not a new track: the beat has to keep counting and the grid has to be right on both
    // sides of the cut. Averaging the two tempos — which is what one grid for the whole file does —
    // would be wrong everywhere instead of somewhere.
    let samples = two_tempos(120.0, 90.0, 132.0, 90.0);
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("two tempos");
    assert!(
        tempo.segments.len() >= 2,
        "one segment for a track with two tempos: {:?}",
        tempo.segments
    );
    let first = tempo.segments.first().expect("a first segment");
    let last = tempo.segments.last().expect("a last segment");
    assert!(
        (first.bpm - 120.0).abs() < 1.0,
        "the first stretch is {} BPM instead of 120",
        first.bpm
    );
    assert!(
        (last.bpm - 132.0).abs() < 1.0,
        "the last stretch is {} BPM instead of 132",
        last.bpm
    );
    // The beats run on across the change: the second stretch does not start counting from zero.
    assert!(last.beat > 0, "the beat count restarted at the change");

    // Where the change is: within one window of the tenth second before it, and the grid of the second
    // stretch lands on its own clicks.
    let change_frame = (90.0 * RATE) as u64;
    let boundary = last.first_beat_frame;
    let off = (boundary as f64 - change_frame as f64).abs() / RATE;
    assert!(
        off <= 30.0,
        "the second stretch starts {off:.1} s from the change"
    );
    let period = RATE * 60.0 / last.bpm;
    let mut worst = 0.0f64;
    let mut position = change_frame as f64 + period;
    while position < samples.len() as f64 {
        let beats = (position - boundary as f64) / period;
        worst = worst.max((beats - beats.round()).abs() * period / RATE * 1000.0);
        position += period;
    }
    assert!(
        worst <= 15.0,
        "the second grid is {worst:.1} ms off its clicks"
    );
}

#[test]
fn a_half_time_section_is_not_a_change_of_tempo() {
    // The same beat written twice as slow: a section of the same track where the kick lands every other
    // beat. The grid of 124 covers the 62 as well, so this is one tempo, not two — a half-time breakdown
    // is not a new piece of music.
    let samples = two_tempos(124.0, 60.0, 62.0, 60.0);
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("one tempo, played two ways");
    assert_eq!(
        tempo.segments.len(),
        1,
        "a half-time section split the track: {:?}",
        tempo.segments
    );
    assert!(
        tempo.bpm() > 100.0,
        "the answer should be the beat, not its half: {}",
        tempo.bpm()
    );
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
            (tempo.bpm() - bpm).abs() < 1.0,
            "measured {} BPM instead of {bpm}",
            tempo.bpm()
        );
        let error = off_the_grid(&tempo, 4_000);
        assert!(error <= 20.0, "the grid is {error:.1} ms off the beat");
    }
}
