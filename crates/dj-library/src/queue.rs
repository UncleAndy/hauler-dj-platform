//! Importing a batch of files: several workers, one progress line, one cancel switch.
//!
//! Files are independent — reading tags, decoding for the fingerprint, copying, writing metadata — so a
//! batch is a job list with an index that workers take from. The number of workers is a setting: one is
//! a legitimate choice on a slow disk, where parallel reads only make each other wait.
//!
//! With a catalogue the batch also answers the duplicate question (DJP-MEDIA-005): a file that is the
//! same recording as one already in the collection is reported and not copied, a file that is another
//! edition is copied and reported.
//!
//! Progress is a cheap snapshot (atomics plus one mutex for the current path), because the caller is a
//! UI publishing at frame rate, not a logger.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::identity::{Catalog, Verdict};
use crate::import::{self, Imported, Outcome, Policy};

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
    /// Files that turned out to be the same recording as one already in the collection.
    pub duplicates: usize,
    pub running: bool,
    /// The file a worker is on. With several workers this is whichever started last: it names the
    /// batch's whereabouts, not the only thing happening.
    pub current: Option<PathBuf>,
    pub cancelled: bool,
}

impl Progress {
    /// Files that reached a verdict, either way.
    pub fn finished(&self) -> usize {
        self.done
            .saturating_add(self.failed)
            .saturating_add(self.duplicates)
    }

    /// Finished as a share of the batch, 0…1.
    pub fn share(&self) -> f64 {
        if self.total == 0 {
            return 1.0;
        }
        self.finished() as f64 / self.total as f64
    }
}

/// A file that is the same recording and the same edition as one already in the collection.
#[derive(Debug, Clone, PartialEq)]
pub struct Duplicate {
    /// The file that was offered.
    pub source: PathBuf,
    /// The track it repeats.
    pub existing: PathBuf,
    pub agreement: f64,
}

/// A file that replaced the copy the collection held, which went to `_duplicates/`.
#[derive(Debug, Clone, PartialEq)]
pub struct Replaced {
    pub source: PathBuf,
    pub rejected: PathBuf,
    pub agreement: f64,
}

/// A file that is the same recording as another edition already in the collection.
#[derive(Debug, Clone, PartialEq)]
pub struct Version {
    pub source: PathBuf,
    pub existing: PathBuf,
    pub agreement: f64,
}

/// What a finished batch produced.
#[derive(Debug, Default)]
pub struct Report {
    /// Imports that landed, in path order.
    pub imported: Vec<Imported>,
    /// Files that did not land, with the reason.
    pub failures: Vec<(PathBuf, String)>,
    /// Files that were not copied because the collection already has them.
    pub duplicates: Vec<Duplicate>,
    /// Files that landed as another edition of a track already there.
    pub versions: Vec<Version>,
    /// Files that replaced the copy the collection held; that copy is in `_duplicates/`.
    pub replaced: Vec<Replaced>,
    pub cancelled: bool,
    pub elapsed_ms: u64,
}

impl Report {
    /// Every file the batch reached a verdict on.
    pub fn total(&self) -> usize {
        self.imported
            .len()
            .saturating_add(self.failures.len())
            .saturating_add(self.duplicates.len())
    }
}

struct Shared {
    root: PathBuf,
    jobs: Vec<PathBuf>,
    now_ms: u64,
    /// When present, each file is checked against the collection and joins it after landing.
    catalog: Option<Mutex<Catalog>>,
    next: AtomicUsize,
    done: AtomicUsize,
    failed: AtomicUsize,
    duplicates: AtomicUsize,
    cancelled: AtomicBool,
    running: AtomicBool,
    current: Mutex<Option<PathBuf>>,
    imported: Mutex<Vec<Imported>>,
    failures: Mutex<Vec<(PathBuf, String)>>,
    found_duplicates: Mutex<Vec<Duplicate>>,
    found_versions: Mutex<Vec<Version>>,
    found_replaced: Mutex<Vec<Replaced>>,
    policy: Policy,
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
    /// Start importing `jobs` into `root`, placing every file without asking about duplicates.
    pub fn start(
        root: impl Into<PathBuf>,
        jobs: Vec<PathBuf>,
        threads: usize,
        now_ms: u64,
    ) -> Self {
        Self::spawn(root.into(), jobs, threads, now_ms, None, Policy::Ask)
    }

    /// Start importing, telling duplicates from versions against `catalog`.
    pub fn start_checked(
        root: impl Into<PathBuf>,
        jobs: Vec<PathBuf>,
        threads: usize,
        now_ms: u64,
        catalog: Catalog,
        policy: Policy,
    ) -> Self {
        Self::spawn(root.into(), jobs, threads, now_ms, Some(catalog), policy)
    }

