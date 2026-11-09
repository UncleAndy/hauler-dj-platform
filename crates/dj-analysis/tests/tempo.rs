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

/// A kick on every beat, every `bar`-th of them louder: a four-on-the-floor bar with an accent.
fn accented_kick_track(bpm: f64, seconds: f64, bar: usize) -> Vec<f32> {
    let frames = (RATE * seconds) as usize;
    let mut samples = vec![0.0f32; frames];
    let period = RATE * 60.0 / bpm;
    let mut position = 0.0f64;
    let mut beat = 0usize;
    while (position as usize) + 4_800 < frames {
        let level = if beat.is_multiple_of(bar) { 0.95 } else { 0.45 };
        kick(&mut samples, position as usize, level);
        position += period;
        beat += 1;
    }
    samples
}

/// A click every beat, every `bar`-th of them louder: the accent pattern of a four-on-the-floor record.
///
/// The loudest periodicity in the spectrum of this is the bar and the half of it, which is how a house
/// record at 128 used to be measured at 64.
fn accented_click_track(bpm: f64, seconds: f64, bar: usize) -> Vec<f32> {
    let frames = (RATE * seconds) as usize;
    let mut samples = vec![0.0f32; frames];
    let period = RATE * 60.0 / bpm;
    let mut position = 0.0f64;
    let mut beat = 0usize;
    while (position as usize) + 256 < frames {
        let level = if beat.is_multiple_of(bar) { 0.95 } else { 0.35 };
        let start = position as usize;
        for i in 0..192 {
            let decay = 1.0 - i as f32 / 192.0;
            samples[start + i] = level * decay * if i % 2 == 0 { 0.8 } else { -0.8 };
        }
        position += period;
        beat += 1;
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

/// Kick on every beat, a louder low hit four fifths of a beat later on every second beat.
///
/// A bass playing against the beat, and not the same way on every beat: the pattern repeats every bar,
/// which is exactly what lets the kick band count a bar and believe it has found the beat. The beat
/// itself carries the wideband content — a stab and a hat — while the band below 150 Hz is loudest on
/// the syncopation. This is the shape of a track whose grid came out a fifth of a beat off: the low band
/// was allowed to say where the beat is, and in this music the low band is not playing it.
fn syncopated_kick(bpm: f64, offset: usize, seconds: f64) -> Vec<f32> {
    let frames = (RATE * seconds) as usize;
    let mut samples = vec![0.0f32; frames];
    let beat = RATE * 60.0 / bpm;
    let mut index = 0usize;
    while (offset as f64 + index as f64 * beat + beat + 1024.0) < frames as f64 {
        let at = offset + (index as f64 * beat) as usize;
        kick(&mut samples, at, 0.2);
        stab(&mut samples, at, 1.5);
        hat(&mut samples, at, 1.0);
        if index.is_multiple_of(2) {
            let off = at + (beat * 0.8) as usize;
            kick(&mut samples, off, 1.4);
        }
        index += 1;
    }
    samples
}

#[test]
fn a_syncopated_kick_does_not_move_the_grid_off_the_beat() {
    // The beat is where the onsets are: the kick band is the loudest thing in the low end but it plays
    // against the beat, and a grid that follows it lands four fifths of a beat away.
    let samples = syncopated_kick(120.0, 9_000, 40.0);
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
    assert!(
        (tempo.bpm() - 120.0).abs() < 0.5,
        "measured {} BPM instead of 120",
        tempo.bpm()
    );
    let error = off_the_grid(&tempo, 9_000);
    assert!(
        error <= 15.0,
        "the nearest beat is {error:.1} ms away from the first beat"
    );
}

#[test]
fn a_tempo_that_is_not_a_whole_number_of_hops_is_measured_where_it_is() {
    // 100,06 BPM is 112,4 analysis hops per beat at 48 kHz — not a whole number. A tenth of a percent
    // walks a tenth of a beat across two minutes, which is a grid that looks right at the start and is off
    // by a sixteenth at the end. A clean click track is measured right with or without the fine period
    // search (the witness for that is the real track, where the coarse answer was 100,11); this keeps the
    // between-the-hops case from regressing.
    let samples = click_track(100.06, 0, 120.0);
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
    assert!(
        (tempo.bpm() - 100.06).abs() < 0.02,
        "measured {} BPM instead of 100,06",
        tempo.bpm()
    );
}

#[test]
fn a_beat_that_drifts_gets_a_grid_that_follows_it() {
    // Two minutes of clicks that speed up from 124 to 125,6 BPM. One tempo cannot be right for that: a
    // straight line drawn at the best single tempo is off by a tenth of a second at the ends. The grid in a
    // file is a list of segments, so it can say where the beats are — this is DJP-SYNC-002.
    let rate = RATE;
    let samples = creeping_click_beat(&[124.0, 124.8, 125.6], 40.0);
    let tempo = dj_analysis::tempo::detect(&samples, rate).expect("a tempo");
    assert!(
        tempo.segments.len() > 1,
        "a grid of {} segment(s) cannot follow a beat that drifts",
        tempo.segments.len()
    );
    // Where the grid says a beat is, read the way the deck reads it: the segment the frame falls in, and
    // that segment's tempo from its own anchor.
    let beat_at = |frame: f64| -> (f64, f64) {
        let mut chosen = tempo.segments[0];
        for segment in &tempo.segments {
            if (segment.first_beat_frame as f64) <= frame {
                chosen = *segment;
            }
        }
        let period = rate * 60.0 / chosen.bpm;
        (
            chosen.beat as f64 + (frame - chosen.first_beat_frame as f64) / period,
            period,
        )
    };
    // Every click of the fixture, recomputed the way it was laid down.
    let mut section = 0usize;
    let mut section_end = rate * 40.0;
    let mut position = 5_000.0f64;
    let mut checked = 0;
    while (position as usize) + 256 < samples.len() {
        if position >= section_end && section + 1 < 3 {
            section += 1;
            section_end += rate * 40.0;
        }
        let (counted, period) = beat_at(position);
        let off = (counted - counted.round()).abs() * period / rate * 1000.0;
        // The cheap path: a segment per stretch of windows, so the grid is exact inside a stretch and up to
        // a twelfth of a beat off where one stretch hands over to the next — a handover sits on a window
        // boundary, not on the beat the tempo changed. One tempo for this track is 260 ms off at the ends;
        // this is a sixth of that, and the honest ceiling of the cheap path (see GRID_TOLERANCE).
        assert!(
            off <= 40.0,
            "click {checked} at frame {position:.0} ({} BPM section): the nearest beat is {off:.1} ms away",
            [124.0, 124.8, 125.6][section]
        );
        checked += 1;
        position += rate * 60.0 / [124.0, 124.8, 125.6][section];
    }
    assert!(checked > 200, "only {checked} clicks checked");
}

#[test]
fn a_beat_that_holds_its_tempo_is_one_segment_still() {
    // The file this writes has to be the file it wrote before the elastic grid existed: a steady track is
    // one tempo and one anchor, not a list of them.
    let samples = click_track(128.0, 4_800, 40.0);
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
    assert_eq!(
        tempo.segments.len(),
        1,
        "a steady beat came back as {} segments",
        tempo.segments.len()
    );
}

#[test]
fn a_beat_that_begins_after_four_minutes_is_still_found() {
    // A five-minute take whose count starts late: four minutes and ten seconds of quiet, then forty of
    // clicks. The analysis used to stop at four minutes — a guess about how long a track is — so the whole
    // file read as silence and the deck got no grid at all.
    let mut samples = vec![0.0f32; (RATE * 250.0) as usize];
    samples.extend(click_track(124.0, 0, 40.0));
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo after four minutes");
    assert!(
        (tempo.bpm() - 124.0).abs() < 0.5,
        "measured {} BPM instead of 124",
        tempo.bpm()
    );
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

/// A beat that speeds up without a seam: every click is placed one period after the last, and the period
/// is taken from the section the click falls in. This is what a band speeding up sounds like, and it is
/// the case a grid of one tempo cannot describe — the last click is a tenth of a second off the line the
/// first one implies.
fn creeping_click_beat(sections: &[f64], seconds_each: f64) -> Vec<f32> {
    let total = (RATE * seconds_each * sections.len() as f64) as usize;
    let mut samples = vec![0.0f32; total];
    let mut section = 0usize;
    let mut section_end = RATE * seconds_each;
    let mut position = 5_000.0f64;
    while (position as usize) + 256 < total {
        let start = position as usize;
        for i in 0..192 {
            let decay = 1.0 - i as f32 / 192.0;
            samples[start + i] = decay * if i % 2 == 0 { 0.8 } else { -0.8 };
        }
        if position >= section_end && section + 1 < sections.len() {
            section += 1;
            section_end += RATE * seconds_each;
        }
        position += RATE * 60.0 / sections[section];
    }
    samples
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
fn a_bar_accent_does_not_halve_the_tempo() {
    // A click on every beat at 128 with every fourth one loud: the strongest periodicity is the bar, and
    // the answer used to be the half of the beat — which is what four house tracks of the reference set
    // came back as (64, 63, 65,5 and 65 BPM). A DJ counts this at 128.
    for bpm in [120.0f64, 128.0, 131.0] {
        let samples = accented_click_track(bpm, 90.0, 4);
        let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
        assert!(
            (tempo.bpm() - bpm).abs() < 1.0,
            "measured {:.2} BPM instead of {bpm}",
            tempo.bpm()
        );
    }
}

#[test]
fn the_grid_holds_the_click_for_four_minutes() {
    // The reference grids a DJ sets by hand are exact to the sample, and the difference between a comb
    // measured on hops and the beat of the music grows with the track: a hundredth of a hop of period
    // error is a third of a second after five minutes. This one asks for the whole four minutes inside
    // ten milliseconds, on a tempo that is not a whole number of hops.
    let samples = click_track(128.0006, 0, 240.0);
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
    assert!(
        (tempo.bpm() - 128.0006).abs() < 0.002,
        "measured {:.5} BPM instead of 128,0006",
        tempo.bpm()
    );
    let period = RATE * 60.0 / 128.0006;
    let mut position = 0.0f64;
    let mut worst = 0.0f64;
    while position < RATE * 240.0 {
        worst = worst.max(off_the_grid(&tempo, position as u64));
        position += period;
    }
    assert!(
        worst <= 10.0,
        "the grid drifts {worst:.1} ms from the click"
    );
}

#[test]
fn the_bar_starts_on_the_beat_the_accent_is_on() {
    // A kick every beat, every fourth one loud: the bar starts on the loud one, and the grid's first beat
    // has to be one of those — a grid a beat or three beats late draws its bar lines across the music.
    let samples = accented_kick_track(128.0, 120.0, 4);
    let tempo = dj_analysis::tempo::detect(&samples, RATE).expect("a tempo");
    let period = RATE * 60.0 / tempo.bpm();
    let beats = tempo.first_beat_frame() as f64 / period;
    let slot = beats.rem_euclid(4.0);
    assert!(
        !(0.05..=3.95).contains(&slot),
        "the bar starts {slot:.2} beats after the accent"
    );
    assert_eq!(tempo.metre, 4, "the bar was counted in {}", tempo.metre);
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
