//! Deck behaviour: transport, loop boundaries, hotcues, display state (A03).

// A failing expectation in a test is a failure message; the workspace forbids them in library code.
#![allow(clippy::expect_used)]

use dj_codec::{PrefetchConfig, Prefetcher};
use dj_deck::{CachedReaderSource, CachedSource, Deck, DeckSource};
use dj_domain::{BeatGrid, DeckId, DeckLoad, FrameRange, SourceFrame, TrackMeta};

/// A source whose sample value encodes its own frame number, so any position is verifiable.
struct RampSource {
    frames: u64,
    position: u64,
}

impl RampSource {
    fn new(frames: u64) -> Self {
        Self {
            frames,
            position: 0,
        }
    }
}

fn ramp(frame: u64) -> f32 {
    ((frame % 20_000) as f32 / 20_000.0) * 0.5
}

impl DeckSource for RampSource {
    fn sample_rate(&self) -> u32 {
        44_100
    }

    fn frames(&self) -> Option<u64> {
        Some(self.frames)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let want = out.len() / 2;
        let available = usize::try_from(self.frames.saturating_sub(self.position)).unwrap_or(0);
        let take = want.min(available);
        for index in 0..take {
            let value = ramp(self.position + index as u64);
            out[index * 2] = value;
            out[index * 2 + 1] = value;
        }
        self.position += take as u64;
        Ok(take)
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.position = frame.min(self.frames.saturating_sub(1));
        Ok(self.position)
    }
}

/// A source whose every sample says which frame it is: `value = frame / 4 000 000`.
///
/// Unlike the ramp above, this one does not repeat, so an output frame can be traced back to exactly one
/// place in the file. Used to ask whether a loop plays the material it says it does.
struct CodeSource {
    frames: u64,
    position: u64,
}

impl DeckSource for CodeSource {
    fn sample_rate(&self) -> u32 {
        44_100
    }
    fn frames(&self) -> Option<u64> {
        Some(self.frames)
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let want = out.len() / 2;
        let available = self.frames.saturating_sub(self.position);
        let take = want.min(usize::try_from(available).unwrap_or(want));
        for index in 0..take {
            #[allow(clippy::cast_precision_loss)]
            let value = ((self.position + index as u64) as f32) / 4_000_000.0;
            out[index * 2] = value;
            out[index * 2 + 1] = value;
        }
        self.position += take as u64;
        Ok(take)
    }
    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.position = frame.min(self.frames.saturating_sub(1));
        Ok(self.position)
    }
}

fn code_deck(frames: u64) -> Deck {
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(44_100);
    deck.attach(
        Box::new(CodeSource {
            frames,
            position: 0,
        }),
        None,
    );
    deck
}

fn deck_with_ramp(frames: u64) -> Deck {
    let mut deck = Deck::new(DeckId::A);
    // The ramp is 44.1 kHz material on a deck that plays 44.1 kHz: these tests are about transport and
    // loops, not about rate conversion, and a conversion nobody asked for would move every position.
    deck.set_output_rate(44_100);
    deck.attach(Box::new(RampSource::new(frames)), None);
    deck
}

/// A deck that plays a 48 kHz device's rate, holding whatever source the test attaches.
fn deck_at_48k(frames: u64, source_rate: u32) -> Deck {
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(48_000);
    deck.attach(
        Box::new(ToneAtRate {
            frames,
            position: 0,
            rate: source_rate,
            hertz: 1_000.0,
        }),
        None,
    );
    deck
}

#[test]
fn a_deck_is_silent_until_the_transport_runs() {
    let mut deck = deck_with_ramp(44_100);
    // Frames are the transport's contract here; key lock is a separate stage with its own delay,
    // so this test asks the vinyl path.
    deck.set_keylock(false);
    let mut block = vec![1.0f32; 512 * 2];
    deck.render(&mut block);
    assert!(
        block.iter().all(|sample| *sample == 0.0),
        "paused deck must be silent"
    );
    assert_eq!(deck.position(), 0);

    assert!(deck.play());
    deck.render(&mut block);
    assert!(block.iter().any(|sample| *sample != 0.0));
    assert_eq!(deck.position(), 512);
}

#[test]
fn a_loop_wraps_at_the_exact_sample() {
    let mut deck = deck_with_ramp(44_100);
    // Frames are the transport's contract here; key lock is a separate stage with its own delay,
    // so this test asks the vinyl path.
    deck.set_keylock(false);
    let region = FrameRange::new(1_000, 3_000).unwrap_or_else(|| panic!("region"));
    deck.set_loop(Some(region));
    deck.play();

    let mut block = vec![0.0f32; 4_000 * 2];
    deck.render(&mut block);

    // 0..3000 plays straight through, then the loop restarts at 1000. The seam itself is faded over
    // two milliseconds (DJP-DECK-005): the tail runs out and the head comes up, so the frames inside
    // that window are not the material at full level — the frames after it are.
    assert!((block[0] - ramp(0)).abs() < 1e-6);
    assert!((block[(3_000 - 96) * 2] - ramp(2_904)).abs() < 1e-6);
    assert!(
        block[3_000 * 2] > 0.0 && block[3_000 * 2] < ramp(1_000) * 0.2,
        "frame 3000 must be the loop start fading in, got {}",
        block[3_000 * 2]
    );
    assert!(
        (block[(3_000 + 96) * 2] - ramp(1_096)).abs() < 1e-6,
        "two milliseconds after the wrap the material is back at full level"
    );
    assert!((block[3_999 * 2] - ramp(1_999)).abs() < 1e-6);
    // And the seam does not step: every frame of the fade is within the material's own step of the last.
    let mut previous = block[(3_000 - 97) * 2];
    for frame in (3_000 - 96)..(3_000 + 96) {
        let step = (block[frame * 2] - previous).abs();
        assert!(step < 0.01, "frame {frame}: the seam steps by {step:.4}");
        previous = block[frame * 2];
    }
    assert_eq!(deck.position(), 2_000);
    assert_eq!(deck.stats().loop_wraps, 1);
}

#[test]
fn switching_loop_mode_off_stops_the_copy_repeating() {
    // The button's loop is the one held in memory, so it is the one that could go on repeating behind a
    // switched-off mode. Switching off must stop that: the copy runs to its end, the track carries on
    // from there, and the switch costs no seek.
    let mut deck = deck_with_ramp(44_100);
    // Frames are the transport's contract here; key lock is a separate stage with its own delay,
    // so this test asks the vinyl path.
    deck.set_keylock(false);
    let region = FrameRange::new(1_000, 2_000).unwrap_or_else(|| panic!("region"));
    deck.set_loop(Some(region));
    assert_eq!(deck.active_loops().len(), 1, "the button's loop is armed");
    deck.seek(1_000);
    deck.play();

    // One pass fills the capture; the next wrap is served from memory.
    let mut pass = vec![0.0f32; 1_000 * 2];
    deck.render(&mut pass);
    assert!(deck.loop_is_buffered(), "the loop is playing from memory");
    assert_eq!(deck.position(), 1_000, "it wrapped once");

    let seeks_before = deck.stats().seeks;
    deck.set_loop_mode(false);
    let mut after = vec![0.0f32; 2_000 * 2];
    deck.render(&mut after);
    for frame in 0..2_000usize {
        let expected = ramp(1_000 + frame as u64);
        assert!(
            (after[frame * 2] - expected).abs() < 1e-6,
            "frame {frame}: the track must run on unbroken after the switch, expected {expected}, got {}",
            after[frame * 2]
        );
    }
    assert_eq!(
        deck.stats().seeks,
        seeks_before,
        "switching the mode off is not a seek"
    );
    assert_eq!(
        deck.position(),
        3_000,
        "the playhead ran past the loop's end"
    );
}

#[test]
fn saving_a_playing_loop_on_a_pad_keeps_the_sound_where_it_is() {
    // Saving a loop is a bookkeeping act. Halfway through a loop played from memory, it must not move
    // the sound: the copy keeps playing, and the loop keeps repeating because it is armed as a pad.
    let mut deck = deck_with_ramp(44_100);
    // Frames are the transport's contract here; key lock is a separate stage with its own delay,
    // so this test asks the vinyl path.
    deck.set_keylock(false);
    let region = FrameRange::new(1_000, 2_000).unwrap_or_else(|| panic!("region"));
    deck.set_loop(Some(region));
    deck.seek(1_000);
    deck.play();

    let mut pass = vec![0.0f32; 1_000 * 2];
    deck.render(&mut pass);
    assert!(deck.loop_is_buffered(), "the loop is playing from memory");
    assert_eq!(deck.position(), 1_000);

    let mut half = vec![0.0f32; 400 * 2];
    deck.render(&mut half);
    assert_eq!(deck.position(), 1_400);

    let seeks_before = deck.stats().seeks;
    assert!(deck.set_pad_loop(3, Some(region)), "the pad takes the loop");
    assert!(deck.loop_region().is_none(), "the loop moved to the pad");

    let mut after = vec![0.0f32; 1_000 * 2];
    deck.render(&mut after);
    // The wrap is where the deck evens out the step of the seam (two milliseconds, DJP-DECK-005): the
    // copy is the material everywhere else, and at the seam itself it must not step.
    // The seam crossfade is the last two milliseconds of the loop: the tail fades out while the head
    // fades in, and the pass after it starts at the head's ninety-seventh frame — the head's first two
    // milliseconds are heard during the fades only. So the material after the wrap is the region shifted
    // by the fade, and inside the fade the sample is a blend of two places, not one.
    let seam = 2_000u64.saturating_sub(1_400) as usize;
    // The fade is two milliseconds at this deck's rate, which is 88 frames at 44,1 kHz, not 96: the test
    // asks the deck instead of assuming.
    let fade_frames = deck.loop_config().seam_frames(44_100);
    let fade = seam - fade_frames;
    for frame in 0..1_000usize {
        let position = 1_400 + frame as u64;
        let wrapped = if position < 2_000 {
            position
        } else {
            1_000 + fade_frames as u64 + (position - 2_000)
        };
        let expected = ramp(wrapped);
        // The two milliseconds before the wrap and the two after it are the fade, and its last frame is
        // a frame or two past the nominal window. Everything outside that is the material, exactly.
        if (fade..seam + fade_frames * 2).contains(&frame) {
            continue;
        }
        assert!(
            (after[frame * 2] - expected).abs() < 1e-6,
            "frame {frame}: saving the loop moved the sound, expected {expected}, got {}",
            after[frame * 2]
        );
    }
    let mut previous = after[(fade - 1) * 2];
    for frame in fade..seam.min(1_000) {
        let step = (after[frame * 2] - previous).abs();
        assert!(
            step < 0.01,
            "frame {frame}: the seam steps by {step:.4} — the copy is cut, not joined"
        );
        previous = after[frame * 2];
    }
    assert_eq!(
        deck.stats().seeks,
        seeks_before,
        "saving a loop must not ask the decoder for a seek"
    );
}

#[test]
fn a_new_loop_replaces_the_one_that_is_not_saved() {
    // The owner's rule: a loop that is not on a pad is not a loop to defend — putting a new one replaces
    // it. Only saved loops are protected, and two of those must never nest (the outer one would wrap
    // before the play head ever reached the inner one).
    let mut deck = deck_with_ramp(44_100);
    let outer = FrameRange::new(1_000, 4_000).unwrap_or_else(|| panic!("outer"));
    let inner = FrameRange::new(2_000, 3_000).unwrap_or_else(|| panic!("inner"));
    let crossing = FrameRange::new(3_000, 5_000).unwrap_or_else(|| panic!("crossing"));
    assert!(deck.set_loop(Some(outer)), "the first loop is taken");
    assert!(
        deck.set_loop(Some(inner)),
        "a new loop replaces a loop that is not saved"
    );
    assert_eq!(deck.loop_region(), Some(inner));

    // Saved, it is protected: a loop that crosses it is refused and nothing changes.
    assert!(
        deck.set_pad_loop(1, Some(outer)),
        "the outer loop is saved to a pad"
    );
    assert!(
        !deck.set_loop(Some(crossing)),
        "a loop crossing a saved one must be refused"
    );
    assert_eq!(
        deck.loop_region(),
        Some(inner),
        "the refused loop changed nothing"
    );
    // The same region on a second pad is not a clash: a loop may live on many pads.
    assert!(deck.set_pad_loop(2, Some(outer)));
    // And a loop over a saved one is refused by length too, not only by region.
    let free = FrameRange::new(6_000, 7_000).unwrap_or_else(|| panic!("free"));
    assert!(deck.set_pad_loop(3, Some(free)));
    assert_eq!(deck.loop_clash(crossing, None), Some(outer));
}

#[test]
fn deleting_a_pad_takes_its_loop_with_it() {
    // A pad that holds a loop is deleted the same way a pad that holds a point is: everything on it goes,
    // including the loop and the mark on the strip.
    let mut deck = deck_with_ramp(44_100);
    let region = FrameRange::new(1_000, 2_000).unwrap_or_else(|| panic!("region"));
    assert!(deck.set_pad_loop(3, Some(region)));
    assert!(deck.set_cue_meta(3, Some("Drop".to_string()), Some("#ff4d4d".to_string())));
    deck.set_loop_mode(true);
    assert_eq!(deck.active_loops(), &[region]);
    assert_eq!(deck.pad_loops()[3], Some(region));

    assert!(deck.delete_hotcue(3), "the pad was not empty");
    assert!(deck.pad_loops()[3].is_none(), "the loop stayed on the pad");
    assert!(deck.cue_labels()[3].is_none(), "the name stayed on the pad");
    assert!(
        deck.cue_colors()[3].is_none(),
        "the colour stayed on the pad"
    );
    assert!(
        deck.active_loops().is_empty(),
        "a deleted loop must not stay armed"
    );
    assert!(
        !deck.delete_hotcue(3),
        "deleting an empty pad changes nothing"
    );
}

