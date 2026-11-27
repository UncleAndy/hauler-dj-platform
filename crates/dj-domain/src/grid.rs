//! The beat grid: where the beats are, as a map both ways between frames and beats (DJP-SYNC-003).
//!
//! One tempo for a whole track is the common case, not the only one: an edit can cut into a new tempo,
//! a remix can change it between sections, and a live take drifts. So the grid is a list of segments —
//! each one a tempo and the frame its first beat sits on — and every question about beats is asked of
//! it: which beat is at this frame, where is this beat, what is the tempo here. One segment is exactly
//! the old `bpm` + `first_beat_frame` pair, so a track with a single tempo reads and behaves as before.
//!
//! Segments are pinned by their own numbers rather than by a `first_beat_frame` per stretch: a cut
//! between two tempos is a jump in the frame of a beat, not a new grid with its own zero, and the beat
//! count has to keep running across it or every later bar number is wrong.

use serde::{Deserialize, Serialize};

/// One stretch of a track at one tempo.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BeatSegment {
    /// Frame of this segment's first beat, in source frames.
    pub frame: u64,
    /// Beat number of that frame, counted from the track's first beat. Fractional never: a segment
    /// starts on a beat.
    pub beat: u64,
    /// Beats per minute in this segment.
    pub bpm: f64,
}

/// The beat grid of a track: a monotone map from beats to frames, in segments of one tempo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BeatGrid {
    /// Segments, sorted by frame and by beat. Never empty.
    segments: Vec<BeatSegment>,
    /// Beats in a bar, as the measurement counted them.
    ///
    /// A bar is what four of them make a square of, and what the strip counts in. It is measured from
    /// the accents of the low band, which is the only band that has them (DJP-MEDIA-006); a file that
    /// does not say, and every file written before this was measured, is read as four.
    #[serde(default = "four_beats_to_the_bar")]
    beats_per_bar: u8,
}

/// A bar of four beats: the assumption of every grid that has no measurement behind it.
fn four_beats_to_the_bar() -> u8 {
    4
}

/// A frame scaled away from an anchor, refusing a negative or absurd result.
fn rescale_frame(frame: u64, anchor: u64, scale: f64) -> Option<u64> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let offset = (frame as f64 - anchor as f64) * scale;
    let moved = anchor as f64 + offset;
    if !moved.is_finite() || moved < 0.0 || moved > u64::MAX as f64 {
        return None;
    }
    Some(moved.round() as u64)
}

impl BeatGrid {
    /// A grid of one tempo: the shape every measured track has today.
    ///
    /// The frame is where beat zero sits; a track whose first beat was never found has no grid at all
    /// rather than a grid at zero, because those are different claims.
    pub fn single(bpm: f64, first_beat_frame: u64) -> Option<Self> {
        Self::from_segments(vec![BeatSegment {
            frame: first_beat_frame,
            beat: 0,
            bpm,
        }])
    }

    /// A grid from explicit segments, refused when the numbers cannot describe a track.
    ///
    /// An anchor that does not move forwards is not a correction but a mistake: frames and beats both
    /// have to grow strictly, and a tempo has to be a positive number. Refusing here means the rest of
    /// the program never has to wonder whether a grid makes sense.
    pub fn from_segments(segments: Vec<BeatSegment>) -> Option<Self> {
        if segments.is_empty() {
            return None;
        }
        Self::rebuilt(segments, four_beats_to_the_bar())
    }

    /// A grid built from segments and a bar length, both already checked for shape.
    fn rebuilt(segments: Vec<BeatSegment>, beats_per_bar: u8) -> Option<Self> {
        let mut previous: Option<BeatSegment> = None;
        for segment in &segments {
            if !segment.bpm.is_finite() || segment.bpm <= 0.0 {
                return None;
            }
            if let Some(before) = previous
                && (segment.frame <= before.frame || segment.beat <= before.beat)
            {
                return None;
            }
            previous = Some(*segment);
        }
        Some(Self {
            segments,
            beats_per_bar: if (2..=8).contains(&beats_per_bar) {
                beats_per_bar
            } else {
                four_beats_to_the_bar()
            },
        })
    }

