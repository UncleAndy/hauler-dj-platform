//! Autosave that never stands in the audio path (DJP-SET-003).
//!
//! The caller hands over a document whenever something changed. Requests are coalesced: only the
//! freshest document is worth writing, and a set-up that changes ten times a second must not turn
//! into ten writes a second. The write itself happens on a worker thread, so the thread that owns the
//! UI or the audio graph never waits for a disk.

use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

use crate::doc::SessionDoc;
use crate::store::{SessionError, SessionStore};

enum Command {
    Save(Box<SessionDoc>),
    Flush(Sender<Result<(), String>>),
}

/// A background writer with one pending document at a time.
pub struct Autosave {
    sender: Sender<Command>,
    worker: Option<JoinHandle<()>>,
}

impl Autosave {
    /// Start a worker for this store.
    pub fn start(store: SessionStore) -> Self {
        let (sender, receiver) = mpsc::channel::<Command>();
        let worker = std::thread::Builder::new()
            .name("dj-session-autosave".to_owned())
            .spawn(move || {
                while let Ok(first) = receiver.recv() {
                    match first {
                        Command::Save(document) => {
                            let mut latest = document;
                            // Everything already queued is older than what is in hand: skip it.
                            while let Ok(next) = receiver.try_recv() {
                                match next {
                                    Command::Save(document) => latest = document,
                                    Command::Flush(reply) => {
                                        let result =
                                            store.save(&latest).map_err(|error| error.to_string());
                                        let _ = reply.send(result);
                                    }
                                }
                            }
                            let _ = store.save(&latest);
                        }
                        Command::Flush(reply) => {
                            let _ = reply.send(Ok(()));
                        }
                    }
                }
            })
            .ok();
        Self { sender, worker }
    }

    /// Queue a document. Never blocks, never fails because the disk is slow.
    pub fn request(&self, document: &SessionDoc) {
        let _ = self.sender.send(Command::Save(Box::new(document.clone())));
    }

    /// Wait until everything queued so far is on disk.
    pub fn flush(&self) -> Result<(), SessionError> {
        let (reply, answer) = mpsc::channel();
        if self.sender.send(Command::Flush(reply)).is_err() {
            return Ok(());
        }
        match answer.recv() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(SessionError::Parse(serde_json::Error::io(
                std::io::Error::other(message),
            ))),
            Err(_) => Ok(()),
        }
    }
}

impl Drop for Autosave {
    fn drop(&mut self) {
        let _ = self.flush();
        // Close the channel so the worker's `recv` returns, then wait for it: a caller that deletes
        // its directory right after must not race a write that is still in flight.
        let (closed, _) = mpsc::channel();
        let sender = std::mem::replace(&mut self.sender, closed);
        drop(sender);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
