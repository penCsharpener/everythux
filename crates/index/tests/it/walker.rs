//! Walker behaviour on real directory trees.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use crossbeam_channel::bounded;
use everythux_core::config::VolumesConfig;
use everythux_core::exclusion::{ExclusionRule, ExclusionSet, RuleKind};
use everythux_core::model::EntryKind;
use everythux_index::test_utils::{TreeBuilder, generate_tree, raw_name};
use everythux_index::walker::{
    DirListing, IdAlloc, ListingKind, ScanBatch, WalkControl, WalkJob, WalkMode, WalkOptions, WalkSummary,
    dry_run, make_pool,
};

use crate::common::{is_root, raw};

struct Walked {
    listings: Vec<DirListing>,
    batches: usize,
    summary: WalkSummary,
}

impl Walked {
    /// `path -> listing` for folders.
    fn dirs(&self) -> BTreeMap<PathBuf, &DirListing> {
        self.listings
            .iter()
            .map(|l| (PathBuf::from(OsStr::from_bytes(&l.path)), l))
            .collect()
    }

    /// Names (lossy) of the entries of the listing for `dir`.
    fn names(&self, dir: &Path) -> Vec<String> {
        let dirs = self.dirs();
        let l = dirs
            .get(dir)
            .unwrap_or_else(|| panic!("no listing for {}", dir.display()));
        let mut names: Vec<_> = l
            .entries
            .iter()
            .map(|r| String::from_utf8_lossy(l.name(r)).into_owned())
            .collect();
        names.sort();
        names
    }

    fn entry_count(&self) -> usize {
        self.listings.iter().map(|l| l.entries.len()).sum()
    }
}

fn walk_with(root: &Path, opts: &WalkOptions, threads: usize) -> Walked {
    let pool = make_pool(threads);
    let ids = IdAlloc::new(1);
    let ctl = WalkControl::new();
    let mode = WalkMode::Fresh { existing_root: None };
    let job = WalkJob {
        root,
        mode: &mode,
        opts,
        ids: &ids,
        ctl: &ctl,
        pool: pool.as_ref(),
    };
    let (tx, rx) = bounded::<ScanBatch>(2);
    let mut listings = Vec::new();
    let mut batches = 0;
    std::thread::scope(|s| {
        let h = s.spawn(|| {
            let r = job.run(&tx);
            drop(tx);
            r
        });
        for batch in &rx {
            batches += 1;
            listings.extend(batch.listings);
        }
        h.join().expect("walker thread").expect("walk");
    });
    Walked {
        listings,
        batches,
        summary: ctl.counters.summary(),
    }
}

