//! Importing a batch of files: several workers, one progress line, one cancel switch.
//!
//! Files are independent — reading tags, copying, writing metadata — so a batch is a job list with an
//! index that workers take from. The number of workers is a setting: one is a legitimate choice on a
//! slow disk, where parallel reads only make each other wait.
//!
//! Progress is a cheap snapshot (atomics plus one mutex for the current path), because the caller is a
//! UI publishing at frame rate, not a logger.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::import::{self, Imported};

/// How many workers a batch should use. `0` means one per core.
pub fn threads_for(requested: usize) -> usize {
    if requested > 0 {
        return requested;
    }
    std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
}

/// What a running batch has done so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub running: bool,
    /// The file a worker is on. With several workers this is whichever started last: it names the
    /// batch's whereabouts, not the only thing happening.
    pub current: Option<PathBuf>,
    pub cancelled: bool,
}

impl Progress {
    /// Files that reached a verdict, either way.
    pub fn finished(&self) -> usize {
        self.done.saturating_add(self.failed)
    }

    /// Done and failed as a share of the batch, 0…1.
    pub fn share(&self) -> f64 {
        if self.total == 0 {
            return 1.0;
        }
        self.finished() as f64 / self.total as f64
    }
}

/// What a finished batch produced.
#[derive(Debug, Default)]
pub struct Report {
    /// Imports that landed, in path order.
    pub imported: Vec<Imported>,
    /// Files that did not land, with the reason.
    pub failures: Vec<(PathBuf, String)>,
    pub cancelled: bool,
    pub elapsed_ms: u64,
}

impl Report {
    pub fn total(&self) -> usize {
        self.imported.len().saturating_add(self.failures.len())
    }
}

struct Shared {
    root: PathBuf,
    jobs: Vec<PathBuf>,
    now_ms: u64,
    next: AtomicUsize,
    done: AtomicUsize,
    failed: AtomicUsize,
    cancelled: AtomicBool,
    running: AtomicBool,
    current: Mutex<Option<PathBuf>>,
    imported: Mutex<Vec<Imported>>,
    failures: Mutex<Vec<(PathBuf, String)>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A worker that panicked leaves the data as it was; the batch keeps going rather than losing
    // everything already imported.
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// A batch of imports running on several workers.
pub struct Run {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
    started: Instant,
}

impl Run {
    /// Start importing `jobs` into `root`. `threads` is clamped to the number of jobs.
    pub fn start(
        root: impl Into<PathBuf>,
        jobs: Vec<PathBuf>,
        threads: usize,
        now_ms: u64,
    ) -> Self {
        let shared = Arc::new(Shared {
            root: root.into(),
            next: AtomicUsize::new(0),
            done: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            running: AtomicBool::new(true),
            current: Mutex::new(None),
            imported: Mutex::new(Vec::new()),
            failures: Mutex::new(Vec::new()),
            now_ms,
            jobs,
        });
        let workers = threads_for(threads).clamp(1, shared.jobs.len().max(1));
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let shared = Arc::clone(&shared);
            handles.push(std::thread::spawn(move || work(&shared)));
        }
        Self {
            shared,
            workers: handles,
            started: Instant::now(),
        }
    }

    /// A snapshot for the UI: cheap, and true as of this call.
    pub fn progress(&self) -> Progress {
        Progress {
            total: self.shared.jobs.len(),
            done: self.shared.done.load(Ordering::Relaxed),
            failed: self.shared.failed.load(Ordering::Relaxed),
            running: self.shared.running.load(Ordering::Relaxed),
            current: lock(&self.shared.current).clone(),
            cancelled: self.shared.cancelled.load(Ordering::Relaxed),
        }
    }

    /// Stop taking new files. The file a worker is on is finished: half an import is worse than none.
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::Relaxed);
    }

    /// Wait for the workers and collect what happened.
    pub fn join(self) -> Report {
        for worker in self.workers {
            let _ = worker.join();
        }
        let mut imported = lock(&self.shared.imported).clone();
        imported.sort_by(|left, right| left.path.cmp(&right.path));
        let mut failures = lock(&self.shared.failures).clone();
        failures.sort_by(|left, right| left.0.cmp(&right.0));
        Report {
            imported,
            failures,
            cancelled: self.shared.cancelled.load(Ordering::Relaxed),
            elapsed_ms: self.started.elapsed().as_millis() as u64,
        }
    }
}

fn work(shared: &Shared) {
    loop {
        if shared.cancelled.load(Ordering::Relaxed) {
            break;
        }
        let index = shared.next.fetch_add(1, Ordering::Relaxed);
        let Some(path) = shared.jobs.get(index) else {
            break;
        };
        *lock(&shared.current) = Some(path.clone());
        let now_ms = shared.now_ms.saturating_add(index as u64 * 1_000);
        match import::import(path, &shared.root, now_ms) {
            Ok(imported) => {
                lock(&shared.imported).push(imported);
                shared.done.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => {
                lock(&shared.failures).push((path.clone(), error.to_string()));
                shared.failed.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    *lock(&shared.current) = None;
    shared.running.store(false, Ordering::Relaxed);
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn zero_threads_means_one_per_core_and_never_none() {
        assert!(threads_for(0) >= 1);
        assert_eq!(threads_for(3), 3);
    }

    #[test]
    fn share_counts_both_verdicts() {
        let progress = Progress {
            total: 4,
            done: 1,
            failed: 1,
            ..Progress::default()
        };
        assert_eq!(progress.finished(), 2);
        assert!((progress.share() - 0.5).abs() < f64::EPSILON);
        assert!((Progress::default().share() - 1.0).abs() < f64::EPSILON);
    }
}
