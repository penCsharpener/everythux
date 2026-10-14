//! Background search thread.
//!
//! One worker thread owns the current [`SearchIndex`]. [`SearchService::submit`] bumps a shared
//! generation counter, which the running search polls to cancel itself, then queues the request.
//! Before each run the worker drains its queue and keeps only the newest request, so typing faster
//! than the engine answers costs nothing. Snapshots are indexed on a separate thread and swapped in
//! when ready, so searches never wait for a rebuild.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use everythux_core::ids::EntryId;
use everythux_core::snapshot::IndexSnapshot;

use crate::engine::{SearchIndex, Sort};
use crate::query::{QueryOptions, parse_many};

/// What to search for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    /// The text of each search bar. Each is parsed on its own and the results are combined with
    /// AND ([`crate::parse_many`]); empty texts change nothing, and no texts match everything.
    pub texts: Vec<String>,
    /// Switches.
    pub options: QueryOptions,
    /// Ordering.
    pub sort: Sort,
    /// Result cap (0 = unlimited).
    pub limit: usize,
}

/// Results of one request.
#[derive(Debug, Clone)]
pub struct SearchResults {
    /// Generation returned by [`SearchService::submit`]; the UI ignores results older than the
    /// latest generation it submitted.
    pub generation: u64,
    /// Matching entries in display order. Indexes into `snapshot`.
    pub ids: Arc<[EntryId]>,
    /// The snapshot `ids` refer to.
    pub snapshot: Arc<IndexSnapshot>,
    /// Entries in the snapshot (for "N of M objects").
    pub total_entries: usize,
    /// Time spent searching.
    pub elapsed: Duration,
    /// Parse error; when set, `ids` holds the previous results unchanged.
    pub error: Option<SearchError>,
}

/// Why a request could not run: a search bar whose text doesn't parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchError {
    /// Index of the failing bar in [`SearchRequest::texts`].
    pub bar: usize,
    /// The message to show under that bar.
    pub message: String,
}

enum Msg {
    Search { generation: u64, request: SearchRequest },
    Index { epoch: u64, index: Arc<SearchIndex> },
    Stop,
}

/// State shared between the handle, the worker and the index builders.
#[derive(Debug, Default)]
struct Shared {
    /// Bumped by every `submit`; a search is stale once this differs from its own generation.
    generation: AtomicU64,
    /// Bumped by every `set_snapshot`, so a slow older build can't replace a newer index.
    epoch: AtomicU64,
    closed: AtomicBool,
}

/// What one pass of the worker has to do after draining its queue.
#[derive(Default)]
struct Batch {
    search: Option<(u64, SearchRequest)>,
    index: Option<(u64, Arc<SearchIndex>)>,
    stop: bool,
}

impl Batch {
    fn push(&mut self, msg: Msg) {
        match msg {
            Msg::Search { generation, request } => {
                if self.search.as_ref().is_none_or(|(g, _)| generation > *g) {
                    self.search = Some((generation, request));
                }
            }
            Msg::Index { epoch, index } => {
                if self.index.as_ref().is_none_or(|(e, _)| epoch > *e) {
                    self.index = Some((epoch, index));
                }
            }
            Msg::Stop => self.stop = true,
        }
    }
}

/// Handle to the search thread. Dropping it stops the thread.
#[derive(Debug)]
pub struct SearchService {
    shared: Arc<Shared>,
    tx: Sender<Msg>,
    worker: Option<JoinHandle<()>>,
    builders: Mutex<Vec<JoinHandle<()>>>,
}

impl SearchService {
    /// Starts the thread with an initial snapshot. Results arrive on the returned channel.
    ///
    /// The initial [`SearchIndex`] is built on the worker thread, so this returns immediately;
    /// requests submitted meanwhile are answered once it is ready.
    pub fn start(snapshot: Arc<IndexSnapshot>) -> (Self, Receiver<SearchResults>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (results_tx, results_rx) = crossbeam_channel::unbounded();
        let shared = Arc::new(Shared::default());
        let worker = {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("everythux-search".to_owned())
                .spawn(move || Worker::new(snapshot, results_tx, shared).run(&rx))
                .inspect_err(|error| tracing::error!(%error, "cannot spawn the search thread"))
                .ok()
        };
        let service = Self {
            shared,
            tx,
            worker,
            builders: Mutex::new(Vec::new()),
        };
        (service, results_rx)
    }

