//! The journal of plays (DJP-MEDIA-004).
//!
//! A play is a *continuous interval of playing*: it opens when a deck starts playing and closes when it
//! stops. Loops, jumps and tempo changes do not close it — those are things a DJ does while a track
//! plays, not reasons to count the play twice. Intervals shorter than the owner's threshold
//! (`library.history_min_seconds`, thirty seconds by default) are dropped: a two-second listen is not
//! a play.
//!
//! The clock is the audio clock the snapshot carries, not the wall clock: what the DJ heard is what is
//! counted, and a stalled publisher cannot inflate it.

use std::time::{SystemTime, UNIX_EPOCH};

use dj_domain::PlayRow;

/// How many finished plays the tracker keeps for the wire, newest last.
pub const RECENT_LIMIT: usize = 8;

/// The longest clock step one observation may count, in seconds.
///
/// The publisher runs at the state rate, so an honest step is a few milliseconds, and on a busy machine
/// it can be a second or two. A step of several seconds means the engine itself was not running — a
/// suspend, a debugger, a stopped process — and the deck is not known to have played through it, so the
/// interval does not grow by more than this.
const MAX_STEP_SECONDS: f64 = 3.0;

/// A play that is still going.
#[derive(Debug, Clone, PartialEq)]
struct OpenPlay {
    fingerprint: String,
    path: String,
    started_at_ms: u64,
    played_frames: u64,
    last_clock: u64,
}

/// The journal: one open interval per deck, the plays waiting for the writer, the last few for the wire.
#[derive(Debug)]
pub struct History {
    open: Vec<Option<OpenPlay>>,
    finished: Vec<PlayRow>,
    recent: Vec<PlayRow>,
    min_seconds: f64,
}

impl History {
    /// A journal that records plays of at least `min_seconds`.
    pub fn new(min_seconds: u32) -> Self {
        Self {
            open: Vec::new(),
            finished: Vec::new(),
            recent: Vec::new(),
            min_seconds: f64::from(min_seconds),
        }
    }

    /// Follow one deck for one snapshot.
    ///
    /// `print` is asked for the file's key only when an interval opens, so a play costs one fingerprint
    /// rather than one per snapshot.
    pub fn observe(
        &mut self,
        deck: usize,
        playing: bool,
        path: Option<&str>,
        clock: u64,
        sample_rate: u32,
        print: &dyn Fn(&str) -> Option<String>,
    ) {
        if self.open.len() <= deck {
            self.open.resize(deck + 1, None);
        }
        let rate = f64::from(sample_rate.max(1));
        // The deck stopped, or it is playing something else now: whatever was open ends here.
        let another_track = match (&self.open[deck], path) {
            (Some(open), Some(path)) => open.path != path,
            _ => false,
        };
        if (!playing || another_track)
            && let Some(open) = self.open[deck].take()
        {
            self.close(deck, open, clock, rate);
        }
        if !playing {
            return;
        }
        let Some(path) = path else {
            return;
        };
        if self.open[deck].is_none() {
            let Some(fingerprint) = print(path) else {
                return;
            };
            self.open[deck] = Some(OpenPlay {
                fingerprint,
                path: path.to_string(),
                started_at_ms: now_ms(),
                played_frames: 0,
                last_clock: clock,
            });
            return;
        }
        if let Some(open) = self.open[deck].as_mut() {
            open.played_frames += step(open.last_clock, clock, rate);
            open.last_clock = clock;
        }
    }

    /// Close every open interval: the engine is leaving, and a play that was not closed is a play lost.
    pub fn close_all(&mut self, clock: u64, sample_rate: u32) -> Vec<PlayRow> {
        let rate = f64::from(sample_rate.max(1));
        for deck in 0..self.open.len() {
            if let Some(open) = self.open[deck].take() {
                self.close(deck, open, clock, rate);
            }
        }
        self.take_finished()
    }

    /// The plays finished since the last call: the writer's half of the journal.
    pub fn take_finished(&mut self) -> Vec<PlayRow> {
        std::mem::take(&mut self.finished)
    }

    /// The last few plays, for the wire.
    pub fn recent(&self) -> Vec<PlayRow> {
        self.recent.clone()
    }

    fn close(&mut self, deck: usize, open: OpenPlay, clock: u64, rate: f64) {
        let frames = open.played_frames + step(open.last_clock, clock, rate);
        let seconds = frames as f64 / rate;
        if seconds + 1e-9 < self.min_seconds {
            return;
        }
        let row = PlayRow {
            fingerprint: open.fingerprint,
            path: open.path,
            deck: deck.min(255) as u8,
            started_at_ms: open.started_at_ms,
            seconds,
        };
        self.recent.push(row.clone());
        if self.recent.len() > RECENT_LIMIT {
            self.recent.remove(0);
        }
        self.finished.push(row);
    }
}