fn walk(root: &Path) -> Walked {
    walk_with(root, &WalkOptions::default(), 4)
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[test]
fn records_metadata_names_kinds_and_extensions() {
    let dir = tmp();
    let tb = TreeBuilder::new(dir.path());
    tb.dir("sub")
        .unwrap()
        .file("sub/Photo.JPG", 1234)
        .unwrap()
        .file("Makefile", 7)
        .unwrap();
    tb.symlink("link", "sub").unwrap();
    let w = walk(dir.path());

    let dirs = w.dirs();
    let root = dirs[dir.path()];
    assert_eq!(root.kind, ListingKind::Insert);
    assert_eq!(root.parent_id, None);
    let by_name: BTreeMap<String, _> = root
        .entries
        .iter()
        .map(|r| (String::from_utf8_lossy(root.name(r)).into_owned(), *r))
        .collect();
    assert_eq!(by_name["Makefile"].kind, EntryKind::File);
    assert_eq!(by_name["Makefile"].size, 7);
    assert_eq!(by_name["link"].kind, EntryKind::Symlink);
    assert_eq!(by_name["link"].child_dir, 0, "symlinks are never descended");
    assert_eq!(by_name["sub"].kind, EntryKind::Dir);
    assert_ne!(by_name["sub"].child_dir, 0);

    let meta = std::fs::symlink_metadata(dir.path().join("Makefile")).unwrap();
    assert_eq!(by_name["Makefile"].inode, meta.ino());
    assert_eq!(by_name["Makefile"].uid, meta.uid());
    assert_eq!(by_name["Makefile"].mode, meta.mode());
    assert_eq!(
        by_name["Makefile"].mtime_ns,
        meta.mtime() * 1_000_000_000 + meta.mtime_nsec()
    );

    let sub = dirs[&dir.path().join("sub")];
    assert_eq!(
        sub.id, by_name["sub"].child_dir,
        "pre-assigned id matches the parent's entry"
    );
    assert_eq!(sub.parent_id, Some(root.id));
    let photo = sub.entries[0];
    assert_eq!(sub.name(&photo), b"Photo.JPG");
    assert_eq!(sub.fold(&photo), b"photo.jpg");
    assert_eq!(sub.ext(&photo), b"jpg");
    assert_eq!(photo.size, 1234);
    assert_eq!(w.summary.entries, 4);
    assert_eq!(w.summary.listed, 2);
    assert_eq!(w.summary.errors, 0);
}

#[test]
fn invalid_utf8_names_survive_as_bytes() {
    let dir = tmp();
    let bad = raw_name(b"bad\xffname.TXT");
    let tb = TreeBuilder::new(dir.path());
    tb.file(&bad, 3)
        .unwrap()
        .dir(raw_name(b"d\xfe"))
        .unwrap()
        .file(Path::new(&raw_name(b"d\xfe")).join("in"), 1)
        .unwrap();
    let w = walk(dir.path());
    let root = w.dirs()[dir.path()];
    let names: Vec<&[u8]> = root.entries.iter().map(|r| root.name(r)).collect();
    assert!(names.contains(&&b"bad\xffname.TXT"[..]), "{names:?}");
    let r = root
        .entries
        .iter()
        .find(|r| root.name(r).starts_with(b"bad"))
        .unwrap();
    assert_eq!(root.fold(r), "bad\u{fffd}name.txt".as_bytes());
    assert_eq!(root.ext(r), b"txt");
    // The folder with an invalid name was entered, with its path as raw bytes.
    let inner = w
        .listings
        .iter()
        .find(|l| l.path.ends_with(b"/d\xfe"))
        .expect("listing of d\\xfe");
    assert_eq!(inner.entries.len(), 1);
    assert_eq!(raw(&inner.path), dir.path().join(raw_name(b"d\xfe")));
}

#[test]
fn deep_nesting_is_walked_completely() {
    let dir = tmp();
    // 300 levels of 10 bytes: far deeper than a recursive walker's stack would like.
    let mut rel = PathBuf::new();
    for i in 0..300 {
        rel.push(format!("level{i:04}"));
    }
    let tb = TreeBuilder::new(dir.path());
    tb.dir(&rel).unwrap().file(rel.join("leaf.txt"), 1).unwrap();
    for threads in [1, 4] {
        let w = walk_with(dir.path(), &WalkOptions::default(), threads);
        assert_eq!(w.summary.listed, 301, "threads={threads}");
        assert_eq!(w.entry_count(), 301);
        assert_eq!(w.summary.errors, 0);
    }
}

#[test]
fn symlink_loops_and_dangling_links_are_not_followed() {
    let dir = tmp();
    let tb = TreeBuilder::new(dir.path());
    tb.dir("a/b").unwrap();
    tb.symlink("loop", ".").unwrap();
    tb.symlink("a/up", "..").unwrap();
    tb.symlink("a/b/abs", dir.path()).unwrap();
    tb.symlink("dangling", "/nonexistent/target").unwrap();
    tb.symlink("a/self", "self").unwrap();
    let w = walk(dir.path());
    assert_eq!(w.summary.listed, 3, "root, a, a/b only");
    assert_eq!(w.summary.errors, 0);
    let root = w.dirs()[dir.path()];
    let kinds: Vec<_> = root
        .entries
        .iter()
        .map(|r| (root.name(r).to_vec(), r.kind))
        .collect();
    assert!(kinds.contains(&(b"loop".to_vec(), EntryKind::Symlink)));
    assert!(kinds.contains(&(b"dangling".to_vec(), EntryKind::Symlink)));
}

/// Creates `levels` nested folders named `level_NNNN` below `base` using only descriptor-relative
/// calls, so the result can be deeper than `PATH_MAX` (4096 bytes) allows for path-based calls.
fn make_very_deep_tree(base: &Path, levels: usize) {
    use rustix::fs::{CWD, Mode, OFlags, mkdirat, openat};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    let mut fd = openat(CWD, base, flags, Mode::empty()).expect("open base");
    for i in 0..levels {
        let name = format!("level_{i:04}");
        mkdirat(&fd, name.as_str(), Mode::RWXU).expect("mkdirat");
        fd = openat(&fd, name.as_str(), flags, Mode::empty()).expect("openat");
    }
}

/// Removes what [`make_very_deep_tree`] created, deepest folder first (one descriptor at a time).
fn remove_very_deep_tree(base: &Path, levels: usize) {
    use rustix::fs::{AtFlags, CWD, Mode, OFlags, openat, unlinkat};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    for depth in (0..levels).rev() {
        let mut fd = openat(CWD, base, flags, Mode::empty()).expect("open base");
        for i in 0..depth {
            fd = openat(&fd, format!("level_{i:04}").as_str(), flags, Mode::empty()).expect("openat");
        }
        unlinkat(&fd, format!("level_{depth:04}").as_str(), AtFlags::REMOVEDIR).expect("rmdir");
    }
}

#[test]
fn folders_deeper_than_path_max_are_an_error_not_a_crash() {
    // The walker opens folders by absolute path, so it cannot enter folders whose path exceeds
    // PATH_MAX. That is counted like any unreadable folder; the rest of the tree is unaffected.
    let dir = tmp();
    TreeBuilder::new(dir.path()).file("fine/file.txt", 1).unwrap();
    let levels = 450; // 450 * 11 bytes > 4096
    make_very_deep_tree(dir.path(), levels);
    let w = walk(dir.path());
    remove_very_deep_tree(dir.path(), levels);

    assert!(w.summary.errors >= 1, "{:?}", w.summary);
    assert_eq!(w.names(&dir.path().join("fine")), ["file.txt"]);
    assert!(
        w.summary.listed >= 2 + 300,
        "the first few hundred levels are still listed: {:?}",
        w.summary
    );
}

#[test]
fn long_names_are_kept() {
    let dir = tmp();
    let long = "x".repeat(255);
    TreeBuilder::new(dir.path()).file(&long, 1).unwrap();
    let w = walk(dir.path());
    assert_eq!(w.names(dir.path()), [long]);
}

#[test]
fn excluded_folders_are_pruned_before_reading() {
    let dir = tmp();
    let tb = TreeBuilder::new(dir.path());
    tb.dir("src").unwrap().file("src/main.rs", 1).unwrap();
    for i in 0..50 {
        tb.dir(format!("proj/node_modules/pkg{i}/deep")).unwrap();
        tb.file(format!("proj/node_modules/pkg{i}/deep/x.js"), 1).unwrap();
    }
    tb.file("proj/keep.txt", 1)
        .unwrap()
        .file("proj/scratch.tmp", 1)
        .unwrap();
    tb.dir("old/stuff").unwrap().file("old/stuff/f", 1).unwrap();
    let opts = WalkOptions {
        exclusions: ExclusionSet::compile(&[
            ExclusionRule::new("node_modules", RuleKind::Dir),
            ExclusionRule::new("*.tmp", RuleKind::File),
            ExclusionRule::new(dir.path().join("old").to_str().unwrap(), RuleKind::Path),
        ])
        .unwrap(),
        ..WalkOptions::default()
    };
    // Make the excluded folder unreadable where that is observable: pruning means no error.
    if !is_root() {
        std::fs::set_permissions(
            dir.path().join("proj/node_modules"),
            std::os::unix::fs::PermissionsExt::from_mode(0o000),
        )
        .unwrap();
    }
    let w = walk_with(dir.path(), &opts, 4);
    if !is_root() {
        std::fs::set_permissions(
            dir.path().join("proj/node_modules"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
    }

    assert_eq!(w.summary.errors, 0, "an excluded folder is never opened");
    assert_eq!(w.names(&dir.path().join("proj")), ["keep.txt"]);
    assert_eq!(
        w.names(dir.path()),
        ["proj", "src"],
        "the excluded path rule drops `old`"
    );
    // root, src, proj: nothing from node_modules or old was visited at all.
    assert_eq!(w.summary.dirs, 3);
    assert_eq!(w.summary.listed, 3);
    assert_eq!(w.entry_count(), 2 + 1 + 1);
}

#[test]
fn unreadable_folders_are_counted_not_fatal() {
    if is_root() {
        eprintln!("skipped: root can read everything");
        return;
    }
    let dir = tmp();
    let tb = TreeBuilder::new(dir.path());
    tb.dir("open")
        .unwrap()
        .file("open/a", 1)
        .unwrap()
        .dir("locked/inner")
        .unwrap();
    let locked = dir.path().join("locked");
    std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    let w = walk(dir.path());
    std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    assert_eq!(w.summary.errors, 1);
    assert_eq!(w.names(&dir.path().join("open")), ["a"]);
    assert_eq!(
        w.names(&locked),
        Vec::<String>::new(),
        "the locked folder is recorded as empty"
    );
}

#[test]
fn many_entries_are_split_into_bounded_batches() {
    let dir = tmp();
    generate_tree(dir.path(), 3000, 60, 1).unwrap();
    let opts = WalkOptions {
        batch_rows: 200,
        ..WalkOptions::default()
    };
    let w = walk_with(dir.path(), &opts, 4);
    assert_eq!(w.summary.entries, 3060);
    assert!(w.batches >= 3060 / 400, "got {} batches", w.batches);
    let ids: std::collections::HashSet<_> = w.listings.iter().map(|l| l.id).collect();
    assert_eq!(ids.len(), w.listings.len(), "every folder gets a unique id");
}

#[test]
fn single_and_multi_threaded_walks_agree() {
    let dir = tmp();
    generate_tree(dir.path(), 2000, 150, 42).unwrap();
    let summarize = |threads| {
        let w = walk_with(dir.path(), &WalkOptions::default(), threads);
        let mut v: Vec<_> = w
            .listings
            .iter()
            .flat_map(|l| {
                l.entries.iter().map(|r| {
                    let mut p = l.path.clone();
                    p.push(b'/');
                    p.extend_from_slice(l.name(r));
                    (p, r.kind, r.size)
                })
            })
            .collect();
        v.sort();
        v
    };
    let one = summarize(1);
    assert_eq!(one.len(), 2150);
    assert_eq!(one, summarize(8));
}

#[test]
fn cancelled_walks_stop_early() {
    let dir = tmp();
    generate_tree(dir.path(), 500, 50, 3).unwrap();
    let ctl = WalkControl::new();
    ctl.cancel.store(true, Ordering::Relaxed);
    let summary = {
        let mode = WalkMode::Fresh { existing_root: None };
        let ids = IdAlloc::new(1);
        let opts = WalkOptions::default();
        let job = WalkJob {
            root: dir.path(),
            mode: &mode,
            opts: &opts,
            ids: &ids,
            ctl: &ctl,
            pool: None,
        };
        let (tx, rx) = bounded(1);
        job.run(&tx).unwrap();
        drop(tx);
        assert_eq!(rx.iter().count(), 0, "nothing is emitted after cancellation");
        ctl.counters.summary()
    };
    assert_eq!(summary.listed, 0);
}

#[test]
fn root_must_be_a_folder() {
    let dir = tmp();
    TreeBuilder::new(dir.path()).file("f", 1).unwrap();
    let ctl = WalkControl::new();
    assert!(dry_run(&dir.path().join("f"), &WalkOptions::default(), &ctl, 1).is_err());
    assert!(dry_run(&dir.path().join("missing"), &WalkOptions::default(), &ctl, 1).is_err());
    let ok = dry_run(dir.path(), &WalkOptions::default(), &ctl, 2).unwrap();
    assert_eq!((ok.listed, ok.entries), (1, 1));
}

#[test]
fn discovery_include_roots_do_not_touch_the_real_disk() {
    let dir = tmp();
    let found = everythux_index::discover_volumes(&VolumesConfig {
        auto_discover_fs_types: vec![],
        include: vec![dir.path().to_path_buf()],
        disabled_uuids: vec![],
    })
    .unwrap();
    assert_eq!(found.len(), 1);
}
