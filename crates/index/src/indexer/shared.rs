//! State shared between the [`crate::Indexer`] handle, the worker thread and the watcher.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

use everythux_core::snapshot::IndexSnapshot;

use super::{IndexerSettings, Job, JobId};

#[derive(Debug, Default)]
struct State {
    queue: VecDeque<(JobId, Job)>,
    running: Option<(JobId, Job)>,
    pending_settings: Option<IndexerSettings>,
    shutdown: bool,
}

/// What the worker should do next.
#[derive(Debug)]
pub(super) enum Work {
    Job(JobId, Job),
    Settings(Box<IndexerSettings>),
    Shutdown,
    /// The wait deadline passed with nothing to do.
    Timeout,
}

#[derive(Debug)]
pub(super) struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    /// Set to abort the running job; cleared when the next job starts.
    pub(super) cancel: Arc<AtomicBool>,
    next_job: AtomicU64,
    snapshot: RwLock<Arc<IndexSnapshot>>,
}

impl Shared {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            next_job: AtomicU64::new(1),
            snapshot: RwLock::new(Arc::new(IndexSnapshot::empty())),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A panicking worker must not wedge `submit` / `Drop`.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues a job. A `FullReindex` while another one is queued or running returns that job's id.
    pub(super) fn submit(&self, job: Job) -> JobId {
        let mut state = self.lock();
        if job == Job::FullReindex {
            let existing = state
                .running
                .iter()
                .chain(state.queue.iter())
                .find(|(_, j)| *j == Job::FullReindex);
            if let Some((id, _)) = existing {
                return *id;
            }
        }
        // Live-update bursts pile up behind a long scan: merge them.
        if let Job::RefreshDirs(new) = &job
            && let Some((id, Job::RefreshDirs(queued))) = state.queue.back_mut()
        {
            queued.extend(new.iter().cloned());
            queued.sort_unstable();
            queued.dedup();
            return *id;
        }
        let id = JobId(self.next_job.fetch_add(1, Ordering::Relaxed));
        state.queue.push_back((id, job));
        drop(state);
        self.wake.notify_all();
        id
    }

    /// Asks the running job (if any) to stop.
    pub(super) fn cancel_current(&self) {
        if self.lock().running.is_some() {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn update_settings(&self, settings: IndexerSettings) {
        self.lock().pending_settings = Some(settings);
        self.wake.notify_all();
    }

    pub(super) fn shutdown(&self) {
        self.lock().shutdown = true;
        self.cancel.store(true, Ordering::Relaxed);
        self.wake.notify_all();
    }

    pub(super) fn is_shutdown(&self) -> bool {
        self.lock().shutdown
    }

    pub(super) fn has_queued(&self) -> bool {
        !self.lock().queue.is_empty()
    }

    /// Marks the running job as done.
    pub(super) fn finish_running(&self) {
        self.lock().running = None;
    }

    /// Blocks until there is work (shutdown first, then settings, then jobs in order) or
    /// `deadline` passes.
    pub(super) fn next_work(&self, deadline: Option<Instant>) -> Work {
        let mut state = self.lock();
        loop {
            if state.shutdown {
                return Work::Shutdown;
            }
            if let Some(settings) = state.pending_settings.take() {
                return Work::Settings(Box::new(settings));
            }
            if let Some((id, job)) = state.queue.pop_front() {
                // Reset under the lock so a late `cancel_current` cannot hit the next job.
                self.cancel.store(false, Ordering::Relaxed);
                state.running = Some((id, job.clone()));
                return Work::Job(id, job);
            }
            match deadline {
                None => state = self.wake.wait(state).unwrap_or_else(PoisonError::into_inner),
                Some(d) => {
                    let now = Instant::now();
                    if d <= now {
                        return Work::Timeout;
                    }
                    let (guard, _) = self
                        .wake
                        .wait_timeout(state, d - now)
                        .unwrap_or_else(PoisonError::into_inner);
                    state = guard;
                    if Instant::now() >= d && state.queue.is_empty() && state.pending_settings.is_none() {
                        return if state.shutdown {
                            Work::Shutdown
                        } else {
                            Work::Timeout
                        };
                    }
                }
            }
        }
    }

    pub(super) fn snapshot(&self) -> Arc<IndexSnapshot> {
        Arc::clone(&self.snapshot.read().unwrap_or_else(PoisonError::into_inner))
    }

    pub(super) fn set_snapshot(&self, snapshot: Arc<IndexSnapshot>) {
        *self.snapshot.write().unwrap_or_else(PoisonError::into_inner) = snapshot;
    }
}

/// Convenience: how long a duration is in whole milliseconds.
pub(super) fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}