/// How many frames passed, with a stalled publisher not counted as playing.
fn step(from: u64, to: u64, rate: f64) -> u64 {
    to.saturating_sub(from)
        .min((MAX_STEP_SECONDS * rate) as u64)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// A tick of `seconds`, and the fake fingerprint: every path is its own key.
    fn key(path: &str) -> Option<String> {
        Some(path.to_string())
    }

    fn tick(history: &mut History, deck: usize, playing: bool, path: Option<&str>, clock: u64) {
        history.observe(deck, playing, path, clock, RATE, &key);
    }

    #[test]
    fn a_short_play_is_not_recorded() {
        let mut history = History::new(30);
        tick(&mut history, 0, true, Some("/a.wav"), 0);
        tick(&mut history, 0, true, Some("/a.wav"), 10 * RATE as u64);
        tick(&mut history, 0, false, Some("/a.wav"), 10 * RATE as u64);
        assert!(
            history.take_finished().is_empty(),
            "ten seconds is not a play"
        );
    }

    #[test]
    fn a_play_long_enough_is_recorded_once() {
        let mut history = History::new(30);
        tick(&mut history, 0, true, Some("/a.wav"), 0);
        for second in 1..=40u64 {
            tick(&mut history, 0, true, Some("/a.wav"), second * RATE as u64);
        }
        tick(&mut history, 0, false, Some("/a.wav"), 40 * RATE as u64);
        let plays = history.take_finished();
        assert_eq!(plays.len(), 1);
        assert!(
            (plays[0].seconds - 40.0).abs() < 1e-6,
            "{}",
            plays[0].seconds
        );
        assert_eq!(plays[0].fingerprint, "/a.wav");
        assert!(
            history.take_finished().is_empty(),
            "a play is handed over once"
        );
    }

    #[test]
    fn a_pause_ends_the_interval() {
        let mut history = History::new(30);
        tick(&mut history, 0, true, Some("/a.wav"), 0);
        tick(&mut history, 0, true, Some("/a.wav"), 20 * RATE as u64);
        tick(&mut history, 0, false, Some("/a.wav"), 20 * RATE as u64);
        tick(&mut history, 0, true, Some("/a.wav"), 21 * RATE as u64);
        tick(&mut history, 0, true, Some("/a.wav"), 46 * RATE as u64);
        tick(&mut history, 0, false, Some("/a.wav"), 46 * RATE as u64);
        assert!(
            history.take_finished().is_empty(),
            "twenty seconds, a pause and twenty-five is two short listens, not a play"
        );
    }

    #[test]
    fn a_stalled_publisher_does_not_inflate_the_interval() {
        let mut history = History::new(30);
        tick(&mut history, 0, true, Some("/a.wav"), 0);
        // The publisher was away for ten minutes: the deck's own clock says so, but nothing says the
        // deck played through it.
        tick(&mut history, 0, true, Some("/a.wav"), 600 * RATE as u64);
        tick(&mut history, 0, false, Some("/a.wav"), 600 * RATE as u64);
        assert!(
            history.take_finished().is_empty(),
            "a gap in the publisher is not playing time"
        );
    }

    #[test]
    fn another_track_on_the_same_deck_closes_the_first() {
        let mut history = History::new(1);
        tick(&mut history, 0, true, Some("/a.wav"), 0);
        tick(&mut history, 0, true, Some("/b.wav"), 5 * RATE as u64);
        tick(&mut history, 0, false, Some("/b.wav"), 6 * RATE as u64);
        let plays = history.take_finished();
        assert_eq!(plays.len(), 2, "a load without a stop is two plays");
        assert_eq!(plays[0].path, "/a.wav");
        assert_eq!(plays[1].path, "/b.wav");
    }

    #[test]
    fn closing_the_journal_records_what_is_still_open() {
        let mut history = History::new(1);
        tick(&mut history, 1, true, Some("/c.wav"), 0);
        // The publisher ticks every few milliseconds; a second at a time is already a coarse stand-in
        // for that, and staying under the stall window keeps the test about closing, not about gaps.
        for second in 1..=5u64 {
            tick(&mut history, 1, true, Some("/c.wav"), second * RATE as u64);
        }
        let plays = history.close_all(5 * RATE as u64, RATE);
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].deck, 1);
        assert!((plays[0].seconds - 5.0).abs() < 1e-6);
    }
}