#[test]
fn a_loop_is_armed_once_however_it_was_made() {
    // One loop is one region on the strip. The button's loop, once saved on a pad, belongs to the pad;
    // pressing that pad arms the mode without adding a second copy of the same region.
    let mut deck = deck_with_ramp(44_100);
    let region = FrameRange::new(1_000, 2_000).unwrap_or_else(|| panic!("region"));

    deck.set_loop(Some(region));
    assert_eq!(deck.active_loops().len(), 1, "the button's loop is armed");

    assert!(deck.set_pad_loop(2, Some(region)));
    assert!(
        deck.loop_region().is_none(),
        "saving the button's loop moves it to the pad"
    );
    assert_eq!(
        deck.active_loops(),
        &[region],
        "one loop is armed once, not twice"
    );

    deck.set_loop_mode(false);
    assert!(deck.active_loops().is_empty());
    assert_eq!(
        deck.jump_pad(2),
        Some(1_000),
        "the pad arms it and takes the playhead"
    );
    assert!(
        deck.loop_region().is_none(),
        "the pad did not copy itself into the button's slot"
    );
    assert_eq!(deck.active_loops(), &[region]);
}

#[test]
fn loop_mode_repeats_every_saved_loop() {
    // A track can hold many loops. Loop mode is one switch for all of them: a playhead that comes to any
    // saved loop repeats that loop, wherever it was dropped.
    let mut deck = deck_with_ramp(44_100);
    // Frames are the transport's contract here; key lock is a separate stage with its own delay,
    // so this test asks the vinyl path.
    deck.set_keylock(false);
    let first = FrameRange::new(1_000, 2_000).unwrap_or_else(|| panic!("first"));
    let second = FrameRange::new(5_000, 6_000).unwrap_or_else(|| panic!("second"));
    assert!(deck.set_pad_loop(1, Some(first)));
    assert!(deck.set_pad_loop(2, Some(second)));
    deck.set_loop_mode(true);
    assert_eq!(deck.active_loops().len(), 2, "both saved loops are armed");

    // Dropped inside the second loop: it wraps at its own end into its own start.
    deck.seek(5_500);
    deck.play();
    let mut block = vec![0.0f32; 1_000 * 2];
    deck.render(&mut block);
    let rise = deck.loop_config().seam_frames(44_100);
    assert!((block[rise * 2] - ramp(5_500 + rise as u64)).abs() < 1e-6);
    // The pad's loop wraps through the decoder (only the button's own loop is held in memory), and the
    // decoder seam is faded over two milliseconds: the material is there just after the fade.
    assert!(
        block[500 * 2] < ramp(5_000) * 0.2,
        "the second loop must repeat, fading in: expected a small head, got {}",
        block[500 * 2]
    );
    assert!(
        (block[(500 + 96) * 2] - ramp(5_096)).abs() < 1e-6,
        "two milliseconds after the wrap the loop's material is there"
    );
    assert!((block[999 * 2] - ramp(5_499)).abs() < 1e-6);

    // Dropped before the first loop: it plays up to that loop and repeats it, not the whole file.
    deck.seek(500);
    let mut early = vec![0.0f32; 2_000 * 2];
    deck.render(&mut early);
    // The last two milliseconds of the loop fade out, and the two after the wrap fade in: what must be
    // exact is the material just outside that window.
    assert!((early[(1_500 - 97) * 2] - ramp(1_903)).abs() < 1e-6);
    assert!(
        early[1_500 * 2] < ramp(1_000) * 0.2,
        "the first loop must repeat, fading in: expected a small head, got {}",
        early[1_500 * 2]
    );
    assert!(
        (early[(1_500 + 96) * 2] - ramp(1_096)).abs() < 1e-6,
        "two milliseconds after the wrap the loop's material is there"
    );

    // Switched off, nothing repeats.
    deck.set_loop_mode(false);
    assert!(deck.active_loops().is_empty());
    deck.seek(500);
    let mut free = vec![0.0f32; 2_000 * 2];
    deck.render(&mut free);
    assert!(
        (free[1_999 * 2] - ramp(2_499)).abs() < 1e-6,
        "with loop mode off the track runs on"
    );
}

#[test]
fn a_loop_armed_while_playing_inside_it_has_no_silent_head() {
    // Arming a loop from inside it starts the capture mid-region. If that partial capture counts as
    // progress, the wrap plays the uncaptured head of the buffer — silence — until it reaches the point
    // where the loop was switched on.
    let mut deck = deck_with_ramp(44_100);
    // Frames are the transport's contract here; key lock is a separate stage with its own delay,
    // so this test asks the vinyl path.
    deck.set_keylock(false);
    let region = FrameRange::new(1_000, 3_000).unwrap_or_else(|| panic!("region"));
    deck.seek(2_000);
    deck.set_loop(Some(region));
    deck.play();

    let mut block = vec![0.0f32; 1_500 * 2];
    deck.render(&mut block);

    // The first two milliseconds are the rise that keeps a jump from standing against silence; the
    // playhead is the same one, so the material past the rise is the material of frame 2 088.
    let rise = deck.loop_config().seam_frames(44_100);
    assert!(
        (block[rise * 2] - ramp(2_000 + rise as u64)).abs() < 1e-6,
        "the armed loop must not move the playhead"
    );
    // The head is faded in over two milliseconds, so the first frame after the wrap is near zero by
    // design; what must not happen is a *silent* stretch of the buffer. Two milliseconds later the
    // material is there at full level, and the fade itself carries real energy.
    assert!(
        (block[(1_000 + 96) * 2] - ramp(1_096)).abs() < 1e-6,
        "two milliseconds after the wrap the loop start must be there at full level, got {}",
        block[(1_000 + 96) * 2]
    );
    let mut energy = 0.0f64;
    for frame in 1_000..1_096 {
        energy += f64::from(block[frame * 2]) * f64::from(block[frame * 2]);
    }
    let fade_rms = (energy / 96.0).sqrt();
    assert!(
        fade_rms > 0.3 * f64::from(ramp(1_048)),
        "the fade carries {fade_rms:.4}, which is silence, not a fade"
    );
    assert!((block[1_499 * 2] - ramp(1_499)).abs() < 1e-6);
}

#[test]
fn clearing_a_loop_is_inaudible() {
    // Switching a loop off inside it must be inaudible: no repeat of the tail already heard, no jump,
    // no click. The playhead finishes the copy it is inside and the decoder takes over where it stood.
    let mut deck = deck_with_ramp(44_100);
    // Frames are the transport's contract here; key lock is a separate stage with its own delay,
    // so this test asks the vinyl path.
    deck.set_keylock(false);
    let region = FrameRange::new(1_000, 3_000).unwrap_or_else(|| panic!("region"));
    deck.seek(1_000);
    deck.set_loop(Some(region));
    deck.play();

    // One pass through the loop fills the capture, so the next wrap is served from memory.
    let mut pass = vec![0.0f32; 2_000 * 2];
    deck.render(&mut pass);
    assert!(deck.loop_is_buffered(), "one pass must fill the capture");
    assert_eq!(deck.position(), 1_000, "the wrap returns to the loop start");

    let mut inside = vec![0.0f32; 500 * 2];
    deck.render(&mut inside);
    assert_eq!(deck.position(), 1_500);

    let seeks_before = deck.stats().seeks;
    deck.set_loop(None);
    assert_eq!(
        deck.position(),
        1_500,
        "clearing a loop must not move the playhead"
    );
    assert_eq!(
        deck.stats().seeks,
        seeks_before,
        "clearing a loop must not ask the decoder to seek: that request is the click the DJ hears"
    );

    // The next block crosses the loop's end: the copy runs out at 3_000 and the source carries on.
    let mut next = vec![0.0f32; 2_000 * 2];
    deck.render(&mut next);
    for frame in 0..2_000usize {
        let expected = ramp(1_500 + frame as u64);
        assert!(
            (next[frame * 2] - expected).abs() < 1e-6,
            "frame {frame}: the sound must run on unbroken, expected {expected}, got {}",
            next[frame * 2]
        );
    }
}

#[test]
fn a_loop_pad_takes_the_sound_with_it() {
    // The marker and the sound have to move together: a pad that arms a loop must move the decoder too,
    // otherwise the strip shows the loop start while the ears still hear the old position.
    let mut deck = deck_with_ramp(44_100);
    // Frames are the transport's contract here; key lock is a separate stage with its own delay,
    // so this test asks the vinyl path.
    deck.set_keylock(false);
    let region = FrameRange::new(10_000, 14_000).unwrap_or_else(|| panic!("region"));
    assert!(deck.set_pad_loop(4, Some(region)));
    deck.seek(30_000);

    let landed = deck
        .jump_pad(4)
        .unwrap_or_else(|| panic!("pad 4 holds a loop"));
    assert_eq!(
        landed, 10_000,
        "the pad took the playhead to the loop start"
    );
    assert_eq!(deck.position(), 10_000);

    deck.play();
    let mut block = vec![0.0f32; 256 * 2];
    deck.render(&mut block);
    // Past the two-millisecond rise the material is exactly the loop start's, frame for frame.
    let rise = deck.loop_config().seam_frames(44_100);
    assert!(
        (block[rise * 2] - ramp(10_000 + rise as u64)).abs() < 1e-6,
        "the first sample after the rise must come from the loop start, got {}",
        block[rise * 2]
    );
    assert!((block[200 * 2] - ramp(10_200)).abs() < 1e-6);
}

#[test]
fn a_hotcue_jumps_to_the_stored_frame() {
    let mut deck = deck_with_ramp(44_100);
    // Frames are the transport's contract here; key lock is a separate stage with its own delay,
    // so this test asks the vinyl path.
    deck.set_keylock(false);
    deck.seek(5_000);
    assert!(deck.store_hotcue(3));
    deck.seek(0);
    let landed = deck
        .jump_hotcue(3)
        .unwrap_or_else(|| panic!("hotcue stored"));
    assert_eq!(landed, 5_000);
    assert_eq!(deck.position(), 5_000);

    deck.play();
    let mut block = vec![0.0f32; 256 * 2];
    deck.render(&mut block);
    let rise = deck.loop_config().seam_frames(44_100);
    assert!((block[rise * 2] - ramp(5_000 + rise as u64)).abs() < 1e-6);
    assert_eq!(deck.stats().hotcue_jumps, 1);
}

#[test]
fn eight_hotcues_are_independent() {
    let mut deck = deck_with_ramp(44_100);
    let targets: [u64; 8] = [100, 1_000, 2_000, 3_000, 10_000, 20_000, 30_000, 40_000];
    for (slot, frame) in targets.iter().enumerate() {
        deck.seek(*frame);
        assert!(deck.store_hotcue(slot));
    }
    for (slot, frame) in targets.iter().enumerate() {
        assert_eq!(deck.jump_hotcue(slot), Some(*frame));
    }
    assert_eq!(deck.hotcues().len(), 8);
    assert!(!deck.store_hotcue(8), "a ninth slot does not exist");
    assert!(deck.delete_hotcue(0));
    assert_eq!(deck.jump_hotcue(0), None);
}

#[test]
fn gain_scales_the_block() {
    let mut deck = deck_with_ramp(44_100);
    deck.play();
    deck.set_gain(0.5);
    let mut block = vec![0.0f32; 256 * 2];
    deck.render(&mut block);
    let expected = ramp(0) * 0.5;
    assert!((block[0] - expected).abs() < 1e-6);
    assert!(deck.snapshot().levels.peak_left <= 0.5 + 1e-6);
}

#[test]
fn a_snapshot_carries_everything_a_display_needs() {
    let mut deck = Deck::new(DeckId::B);
    let track = TrackMeta {
        title: "Test Track".to_string(),
        artist: "Nobody".to_string(),
        duration_frames: Some(44_100),
        bpm: Some(128.0),
        first_beat_frame: None,
        grid: None,
        key: Some("8A".to_string()),

        analysis_lock: false,
    };
    deck.attach(Box::new(RampSource::new(44_100)), Some(track));
    deck.seek(10_000);

    let snapshot = deck.snapshot();
    assert_eq!(snapshot.deck, DeckId::B);
    assert_eq!(snapshot.load, DeckLoad::Ready);
    assert_eq!(snapshot.source_frame, SourceFrame(10_000));
    assert_eq!(snapshot.remaining_frames, Some(34_100));
    let ratio = snapshot.position_ratio.unwrap_or_default();
    assert!((ratio - 10_000.0 / 44_100.0).abs() < 1e-6);
    assert_eq!(snapshot.hotcues.len(), 8);
    assert_eq!(
        snapshot.track.map(|track| track.title),
        Some("Test Track".to_string())
    );
    assert_eq!(snapshot.loop_region, None);
}

#[test]
fn a_failed_load_is_a_state_not_a_silent_empty_deck() {
    let mut deck = Deck::new(DeckId::C);
    deck.set_progress(0.4);
    assert!(matches!(deck.load_state(), DeckLoad::Loading { .. }));
    deck.fail("cannot decode: unsupported codec");
    match deck.load_state() {
        DeckLoad::Failed { message } => assert!(message.contains("unsupported codec")),
        other => panic!("expected a failure, got {other:?}"),
    }
    assert!(!deck.play(), "a failed deck cannot be played");
}