    /// Beats in a bar: four unless the measurement counted otherwise.
    pub fn beats_per_bar(&self) -> u8 {
        self.beats_per_bar
    }

    /// Beats in one square: four bars, as the DJ counts them.
    pub fn square_beats(&self) -> u64 {
        4 * u64::from(self.beats_per_bar)
    }

    /// The same grid counted in another bar length, for a track the measurement found in three.
    pub fn with_beats_per_bar(mut self, beats: u8) -> Self {
        if (2..=8).contains(&beats) {
            self.beats_per_bar = beats;
        }
        self
    }

    /// The segments, in track order.
    pub fn segments(&self) -> &[BeatSegment] {
        &self.segments
    }

    /// Whether the track holds one tempo throughout.
    pub fn is_single(&self) -> bool {
        self.segments.len() == 1
    }

    /// Frame of the first beat of the grid.
    pub fn first_beat_frame(&self) -> u64 {
        self.segments[0].frame
    }

    /// The tempo of the first segment: what a track list shows without knowing where the play head is.
    pub fn bpm(&self) -> f64 {
        self.segments[0].bpm
    }

    /// The tempo that holds the most of the track.
    ///
    /// A list has one column for the tempo, and a track whose sections disagree needs a number that stands for the whole
    /// of it: the stretch that plays longest is the honest one (owner's rule, 26.11). Weight is time, taken from the
    /// frames a stretch covers — from its first beat to the next stretch's, and for the last one to the file's end,
    /// `end_frame`.
    ///
    /// A caller that does not know the length leaves the last stretch without weight, so the answer comes from the
    /// stretches that do have one. That is a worse answer than a file's own length gives, and never a wrong one: it is
    /// still a tempo the track really plays.
    pub fn dominant_bpm(&self, end_frame: Option<u64>) -> f64 {
        let weight_of = |index: usize| -> f64 {
            let segment = self.segments[index];
            match self.segments.get(index + 1) {
                Some(next) => next.frame.saturating_sub(segment.frame) as f64,
                None => end_frame.map_or(0.0, |end| end.saturating_sub(segment.frame) as f64),
            }
        };
        let mut best = 0;
        for index in 1..self.segments.len() {
            // Strictly greater: a tie goes to the stretch the track starts with.
            if weight_of(index) > weight_of(best) {
                best = index;
            }
        }
        self.segments[best].bpm
    }

    /// Frames per beat in a segment, at a sample rate.
    fn frames_per_beat(segment: &BeatSegment, rate: f64) -> f64 {
        rate * 60.0 / segment.bpm
    }

    /// Frames in one square of four bars: the period a grid repeats with, so the amount the anchor may
    /// be moved by without changing a single line.
    fn square_frames(&self, segment: &BeatSegment, rate: f64) -> f64 {
        self.square_beats() as f64 * Self::frames_per_beat(segment, rate)
    }

    /// The segment a frame falls in, or the first one when the frame is before the grid starts.
    fn segment_at(&self, frame: u64) -> &BeatSegment {
        // The list is short — one segment for a track that holds its tempo, a handful for an edit — so
        // a walk from the end beats a binary search for the sizes we actually store.
        for segment in self.segments.iter().rev() {
            if frame >= segment.frame {
                return segment;
            }
        }
        &self.segments[0]
    }

    /// The segment a beat falls in, or the first one when the beat is before the grid starts.
    fn segment_of_beat(&self, beat: f64) -> &BeatSegment {
        for segment in self.segments.iter().rev() {
            if beat >= segment.beat as f64 {
                return segment;
            }
        }
        &self.segments[0]
    }

    /// Which beat is at this frame, as a fraction: `0.5` is halfway between the first and second beat.
    ///
    /// Before the grid starts the answer is negative, which is how a client tells "before the first
    /// beat" from "on it".
    pub fn beat_at(&self, frame: u64, rate: f64) -> f64 {
        if rate <= 0.0 {
            return 0.0;
        }
        let segment = self.segment_at(frame);
        let per_beat = Self::frames_per_beat(segment, rate);
        segment.beat as f64 + (frame as f64 - segment.frame as f64) / per_beat
    }

