//! Tempo and the first beat of the grid (B02, DJP-MEDIA-006, DJP-SYNC-003).
//!
//! The detector is deliberately classical and has no model behind it: a spectral flux envelope, the
//! spectrum of that envelope (whose lowest strong peak is the beat rate), and a comb over the phase to
//! place the grid. Every number below was chosen by measurement on click tracks with a known tempo and
//! a known offset — see `tests/tempo.rs` and `docs/qa/evidence/A15/summary.md`.
//!
//! What it does not do: half/double correction by ear (that is a control in the UI, DJP-SYNC-003),
//! tempo maps (DJP-SYNC-004), or any decision about which beat is the downbeat.

use dj_codec::{MediaError, StreamDecoder};
use dj_signalsmith_sys::Spectrum;

/// Analysis block, in samples: 1024 at 48 kHz is 21 ms — long enough to see a kick, short enough that
/// two sixteenths never merge into one onset.
pub const BLOCK: usize = 1024;

/// Hop between blocks: 256 samples is 5,3 ms at 48 kHz, so a beat lands within a hundredth of a beat.
pub const HOP: usize = 256;

/// Tempo range the search covers, in beats per minute.
pub const MIN_BPM: f64 = 60.0;
/// Upper end of the search: above this the search starts finding eighths, not beats.
pub const MAX_BPM: f64 = 200.0;

/// A peak below this share of the strongest is a ripple, not a candidate tempo.
const PEAK_SHARE: f32 = 0.15;

/// Half-width, in hops, of the window around a grid point that counts as "on the beat". The flux of one
/// onset is a few hops wide, and a grid that is a fraction of a hop out is not wrong; a window this wide
/// measures whether the beat is there, not whether the arithmetic is perfect.
const SHARE_SPREAD: f64 = 1.5;

/// How many beats of the grid must carry a real onset: a beat that happens twice is not a tempo.
const MIN_BEATS: usize = 8;

/// Share of the onset energy that must sit on the grid. Measured: 0,93 for a click track, 0,39 for a
/// drum loop, **0,16 for a real electronic track with triplet arpeggios** (most of its energy is off the
/// beat), and 0,02 … 0,04 for an aperiodic line. 0,08 keeps a factor of two on both sides: a gate at
/// 0,15 sat within one percent of that real track and would have refused the next one.
const MIN_EXPLAINED_SHARE: f64 = 0.08;

/// Where most music a DJ plays sits, and how far the octave choice may be pulled towards it.
///
/// The measure cannot tell a tempo from its double: a hit every second makes a grid of one second and a
/// grid of half a second both land on hits. The score already prefers the grid that is actually played,
/// which settles most tracks, but the finer grid always has at least as many empty points as the coarser
/// one, so near a tie the coarser reading wins by a hair. This prior decides only that hair — it is flat
/// across 90 … 150 and falls to about 0.57 at the edges of the range, so a real half-time track, whose
/// fine grid is half empty, still beats the reading the prior would prefer.
const PREFERRED_BPM: f64 = 120.0;
const PRIOR_SPREAD_OCTAVES: f64 = 0.9;

/// Length of one window the tempo is looked for in, in seconds.
pub const WINDOW_SECONDS: f64 = 40.0;

/// Distance between those windows. They do not overlap: a tempo is either in a window or it is not, and
/// the point of having several is that an intro without a beat gets outvoted by the rest of the track.
pub const WINDOW_HOP_SECONDS: f64 = 40.0;

/// Length of the window a tempo *change* is looked for in, in seconds.
///
/// Shorter than the window the tempo itself is chosen in: a change is a local question ("is the beat
/// here the same as there?"), and twenty seconds hold forty beats at 120 BPM — enough to answer it,
/// where forty seconds would smear the change across the whole look-back.
pub const CHANGE_WINDOW_SECONDS: f64 = 20.0;

/// Distance between those windows, in seconds. It is also how precisely a change is placed: the beat
/// inside a stretch is right to the millisecond, the frame where the stretch starts to the hop.
pub const CHANGE_HOP_SECONDS: f64 = 10.0;

/// How many windows in a row a new tempo has to hold before it counts as a change.
///
/// One window is not a change: a bar of silence, a fill, a spoken line all read as a tempo of their own.
/// Three windows — thirty seconds — is a section.
const CHANGE_WINDOWS: usize = 3;

/// How much better the other tempo has to explain a section than the track's own does.
///
/// A section is not "another tempo" because some window said so: the triplets of an arpeggio read as a
/// careful-sounding tempo of their own in a short window, and the track's own grid still explains them
/// better than that answer does. Only a stretch the track's tempo cannot explain is a change.
const CHANGE_MARGIN: f64 = 1.15;

/// How far a window's answer may sit from another and still be the same tempo, as a fraction of it.
const TEMPO_TOLERANCE: f64 = 0.03;

/// How close two windows' tempos have to be to be the same *grid* (DJP-SYNC-002, the cheap path).
///
/// [`TEMPO_TOLERANCE`] asks "is this the same tempo a listener would name", and three percent is right for
/// that: a track does not change tempo because a window was measured a little differently. A grid asks a
/// sharper question — where is the beat two minutes from now — and a tenth of a percent per section is
/// already a tenth of a beat by the end. So the drift pass reads each window with the comb and splits the track at four tenths of a percent.
///
/// TODO(DJP-SYNC-002): this is the cheap path to a grid that follows a drifting tempo — the segments are
/// the existing stretch machinery with a finer merge, kept only when the whole grid explains the music
/// better than one tempo does. It cannot follow a drift that has no steps (a window's own tempo estimate
/// is biased by its edges: measured 128,58 BPM on a steady 128 BPM click track). If a grid that follows a
/// continuous drift is wanted, it needs a local tempo estimate of its own — see the skill's
/// `beatgrid-and-sync.md`, "Эластичная сетка": three designs and why each failed.
const DRIFT_TOLERANCE: f64 = 0.004;

/// How far from a whole octave a tempo may sit and still be the same beat read twice.
const OCTAVE_TOLERANCE: f64 = 0.03;

/// How much of a stretch is used to place its first beat, in seconds.
///
/// The phase is measured at the start of the stretch on purpose: a grid that is right there is what the
/// DJ needs at the moment the music changes, and measuring it over the whole stretch would average the
/// drift of everything that follows into the one beat that matters.
const PHASE_HEAD_SECONDS: f64 = 20.0;

/// One stretch of a track at one tempo, as the analysis found it.
///
/// The same shape as `dj_domain::BeatSegment` on purpose — the engine turns these into a grid — but this
/// crate stays free of the protocol: an analyser is useful without a wire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoSegment {
    /// Beats per minute in this stretch, as measured (not corrected to a half or double).
    pub bpm: f64,
    /// Frame of the stretch's first beat.
    pub first_beat_frame: u64,
    /// Beat number of that frame, counted from the track's first beat.
    pub beat: u64,
}

/// What the analysis found: the grid, as one segment per tempo.
#[derive(Debug, Clone, PartialEq)]
pub struct Tempo {
    /// The grid in order. Never empty; the first segment carries beat zero.
    pub segments: Vec<TempoSegment>,
    /// How sure the measurement is, 0 … 1: the share of the onset energy the weakest segment's grid
    /// explains. The weakest, not the average: a track is only as measurable as its least measurable part.
    pub confidence: f32,
    /// Beats in a bar: three or four, as the kick band counted them, or [`DEFAULT_METRE`] when the band
    /// has no accent to count by. A bar is a pattern of accents; the full spectrum throws them away, so
    /// a track whose accents are not in the kick band is counted in four and left at that.
    pub metre: u8,
}

impl Tempo {
    /// The tempo of the first stretch: what a track list shows without knowing where the play head is.
    pub fn bpm(&self) -> f64 {
        self.segments.first().map_or(0.0, |segment| segment.bpm)
    }

    /// Frame of the grid's first beat.
    pub fn first_beat_frame(&self) -> u64 {
        self.segments
            .first()
            .map_or(0, |segment| segment.first_beat_frame)
    }

    /// Whether the track holds one tempo throughout.
    pub fn is_single(&self) -> bool {
        self.segments.len() == 1
    }
}

/// Spectral flux: the positive part of the change in magnitude between two blocks.
///
/// Level rises equally with every instrument; flux rises with *onsets*, which is what a beat is. The
/// envelope is one value per hop, in the frame scale of the file (`n · HOP`).
pub fn onset_envelope(mono: &[f32]) -> Option<Vec<f32>> {
    let mut spectrum = Spectrum::new(BLOCK)?;
    let bins = BLOCK / 2;
    let mut magnitudes = vec![0.0f32; bins];
    let mut previous = vec![0.0f32; bins];
    let mut block = vec![0.0f32; BLOCK];
    let mut flux = Vec::with_capacity(mono.len() / HOP + 1);

    let mut start = 0;
    while start + BLOCK <= mono.len() {
        block.copy_from_slice(&mono[start..start + BLOCK]);
        spectrum.magnitudes(&block, &mut magnitudes);
        let mut sum = 0.0f32;
        for (bin, before) in magnitudes.iter().zip(previous.iter()) {
            sum += (bin - before).max(0.0);
        }
        flux.push(sum);
        previous.copy_from_slice(&magnitudes);
        start += HOP;
    }
    // A handful of blocks cannot carry a tempo; refusing beats returning a number that means nothing.
    if flux.len() < 64 {
        return None;
    }
    // A slowly moving mean is subtracted so a loud section does not outweigh a quiet one: the tempo is
    // in the pattern, not in the level.
    Some(remove_the_running_mean(flux, 94))
}

/// Subtract a centred moving average of `window` envelope points, and clamp the rest at zero.
///
/// Centred, not trailing: a trailing mean delays the whole envelope by half its window, and the phase
/// of the first beat is read straight off this envelope.
fn remove_the_running_mean(flux: Vec<f32>, window: usize) -> Vec<f32> {
    let half = (window / 2).max(1);
    let mut out = Vec::with_capacity(flux.len());
    for index in 0..flux.len() {
        let from = index.saturating_sub(half);
        let to = (index + half + 1).min(flux.len());
        let mean: f64 = flux[from..to]
            .iter()
            .map(|value| f64::from(*value))
            .sum::<f64>()
            / (to - from) as f64;
        out.push((f64::from(flux[index]) - mean).max(0.0) as f32);
    }
    out
}

/// Detect the tempo and the first beat of a mono signal.
///
/// Two bands, two questions. The full spectrum says where the onsets are, which is the tempo and
/// nothing else: on dense percussion it cannot tell a beat from a group of three sixteenths, because
/// both grids land on onsets. The band below [`KICK_HZ`] says where the bar is, and a bar is what
/// separates a beat from its subdivisions and tells which beat of it is the first.
pub fn detect(mono: &[f32], rate: f64) -> Option<Tempo> {
    let envelope = onset_envelope(mono)?;
    let mut tempo = detect_in_envelope(&envelope, rate)?;
    let kick = kick_envelope(mono, rate);
    if let Some(band) = kick.as_deref() {
        on_the_bar(&envelope, band, rate, &mut tempo);
    }
    // The last word belongs to the onsets the grid lands on: the level and the bar are choices, the
    // period and the anchor are measurements, and a measurement is taken where the music is.
    let per_second = rate / HOP as f64;
    snap_to_onsets(&mut tempo, &envelope, per_second);
    // Which side of the beat the grid ended up on. A full-spectrum envelope likes the hats, and the hats
    // are the offbeat: on `Leash Broken` the settled grid sat exactly half a beat from the owner's, on the
    // offbeat eighths. The kick band is what says where the beat is, so the side is decided on it.
    on_the_beat_side(&mut tempo, kick.as_deref().unwrap_or(&envelope), per_second);
    // The bar last of all: which beat of it is the first is a question about the accent, and the accent
    // has to be counted on the beats the music actually plays. Measured before the period was settled, it
    // was counted on a lattice a hair out of place — and the shift was then written over by the settle.
    if let Some(band) = kick.as_deref() {
        align_the_bar(&mut tempo, band, per_second);
        // And the reading itself: four thirds of the music's is a tempo the onsets support as well as the
        // truth, and only the bar's accent tells them apart.
        settle_four_the_third(&mut tempo, &envelope, band, per_second);
    }
    Some(tempo)
}

