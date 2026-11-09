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

/// How much of the file the detector reads, in seconds. Four minutes covers every intro and breakdown
/// a track is likely to open with, and a DJ waits for the number.
pub const ANALYSED_SECONDS: f64 = 240.0;

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
    if let Some(kick) = kick_envelope(mono, rate) {
        on_the_bar(&kick, rate, &mut tempo);
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
    let stretches = stretches_of(envelope, per_second, bpm);
    if stretches.len() > 1
        && let Some(changing) = grid_of_stretches(envelope, per_second, bpm, &stretches)
    {
        return Some(changing);
    }
    single_tempo(envelope, per_second, bpm, &clear)
}

/// The grid of a track that holds one tempo: the old answer, unchanged.
fn single_tempo(
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

/// Whether two tempos are the same beat, read the same way.
fn same_tempo(a: f64, b: f64) -> bool {
    a > 0.0 && b > 0.0 && ((a / b).log2()).abs() <= TEMPO_TOLERANCE
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
fn stretches_of(envelope: &[f32], per_second: f64, dominant: f64) -> Vec<Stretch> {
    let window = (CHANGE_WINDOW_SECONDS * per_second) as usize;
    let hop = (CHANGE_HOP_SECONDS * per_second).max(1.0) as usize;
    if window < 64 || envelope.len() < window * (CHANGE_WINDOWS + 1) {
        return Vec::new();
    }
    let mut stretches: Vec<Stretch> = Vec::new();
    let mut start = 0;
    while start + window <= envelope.len() {
        let reading = tempo_of_window(&envelope[start..start + window], per_second)
            .map(|bpm| snap_octave(bpm, dominant));
        match (reading, stretches.last_mut()) {
            (Some(bpm), Some(last)) if same_tempo(bpm, last.bpm) => {
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
    stretches.retain(|stretch| stretch.windows >= CHANGE_WINDOWS);
    // Whatever was dropped belongs to a neighbour: the spans are laid end to end, so the grid covers
    // the track instead of leaving stretches of it without one.
    for index in 0..stretches.len() {
        if index > 0 {
            stretches[index].start = stretches[index - 1].end;
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
        if !segments.is_empty() && dominant_period > 0.0 {
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
        // Between clusters the prior has its say: it cannot make a weak reading win, only settle a tie.
        let cluster_bpm = moment / weight;
        let settled = weight * tempo_prior(cluster_bpm);
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

/// Place the beats on the kicks and fold them into a bar of `metre` beats, returning the phase and it.
///
/// The phase is searched over one whole period in six hundred steps: the beat has to sit on the kick,
/// and where in the period it sits is the one thing a comb can settle exactly. The bar is the fold of
/// those beats — the loudest of them over the average one is the accent, and its slot is the downbeat.
fn bar_of_kick(kick: &[f32], period_hops: f64, metre: u8) -> Option<(f64, BarGuess)> {
    if !(period_hops.is_finite() && period_hops >= 1.0) {
        return None;
    }
    let end = kick.len() as f64 - 1.0;
    let everywhere: f64 =
        kick.iter().map(|value| f64::from(*value)).sum::<f64>() / kick.len() as f64;
    if everywhere <= 0.0 {
        return None;
    }
    let mut phase = 0.0;
    let mut best = f64::NEG_INFINITY;
    let mut beats = 0usize;
    for step in 0..600 {
        let guess = step as f64 * period_hops / 600.0;
        let mut sum = 0.0;
        let mut count = 0usize;
        let mut position = guess;
        while position < end {
            sum += f64::from(kick[position.round() as usize]);
            count += 1;
            position += period_hops;
        }
        if sum > best {
            best = sum;
            beats = count.max(1);
            phase = guess;
        }
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
    Some((
        phase,
        BarGuess {
            metre,
            downbeat,
            contrast: peak / average,
            on_the_beats: best / beats as f64 / everywhere,
        },
    ))
}

/// The tempo, the phase and the bar the kick band agrees with.
///
/// The spectrum's own answer is kept whenever the accents divide by it: a track whose grid is right is
/// not moved because another reading also has a bar. Only when no accent period is a whole number of
/// its beats is the grid wrong in principle — a beat that lands a bar and a third apart from the accents
/// is a subdivision of them, not their beat — and then the answer is the tempo an accent period implies,
/// the strongest period first.
fn bar_tempo_of(kick: &[f32], per_second: f64, bpm: f64) -> Option<(f64, f64, BarGuess)> {
    let lags = accent_lags(kick, per_second);
    let own_period = 60.0 / bpm;
    if let Some(metre) = lags.iter().find_map(|(lag, _)| metre_of(*lag, own_period)) {
        let (phase, bar) = bar_of_kick(kick, per_second * own_period, metre)?;
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
            let Some((phase, bar)) = bar_of_kick(kick, period * per_second, metre) else {
                continue;
            };
            if bar.on_the_beats < KICK_ON_BEATS {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|(_, _, _, score)| *strength > *score)
            {
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
fn on_the_bar(kick: &[f32], rate: f64, tempo: &mut Tempo) {
    // A track with several tempos has a bar per stretch, and turning all of them by the first one's
    // accent is a claim the measurement does not make. Such a track keeps its answer as it is.
    if !tempo.is_single() {
        return;
    }
    let per_second = rate / HOP as f64;
    let Some(segment) = tempo.segments.first().copied() else {
        return;
    };
    let Some((bpm, phase, bar)) = bar_tempo_of(kick, per_second, segment.bpm) else {
        return;
    };
    // The band has to be on the grid and the bar has to have an accent in it. A kick band that carries
    // nothing at the beats is a track whose beat is not a kick — a pad, a string line, a snare-led
    // pattern — and such a track keeps the answer the spectrum gave it, downbeat and all.
    if bar.on_the_beats < KICK_ON_BEATS || bar.contrast < KICK_METRE_MIN {
        return;
    }
    let period = per_second * 60.0 / bpm;
    let beat = phase + bar.downbeat as f64 * period;
    // Half a block, as in `single_tempo`: the block that saw the onset starts a hop before it.
    let first = (beat * HOP as f64 + BLOCK as f64 / 2.0).round();
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

/// Read the head of a file as mono and measure its tempo.
pub fn analyse(path: &std::path::Path) -> Result<Option<Tempo>, MediaError> {
    let mut decoder = StreamDecoder::open(path)?;
    let rate = decoder.sample_rate();
    let limit = (f64::from(rate) * ANALYSED_SECONDS) as usize;
    let mut buffer = vec![0.0f32; 4096 * 2];
    let mut mono = Vec::with_capacity(limit.min(1 << 22));
    while mono.len() < limit {
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
