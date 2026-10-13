//! The indexer end to end: jobs, events, snapshots, partial rescans.

use std::path::Path;
use std::time::Duration;

use everythux_core::exclusion::{ExclusionRule, RuleKind};
use everythux_core::model::{EntryKind, VolumeState};
use everythux_index::test_utils::{TreeBuilder, generate_tree, raw_name};
use everythux_index::{IndexEvent, Indexer, Job, ScanMode};

use crate::common::{
    Done, check_invariants, dir_total, dir_totals, eventually, listing, row, run, run_ok, settings,
    start_scanned, wait_done, wait_for,
};

const OLD: u64 = 1_000_000;
/// Total size of `sample_tree`: 10 + 20 + 5 + 7 bytes of files plus the 1-byte symlink `link -> a`.
const BASE: u64 = 43;

/// A small tree with fixed (old) folder mtimes, so later changes are always visible to quick scans.
fn sample_tree(root: &Path) {
    let tb = TreeBuilder::new(root);
    tb.file("a/b/c.txt", 10).expect("fixture");
    tb.file("a/d.TXT", 20).expect("fixture");
    tb.dir("e").expect("fixture");
    tb.file("top.bin", 5).expect("fixture");
    tb.symlink("link", "a").expect("fixture");
    tb.file(raw_name(b"bad\xffname.Log"), 7).expect("fixture");
    freeze_mtimes(root);
}

/// Sets every folder's mtime (children first) to a fixed old time.
fn freeze_mtimes(root: &Path) {
    fn walk(tb: &TreeBuilder, dir: &Path) {
        for entry in std::fs::read_dir(dir).expect("fixture").flatten() {
            if entry.file_type().expect("fixture").is_dir() {
                walk(tb, &entry.path());
            }
        }
        tb.set_mtime(dir, OLD).expect("fixture");
    }
    walk(&TreeBuilder::new(root), root);
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn kind_name(e: &IndexEvent) -> &'static str {
    match e {
        IndexEvent::Started { .. } => "Started",
        IndexEvent::Progress { .. } => "Progress",
        IndexEvent::SnapshotReady(_) => "SnapshotReady",
        IndexEvent::VolumesChanged(_) => "VolumesChanged",
        IndexEvent::Finished { .. } => "Finished",
        IndexEvent::Cancelled { .. } => "Cancelled",
        IndexEvent::Failed { .. } => "Failed",
    }
}

#[test]
fn startup_event_flow_and_first_full_scan() {
    let (db, tree) = (tmp(), tmp());
    sample_tree(tree.path());
    let (indexer, rx) = Indexer::start(&db.path().join("index.db"), settings(&[tree.path()])).unwrap();

    // Order: empty snapshot, volumes, then the automatic full scan of the empty database.
    let first = rx.recv_timeout(Duration::from_secs(60)).unwrap();
    let IndexEvent::SnapshotReady(empty) = first else {
        panic!("expected SnapshotReady first, got {first:?}");
    };
    assert!(empty.is_empty());
    let second = rx.recv_timeout(Duration::from_secs(60)).unwrap();
    let IndexEvent::VolumesChanged(volumes) = second else {
        panic!("expected VolumesChanged second, got {second:?}");
    };
    assert_eq!(volumes.len(), 1);
    assert_eq!(volumes[0].state, VolumeState::Online);

    let mut seen = Vec::new();
    let stats = wait_for(&rx, "first scan", |e| {
        seen.push(kind_name(e));
        match e {
            IndexEvent::Finished { stats, .. } => Some(stats.clone()),
            IndexEvent::Failed { error, .. } => panic!("{error}"),
            _ => None,
        }
    });
    assert_eq!(seen.first().copied(), Some("Started"), "{seen:?}");
    let snapshot_pos = seen
        .iter()
        .position(|s| *s == "SnapshotReady")
        .expect("snapshot published");
    assert!(
        snapshot_pos < seen.len() - 1,
        "SnapshotReady precedes Finished: {seen:?}"
    );
    assert!(stats.upserted >= 7, "{stats:?}");
    assert_eq!(stats.errors, 0);

    let snap = indexer.snapshot();
    check_invariants(&snap);
    let rows = listing(&snap, tree.path());
    assert_eq!(rows["a/b/c.txt"].size, 10);
    assert_eq!(rows["a/d.TXT"].ext, "txt");
    assert_eq!(rows["link"].kind, EntryKind::Symlink);
    assert_eq!(rows["e"].kind, EntryKind::Dir);
    assert_eq!(rows["a"].size, 30);
    assert_eq!(dir_total(&snap, tree.path()), Some(BASE));
    let bad = rows
        .keys()
        .find(|k| k.starts_with("bad"))
        .expect("invalid utf-8 name indexed");
    assert!(bad.contains('\u{fffd}'), "{bad:?}");
    let id = (0..snap.len())
        .map(|i| everythux_core::ids::EntryId(u32::try_from(i).unwrap()))
        .find(|&id| snap.name(id).starts_with(b"bad"))
        .unwrap();
    assert_eq!(snap.name(id), b"bad\xffname.Log", "names stay raw bytes");
    assert_eq!(snap.extension(id), "log");
}