/// The same, for an envelope someone already has: the analysis decodes the file once and works on it.
///
/// Two questions, answered apart. The tempo is chosen first, over windows of forty seconds, exactly as
/// it has always been — that answer is what a track that holds its tempo gets, and it stays byte for
/// byte what it was. Then the track is asked whether it holds that tempo all the way through: if some
/// stretch of it answers a different beat for a section, the grid is built from stretches, and a track
/// that changes tempo is described as one instead of being averaged into a tempo it never plays.
pub fn detect_in_envelope(envelope: &[f32], rate: f64) -> Option<Tempo> {
    let per_second = rate / HOP as f64;
    let (bpm, clear) = choose_tempo(envelope, per_second)?;
    // The windows answer a periodicity; which beat it is, is a second question — see [`decide_level`].
    let bpm = decide_level(envelope, per_second, bpm);
    let rigid = single_tempo(envelope, per_second, bpm, &clear);
    // A track that changes tempo outright: the stretches are told apart with the tolerance a listener
    // uses, and the first one that holds wins, as it always has.
    let stretches = stretches_of(
        envelope,
        per_second,
        bpm,
        TEMPO_TOLERANCE,
        CHANGE_WINDOWS,
        false,
    );
    if stretches.len() > 1
        && let Some(changing) = grid_of_stretches(envelope, per_second, bpm, &stretches, true)
    {
        return carries_the_beat(envelope, per_second, changing);
    }
    // A track that drifts: the same machinery with a tenth of a percent of tolerance, kept only when the
    // whole grid explains the onsets better than one tempo does (DJP-SYNC-002, the cheap path — see
    // [`GRID_TOLERANCE`] for what it cannot do).
    if let Some(rigid) = &rigid {
        let fine = stretches_of(envelope, per_second, bpm, DRIFT_TOLERANCE, 2, true);
        if fine.len() > 1
            && let Some(drifting) = grid_of_stretches(envelope, per_second, bpm, &fine, false)
        {
            let drift_score = grid_onset(
                envelope,
                &drifting.segments,
                per_second,
                envelope.len() as f64,
            );
            let rigid_score =
                grid_onset(envelope, &rigid.segments, per_second, envelope.len() as f64);
            // A drift goes one way: a track that speeds up does not slow down two stretches later. Windows
            // of ordinary music differ by a fraction of a percent at random, and a grid cut up by that
            // noise is worse than one tempo — so the grid is taken only when its tempo walks in one
            // direction, and only when it explains the music by the margin.
            // A drift has to be worth describing: a track that moves a tenth of a percent over six minutes
            // is one tempo, and cutting it into eight stretches buys nothing but anchors in the file.
            let walked = match (drifting.segments.first(), drifting.segments.last()) {
                (Some(first), Some(last)) => (last.bpm / first.bpm - 1.0).abs(),
                _ => 0.0,
            };
            if drifting.segments.len() >= 3
                && walked >= MIN_DRIFT
                && drifts_one_way(&drifting.segments)
                && same_level(&drifting.segments, rigid.bpm())
                && drift_score > rigid_score * ELASTIC_MARGIN
            {
                return carries_the_beat(envelope, per_second, drifting);
            }
        }
    }
    rigid.and_then(|tempo| carries_the_beat(envelope, per_second, tempo))
}

/// Whether the grid's beats carry the music or sit between its hits.
///
/// A share of the onsets is not enough on its own: a rubato line, a pad and a wash of noise put something
/// near every point of any grid fine enough, and the share of a fast grid rises until it passes the floor.
/// What a beat has that those points do not is weight — the onsets on the beats are stronger than the ones
/// halfway between them, because the music puts its hits there. Measured on the reference set, the weakest
/// track of the seven came out at 1,28 and the strongest at 2,68; three minutes of rubato, which the detector
/// must keep refusing, comes out at 1,00 to 1,05.
const BEAT_CONTRAST_FLOOR: f64 = 1.2;

fn carries_the_beat(envelope: &[f32], per_second: f64, tempo: Tempo) -> Option<Tempo> {
    let mut ratios = Vec::new();
    for segment in &tempo.segments {
        let period = per_second * 60.0 / segment.bpm;
        if !(period.is_finite() && period >= 2.0) {
            continue;
        }
        // The same phase the level was chosen on: the grid's own anchor is a beat of that lattice, but
        // the comb's phase is the one the measure was taken at, and two answers to one question would
        // have the gate refusing grids the level decision accepted.
        let (phase, _) = best_phase_and_score(envelope, period);
        let (on, halfway) = beat_against_halfway(envelope, period, phase);
        if on <= 0.0 {
            continue;
        }
        // A grid with nothing between its beats — a click track, a bare kick — has no halfway energy to
        // compare with: that is the strongest contrast there is, not a missing measurement.
        ratios.push(if halfway > 0.0 { on / halfway } else { 3.0 });
    }
    if ratios.is_empty() {
        return None;
    }
    let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
    (mean >= BEAT_CONTRAST_FLOOR).then_some(tempo)
}

/// The grid of a track that holds one tempo: the old answer, unchanged.
/// The grid of a track that holds one tempo: the answer, or the answer one level away when the first
/// cannot be a grid at all.
///
/// The windows answer whichever periodicity they heard clearest, and on an acoustic take that is the
/// strumming, not the beat: `Среди своих (v3 - soft rock accustic)` answered 60,7 BPM and refused the
/// grid (it carried 6,5 % of the onsets, under the 8 % floor), while the onsets of the whole file sit on
/// 138,5. So when the answer cannot be published, the levels around it are asked — and only then: a track
/// whose answer holds is not moved, because on the tracks where the rough answer is right (a remix whose
/// eighth-note grid carries more onsets than its beat) the finer level would win on the count alone.
fn single_tempo(
    envelope: &[f32],
    per_second: f64,
    bpm: f64,
    clear: &std::ops::Range<usize>,
) -> Option<Tempo> {
    if let Some(tempo) = grid_at_level(envelope, per_second, bpm, clear) {
        return Some(tempo);
    }
    let level = refine_level(envelope, per_second, bpm);
    if (level / bpm - 1.0).abs() <= 0.02 {
        return None;
    }
    grid_at_level(envelope, per_second, level, clear)
}

/// The grid of a track that holds one tempo, at the tempo it was given.
fn grid_at_level(
    envelope: &[f32],
    per_second: f64,
    bpm: f64,
    clear: &std::ops::Range<usize>,
) -> Option<Tempo> {
    // The spectrum is coarse by construction: it says "128-ish". The comb over the whole analysed span
    // says "this period and no other", which is what keeps a five-minute track from drifting away from
    // its own grid by a beat.
    let hint = per_second * 60.0 / bpm;
    let mut period = refine_period(envelope, hint);
    if period <= 0.0 {
        return None;
    }
    period = refined_period(envelope, period);
    // How much of the onset energy actually sits on this grid: a real beat explains nearly all of it,
    // while a rubato line or a sparse noise explains a fraction. This is the confidence that matters —
    // the spectrum can be sharp about a tempo nothing is playing to.
    // Measured where the tempo is actually clear, not over the whole file: an intro that explains
    // nothing would otherwise veto a grid the rest of the track makes perfectly well.
    //
    // The phase is measured there too, and not fitted over the whole span. A grid that is right in one
    // place is wrong in another: a tenth of a percent of tempo error walks a quarter of a beat across
    // four minutes, and a phase fitted over all of it lands between the beats of the very window the
    // gate is judging — which is how a track whose every window found the tempo was refused as a whole.
    let mut inside = best_phase(&envelope[clear.clone()], period);
    let (mut explained, mut beats) = explained_share(&envelope[clear.clone()], period, inside);
    // A long baseline is the more precise estimate, and the right one for a track that holds its tempo.
    // When the grid does not survive the very window it is checked in, the track does not hold one: the
    // answer is then the tempo of the part that plays, measured there and published as that.
    if explained < MIN_EXPLAINED_SHARE {
        let local = refine_period(&envelope[clear.clone()], hint);
        if local > 0.0 {
            let local_inside = best_phase(&envelope[clear.clone()], local);
            let (local_explained, local_beats) =
                explained_share(&envelope[clear.clone()], local, local_inside);
            if local_explained > explained {
                period = local;
                inside = local_inside;
                explained = local_explained;
                beats = local_beats;
            }
        }
    }
    // A grid has to be played, not just hit once: a held note explains its own beginning and nothing
    // else, and a tempo needs a run of beats to be a tempo.
    if explained < MIN_EXPLAINED_SHARE || beats < MIN_BEATS {
        return None;
    }
    // The phase comes from the comb, not from the beat tracker. The tracker places every beat where the
    // music wants it, which on a track whose period is not exactly the grid's means its beats walk a whole
    // period over the file (measured: phases 0,25 … 103 hops on `Shadow Rhythm — Deep Resonance`); a grid
    // in `.djmeta` is rigid — one tempo per segment — so the best rigid grid is the comb's, and the tracker
    // is kept for the level choice and the confidence, where its local view is the right one.
    let bpm = per_second * 60.0 / period;
    // Half a block: the block that saw the onset starts at `phase · HOP`, and flux rises as soon as its
    // 21 ms window touches the beat, so the block's centre is the better estimate of where the beat is.
    // Measured on click tracks: 1 … 27 ms late over 72 … 174 BPM, against 5 … 38 ms early without it.
    // The phase is counted from the window the grid was checked in, so the anchor is a beat of the audio
    // and not of a straight line drawn through four minutes of it; it is then carried back to the first
    // beat of the track, which is where a grid is read from.
    let anchor = (clear.start as f64 + inside).rem_euclid(period);
    let first_beat_frame = (anchor * HOP as f64 + BLOCK as f64 / 2.0).round().max(0.0) as u64;
    Some(Tempo {
        segments: vec![TempoSegment {
            bpm,
            first_beat_frame,
            beat: 0,
        }],
        confidence: explained as f32,
        metre: DEFAULT_METRE,
    })
}

/// A stretch of the envelope whose windows agree on one tempo.
#[derive(Debug, Clone, Copy)]
struct Stretch {
    /// Tempo the windows of this stretch agreed on, snapped to the octave of the track's own.
    bpm: f64,
    /// First envelope frame.
    start: usize,
    /// One past the last envelope frame.
    end: usize,
    /// How many windows it was seen in.
    windows: usize,
}