#[test]
fn an_empty_read_is_an_underrun_not_a_panic() {
    struct Dry;
    impl DeckSource for Dry {
        fn sample_rate(&self) -> u32 {
            44_100
        }
        fn frames(&self) -> Option<u64> {
            Some(1_000)
        }
        fn read(&mut self, _out: &mut [f32]) -> Result<usize, String> {
            Ok(0)
        }
        fn seek(&mut self, frame: u64) -> Result<u64, String> {
            Ok(frame)
        }
    }

    let mut deck = Deck::new(DeckId::A);
    deck.attach(Box::new(Dry), None);
    deck.play();
    let mut block = vec![0.7f32; 256 * 2];
    deck.render(&mut block);
    assert!(block.iter().all(|sample| *sample == 0.0));
    assert_eq!(deck.stats().underruns, 1);
}

#[test]
fn a_real_file_plays_through_the_prefetcher() {
    // The wiring test: dj-codec produces the frames, the deck decides which ones.
    let path = std::env::temp_dir().join(format!("dj-deck-{}.wav", std::process::id()));
    write_tone(&path);
    let prefetcher = Prefetcher::open(
        &path,
        PrefetchConfig {
            window_frames: 16_384,
            block_frames: 1_024,
        },
    )
    .unwrap_or_else(|err| panic!("open: {err}"));

    let mut deck = Deck::new(DeckId::A);
    deck.attach(Box::new(prefetcher), None);
    assert!(deck.play());
    std::thread::sleep(std::time::Duration::from_millis(20));

    let mut block = vec![0.0f32; 4_096 * 2];
    deck.render(&mut block);
    let peak = block
        .iter()
        .fold(0.0f32, |acc, sample| acc.max(sample.abs()));
    assert!(peak > 0.1, "expected audio, peak was {peak}");
    assert!(deck.snapshot().remaining_frames.unwrap_or(0) > 0);
}

fn write_tone(path: &std::path::Path) {
    let rate = 44_100u32;
    let frames = rate;
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
        let value = 0.6 * (std::f32::consts::TAU * 440.0 * frame as f32 / rate as f32).sin();
        let sample = (value * 32_000.0) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap_or_else(|err| panic!("write: {err}"));
}

/// A source whose blocking seek is slow and whose deferred seek is instant.
///
/// It stands in for the prefetcher: outside the window a real seek spins until the decoder
/// answers, which is fine offline and unacceptable in the audio callback.
struct SlowSeekSource {
    position: u64,
    frames: u64,
    blocking_seeks: std::sync::Arc<std::sync::atomic::AtomicU64>,
    deferred_seeks: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl DeckSource for SlowSeekSource {
    fn sample_rate(&self) -> u32 {
        48_000
    }

    fn frames(&self) -> Option<u64> {
        Some(self.frames)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let want = out.len() / 2;
        let available = usize::try_from(self.frames.saturating_sub(self.position)).unwrap_or(0);
        let take = want.min(available);
        for sample in &mut out[..take * 2] {
            *sample = 0.25;
        }
        self.position += take as u64;
        Ok(take)
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.blocking_seeks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(1_500));
        self.position = frame;
        Ok(frame)
    }

    fn request_seek(&mut self, frame: u64) -> Result<u64, String> {
        self.deferred_seeks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.position = frame;
        Ok(frame)
    }
}

#[test]
fn a_live_deck_never_waits_for_a_seek() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    let blocking = Arc::new(AtomicU64::new(0));
    let deferred = Arc::new(AtomicU64::new(0));
    let source = SlowSeekSource {
        position: 0,
        frames: 480_000,
        blocking_seeks: Arc::clone(&blocking),
        deferred_seeks: Arc::clone(&deferred),
    };

    let mut deck = Deck::new(DeckId::A);
    deck.attach(Box::new(source), None);
    // A short loop forces a wrap inside one block, which is where the deck seeks.
    let Some(region) = dj_domain::FrameRange::new(0, 200) else {
        panic!("a 0..200 loop region must be valid");
    };
    // Buffering off: this test is about the fallback that long loops take, where wrapping really
    // does ask the source for a seek.
    deck.set_loop_config(dj_deck::LoopConfig {
        buffer_max_seconds: 0.0,
        ..dj_deck::LoopConfig::default()
    });
    deck.set_loop(Some(region));
    // The live path: the deck must ask, not wait.
    deck.set_blocking(false);
    assert!(deck.play());

    // The stage fills its delay before anything is heard, so the playhead reaches the loop's end a few
    // thousand frames in; the point of the test is that reaching it never waits.
    let started = std::time::Instant::now();
    let mut out = vec![0.0f32; 512];
    for _ in 0..40 {
        deck.render(&mut out);
        if deferred.load(Ordering::Relaxed) >= 1 {
            break;
        }
    }
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "a live render must not wait for a seek, took {elapsed:?}"
    );
    assert!(
        deferred.load(Ordering::Relaxed) >= 1,
        "the wrap must ask for a seek"
    );
    assert_eq!(
        blocking.load(Ordering::Relaxed),
        0,
        "the callback must never take the blocking path"
    );
}

#[test]
fn an_offline_deck_still_waits_for_a_seek() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    let blocking = Arc::new(AtomicU64::new(0));
    let deferred = Arc::new(AtomicU64::new(0));
    let source = SlowSeekSource {
        position: 0,
        frames: 480_000,
        blocking_seeks: Arc::clone(&blocking),
        deferred_seeks: Arc::clone(&deferred),
    };

    let mut deck = Deck::new(DeckId::A);
    // Frames and crossings are the transport's contract here; key lock is a separate stage with its
    // own delay, so these measurements ask the vinyl path.
    deck.set_keylock(false);
    deck.attach(Box::new(source), None);
    assert!(deck.play());
    deck.seek(1_000);

    // Offline rendering trades time for determinism, and that is deliberate.
    assert_eq!(blocking.load(Ordering::Relaxed), 1);
    assert_eq!(deferred.load(Ordering::Relaxed), 0);
    assert_eq!(deck.position(), 1_000);
}

/// A deck at 48 kHz with a grid of 120 BPM: a beat is 24 000 frames, so every number below is exact.
fn deck_on_a_grid(frames: u64) -> Deck {
    let mut deck = tone_deck(frames);
    deck.set_beat_grid(dj_domain::BeatGrid::single(120.0, 1_000).expect("a grid"));
    deck
}

#[test]
fn a_loop_of_four_beats_lands_on_the_grid_and_ends_four_beats_later() {
    // DJP-DECK-005: the two-press loop is whatever the hand hit. A loop the DJ asks for by length is the
    // music's: its start snaps to the nearest beat and its end is that many beats away, so the seam is
    // where the bar is. 120 BPM at 48 kHz is 24 000 frames a beat, the first beat on frame 1 000.
    let mut deck = deck_on_a_grid(4_000_000);
    // 30 000 is 5 000 frames (a fifth of a beat) past the second beat: the loop must start on the beat,
    // not where the play head stood.
    let region = deck.loop_by_beats(30_000, 4).expect("a loop of four beats");
    assert_eq!(region.start.0, 25_000, "the loop must start on a beat");
    assert_eq!(
        region.end.0, 121_000,
        "four beats later, on the beat as well"
    );
    assert_eq!(region.len(), 4 * 24_000);
    assert_eq!(deck.loop_region(), Some(region), "the loop is in force");
    // The capture fills as the loop plays: a region that has never been heard has nothing to hold yet,
    // and the play head has to be in it — that is why the engine seeks into a loop it has just made.
    deck.seek(region.start.0);
    deck.play();
    let mut out = vec![0.0f32; 512];
    for _ in 0..400 {
        deck.render(&mut out);
    }
    assert!(deck.loop_is_buffered(), "a loop this short is captured");
}

#[test]
fn a_loop_asked_for_by_length_is_refused_without_a_grid() {
    // A grid is the only thing that knows where the beats are: without one a "four beat loop" has no
    // meaning, and inventing frames for it would be a loop of nothing in particular.
    let mut deck = tone_deck(4_000_000);
    assert!(deck.beat_grid().is_none());
    assert_eq!(deck.loop_by_beats(30_000, 4), None);
    assert_eq!(deck.loop_region(), None);
    assert_eq!(deck.loop_by_beats(30_000, 0), None);
}

#[test]
fn moving_a_loop_keeps_its_length_and_stays_on_the_beats() {
    // Ten moves of a loop must be the same loop, not one frame shorter each time: the length is kept in
    // frames while the start travels by whole beats.
    let mut deck = deck_on_a_grid(4_000_000);
    let first = deck.loop_by_beats(30_000, 2).expect("a loop of two beats");
    let length = first.len();
    let mut start = first.start.0;
    for _ in 0..10 {
        let moved = deck.move_loop(2).expect("the loop moves");
        assert_eq!(moved.len(), length, "ten moves must not shorten the loop");
        assert_eq!(moved.start.0, start + 2 * 24_000, "two beats at a time");
        assert_eq!(moved.end.0, moved.start.0 + length);
        start = moved.start.0;
    }
    // Backwards as well, and the length survives that too. Ten moves back land on the first loop: the
    // eleventh would start before the file, and that is a refusal, not a loop.
    for _ in 0..10 {
        let moved = deck.move_loop(-2).expect("the loop moves back");
        assert_eq!(moved.len(), length);
    }
    assert_eq!(
        deck.loop_region().map(|region| region.start.0),
        Some(first.start.0)
    );
}

#[test]
fn a_length_loop_replaces_the_unsaved_one_but_not_a_saved_one() {
    // A length loop is a new loop like any other: it replaces the one in force, and is refused only by a
    // loop that was saved.
    let mut deck = deck_on_a_grid(4_000_000);
    deck.set_loop(Some(
        dj_domain::FrameRange::new(200_000, 250_000).expect("a region"),
    ));
    let replaced = deck.loop_by_beats(180_000, 4);
    assert!(
        replaced.is_some(),
        "a length loop replaces the loop that is not saved"
    );
    let in_force = deck.loop_region().map(|region| region.start.0);
    assert_ne!(in_force, Some(200_000), "the old loop is gone");

    // Saved, that loop stands in the way again.
    let saved = dj_domain::FrameRange::new(200_000, 250_000).expect("a region");
    assert!(deck.set_pad_loop(1, Some(saved)));
    let refused = deck.loop_by_beats(180_000, 4);
    assert_eq!(refused, None, "a loop over a saved one must be refused");
    assert_eq!(
        deck.loop_region().map(|region| region.start.0),
        in_force,
        "a refusal changes nothing"
    );
}

#[test]
fn the_seam_of_a_loop_does_not_click() {
    // A loop that does not meet itself: one and a half periods of a 440 Hz tone, so the copy jumps from
    // the crest to the trough — half a volt in one frame, which the ear hears as a click on the seam.
    // The deck evens the step out over two milliseconds, so the worst jump it puts out is a normal one.
    let mut deck = tone_deck(4_000_000);
    // The seam is the copy's own business: with the key locked, the stretcher's latency would put three
    // thousand frames of silence in front of the material and its windows would smear the step anyway.
    deck.set_keylock(false);
    let period = 48_000.0 / 440.0;
    let start = 1_000u64;
    let end = start + (period * 1.5) as u64;
    let Some(region) = FrameRange::new(start, end) else {
        panic!("a loop of one and a half periods must be a valid region");
    };
    deck.set_loop(Some(region));
    deck.seek(start);
    deck.play();
    let mut block = vec![0.0f32; 256 * 2];
    let mut all = Vec::new();
    // Three passes of the loop: the seam twice, and the copy in force by then. The first two blocks are
    // the start of playback — a step from silence that is not the seam — so they are not measured.
    for index in 0..64 {
        deck.render(&mut block);
        // The first blocks are the start of playback: a step from silence, not the seam.
        if index >= 2 {
            all.extend_from_slice(&block);
        }
    }
    assert!(
        deck.loop_is_buffered(),
        "the loop must be playing from its copy"
    );
    assert!(
        deck.stats().loop_wraps >= 2,
        "the check needs a seam to look at"
    );
    let worst = biggest_step(&all);
    assert!(
        worst < 0.2,
        "the seam steps by {worst:.3} — that is the click"
    );
}

#[test]
fn a_loop_that_wraps_through_the_decoder_does_not_click() {
    // This is the seam the owner heard "depending on the place": a loop longer than the capture budget
    // wraps by asking the decoder for its start, and there is no copy of the far side to crossfade with.
    // The seam there is faded out and in over two milliseconds instead — a dip, not a step.
    let mut deck = tone_deck(4_000_000);
    deck.set_keylock(false);
    // One second of a 440 Hz tone is a whole number of periods; the loop below is deliberately not.
    deck.set_loop_config(dj_deck::LoopConfig {
        buffer_max_seconds: 0.1,
        ..dj_deck::LoopConfig::default()
    });
    let Some(region) = FrameRange::new(1_000, 48_000) else {
        panic!("a loop of one second must be a valid region");
    };
    deck.set_loop(Some(region));
    deck.seek(1_000);
    deck.play();
    let mut block = vec![0.0f32; 256 * 2];
    let mut all = Vec::new();
    for index in 0..400 {
        deck.render(&mut block);
        if index >= 2 {
            all.extend_from_slice(&block);
        }
    }
    assert!(
        !deck.loop_is_buffered(),
        "the check needs the decoder path, not the copy"
    );
    assert!(deck.stats().loop_wraps >= 1, "the loop must have wrapped");
    let worst = biggest_step(&all);
    assert!(
        worst < 0.2,
        "the decoder seam steps by {worst:.3} — that is the click"
    );
}