    /// Where a beat sits, in frames, as a fraction: the inverse of [`Self::beat_at`] inside a segment.
    pub fn frame_of_beat(&self, beat: f64, rate: f64) -> f64 {
        if rate <= 0.0 {
            return 0.0;
        }
        let segment = self.segment_of_beat(beat);
        let per_beat = Self::frames_per_beat(segment, rate);
        segment.frame as f64 + (beat - segment.beat as f64) * per_beat
    }

    /// The tempo at this frame: the segment the play head is in, which is what a deck shows and what
    /// sync has to follow.
    pub fn bpm_at(&self, frame: u64) -> f64 {
        self.segment_at(frame).bpm
    }

    /// The same grid, moved along the track by `frames` (positive is later).
    ///
    /// The DJ hears the beat a few milliseconds off and says so; the fix is one number, and the whole
    /// grid moves — every segment, so a track that changes tempo keeps its shape.
    ///
    /// The shift has no wall at the start of the file. A track whose first beat sits half a second in
    /// runs out of frames after one press of "one beat earlier", and the DJ has not finished asking:
    /// the grid is a set of lines, and a set that repeats every square of four bars is described as
    /// well from any of its squares. So when the anchor would walk below zero, the whole grid is
    /// rebased by whole squares — frames and beats move together, every frame keeps the beat it had —
    /// and the lines carry on moving. The DJ sees the picture move, not the anchor jump.
    pub fn shifted(&self, frames: i64, rate: f64) -> Option<Self> {
        if !rate.is_finite() || rate <= 0.0 {
            return None;
        }
        let first = self.segments.first()?;
        let per_beat = Self::frames_per_beat(first, rate);
        if !per_beat.is_finite() || per_beat <= 0.0 {
            return None;
        }
        // Whole squares that keep the anchor inside the file after the shift: one square is sixteen
        // beats, which is also four bars, so rebasing never changes which line is a bar or a square.
        let below = -(first.frame as f64 + frames as f64);
        let square = self.square_beats() as f64 * per_beat;
        let squares = if below > 0.0 {
            (below / square).ceil()
        } else {
            0.0
        };
        if !squares.is_finite() || squares < 0.0 {
            return None;
        }
        let beat_lift = u64::try_from(squares as u128 * u128::from(self.square_beats())).ok()?;
        let mut moved: Vec<BeatSegment> = Vec::with_capacity(self.segments.len());
        for segment in &self.segments {
            let lift = squares * self.square_frames(segment, rate);
            let target = segment.frame as f64 + frames as f64 + lift;
            if !target.is_finite() || target < 0.0 || target > u64::MAX as f64 {
                return None;
            }
            moved.push(BeatSegment {
                frame: target.round() as u64,
                beat: segment.beat.checked_add(beat_lift)?,
                bpm: segment.bpm,
            });
        }
        Self::rebuilt(moved, self.beats_per_bar)
    }

    /// The same grid read at a new tempo, with every beat where the music puts it.
    ///
    /// The first beat stays; the rest of the grid is stretched around it by the ratio of the old tempo
    /// to the new one, so a DJ who says "this is 128, not 124" gets the beats he hears, and one who says
    /// "half" gets every other beat of the same grid.
    pub fn retimed(&self, bpm: f64) -> Option<Self> {
        let first = self.segments.first()?;
        if !bpm.is_finite() || bpm <= 0.0 || !first.bpm.is_finite() || first.bpm <= 0.0 {
            return None;
        }
        let scale = first.bpm / bpm;
        let mut retimed: Vec<BeatSegment> = Vec::with_capacity(self.segments.len());
        for segment in &self.segments {
            retimed.push(BeatSegment {
                frame: rescale_frame(segment.frame, first.frame, scale)?,
                beat: segment.beat,
                bpm: segment.bpm * bpm / first.bpm,
            });
        }
        Self::rebuilt(retimed, self.beats_per_bar)
    }

    /// The grid as it is stored beside the music: the first segment becomes `bpm` and
    /// `first_beat_frame`, the rest become anchors.
    pub fn stored_anchors(&self) -> Vec<(u64, u64, f64)> {
        self.segments
            .iter()
            .skip(1)
            .map(|segment| (segment.frame, segment.beat, segment.bpm))
            .collect()
    }