#[test]
fn quick_rescan_sees_changes_and_skips_unchanged_folders() {
    let (db, tree) = (tmp(), tmp());
    sample_tree(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    let tb = TreeBuilder::new(tree.path());

    // Nothing changed: no folder is re-listed and no new snapshot is needed.
    let stats = run_ok(&indexer, &rx, Job::RescanAll(ScanMode::Quick));
    assert_eq!(stats.dirs_listed, 0, "{stats:?}");
    assert_eq!((stats.upserted, stats.removed), (0, 0));

    // One folder changes: a/b gets a file; top.bin disappears (root changes); a new folder appears.
    tb.file("a/b/new.txt", 100).unwrap();
    tb.remove("top.bin").unwrap();
    tb.file("fresh/deep/x.md", 9).unwrap();
    let stats = run_ok(&indexer, &rx, Job::RescanAll(ScanMode::Quick));
    // listed: a/b (changed), root (changed), fresh and fresh/deep (new). `a` and `e` untouched.
    assert_eq!(stats.dirs_listed, 4, "{stats:?}");

    let snap = indexer.snapshot();
    check_invariants(&snap);
    let rows = listing(&snap, tree.path());
    assert_eq!(rows["a/b/new.txt"].size, 100);
    assert!(!rows.contains_key("top.bin"));
    assert_eq!(rows["fresh/deep/x.md"].ext, "md");
    assert_eq!(dir_total(&snap, tree.path()), Some(BASE - 5 + 100 + 9));
    assert_eq!(rows["a"].size, 130);

    // Removing a whole folder tree.
    tb.remove("a").unwrap();
    run_ok(&indexer, &rx, Job::RescanAll(ScanMode::Quick));
    let snap = indexer.snapshot();
    check_invariants(&snap);
    let rows = listing(&snap, tree.path());
    assert!(
        rows.keys().all(|k| !k.starts_with("a/") && k != "a"),
        "{:?}",
        rows.keys()
    );
}

#[test]
fn quick_misses_in_place_changes_that_full_rescans_catch() {
    let (db, tree) = (tmp(), tmp());
    sample_tree(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));

    // Growing a file does not touch its folder's mtime.
    std::fs::OpenOptions::new()
        .write(true)
        .open(tree.path().join("a/d.TXT"))
        .unwrap()
        .set_len(999)
        .unwrap();
    run_ok(&indexer, &rx, Job::RescanAll(ScanMode::Quick));
    assert_eq!(
        row(&indexer.snapshot(), tree.path(), "a/d.TXT").unwrap().size,
        20,
        "quick scans trust folder mtimes"
    );

    let stats = run_ok(&indexer, &rx, Job::RescanAll(ScanMode::Full));
    assert!(stats.dirs_listed >= 4);
    let snap = indexer.snapshot();
    check_invariants(&snap);
    assert_eq!(row(&snap, tree.path(), "a/d.TXT").unwrap().size, 999);
    assert_eq!(dir_total(&snap, tree.path()), Some(BASE - 20 + 999));
}