#[test]
fn the_seam_under_the_key_lock_does_not_click() {
    // The owner listens with the key locked, so this is the path his ears take: the material goes through
    // the stretcher, and a jump of the read position lands inside its sixty-millisecond windows. Without
    // the pre-roll the phase vocoder continues the old phases and the seam ticks (measured 0.214); with
    // it the seam is as smooth as the copy's own (0.006).
    let mut deck = tone_deck(4_000_000);
    let period = 48_000.0 / 440.0;
    let start = 1_000u64;
    let end = start + (period * 1.5) as u64;
    let Some(region) = FrameRange::new(start, end) else {
        panic!("a loop of one and a half periods must be a valid region");
    };
    deck.set_loop(Some(region));
    deck.seek(start);
    deck.play();
    let mut block = vec![0.0f32; 256 * 2];
    let mut all = Vec::new();
    for index in 0..400 {
        deck.render(&mut block);
        if index >= 40 {
            all.extend_from_slice(&block);
        }
    }
    let worst = biggest_step(&all);
    assert!(
        worst < 0.05,
        "the keylocked seam steps by {worst:.3} — the stretcher was not told about the jump"
    );
}

#[test]
fn jumping_under_the_key_lock_does_not_click() {
    // The owner hears the loop click "at its beginning, depending on the place". Pressing Loop asks for a
    // loop on the grid, and when that region starts before the play head the deck jumps to its start — a
    // jump of the read position under the stretcher's sixty-millisecond windows, which is the same thing
    // that made the seam tick. This measures the jump, not the seam: seam length has nothing to do with it.
    let mut deck = tone_deck(4_000_000);
    deck.seek(200_000);
    deck.play();
    let mut block = vec![0.0f32; 256 * 2];
    let mut before = Vec::new();
    for _ in 0..40 {
        deck.render(&mut block);
        before.extend_from_slice(&block);
    }
    // The first frames of playback ramp up from silence, so the material's own step is taken past them.
    let material = biggest_step(&before[4_000..]);
    deck.seek(20_000);
    let mut after = Vec::new();
    for _ in 0..200 {
        deck.render(&mut block);
        after.extend_from_slice(&block);
    }
    // The stretcher refills after a jump, so the sound returns some frames in; the tick, if any, is where
    // it returns. That is the frame to measure.
    let mono: Vec<f32> = after.chunks_exact(2).map(|frame| frame[0]).collect();
    let resume = mono
        .iter()
        .position(|value| value.abs() > 1e-4)
        .unwrap_or(0);
    let window = &mono[resume.saturating_sub(2)..(resume + 2_000).min(mono.len())];
    let mut jump = 0.0f32;
    let mut at = 0usize;
    for index in 1..window.len() {
        let step = (window[index] - window[index - 1]).abs();
        if step > jump {
            jump = step;
            at = index;
        }
    }
    println!(
        "MEASURE jump: material {material:.4}, at the jump {jump:.4} (resume {resume}, at {at})"
    );
    assert!(
        jump < 0.05,
        "the jump steps by {jump:.3} against the material's {material:.3} — that is the click"
    );
}

#[test]
fn a_loop_plays_only_its_own_material() {
    // The owner hears "other parts of the composition" inside a loop. The ramp says which frame each
    // sample came from — value = frame / 20 000 * 0,5 — so the whole output of a loop can be checked
    // against the loop's own frame range instead of being listened to.
    let mut deck = deck_with_ramp(4_000_000);
    deck.set_keylock(false);
    let Some(region) = FrameRange::new(1_000, 3_000) else {
        panic!("a 1 000..3 000 loop must be valid");
    };
    deck.set_loop(Some(region));
    deck.seek(1_000);
    deck.play();
    let mut block = vec![0.0f32; 256 * 2];
    let mut low = f32::MAX;
    let mut high = f32::MIN;
    let mut frame_index = 0usize;
    let mut outside: Vec<(usize, f32)> = Vec::new();
    for _ in 0..400 {
        deck.render(&mut block);
        for frame in block.chunks_exact(2) {
            let value = frame[0];
            low = low.min(value);
            high = high.max(value);
            // The first two milliseconds rise from silence, so the very start is not the material.
            if frame_index > 200 && !(0.0249..=0.0751).contains(&value) {
                outside.push((frame_index, value));
            }
            frame_index += 1;
        }
    }
    let sample: Vec<String> = outside
        .iter()
        .take(8)
        .map(|(at, value)| format!("{at}:{value:.5}"))
        .collect();
    println!(
        "MEASURE outside: {} of {frame_index} frames, first {sample:?}",
        outside.len()
    );
    // The loop's own material spans these two values (with a little room for the seam's crossfade).
    println!(
        "MEASURE loop range: {low:.5}..{high:.5} as frames {:.0}..{:.0}",
        low / 0.5 * 20_000.0,
        high / 0.5 * 20_000.0
    );
    // The first two milliseconds are the rise after the jump, not the material.
    assert!(
        outside.is_empty(),
        "the loop played {} frames from outside 1 000..3 000: {sample:?}",
        outside.len()
    );
}

#[test]
fn a_loop_keeps_the_playhead_inside_itself() {
    // "Other parts of the composition play inside the loop" is a claim about the transport, so this
    // traces where the play head actually is on every block, with the key lock both off and on. A loop
    // whose head leaves 1 000..3 000 is playing something that is not the loop.
    for keylock in [false, true] {
        let mut deck = deck_with_ramp(4_000_000);
        if keylock {
            deck.set_tempo(1.05);
        } else {
            deck.set_keylock(false);
        }
        let Some(region) = FrameRange::new(1_000, 3_000) else {
            panic!("a 1 000..3 000 loop must be valid");
        };
        deck.set_loop(Some(region));
        deck.seek(1_000);
        deck.play();
        let mut block = vec![0.0f32; 256 * 2];
        let mut low = u64::MAX;
        let mut high = 0u64;
        let mut wraps = 0usize;
        let mut previous = deck.position();
        let mut first_after_wrap: Vec<u64> = Vec::new();
        for _ in 0..400 {
            deck.render(&mut block);
            let position = deck.position();
            if position < previous {
                wraps += 1;
                if first_after_wrap.len() < 4 {
                    first_after_wrap.push(position);
                }
            }
            low = low.min(position);
            high = high.max(position);
            previous = position;
        }
        println!(
            "MEASURE loop transport (keylock {keylock}): head {low}..{high}, {wraps} wraps, back to {first_after_wrap:?}"
        );
        assert!(
            low >= 1_000 && high <= 3_000,
            "the head left the loop: {low}..{high} instead of 1 000..3 000"
        );
        assert!(wraps >= 1, "the loop must wrap");
    }
}

#[test]
fn a_moved_loop_plays_only_the_material_it_says() {
    // The owner hears "the beats just outside the loop" after moving a loop with the arrows several
    // times. The region and the play head were already checked; this checks the audio: every output
    // frame must come from inside the region the deck reports.
    let mut deck = code_deck(4_000_000);
    deck.set_keylock(false);
    let grid =
        dj_domain::BeatGrid::single(120.0, 4_800).unwrap_or_else(|| panic!("a 120 BPM grid"));
    deck.set_beat_grid(grid);
    deck.seek(44_100);
    deck.play();
    let mut block = vec![0.0f32; 256 * 2];
    let Some(region) = deck.loop_by_beats(44_100, 2) else {
        panic!("a two-beat loop on the grid");
    };
    // A handful of frames at the wrap boundary are the resampler's own blend of the two sides; they are
    // not material from elsewhere. Everything else must be the loop's own.
    let check = |block: &[f32], region: FrameRange, when: &str| {
        let low = region.start.0 as f32 / 4_000_000.0;
        let high = region.end.0 as f32 / 4_000_000.0;
        let slack = 16.0 / 4_000_000.0;
        let outside = block
            .chunks_exact(2)
            .filter(|frame| {
                let value = frame[0];
                value != 0.0 && !((low - slack)..=(high + slack)).contains(&value)
            })
            .count();
        assert!(
            outside <= block.len() / 2 / 50,
            "{when}: {outside} frames of {} came out of a loop of {:.0}..{:.0}",
            block.len() / 2,
            region.start.0,
            region.end.0
        );
    };
    // The first blocks after the seek still carry what the prefetcher had from before it — that is the
    // "no silent head" behaviour, and it is not the loop.
    for _ in 0..20 {
        deck.render(&mut block);
    }
    for _ in 0..40 {
        deck.render(&mut block);
        check(&block, region, "first pass");
    }
    for move_index in 0..4 {
        let Some(moved) = deck.move_loop(1) else {
            panic!("the loop moves a beat");
        };
        println!(
            "MEASURE moved loop {move_index}: {}..{}, head {}",
            moved.start.0,
            moved.end.0,
            deck.position()
        );
        // The stretch after the move may open with silence and then rises from it; the rise's frames are
        // the material scaled down, and a scaled frame decodes as a wrong one. The check starts past it.
        let mut all: Vec<f32> = Vec::new();
        for _ in 0..40 {
            deck.render(&mut block);
            all.extend_from_slice(&block);
        }
        let audible = all
            .chunks_exact(2)
            .position(|frame| frame[0].abs() > 1e-6)
            .unwrap_or(all.len() / 2);
        let from = ((audible + 200) * 2).min(all.len());
        check(&all[from..], moved, &format!("after move {move_index}"));
    }
}

#[test]
fn moving_a_loop_around_the_playhead_does_not_move_the_sound() {
    // The owner's rule: a loop nudged by a beat while the head is still inside it must not be heard as a
    // jump. The material says where the sound is, so consecutive output frames tell whether it moved.
    let mut deck = code_deck(4_000_000);
    deck.set_keylock(false);
    let grid =
        dj_domain::BeatGrid::single(120.0, 4_800).unwrap_or_else(|| panic!("a 120 BPM grid"));
    deck.set_beat_grid(grid);
    let Some(region) = FrameRange::new(48_900, 93_000) else {
        panic!("a loop");
    };
    deck.set_loop(Some(region));
    // Two beats in: far enough inside that moving the loop one beat forward still holds the head.
    deck.seek(80_000);
    deck.play();
    let mut block = vec![0.0f32; 256 * 2];
    let mut all: Vec<f32> = Vec::new();
    for _ in 0..20 {
        deck.render(&mut block);
        all.extend_from_slice(&block);
    }
    let before = deck.position();
    let Some(moved) = deck.move_loop(1) else {
        panic!("the loop moves a beat");
    };
    assert!(
        moved.contains(SourceFrame(before)),
        "the check needs the head to stay inside: head {before}, loop {}..{}",
        moved.start.0,
        moved.end.0
    );
    for _ in 0..20 {
        deck.render(&mut block);
        all.extend_from_slice(&block);
    }
    // Every output frame must be the next one after the previous: no jump in the middle of the stretch.
    let mono: Vec<f32> = all.chunks_exact(2).map(|frame| frame[0]).collect();
    // The first blocks carry what the prefetcher had before the seek and then the rise, so the trace is
    // read from where the material settles.
    let audible = mono
        .iter()
        .position(|value| value.abs() > 1e-6)
        .unwrap_or(0);
    let from = (audible + 300).min(mono.len().saturating_sub(1));
    let mut worst = 0.0f32;
    let mut at = 0usize;
    for index in (from + 1)..mono.len() {
        let step = (mono[index] - mono[index - 1]).abs() * 4_000_000.0;
        if step > worst {
            worst = step;
            at = index;
        }
    }
    println!(
        "MEASURE move without a jump: head {before} -> {}, worst step {worst:.1} frames at {at}",
        deck.position()
    );
    // One frame's worth of material is a step of one; anything much larger is the sound having moved.
    assert!(
        worst < 8.0,
        "the sound moved by {worst:.0} frames at {at}: the loop shift was heard as a jump"
    );
}

#[test]
fn a_loop_moved_while_its_copy_plays_does_not_jump() {
    // The owner hears a jump when he moves a loop that is already playing. The loop is read from its copy
    // by then, and the decoder behind it stands at the end of the first pass: taking the sound back to the
    // decoder without saying where the sound is replays from there — a whole loop away. This is the case
    // the earlier "no jump" test does not cover, because it never lets the loop wrap.
    let mut deck = code_deck(4_000_000);
    // The owner listens with the key locked, and the stretcher is a stage of its own across a change of
    // the loop: the same measurement is run on both paths.
    deck.set_tempo(1.05);
    let grid =
        dj_domain::BeatGrid::single(120.0, 4_800).unwrap_or_else(|| panic!("a 120 BPM grid"));
    deck.set_beat_grid(grid);
    let Some(region) = FrameRange::new(48_900, 93_000) else {
        panic!("a loop");
    };
    deck.set_loop(Some(region));
    deck.seek(48_900);
    deck.play();
    let mut block = vec![0.0f32; 256 * 2];
    for _ in 0..200 {
        deck.render(&mut block);
    }
    assert!(
        deck.stats().loop_wraps >= 1 && deck.loop_is_buffered(),
        "the check needs the copy to be playing: wraps {}, buffered {}",
        deck.stats().loop_wraps,
        deck.loop_is_buffered()
    );
    let before = deck.position();
    // Backwards: the head stays inside the moved loop, so nothing should move the sound.
    let Some(moved) = deck.move_loop(-1) else {
        panic!("the loop moves back a beat");
    };
    assert!(moved.contains(SourceFrame(before)), "the head stays inside");
    let mut all: Vec<f32> = Vec::new();
    for _ in 0..20 {
        deck.render(&mut block);
        all.extend_from_slice(&block);
    }
    let mono: Vec<f32> = all.chunks_exact(2).map(|frame| frame[0]).collect();
    let audible = mono
        .iter()
        .position(|value| value.abs() > 1e-6)
        .unwrap_or(0);
    let from = (audible + 300).min(mono.len().saturating_sub(1));
    let mut worst = 0.0f32;
    let mut at = 0usize;
    for index in (from + 1)..mono.len() {
        let step = (mono[index] - mono[index - 1]).abs() * 4_000_000.0;
        if step > worst {
            worst = step;
            at = index;
        }
    }
    println!(
        "MEASURE move from the copy: head {before} -> {}, worst step {worst:.1} frames at {at}",
        deck.position()
    );
    // The key-lock path smooths the change through the stretcher, so a few dozen frames' worth of the
    // ramp is expected; a jump of a loop would be tens of thousands. 200 frames is 0,00005 in amplitude —
    // some 86 dB below full scale, which is the stretcher's own smoothing, not a jump.
    assert!(
        worst < 200.0,
        "the sound moved by {worst:.0} frames at {at}: moving a playing loop was heard as a jump"
    );
}

