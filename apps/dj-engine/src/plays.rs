//! Writes finished plays into the index (DJP-MEDIA-004), off the audio path.
//!
//! The same shape as the metadata writer: the publisher hands over a finished play, this thread turns
//! it into a row. A database write in the callback is not allowed, and a play the DJ made and the
//! engine forgot is the one failure this module exists to prevent — so the queue is drained on the way
//! out, exactly like `meta::MetaWriter`.

use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dj_library::{Index, PlayRecord};

/// How long the shutdown waits for the last play to land before saying it gave up.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// The wire type and the stored type are two types — the library keeps its own, the way it keeps
/// `TrackRow` — and this is the one place that knows both.
pub fn record_of(play: &dj_domain::PlayRow) -> PlayRecord {
    PlayRecord {
        fingerprint: play.fingerprint.clone(),
        path: play.path.clone(),
        deck: play.deck,
        started_at_ms: play.started_at_ms,
        seconds: play.seconds,
    }
}

/// The thread that turns finished plays into rows.
pub struct PlayWriter {
    /// Our own sender: dropping it closes the queue, which is what ends the thread.
    queue: Option<Sender<PlayRecord>>,
    thread: Option<JoinHandle<()>>,
}

impl PlayWriter {
    /// Start the writer. The index is opened in the thread, once, when the first play arrives: a run
    /// that never plays anything never touches the database.
    pub fn spawn() -> (Self, Sender<PlayRecord>) {
        let (queue, receiver) = mpsc::channel::<PlayRecord>();
        let handoff = queue.clone();
        let thread = thread::Builder::new()
            .name("dj-plays".to_string())
            .spawn(move || {
                let mut index: Option<Index> = None;
                while let Ok(play) = receiver.recv() {
                    if index.is_none() {
                        index = Index::open_default().ok();
                    }
                    match index.as_mut() {
                        Some(index) => {
                            if let Err(error) = index.record_play(&play) {
                                eprintln!(
                                    "dj-engine: the play of {} was not recorded: {error}",
                                    play.path
                                );
                            }
                        }
                        None => {
                            eprintln!("dj-engine: no index to record the play of {} in", play.path)
                        }
                    }
                }
            })
            .ok();
        (
            Self {
                queue: Some(queue),
                thread,
            },
            handoff,
        )
    }
}

impl Drop for PlayWriter {
    fn drop(&mut self) {
        // Closing our side is what ends the queue: the thread finishes what is already in it.
        drop(self.queue.take());
        let Some(thread) = self.thread.take() else {
            return;
        };
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while !thread.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(2));
        }
        if thread.is_finished() {
            let _ = thread.join();
        } else {
            eprintln!("dj-engine: the play writer is still busy; the last play may be lost");
        }
    }
}
