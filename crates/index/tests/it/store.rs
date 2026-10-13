//! The SQLite store: schema handling, rebuild/swap, crash safety.

use std::path::Path;

use everythux_core::model::VolumeState;
use everythux_index::test_utils::{TreeBuilder, generate_tree};
use everythux_index::walker::{WalkControl, WalkJob, WalkMode, WalkOptions, make_pool};
use everythux_index::{DiscoveredVolume, IndexError, Rebuild, Store};

use crate::common::{check_invariants, dir_total, listing};

fn volume_for(root: &Path) -> DiscoveredVolume {
    everythux_index::discover_volumes(&everythux_core::config::VolumesConfig {
        auto_discover_fs_types: vec![],
        include: vec![root.to_path_buf()],
        disabled_uuids: vec![],
    })
    .expect("discover")
    .remove(0)
}

/// Walks `volume` into `rebuild` through the public API (what the indexer does).
fn scan_into(rebuild: &mut Rebuild, volume: &DiscoveredVolume) {
    let id = rebuild.add_volume(volume).expect("add volume");
    let ids = rebuild.ids();
    let ctl = WalkControl::new();
    let pool = make_pool(2);
    let opts = WalkOptions {
        batch_rows: 100,
        ..WalkOptions::default()
    };
    let mode = WalkMode::Fresh { existing_root: None };
    let job = WalkJob {
        root: &volume.mount_point,
        mode: &mode,
        opts: &opts,
        ids: &ids,
        ctl: &ctl,
        pool: pool.as_ref(),
    };
    let (tx, rx) = crossbeam_channel::bounded(2);
    std::thread::scope(|s| {
        let walker = s.spawn(|| {
            let r = job.run(&tx);
            drop(tx);
            r
        });
        for batch in &rx {
            rebuild.apply(id, batch).expect("apply");
        }
        walker.join().expect("join").expect("walk");
    });
    rebuild.finish_volume(id, 1).expect("finish volume");
}

fn build(store: &mut Store, root: &Path) -> DiscoveredVolume {
    let volume = volume_for(root);
    store.sync_volumes(std::slice::from_ref(&volume)).expect("sync");
    let mut rebuild = store.begin_rebuild(std::slice::from_ref(&volume)).expect("begin");
    scan_into(&mut rebuild, &volume);
    rebuild.finish(store).expect("finish");
    volume
}

#[test]
fn fresh_database_is_empty_and_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("sub/dir/index.db");
    {
        let store = Store::open(&db).unwrap();
        assert!(store.is_empty().unwrap());
        assert_eq!(store.stats().unwrap().entries, 0);
        assert_eq!(
            store.volumes().unwrap(),
            [] as [everythux_core::model::VolumeInfo; 0]
        );
        let snap = store.load_snapshot().unwrap();
        assert!(snap.is_empty());
        assert_eq!(&*snap.extensions[0], "");
    }
    Store::open(&db).unwrap();
}

#[test]
fn rebuild_swaps_atomically_and_loads_a_correct_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    let tb = TreeBuilder::new(tree.path());
    tb.file("a/b/c.txt", 10)
        .unwrap()
        .file("a/d.TXT", 20)
        .unwrap()
        .dir("e")
        .unwrap()
        .file("top.bin", 5)
        .unwrap();
    let db = dir.path().join("index.db");
    let mut store = Store::open(&db).unwrap();
    let volume = build(&mut store, tree.path());

    assert!(
        !dir.path().join("index.db.building").exists(),
        "building file is gone after the swap"
    );
    let stats = store.stats().unwrap();
    assert_eq!(
        (stats.entries, stats.dirs),
        (6, 4),
        "entries: a b e top.bin d.TXT c.txt; folders: root a a/b e"
    );
    assert_eq!(stats.volumes.len(), 1);
    assert_eq!(stats.volumes[0].0.fs_uuid, volume.fs_uuid);
    assert_eq!(stats.volumes[0].0.state, VolumeState::Online);
    assert_eq!(stats.volumes[0].1, 6);
    assert!(stats.volumes[0].0.last_full_scan_ns.is_some());
    assert!(stats.db_bytes > 0);

    let snap = store.load_snapshot().unwrap();
    check_invariants(&snap);
    let rows = listing(&snap, tree.path());
    assert_eq!(rows["a/b/c.txt"].size, 10);
    assert_eq!(rows["a/b/c.txt"].ext, "txt");
    assert_eq!(rows["a/d.TXT"].ext, "txt");
    assert_eq!(rows["a"].size, 30, "folder entries carry the recursive size");
    assert_eq!(rows["e"].size, 0);
    assert_eq!(dir_total(&snap, tree.path()), Some(35));
    assert_eq!(dir_total(&snap, &tree.path().join("a")), Some(30));
    assert_eq!(snap.volumes.len(), 1);
}

#[test]
fn aborted_build_leaves_the_old_database_intact() {
    let dir = tempfile::tempdir().unwrap();
    let old_tree = tempfile::tempdir().unwrap();
    TreeBuilder::new(old_tree.path()).file("old.txt", 1).unwrap();
    let db = dir.path().join("index.db");
    let mut store = Store::open(&db).unwrap();
    build(&mut store, old_tree.path());
    let before = listing(&store.load_snapshot().unwrap(), old_tree.path());

    // A second build over a different, larger tree that is abandoned half way.
    let new_tree = tempfile::tempdir().unwrap();
    generate_tree(new_tree.path(), 500, 30, 5).unwrap();
    let volume = volume_for(new_tree.path());
    let building = dir.path().join("index.db.building");
    {
        let mut rebuild = store.begin_rebuild(std::slice::from_ref(&volume)).unwrap();
        scan_into(&mut rebuild, &volume);
        assert!(building.exists(), "the build happens in <db>.building");
        // Dropped without finish(): this is what a cancelled or crashed build looks like.
    }
    assert!(!building.exists(), "an abandoned build cleans up after itself");

    assert_eq!(listing(&store.load_snapshot().unwrap(), old_tree.path()), before);
    drop(store);
    let reopened = Store::open(&db).unwrap();
    assert_eq!(
        listing(&reopened.load_snapshot().unwrap(), old_tree.path()),
        before
    );
}