/// Fold a tempo onto the track's own when it is that tempo read an octave away, and leave it otherwise.
///
/// A window can answer the beat, its double or its half: the score prefers the grid that is actually
/// played, but over twenty seconds a section of half-time feel can read either way. Snapping to the
/// track's own octave keeps one track from looking like three, while a real change of tempo — 124 to
/// 128, a ratio no power of two explains — stays a change.
fn snap_octave(bpm: f64, reference: f64) -> f64 {
    if bpm <= 0.0 || reference <= 0.0 {
        return bpm;
    }
    let octaves = (bpm / reference).log2();
    // On an octave of the track's own reading, the window answered the same beat: the value the stretch
    // carries is the track's own, so a half-time section does not look like a tempo of its own.
    if (octaves - octaves.round()).abs() <= OCTAVE_TOLERANCE {
        reference
    } else {
        bpm
    }
}

/// The stretches of the envelope that hold one tempo, in order.
///
/// The windows here are short on purpose and the answer is coarse on purpose: this pass is not asked
/// what the tempo is — the forty-second windows answered that — only whether the answer stays the same
/// from one part of the track to the next. A reading shorter than [`CHANGE_WINDOWS`] windows is not a
/// section, it is a fill or a line of speech, and it is folded back into its neighbour.
fn stretches_of(
    envelope: &[f32],
    per_second: f64,
    dominant: f64,
    tolerance: f64,
    min_windows: usize,
    fine: bool,
) -> Vec<Stretch> {
    let window = (CHANGE_WINDOW_SECONDS * per_second) as usize;
    let hop = (CHANGE_HOP_SECONDS * per_second).max(1.0) as usize;
    if window < 64 || envelope.len() < window * (CHANGE_WINDOWS + 1) {
        return Vec::new();
    }
    let mut stretches: Vec<Stretch> = Vec::new();
    let mut start = 0;
    while start + window <= envelope.len() {
        // Two readings are possible: the spectrum's peaks, which name a tempo the way a listener would
        // and are coarse by construction, and the comb's own period, which is fine enough to see a drift
        // of a tenth of a percent but needs a hint to stay on the same level.
        let span = &envelope[start..start + window];
        let reading = if fine {
            let period = local_period(span, per_second * 60.0 / dominant);
            (period > 0.0).then(|| per_second * 60.0 / period)
        } else {
            tempo_of_window(span, per_second).map(|bpm| snap_octave(bpm, dominant))
        };
        match (reading, stretches.last_mut()) {
            (Some(bpm), Some(last))
                if bpm > 0.0 && last.bpm > 0.0 && ((bpm / last.bpm).log2()).abs() <= tolerance =>
            {
                last.end = start + window;
                last.windows += 1;
            }
            (Some(bpm), _) => stretches.push(Stretch {
                bpm,
                start,
                end: start + window,
                windows: 1,
            }),
            // A window with no answer neither extends a stretch nor starts one: silence inside a
            // section is not the end of it, and it is certainly not a new tempo.
            (None, _) => {}
        }
        start += hop;
    }
    stretches.retain(|stretch| stretch.windows >= min_windows);
    // Whatever was dropped belongs to a neighbour: the spans are laid end to end, so the grid covers
    // the track instead of leaving stretches of it without one.
    for index in 0..stretches.len() {
        if index > 0 {
            // A window that straddles a change is counted with the stretch it ends in, so the previous
            // stretch's end is up to one window past the change. When the windows are read finely, the
            // change is put back by one hop — the distance the straddling window was reading across.
            let back = if fine { hop } else { 0 };
            stretches[index].start = stretches[index - 1].end.saturating_sub(back);
        }
    }
    if let Some(last) = stretches.last_mut() {
        last.end = envelope.len();
    }
    stretches
}

/// The grid of a track that changes tempo: one segment per stretch, measured where it plays.
fn grid_of_stretches(
    envelope: &[f32],
    per_second: f64,
    dominant: f64,
    stretches: &[Stretch],
    strict: bool,
) -> Option<Tempo> {
    let dominant_period = refine_period(envelope, per_second * 60.0 / dominant);
    let mut segments: Vec<TempoSegment> = Vec::new();
    let mut confidence = 1.0f64;
    for stretch in stretches {
        let span = &envelope[stretch.start..stretch.end];
        let hint = per_second * 60.0 / stretch.bpm;
        let period = refine_period(span, hint);
        if period <= 0.0 {
            continue;
        }
        let head_end =
            (stretch.start + (PHASE_HEAD_SECONDS * per_second) as usize).min(stretch.end);
        let inside = best_phase(&envelope[stretch.start..head_end], period);
        let (explained, beats) = explained_share(span, period, inside);
        // A stretch that cannot show its beat is not a tempo of its own: publishing it would put a grid
        // on music nobody can hear a beat in, and the neighbour that does hold one is already right.
        if explained < MIN_EXPLAINED_SHARE || beats < MIN_BEATS {
            continue;
        }
        // Nor is a stretch a change just because a short window said so: the track's own tempo has to
        // fail on it first. This is what keeps the triplets of an arpeggio from being read as a second
        // tempo — the beat still lands on them better than their own answer does.
        if strict && !segments.is_empty() && dominant_period > 0.0 {
            let own = best_phase(span, dominant_period);
            let (track_share, _) = explained_share(span, dominant_period, own);
            if explained < track_share * CHANGE_MARGIN {
                continue;
            }
        }
        let frame = ((stretch.start as f64 + inside) * HOP as f64 + BLOCK as f64 / 2.0)
            .round()
            .max(0.0) as u64;
        let beat = match segments.last() {
            None => 0,
            Some(previous) => {
                let period_frames = per_second * 60.0 / previous.bpm * HOP as f64;
                let elapsed = frame as f64 - previous.first_beat_frame as f64;
                if elapsed <= 0.0 || period_frames <= 0.0 {
                    continue;
                }
                // The beat count runs on across the change: a bar number after the change is a bar
                // number, not a second grid starting at zero.
                previous.beat + (elapsed / period_frames).round() as u64
            }
        };
        if let Some(previous) = segments.last()
            && beat <= previous.beat
        {
            continue;
        }
        segments.push(TempoSegment {
            bpm: per_second * 60.0 / period,
            first_beat_frame: frame,
            beat,
        });
        confidence = confidence.min(explained);
    }
    if segments.len() < 2 {
        return None;
    }
    Some(Tempo {
        segments,
        confidence: confidence as f32,
        metre: DEFAULT_METRE,
    })
}

/// The tempo of a track, read from several windows of its envelope.
///
/// A track can open with an intro that has no beat at all — a pad, a spoken word, a rubato line. Read as
/// one block of audio, that intro dilutes the beat and the detector refuses the file. Read window by
/// window and the rest of the track still answers: the tempo that most of the windows agree on wins, so
/// an intro can add a vote but not cast the deciding one.
fn choose_tempo(envelope: &[f32], per_second: f64) -> Option<(f64, std::ops::Range<usize>)> {
    let window = (WINDOW_SECONDS * per_second) as usize;
    let hop = (WINDOW_HOP_SECONDS * per_second).max(1.0) as usize;
    if window < 64 || envelope.len() <= window {
        let bpm = tempo_of_window(envelope, per_second)?;
        return Some((bpm, 0..envelope.len()));
    }
    // Each answer remembers where it was found: the gate at the end asks "does this grid explain the
    // audio", and it has to ask a window that has a beat in it.
    let mut found: Vec<(f64, f32, std::ops::Range<usize>)> = Vec::new();
    let mut start = 0;
    while start + window <= envelope.len() {
        if let Some(bpm) = tempo_of_window(&envelope[start..start + window], per_second) {
            // A window's vote is its contrast: a clear beat is a confident vote, a muddled one is not.
            let period = per_second * 60.0 / bpm;
            let contrast = contrast_of(&envelope[start..start + window], period);
            found.push((bpm, contrast as f32, start..start + window));
        }
        start += hop;
    }
    if start < envelope.len()
        && envelope.len() - start >= window / 2
        && let Some(bpm) = tempo_of_window(&envelope[start..], per_second)
    {
        let period = per_second * 60.0 / bpm;
        let contrast = contrast_of(&envelope[start..], period);
        found.push((bpm, contrast as f32, start..envelope.len()));
    }
    if found.is_empty() {
        return None;
    }
    if found.len() == 1 {
        let only = found.first()?;
        return Some((only.0, only.2.clone()));
    }

    // Cluster the answers by tempo: windows of the same track land within a fraction of a percent of
    // each other, and a window that saw a doubled or halved pattern lands an octave away, in its own
    // cluster. The cluster with the most contrast behind it is the answer.
    let mut ordered = found.clone();
    ordered.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut best: Option<(f64, f64, std::ops::Range<usize>)> = None;
    let mut index = 0;
    while index < ordered.len() {
        let mut weight = 0.0f64;
        let mut moment = 0.0f64;
        let mut clear = 0..0usize;
        let mut clearest = f32::NEG_INFINITY;
        let anchor = ordered[index].0;
        let mut inner = index;
        while inner < ordered.len() {
            let (bpm, contrast, range) = &ordered[inner];
            // Within 3 % is the same tempo: the estimate is coarse, the windows are not exact.
            if (bpm / anchor).log2().abs() > 0.03 {
                break;
            }
            let value = f64::from(*contrast);
            weight += value;
            moment += value * bpm;
            if *contrast > clearest {
                clearest = *contrast;
                clear = range.clone();
            }
            inner += 1;
        }
        // Which cluster is the beat: the one whose grid carries the most of this track's onsets, measured
        // on the whole envelope and not on the windows that voted. A window's contrast says how clearly
        // that window repeated, which is a different question: on the acoustic take the loudest windows
        // said 60,7 BPM while the onsets of the whole file sit on 138,5, twice as well.
        let cluster_bpm = moment / weight;
        let period = per_second * 60.0 / cluster_bpm;
        let inside = best_phase(envelope, period);
        let (share, _) = explained_share(envelope, period, inside);
        // The prior cannot make a weak reading win, only settle a tie; the window weight keeps a cluster
        // nobody measured strongly from winning on a fraction of a percent of share.
        let settled = (share + weight * 0.001) * tempo_prior(cluster_bpm);
        if best
            .as_ref()
            .is_none_or(|(best_weight, _, _)| settled > *best_weight)
        {
            best = Some((settled, cluster_bpm, clear));
        }
        index = inner.max(index + 1);
    }
    let (_, bpm, clear) = best?;
    Some((bpm, clear))
}

/// How plausible a tempo is for the music a DJ plays, as a weight of 0 … 1.
fn tempo_prior(bpm: f64) -> f64 {
    if bpm <= 0.0 {
        return 0.0;
    }
    let octaves = (bpm / PREFERRED_BPM).log2() / PRIOR_SPREAD_OCTAVES;
    (-0.5 * octaves * octaves).exp()
}

