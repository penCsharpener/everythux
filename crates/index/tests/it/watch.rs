//! Live updates through inotify.

use std::path::Path;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use everythux_core::exclusion::{ExclusionRule, RuleKind};
use everythux_core::snapshot::IndexSnapshot;
use everythux_index::test_utils::TreeBuilder;
use everythux_index::{IndexEvent, Indexer};

use crate::common::{TIMEOUT, check_invariants, dir_total, listing, settings, start_scanned};

/// Waits for a published snapshot satisfying `ok` (live updates arrive asynchronously).
fn wait_snapshot(rx: &Receiver<IndexEvent>, what: &str, ok: impl Fn(&IndexSnapshot) -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(IndexEvent::SnapshotReady(snap)) if ok(&snap) => return,
            Ok(IndexEvent::Failed { error, .. }) => panic!("job failed while waiting for {what}: {error}"),
            Ok(_) => {}
            Err(e) => panic!("timed out waiting for {what}: {e}"),
        }
    }
}

fn watching_settings(root: &Path) -> everythux_index::IndexerSettings {
    let mut s = settings(&[root]);
    s.indexer.live_updates = true;
    s.indexer.watch_roots = vec![root.to_path_buf()];
    s
}

#[test]
fn created_changed_and_deleted_files_show_up_without_a_rescan() {
    let (db, tree) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let tb = TreeBuilder::new(tree.path());
    tb.file("existing/one.txt", 1).unwrap();
    let (indexer, rx) = start_scanned(db.path(), watching_settings(tree.path()));
    let root = tree.path().to_path_buf();

    tb.file("existing/created.txt", 10).unwrap();
    wait_snapshot(&rx, "created file", |s| {
        listing(s, &root).contains_key("existing/created.txt")
    });

    tb.file("brand_new/inner/deep.md", 20).unwrap();
    wait_snapshot(&rx, "new folder with content", |s| {
        listing(s, &root).contains_key("brand_new/inner/deep.md")
    });

    // The new folder is watched too.
    tb.file("brand_new/inner/later.txt", 5).unwrap();
    wait_snapshot(&rx, "file in a folder created while running", |s| {
        listing(s, &root).contains_key("brand_new/inner/later.txt")
    });

    tb.remove("existing/one.txt").unwrap();
    wait_snapshot(&rx, "deleted file", |s| {
        !listing(s, &root).contains_key("existing/one.txt")
    });

    tb.remove("brand_new").unwrap();
    wait_snapshot(&rx, "deleted folder", |s| {
        listing(s, &root).keys().all(|k| !k.starts_with("brand_new"))
    });

    let snap = indexer.snapshot();
    check_invariants(&snap);
    assert_eq!(dir_total(&snap, &root), Some(10));
}

#[test]
fn excluded_folders_are_not_watched() {
    let (db, tree) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let tb = TreeBuilder::new(tree.path());
    tb.dir("node_modules").unwrap().dir("src").unwrap();
    let mut s = watching_settings(tree.path());
    s.exclusions = vec![ExclusionRule::new("node_modules", RuleKind::Dir)];
    let (indexer, rx) = start_scanned(db.path(), s);
    let root = tree.path().to_path_buf();

    tb.file("node_modules/ignored.js", 1).unwrap();
    tb.file("src/seen.rs", 1).unwrap();
    wait_snapshot(&rx, "src file", |s| listing(s, &root).contains_key("src/seen.rs"));
    // Give a (wrongly) watched folder time to be refreshed, then check it was not.
    std::thread::sleep(Duration::from_millis(600));
    let snap = indexer.snapshot();
    assert!(!listing(&snap, &root).contains_key("node_modules/ignored.js"));
}

#[test]
fn dropping_the_indexer_does_not_wait_for_filesystem_events() {
    let (db, tree) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    TreeBuilder::new(tree.path()).file("a/b/c", 1).unwrap();
    let (indexer, _rx) = Indexer::start(&db.path().join("index.db"), watching_settings(tree.path())).unwrap();
    let started = Instant::now();
    drop(indexer);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "shutdown took {:?}",
        started.elapsed()
    );
}

#[test]
fn the_apps_own_config_and_log_folders_are_not_watched() {
    // Saving a setting rewrites config.json (temp file + rename). If that folder were watched,
    // every save would trigger a rescan and a fresh snapshot, which used to drop the UI selection.
    let (db, tree) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let tb = TreeBuilder::new(tree.path());
    tb.dir(".config/everythux")
        .unwrap()
        .dir(".local/state/everythux")
        .unwrap()
        .dir("src")
        .unwrap();
    let mut s = watching_settings(tree.path());
    s.watch_ignore = vec![
        tree.path().join(".config/everythux"),
        tree.path().join(".local/state/everythux"),
    ];
    let (indexer, rx) = start_scanned(db.path(), s);
    let root = tree.path().to_path_buf();

    tb.file(".config/everythux/.config.json.tmp", 2).unwrap();
    std::fs::rename(
        tree.path().join(".config/everythux/.config.json.tmp"),
        tree.path().join(".config/everythux/config.json"),
    )
    .unwrap();
    tb.file(".local/state/everythux/everythux.log", 3).unwrap();
    tb.file("src/seen.rs", 1).unwrap();
    wait_snapshot(&rx, "src file", |s| listing(s, &root).contains_key("src/seen.rs"));
    // Give a (wrongly) watched folder time to be refreshed, then check it was not.
    std::thread::sleep(Duration::from_millis(600));
    let snap = indexer.snapshot();
    let rows = listing(&snap, &root);
    assert!(
        !rows.contains_key(".config/everythux/config.json"),
        "config folder was watched"
    );
    assert!(
        !rows.contains_key(".local/state/everythux/everythux.log"),
        "log folder was watched"
    );
}

#[test]
fn manual_mode_starts_no_watcher_and_switching_it_on_or_off_takes_effect_at_runtime() {
    let (db, tree) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let tb = TreeBuilder::new(tree.path());
    tb.file("dir/one.txt", 1).unwrap();
    let root = tree.path().to_path_buf();
    let mut manual = watching_settings(tree.path());
    manual.indexer.automatic = false;
    let (indexer, rx) = start_scanned(db.path(), manual.clone());

    // Off: a change is not noticed (no watcher, so no refresh job and no new snapshot).
    tb.file("dir/ignored.txt", 1).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    assert!(!listing(&indexer.snapshot(), &root).contains_key("dir/ignored.txt"));

    // On, without a restart: the watcher starts. `ApplyExclusions` is a no-op job that tells
    // us the settings were applied (the worker takes settings before queued jobs).
    indexer.update_settings(watching_settings(tree.path()));
    crate::common::run_ok(&indexer, &rx, everythux_index::Job::ApplyExclusions);
    // The watcher adds its watches on its own thread, so a file created too early is missed:
    // keep creating files until one is noticed.
    let deadline = Instant::now() + TIMEOUT;
    let mut n = 0;
    while !listing(&indexer.snapshot(), &root)
        .keys()
        .any(|k| k.starts_with("dir/seen"))
    {
        assert!(
            Instant::now() < deadline,
            "no live update after switching automatic on"
        );
        n += 1;
        tb.file(format!("dir/seen{n}.txt"), 1).unwrap();
        std::thread::sleep(Duration::from_millis(500));
    }

    // Off again: the watcher stops.
    indexer.update_settings(manual);
    crate::common::run_ok(&indexer, &rx, everythux_index::Job::ApplyExclusions);
    tb.file("dir/after.txt", 1).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    assert!(!listing(&indexer.snapshot(), &root).contains_key("dir/after.txt"));
}