    fn spawn(
        root: PathBuf,
        jobs: Vec<PathBuf>,
        threads: usize,
        now_ms: u64,
        catalog: Option<Catalog>,
        policy: Policy,
    ) -> Self {
        let shared = Arc::new(Shared {
            root,
            next: AtomicUsize::new(0),
            done: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            duplicates: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            running: AtomicBool::new(true),
            current: Mutex::new(None),
            imported: Mutex::new(Vec::new()),
            failures: Mutex::new(Vec::new()),
            found_duplicates: Mutex::new(Vec::new()),
            found_versions: Mutex::new(Vec::new()),
            found_replaced: Mutex::new(Vec::new()),
            policy,
            catalog: catalog.map(Mutex::new),
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
            duplicates: self.shared.duplicates.load(Ordering::Relaxed),
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
        let mut duplicates = lock(&self.shared.found_duplicates).clone();
        duplicates.sort_by(|left, right| left.source.cmp(&right.source));
        let mut versions = lock(&self.shared.found_versions).clone();
        versions.sort_by(|left, right| left.source.cmp(&right.source));
        let mut replaced = lock(&self.shared.found_replaced).clone();
        replaced.sort_by(|left, right| left.source.cmp(&right.source));
        Report {
            imported,
            failures,
            duplicates,
            versions,
            replaced,
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
        let outcome = match &shared.catalog {
            None => import::import(path, &shared.root, now_ms).map(Outcome::Placed),
            Some(catalog) => import::import_checked(
                path,
                &shared.root,
                now_ms,
                |identity| {
                    lock(catalog)
                        .find(identity)
                        .map(|(found, existing, verdict)| {
                            (found.to_path_buf(), existing.clone(), verdict)
                        })
                },
                shared.policy,
            ),
        };
        match outcome {
            Ok(Outcome::Placed(imported)) => {
                remember(shared, &imported);
                lock(&shared.imported).push(imported);
                shared.done.fetch_add(1, Ordering::Relaxed);
            }
            Ok(Outcome::Version {
                imported,
                existing,
                verdict,
            }) => {
                remember(shared, &imported);
                if let Verdict::Version { agreement } | Verdict::Duplicate { agreement, .. } =
                    verdict
                {
                    lock(&shared.found_versions).push(Version {
                        source: path.clone(),
                        existing,
                        agreement,
                    });
                }
                lock(&shared.imported).push(imported);
                shared.done.fetch_add(1, Ordering::Relaxed);
            }
            Ok(Outcome::Rejected {
                existing,
                stored,
                verdict,
            }) => {
                if let Verdict::Duplicate { agreement, .. } | Verdict::Version { agreement } =
                    verdict
                {
                    lock(&shared.found_duplicates).push(Duplicate {
                        source: path.clone(),
                        existing,
                        agreement,
                    });
                }
                let _ = stored;
                shared.duplicates.fetch_add(1, Ordering::Relaxed);
            }
            Ok(Outcome::Replaced {
                imported,
                rejected,
                verdict,
            }) => {
                if let Verdict::Duplicate { agreement, .. } | Verdict::Version { agreement } =
                    verdict
                {
                    lock(&shared.found_replaced).push(Replaced {
                        source: path.clone(),
                        rejected,
                        agreement,
                    });
                }
                remember(shared, &imported);
                lock(&shared.imported).push(imported);
                shared.done.fetch_add(1, Ordering::Relaxed);
            }
            Ok(Outcome::Duplicate {
                existing, verdict, ..
            }) => {
                if let Verdict::Duplicate { agreement, .. } | Verdict::Version { agreement } =
                    verdict
                {
                    lock(&shared.found_duplicates).push(Duplicate {
                        source: path.clone(),
                        existing,
                        agreement,
                    });
                }
                shared.duplicates.fetch_add(1, Ordering::Relaxed);
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

/// A landed track joins the catalogue, so the next file of the same batch sees it.
fn remember(shared: &Shared, imported: &Imported) {
    if let Some(catalog) = &shared.catalog {
        lock(catalog).add(imported.path.clone(), imported.identity.clone());
    }
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
    fn share_counts_every_verdict() {
        let progress = Progress {
            total: 4,
            done: 1,
            failed: 1,
            duplicates: 1,
            ..Progress::default()
        };
        assert_eq!(progress.finished(), 3);
        assert!((progress.share() - 0.75).abs() < f64::EPSILON);
        assert!((Progress::default().share() - 1.0).abs() < f64::EPSILON);
    }
}
