//! The background worker behind [`PreviewService`].

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::cache::{BYTE_BUDGET, CAPACITY, CacheKey, PreviewCache, is_cacheable};
use crate::cancel::Cancel;
use crate::env::Env;
use crate::error::PreviewError;
use crate::{PreviewContent, PreviewLimits, PreviewResult, render};

struct Job {
    id: u64,
    path: PathBuf,
}

/// Background preview worker.
///
/// Requests run one at a time on a dedicated thread. A new [`request`](Self::request) cancels the
/// running one (decoders stop reading, helper programs are killed) and replaces everything still
/// queued, so only the newest request ever produces a [`PreviewResult`]. Finished previews are kept
/// in an LRU cache keyed by path, mtime, size and limits.
///
/// Dropping the service cancels the running request, stops the thread and waits for it.
#[derive(Debug)]
pub struct PreviewService {
    /// Id of the newest request. Each worker step compares its own id against this.
    latest: Arc<AtomicU64>,
    limits: Arc<Mutex<PreviewLimits>>,
    jobs: Option<Sender<Job>>,
    worker: Option<JoinHandle<()>>,
}

impl PreviewService {
    /// Starts the worker.
    ///
    /// Results arrive on the returned receiver, which is unbounded: one result per finished
    /// request, and superseded requests produce none. If a request is superseded in the instant
    /// between its last check and the send, one stale result can slip through, so callers should
    /// still compare [`PreviewResult::request`] with the id of their latest request.
    pub fn start(limits: PreviewLimits) -> (Self, Receiver<PreviewResult>) {
        let env = Env::system();
        Self::start_with(limits, move |path, limits, cancel| {
            render::render(path, limits, &env, cancel)
        })
    }

    /// Starts the worker with a custom renderer (tests substitute a controllable one).
    pub(crate) fn start_with<F>(limits: PreviewLimits, render: F) -> (Self, Receiver<PreviewResult>)
    where
        F: FnMut(&Path, PreviewLimits, &Cancel) -> Result<PreviewContent, PreviewError> + Send + 'static,
    {
        let latest = Arc::new(AtomicU64::new(0));
        let limits = Arc::new(Mutex::new(limits));
        let (job_tx, job_rx) = unbounded();
        let (result_tx, result_rx) = unbounded();
        let worker = thread::Builder::new()
            .name("everythux-preview".to_owned())
            .spawn({
                let latest = Arc::clone(&latest);
                let limits = Arc::clone(&limits);
                move || worker_loop(&job_rx, &result_tx, &latest, &limits, render)
            })
            .inspect_err(|e| tracing::error!(error = %e, "cannot start preview worker"))
            .ok();
        let service = Self {
            latest,
            limits,
            jobs: Some(job_tx),
            worker,
        };
        (service, result_rx)
    }

    /// Requests a preview of `path` (cancels the previous request). Returns the request id.
    ///
    /// Ids increase with every call, starting at 1.
    pub fn request(&self, path: PathBuf) -> u64 {
        // Bumping `latest` is what cancels the running request, before the new job is even queued.
        let id = self.latest.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(jobs) = &self.jobs
            && jobs.send(Job { id, path }).is_err()
        {
            tracing::warn!("preview worker is gone; request {id} dropped");
        }
        id
    }

    /// Updates limits for later requests.
    pub fn set_limits(&self, limits: PreviewLimits) {
        *self.limits.lock().unwrap_or_else(PoisonError::into_inner) = limits;
    }
}

impl Drop for PreviewService {
    fn drop(&mut self) {
        // No request id can equal this, so whatever is running sees itself superseded and stops
        // (killing its helper process); closing the queue then ends the loop.
        self.latest.store(u64::MAX, Ordering::Relaxed);
        drop(self.jobs.take());
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::error!("preview worker panicked");
        }
    }
}

fn worker_loop<F>(
    jobs: &Receiver<Job>,
    results: &Sender<PreviewResult>,
    latest: &Arc<AtomicU64>,
    limits: &Mutex<PreviewLimits>,
    mut render: F,
) where
    F: FnMut(&Path, PreviewLimits, &Cancel) -> Result<PreviewContent, PreviewError>,
{
    let mut cache = PreviewCache::new(CAPACITY, BYTE_BUDGET);
    while let Ok(mut job) = jobs.recv() {
        // Coalesce: only the newest queued request is worth any work.
        while let Ok(newer) = jobs.try_recv() {
            job = newer;
        }
        let cancel = Cancel::new(Arc::clone(latest), job.id);
        if cancel.is_cancelled() {
            continue;
        }
        let limits = *limits.lock().unwrap_or_else(PoisonError::into_inner);

        let key = CacheKey::of(&job.path, limits);
        let cached = key.as_ref().and_then(|k| cache.get(k)).cloned();
        let Some(content) =
            cached.or_else(|| compute(&mut render, &job.path, limits, &cancel, key, &mut cache))
        else {
            continue; // superseded while rendering
        };

        if cancel.is_cancelled() {
            continue;
        }
        let result = PreviewResult {
            request: job.id,
            path: job.path,
            content,
        };
        if results.send(result).is_err() {
            break; // nobody listens any more
        }
    }
}

