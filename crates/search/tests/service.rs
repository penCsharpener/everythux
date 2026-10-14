//! `SearchService` behaviour. Everything waits on channels with a generous timeout, never on sleeps.

use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Receiver;
use everythux_core::snapshot::IndexSnapshot;
use everythux_search::synthetic::{SnapshotBuilder, generate};
use everythux_search::{QueryOptions, SearchRequest, SearchResults, SearchService, Sort};

const TIMEOUT: Duration = Duration::from_secs(120);

fn snapshot(extra: &[&str]) -> Arc<IndexSnapshot> {
    let mut b = SnapshotBuilder::new();
    let v = b.volume("/", None);
    let root = b.dir("/", v);
    for name in ["alpha.txt", "beta.txt", "gamma.rs", "alphabet.md"]
        .iter()
        .chain(extra)
    {
        b.file(root, name.as_bytes(), 1, 1);
    }
    Arc::new(b.finish())
}

fn request(text: &str) -> SearchRequest {
    SearchRequest {
        texts: vec![text.to_owned()],
        options: QueryOptions::default(),
        sort: Sort::default(),
        limit: 0,
    }
}

fn recv(rx: &Receiver<SearchResults>) -> SearchResults {
    rx.recv_timeout(TIMEOUT).expect("results arrive")
}

fn names(r: &SearchResults) -> Vec<String> {
    r.ids
        .iter()
        .map(|&id| String::from_utf8_lossy(r.snapshot.name(id)).into_owned())
        .collect()
}

#[test]
fn answers_a_request() {
    let snap = snapshot(&[]);
    let (service, rx) = SearchService::start(Arc::clone(&snap));
    let generation = service.submit(request("alpha"));
    let r = recv(&rx);
    assert_eq!(r.generation, generation);
    assert_eq!(names(&r), ["alpha.txt", "alphabet.md"]);
    assert_eq!(r.total_entries, 5 - 1, "four files; the root folder has no entry");
    assert!(Arc::ptr_eq(&r.snapshot, &snap));
    assert!(r.error.is_none());
}

#[test]
fn rapid_submissions_end_with_the_latest_result() {
    let (service, rx) = SearchService::start(snapshot(&[]));
    let last = (0..50)
        .map(|i| service.submit(request(&format!("a{}", "x".repeat(i % 3)))))
        .last()
        .unwrap();
    let final_request = service.submit(request("beta"));
    assert!(final_request > last);
    let mut seen = Vec::new();
    loop {
        let r = recv(&rx);
        seen.push(r.generation);
        if r.generation == final_request {
            assert_eq!(names(&r), ["beta.txt"]);
            break;
        }
    }
    assert!(
        seen.windows(2).all(|w| w[0] < w[1]),
        "generations only move forward: {seen:?}"
    );
    assert!(seen.len() <= 51);
}

#[test]
fn parse_errors_keep_the_previous_ids() {
    let snap = snapshot(&[]);
    let (service, rx) = SearchService::start(Arc::clone(&snap));
    let first_error = service.submit(request("\"unterminated"));
    let r = recv(&rx);
    assert_eq!(r.generation, first_error);
    assert!(
        r.error
            .as_ref()
            .is_some_and(|e| e.bar == 0 && e.message.contains("quote")),
        "{:?}",
        r.error
    );
    assert!(r.ids.is_empty(), "nothing to keep yet");

    service.submit(request("txt"));
    let good = recv(&rx);
    assert_eq!(names(&good), ["alpha.txt", "beta.txt"]);

    let bad = service.submit(request("regex:("));
    let r = recv(&rx);
    assert_eq!(r.generation, bad);
    assert!(r.error.is_some());
    assert_eq!(r.ids, good.ids, "previous results stay");
    assert!(Arc::ptr_eq(&r.snapshot, &good.snapshot));
}

#[test]
fn set_snapshot_swaps_and_reruns_the_last_request() {
    let (service, rx) = SearchService::start(snapshot(&[]));
    let generation = service.submit(request("alpha"));
    let before = recv(&rx);
    assert_eq!(names(&before), ["alpha.txt", "alphabet.md"]);

    let newer = snapshot(&["alpha2.txt"]);
    service.set_snapshot(Arc::clone(&newer));
    let after = recv(&rx);
    assert_eq!(after.generation, generation, "same request, same generation");
    assert!(Arc::ptr_eq(&after.snapshot, &newer));
    assert_eq!(names(&after), ["alpha.txt", "alpha2.txt", "alphabet.md"]);
    assert_eq!(after.total_entries, newer.len());
}

#[test]
fn the_newest_snapshot_wins() {
    let (service, rx) = SearchService::start(snapshot(&[]));
    let generation = service.submit(request("alpha"));
    recv(&rx);
    let second = snapshot(&["alpha2.txt", "alpha3.txt"]);
    service.set_snapshot(snapshot(&["alpha1.txt"]));
    service.set_snapshot(Arc::clone(&second));
    loop {
        let r = recv(&rx);
        assert_eq!(r.generation, generation);
        if Arc::ptr_eq(&r.snapshot, &second) {
            assert_eq!(r.ids.len(), 4);
            break;
        }
    }
}

#[test]
fn requests_submitted_while_the_first_index_builds_are_answered() {
    let (service, rx) = SearchService::start(Arc::new(generate(200_000, 5)));
    let generation = service.submit(request("report"));
    let r = recv(&rx);
    assert_eq!(r.generation, generation);
    assert!(!r.ids.is_empty());
    assert_eq!(r.total_entries, 200_000);
}

#[test]
fn results_honour_sort_and_limit() {
    let (service, rx) = SearchService::start(snapshot(&[]));
    let mut req = request("");
    req.sort = Sort {
        column: everythux_core::config::ColumnId::Name,
        ascending: false,
    };
    req.limit = 2;
    service.submit(req);
    assert_eq!(names(&recv(&rx)), ["gamma.rs", "beta.txt"]);
}

#[test]
fn dropping_the_service_closes_the_channel() {
    let (service, rx) = SearchService::start(snapshot(&[]));
    service.submit(request("alpha"));
    service.set_snapshot(snapshot(&["x.txt"]));
    drop(service);
    // The worker has been joined, so its sender is gone and the iterator terminates.
    assert!(rx.iter().count() <= 2);
    assert!(rx.recv().is_err());
}

#[test]
fn several_bars_combine_with_and_and_report_the_failing_bar() {
    let (service, rx) = SearchService::start(snapshot(&[]));
    let mut req = request("alpha");
    req.texts.push(String::new());
    req.texts.push("txt".to_owned());
    service.submit(req.clone());
    let good = recv(&rx);
    assert_eq!(names(&good), ["alpha.txt"]);

    req.texts[1] = "\"open".to_owned();
    service.submit(req);
    let r = recv(&rx);
    let error = r.error.expect("a parse error");
    assert_eq!(error.bar, 1, "the broken middle bar is named");
    assert_eq!(r.ids, good.ids, "previous results stay");

    let mut none = request("");
    none.texts.clear();
    service.submit(none);
    assert_eq!(recv(&rx).ids.len(), 4, "no bars match everything");
}