/// The tempo of one window: the candidate whose grid sits on the onsets and not between them.
///
/// The spectrum alone cannot answer this. A track built on sixteenths puts its strongest peak at four
/// times its tempo, and its true beat can be a weak peak beside it; picking the lowest strong peak then
/// picks a harmonic, or nothing at all. What separates a beat from its subdivisions is contrast: at the
/// tempo, the grid lands on the loud hits and the halfway points land on quiet ones.
fn tempo_of_window(envelope: &[f32], per_second: f64) -> Option<f64> {
    let peaks = spectral_peaks(envelope, per_second);
    if peaks.is_empty() {
        return None;
    }
    let mut best = (0.0f64, 0.0f64); // (contrast, bpm)
    for (bpm, _) in &peaks {
        // A peak is either the tempo or one of its multiples: walk down by octaves and by thirds, and
        // let the contrast decide which one the beat is.
        let mut candidate = *bpm;
        let mut steps = 0;
        while candidate >= MIN_BPM && steps < 8 {
            if (MIN_BPM..=MAX_BPM).contains(&candidate) {
                let period = per_second * 60.0 / candidate;
                let contrast = contrast_of(envelope, period);
                if contrast > best.0 {
                    best = (contrast, candidate);
                }
            }
            candidate /= 2.0;
            steps += 1;
        }
        let mut thirds = 3.0;
        while *bpm / thirds >= MIN_BPM && thirds <= 9.0 {
            let candidate = *bpm / thirds;
            if (MIN_BPM..=MAX_BPM).contains(&candidate) {
                let period = per_second * 60.0 / candidate;
                let contrast = contrast_of(envelope, period);
                if contrast > best.0 {
                    best = (contrast, candidate);
                }
            }
            thirds *= 3.0;
        }
        // And the levels *above* the peak. A peak is the strongest periodicity, not the beat: an acoustic
        // track strums at twice the beat and accents every third of it, so its envelope spectrum peaks at
        // half and two thirds of the tempo while the beat itself is no peak at all. Measured on
        // `Среди своих (v3 - soft rock accustic)`: the windows answered 60,7 and 90,2 BPM, the beat is
        // 138,5 — and no division of either peak reaches it, only 90,2 × 3/2 does.
        for multiple in [2.0f64, 3.0, 1.5, 4.0 / 3.0] {
            let candidate = *bpm * multiple;
            if !(MIN_BPM..=MAX_BPM).contains(&candidate) {
                continue;
            }
            let period = per_second * 60.0 / candidate;
            let contrast = contrast_of(envelope, period);
            if contrast > best.0 {
                best = (contrast, candidate);
            }
        }
    }
    if best.1 <= 0.0 {
        return None;
    }
    Some(best.1)
}

/// How well a period works as the beat: the beat stands out from its own subdivisions, and the grid is
/// actually played.
///
/// Contrast alone cannot tell a beat from its double: a click every second makes a grid of one second
/// and a grid of half a second both look perfect, because neither has anything between its points. What
/// separates them is that the doubled grid has points nobody played — half of them land on silence — so
/// the score is the contrast times the share of the grid that carries an onset.
fn contrast_of(envelope: &[f32], period: f64) -> f64 {
    let mut best = 0.0f64;
    let steps = ((period / 0.5).ceil() as usize).clamp(4, 512);
    let strongest = envelope.iter().cloned().fold(0.0f32, f32::max);
    for step in 0..steps {
        let phase = period * step as f64 / steps as f64;
        let on = comb_score(envelope, period, phase);
        let between = comb_score(envelope, period, phase + period / 2.0);
        if on <= 0.0 {
            continue;
        }
        let contrast = on / between.max(1e-6);
        let played = played_share(envelope, period, phase, strongest);
        let score = contrast * played;
        if score > best {
            best = score;
        }
    }
    best
}

/// Share of a grid's points that carry a real onset, 0 … 1.
fn played_share(envelope: &[f32], period: f64, phase: f64, strongest: f32) -> f64 {
    if strongest <= 0.0 {
        return 0.0;
    }
    let floor = strongest * 0.1;
    let mut counted = 0usize;
    let mut played = 0usize;
    let mut position = phase;
    let length = envelope.len() as f64;
    while position < length && counted < 4_096 {
        let from = (position - SHARE_SPREAD).max(0.0) as usize;
        let to = ((position + SHARE_SPREAD) as usize + 1).min(envelope.len());
        let here: f32 = envelope[from..to].iter().sum();
        if here >= floor {
            played += 1;
        }
        counted += 1;
        position += period;
    }
    if counted == 0 {
        0.0
    } else {
        played as f64 / counted as f64
    }
}

/// The period the comb likes best around a hint, searched over ±0.6 % in steps of 0.02 %.
/// How much better a drifting grid has to explain the onsets before it replaces one tempo.
///
/// Measured: on a steady track the stretch machinery with a fine merge finds the same grid, so the two
/// scores are equal and the rigid answer stands; on a track that drifts the fine grid is better by a
/// quarter at least. Twenty percent is between.
const ELASTIC_MARGIN: f64 = 1.2;

/// How far a track's tempo has to move before a grid of segments is worth publishing (DJP-SYNC-002).
///
/// One percent: the smallest drift a DJ can hear across a track, and well above the jitter of a window's
/// own estimate. Measured on `Среди своих (v3 - soft rock accustic)`: windows differ by a fraction of a
/// percent from window to window, and without this gate the track came back cut into eight stretches that
/// all claimed the same tempo.
const MIN_DRIFT: f64 = 0.01;

/// The mean onset strength at the beats a grid implies, up to `end` (in hops).
fn grid_onset(envelope: &[f32], segments: &[TempoSegment], per_second: f64, end: f64) -> f64 {
    let mut sum = 0.0f64;
    let mut counted = 0usize;
    for (position, segment) in segments.iter().enumerate() {
        if !(segment.bpm.is_finite() && segment.bpm > 0.0) {
            return 0.0;
        }
        let period = per_second * 60.0 / segment.bpm;
        let until = segments
            .get(position + 1)
            .map_or(end, |next| {
                (next.first_beat_frame as f64 - BLOCK as f64 / 2.0) / HOP as f64
            })
            .min(end);
        let mut hop = (segment.first_beat_frame as f64 - BLOCK as f64 / 2.0) / HOP as f64;
        let mut guard = 0usize;
        while hop < until && guard < 100_000 {
            if let Some(value) = envelope.get(hop.round().max(0.0) as usize) {
                sum += f64::from(*value);
                counted += 1;
            }
            hop += period;
            guard += 1;
        }
    }
    if counted == 0 {
        0.0
    } else {
        sum / counted as f64
    }
}

/// Whether every segment of a drifting grid is still the same beat the rigid grid found.
///
/// The drift pass is about a tempo that *moves*, not about which metrical level it is: a track whose
/// windows answer half the beat should not come back as an elastic grid of half-time. Measured on
/// `Среди своих (v3 - soft rock accustic)`: the drift pass cut the track into eight stretches at 90 BPM
/// while the onsets of the whole file sit on 138,5 — a different beat, published because a drifting grid
/// was allowed to answer a question that was never asked of it.
fn same_level(segments: &[TempoSegment], bpm: f64) -> bool {
    bpm > 0.0
        && segments
            .iter()
            .all(|segment| segment.bpm > 0.0 && (segment.bpm / bpm - 1.0).abs() <= 0.03)
}

/// Whether the segments' tempo walks in one direction from the first to the last.
///
/// A window's tempo estimate jitters by a fraction of a percent, and a track that holds its tempo would
/// otherwise be cut into stretches by that jitter. A real drift — a band speeding up, a tape running down —
/// has a direction.
fn drifts_one_way(segments: &[TempoSegment]) -> bool {
    let mut rising = 0usize;
    let mut falling = 0usize;
    for pair in segments.windows(2) {
        if pair[1].bpm > pair[0].bpm {
            rising += 1;
        } else if pair[1].bpm < pair[0].bpm {
            falling += 1;
        }
    }
    rising == 0 || falling == 0
}

/// The range of tempos a DJ counts in.
///
/// A grid is a way of counting, and nobody counts a house record at 64: the music is at 128 and the coarser
/// grid is half of it. The choice between a period and its double cannot be made from the onsets alone —
/// four-on-the-floor puts a hit on every beat, so both grids land on hits — and it is not a property of the
/// music, it is the way the music is counted. So the levels outside this range are not candidates at all,
/// and the ones inside it are decided by their own measure. Measured on the reference set: without the
/// range, `Emotions`, `Movin My Body`, `One Love` and `Endless Summer` all came back at half tempo (64,
/// 63, 65,5, 65 BPM) with grids that fit the reference perfectly — every other beat of it.
const LEVEL_MIN_BPM: f64 = 70.0;
const LEVEL_MAX_BPM: f64 = 180.0;

/// Which beat the tempo is: the rough answer or one of its relatives, scored over the whole track.
///
/// Three numbers per candidate: the share of the onsets its grid carries, the prior over tempos, and how
/// much stronger the onsets on its beats are than the ones halfway between them. The share alone always
/// prefers the finer grid — twice as many beats catch at least as many onsets — and the ratio alone cannot
/// tell a four-on-the-floor beat from its double; the prior is what settles those, and it only has to
/// settle them because the range has already thrown out the levels nobody counts in.
/// The prior over the levels a track is counted in: where a DJ's tempos sit, tightly.
///
/// This is a narrower statement than [`tempo_prior`] and it is a different statement: that one settles
/// which of two clusters a window heard, this one says that a record nobody counts at 180 is counted at 90.
/// Half a tempo and twice a tempo are both possible descriptions of the same onsets; what makes one of them
/// the beat is that a DJ would count it there. Measured on the reference set, a spread of a third of an
/// octave around 120 counts every one of the seven tracks at the tempo its owner set — including
/// `Leash Broken`, where the doubled grid carries two thirds more onsets and is still not the beat.
fn level_prior(bpm: f64) -> f64 {
    if bpm <= 0.0 {
        return 0.0;
    }
    let octaves = (bpm / PREFERRED_BPM).log2() / 0.35;
    (-0.5 * octaves * octaves).exp()
}

fn decide_level(envelope: &[f32], per_second: f64, bpm: f64) -> f64 {
    if !(bpm.is_finite() && bpm > 0.0) {
        return bpm;
    }
    let mut best = (bpm, f64::NEG_INFINITY);
    // Octaves only. Four-thirds and three-halves are not levels: they are what a *metre* counted wrongly
    // does to the tempo, and they are corrected where the metre is read — offering them here let a 90 BPM
    // track be counted at 180 and left `Среди своих Stems` at 124,103 anyway.
    for multiple in [1.0, 2.0, 0.5] {
        let candidate = bpm * multiple;
        if !(LEVEL_MIN_BPM..=LEVEL_MAX_BPM).contains(&candidate) {
            continue;
        }
        let period = per_second * 60.0 / candidate;
        let (phase, _) = best_phase_and_score(envelope, period);
        let (share, _) = explained_share(envelope, period, phase);
        let (on, halfway) = beat_against_halfway(envelope, period, phase);
        let contrast = (on / halfway.max(1e-6)).clamp(0.5, 2.0);
        let score = share * level_prior(candidate) * contrast;
        if score > best.1 {
            best = (candidate, score);
        }
    }
    best.0
}

