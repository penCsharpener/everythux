//! `Job::Renamed`: the index follows a rename the app already did on disk.

use std::path::{Path, PathBuf};

use everythux_core::exclusion::{ExclusionRule, RuleKind};
use everythux_core::model::EntryKind;
use everythux_index::test_utils::{TreeBuilder, generate_tree, raw_name};
use everythux_index::{Indexer, Job};

use crate::common::{
    Done, check_invariants, dir_total, dir_totals, eventually, listing, run, run_ok, settings, start_scanned,
};

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// Renames on disk (as the app does through `platform`), then submits the job.
fn rename(
    indexer: &Indexer,
    rx: &crossbeam_channel::Receiver<everythux_index::IndexEvent>,
    from: &Path,
    to: &Path,
) -> everythux_index::JobStats {
    std::fs::rename(from, to).expect("rename on disk");
    run_ok(
        indexer,
        rx,
        Job::Renamed {
            from: from.to_owned(),
            to: to.to_owned(),
        },
    )
}

fn sample(root: &Path) {
    let tb = TreeBuilder::new(root);
    tb.file("docs/Report.TXT", 10).expect("fixture");
    tb.file("docs/deep/er/est/file.md", 20).expect("fixture");
    tb.file("docs/deep/x.bin", 3).expect("fixture");
    tb.file("docs/sibling.txt", 4).expect("fixture");
    tb.dir("docs/empty").expect("fixture");
    tb.file("top.bin", 5).expect("fixture");
}