#[test]
fn leftover_building_file_from_a_crash_is_ignored_and_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    TreeBuilder::new(tree.path()).file("f.txt", 3).unwrap();
    let db = dir.path().join("index.db");
    let mut store = Store::open(&db).unwrap();
    build(&mut store, tree.path());

    // A process killed mid-build leaves garbage next to the live database.
    std::fs::write(dir.path().join("index.db.building"), b"half written garbage").unwrap();
    drop(store);
    let mut store = Store::open(&db).expect("the live database is unaffected");
    assert_eq!(store.stats().unwrap().entries, 1);

    TreeBuilder::new(tree.path()).file("g.txt", 4).unwrap();
    build(&mut store, tree.path());
    assert_eq!(store.stats().unwrap().entries, 2);
    assert!(!dir.path().join("index.db.building").exists());
}

#[test]
fn rebuild_keeps_volumes_that_are_not_mounted() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    TreeBuilder::new(a.path()).file("in_a.txt", 1).unwrap();
    TreeBuilder::new(b.path()).file("in_b.txt", 2).unwrap();
    let db = dir.path().join("index.db");
    let mut store = Store::open(&db).unwrap();
    let (va, vb) = (volume_for(a.path()), volume_for(b.path()));

    store.sync_volumes(&[va.clone(), vb.clone()]).unwrap();
    let mut rebuild = store.begin_rebuild(&[va.clone(), vb.clone()]).unwrap();
    scan_into(&mut rebuild, &va);
    scan_into(&mut rebuild, &vb);
    rebuild.finish(&mut store).unwrap();
    assert_eq!(store.stats().unwrap().entries, 2);

    // `b` disappears; rebuilding only `a` must keep b's entries, marked offline.
    store.sync_volumes(std::slice::from_ref(&va)).unwrap();
    let mut rebuild = store.begin_rebuild(std::slice::from_ref(&va)).unwrap();
    scan_into(&mut rebuild, &va);
    rebuild.finish(&mut store).unwrap();

    let snap = store.load_snapshot().unwrap();
    check_invariants(&snap);
    assert_eq!(snap.len(), 2);
    let state = |uuid: &str| snap.volumes.iter().find(|v| v.fs_uuid == uuid).map(|v| v.state);
    assert_eq!(state(&va.fs_uuid), Some(VolumeState::Online));
    assert_eq!(state(&vb.fs_uuid), Some(VolumeState::Offline));
    assert!(listing(&snap, b.path()).contains_key("in_b.txt"));
}

#[test]
fn newer_schema_versions_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("index.db");
    drop(Store::open(&db).unwrap());
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute("UPDATE meta SET value = '99' WHERE key = 'schema_version'", [])
        .unwrap();
    drop(conn);
    match Store::open(&db) {
        Err(IndexError::SchemaTooNew { found: 99, supported }) => assert!(supported < 99),
        other => panic!("expected SchemaTooNew, got {other:?}"),
    }
}

#[test]
fn damaged_databases_are_reported_as_corrupt_and_can_be_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("index.db");

    std::fs::write(&db, b"this is definitely not an sqlite database, just text").unwrap();
    assert!(matches!(Store::open(&db), Err(IndexError::Corrupt { .. })));
    Store::delete_database(&db).unwrap();
    assert!(!db.exists());
    Store::open(&db).expect("a fresh database can be created after deleting");

    // A real index with a smashed page in the middle.
    let tree = tempfile::tempdir().unwrap();
    generate_tree(tree.path(), 2000, 50, 9).unwrap();
    {
        let mut store = Store::open(&db).unwrap();
        build(&mut store, tree.path());
    }
    let mut bytes = std::fs::read(&db).unwrap();
    assert!(bytes.len() > 40_000, "{}", bytes.len());
    let mid = bytes.len() / 2;
    bytes[mid..mid + 4096].fill(0xA5);
    std::fs::write(&db, &bytes).unwrap();
    assert!(matches!(Store::open(&db), Err(IndexError::Corrupt { .. })));
}

#[test]
fn snapshot_ids_are_dense_even_after_deletions() {
    let dir = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    let tb = TreeBuilder::new(tree.path());
    tb.file("x/1", 1)
        .unwrap()
        .file("y/2", 2)
        .unwrap()
        .file("z/3", 3)
        .unwrap();
    let mut store = Store::open(&dir.path().join("index.db")).unwrap();
    let volume = build(&mut store, tree.path());

    // Delete the middle folder through an update transaction: rowids now have a gap.
    let mut update = store.begin_update().unwrap();
    let vid = update.volume_id(&volume.fs_uuid).unwrap().unwrap();
    let y = update.find_dir(vid, &tree.path().join("y")).unwrap().unwrap();
    update.remove_dir(y.id).unwrap();
    let stats = update.commit().unwrap();
    assert_eq!(stats.removed, 2, "the folder entry and the file inside");

    let snap = store.load_snapshot().unwrap();
    check_invariants(&snap);
    assert_eq!(snap.dirs.len(), 3);
    assert_eq!(snap.len(), 4);
    assert_eq!(dir_total(&snap, tree.path()), Some(4));
}