/// The metrical level of the tempo: the rough answer and its relatives, scored over the whole track.
///
/// The windows answer whichever periodicity they heard clearest, and that is not always the beat: on
/// `Среди своих (v3 - soft rock accustic)` the six windows answered 60,7 and 90,2 BPM, while the onsets of
/// the whole file sit on 138,5 — twice as many of them, once the level is asked of the whole track instead
/// of a forty-second slice. So the rough answer is re-asked among its relatives (the double, the half, the
/// three-halves, the four-thirds and their inverses), each scored by the share of the onsets its grid
/// carries, weighted by the same prior the clusters use — the prior is what keeps a sixteenth-note grid
/// from winning on the count of onsets alone.
fn refine_level(envelope: &[f32], per_second: f64, bpm: f64) -> f64 {
    if !(bpm.is_finite() && bpm > 0.0) {
        return bpm;
    }
    let mut best = (bpm, f64::NEG_INFINITY);
    for multiple in [1.0, 2.0, 1.5, 4.0 / 3.0, 0.75, 2.0 / 3.0, 0.5] {
        let candidate = bpm * multiple;
        if !(MIN_BPM..=MAX_BPM).contains(&candidate) {
            continue;
        }
        let period = per_second * 60.0 / candidate;
        let (share, _) = explained_share(envelope, period, best_phase(envelope, period));
        let score = share * tempo_prior(candidate);
        if score > best.1 {
            best = (candidate, score);
        }
    }
    best.0
}

/// The period a stretch of the track plays, searched three percent either way.
///
/// [`refine_period`] searches six tenths of a percent, which is right for a track that holds its tempo and
/// wrong for a window of one that does not: a stretch three percent slower than the track's own would be
/// pinned at the edge of the band and described at a tempo it does not play. Coarse then fine, so the search
/// costs about thirty periods rather than three hundred.
fn local_period(envelope: &[f32], hint: f64) -> f64 {
    if !hint.is_finite() || hint <= 1.0 {
        return hint;
    }
    let coarse: Vec<f64> = (-10..=10)
        .map(|step| hint * (1.0 + f64::from(step) * 0.003))
        .collect();
    let mut best = best_period(envelope, &coarse).unwrap_or((hint, f64::NEG_INFINITY));
    let fine: Vec<f64> = (-5..=5)
        .map(|step| best.0 * (1.0 + f64::from(step) * 0.0003))
        .collect();
    if let Some((period, score)) = best_period(envelope, &fine)
        && score > best.1
    {
        best = (period, score);
    }
    best.0
}

/// The period among the candidates whose grid explains the most onsets, and that score.
fn best_period(envelope: &[f32], candidates: &[f64]) -> Option<(f64, f64)> {
    let mut best: Option<(f64, f64)> = None;
    for period in candidates {
        if !(period.is_finite() && *period > 1.0) {
            continue;
        }
        let score = best_phase_and_score(envelope, *period).1;
        if best.is_none_or(|(_, top)| score > top) {
            best = Some((*period, score));
        }
    }
    best
}

/// The period searched finely around a guess, kept only when it explains the music better.
///
/// A tenth of a percent of period error walks a tenth of a beat across two minutes, and the comb is sharp
/// enough to see it: on `Shadow Rhythm — Deep Resonance` the coarse 103,250 hops score 49 while 103,297
/// score 202 — a rigid grid either sits on the music or does not. The search steps a two-hundredth of a
/// hop over ±0,4 %, and its answer is taken only when the onset strength on the grid improves by more than
/// two percent: an eight-second clip cannot tell a tenth of a percent from a coincidence, and there the
/// guess is kept.
fn refined_period(envelope: &[f32], period: f64) -> f64 {
    if !(period.is_finite() && period > 0.0) {
        return period;
    }
    let coarse = comb_score(envelope, period, best_phase(envelope, period));
    let mut fine = period * 0.996;
    let mut chosen = (period, coarse);
    while fine <= period * 1.004 {
        let score = comb_score(envelope, fine, best_phase(envelope, fine));
        if score > chosen.1 {
            chosen = (fine, score);
        }
        fine += 0.005;
    }
    if chosen.1 > coarse * 1.02 {
        chosen.0
    } else {
        period
    }
}

fn refine_period(envelope: &[f32], hint: f64) -> f64 {
    if !hint.is_finite() || hint <= 1.0 {
        return hint;
    }
    let mut best = (hint, f64::NEG_INFINITY);
    for step in -30..=30 {
        let period = hint * (1.0 + f64::from(step) * 0.0002);
        if period <= 1.0 {
            continue;
        }
        let score = best_phase_and_score(envelope, period).1;
        if score > best.1 {
            best = (period, score);
        }
    }
    best.0
}