#[test]
fn rescan_path_replaces_only_that_subtree() {
    let (db, tree) = (tmp(), tmp());
    sample_tree(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    let tb = TreeBuilder::new(tree.path());

    tb.file("a/b/c2.txt", 11).unwrap().remove("a/d.TXT").unwrap();
    tb.file("outside.txt", 77).unwrap(); // not under `a`: must stay unseen
    let stats = run_ok(
        &indexer,
        &rx,
        Job::RescanPath {
            path: tree.path().join("a"),
            mode: ScanMode::Full,
        },
    );
    assert_eq!(stats.dirs_listed, 2, "a and a/b");
    let snap = indexer.snapshot();
    check_invariants(&snap);
    let rows = listing(&snap, tree.path());
    assert!(rows.contains_key("a/b/c2.txt"));
    assert!(!rows.contains_key("a/d.TXT"));
    assert!(
        !rows.contains_key("outside.txt"),
        "partial scan must not touch the rest"
    );
    assert_eq!(rows["a"].size, 21, "ancestors' sizes follow the subtree");
    assert_eq!(dir_total(&snap, tree.path()), Some(BASE - 20 + 11));

    // Quick mode on the same path with a new nested folder.
    tb.file("a/b/new/deeper/z", 3).unwrap();
    run_ok(
        &indexer,
        &rx,
        Job::RescanPath {
            path: tree.path().join("a/b"),
            mode: ScanMode::Quick,
        },
    );
    let snap = indexer.snapshot();
    check_invariants(&snap);
    assert!(listing(&snap, tree.path()).contains_key("a/b/new/deeper/z"));

    // A folder that is not indexed yet is found through its nearest indexed ancestor.
    tb.file("brand/new/file.txt", 1).unwrap();
    run_ok(
        &indexer,
        &rx,
        Job::RescanPath {
            path: tree.path().join("brand/new"),
            mode: ScanMode::Full,
        },
    );
    assert!(listing(&indexer.snapshot(), tree.path()).contains_key("brand/new/file.txt"));

    // A folder that is gone is removed.
    tb.remove("brand").unwrap();
    run_ok(
        &indexer,
        &rx,
        Job::RescanPath {
            path: tree.path().join("brand"),
            mode: ScanMode::Quick,
        },
    );
    let snap = indexer.snapshot();
    check_invariants(&snap);
    assert!(!listing(&snap, tree.path()).keys().any(|k| k.starts_with("brand")));
}

#[test]
fn rescan_volume_accepts_uuid_and_mount_point_and_rejects_strangers() {
    let (db, tree) = (tmp(), tmp());
    sample_tree(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    let canonical = tree.path().canonicalize().unwrap();
    let uuid = format!("dir:{}", canonical.display());

    TreeBuilder::new(&canonical).file("one.txt", 1).unwrap();
    for ident in [uuid, canonical.display().to_string()] {
        run_ok(
            &indexer,
            &rx,
            Job::RescanVolume {
                volume: ident,
                mode: ScanMode::Quick,
            },
        );
    }
    assert!(listing(&indexer.snapshot(), &canonical).contains_key("one.txt"));

    let Done::Failed(msg) = run(
        &indexer,
        &rx,
        Job::RescanVolume {
            volume: "nope".into(),
            mode: ScanMode::Full,
        },
    ) else {
        panic!("unknown volume must fail");
    };
    assert!(
        msg.contains("not part of any indexed volume") && msg.contains("nope"),
        "{msg}"
    );
    let Done::Failed(msg) = run(
        &indexer,
        &rx,
        Job::RescanPath {
            path: "/definitely/elsewhere".into(),
            mode: ScanMode::Quick,
        },
    ) else {
        panic!("path outside every volume must fail");
    };
    assert!(msg.contains("not part of any indexed volume"), "{msg}");
}

#[test]
fn apply_exclusions_removes_matching_entries_without_touching_disk() {
    let (db, tree) = (tmp(), tmp());
    let tb = TreeBuilder::new(tree.path());
    tb.file("proj/src/main.rs", 100).unwrap();
    tb.file("proj/node_modules/pkg/index.js", 1000).unwrap();
    tb.file("proj/node_modules/x.js", 5).unwrap();
    tb.file("proj/build.tmp", 7).unwrap();
    tb.file("other/node_modules/y.js", 9).unwrap();
    tb.file("other/keep.txt", 1).unwrap();
    tb.file("secret/a.txt", 2).unwrap();
    freeze_mtimes(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    assert_eq!(
        dir_total(&indexer.snapshot(), tree.path()),
        Some(100 + 1000 + 5 + 7 + 9 + 1 + 2)
    );

    let mut s = settings(&[tree.path()]);
    s.exclusions = vec![
        ExclusionRule::new("[", RuleKind::Any), // invalid: must be skipped, not fatal
        ExclusionRule::new("node_modules", RuleKind::Dir),
        ExclusionRule::new("*.tmp", RuleKind::File),
        ExclusionRule::new(tree.path().join("secret").to_str().unwrap(), RuleKind::Path),
    ];
    indexer.update_settings(s);
    let stats = run_ok(&indexer, &rx, Job::ApplyExclusions);
    assert!(stats.removed >= 8, "{stats:?}");

    let snap = indexer.snapshot();
    check_invariants(&snap);
    let rows = listing(&snap, tree.path());
    let keys: Vec<_> = rows.keys().map(String::as_str).collect();
    assert!(keys.iter().all(|k| !k.contains("node_modules")), "{keys:?}");
    assert!(!keys.contains(&"proj/build.tmp"));
    assert!(!keys.iter().any(|k| k.starts_with("secret")), "{keys:?}");
    assert!(keys.contains(&"proj/src/main.rs") && keys.contains(&"other/keep.txt"));
    assert_eq!(dir_total(&snap, tree.path()), Some(100 + 1));
    assert_eq!(rows["proj"].size, 100);
    assert!(
        tree.path().join("proj/node_modules/x.js").exists(),
        "the disk is untouched"
    );

    // The same rules now prune a later full scan.
    TreeBuilder::new(tree.path())
        .file("proj/node_modules/again.js", 1)
        .unwrap();
    run_ok(&indexer, &rx, Job::RescanAll(ScanMode::Full));
    let rows = listing(&indexer.snapshot(), tree.path());
    assert!(rows.keys().all(|k| !k.contains("node_modules")));
}

#[test]
fn full_reindex_matches_incremental_updates() {
    // Property: after arbitrary changes, incremental jobs and a from-scratch build agree.
    let (db, db2, tree) = (tmp(), tmp(), tmp());
    generate_tree(tree.path(), 600, 40, 11).unwrap();
    freeze_mtimes(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    let tb = TreeBuilder::new(tree.path());

    tb.file("new/a/b/c.txt", 123).unwrap();
    tb.dir("emptydir").unwrap();
    let victim = std::fs::read_dir(tree.path())
        .unwrap()
        .flatten()
        .find(|e| e.file_type().unwrap().is_dir() && e.file_name() != "new" && e.file_name() != "emptydir")
        .map(|e| e.file_name())
        .expect("generated tree has folders");
    tb.remove(&victim).unwrap();
    run_ok(&indexer, &rx, Job::RescanAll(ScanMode::Quick));
    run_ok(
        &indexer,
        &rx,
        Job::RefreshDirs(vec![tree.path().to_path_buf(), tree.path().join("new")]),
    );

    let incremental = indexer.snapshot();
    check_invariants(&incremental);

    let (fresh, _rx2) = start_scanned(db2.path(), settings(&[tree.path()]));
    let rebuilt = fresh.snapshot();
    assert_eq!(listing(&incremental, tree.path()), listing(&rebuilt, tree.path()));
    assert_eq!(
        dir_totals(&incremental, tree.path()),
        dir_totals(&rebuilt, tree.path())
    );
}

#[test]
fn refresh_dirs_relists_exactly_the_given_folders() {
    let (db, tree) = (tmp(), tmp());
    sample_tree(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    let tb = TreeBuilder::new(tree.path());

    tb.file("a/live.txt", 5)
        .unwrap()
        .file("e/also.txt", 6)
        .unwrap()
        .file("top2.bin", 8)
        .unwrap();
    let stats = run_ok(&indexer, &rx, Job::RefreshDirs(vec![tree.path().join("a")]));
    assert_eq!(stats.dirs_listed, 1, "{stats:?}");
    let snap = eventually(&indexer, "refreshed folder", |s| {
        listing(s, tree.path()).contains_key("a/live.txt")
    });
    let rows = listing(&snap, tree.path());
    assert!(
        !rows.contains_key("e/also.txt") && !rows.contains_key("top2.bin"),
        "other folders are not re-listed"
    );

    // A freshly created folder with content arrives through its parent and is indexed fully.
    tb.file("moved_in/x/y.txt", 4).unwrap();
    run_ok(&indexer, &rx, Job::RefreshDirs(vec![tree.path().to_path_buf()]));
    let snap = eventually(&indexer, "new folder", |s| {
        listing(s, tree.path()).contains_key("moved_in/x/y.txt")
    });
    check_invariants(&snap);

    // Vanished folders and files disappear; unknown paths are ignored.
    tb.remove("a").unwrap();
    run_ok(
        &indexer,
        &rx,
        Job::RefreshDirs(vec![
            tree.path().join("a"),
            tree.path().join("a/b"),
            "/elsewhere".into(),
        ]),
    );
    let snap = eventually(&indexer, "removed folder", |s| {
        listing(s, tree.path())
            .keys()
            .all(|k| k != "a" && !k.starts_with("a/"))
    });
    check_invariants(&snap);
}

#[test]
fn full_reindex_jobs_are_coalesced() {
    let (db, tree) = (tmp(), tmp());
    generate_tree(tree.path(), 300, 20, 2).unwrap();
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    let busy = indexer.submit(Job::RescanAll(ScanMode::Full));
    let first = indexer.submit(Job::FullReindex);
    let second = indexer.submit(Job::FullReindex);
    assert_eq!(first, second, "a queued FullReindex absorbs duplicates");
    assert_ne!(first, busy);
    wait_done(&rx, busy);
    wait_done(&rx, first).stats();
    // Once finished, a new one is a new job.
    assert_ne!(indexer.submit(Job::FullReindex), first);
}

#[test]
fn cancelling_a_full_reindex_keeps_the_previous_index() {
    let (db, tree) = (tmp(), tmp());
    sample_tree(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    let before = listing(&indexer.snapshot(), tree.path());

    // Make the next build long enough to cancel reliably.
    std::fs::create_dir(tree.path().join("big")).unwrap();
    generate_tree(&tree.path().join("big"), 20_000, 400, 4).unwrap();
    let id = indexer.submit(Job::FullReindex);
    wait_for(&rx, "start", |e| {
        matches!(e, IndexEvent::Started { job, .. } if *job == id).then_some(())
    });
    indexer.cancel_current();
    match wait_done(&rx, id) {
        Done::Cancelled => {
            assert_eq!(
                listing(&indexer.snapshot(), tree.path()),
                before,
                "the old index stays searchable"
            );
            assert!(!db.path().join("index.db.building").exists());
        }
        other => panic!("expected the job to be cancelled, got {other:?}"),
    }
    // The indexer is still usable afterwards.
    let stats = run_ok(&indexer, &rx, Job::FullReindex);
    assert!(stats.upserted > 20_000);
    assert!(listing(&indexer.snapshot(), tree.path()).len() > before.len());
}

#[test]
fn offline_volumes_keep_their_entries_and_come_back() {
    let (db, a, b) = (tmp(), tmp(), tmp());
    TreeBuilder::new(a.path()).file("in_a.txt", 1).unwrap();
    TreeBuilder::new(b.path()).file("in_b.txt", 2).unwrap();
    let (indexer, rx) = start_scanned(db.path(), settings(&[a.path(), b.path()]));
    assert_eq!(indexer.snapshot().volumes.len(), 2);

    // Only `a` is "mounted" now.
    indexer.update_settings(settings(&[a.path()]));
    run_ok(&indexer, &rx, Job::FullReindex);
    let snap = indexer.snapshot();
    check_invariants(&snap);
    assert_eq!(
        snap.volumes
            .iter()
            .filter(|v| v.state == VolumeState::Offline)
            .count(),
        1
    );
    assert!(
        listing(&snap, b.path()).contains_key("in_b.txt"),
        "offline entries stay searchable"
    );
    let Done::Failed(msg) = run(
        &indexer,
        &rx,
        Job::RescanVolume {
            volume: b.path().canonicalize().unwrap().display().to_string(),
            mode: ScanMode::Quick,
        },
    ) else {
        panic!("rescanning an offline volume must fail");
    };
    assert!(msg.contains("not mounted"), "{msg}");

    // It comes back (the same folder is the same volume identity) and is rescanned.
    TreeBuilder::new(b.path()).file("later.txt", 3).unwrap();
    indexer.update_settings(settings(&[a.path(), b.path()]));
    run_ok(&indexer, &rx, Job::RescanAll(ScanMode::Quick));
    let snap = indexer.snapshot();
    assert!(snap.volumes.iter().all(|v| v.state == VolumeState::Online));
    assert!(listing(&snap, b.path()).contains_key("later.txt"));
}

#[test]
fn restart_reuses_the_database_and_rescans_quickly() {
    let (db, tree) = (tmp(), tmp());
    sample_tree(tree.path());
    let path = db.path().join("index.db");
    {
        let (_indexer, _rx) = start_scanned(db.path(), settings(&[tree.path()]));
    }
    TreeBuilder::new(tree.path()).file("while_away.txt", 1).unwrap();
    let mut s = settings(&[tree.path()]);
    s.indexer.rescan_on_startup = everythux_core::config::StartupRescan::Quick;
    let (indexer, rx) = Indexer::start(&path, s).unwrap();
    // First snapshot already holds the stored index …
    let IndexEvent::SnapshotReady(stored) = wait_for(&rx, "snapshot", |e| match e {
        IndexEvent::SnapshotReady(_) => Some(e.clone()),
        _ => None,
    }) else {
        unreachable!()
    };
    assert!(listing(&stored, tree.path()).contains_key("a/d.TXT"));
    assert!(!listing(&stored, tree.path()).contains_key("while_away.txt"));
    // … and the startup quick rescan brings it up to date.
    wait_for(&rx, "rescan", |e| {
        matches!(e, IndexEvent::Finished { .. }).then_some(())
    });
    assert!(listing(&indexer.snapshot(), tree.path()).contains_key("while_away.txt"));
}

#[test]
fn dropping_the_indexer_stops_the_worker() {
    let (db, tree) = (tmp(), tmp());
    sample_tree(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    drop(indexer);
    // The worker is joined and released its sender: the event stream ends.
    while rx.recv_timeout(Duration::from_secs(10)).is_ok() {}
    assert!(rx.recv_timeout(Duration::from_millis(10)).is_err());
}

#[test]
fn settings_updates_apply_to_later_jobs() {
    let (db, tree) = (tmp(), tmp());
    TreeBuilder::new(tree.path())
        .file("keep.txt", 1)
        .unwrap()
        .file("skip/me.txt", 1)
        .unwrap();
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    assert!(listing(&indexer.snapshot(), tree.path()).contains_key("skip/me.txt"));

    let mut s = settings(&[tree.path()]);
    s.exclusions = vec![ExclusionRule::new("skip", RuleKind::Dir)];
    indexer.update_settings(s);
    run_ok(&indexer, &rx, Job::FullReindex);
    let rows = listing(&indexer.snapshot(), tree.path());
    assert!(rows.contains_key("keep.txt") && !rows.contains_key("skip") && !rows.contains_key("skip/me.txt"));
}

#[test]
fn scanning_state_is_announced_and_reset_even_when_nothing_changed() {
    let (db, tree) = (tmp(), tmp());
    sample_tree(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));

    let id = indexer.submit(Job::RescanAll(ScanMode::Quick));
    let (mut saw_scanning, mut last) = (false, Vec::new());
    wait_for(&rx, "the quick rescan", |e| match e {
        IndexEvent::VolumesChanged(volumes) => {
            saw_scanning |= volumes.iter().any(|v| v.state == VolumeState::Scanning);
            last.clone_from(volumes);
            None
        }
        IndexEvent::Finished { job, stats } if *job == id => {
            assert_eq!((stats.upserted, stats.removed), (0, 0), "nothing changed");
            Some(())
        }
        _ => None,
    });
    assert!(saw_scanning, "the UI is told which volume is being scanned");
    assert!(
        last.iter().all(|v| v.state == VolumeState::Online),
        "the final volume list replaces the transient state: {last:?}"
    );
}
