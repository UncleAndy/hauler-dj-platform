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
}

/// Move a frame by a signed amount, refusing to walk off the start of the file.
fn shift_frame(frame: u64, frames: i64) -> Option<u64> {
    let moved = i128::from(frame) + i128::from(frames);
    u64::try_from(moved).ok()
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
        Some(Self { segments })
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

    /// Frames per beat in a segment, at a sample rate.
    fn frames_per_beat(segment: &BeatSegment, rate: f64) -> f64 {
        rate * 60.0 / segment.bpm
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
    pub fn shifted(&self, frames: i64) -> Option<Self> {
        let mut moved: Vec<BeatSegment> = Vec::with_capacity(self.segments.len());
        for segment in &self.segments {
            moved.push(BeatSegment {
                frame: shift_frame(segment.frame, frames)?,
                beat: segment.beat,
                bpm: segment.bpm,
            });
        }
        Self::from_segments(moved)
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
        Self::from_segments(retimed)
    }

    /// The grid as it is stored beside the music: the first segment becomes `bpm` and
    /// `first_beat_frame`, the rest become anchors.
    pub fn stored_anchors(&self) -> Vec<(u64, u64)> {
        self.segments
            .iter()
            .skip(1)
            .map(|segment| (segment.frame, segment.beat))
            .collect()
    }

    /// A grid from what a file holds: a tempo, the first beat, and the anchors that pin the rest.
    ///
    /// An anchor only says where a beat is, so the tempo of the stretch it starts comes from the anchor
    /// after it — that is what "the grid passes through both" means. The last stretch keeps the tempo
    /// the file states, which is also the tempo a file without anchors has throughout.
    pub fn from_stored(
        bpm: f64,
        first_beat_frame: u64,
        anchors: &[(u64, u64)],
        rate: f64,
    ) -> Option<Self> {
        let mut segments = vec![BeatSegment {
            frame: first_beat_frame,
            beat: 0,
            bpm,
        }];
        for (frame, beat) in anchors {
            segments.push(BeatSegment {
                frame: *frame,
                beat: *beat,
                bpm: 0.0, // filled below from the stretch's own length
            });
        }
        for index in 0..segments.len() {
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
        Self::from_segments(segments)
    }
}