/// The strongest peaks of the onset spectrum, in BPM, between [`MIN_BPM`] and four times it.
///
/// Four times, not twice: a track built on sixteenths at 100 BPM has its strongest peak at 400, and the
/// candidate walk needs to see it to find the 100 underneath.
fn spectral_peaks(envelope: &[f32], per_second: f64) -> Vec<(f64, f32)> {
    let size = envelope.len().clamp(2_048, 16_384).next_power_of_two();
    let Some(mut spectrum) = Spectrum::new(size) else {
        return Vec::new();
    };
    let mut block = vec![0.0f32; size];
    let take = envelope.len().min(size);
    block[..take].copy_from_slice(&envelope[..take]);
    let mut magnitudes = vec![0.0f32; size / 2];
    spectrum.magnitudes(&block, &mut magnitudes);

    let bin_hz = per_second / size as f64;
    let lowest = (MIN_BPM / 60.0 / bin_hz).floor().max(2.0) as usize;
    let highest = (MAX_BPM * 4.0 / 60.0 / bin_hz).ceil() as usize;
    let highest = highest.min(magnitudes.len().saturating_sub(2));
    if lowest + 2 >= highest {
        return Vec::new();
    }
    let strongest = magnitudes[lowest..highest]
        .iter()
        .cloned()
        .fold(0.0f32, f32::max);
    if strongest <= 0.0 {
        return Vec::new();
    }
    let mut peaks: Vec<(f64, f32)> = Vec::new();
    for bin in lowest + 1..highest {
        let value = magnitudes[bin];
        if value < strongest * PEAK_SHARE {
            continue;
        }
        if value >= magnitudes[bin - 1] && value >= magnitudes[bin + 1] {
            // Parabolic fit, so a peak between two bins is not rounded to the bin.
            let (left, centre, right) = (
                f64::from(magnitudes[bin - 1]),
                f64::from(magnitudes[bin]),
                f64::from(magnitudes[bin + 1]),
            );
            let denominator = left - 2.0 * centre + right;
            let offset = if denominator.abs() > f64::EPSILON {
                (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            peaks.push(((bin as f64 + offset) * bin_hz * 60.0, value / strongest));
        }
    }
    peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    peaks.truncate(16);
    peaks
}

/// The phase of the grid, in hops: where inside one period the beats start.
fn best_phase(envelope: &[f32], period: f64) -> f64 {
    best_phase_and_score(envelope, period).0
}

/// The phase of the grid and how well the comb fits there.
///
/// Searched in quarter-hop steps and then refined by a parabola through the best three, because the
/// guess at whole hops alone would be worth ten milliseconds at a fast tempo — audible as a grid that
/// sits behind the kick.
/// The phase a grid of `period` hops sits on, and the score of that grid.
pub fn best_phase_for(envelope: &[f32], period: f64) -> (f64, f64) {
    best_phase_and_score(envelope, period)
}

/// The share of the onsets a grid carries, and how many of its beats carry any.
pub fn share_for(envelope: &[f32], period: f64, phase: f64) -> (f64, usize) {
    explained_share(envelope, period, phase)
}

/// How even the onsets on the beats are: the mean of the envelope over them, and the coefficient of
/// variation of that mean.
///
/// A grid at the beat lands on hits of the same kind — the kick of every beat, the strum of every beat —
/// and a grid at twice the beat alternates between the beat's hit and whatever sits between beats, so the
/// strengths come out uneven. This is what tells 90 BPM from 180 on a track whose beat carries the weight.
pub fn beat_evenness(envelope: &[f32], period: f64, phase: f64) -> (f64, f64) {
    if !(period.is_finite() && period >= 2.0) {
        return (0.0, 0.0);
    }
    let reach = (period * 0.12).clamp(1.0, 8.0) as usize;
    let mut values: Vec<f64> = Vec::new();
    let mut position = phase;
    while position < envelope.len() as f64 {
        let at = position.round() as usize;
        let from = at.saturating_sub(reach);
        let to = (at + reach).min(envelope.len().saturating_sub(1));
        if from <= to {
            let loudest = envelope[from..=to].iter().copied().fold(0.0f32, f32::max);
            values.push(f64::from(loudest));
        }
        position += period;
    }
    if values.len() < 4 {
        return (0.0, 0.0);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    if mean <= 0.0 {
        return (0.0, 0.0);
    }
    let variance = values
        .iter()
        .map(|value| (value - mean) * (value - mean))
        .sum::<f64>()
        / values.len() as f64;
    (mean, variance.sqrt() / mean)
}

/// The share of the beats that carry the pulse: the fraction whose kick-band onset is within a third of
/// the loudest beat's.
///
/// A level that doubles the beat puts a hit on every other beat of it and nothing on the rest, so half of
/// its beats carry no pulse — the grid describes a beat the track does not play. This is the measure that
/// tells 90 BPM from 180 on a record whose subdivisions are as loud as its beat.
pub fn pulse_duty(kick: &[f32], period: f64, phase: f64) -> f64 {
    if !(period.is_finite() && period >= 2.0) {
        return 0.0;
    }
    let reach = (period * 0.12).clamp(1.0, 8.0) as usize;
    let mut values: Vec<f32> = Vec::new();
    let mut position = phase;
    while position < kick.len() as f64 {
        let at = position.round() as usize;
        let from = at.saturating_sub(reach);
        let to = (at + reach).min(kick.len().saturating_sub(1));
        if from <= to {
            values.push(kick[from..=to].iter().copied().fold(0.0f32, f32::max));
        }
        position += period;
    }
    if values.len() < 4 {
        return 0.0;
    }
    let loudest = values.iter().copied().fold(0.0f32, f32::max);
    if loudest <= 0.0 {
        return 0.0;
    }
    let carrying = values
        .iter()
        .filter(|value| **value >= loudest * 0.33)
        .count();
    carrying as f64 / values.len() as f64
}

/// How strong the onsets on the beats are against the ones halfway between them.
///
/// This is what tells a beat from half of it. A grid at half the tempo lands on every other beat, so the
/// onsets halfway between its beats are onsets of the music's own beat: the two sides come out alike. A
/// grid on the beat has the kick on it and whatever sits halfway is not the kick, and the two sides do not
/// come out alike. The measure is a mean of the envelope over the beats and over the points halfway
/// between them, each taken as the loudest hop within a fraction of a beat so a small phase error does not
/// decide the question.
pub fn beat_against_halfway(envelope: &[f32], period: f64, phase: f64) -> (f64, f64) {
    if !(period.is_finite() && period >= 2.0) {
        return (0.0, 0.0);
    }
    let reach = (period * 0.12).clamp(1.0, 8.0) as usize;
    let end = envelope.len() as f64;
    let loudest = |centre: f64| -> f32 {
        let at = centre.round() as usize;
        let from = at.saturating_sub(reach);
        let to = (at + reach).min(envelope.len().saturating_sub(1));
        envelope[from..=to].iter().copied().fold(0.0f32, f32::max)
    };
    let mut on = 0.0f64;
    let mut on_count = 0usize;
    let mut halfway = 0.0f64;
    let mut halfway_count = 0usize;
    let mut position = phase;
    while position < end {
        if (position.round() as usize) < envelope.len() {
            on += f64::from(loudest(position));
            on_count += 1;
        }
        let between = position + period * 0.5;
        if (between.round() as usize) < envelope.len() {
            halfway += f64::from(loudest(between));
            halfway_count += 1;
        }
        position += period;
    }
    (
        if on_count > 0 {
            on / on_count as f64
        } else {
            0.0
        },
        if halfway_count > 0 {
            halfway / halfway_count as f64
        } else {
            0.0
        },
    )
}

fn best_phase_and_score(envelope: &[f32], period: f64) -> (f64, f64) {
    const STEP: f64 = 0.25;
    let steps = ((period / STEP).ceil() as usize).clamp(4, 8_192);
    let mut scores = Vec::with_capacity(steps);
    let mut best = (0.0f64, f64::NEG_INFINITY);
    let mut best_index = 0usize;
    for step in 0..steps {
        let phase = step as f64 * STEP;
        let score = comb_score(envelope, period, phase);
        scores.push(score);
        if score > best.1 {
            best = (phase, score);
            best_index = step;
        }
    }
    // Parabola through the neighbours of the winner: the true phase is between two quarter hops.
    if best_index > 0 && best_index + 1 < scores.len() {
        let (left, centre, right) = (
            scores[best_index - 1],
            scores[best_index],
            scores[best_index + 1],
        );
        let denominator = left - 2.0 * centre + right;
        if denominator.abs() > f64::EPSILON {
            let offset = 0.5 * (left - right) / denominator;
            return ((best.0 + offset.clamp(-0.5, 0.5) * STEP).max(0.0), best.1);
        }
    }
    best
}

/// The share of the envelope's energy that lands on the grid, and how many beats carry any of it.
///
/// A metronome puts everything on the grid; a rubato line puts a fraction there by coincidence; a held
/// note puts one bump on one beat and nothing anywhere else. The count is what tells the last two apart.
fn explained_share(envelope: &[f32], period: f64, phase: f64) -> (f64, usize) {
    let total: f64 = envelope.iter().map(|value| f64::from(*value)).sum();
    if total <= 0.0 {
        return (0.0, 0);
    }
    let strongest = envelope.iter().cloned().fold(0.0f32, f32::max);
    // Half a hop either side of each grid point: the flux of one onset spans a couple of hops, and a
    // grid that hits the onset exactly would otherwise be punished for the smear it created.
    let mut on_grid = 0.0f64;
    let mut beats = 0usize;
    let mut position = phase;
    let length = envelope.len() as f64;
    let mut last = f64::NEG_INFINITY;
    while position < length {
        let from = (position - SHARE_SPREAD).max(0.0) as usize;
        let to = ((position + SHARE_SPREAD) as usize + 1).min(envelope.len());
        if from as f64 > last {
            let mut here = 0.0f32;
            for value in &envelope[from..to] {
                here += *value;
            }
            on_grid += f64::from(here);
            // A beat that carries a tenth of the loudest onset is a beat somebody played.
            if here >= strongest * 0.1 {
                beats += 1;
            }
            last = to as f64;
        }
        position += period;
    }
    ((on_grid / total).clamp(0.0, 1.0), beats)
}

/// Mean onset strength on the grid of `period` hops starting at `phase`.
fn comb_score(envelope: &[f32], period: f64, phase: f64) -> f64 {
    let mut sum = 0.0f64;
    let mut counted = 0usize;
    let mut position = phase;
    let length = envelope.len() as f64;
    while position < length && counted < 1_024 {
        let index = position as usize;
        let fraction = position - index as f64;
        let value = if index + 1 < envelope.len() {
            f64::from(envelope[index]) * (1.0 - fraction)
                + f64::from(envelope[index + 1]) * fraction
        } else {
            f64::from(envelope[index])
        };
        sum += value;
        counted += 1;
        position += period;
    }
    if counted == 0 {
        0.0
    } else {
        sum / counted as f64
    }
}

/// Beats in a bar when the kick band has nothing to count by.
pub const DEFAULT_METRE: u8 = 4;

/// Top of the band the kick lives in, in hertz.
///
/// The beat of most music a DJ plays is carried by the kick and the bass, and everything above that
/// marks the subdivisions. Measured on a track the full spectrum read as 133,41 BPM while the music is
/// four-four at 100: below 150 Hz the bar folds cleanly at 100 and not at 133,41 — the kick band is
/// flat at 133,41 (contrast 1,48) and puts the kick on one and three at 100 (2,87). Its sixteenths are
/// above this band, which is why the band can tell the two apart at all.
const KICK_HZ: f64 = 150.0;

/// How strongly the kick has to favour one beat of the bar before the grid is turned to it.
///
/// A click track, a pad and an aperiodic line put as much on every beat as on every other (1,0); a
/// four-four with the kick on one and three measures 2,1 … 3,0 and its intro 2,5 … 3,0. Nothing between
/// 1,25 and 2,0 was measured on real music, so the gate sits in the empty part of the range.
const KICK_METRE_MIN: f64 = 1.25;

/// How far an accent period may be from a whole number of beats and still count as one, as a fraction
/// of that number. Two percent of four beats is 24 ms at 100 BPM: wider than the measurement error of a
/// hop and far narrower than the third of a beat that separates a beat from three sixteenths.
const BAR_TOLERANCE: f64 = 0.02;

/// How much more kick the beat positions have to carry than the average point of the band before the
/// grid counts as carrying the kick at all. Measured: 2,5 … 4,0 on electronic music whose beat is the
/// kick; 0,9 … 1,2 on a grid that only meets it by arithmetic.
const KICK_ON_BEATS: f64 = 1.4;

/// The metres the bar is measured against, in beats. Four first: when the band cannot tell them apart
/// the more common one is the better guess, and the gate above is what decides whether it can.
const METRES: [u8; 2] = [4, 3];

/// The onset envelope of the kick band: flux below [`KICK_HZ`], weighted by the magnitude it reaches.
///
/// Weighted, unlike [`onset_envelope`]: the rise of a kick is wide and slow, and weighting it by the
/// level it reaches favours the boom over the rumble under it. Same blocks and hops as the full
/// envelope, so a frame means the same thing in both.
pub fn kick_envelope(mono: &[f32], rate: f64) -> Option<Vec<f32>> {
    let mut spectrum = Spectrum::new(BLOCK)?;
    let bins = BLOCK / 2;
    let hz_per_bin = rate / BLOCK as f64;
    let low_to = ((KICK_HZ / hz_per_bin).round() as usize).clamp(1, bins - 1);
    let mut magnitudes = vec![0.0f32; bins];
    let mut previous = vec![0.0f32; bins];
    let mut block = vec![0.0f32; BLOCK];
    let mut flux = Vec::with_capacity(mono.len() / HOP + 1);
    let mut start = 0;
    while start + BLOCK <= mono.len() {
        block.copy_from_slice(&mono[start..start + BLOCK]);
        spectrum.magnitudes(&block, &mut magnitudes);
        let mut sum = 0.0f32;
        for bin in 1..low_to {
            sum += (magnitudes[bin] - previous[bin]).max(0.0) * magnitudes[bin];
        }
        flux.push(sum);
        previous.copy_from_slice(&magnitudes);
        start += HOP;
    }
    if flux.len() < 64 {
        return None;
    }
    Some(remove_the_running_mean(flux, 94))
}

/// Where the bar is, as the kick band sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarGuess {
    /// Beats in a bar: one of [`METRES`].
    pub metre: u8,
    /// Which beat of the bar carries the kick, counted from the first beat of the grid.
    pub downbeat: usize,
    /// The loudest beat's mean over the bar's mean: 1,0 is a bar with no accent in it.
    pub contrast: f64,
    /// The kick band's mean on the beats over its mean everywhere: how much of the band the grid
    /// actually carries. A grid that is not the beat of this music catches the kick now and then, by
    /// arithmetical accident, and this is what tells the accident from the beat.
    pub on_the_beats: f64,
}

/// How well an envelope repeats after `lag` hops, as a correlation of 0 … 1.
///
/// A bar of music comes back: the kick of this bar falls where the kick of the last one did. A lag that
/// is not a bar — a beat and a half, say — cuts across the pattern and correlates with nothing.
fn repeat_after(envelope: &[f32], lag: f64) -> f64 {
    let hops = lag.round();
    if !hops.is_finite() || hops < 1.0 || hops as usize >= envelope.len() {
        return 0.0;
    }
    let lag = hops as usize;
    let mut cross = 0.0;
    let mut left = 0.0;
    let mut right = 0.0;
    for index in 0..envelope.len() - lag {
        let a = f64::from(envelope[index]);
        let b = f64::from(envelope[index + lag]);
        cross += a * b;
        left += a * a;
        right += b * b;
    }
    if left <= 0.0 || right <= 0.0 {
        return 0.0;
    }
    cross / (left * right).sqrt()
}

/// The accent periods of the kick band: the lags it repeats after, strongest first.
///
/// This is the measurement the whole bar rests on, and it is the one the full spectrum cannot make: the
/// accents of a bar are a pattern in the low band, and the spectrum of the onset envelope keeps the
/// energy while throwing the pattern away. A track whose every sixteenth has an onset — which is what
/// dense percussion is — has a beat that explains as many onsets as a grid of three sixteenths, and that
/// ambiguity is what put a 4/4 at 100 BPM one third of a bar off in the first place.
///
/// The band does not say which of its periods is the bar: a kick on one and three repeats every two
/// beats as well as every four, and a bar of three repeats every six if two bars are played alike. It
/// says the list, and the grid then has to divide one of them by a whole number of beats.
fn accent_lags(kick: &[f32], per_second: f64) -> Vec<(f64, f64)> {
    // A bar is between a third of a second (a fast bar) and four seconds (a slow one). Under a third of
    // a second there is no bar a DJ grid needs; over four, a track would be under fifteen bars a minute.
    let shortest = (0.3 * per_second).round().max(1.0) as usize;
    let longest = ((4.0 * per_second).round() as usize).min(kick.len().saturating_sub(1));
    if longest <= shortest + 2 {
        return Vec::new();
    }
    let mut points = Vec::with_capacity(longest - shortest + 1);
    for lag in shortest..=longest {
        points.push((lag as f64 / per_second, repeat_after(kick, lag as f64)));
    }
    // Local maxima only: a plateau of nearly equal lags is one accent period, not twenty.
    let mut peaks: Vec<(f64, f64)> = Vec::new();
    for index in 1..points.len().saturating_sub(1) {
        let (lag, value) = points[index];
        if value >= points[index - 1].1 && value > points[index + 1].1 && value > 0.0 {
            peaks.push((lag, value));
        }
    }
    peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
    peaks.truncate(5);
    peaks
}

/// The metre an accent period gives a beat, if it gives one: it has to be a whole number of beats.
///
/// Three beats in the bar is a waltz and is counted in three. Two, four and eight are a four-beat bar
/// heard with an accent on every second beat, or two bars played alike. Five and seven are neither, and
/// a lag that is not a whole number of beats is not the bar of this grid at all.
fn metre_of(lag: f64, period: f64) -> Option<u8> {
    if !(period.is_finite() && period > 0.0) {
        return None;
    }
    let ratio = lag / period;
    let beats = ratio.round();
    if !(2.0..=8.0).contains(&beats) || (ratio - beats).abs() / beats > BAR_TOLERANCE {
        return None;
    }
    match beats as u8 {
        3 | 6 => Some(3),
        2 | 4 | 8 => Some(4),
        _ => None,
    }
}

/// The bar as the kick band sees it, at a beat the onsets chose.
///
/// The band is asked two things: which beat of the bar carries the accent, and how much of the band the
/// beats carry at all. It is not asked where the beat is. In music where the bass plays against the beat
/// the band's own maximum sits on the syncopation, and a grid that followed it landed a fifth of a beat
/// off the music — measured on `Shadow Rhythm — Deep Resonance` (the grid scored 1,4 % of the best the
/// onset envelope offers) and reproduced by `a_syncopated_kick_does_not_move_the_grid_off_the_beat`.
fn bar_of_kick(kick: &[f32], period_hops: f64, metre: u8, phase: f64) -> Option<BarGuess> {
    if !(period_hops.is_finite() && period_hops >= 1.0) {
        return None;
    }
    let end = kick.len() as f64 - 1.0;
    let everywhere: f64 =
        kick.iter().map(|value| f64::from(*value)).sum::<f64>() / kick.len() as f64;
    if everywhere <= 0.0 {
        return None;
    }
    let mut on = 0.0f64;
    let mut beats = 0usize;
    let mut position = phase;
    while position < end {
        on += f64::from(kick[position.round() as usize]);
        beats += 1;
        position += period_hops;
    }
    if beats == 0 {
        return None;
    }
    let slots = metre as usize;
    let mut sums = vec![0.0f64; slots];
    let mut counts = vec![0usize; slots];
    let mut position = phase;
    let mut index = 0usize;
    while position < end {
        let at = position.round() as usize;
        if at < kick.len() {
            sums[index % slots] += f64::from(kick[at]);
            counts[index % slots] += 1;
        }
        position += period_hops;
        index += 1;
    }
    // Two bars of beats is the least that can show an accent in every slot of the bar.
    if index < 2 * slots {
        return None;
    }
    let means: Vec<f64> = sums
        .iter()
        .zip(counts.iter())
        .map(|(sum, count)| if *count > 0 { sum / *count as f64 } else { 0.0 })
        .collect();
    let average = means.iter().sum::<f64>() / slots as f64;
    if average <= 0.0 {
        return None;
    }
    let peak = means.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let downbeat = means
        .iter()
        .position(|value| (*value - peak).abs() < f64::EPSILON)
        .unwrap_or(0);
    Some(BarGuess {
        metre,
        downbeat,
        contrast: peak / average,
        on_the_beats: on / beats as f64 / everywhere,
    })
}

/// The tempo, the phase and the bar the kick band agrees with.
///
/// The spectrum's own answer is kept whenever the accents divide by it: a track whose grid is right is
/// not moved because another reading also has a bar. Only when no accent period is a whole number of
/// its beats is the grid wrong in principle — a beat that lands a bar and a third apart from the accents
/// is a subdivision of them, not their beat — and then the answer is the tempo an accent period implies,
/// the strongest period first.
fn bar_tempo_of(
    kick: &[f32],
    onsets: &[f32],
    per_second: f64,
    bpm: f64,
) -> Option<(f64, f64, BarGuess)> {
    let lags = accent_lags(kick, per_second);
    let own_period = 60.0 / bpm;
    if let Some(metre) = lags.iter().find_map(|(lag, _)| metre_of(*lag, own_period)) {
        // The beat is where the onsets put it; the band only says which beat of the bar is the first.
        let period_hops = per_second * own_period;
        let phase = best_phase(onsets, period_hops);
        let bar = bar_of_kick(kick, period_hops, metre, phase)?;
        return Some((bpm, phase, bar));
    }
    let mut best: Option<(f64, f64, BarGuess, f64)> = None;
    for (lag, strength) in lags.iter().take(3) {
        for metre in METRES {
            let period = *lag / f64::from(metre);
            let candidate = 60.0 / period;
            if !(MIN_BPM..=MAX_BPM).contains(&candidate) {
                continue;
            }
            let period_hops = refined_period(onsets, period * per_second);
            let phase = best_phase(onsets, period_hops);
            let Some(bar) = bar_of_kick(kick, period_hops, metre, phase) else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|(_, _, _, score)| *strength > *score)
            {
                let candidate = per_second * 60.0 / period_hops;
                best = Some((candidate, phase, bar, *strength));
            }
        }
    }
    best.map(|(bpm, phase, bar, _)| (bpm, phase, bar))
}

/// Turn the grid onto the bar: the beat the kick band calls the first, and the metre it counts in.
///
/// Two questions the full spectrum cannot answer. It cannot tell a beat from a group of three of them:
/// dense percussion accents every third sixteenth as well, so a grid of three sixteenths explains as
/// many onsets as the beat does and the tempo comes out four thirds of the music's. And it cannot say
/// which beat of the bar is the first, because that is a pattern of accents, and the spectrum keeps the
/// energy while throwing the pattern away. The kick band has both, so both are asked of it — and taken
/// only when it is clear about them, because a grid moved on a guess is worse than a grid left alone.
fn on_the_bar(onsets: &[f32], kick: &[f32], rate: f64, tempo: &mut Tempo) {
    // A track with several tempos has a bar per stretch, and turning all of them by the first one's
    // accent is a claim the measurement does not make. Such a track keeps its answer as it is.
    if !tempo.is_single() {
        return;
    }
    let per_second = rate / HOP as f64;
    let Some(segment) = tempo.segments.first().copied() else {
        return;
    };
    let Some((bpm, phase, bar)) = bar_tempo_of(kick, onsets, per_second, segment.bpm) else {
        return;
    };
    // The band names the bar — which beat of it is the first — and it is believed only when it has an
    // accent to show. A band with nothing on the beats is a track whose beat is not a kick: a pad, a
    // string line, a snare-led pattern. Such a track starts its bars on the beat the onsets gave, a guess
    // that changes nothing about where the beats are.
    let downbeat = if bar.on_the_beats >= KICK_ON_BEATS && bar.contrast >= KICK_METRE_MIN {
        bar.downbeat
    } else {
        0
    };
    let period = per_second * 60.0 / bpm;
    // When the tempo does not move, the anchor the tracker found is kept and only the bar is laid on it:
    // the downbeat is a whole number of beats, so shifting the anchor by them changes which beat is the
    // first of a bar and nothing about where the beats are.
    let first = if (bpm / segment.bpm - 1.0).abs() < 0.001 {
        segment.first_beat_frame as f64 + downbeat as f64 * period * HOP as f64
    } else {
        let beat = phase + downbeat as f64 * period;
        // Half a block, as in `single_tempo`: the block that saw the onset starts a hop before it.
        beat * HOP as f64 + BLOCK as f64 / 2.0
    };
    if !first.is_finite() || first < 0.0 || first > u64::MAX as f64 {
        return;
    }
    tempo.segments = vec![TempoSegment {
        bpm,
        first_beat_frame: first as u64,
        beat: 0,
    }];
    tempo.metre = bar.metre;
}

/// The onset nearest `at`, refined to a fraction of a hop: where the envelope's peak actually is.
///
/// The peak is a parabola through three hops: the envelope is smoothed by the spectrum's own window, so
/// the true attack sits between two hops and the parabola finds it to a few hundredths of one.
/// The same, for measurement: where the peak nearest `at` actually is, in hops.
pub fn onset_near_peak(envelope: &[f32], at: f64, reach: f64) -> Option<f64> {
    onset_near(envelope, at, reach)
}

fn onset_near(envelope: &[f32], at: f64, reach: f64) -> Option<f64> {
    let first = (at - reach).floor().max(1.0) as usize;
    let last = ((at + reach).ceil() as usize).min(envelope.len().saturating_sub(2));
    if last <= first {
        return None;
    }
    let mut best = first;
    for index in first..=last {
        if envelope[index] > envelope[best] {
            best = index;
        }
    }
    if envelope[best] <= 0.0 {
        return None;
    }
    let (left, centre, right) = (
        f64::from(envelope[best - 1]),
        f64::from(envelope[best]),
        f64::from(envelope[best + 1]),
    );
    let denominator = left - 2.0 * centre + right;
    let offset = if denominator.abs() > f64::EPSILON {
        (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    Some(best as f64 + offset)
}

/// Put the single-tempo grid on the onsets it lands on, to the sample.
///
/// The tempo is chosen by a comb over hops and the phase to a quarter of one; that is enough to know
/// which periodicity the music has and not enough to describe it. A hop is 5,8 ms at 44,1 kHz, and a
/// quarter hop of period error is a quarter of a hop per beat — 6 ms after a hundred beats, a third of a
/// second after a five-minute track. The reference grids a DJ sets by hand are exact to the sample, and a
/// grid that drifts away from the music by a third of a second is not the same grid.
///
/// So the last step is a fit: every beat of the chosen lattice is matched with the onset it lands on, and
/// a line through those onsets — time against beat number — is the tempo and the anchor. The fit is
/// weighted by nothing and repeated on the beats that agree with it, because a beat landing on a
/// breakdown has no onset to fit and a beat landing on a syncopation has the wrong one.
fn snap_to_onsets(tempo: &mut Tempo, envelope: &[f32], per_second: f64) {
    // A grid of stretches has a period per stretch, and each is settled by the same rule; a track that
    // holds its tempo is what this fits.
    if !tempo.is_single() {
        return;
    }
    let Some(segment) = tempo.segments.first().copied() else {
        return;
    };
    let Some((bpm, first)) = settle_tempo(
        envelope,
        per_second,
        segment.bpm,
        segment.first_beat_frame as f64,
    ) else {
        return;
    };
    if !(bpm.is_finite() && bpm > 0.0 && first.is_finite() && first >= 0.0) {
        return;
    }
    tempo.segments = vec![TempoSegment {
        bpm,
        first_beat_frame: first.round() as u64,
        beat: 0,
    }];
}

/// How much the first beat of the bar stands above the rest of it, in the kick band.
///
/// The mean kick on the bar's first slot over the mean of the other slots. A bar that is really counted
/// the way the music plays it has an accent: the first beat carries the loudest hit of the four. A bar cut
/// into the wrong number of beats has no accent at all — its slots all carry the same mixture — so this is
/// a measure of whether the bar is counted right, not of how loud the track is.
fn bar_contrast(kick: &[f32], period: f64, phase: f64, metre: usize) -> f64 {
    if !(period.is_finite() && period >= 2.0) || metre < 2 {
        return 0.0;
    }
    let reach = (period * 0.12).clamp(1.0, 8.0) as usize;
    let mut sums = vec![0.0f64; metre];
    let mut counts = vec![0usize; metre];
    let mut beat = 0usize;
    let mut position = phase.rem_euclid(period);
    while position < kick.len() as f64 {
        let at = position.round() as usize;
        if at < kick.len() {
            let from = at.saturating_sub(reach);
            let to = (at + reach).min(kick.len().saturating_sub(1));
            let loudest = kick[from..=to].iter().copied().fold(0.0f32, f32::max);
            sums[beat % metre] += f64::from(loudest);
            counts[beat % metre] += 1;
        }
        beat += 1;
        position += period;
    }
    if beat < 2 * metre || counts.contains(&0) {
        return 0.0;
    }
    let means: Vec<f64> = (0..metre)
        .map(|slot| sums[slot] / counts[slot] as f64)
        .collect();
    let first = means[0];
    let rest = means[1..].iter().sum::<f64>() / (metre - 1) as f64;
    if rest <= 0.0 {
        return 0.0;
    }
    first / rest
}

/// Whether the tempo is four thirds of the music's, decided by the bar's accent.
///
/// The same accent period divides into four beats of the music and into three of the answer: the tempo comes
/// out four thirds of the truth, and the onsets fit that answer as well as they fit the truth — a grid of
/// three quarter-notes catches the same hits as a grid of four, only spread differently. What does not fit
/// is the bar: counted in four the first beat carries the accent, counted in three the accents fall on
/// different slots and average out.
///
/// So the reading is asked twice — as it stands and at three quarters of it — and the bar's accent decides.
/// Measured on `Среди своих Stems - Music.wav`: ours 124,103 BPM with an accent ratio near 1, the owner's
/// 93,078 with 3,0. The margin is a third: an accent that stands only a little above the rest does not
/// move a grid.
fn settle_four_the_third(tempo: &mut Tempo, envelope: &[f32], kick: &[f32], per_second: f64) {
    if !tempo.is_single() {
        return;
    }
    let Some(segment) = tempo.segments.first().copied() else {
        return;
    };
    let metre = usize::from(tempo.metre.clamp(2, 8));
    let own_period = per_second * 60.0 / segment.bpm;
    let own_phase = ((segment.first_beat_frame as f64 - BLOCK as f64 / 2.0) / HOP as f64)
        .rem_euclid(own_period);
    let own = bar_contrast(kick, own_period, own_phase, metre);
    // Only when the bar we found has no accent to speak of: a bar that carries an accent is counted the way
    // the music plays it, and looking further from there found accents that were not there — `One Love`
    // went from 131 to 174,667 and `Endless Summer` from 130 to 173,334 on this rule.
    if own >= 2.0 {
        return;
    }
    let mut best = (own, segment.bpm, segment.first_beat_frame as f64);
    // Downwards only: the mistake being corrected is a tempo measured four thirds too high, and offering
    // the other direction as well let three tracks be counted at four thirds of themselves.
    for multiple in [0.75f64] {
        let candidate = segment.bpm * multiple;
        if !(LEVEL_MIN_BPM..=LEVEL_MAX_BPM).contains(&candidate) {
            continue;
        }
        let period = per_second * 60.0 / candidate;
        let (phase, _) = best_phase_and_score(envelope, period);
        let contrast = bar_contrast(kick, period, phase, metre);
        if contrast > best.0 {
            best = (contrast, candidate, phase * HOP as f64 + BLOCK as f64 / 2.0);
        }
    }
    // The reading has to *have* an accent, not merely a bigger number than a bar with none: on `One Love`
    // the four-thirds reading scored 0,96 against our 0,15, and 0,96 is still a bar with no accent in it.
    // What the accent is worth is read off the scale of the measure itself — a bar counted right puts the
    // first beat well above the rest, and only a real accent (over two) is worth moving a grid for.
    if best.1 == segment.bpm || best.0 < 2.0 || best.0 <= own * 1.3 {
        return;
    }
    // The new reading gets the same treatment as any other: its period settled over the whole track, its
    // side of the beat decided, its bar put on the accent.
    let mut moved = Tempo {
        segments: vec![TempoSegment {
            bpm: best.1,
            first_beat_frame: best.2.round().max(0.0) as u64,
            beat: 0,
        }],
        confidence: tempo.confidence,
        metre: tempo.metre,
    };
    snap_to_onsets(&mut moved, envelope, per_second);
    on_the_beat_side(&mut moved, kick, per_second);
    align_the_bar(&mut moved, kick, per_second);
    if let Some(segment) = moved.segments.first().copied()
        && (segment.bpm / best.1 - 1.0).abs() < 0.01
    {
        *tempo = moved;
    }
}

/// Move the grid so that its first beat is the first beat of the bar.
///
/// A bar is a pattern of accents: the kick of the first beat is the loudest of the four, and that is what
/// says where the bar starts. The question is asked of the kick band over the whole track — every beat is
/// given the slot it holds in the bar, the slot with the loudest mean wins, and the grid is moved by whole
/// beats onto it. Whole beats keep the lattice: the lines do not move, only which of them opens a bar.
///
/// Measured on the reference set: the loudest kick stands on the owner's first beat of the bar on all seven
/// readable tracks, and before this the grid sat one beat late on five of them and three beats late on one.
fn align_the_bar(tempo: &mut Tempo, kick: &[f32], per_second: f64) {
    if !tempo.is_single() {
        return;
    }
    let Some(segment) = tempo.segments.first().copied() else {
        return;
    };
    let metre = usize::from(tempo.metre.clamp(2, 8));
    let period = per_second * 60.0 / segment.bpm;
    if !(period.is_finite() && period >= 2.0) {
        return;
    }
    let phase =
        ((segment.first_beat_frame as f64 - BLOCK as f64 / 2.0) / HOP as f64).rem_euclid(period);
    let reach = (period * 0.12).clamp(1.0, 8.0) as usize;
    let mut sums = vec![0.0f64; metre];
    let mut counts = vec![0usize; metre];
    let mut beat = 0usize;
    let mut position = phase;
    while position < kick.len() as f64 {
        let at = position.round() as usize;
        if at < kick.len() {
            let from = at.saturating_sub(reach);
            let to = (at + reach).min(kick.len().saturating_sub(1));
            let loudest = kick[from..=to].iter().copied().fold(0.0f32, f32::max);
            sums[beat % metre] += f64::from(loudest);
            counts[beat % metre] += 1;
        }
        beat += 1;
        position += period;
    }
    // Two bars of beats is the least that can show an accent in every slot.
    if beat < 2 * metre || counts.contains(&0) {
        return;
    }
    let mut best = 0usize;
    let mut best_mean = f64::NEG_INFINITY;
    for slot in 0..metre {
        let mean = sums[slot] / counts[slot] as f64;
        if mean > best_mean {
            best_mean = mean;
            best = slot;
        }
    }
    if best == 0 || best_mean <= 0.0 {
        return;
    }
    // The first beat of the bar is `best` beats after the one the grid calls its own: the anchor moves
    // forward onto it — a later beat of the same lattice, so the lines stay where they are. Taking the
    // sum back into the period would undo the move: it is the same lattice, but the anchor has to be a
    // *named* beat for the bar to start there.
    let first = phase + best as f64 * period;
    let frames = first * HOP as f64 + BLOCK as f64 / 2.0;
    if !(frames.is_finite() && frames >= 0.0) {
        return;
    }
    tempo.segments = vec![TempoSegment {
        bpm: segment.bpm,
        first_beat_frame: frames.round() as u64,
        beat: 0,
    }];
}

/// Put the grid on the side of the beat the weight of the music is on.
///
/// A lattice and the same lattice half a beat later both fit an onset envelope that has something on every
/// eighth: the comb cannot tell which of the two sides is the beat, and the full spectrum prefers the hats,
/// which are broadband. The kick band does not: a track's beat is the kick, and the offbeat rarely has one.
/// So the side is decided there, and a grid found on the wrong side is moved by half a period.
fn on_the_beat_side(tempo: &mut Tempo, band: &[f32], per_second: f64) {
    if !tempo.is_single() {
        return;
    }
    let Some(segment) = tempo.segments.first().copied() else {
        return;
    };
    let period = per_second * 60.0 / segment.bpm;
    if !(period.is_finite() && period >= 2.0) {
        return;
    }
    let phase =
        ((segment.first_beat_frame as f64 - BLOCK as f64 / 2.0) / HOP as f64).rem_euclid(period);
    let (on, halfway) = beat_against_halfway(band, period, phase);
    if !(on.is_finite() && halfway.is_finite()) || halfway <= on * 2.0 {
        return;
    }
    let shifted = phase + period * 0.5;
    let first = shifted * HOP as f64 + BLOCK as f64 / 2.0;
    if !(first.is_finite() && first >= 0.0) {
        return;
    }
    tempo.segments = vec![TempoSegment {
        bpm: segment.bpm,
        first_beat_frame: first.round() as u64,
        beat: 0,
    }];
}

/// The tempo and the anchor of a single-tempo grid, settled over the whole track to a fraction of a hop.
///
/// The periods before this one are measured on windows and on hops: a hop is 5,8 ms at 44,1 kHz and the
/// comb that chose the tempo resolves its period to about a hundredth of one, which is a sixteenth of a
/// millisecond per beat — and a hundredth of a millisecond per beat is a third of a second over a
/// five-minute track. A grid a DJ sets by hand is exact: the lines sit on the transients from the first bar
/// to the last, and that is the standard this has to meet.
///
/// So the search is on parts per million of the period, over the whole track, in three passes: a wide one
/// to find the valley the windowed answer is near, a narrow one to fall into it, and a fine one to settle
/// at the bottom. Each pass asks the same question — how much of the envelope sits on the grid — and at
/// each period it also chooses the phase, because a period that is wrong by a hair fits best at a phase
/// that is wrong by a hair too.
fn settle_tempo(envelope: &[f32], per_second: f64, bpm: f64, first: f64) -> Option<(f64, f64)> {
    let mut period = per_second * 60.0 / bpm;
    if !(period.is_finite() && period >= 2.0) {
        return None;
    }
    let mut phase = (first - BLOCK as f64 / 2.0) / HOP as f64;
    // A phase measured before the first block is the same lattice: only the offset inside the period counts.
    phase = phase.rem_euclid(period);
    for (span, step) in [(800.0f64, 40.0f64), (40.0, 4.0), (4.0, 0.4)] {
        let mut best = (f64::NEG_INFINITY, period, phase);
        let mut delta = -span;
        while delta <= span {
            let candidate = period * (1.0 + delta * 1e-6);
            let (found, score) = best_phase_and_score(envelope, candidate);
            if score > best.0 {
                best = (score, candidate, found);
            }
            delta += step;
        }
        if !best.0.is_finite() {
            return None;
        }
        period = best.1;
        phase = best.2;
    }
    // The phase to a fiftieth of a hop: the passes above chose it on a quarter-hop grid.
    let mut best = (f64::NEG_INFINITY, phase);
    for index in -32..=32 {
        let candidate = (phase + f64::from(index) * 0.02).rem_euclid(period);
        let score = comb_score(envelope, period, candidate);
        if score > best.0 {
            best = (score, candidate);
        }
    }
    if !best.0.is_finite() {
        return None;
    }
    Some((
        per_second * 60.0 / period,
        best.1 * HOP as f64 + BLOCK as f64 / 2.0,
    ))
}

/// Read the head of a file as mono and measure its tempo.
pub fn analyse(path: &std::path::Path) -> Result<Option<Tempo>, MediaError> {
    let mut decoder = StreamDecoder::open(path)?;
    let rate = decoder.sample_rate();
    // The whole file. A four-minute cap was a guess about how long a track is, and on a six-minute acoustic
    // take it cut the measurement in half: the first 240 s said 60,7 BPM with a share too low to publish,
    // while all 379 s said 138,5 with twice the evidence. The decode is one pass and the envelope is small
    // (71 000 frames for six minutes), so there is nothing to gain by stopping early.
    let mut buffer = vec![0.0f32; 4096 * 2];
    let mut mono = Vec::with_capacity(1 << 20);
    loop {
        let read = decoder.read_frames(&mut buffer)?;
        if read == 0 {
            break;
        }
        for frame in 0..read {
            mono.push((buffer[frame * 2] + buffer[frame * 2 + 1]) * 0.5);
        }
    }
    Ok(detect(&mono, f64::from(rate)))
}