    /// The stretch that plays at this frame, as a number in track order.
    ///
    /// A frame before the first beat belongs to the first stretch: the grid is drawn both ways from its
    /// anchor, and the stretch the DJ is looking at is the first one.
    pub fn stretch_at(&self, frame: u64) -> usize {
        let mut found = 0;
        for (index, segment) in self.segments.iter().enumerate() {
            if segment.frame <= frame {
                found = index;
            }
        }
        found
    }

    /// Move the stretch that plays at this frame by `frames`, leaving the others where they are.
    ///
    /// The stretch's own anchor is what moves, and it has to stay between its neighbours: a section that
    /// overtakes the one after it is not a correction but a reordering.
    pub fn with_stretch_shift(&self, frame: u64, frames: i64) -> Option<Self> {
        if frames == 0 {
            return Some(self.clone());
        }
        let index = self.stretch_at(frame);
        let segment = self.segments[index];
        let moved = i64::try_from(segment.frame).ok()? + frames;
        if moved < 0 {
            return None;
        }
        let moved = moved as u64;
        if index > 0 && moved <= self.segments[index - 1].frame {
            return None;
        }
        if let Some(next) = self.segments.get(index + 1)
            && moved >= next.frame
        {
            return None;
        }
        let mut segments = self.segments.clone();
        segments[index].frame = moved;
        Self::rebuilt(segments, self.beats_per_bar)
    }

    /// Put the nearest beat of the stretch at this frame onto the frame itself.
    ///
    /// What moves is the stretch under the play head; the beat count is untouched, because the lines are
    /// what is being placed and not the counting.
    pub fn with_stretch_snap(&self, frame: u64, rate: f64) -> Option<Self> {
        if !(rate.is_finite() && rate > 0.0) {
            return None;
        }
        let beat = self.beat_at(frame, rate).round();
        let placed = self.frame_of_beat(beat, rate);
        if !placed.is_finite() {
            return None;
        }
        let offset = (frame as f64 - placed).round() as i64;
        self.with_stretch_shift(frame, offset)
    }

    /// Read the stretch that plays at this frame at `factor` times its tempo.
    pub fn with_stretch_level(&self, frame: u64, factor: f64) -> Option<Self> {
        if !(factor.is_finite() && factor > 0.0) {
            return None;
        }
        let index = self.stretch_at(frame);
        self.with_stretch_tempo(frame, self.segments[index].bpm * factor)
    }

    /// Split the stretch that plays at this frame: a new section starts there, at the tempo it has.
    ///
    /// Refused when there is nothing to split — the frame is on the stretch's own first beat — because a
    /// section with no length describes nothing.
    pub fn split_at(&self, frame: u64, rate: f64) -> Option<Self> {
        let index = self.stretch_at(frame);
        let stretch = self.segments[index];
        if frame <= stretch.frame || !(rate.is_finite() && rate > 0.0) {
            return None;
        }
        let beat = self.beat_at(frame, rate).round() as u64;
        if beat <= stretch.beat {
            return None;
        }
        let mut segments = self.segments.clone();
        segments.insert(
            index + 1,
            BeatSegment {
                frame,
                beat,
                bpm: stretch.bpm,
            },
        );
        Self::rebuilt(segments, self.beats_per_bar)
    }

    /// Drop the stretch that plays at this frame, merging it into the one before it.
    ///
    /// The first stretch is refused: the grid has to describe the beginning of the track.
    pub fn without_stretch_at(&self, frame: u64) -> Option<Self> {
        if self.segments.len() < 2 {
            return None;
        }
        let index = self.stretch_at(frame);
        if index == 0 {
            return None;
        }
        let mut segments = self.segments.clone();
        segments.remove(index);
        Self::rebuilt(segments, self.beats_per_bar)
    }