#[test]
fn renaming_a_file_updates_name_and_extension() {
    let (db, tree) = (tmp(), tmp());
    sample(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    let before = indexer.snapshot();
    let totals = dir_totals(&before, tree.path());

    let stats = rename(
        &indexer,
        &rx,
        &tree.path().join("docs/Report.TXT"),
        &tree.path().join("docs/Summary.Md"),
    );
    assert_eq!(stats.dirs_listed, 0, "{stats:?}");
    let snap = indexer.snapshot();
    let rows = listing(&snap, tree.path());
    assert!(!rows.contains_key("docs/Report.TXT"));
    assert_eq!(rows["docs/Summary.Md"].ext, "md");
    assert_eq!(rows["docs/Summary.Md"].size, 10);
    assert_eq!(dir_totals(&snap, tree.path()), totals, "sizes are unchanged");
    check_invariants(&snap);
    // The search side folds names: the folded name changed too.
    let id = (0..snap.len())
        .map(|i| everythux_core::ids::EntryId(u32::try_from(i).expect("fits")))
        .find(|&id| snap.name(id) == b"Summary.Md")
        .expect("renamed entry");
    assert_eq!(snap.extension(id), "md");
}

#[test]
fn renaming_a_folder_rewrites_every_descendant_path() {
    let (db, tree) = (tmp(), tmp());
    sample(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    let before = indexer.snapshot();
    let old_rows = listing(&before, tree.path());
    let old_totals = dir_totals(&before, tree.path());

    let stats = rename(
        &indexer,
        &rx,
        &tree.path().join("docs"),
        &tree.path().join("Papers.v2"),
    );
    assert_eq!(stats.dirs_listed, 0, "no walk: {stats:?}");
    assert_eq!(stats.upserted, 1 + 5, "the entry plus 5 folder rows: {stats:?}");

    let snap = indexer.snapshot();
    check_invariants(&snap);
    let rows = listing(&snap, tree.path());
    assert!(
        rows.keys().all(|k| k != "docs" && !k.starts_with("docs/")),
        "no old paths: {:?}",
        rows.keys().collect::<Vec<_>>()
    );
    let expected: Vec<(String, _)> = old_rows
        .iter()
        .filter_map(|(k, v)| {
            let new = if k == "docs" {
                "Papers.v2".to_owned()
            } else {
                k.strip_prefix("docs/").map(|r| format!("Papers.v2/{r}"))?
            };
            Some((new, v.clone()))
        })
        .collect();
    assert!(expected.len() >= 8);
    for (k, v) in expected {
        let got = rows.get(&k).unwrap_or_else(|| panic!("{k} is indexed"));
        assert_eq!(got.kind, v.kind, "{k}");
        assert_eq!(got.size, v.size, "{k}");
    }
    assert_eq!(
        rows["Papers.v2"].ext, "v2",
        "the folder entry's extension follows the name"
    );
    assert_eq!(rows["Papers.v2/deep/er/est/file.md"].size, 20);
    assert_eq!(dir_total(&snap, &tree.path().join("Papers.v2")), Some(37));
    assert_eq!(dir_total(&snap, tree.path()), old_totals.get("").copied());
    assert_eq!(rows["Papers.v2"].kind, EntryKind::Dir);
}

#[test]
fn the_database_matches_a_fresh_scan_after_a_folder_rename() {
    let (db, tree) = (tmp(), tmp());
    sample(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    rename(
        &indexer,
        &rx,
        &tree.path().join("docs/deep"),
        &tree.path().join("docs/deeper"),
    );
    let incremental = indexer.snapshot();
    run_ok(&indexer, &rx, Job::FullReindex);
    let rebuilt = indexer.snapshot();
    assert_eq!(listing(&incremental, tree.path()), listing(&rebuilt, tree.path()));
    assert_eq!(
        dir_totals(&incremental, tree.path()),
        dir_totals(&rebuilt, tree.path())
    );
}

#[test]
fn case_only_rename() {
    let (db, tree) = (tmp(), tmp());
    sample(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));

    let stats = rename(
        &indexer,
        &rx,
        &tree.path().join("docs"),
        &tree.path().join("DOCS"),
    );
    assert_eq!(stats.dirs_listed, 0, "{stats:?}");
    let rows = listing(&indexer.snapshot(), tree.path());
    assert!(rows.contains_key("DOCS/deep/x.bin") && !rows.contains_key("docs/deep/x.bin"));

    let stats = rename(
        &indexer,
        &rx,
        &tree.path().join("top.bin"),
        &tree.path().join("TOP.BIN"),
    );
    assert_eq!(stats.dirs_listed, 0, "{stats:?}");
    let snap = indexer.snapshot();
    let rows = listing(&snap, tree.path());
    assert!(rows.contains_key("TOP.BIN") && !rows.contains_key("top.bin"));
    assert_eq!(rows["TOP.BIN"].ext, "bin");
    check_invariants(&snap);
}

#[test]
fn rename_to_a_non_utf8_name() {
    let (db, tree) = (tmp(), tmp());
    sample(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));

    let bad_dir = tree.path().join(raw_name(b"caf\xe9-dir"));
    let stats = rename(&indexer, &rx, &tree.path().join("docs"), &bad_dir);
    assert_eq!(stats.dirs_listed, 0, "{stats:?}");
    let bad_file = bad_dir.join(raw_name(b"na\xefve.TXT"));
    std::fs::rename(bad_dir.join("sibling.txt"), &bad_file).expect("disk rename");
    run_ok(
        &indexer,
        &rx,
        Job::Renamed {
            from: bad_dir.join("sibling.txt"),
            to: bad_file.clone(),
        },
    );

    let snap = indexer.snapshot();
    check_invariants(&snap);
    let names: Vec<Vec<u8>> = (0..snap.len())
        .map(|i| {
            snap.name(everythux_core::ids::EntryId(u32::try_from(i).expect("fits")))
                .to_vec()
        })
        .collect();
    assert!(names.contains(&b"caf\xe9-dir".to_vec()), "raw bytes preserved");
    assert!(names.contains(&b"na\xefve.TXT".to_vec()));
    // Descendants of the non-UTF-8 folder have rewritten paths too.
    let deep = bad_dir.join("deep/er/est");
    assert!(
        dir_total(&snap, &deep).is_some(),
        "folder path rewritten as raw bytes"
    );
    // A full rescan agrees.
    run_ok(&indexer, &rx, Job::FullReindex);
    assert_eq!(
        listing(&snap, tree.path()),
        listing(&indexer.snapshot(), tree.path())
    );
}

#[test]
fn a_rename_into_another_folder_is_rejected() {
    let (db, tree) = (tmp(), tmp());
    sample(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    let before = listing(&indexer.snapshot(), tree.path());

    std::fs::rename(tree.path().join("top.bin"), tree.path().join("docs/top.bin")).expect("move");
    let done = run(
        &indexer,
        &rx,
        Job::Renamed {
            from: tree.path().join("top.bin"),
            to: tree.path().join("docs/top.bin"),
        },
    );
    let Done::Failed(message) = done else {
        panic!("expected a failure, got {done:?}");
    };
    assert!(message.contains("same folder"), "{message}");
    assert_eq!(
        listing(&indexer.snapshot(), tree.path()),
        before,
        "index untouched"
    );
}

#[test]
fn renaming_to_an_excluded_name_removes_the_subtree() {
    let (db, tree) = (tmp(), tmp());
    sample(tree.path());
    let mut s = settings(&[tree.path()]);
    s.exclusions = vec![
        ExclusionRule::new("node_modules", RuleKind::Dir),
        ExclusionRule::new("*.tmp", RuleKind::File),
    ];
    let (indexer, rx) = start_scanned(db.path(), s);

    rename(
        &indexer,
        &rx,
        &tree.path().join("docs"),
        &tree.path().join("node_modules"),
    );
    let snap = indexer.snapshot();
    check_invariants(&snap);
    let rows = listing(&snap, tree.path());
    assert!(
        rows.keys()
            .all(|k| !k.starts_with("docs") && !k.starts_with("node_modules")),
        "{:?}",
        rows.keys().collect::<Vec<_>>()
    );
    assert_eq!(rows["top.bin"].size, 5);
    assert_eq!(dir_total(&snap, tree.path()), Some(5));

    // A file renamed to an excluded name disappears as well.
    rename(
        &indexer,
        &rx,
        &tree.path().join("top.bin"),
        &tree.path().join("scratch.tmp"),
    );
    let snap = indexer.snapshot();
    check_invariants(&snap);
    assert!(listing(&snap, tree.path()).is_empty());
}

#[test]
fn renaming_a_path_that_was_not_indexed_indexes_the_new_one() {
    let (db, tree) = (tmp(), tmp());
    sample(tree.path());
    TreeBuilder::new(tree.path())
        .file("node_modules/pkg/index.js", 9)
        .expect("fixture");
    let mut s = settings(&[tree.path()]);
    s.exclusions = vec![ExclusionRule::new("node_modules", RuleKind::Dir)];
    let (indexer, rx) = start_scanned(db.path(), s);
    assert!(
        listing(&indexer.snapshot(), tree.path())
            .keys()
            .all(|k| !k.starts_with("node_modules"))
    );

    let stats = rename(
        &indexer,
        &rx,
        &tree.path().join("node_modules"),
        &tree.path().join("modules"),
    );
    assert!(stats.dirs_listed >= 1, "the fallback re-lists: {stats:?}");
    let snap = indexer.snapshot();
    check_invariants(&snap);
    let rows = listing(&snap, tree.path());
    assert_eq!(rows["modules/pkg/index.js"].size, 9);
    assert_eq!(dir_total(&snap, &tree.path().join("modules")), Some(9));
}

#[test]
fn the_watcher_follow_up_refresh_changes_nothing() {
    let (db, tree) = (tmp(), tmp());
    sample(tree.path());
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));

    for (from, to) in [("docs/deep", "docs/deeper"), ("docs/sibling.txt", "docs/s2.txt")] {
        rename(&indexer, &rx, &tree.path().join(from), &tree.path().join(to));
        let parent = tree.path().join("docs");
        let stats = run_ok(&indexer, &rx, Job::RefreshDirs(vec![parent.clone()]));
        assert_eq!((stats.upserted, stats.removed), (0, 0), "{from}: {stats:?}");
        // The watcher also reports the renamed folder itself.
        let stats = run_ok(
            &indexer,
            &rx,
            Job::RefreshDirs(vec![tree.path().join("docs/deeper")]),
        );
        assert_eq!((stats.upserted, stats.removed), (0, 0), "{from}: {stats:?}");
    }
    check_invariants(&indexer.snapshot());
}

