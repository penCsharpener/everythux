//! Real ext4: discovery through `/proc/self/mountinfo` and the one-filesystem rule.
//!
//! Needs root (loop mounts): `cargo test -p everythux-index -- --ignored`.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use everythux_core::config::VolumesConfig;
use everythux_core::model::EntryKind;
use everythux_index::test_utils::TreeBuilder;
use everythux_index::walker::{WalkControl, WalkOptions, dry_run};
use everythux_index::{DiscoveredVolume, discover_volumes};

use crate::common::{check_invariants, listing, settings, start_scanned};

fn sh(program: &str, args: &[&str]) {
    let out = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {program}: {e}"));
    assert!(
        out.status.success(),
        "{program} {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Mounts made by a test; unmounted in reverse order on drop (lazily, so a busy mount cannot
/// wedge the test run), also when the test panics.
#[derive(Default)]
struct Mounts(Vec<PathBuf>);

impl Mounts {
    fn mount(&mut self, args: &[&str], target: &Path) {
        let mut all: Vec<&str> = args.to_vec();
        all.push(target.to_str().expect("utf-8 path"));
        sh("mount", &all);
        self.0.push(target.to_path_buf());
    }
}

impl Drop for Mounts {
    fn drop(&mut self) {
        while let Some(target) = self.0.pop() {
            let _ = Command::new("umount").arg("-l").arg(target).status();
        }
    }
}

fn auto_ext4() -> VolumesConfig {
    VolumesConfig::default()
}

fn find<'a>(volumes: &'a [DiscoveredVolume], mount_point: &Path) -> Option<&'a DiscoveredVolume> {
    volumes.iter().find(|v| v.mount_point == mount_point)
}

/// A loop-mounted ext4 image with a nested tmpfs, a bind mount of a sub-folder and a bind mount
/// of a single file.
struct Fixture {
    mnt: PathBuf,
    bind: PathBuf,
    file_bind: PathBuf,
    // Fields drop in declaration order: unmount first, then delete the directory.
    _mounts: Mounts,
    _work: tempfile::TempDir,
}

fn setup() -> Fixture {
    assert!(crate::common::is_root(), "this test must run as root");
    let work = tempfile::tempdir().expect("tempdir");
    let base = work.path().canonicalize().expect("canonicalize");
    let image = base.join("disk.img");
    sh("truncate", &["-s", "64M", image.to_str().expect("utf-8")]);
    sh("mkfs.ext4", &["-q", image.to_str().expect("utf-8")]);
    let (mnt, bind, file_bind) = (base.join("mnt"), base.join("bind"), base.join("hosts"));
    std::fs::create_dir_all(&mnt).expect("mkdir");
    std::fs::create_dir_all(&bind).expect("mkdir");
    std::fs::write(&file_bind, b"").expect("touch");

    let mut mounts = Mounts::default();
    mounts.mount(&["-o", "loop", image.to_str().expect("utf-8")], &mnt);
    let tb = TreeBuilder::new(&mnt);
    tb.file("docs/report.pdf", 1000)
        .and_then(|t| t.file("docs/sub/notes.txt", 30))
        .and_then(|t| t.dir("nested"))
        .expect("tree");
    // A tmpfs mounted *inside* the ext4 filesystem, with content that must not be indexed.
    mounts.mount(&["-t", "tmpfs", "tmpfs"], &mnt.join("nested"));
    tb.file("nested/other_fs.txt", 5).expect("file");
    // A bind mount of a sub-folder of the same filesystem, and of a single file.
    mounts.mount(&["--bind", mnt.join("docs").to_str().expect("utf-8")], &bind);
    mounts.mount(
        &["--bind", mnt.join("docs/report.pdf").to_str().expect("utf-8")],
        &file_bind,
    );
    Fixture {
        mnt,
        bind,
        file_bind,
        _mounts: mounts,
        _work: work,
    }
}