    /// Give the stretch that plays at this frame this tempo, leaving the others alone.
    pub fn with_stretch_tempo(&self, frame: u64, bpm: f64) -> Option<Self> {
        if !(bpm.is_finite() && bpm > 0.0) {
            return None;
        }
        let index = self.stretch_at(frame);
        let mut segments = self.segments.clone();
        segments[index].bpm = bpm;
        Self::rebuilt(segments, self.beats_per_bar)
    }

    /// A grid from what a file holds: a tempo, the first beat, and the anchors that pin the rest.
    ///
    /// An anchor only says where a beat is, so the tempo of the stretch it starts comes from the anchor
    /// after it — that is what "the grid passes through both" means. The last stretch keeps the tempo
    /// the file states, which is also the tempo a file without anchors has throughout.
    pub fn from_stored(
        bpm: f64,
        first_beat_frame: u64,
        anchors: &[(u64, u64, Option<f64>)],
        rate: f64,
        beats_per_bar: u8,
    ) -> Option<Self> {
        let mut segments = vec![BeatSegment {
            frame: first_beat_frame,
            beat: 0,
            bpm,
        }];
        // Which stretches state their own tempo. A file written before anchors had a tempo states one
        // tempo for the whole grid and lets the stretches be read out of the pairs of points; a file
        // written by this build states each stretch's tempo, and then the first stretch's own tempo is the
        // one in the file, not something derived from the first anchor — deriving it there was what made
        // a saved 93 BPM grid read back as 192 after a restart.
        let stated_any = anchors
            .iter()
            .any(|(_, _, own)| own.is_some_and(|value| value.is_finite() && value > 0.0));
        let mut stated = vec![stated_any];
        for (frame, beat, own) in anchors {
            let own = own.filter(|value| value.is_finite() && *value > 0.0);
            stated.push(own.is_some());
            segments.push(BeatSegment {
                frame: *frame,
                beat: *beat,
                bpm: own.unwrap_or(0.0),
            });
        }
        for index in 0..segments.len() {
            if stated[index] {
                continue;
            }
            let length = match segments.get(index + 1) {
                Some(next) => {
                    let frames = next.frame.saturating_sub(segments[index].frame) as f64;
                    let beats = next.beat.saturating_sub(segments[index].beat) as f64;
                    if frames <= 0.0 || beats <= 0.0 || rate <= 0.0 {
                        return None;
                    }
                    beats * 60.0 * rate / frames
                }
                // The last stretch has no next anchor to measure against: it keeps the stated tempo,
                // and so does a segment whose own bpm the file gave.
                None => bpm,
            };
            segments[index].bpm = length;
        }
        Self::rebuilt(segments, beats_per_bar)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{BeatGrid, BeatSegment};

    /// Five minutes at 128, then a second stretch at 132: one number has to stand for the track, and time decides.
    fn two_tempos() -> BeatGrid {
        BeatGrid::from_segments(vec![
            BeatSegment {
                frame: 0,
                beat: 0,
                bpm: 128.0,
            },
            BeatSegment {
                frame: 5 * 60 * 48_000,
                beat: 640,
                bpm: 132.0,
            },
        ])
        .expect("a grid of two tempos")
    }

    #[test]
    fn the_tempo_that_holds_most_of_the_track_is_the_one_a_list_shows() {
        let grid = two_tempos();
        assert_eq!(
            grid.dominant_bpm(Some(8 * 60 * 48_000)),
            128.0,
            "five minutes against three: the first stretch holds the track"
        );
        assert_eq!(
            grid.dominant_bpm(Some(12 * 60 * 48_000)),
            132.0,
            "seven minutes against five: now the second one does"
        );
        assert_eq!(
            grid.dominant_bpm(Some(10 * 60 * 48_000)),
            128.0,
            "a tie goes to the stretch the track starts with"
        );
        assert_eq!(
            grid.dominant_bpm(Some(5 * 60 * 48_000)),
            128.0,
            "a last stretch with no time at all does not win"
        );
        assert_eq!(
            grid.dominant_bpm(None),
            128.0,
            "without the file's length the last stretch has no weight, and the answer is still a tempo the track plays"
        );
        assert_eq!(
            BeatGrid::single(126.0, 0)
                .expect("one tempo")
                .dominant_bpm(None),
            126.0,
            "a track of one tempo answers with it"
        );
    }
}