    /// Queues a request, cancelling any running one; returns its generation. Rapid submissions
    /// are coalesced (only the latest runs).
    pub fn submit(&self, request: SearchRequest) -> u64 {
        let generation = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        // A send error means the worker is gone (it failed to start); nothing to do.
        let _ = self.tx.send(Msg::Search { generation, request });
        generation
    }

    /// Swaps in a new snapshot (sort permutations are rebuilt in the background; the last
    /// request is re-run against the new snapshot when ready).
    pub fn set_snapshot(&self, snapshot: Arc<IndexSnapshot>) {
        let epoch = self.shared.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let tx = self.tx.clone();
        let shared = Arc::clone(&self.shared);
        let spawned = thread::Builder::new()
            .name("everythux-index-build".to_owned())
            .spawn(move || {
                if shared.closed.load(Ordering::Acquire) {
                    return;
                }
                let started = Instant::now();
                let index = Arc::new(SearchIndex::build(snapshot));
                tracing::debug!(
                    epoch,
                    elapsed_ms = started.elapsed().as_millis(),
                    "search index built"
                );
                // The worker may be gone already; then the index is simply dropped.
                let _ = tx.send(Msg::Index { epoch, index });
            });
        match spawned {
            Ok(handle) => {
                let mut builders = self.builders.lock().unwrap_or_else(PoisonError::into_inner);
                builders.retain(|h| !h.is_finished());
                builders.push(handle);
            }
            Err(error) => tracing::error!(%error, "cannot spawn the index build thread"),
        }
    }
}

impl Drop for SearchService {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
        let _ = self.tx.send(Msg::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let builders = std::mem::take(&mut *self.builders.lock().unwrap_or_else(PoisonError::into_inner));
        for builder in builders {
            let _ = builder.join();
        }
    }
}

struct Worker {
    index: Arc<SearchIndex>,
    applied_epoch: u64,
    /// The newest request, re-run when a new index arrives.
    last_request: Option<(u64, SearchRequest)>,
    /// The last successful results, repeated alongside parse errors.
    last_ids: Arc<[EntryId]>,
    last_snapshot: Arc<IndexSnapshot>,
    results: Sender<SearchResults>,
    shared: Arc<Shared>,
}

impl Worker {
    fn new(snapshot: Arc<IndexSnapshot>, results: Sender<SearchResults>, shared: Arc<Shared>) -> Self {
        Self {
            index: Arc::new(SearchIndex::build(Arc::clone(&snapshot))),
            applied_epoch: 0,
            last_request: None,
            last_ids: Arc::from([]),
            last_snapshot: snapshot,
            results,
            shared,
        }
    }

    fn run(mut self, rx: &Receiver<Msg>) {
        while let Ok(first) = rx.recv() {
            let mut batch = Batch::default();
            batch.push(first);
            for msg in rx.try_iter() {
                batch.push(msg);
            }
            if batch.stop || self.shared.closed.load(Ordering::Acquire) {
                return;
            }
            let swapped = match batch.index {
                Some((epoch, index)) if epoch > self.applied_epoch => {
                    self.applied_epoch = epoch;
                    self.index = index;
                    true
                }
                _ => false,
            };
            if let Some(search) = batch.search {
                self.last_request = Some(search);
            } else if !swapped {
                continue;
            }
            self.execute();
        }
    }