#[test]
fn a_big_folder_is_renamed_without_a_walk() {
    let (db, tree) = (tmp(), tmp());
    let big = tree.path().join("big");
    std::fs::create_dir(&big).expect("fixture");
    let made = generate_tree(&big, 20_000, 10_000, 7).expect("generate");
    let (indexer, rx) = start_scanned(db.path(), settings(&[tree.path()]));
    let before = indexer.snapshot();
    let totals = dir_totals(&before, tree.path());

    let to: PathBuf = tree.path().join("big renamed");
    let stats = rename(&indexer, &rx, &big, &to);
    assert_eq!(stats.dirs_listed, 0, "{stats:?}");
    assert_eq!(stats.upserted, 1 + 1 + made.dirs as u64, "{stats:?}");
    eprintln!(
        "rename of a folder with {} descendant folders: {:?}",
        made.dirs, stats.elapsed
    );
    assert!(
        stats.elapsed < std::time::Duration::from_secs(5),
        "well under a second in release builds: {:?}",
        stats.elapsed
    );

    let snap = eventually(&indexer, "renamed snapshot", |s| {
        listing(s, tree.path()).contains_key("big renamed")
    });
    check_invariants(&snap);
    let after = dir_totals(&snap, tree.path());
    assert_eq!(after.len(), totals.len());
    assert_eq!(after.get("big renamed"), totals.get("big"));
    assert!(
        listing(&snap, tree.path())
            .keys()
            .all(|k| k != "big" && !k.starts_with("big/"))
    );
}