fn check_discovery(f: &Fixture) -> DiscoveredVolume {
    let volumes = discover_volumes(&auto_ext4()).expect("discover");
    let vol = find(&volumes, &f.mnt)
        .unwrap_or_else(|| panic!("loop mount not discovered: {volumes:#?}"))
        .clone();
    assert_eq!(vol.fs_type, "ext4");
    assert_eq!(vol.dev, std::fs::metadata(&f.mnt).expect("stat").dev());
    assert!(
        vol.device.to_string_lossy().starts_with("/dev/loop"),
        "{}",
        vol.device.display()
    );
    assert!(
        find(&volumes, &f.bind).is_none(),
        "bind mount of the same filesystem is deduplicated"
    );
    assert!(find(&volumes, &f.file_bind).is_none(), "a file is not a volume");
    assert_eq!(
        volumes.iter().filter(|v| v.dev == vol.dev).count(),
        1,
        "one filesystem, one volume"
    );
    assert!(
        find(&volumes, &f.mnt.join("nested")).is_none(),
        "tmpfs is not in auto_discover_fs_types"
    );
    if Path::new("/dev/disk/by-uuid").exists() {
        assert!(
            !vol.fs_uuid.starts_with("dev:"),
            "UUID resolved through by-uuid: {}",
            vol.fs_uuid
        );
    } else {
        let (major, minor) = (rustix::fs::major(vol.dev), rustix::fs::minor(vol.dev));
        assert_eq!(vol.fs_uuid, format!("dev:{major}:{minor}"));
    }

    let disabled = VolumesConfig {
        disabled_uuids: vec![vol.fs_uuid.clone()],
        ..auto_ext4()
    };
    let remaining = discover_volumes(&disabled).expect("discover");
    assert!(find(&remaining, &f.mnt).is_none(), "disabled uuids are dropped");
    vol
}

fn check_walker(f: &Fixture) {
    let summary = dry_run(&f.mnt, &WalkOptions::default(), &WalkControl::new(), 2).expect("walk");
    // root, docs, docs/sub, lost+found; `nested` (the mount point) is recorded but not entered.
    assert_eq!(summary.listed, 4, "{summary:?}");
    assert_eq!(summary.errors, 0);
    let opts = WalkOptions {
        one_filesystem: false,
        ..WalkOptions::default()
    };
    let crossing = dry_run(&f.mnt, &opts, &WalkControl::new(), 2).expect("walk");
    assert_eq!(
        crossing.listed, 5,
        "tmpfs content is found when crossing is allowed: {crossing:?}"
    );
    assert_eq!(crossing.entries, summary.entries + 1);
}

fn check_indexer(f: &Fixture, vol: &DiscoveredVolume) {
    let db = tempfile::tempdir().expect("tempdir");
    let (indexer, _rx) = start_scanned(db.path(), settings(&[&f.mnt]));
    let snap = indexer.snapshot();
    check_invariants(&snap);
    let rows = listing(&snap, &f.mnt);
    assert_eq!(rows["docs/sub/notes.txt"].size, 30);
    assert_eq!(
        rows["nested"].kind,
        EntryKind::Dir,
        "the mount point itself is an entry"
    );
    assert!(
        !rows.contains_key("nested/other_fs.txt"),
        "other filesystems are not indexed"
    );
    assert_eq!(snap.volumes.len(), 1);
    assert_eq!(
        snap.volumes[0].fs_uuid, vol.fs_uuid,
        "include of a mount root reuses the volume identity"
    );
    assert_eq!(snap.volumes[0].fs_type, "ext4");
}

#[test]
#[ignore = "needs root: loop-mount ext4"]
fn discovery_dedupes_binds_and_the_walker_stays_on_one_filesystem() {
    let fixture = setup();
    let volume = check_discovery(&fixture);
    check_walker(&fixture);
    check_indexer(&fixture, &volume);
}