    /// Runs `last_request` and sends the results (unless it was cancelled meanwhile).
    fn execute(&mut self) {
        let Some((generation, request)) = self.last_request.clone() else {
            return;
        };
        let started = Instant::now();
        let outcome = parse_many(&request.texts, request.options).map(|query| {
            let cancelled = || {
                self.shared.generation.load(Ordering::Relaxed) != generation
                    || self.shared.closed.load(Ordering::Relaxed)
            };
            self.index.search(&query, request.sort, request.limit, &cancelled)
        });
        let results = match outcome {
            // Superseded: the newer request is already queued.
            Ok(None) => return,
            Ok(Some(ids)) => {
                let ids: Arc<[EntryId]> = ids.into();
                self.last_ids = Arc::clone(&ids);
                self.last_snapshot = Arc::clone(self.index.snapshot());
                SearchResults {
                    generation,
                    ids,
                    total_entries: self.last_snapshot.len(),
                    snapshot: Arc::clone(&self.last_snapshot),
                    elapsed: started.elapsed(),
                    error: None,
                }
            }
            Err(error) => SearchResults {
                generation,
                ids: Arc::clone(&self.last_ids),
                total_entries: self.last_snapshot.len(),
                snapshot: Arc::clone(&self.last_snapshot),
                elapsed: started.elapsed(),
                error: Some(SearchError {
                    bar: error.bar,
                    message: error.error.to_string(),
                }),
            },
        };
        // The receiver may be gone (UI shutting down); the next recv will notice the stop message.
        let _ = self.results.send(results);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(text: &str) -> SearchRequest {
        SearchRequest {
            texts: vec![text.to_owned()],
            options: QueryOptions::default(),
            sort: Sort::default(),
            limit: 0,
        }
    }

    fn search_msg(generation: u64, text: &str) -> Msg {
        Msg::Search {
            generation,
            request: request(text),
        }
    }

    #[test]
    fn batch_keeps_only_the_newest_search() {
        let mut batch = Batch::default();
        for (g, text) in [(1, "a"), (2, "b"), (3, "c")] {
            batch.push(search_msg(g, text));
        }
        let (generation, req) = batch.search.expect("a search is queued");
        assert_eq!((generation, req.texts), (3, vec!["c".to_owned()]));
        assert!(!batch.stop);
    }

    #[test]
    fn batch_keeps_the_newest_index_and_remembers_stop() {
        let snapshot = Arc::new(IndexSnapshot::empty());
        let mut batch = Batch::default();
        let index = |epoch| Msg::Index {
            epoch,
            index: Arc::new(SearchIndex::build(Arc::clone(&snapshot))),
        };
        batch.push(index(2));
        batch.push(index(1));
        batch.push(Msg::Stop);
        assert_eq!(batch.index.map(|(e, _)| e), Some(2));
        assert!(batch.stop);
    }

    /// Runs a worker to completion over pre-queued messages (the sender is dropped, so it exits
    /// when the queue is empty). Returns what it sent.
    fn run_queued(msgs: Vec<Msg>, current_generation: u64) -> Vec<SearchResults> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (results_tx, results_rx) = crossbeam_channel::unbounded();
        for msg in msgs {
            tx.send(msg).expect("queue is open");
        }
        drop(tx);
        let shared = Arc::new(Shared::default());
        shared.generation.store(current_generation, Ordering::SeqCst);
        Worker::new(Arc::new(IndexSnapshot::empty()), results_tx, shared).run(&rx);
        results_rx.try_iter().collect()
    }

    #[test]
    fn worker_runs_only_the_newest_queued_request() {
        let results = run_queued(
            vec![search_msg(1, "a"), search_msg(2, "b"), search_msg(3, "c")],
            3,
        );
        assert_eq!(results.iter().map(|r| r.generation).collect::<Vec<_>>(), [3]);
    }

    #[test]
    fn worker_drops_a_request_that_was_superseded_while_running() {
        // The shared generation is already 4, so request 3 is cancelled at its first poll.
        assert!(run_queued(vec![search_msg(3, "c")], 4).is_empty());
    }

    #[test]
    fn worker_reports_parse_errors_with_previous_ids() {
        let results = run_queued(vec![search_msg(1, "\"open")], 1);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.is_some());
        assert!(results[0].ids.is_empty());
    }

    #[test]
    fn worker_reruns_the_last_request_on_a_new_index_only() {
        let snapshot = Arc::new(IndexSnapshot::empty());
        let index = |epoch| Msg::Index {
            epoch,
            index: Arc::new(SearchIndex::build(Arc::clone(&snapshot))),
        };
        // A stale index (epoch 0 is already applied) and no request: nothing to do.
        assert!(run_queued(vec![index(0)], 0).is_empty());
        // No request has been seen yet, so a fresh index has nothing to re-run either.
        assert!(run_queued(vec![index(1)], 0).is_empty());
        // Request and index in the same batch produce a single result.
        let results = run_queued(vec![search_msg(1, "a"), index(1)], 1);
        assert_eq!(results.len(), 1);
    }
}