#[test]
fn a_short_loop_plays_through_its_seam_without_a_gap() {
    // A loop that fits the capture budget must wrap by copying, not by asking the source for a
    // seek: that seek is where the audible block of silence came from (ADR-0009).
    let blocking = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let deferred = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let source = SlowSeekSource {
        position: 0,
        frames: 480_000,
        blocking_seeks: std::sync::Arc::clone(&blocking),
        deferred_seeks: std::sync::Arc::clone(&deferred),
    };

    let mut deck = Deck::new(DeckId::A);
    deck.attach(Box::new(source), None);
    // 9 600 frames is 200 ms at 48 kHz — a musical loop, well inside the default budget.
    let Some(region) = dj_domain::FrameRange::new(0, 9_600) else {
        panic!("a 0..9600 loop region must be valid");
    };
    deck.set_loop(Some(region));
    deck.set_blocking(false);
    assert!(deck.play());

    // 512 samples per render is 256 frames, so 60 renders cover the 9 600-frame loop with room
    // to wrap once.
    let mut out = vec![0.0f32; 512];
    for _ in 0..60 {
        deck.render(&mut out);
    }
    assert!(deck.loop_is_buffered(), "the loop must be captured");

    let wraps_before = deck.stats().loop_wraps;
    for _ in 0..200 {
        deck.render(&mut out);
    }

    assert_eq!(
        deck.stats().underruns,
        0,
        "a captured loop must not starve the deck at its seam"
    );
    assert!(
        deck.stats().loop_wraps > wraps_before,
        "the loop must actually have wrapped again"
    );
    assert_eq!(
        deferred.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "a captured loop must not ask the source for a seek"
    );
}

#[test]
fn a_loop_longer_than_the_budget_is_not_captured() {
    let blocking = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let deferred = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let source = SlowSeekSource {
        position: 0,
        frames: 480_000,
        blocking_seeks: std::sync::Arc::clone(&blocking),
        deferred_seeks: std::sync::Arc::clone(&deferred),
    };

    let mut deck = Deck::new(DeckId::A);
    deck.attach(Box::new(source), None);
    deck.set_loop_config(dj_deck::LoopConfig {
        buffer_max_seconds: 0.1,
        ..dj_deck::LoopConfig::default()
    });
    // 48 000 frames is one second, ten times the budget.
    let Some(region) = dj_domain::FrameRange::new(0, 48_000) else {
        panic!("a 0..48000 loop region must be valid");
    };
    deck.set_loop(Some(region));
    deck.set_blocking(false);
    assert!(deck.play());

    let mut out = vec![0.0f32; 512];
    for _ in 0..100 {
        deck.render(&mut out);
    }

    assert!(
        !deck.loop_is_buffered(),
        "a loop over the budget must stay unbuffered rather than silently allocate"
    );
}

#[test]
fn key_lock_keeps_the_pitch_and_runs_at_the_nominal_speed() {
    // Two readings of one promise: a key-locked deck at tempo r must consume r source frames per output
    // frame, and a tone must come out at the frequency the file put there. The deck once ran about
    // thirteen cents fast at 1.00 — rate alone would have shown it, pitch is what a listener hears.
    for tempo in [1.0, 0.96, 1.04] {
        let mut speed = tone_deck(48_000 * 60);
        speed.set_keylock(true);
        speed.set_tempo(tempo);
        assert!(speed.play(), "the deck plays");
        let mut block = vec![0.0f32; 512 * 2];
        for _ in 0..200 {
            speed.render(&mut block);
        }
        let start = speed.position();
        let blocks = 400.0;
        for _ in 0..400 {
            speed.render(&mut block);
        }
        let rate = (speed.position() - start) as f64 / (blocks * 512.0);
        assert!(
            (rate - tempo).abs() < 1e-4,
            "key lock at {tempo} consumed {rate} source frames per output frame"
        );

        let mut sound = tone_deck(48_000 * 60);
        sound.set_keylock(true);
        sound.set_tempo(tempo);
        assert!(sound.play(), "the deck plays");
        for _ in 0..200 {
            sound.render(&mut block);
        }
        let seconds = 5.0;
        let rendered = seconds * 48_000.0 / 512.0;
        let mut crossings = 0usize;
        let mut last = 0.0f32;
        for _ in 0..(rendered as usize) {
            sound.render(&mut block);
            for frame in 0..512 {
                let sample = block[frame * 2];
                if last < 0.0 && sample >= 0.0 {
                    crossings += 1;
                }
                last = sample;
            }
        }
        let pitch = crossings as f64 / seconds;
        let cents = 1200.0 * (pitch / 440.0).log2();
        assert!(
            cents.abs() < 5.0,
            "key lock at {tempo} put the tone {cents:.1} cents off 440 Hz"
        );
    }
}

/// A pure tone, so the tempo stage can be measured in pitch and not only in position.
struct ToneSource {
    frames: u64,
    position: u64,
}

impl DeckSource for ToneSource {
    fn sample_rate(&self) -> u32 {
        48_000
    }

    fn frames(&self) -> Option<u64> {
        Some(self.frames)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let want = out.len() / 2;
        let available = usize::try_from(self.frames.saturating_sub(self.position)).unwrap_or(0);
        let take = want.min(available);
        for index in 0..take {
            let frame = self.position + index as u64;
            let value =
                (std::f64::consts::TAU * 440.0 * frame as f64 / 48_000.0).sin() as f32 * 0.5;
            out[index * 2] = value;
            out[index * 2 + 1] = value;
        }
        self.position += take as u64;
        Ok(take)
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.position = frame.min(self.frames.saturating_sub(1));
        Ok(self.position)
    }
}

fn tone_deck(frames: u64) -> Deck {
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(48_000);
    deck.attach(
        Box::new(ToneSource {
            frames,
            position: 0,
        }),
        None,
    );
    deck
}

/// A tone at the rate the file was recorded at, so a test can ask what pitch comes out of the device.
struct ToneAtRate {
    frames: u64,
    position: u64,
    rate: u32,
    hertz: f64,
}

impl DeckSource for ToneAtRate {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn frames(&self) -> Option<u64> {
        Some(self.frames)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, String> {
        let want = out.len() / 2;
        let available = usize::try_from(self.frames.saturating_sub(self.position)).unwrap_or(0);
        let take = want.min(available);
        for index in 0..take {
            let frame = self.position + index as u64;
            let value = (std::f64::consts::TAU * self.hertz * frame as f64 / f64::from(self.rate))
                .sin() as f32
                * 0.5;
            out[index * 2] = value;
            out[index * 2 + 1] = value;
        }
        self.position += take as u64;
        Ok(take)
    }