/// Renders `path`, caching good results. `None` means the request was superseded meanwhile.
fn compute<F>(
    render: &mut F,
    path: &Path,
    limits: PreviewLimits,
    cancel: &Cancel,
    key: Option<CacheKey>,
    cache: &mut PreviewCache,
) -> Option<PreviewContent>
where
    F: FnMut(&Path, PreviewLimits, &Cancel) -> Result<PreviewContent, PreviewError>,
{
    // Providers parse untrusted files; a bug in a decoder must cost one preview, not the worker.
    match catch_unwind(AssertUnwindSafe(|| render(path, limits, cancel))) {
        Ok(Ok(content)) => {
            if let Some(key) = key
                && is_cacheable(&content)
                && !cancel.is_cancelled()
            {
                cache.insert(key, content.clone());
            }
            Some(content)
        }
        Ok(Err(e)) if e.is_cancelled() => None,
        Ok(Err(e)) => Some(PreviewContent::Unavailable { hint: e.hint() }),
        Err(_) => {
            tracing::error!(path = %path.display(), "preview provider panicked");
            Some(PreviewContent::Unavailable {
                hint: "Preview failed unexpectedly".to_owned(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use crossbeam_channel::{RecvTimeoutError, bounded};

    use super::*;

    /// Generous: only bounds how long a *failing* test waits, never gates correctness.
    const WAIT: Duration = Duration::from_secs(20);

    fn text(s: &str) -> PreviewContent {
        PreviewContent::Text {
            text: s.to_owned(),
            truncated: false,
        }
    }

    /// The worker has exited and nothing else is queued: the channel is empty *and* disconnected.
    fn assert_closed(results: &Receiver<PreviewResult>) {
        assert!(
            matches!(results.recv_timeout(WAIT), Err(RecvTimeoutError::Disconnected)),
            "expected a closed, empty result channel"
        );
    }

    fn file(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).expect("write");
        path
    }

    #[test]
    fn ids_increase_and_results_carry_them() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = file(tmp.path(), "a.txt", "alpha");
        let (service, results) = PreviewService::start(PreviewLimits::default());
        let id1 = service.request(a.clone());
        let r1 = results.recv_timeout(WAIT).expect("first result");
        let id2 = service.request(a.clone());
        let r2 = results.recv_timeout(WAIT).expect("second result");
        assert!(id2 > id1);
        assert_eq!((r1.request, r2.request), (id1, id2));
        assert_eq!(r1.path, a);
        assert_eq!(r1.content, text("alpha"));
    }

    #[test]
    fn newer_request_cancels_running_one_and_produces_no_result() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let slow = file(tmp.path(), "slow.txt", "x");
        let fast = file(tmp.path(), "fast.txt", "y");
        let (started_tx, started_rx) = bounded::<()>(1);
        let slow_path = slow.clone();
        let (service, results) =
            PreviewService::start_with(PreviewLimits::default(), move |path, _, cancel| {
                if path == slow_path {
                    started_tx.send(()).expect("signal start");
                    // A long-running provider: it only ends because it notices the cancellation.
                    while !cancel.is_cancelled() {
                        thread::yield_now();
                    }
                    return Err(PreviewError::Cancelled);
                }
                Ok(text("fast"))
            });

        let slow_id = service.request(slow);
        started_rx.recv_timeout(WAIT).expect("slow request started");
        let fast_id = service.request(fast);
        assert!(fast_id > slow_id);

        let result = results.recv_timeout(WAIT).expect("result of the newer request");
        assert_eq!(result.request, fast_id);
        assert_eq!(result.content, text("fast"));
        drop(service); // joins the worker; afterwards the channel must be empty and closed
        assert_closed(&results);
    }

    #[test]
    fn finished_but_superseded_work_is_discarded() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = file(tmp.path(), "1.txt", "1");
        let second = file(tmp.path(), "2.txt", "2");
        let (started_tx, started_rx) = bounded::<()>(1);
        let (release_tx, release_rx) = bounded::<()>(1);
        let first_path = first.clone();
        let (service, results) = PreviewService::start_with(PreviewLimits::default(), move |path, _, _| {
            if path == first_path {
                // Ignores cancellation on purpose and finishes anyway.
                started_tx.send(()).expect("signal start");
                release_rx.recv().expect("release");
            }
            Ok(text(&path.display().to_string()))
        });
        service.request(first);
        started_rx.recv_timeout(WAIT).expect("started");
        let second_id = service.request(second.clone());
        release_tx.send(()).expect("release first");

        let result = results.recv_timeout(WAIT).expect("result");
        assert_eq!(result.request, second_id, "the stale result must not be sent");
        assert_eq!(result.path, second);
        drop(service);
        assert_closed(&results);
    }

    #[test]
    fn queued_requests_are_coalesced_to_the_latest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let blocker = file(tmp.path(), "blocker.txt", "b");
        let (started_tx, started_rx) = bounded::<()>(1);
        let (release_tx, release_rx) = bounded::<()>(1);
        let rendered = Arc::new(Mutex::new(Vec::<PathBuf>::new()));
        let seen = Arc::clone(&rendered);
        let blocker_path = blocker.clone();
        let (service, results) = PreviewService::start_with(PreviewLimits::default(), move |path, _, _| {
            seen.lock().expect("lock").push(path.to_path_buf());
            if path == blocker_path {
                started_tx.send(()).expect("signal start");
                release_rx.recv().expect("release");
            }
            Ok(text("done"))
        });

        service.request(blocker.clone());
        started_rx
            .recv_timeout(WAIT)
            .expect("worker is busy with the blocker");
        // The worker is blocked, so these three pile up in the queue.
        service.request(file(tmp.path(), "q1.txt", "1"));
        service.request(file(tmp.path(), "q2.txt", "2"));
        let last = file(tmp.path(), "q3.txt", "3");
        let last_id = service.request(last.clone());
        release_tx.send(()).expect("release blocker");

        let result = results.recv_timeout(WAIT).expect("result");
        assert_eq!((result.request, &result.path), (last_id, &last));
        drop(service);
        assert_closed(&results);
        assert_eq!(
            *rendered.lock().expect("lock"),
            [blocker, last],
            "q1 and q2 must never be rendered"
        );
    }

    #[test]
    fn repeated_request_is_served_from_the_cache() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = file(tmp.path(), "a.txt", "aaa");
        let b = file(tmp.path(), "b.txt", "bbb");
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let (service, results) = PreviewService::start_with(PreviewLimits::default(), move |path, _, _| {
            counter.fetch_add(1, Ordering::Relaxed);
            Ok(text(&path.display().to_string()))
        });
        for path in [&a, &b, &a, &b, &a] {
            service.request(path.clone());
            let result = results.recv_timeout(WAIT).expect("result");
            assert_eq!(&result.path, path);
            assert_eq!(result.content, text(&path.display().to_string()));
        }
        assert_eq!(
            calls.load(Ordering::Relaxed),
            2,
            "only the first visit of each file renders"
        );
    }

    #[test]
    fn modified_file_or_new_limits_bypass_the_cache() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = file(tmp.path(), "a.txt", "one");
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let (service, results) =
            PreviewService::start_with(PreviewLimits::default(), move |path, limits, _| {
                counter.fetch_add(1, Ordering::Relaxed);
                Ok(text(&format!(
                    "{} {}",
                    fs::read_to_string(path).expect("read"),
                    limits.max_dim
                )))
            });
        let ask = |expect: &str| {
            service.request(a.clone());
            assert_eq!(results.recv_timeout(WAIT).expect("result").content, text(expect));
        };
        ask("one 768");
        ask("one 768");
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        service.set_limits(PreviewLimits {
            max_dim: 64,
            ..PreviewLimits::default()
        });
        ask("one 64");
        assert_eq!(
            calls.load(Ordering::Relaxed),
            2,
            "different limits are a different key"
        );

        fs::write(&a, "three").expect("rewrite");
        ask("three 64");
        assert_eq!(
            calls.load(Ordering::Relaxed),
            3,
            "different size/mtime is a different key"
        );
    }

    #[test]
    fn failures_become_unavailable_and_are_not_cached() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let a = file(tmp.path(), "a.txt", "x");
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let (service, results) = PreviewService::start_with(PreviewLimits::default(), move |_, _, _| {
            counter.fetch_add(1, Ordering::Relaxed);
            Err(PreviewError::NotText)
        });
        for _ in 0..2 {
            service.request(a.clone());
            let result = results.recv_timeout(WAIT).expect("result");
            assert!(matches!(result.content, PreviewContent::Unavailable { .. }));
        }
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn a_panicking_provider_does_not_kill_the_worker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let boom = file(tmp.path(), "boom.txt", "x");
        let ok = file(tmp.path(), "ok.txt", "y");
        let boom_path = boom.clone();
        let (service, results) = PreviewService::start_with(PreviewLimits::default(), move |path, _, _| {
            assert_ne!(path, boom_path, "provider bug");
            Ok(text("fine"))
        });
        service.request(boom);
        let first = results.recv_timeout(WAIT).expect("result");
        assert!(matches!(first.content, PreviewContent::Unavailable { .. }));
        service.request(ok);
        assert_eq!(results.recv_timeout(WAIT).expect("result").content, text("fine"));
    }

    #[test]
    fn dropping_the_service_stops_the_worker() {
        let (service, results) = PreviewService::start(PreviewLimits::default());
        drop(service);
        assert_closed(&results);
    }
}