    fn seek(&mut self, frame: u64) -> Result<u64, String> {
        self.position = frame.min(self.frames.saturating_sub(1));
        Ok(self.position)
    }
}

/// Zero crossings on the left channel: two per cycle, so the pitch is countable.
fn crossings(block: &[f32]) -> usize {
    let mut count = 0usize;
    let mut last = 0.0f32;
    for frame in block.chunks_exact(2) {
        let value = frame[0];
        if (last < 0.0 && value >= 0.0) || (last > 0.0 && value <= 0.0) {
            count += 1;
        }
        last = value;
    }
    count
}

/// A deck with a metronome-tight grid: 120 BPM at 44,1 kHz, first beat on frame 10 000.
///
/// At this rate one beat is 22 050 frames, so every expected position below is a whole number the eye
/// can check against the beat count.
fn grid_deck() -> Deck {
    let mut deck = deck_with_ramp(4_000_000);
    deck.set_beat_grid(BeatGrid::single(120.0, 10_000).expect("a 120 BPM grid"));
    deck
}

/// The material a `CodeSource` generates, decoded once into memory: `value = frame / 4 000 000`.
///
/// The track cache exists to stop decoding during playback, so the first thing it owes is that the
/// bytes a deck plays from it are the bytes it played from the stream — not "close", the same.
fn code_material(frames: u64) -> std::sync::Arc<[f32]> {
    let mut data = Vec::with_capacity(usize::try_from(frames).expect("a track that fits") * 2);
    for frame in 0..frames {
        #[allow(clippy::cast_precision_loss)]
        let value = (frame as f32) / 4_000_000.0;
        data.push(value);
        data.push(value);
    }
    data.into()
}

#[test]
fn a_deck_on_a_cache_entry_plays_the_same_frames_as_a_deck_on_the_stream() {
    // The cache stores the entry in its own sample format, so the adapter that turns bytes back into f32
    // is part of the promise: it must be the same material, not a close relative.
    let frames = 200_000;
    let mut bytes = Vec::with_capacity(usize::try_from(frames).expect("a track that fits") * 8);
    for frame in 0..frames {
        #[allow(clippy::cast_precision_loss)]
        let value = (frame as f32) / 4_000_000.0;
        bytes.extend_from_slice(&value.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let reader = dj_codec::CachedReader::open_memory(
        std::sync::Arc::new(bytes),
        44_100,
        2,
        dj_codec::PcmSampleFormat::F32,
    );

    let mut streamed = code_deck(frames);
    let mut cached = code_deck(frames);
    assert!(cached.swap_source(Box::new(CachedReaderSource::new(reader))));
    streamed.play();
    cached.play();
    let mut block = vec![0.0f32; 512];
    let mut reference = vec![0.0f32; 512];
    for _ in 0..100 {
        streamed.render(&mut block);
        cached.render(&mut reference);
        assert_eq!(
            block, reference,
            "the cache entry must be the same material, sample for sample"
        );
    }
}

/// Material whose sample value is its own frame number, so a block of it can be checked frame by frame.
fn ramp_material(frames: u64) -> std::sync::Arc<[f32]> {
    let mut data = Vec::with_capacity(usize::try_from(frames).expect("a track that fits") * 2);
    for frame in 0..frames {
        #[allow(clippy::cast_precision_loss)]
        let value = frame as f32;
        data.push(value);
        data.push(value);
    }
    data.into()
}

#[test]
fn reverse_walks_the_material_backwards() {
    // The witness is the material itself: a ramp played backwards comes out descending, one frame at a
    // time, and the playhead walks down with it.
    let frames = 200_000;
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(44_100);
    deck.set_keylock(false);
    deck.attach(
        Box::new(CachedSource::new(ramp_material(frames), 44_100)),
        None,
    );
    deck.seek(100_000);
    assert!(deck.play(), "the deck plays");
    let mut block = vec![0.0f32; 512 * 2];
    for _ in 0..40 {
        deck.render(&mut block);
    }
    let before = deck.position();
    assert!(
        deck.set_reverse(true),
        "a track held in the cache can be walked backwards"
    );
    deck.render(&mut block);
    let first = block[0];
    let step = block[0] - block[2];
    assert!(
        step > 0.0,
        "the material must descend, not ascend: first {first}, next {}",
        block[2]
    );
    for index in 0..512usize {
        #[allow(clippy::cast_precision_loss)]
        let expected = first - step * index as f32;
        let heard = block[index * 2];
        assert!(
            (heard - expected).abs() < 1e-2,
            "frame {index}: heard {heard}, expected {expected}"
        );
    }
    assert!(
        deck.position() < before,
        "the playhead walks down: {} was {}, now {}",
        before,
        before,
        deck.position()
    );
}

#[test]
fn reverse_ends_at_the_start_of_the_file() {
    let frames = 40_000;
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(44_100);
    deck.set_keylock(false);
    deck.attach(
        Box::new(CachedSource::new(ramp_material(frames), 44_100)),
        None,
    );
    deck.seek(4_000);
    assert!(deck.play(), "the deck plays");
    assert!(deck.set_reverse(true), "reverse starts");
    let mut block = vec![0.0f32; 512 * 2];
    let mut stopped_at = None;
    for _ in 0..60 {
        deck.render(&mut block);
        if !deck.reverse() {
            // Read here and not later: once reverse is off the deck plays forward again, which is
            // exactly what it should do, and the playhead moves on.
            stopped_at = Some(deck.position());
            break;
        }
    }
    assert_eq!(
        stopped_at,
        Some(0),
        "reverse ends at the first frame and leaves the playhead there"
    );
}

#[test]
fn a_streamed_track_waits_for_its_window_instead_of_stalling() {
    // The old contract was a refusal, because walking a decoder backwards would stall the callback.
    // The deck now accepts reverse and asks the engine for a window; the sound waits in silence while
    // the callback keeps its budget, which is the point.
    let mut deck = code_deck(50_000);
    deck.seek(20_000);
    assert!(deck.play(), "the deck plays");
    let mut block = vec![0.0f32; 512 * 2];
    for _ in 0..10 {
        deck.render(&mut block);
    }
    assert!(
        deck.set_reverse(true),
        "reverse is accepted, the material comes later"
    );
    let before = deck.position();
    deck.render(&mut block);
    assert!(deck.reverse(), "still waiting, so still reversing");
    assert_eq!(
        deck.position(),
        before,
        "nothing was read from the decoder: the callback did not wait on it"
    );
    assert!(
        deck.reverse_window_needed().is_some(),
        "the deck says what it needs"
    );
}

#[test]
fn reverse_with_key_lock_keeps_the_pitch() {
    let rate = 48_000u32;
    let frames = u64::from(rate) * 8;
    let mut data = Vec::with_capacity(usize::try_from(frames).expect("a short track") * 2);
    for frame in 0..frames {
        #[allow(clippy::cast_precision_loss)]
        let phase = std::f64::consts::TAU * 440.0 * frame as f64 / f64::from(rate);
        let value = (phase.sin() * 0.5) as f32;
        data.push(value);
        data.push(value);
    }
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(rate);
    deck.set_keylock(true);
    deck.attach(Box::new(CachedSource::new(data.into(), rate)), None);
    deck.seek(frames - 4 * u64::from(rate));
    deck.set_tempo(1.04);
    assert!(deck.play(), "the deck plays");
    let mut block = vec![0.0f32; 512 * 2];
    for _ in 0..100 {
        deck.render(&mut block);
    }
    assert!(deck.set_reverse(true), "reverse under key lock");
    let mut crossings = 0usize;
    let mut last = 0.0f32;
    let blocks = 200;
    for _ in 0..blocks {
        deck.render(&mut block);
        for frame in 0..512 {
            let sample = block[frame * 2];
            if last < 0.0 && sample >= 0.0 {
                crossings += 1;
            }
            last = sample;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let seconds = (blocks as f64 * 512.0) / f64::from(rate);
    let pitch = crossings as f64 / seconds;
    let cents = 1200.0 * (pitch / 440.0).log2();
    assert!(
        cents.abs() < 5.0,
        "reverse under key lock put the tone {cents:.1} cents off 440 Hz"
    );
}

#[test]
fn reverse_works_while_slip_is_on() {
    // Slip and reverse answer different questions: slip is where the track's own clock stands, reverse
    // is which way the material is read. Both at once must work — the sound walks back while the
    // virtual head runs on, and that is the whole point of the pair.
    let frames = 200_000;
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(44_100);
    deck.set_keylock(false);
    deck.attach(
        Box::new(CachedSource::new(ramp_material(frames), 44_100)),
        None,
    );
    deck.seek(100_000);
    assert!(deck.play(), "the deck plays");
    deck.set_slip(true);
    assert!(deck.slip(), "slip is on");
    let mut block = vec![0.0f32; 512 * 2];
    for _ in 0..40 {
        deck.render(&mut block);
    }
    let head_before = deck.slip_frame().expect("slip publishes its head");
    assert!(
        deck.set_reverse(true),
        "slip must not stop reverse: the two are independent"
    );
    deck.render(&mut block);
    let step = block[0] - block[2];
    assert!(
        step > 0.0,
        "the material must descend under slip as well: first {}, next {}",
        block[0],
        block[2]
    );
    let head_after = deck.slip_frame().expect("slip still publishes its head");
    assert!(
        head_after > head_before,
        "the virtual head keeps its own forward pace while the sound walks back: {} then {}",
        head_before,
        head_after
    );
    // Leaving slip puts the sound on the track's own clock, which is ahead: that is a jump forward,
    // and a jump ends reverse rather than fighting it.
    deck.set_slip(false);
    assert!(
        !deck.reverse(),
        "leaving slip ends reverse, because the sound moves forward"
    );
}

#[test]
fn a_streamed_track_goes_backwards_through_a_window_the_engine_fills() {
    // A decoded track cannot be walked backwards from the audio callback, so the deck asks for a window
    // and waits: silence, not a stall, and not the end of the file either.
    let frames = 200_000;
    let mut deck = code_deck(frames);
    deck.set_keylock(false);
    deck.seek(100_000);
    assert!(deck.play(), "the deck plays");
    let mut block = vec![0.0f32; 512 * 2];
    for _ in 0..40 {
        deck.render(&mut block);
    }
    assert!(
        deck.set_reverse(true),
        "a streamed track accepts reverse: the engine brings the material"
    );
    deck.render(&mut block);
    assert!(
        deck.reverse(),
        "waiting for a window is not the end of the file"
    );
    assert!(
        block.iter().all(|sample| *sample == 0.0),
        "with no window there is nothing to play, and silence is the honest answer"
    );

    let (start, want) = deck
        .reverse_window_needed()
        .expect("the deck asks the engine for a window");
    assert!(want > 0, "a window has material");
    assert!(
        deck.reverse_window_needed().is_none(),
        "a deck that has asked once is not asking again every block"
    );

    // What the engine would do: read forward from `start` and hand it back.
    let mut material = Vec::with_capacity(usize::try_from(want).expect("a window that fits") * 2);
    for frame in start..start + want {
        #[allow(clippy::cast_precision_loss)]
        let value = frame as f32;
        material.push(value);
        material.push(value);
    }
    deck.fill_reverse_window(start, &material);
    deck.render(&mut block);
    let step = block[0] - block[2];
    assert!(
        step > 0.0,
        "the window must play backwards: first {}, next {}",
        block[0],
        block[2]
    );
    assert!(
        block[0] > block[2],
        "the first frame heard is the last frame of the window"
    );
}

#[test]
fn a_window_that_runs_out_asks_for_the_next_one() {
    let frames = 400_000;
    let mut deck = code_deck(frames);
    deck.set_keylock(false);
    deck.seek(300_000);
    assert!(deck.play(), "the deck plays");
    let mut block = vec![0.0f32; 512 * 2];
    for _ in 0..20 {
        deck.render(&mut block);
    }
    assert!(deck.set_reverse(true), "reverse on a streamed track");
    let (start, want) = deck.reverse_window_needed().expect("the first window");
    let mut material = Vec::with_capacity(usize::try_from(want).expect("a window") * 2);
    for frame in start..start + want {
        #[allow(clippy::cast_precision_loss)]
        let value = frame as f32;
        material.push(value);
        material.push(value);
    }
    deck.fill_reverse_window(start, &material);

    // Play the window out; the deck must ask for the next one, and it must end where this one began.
    let mut asked = None;
    for _ in 0..(want as usize / 512 + 4) {
        deck.render(&mut block);
        if let Some((next, next_frames)) = deck.reverse_window_needed() {
            asked = Some((next, next_frames));
            break;
        }
    }
    let (next_start, next_frames) = asked.expect("the deck asks for the next window");
    assert_eq!(
        next_start + next_frames,
        start,
        "the next window ends where this one started, so the material joins and does not repeat"
    );
}

#[test]
fn a_deck_on_the_cache_plays_the_same_frames_as_a_deck_on_the_stream() {
    let frames = 400_000;
    let mut streamed = code_deck(frames);
    let mut cached = code_deck(frames);
    assert!(
        cached.swap_source(Box::new(CachedSource::new(code_material(frames), 44_100))),
        "the cache is a reader like any other"
    );
    streamed.play();
    cached.play();
    let mut block = vec![0.0f32; 512];
    let mut reference = vec![0.0f32; 512];
    for _ in 0..200 {
        streamed.render(&mut block);
        cached.render(&mut reference);
        assert_eq!(
            block, reference,
            "the cache must not change a single sample"
        );
    }
}

#[test]
fn swapping_to_the_cache_mid_play_keeps_the_next_frame() {
    // The handover happens while the deck reads ahead of what is heard, so the new reader has to stand
    // at the read position, not at the heard one: with the key locked those differ by the stretcher's
    // own delay, and either would be heard as a jump.
    for keylock in [false, true] {
        let frames = 400_000;
        let mut streamed = code_deck(frames);
        let mut swapped = code_deck(frames);
        streamed.set_keylock(keylock);
        swapped.set_keylock(keylock);
        streamed.play();
        swapped.play();
        let mut block = vec![0.0f32; 512];
        let mut reference = vec![0.0f32; 512];
        for _ in 0..20 {
            streamed.render(&mut block);
            swapped.render(&mut reference);
        }
        assert!(
            swapped.swap_source(Box::new(CachedSource::new(code_material(frames), 44_100))),
            "the switch has to be possible while the deck plays"
        );
        for round in 0..40 {
            streamed.render(&mut block);
            swapped.render(&mut reference);
            assert_eq!(
                block, reference,
                "block {round} differs after the handover (keylock {keylock})"
            );
        }
    }
}

#[test]
fn a_beat_jump_lands_on_the_beat_it_promised() {
    // Mid-beat start: the jump is asked for one beat, and the grid says where that beat is. Landing
    // 22 050 frames on would keep the playhead off the beat for ever, which is what a DJ hears as
    // "the jump does not hit".
    let mut deck = grid_deck();
    let inside = 10_000 + 3 * 22_050 + 7_000;
    deck.seek(inside);
    let landed = deck.beat_jump(1.0, true).expect("a jump forward");
    assert_eq!(
        landed,
        10_000 + 4 * 22_050,
        "the jump must land on the next beat, not a beat-length past the play head"
    );
    assert_eq!(deck.position(), landed);
}

#[test]
fn a_fractional_jump_lands_on_the_sub_beat() {
    // A quarter is a real DJ step, and quantizing it to whole beats would make it impossible.
    let mut deck = grid_deck();
    deck.seek(10_000 + 4 * 22_050);
    let landed = deck.beat_jump(0.25, true).expect("a quarter of a beat");
    assert_eq!(landed, 10_000 + 93_713, "a quarter past the fourth beat");
}

#[test]
fn a_jump_backwards_stays_on_the_grid() {
    let mut deck = grid_deck();
    deck.seek(10_000 + 8 * 22_050 + 1_000);
    let landed = deck.beat_jump(-4.0, true).expect("a jump back");
    assert_eq!(landed, 10_000 + 4 * 22_050);
}

#[test]
fn a_jump_without_quantize_moves_by_exactly_the_distance_asked() {
    // Quantize off means a plain step of time: the playhead keeps whatever offset it had inside the
    // beat, which is what a DJ nudging a mix by ear relies on.
    let mut deck = grid_deck();
    let start = 10_000 + 3 * 22_050 + 7_000;
    deck.seek(start);
    let landed = deck.beat_jump(1.5, false).expect("a jump forward");
    assert_eq!(landed, start + 33_075);
}

#[test]
fn a_deck_without_a_grid_refuses_a_quantized_jump() {
    // A jump that promises a beat and has no grid to land on is a lie: better a refusal the shell can
    // show than a jump to nowhere.
    let mut deck = deck_with_ramp(4_000_000);
    deck.seek(100_000);
    assert!(
        deck.beat_jump(1.0, true).is_none(),
        "a jump on the grid needs a grid"
    );
}

#[test]
fn a_jump_past_the_end_stops_at_the_last_frame() {
    let mut deck = grid_deck();
    let total = 4_000_000;
    deck.seek(total - 50_000);
    let landed = deck.beat_jump(32.0, false).expect("a long jump forward");
    // With the key locked the last frames are the stretcher's look-ahead: they are read, not heard, so
    // the furthest the play head can go is that much short of the end.
    let last = total - 1 - deck.keylock_latency() as u64;
    assert_eq!(landed, last, "the play head cannot leave the track");
}

#[test]
fn leaving_a_loop_under_slip_lands_on_the_virtual_head() {
    // Slip's other half, the one a DJ calls Flux: the loop is a detour, and dropping it puts the sound
    // back on the track's own clock instead of leaving it inside the repeat.
    let mut deck = deck_with_ramp(4_000_000);
    deck.set_keylock(false);
    deck.set_loop_mode(true);
    let region = FrameRange::new(10_000, 22_000).expect("a twelve-thousand-frame loop");
    assert!(deck.set_loop(Some(region)));
    deck.seek(10_000);
    deck.set_slip(true);
    deck.play();
    let mut block = vec![0.0f32; 256];
    for _ in 0..300 {
        deck.render(&mut block);
    }
    assert_eq!(deck.slip_frame(), Some(48_400));
    deck.set_loop_mode(false);
    assert_eq!(
        deck.position(),
        48_400,
        "dropping the loop puts the sound where the track would be"
    );
    assert!(!deck.loop_mode(), "the mode is still what the button said");
}

#[test]
fn leaving_a_loop_without_slip_keeps_the_sound_where_it_is() {
    // The guard on the rule above: without slip a loop stays a loop, and switching the mode off plays on
    // from inside it. A jump here would be the deck inventing a feature nobody asked for.
    let mut deck = deck_with_ramp(4_000_000);
    deck.set_keylock(false);
    deck.set_loop_mode(true);
    let region = FrameRange::new(10_000, 22_000).expect("a twelve-thousand-frame loop");
    assert!(deck.set_loop(Some(region)));
    deck.seek(10_000);
    deck.play();
    let mut block = vec![0.0f32; 256];
    for _ in 0..300 {
        deck.render(&mut block);
    }
    let inside = deck.position();
    deck.set_loop_mode(false);
    assert_eq!(
        deck.position(),
        inside,
        "the sound stays where the loop left it"
    );
    assert_eq!(deck.slip_frame(), None, "nothing slips when slip is off");
}

#[test]
fn a_slip_jump_keeps_the_virtual_head_running() {
    // Slip: the sound may jump, the track may not. A hotcue under slip moves what is heard while the
    // virtual head carries on as if nothing had happened; letting slip go puts the sound where the
    // track would be — that is the whole model, and it is what makes slip safe to press mid-mix.
    let mut deck = deck_with_ramp(4_000_000);
    deck.set_keylock(false);
    deck.set_hotcue(1, 500_000);
    deck.seek(100_000);
    deck.play();
    let mut block = vec![0.0f32; 256];
    for _ in 0..10 {
        deck.render(&mut block);
    }
    deck.set_slip(true);
    assert_eq!(
        deck.slip_frame(),
        Some(101_280),
        "slip starts where the sound is"
    );

    deck.jump_hotcue(1).expect("the cue is there");
    assert_eq!(deck.position(), 500_000, "the sound jumps to the cue");
    for _ in 0..10 {
        deck.render(&mut block);
    }
    assert_eq!(
        deck.slip_frame(),
        Some(102_560),
        "the virtual head never jumped"
    );
    assert_eq!(deck.position(), 501_280, "the sound plays on from the cue");

    deck.set_slip(false);
    assert_eq!(
        deck.position(),
        102_560,
        "letting slip go puts the sound where the track would be"
    );
}

#[test]
fn a_slip_loop_returns_to_where_the_track_would_be() {
    // Flux: the loop repeats what is heard while the virtual head walks straight through it. Three
    // and a bit times round the loop later, letting go lands past the loop, not at its start.
    let mut deck = deck_with_ramp(4_000_000);
    deck.set_keylock(false);
    deck.set_loop_mode(true);
    let region = FrameRange::new(10_000, 22_000).expect("a twelve-thousand-frame loop");
    assert!(deck.set_loop(Some(region)));
    deck.seek(10_000);
    deck.set_slip(true);
    deck.play();
    let mut block = vec![0.0f32; 256];
    for _ in 0..300 {
        deck.render(&mut block);
    }
    assert!(
        region.contains(SourceFrame(deck.position())),
        "the sound stays in the loop, got {}",
        deck.position()
    );
    assert_eq!(
        deck.slip_frame(),
        Some(48_400),
        "the virtual head went straight through the loop"
    );
    deck.set_slip(false);
    assert_eq!(deck.position(), 48_400);
}

#[test]
fn a_nudge_bends_the_rate_and_leaves_the_fader_alone() {
    // A hand on the platter: the speed changes while it is held, the play head moves only by the
    // material that was played, and letting go returns the deck to the fader's own rate. The block is
    // 128 stereo frames, so ten of them are 1 280 frames at the file's own speed.
    let mut deck = deck_with_ramp(4_000_000);
    deck.set_keylock(false);
    deck.play();
    let mut block = vec![0.0f32; 256];
    deck.render(&mut block);
    let before = deck.position();
    deck.set_nudge(1.0);
    assert!((deck.nudge() - 1.0).abs() < 1e-9);
    for _ in 0..10 {
        deck.render(&mut block);
    }
    let bent = deck.position() - before;
    // Four per cent faster: 1 331 frames instead of 1 280.
    assert!(
        (1_327..=1_335).contains(&bent),
        "the nudge must bend the rate, got {bent} frames"
    );
    assert!(
        (deck.tempo_target() - 1.0).abs() < 1e-9,
        "the fader keeps its value while the platter is bent"
    );
    assert!(
        (deck.tempo_ratio() - 1.04).abs() < 1e-9,
        "the applied rate is the fader with the bend on top"
    );

    let held = deck.position();
    deck.set_nudge(0.0);
    for _ in 0..10 {
        deck.render(&mut block);
    }
    assert_eq!(
        deck.position() - held,
        1_280,
        "letting go returns the deck to its own rate"
    );
}

#[test]
fn a_nudge_stays_inside_the_range_it_promises() {
    let mut deck = deck_with_ramp(4_000_000);
    deck.set_nudge(9.0);
    assert!(
        (deck.nudge() - 1.0).abs() < 1e-9,
        "a nudge stops at the end of its range"
    );
    deck.set_nudge(-9.0);
    assert!((deck.nudge() + 1.0).abs() < 1e-9);
    deck.set_nudge(f64::NAN);
    assert!(
        deck.nudge().abs() < 1e-9,
        "a nudge that is not a number is no nudge"
    );
}

#[test]
fn a_deck_at_its_own_speed_renders_the_same_bytes_as_before_the_tempo_stage() {
    // The golden render compares bytes: a tempo stage that touched the signal at unity would move every
    // baseline in the repository.
    let mut deck = deck_with_ramp(48_000);
    deck.play();
    let mut block = vec![0.0f32; 512];
    deck.render(&mut block);
    let mut reference = vec![0.0f32; 512];
    deck_with_ramp(48_000);
    let mut other = deck_with_ramp(48_000);
    other.play();
    other.render(&mut reference);
    assert_eq!(block, reference);
    assert_eq!(deck.tempo_ratio(), 1.0);
}

#[test]
fn a_faster_tempo_plays_the_material_faster_and_says_so() {
    // Without key lock the tempo stage is a plain resampling, and its timing is exact to the frame.
    let mut deck = deck_with_ramp(48_000);
    deck.set_keylock(false);
    deck.set_tempo(1.08);
    assert_eq!(deck.tempo_ratio(), 1.08);
    deck.play();
    // A thousand frames is two thousand samples: the block is interleaved stereo.
    let mut block = vec![0.0f32; 2000];
    deck.render(&mut block);
    // The playhead moved through eight per cent more material than it played.
    let played = deck.position() as f64;
    assert!(
        (played - 1080.0).abs() < 3.0,
        "1000 frames at 1.08 advanced the source to {played}"
    );
    // And the value at the end of the block is the material from the end of that advance, not the
    // material 1000 frames in: the ramp encodes its own frame number.
    let last = block[block.len() - 2] as f64;
    let expected = f64::from(ramp(1079));
    assert!(
        (last - expected).abs() < 1e-4,
        "the last frame played {last} instead of {expected}"
    );
}

#[test]
fn a_slower_tempo_stretches_the_material() {
    let mut deck = deck_with_ramp(48_000);
    deck.set_keylock(false);
    deck.set_tempo(0.92);
    deck.play();
    let mut block = vec![0.0f32; 2000];
    deck.render(&mut block);
    let played = deck.position() as f64;
    assert!(
        (played - 920.0).abs() < 3.0,
        "1000 frames at 0.92 advanced the source to {played}"
    );
    let last = block[block.len() - 2] as f64;
    let expected = f64::from(ramp(919));
    assert!(
        (last - expected).abs() < 1e-4,
        "the last frame played {last} instead of {expected}"
    );
}

#[test]
fn a_tone_from_a_forty_four_one_file_does_not_fold_back_on_a_forty_eight_device() {
    // 15 kHz sits close to the 44.1 kHz file's own Nyquist. Converting the rate by interpolating between
    // neighbouring samples leaves an image at 44100 - 15000 = 29.1 kHz, and the 48 kHz output folds that
    // down to 18.9 kHz — a whistle four kilohertz away from the tone, on every file the deck opens.
    let mut deck = Deck::new(DeckId::A);
    deck.set_output_rate(48_000);
    deck.attach(
        Box::new(ToneAtRate {
            frames: 44_100 * 2,
            position: 0,
            rate: 44_100,
            hertz: 15_000.0,
        }),
        None,
    );
    deck.play();
    let mut block = vec![0.0f32; 256 * 2];
    let mut all = Vec::new();
    for _ in 0..188 {
        deck.render(&mut block);
        all.extend_from_slice(&block);
    }
    let skip = deck.keylock_latency() + 2400;
    let image = purity(&all[skip * 2..], 18_900.0);
    let tone = purity(&all[skip * 2..], 15_000.0);
    assert!(
        tone > 0.9,
        "the tone itself should still be there, got {tone:.3}"
    );
    assert!(
        image < 0.001,
        "the image at 18.9 kHz carries {image:.4} of the energy — the rate conversion is folding it back"
    );
}

/// Share of the block's energy that sits exactly at `hertz`.
///
/// A stretched sine that keeps its pitch still sounds rough if the windows are placed badly: the damage
/// is not a wrong frequency but a spread of energy away from the one the file has. Zero crossings cannot
/// see that; this can.
fn purity(block: &[f32], hertz: f64) -> f64 {
    let frames = block.len() / 2;
    let mut re = 0.0f64;
    let mut im = 0.0f64;
    let mut energy = 0.0f64;
    for index in 0..frames {
        let value = f64::from(block[index * 2]);
        let phase = std::f64::consts::TAU * hertz * index as f64 / 48_000.0;
        re += value * phase.cos();
        im += value * phase.sin();
        energy += value * value;
    }
    if energy <= 0.0 {
        return 0.0;
    }
    2.0 * (re * re + im * im) / frames as f64 / energy
}

#[test]
fn at_the_files_own_speed_a_key_locked_deck_keeps_time() {
    // The report from the booth: with the key locked, a track at 1.00 runs about 0,78 % fast — 13 cents.
    // The cause is in the staging length, not in the stretcher: the deck asks for `frames * tempo` of
    // material and adds two frames of slack, so the library is told 258 frames in for 256 out at a tempo
    // of 1,00. Two frames per block is one part in 128, forever.
    let mut deck = tone_deck(4_000_000);
    deck.set_tempo(1.08);
    deck.play();
    let mut block = vec![0.0f32; 256 * 2];
    for _ in 0..40 {
        deck.render(&mut block);
    }
    deck.set_tempo(1.0);
    // The rate glides to what the hand asked; let it arrive before the measurement starts.
    for _ in 0..40 {
        deck.render(&mut block);
    }
    for (blocks, allowed) in [(25u64, 1u64), (3125, 4)] {
        let before = deck.position();
        let rendered = blocks * 256;
        for _ in 0..blocks {
            deck.render(&mut block);
        }
        let moved = deck.position().saturating_sub(before);
        assert!(
            moved.abs_diff(rendered) <= allowed,
            "at the file's own speed {rendered} frames out moved the deck {moved} frames of material: \
             {:.4} % off",
            (moved as f64 / rendered as f64 - 1.0) * 100.0
        );
    }
    // And the sound at unity, with the stretcher still in the path, is the tone it was: a ratio of
    // 258/256 also smeared the phase, not only the clock.
    let mut all = Vec::new();
    for _ in 0..188 {
        deck.render(&mut block);
        all.extend_from_slice(&block);
    }
    let skip = deck.keylock_latency();
    let measured = purity(&all[skip * 2..], 440.0);
    assert!(
        measured > 0.99,
        "at the file's own speed only {measured:.3} of the energy stayed at the tone"
    );
}

#[test]
fn a_stretched_tone_stays_clean() {
    // The report from the booth: "жужжание" at any speed other than 1.0. The pitch was right and the
    // window search was centred on the read pointer, which drifts away from the natural continuation by
    // HOP*(ratio-1) per hop — after a second the search never found the seam, and the seams buzzed.
    for ratio in [1.08, 1.01, 0.92] {
        let mut deck = tone_deck(480_000);
        deck.set_tempo(ratio);
        deck.play();
        let mut block = vec![0.0f32; 256 * 2];
        let mut all = Vec::new();
        for _ in 0..188 {
            deck.render(&mut block);
            all.extend_from_slice(&block);
        }
        // The stretcher's own delay is silence by design: skip it before measuring.
        let skip = deck.keylock_latency() + 2400;
        let measured = purity(&all[skip * 2..], 440.0);
        assert!(
            measured > 0.99,
            "at {ratio} only {measured:.3} of the energy stayed at the tone"
        );
    }
}

/// Largest jump between neighbouring samples: a click is a step the material never makes.
fn biggest_step(block: &[f32]) -> f32 {
    let mut worst = 0.0f32;
    let mut previous = block.first().copied().unwrap_or(0.0);
    for frame in block.chunks_exact(2) {
        let value = frame[0];
        worst = worst.max((value - previous).abs());
        previous = value;
    }
    worst
}

#[test]
fn starting_the_tempo_from_playback_does_not_click() {
    // The fader's first movement must not put the stretcher into the path mid-playback: a stage that goes
    // in cold plays silence while it fills its delay, and the edges of that hole are clicks. A 440 Hz tone
    // never steps further than 0,029 from one sample to the next.
    let mut deck = tone_deck(480_000);
    deck.play();
    let mut block = vec![0.0f32; 256 * 2];
    for _ in 0..20 {
        deck.render(&mut block);
    }
    let last_before = block[block.len() - 2];
    deck.set_tempo(1.08);
    let mut after = Vec::new();
    for _ in 0..60 {
        deck.render(&mut block);
        after.extend_from_slice(&block);
    }
    // The seam is the first pair of samples across the change: start from the sample that was playing.
    let mut stream = vec![last_before];
    stream.extend_from_slice(&after);
    let step = biggest_step(&stream);
    assert!(
        step < 0.06,
        "the tempo change stepped {step:.3} in one sample — that is a click"
    );
    let silent = after.chunks_exact(256 * 2).filter(|chunk| {
        let energy: f32 = chunk.iter().map(|value| value * value).sum::<f32>() / chunk.len() as f32;
        energy.sqrt() < 0.01
    });
    assert_eq!(silent.count(), 0, "the tempo change left a hole of silence");
}

#[test]
fn the_pitch_stays_where_the_file_put_it_when_the_tempo_moves() {
    // Key lock is the default (DJP-DECK-008): the deck plays faster, the note stays the note.
    //
    // Measured by where the energy sits, not by counting zero crossings: a stretched tone carries a
    // little high-frequency residue across zero, and the count reads that as extra pitch. Energy at
    // 440 Hz versus energy at 475 Hz (what the pitch *would* be without key lock) separates them.
    let mut fast = tone_deck(480_000);
    assert!(fast.keylock(), "a fresh deck locks the key");
    fast.set_tempo(1.08);
    fast.play();
    // Skip the delay and the fade-in that follows it: both are transitions, not the tone's shape.
    let skip = fast.keylock_latency() + 2400;
    let mut warmup = vec![0.0f32; skip * 2];
    fast.render(&mut warmup);
    let before = fast.read_position();
    let mut moved = vec![0.0f32; 4800 * 2];
    fast.render(&mut moved);
    let window = &moved[..];

    let at_original = purity(window, 440.0);
    let at_moved = purity(window, 440.0 * 1.08);
    assert!(
        at_original > 0.9,
        "the tone is not at 440 Hz any more: {at_original:.3} of the energy"
    );
    assert!(
        at_moved < at_original * 0.2,
        "the pitch moved with the tempo: {at_moved:.3} of the energy sits at 475 Hz"
    );

    // And the material still moves: 4800 frames of output cover about 5184 frames of source. The
    // stretcher works on device-rate material and holds a short history, so the *heard* position
    // trails the read position by that history; both are checked, because reading the right amount
    // and playing it late is a different bug.
    let read = (fast.read_position() - before) as f64;
    assert!(
        (read - 5184.0).abs() < 400.0,
        "4800 frames at 1.08 read {read} frames of source"
    );
    let latency = fast.keylock_latency() as f64;
    let heard = fast.position() as f64;
    let all_read = fast.read_position() as f64;
    // Three things separate the read head from the sound: the stretcher's delay, and the material the
    // resampler has pulled but not yet played (its kernel needs a window, so the read head runs ahead by
    // a few frames and the scratch holds the rest).
    let buffered = all_read - heard - latency;
    assert!(
        latency > 0.0 && (0.0..4096.0).contains(&buffered),
        "the heard position {heard} trails the read position {all_read} by {buffered}, which is not \
         the delay {latency} plus at most a block of buffered material"
    );
}

#[test]
fn a_block_larger_than_the_staging_still_locks_the_key() {
    // The staging buffer holds 8192 frames; a larger block is played in pieces. The pitch has to stay
    // put for that caller too — a fallback to the vinyl path would move it, silently.
    let mut deck = tone_deck(480_000);
    deck.set_tempo(1.08);
    deck.play();
    let skip = deck.keylock_latency() + 2400;
    let mut warmup = vec![0.0f32; skip * 2];
    deck.render(&mut warmup);
    let mut block = vec![0.0f32; 40_000 * 2];
    deck.render(&mut block);
    let at_original = purity(&block, 440.0);
    assert!(
        at_original > 0.9,
        "a 40000-frame block lost the key: {at_original:.3} of the energy at 440 Hz"
    );
}

#[test]
fn a_block_larger_than_the_scratch_still_plays_at_the_tempo() {
    // The vinyl path chunks large blocks too: at 1.08 the source frames for a full scratch do not fit
    // in it, and the size maths has to divide by the step.
    let mut deck = deck_with_ramp(480_000);
    deck.set_keylock(false);
    deck.set_tempo(1.08);
    deck.play();
    let mut block = vec![0.0f32; 40_000 * 2];
    deck.render(&mut block);
    // What the block consumed, not what was read: the resampler pulls a kernel window ahead of the
    // play head, so the read pointer legitimately runs a few frames further.
    let played = deck.position() as f64;
    assert!(
        (played - 40_000.0 * 1.08).abs() < 4.0,
        "a 40000-frame block at 1.08 advanced the source to {played}"
    );
    let last = f64::from(block[block.len() - 2]);
    let expected = f64::from(ramp(43_199));
    assert!(
        (last - expected).abs() < 1e-4,
        "the last frame played {last} instead of {expected}"
    );
}

#[test]
fn without_key_lock_the_pitch_follows_the_rate_as_on_a_turntable() {
    // The vinyl behaviour stays available, and stays measurable: it is what a DJ wants when scratching
    // or when matching a track by ear.
    let mut plain = tone_deck(480_000);
    // Frames and crossings are the transport's contract here; key lock is a separate stage with its
    // own delay, so these measurements ask the vinyl path.
    plain.set_keylock(false);
    plain.play();
    let mut reference = vec![0.0f32; 9600];
    plain.render(&mut reference);
    let baseline = crossings(&reference);

    let mut fast = tone_deck(480_000);
    fast.set_keylock(false);
    fast.set_tempo(1.08);
    fast.play();
    let mut moved = vec![0.0f32; 9600];
    fast.render(&mut moved);
    let ratio = crossings(&moved) as f64 / baseline as f64;
    assert!(
        (ratio - 1.08).abs() < 0.04,
        "without key lock eight per cent faster gave {ratio:.3} times the crossings"
    );
}

#[test]
fn a_seek_drops_the_frames_carried_by_the_tempo_stage() {
    // Without this the first block after a seek would start with a fraction of the material from before
    // it — an audible tick at every jump, and a picture that disagrees with the sound. This is the
    // resampling stage's contract, so it is asked on the vinyl path.
    let mut deck = deck_with_ramp(48_000);
    deck.set_keylock(false);
    deck.set_tempo(1.08);
    deck.play();
    let mut block = vec![0.0f32; 512];
    deck.render(&mut block);

    deck.seek(5_000);
    deck.render(&mut block);
    // The first two milliseconds rise, so the material is read past them; what this test is about — the
    // frames carried from before the seek must be gone — is answered by the frame the rise ends on.
    let rise = deck.loop_config().seam_frames(48_000);
    // The block may open with the prefetcher's silence; the rise starts where the sound does.
    let start = (0..block.len() / 2)
        .find(|index| block[index * 2].abs() > 1e-6)
        .unwrap_or(0);
    let first = block[(start + rise) * 2] as f64;
    // The deck plays at 1,08, so the rise's worth of output covers that much more material.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let material = 5_000 + ((start + rise) as f64 * 1.08).round() as u64;
    let expected = f64::from(ramp(material));
    assert!(
        (first - expected).abs() < 5e-3,
        "the block after the seek started at {first} instead of {expected}"
    );
}

#[test]
fn a_loop_still_wraps_at_tempo() {
    // The wrap belongs to the source, not to the stretcher: the vinyl path shows it without the
    // key-lock delay moving the playhead the assertion reads.
    let mut deck = deck_with_ramp(48_000);
    deck.set_keylock(false);
    deck.set_tempo(1.05);
    deck.set_loop(Some(FrameRange {
        start: SourceFrame(1000),
        end: SourceFrame(2000),
    }));
    deck.seek(1000);
    deck.play();
    let mut block = vec![0.0f32; 512];
    for _ in 0..20 {
        deck.render(&mut block);
    }
    let position = deck.position();
    assert!(
        (1000..2000).contains(&position),
        "the playhead left the loop while playing at tempo: {position}"
    );
    assert!(
        deck.stats().loop_wraps > 0,
        "twenty blocks at 1.05 did not wrap a 1000-frame loop"
    );
}

#[test]
fn the_strip_keeps_a_bar_value_after_the_sound_stops() {
    // The strip's meter is read at the frame rate, not the block rate: a value that lives for four
    // milliseconds is a counter, not a meter. The deck publishes the peak of the block and the bar
    // value that falls back over about half a second.
    let mut deck = tone_deck(480_000);
    deck.play();
    let mut block = vec![0.0f32; 4800]; // 2400 frames: 50 ms
    for _ in 0..12 {
        deck.render(&mut block);
    }
    let loud = deck.levels();
    assert!(
        loud.peak_left > 0.05,
        "the tone should register: {}",
        loud.peak_left
    );
    assert!(
        (loud.held_left - loud.peak_left).abs() < 1e-3,
        "the bar rises to the peak: held {} peak {}",
        loud.held_left,
        loud.peak_left
    );

    deck.pause();
    for _ in 0..40 {
        deck.render(&mut block);
    }
    let quiet = deck.levels();
    assert_eq!(quiet.peak_left, 0.0, "a paused deck has no block peak");
    assert!(
        quiet.held_left > 0.0,
        "the bar must keep falling, not vanish between blocks"
    );
    assert!(
        quiet.held_left < loud.held_left,
        "the bar never falls upward"
    );
}

#[test]
fn a_pad_keeps_its_name_and_its_colour_separately() {
    // Renaming a pad must not wipe the colour the DJ chose for it, and the other way round: the two
    // halves are set independently and an absent half means "leave it alone" (DJP-DECK-003).
    let mut deck = deck_with_ramp(48_000);
    deck.play();
    let mut block = vec![0.0f32; 512];
    deck.render(&mut block);
    deck.store_hotcue(4);

    assert!(deck.set_cue_meta(4, Some("Drop".to_string()), None));
    assert_eq!(deck.cue_labels()[4].as_deref(), Some("Drop"));
    assert_eq!(deck.cue_colors()[4], None);

    assert!(deck.set_cue_meta(4, None, Some("#5fb85f".to_string())));
    assert_eq!(
        deck.cue_labels()[4].as_deref(),
        Some("Drop"),
        "the name survived"
    );
    assert_eq!(deck.cue_colors()[4].as_deref(), Some("#5fb85f"));

    let state = deck.snapshot();
    assert_eq!(state.cue_labels[4].as_deref(), Some("Drop"));
    assert_eq!(state.cue_colors[4].as_deref(), Some("#5fb85f"));
    assert!(state.hotcues[4].is_some());

    // A new track carries its own pads.
    deck.attach(Box::new(RampSource::new(48_000)), None);
    assert_eq!(deck.cue_labels()[4], None);
    assert_eq!(deck.cue_colors()[4], None);
}

#[test]
fn a_new_track_does_not_inherit_the_old_pads() {
    // Loading B over A must not leave A's cues on B's pads: an inherited cue jumps into a position the
    // new track never had, and an inherited name labels a pad that holds nothing.
    let mut deck = deck_with_ramp(48_000);
    deck.play();
    let mut block = vec![0.0f32; 512];
    deck.render(&mut block);
    deck.store_hotcue(3);
    deck.set_cue_meta(3, Some("Drop".to_string()), Some("#5fb85f".to_string()));
    assert!(deck.hotcues()[3].is_some());

    deck.attach(Box::new(RampSource::new(48_000)), None);
    assert_eq!(deck.hotcues()[3], None, "the pad outlived its track");
    assert_eq!(deck.cue_labels()[3], None);
    assert_eq!(deck.cue_colors()[3], None);
}

#[test]
fn a_forty_four_one_file_plays_at_its_own_speed_on_a_forty_eight_device() {
    // The defect this guards: 44.1 kHz material read as if it were 48 kHz plays 8.8 % fast, which the
    // ear reports as "a little faster than it should be" on every file, not as a broken one.
    let mut deck = deck_at_48k(480_000, 44_100);
    // Frames and crossings are the transport's contract here; key lock is a separate stage with its
    // own delay, so these measurements ask the vinyl path.
    deck.set_keylock(false);
    deck.play();
    // Two blocks of 4800 frames: a tenth of a second of device time each.
    let mut block = vec![0.0f32; 9_600];
    for _ in 0..2 {
        deck.render(&mut block);
    }
    let consumed = deck.position() as f64;
    let expected = 9_600.0 * 44_100.0 / 48_000.0;
    assert!(
        (consumed - expected).abs() < 3.0,
        "a fifth of a second of 44.1 kHz material consumed {consumed} source frames, expected {expected}"
    );
}

#[test]
fn a_forty_four_one_tone_keeps_its_pitch_on_a_forty_eight_device() {
    // Same tone, same half second, two rates: the device must hear the same number of cycles. Reading
    // 44.1 kHz as 48 kHz would multiply the pitch by 1.088.
    let mut native = deck_at_48k(480_000, 48_000);
    // Frames and crossings are the transport's contract here; key lock is a separate stage with its
    // own delay, so these measurements ask the vinyl path.
    native.set_keylock(false);
    native.play();
    let mut reference = vec![0.0f32; 19_200];
    native.render(&mut reference);

    let mut foreign = deck_at_48k(480_000, 44_100);
    // Frames and crossings are the transport's contract here; key lock is a separate stage with its
    // own delay, so these measurements ask the vinyl path.
    foreign.set_keylock(false);
    assert!(
        (foreign.rate_ratio() - 44_100.0 / 48_000.0).abs() < 1e-9,
        "the deck did not notice the file's rate: {}",
        foreign.rate_ratio()
    );
    foreign.play();
    let mut converted = vec![0.0f32; 19_200];
    foreign.render(&mut converted);

    let baseline = crossings(&reference);
    let measured = crossings(&converted);
    let ratio = measured as f64 / baseline as f64;
    assert!(
        (ratio - 1.0).abs() < 0.02,
        "a 1 kHz tone recorded at 44.1 kHz came out at {ratio:.3} times the pitch ({baseline} -> {measured})"
    );
}
