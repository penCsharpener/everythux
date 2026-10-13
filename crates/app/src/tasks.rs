//! Blocking computations run on the compute worker (and, in tests, inline).

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use everythux_core::exclusion::{ExclusionRule, ExclusionSet};
use everythux_core::ids::DirId;
use everythux_core::snapshot::IndexSnapshot;

use crate::backend::{BackendEvent, Compute, ExclusionPreview};
use crate::model::{FsMeta, PropTarget, PropertiesInfo};

/// Executes a task and produces the event that reports its result.
pub(crate) fn run(task: Compute) -> BackendEvent {
    match task {
        Compute::ExclusionPreview {
            request,
            snapshot,
            rules,
        } => BackendEvent::ExclusionPreview(exclusion_preview(request, &snapshot, &rules)),
        Compute::Properties { id, snapshot, target } => BackendEvent::Properties {
            id,
            info: properties(&snapshot, &target),
        },
        Compute::ImportRules(path) => BackendEvent::RulesImported(import_rules(&path)),
        Compute::ExportRules { path, rules } => BackendEvent::RulesExported(export_rules(&path, &rules)),
    }
}

/// How many indexed entries (and folders) `rules` would exclude.
///
/// A folder is excluded if it matches itself or any ancestor is excluded; ancestry is resolved
/// through `dirs.parent` with memoisation, so the order of folders in the snapshot is irrelevant.
pub(crate) fn exclusion_preview(
    request: u64,
    snapshot: &IndexSnapshot,
    rules: &[ExclusionRule],
) -> ExclusionPreview {
    let set = match ExclusionSet::compile(rules) {
        Ok(set) => set,
        Err(e) => {
            return ExclusionPreview {
                request,
                entries: 0,
                folders: 0,
                error: Some(e.to_string()),
            };
        }
    };
    let dirs = &snapshot.dirs;
    let n = dirs.len();
    // 0 = unknown, 1 = kept, 2 = excluded
    let mut state = vec![0_u8; n];
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        stack.clear();
        let mut inherited = false;
        let mut d = start;
        loop {
            match state[d] {
                1 => break,
                2 => {
                    inherited = true;
                    break;
                }
                _ => {}
            }
            stack.push(d);
            let parent = dirs.parent[d];
            if parent.is_none() || parent.index() >= n || stack.len() > n {
                break;
            }
            d = parent.index();
        }
        while let Some(d) = stack.pop() {
            let excluded = inherited || set.is_excluded(dirs.path(DirId(d as u32)), true);
            state[d] = if excluded { 2 } else { 1 };
            inherited = excluded;
        }
    }

    let e = &snapshot.entries;
    let mut entries = 0_u64;
    let mut affected: Vec<bool> = state.iter().map(|&s| s == 2).collect();
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    for i in 0..e.len() {
        let dir = e.dir[i].index();
        if state[dir] == 2 {
            entries += 1;
            continue;
        }
        if e.kind[i].is_dir() {
            let own = e.self_dir[i];
            if !own.is_none() && state[own.index()] == 2 {
                entries += 1;
            }
            continue;
        }
        buf.clear();
        buf.extend_from_slice(dirs.paths.get(dir));
        if buf.last() != Some(&b'/') {
            buf.push(b'/');
        }
        buf.extend_from_slice(e.names.get(i));
        if set.is_excluded(Path::new(std::ffi::OsStr::from_bytes(&buf)), false) {
            entries += 1;
            affected[dir] = true;
        }
    }
    ExclusionPreview {
        request,
        entries,
        folders: affected.iter().filter(|&&a| a).count() as u64,
        error: None,
    }
}

/// Files and folders below `path` according to the snapshot (the folder itself not counted).
pub(crate) fn count_below(snapshot: &IndexSnapshot, path: &Path) -> (u64, u64) {
    let target = path.as_os_str().as_bytes();
    let dirs = &snapshot.dirs;
    let mut prefix = target.to_vec();
    if prefix.last() != Some(&b'/') {
        prefix.push(b'/');
    }
    let inside: Vec<bool> = (0..dirs.len())
        .map(|d| {
            let p = dirs.paths.get(d);
            p == target || p.starts_with(&prefix)
        })
        .collect();
    let e = &snapshot.entries;
    let (mut files, mut folders) = (0, 0);
    for i in 0..e.len() {
        if inside[e.dir[i].index()] {
            if e.kind[i].is_dir() {
                folders += 1;
            } else {
                files += 1;
            }
        }
    }
    (files, folders)
}

fn systime_ns(secs: i64, nanos: i64) -> i64 {
    secs.saturating_mul(1_000_000_000).saturating_add(nanos)
}

/// Reads owner, permissions and timestamps without following symlinks.
pub(crate) fn read_fs_meta(path: &Path) -> Result<FsMeta, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let created_ns = meta.created().ok().and_then(|t| {
        let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
        i64::try_from(d.as_nanos()).ok()
    });
    Ok(FsMeta {
        uid: meta.uid(),
        gid: meta.gid(),
        mode: meta.mode(),
        modified_ns: systime_ns(meta.mtime(), meta.mtime_nsec()),
        accessed_ns: systime_ns(meta.atime(), meta.atime_nsec()),
        changed_ns: systime_ns(meta.ctime(), meta.ctime_nsec()),
        created_ns,
    })
}

fn properties(snapshot: &IndexSnapshot, target: &PropTarget) -> PropertiesInfo {
    let (files, folders) = if target.kind.is_dir() {
        count_below(snapshot, &target.path)
    } else {
        (0, 0)
    };
    PropertiesInfo {
        files,
        folders,
        meta: read_fs_meta(&target.path),
    }
}

/// Reads rules from a JSON file: either an array of rules or a full config with `exclusions`.
pub(crate) fn import_rules(path: &Path) -> Result<Vec<ExclusionRule>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let rules = match value {
        serde_json::Value::Object(mut obj) => obj
            .remove("exclusions")
            .ok_or_else(|| format!("{}: no \"exclusions\" array", path.display()))?,
        other => other,
    };
    serde_json::from_value(rules).map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes rules as pretty JSON.
pub(crate) fn export_rules(path: &Path, rules: &[ExclusionRule]) -> Result<(PathBuf, usize), String> {
    let text = serde_json::to_string_pretty(rules).map_err(|e| e.to_string())?;
    std::fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((path.to_owned(), rules.len()))
}

#[cfg(test)]
mod tests {
    use everythux_core::exclusion::RuleKind;

    use super::*;
    use crate::testkit::Fixture;

    #[test]
    fn dir_rule_counts_the_whole_subtree() {
        let f = Fixture::new();
        let p = exclusion_preview(
            1,
            &f.snapshot,
            &[ExclusionRule::new("node_modules", RuleKind::Dir)],
        );
        // folder entry itself + junk.js + more.js
        assert_eq!(p.entries, 3, "{p:?}");
        assert_eq!(p.folders, 1);
        assert_eq!(p.error, None);
        assert_eq!(p.request, 1);
    }

    #[test]
    fn path_rule_covers_descendants() {
        let f = Fixture::new();
        let p = exclusion_preview(
            2,
            &f.snapshot,
            &[ExclusionRule::new("/home/me/proj", RuleKind::Path)],
        );
        // proj itself, node_modules (+2 files) and bin (+1 file)
        assert_eq!(p.entries, 1 + 3 + 2, "{p:?}");
        assert_eq!(p.folders, 3, "proj, node_modules, bin");
    }

    #[test]
    fn scope_protects_system_folders() {
        let f = Fixture::new();
        let scoped = ExclusionRule::new("bin", RuleKind::Dir).scoped("/home/*");
        let p = exclusion_preview(3, &f.snapshot, &[scoped]);
        assert_eq!(p.entries, 2, "bin entry + tool, not /usr/bin: {p:?}");
        let global = exclusion_preview(3, &f.snapshot, &[ExclusionRule::new("bin", RuleKind::Dir)]);
        assert_eq!(
            global.entries, 3,
            "/usr/bin has no entry for itself (volume root) but holds ls"
        );
        assert_eq!(global.folders, 2);
    }

    #[test]
    fn file_rule_counts_matching_files_only() {
        let f = Fixture::new();
        let p = exclusion_preview(4, &f.snapshot, &[ExclusionRule::new("*.js", RuleKind::File)]);
        assert_eq!(p.entries, 2);
        assert_eq!(p.folders, 1);
    }

    #[test]
    fn invalid_rules_report_an_error() {
        let f = Fixture::new();
        let p = exclusion_preview(5, &f.snapshot, &[ExclusionRule::new("[", RuleKind::Any)]);
        assert!(p.error.is_some());
    }

    #[test]
    fn disabled_rules_match_nothing() {
        let f = Fixture::new();
        let mut rule = ExclusionRule::new("node_modules", RuleKind::Dir);
        rule.enabled = false;
        assert_eq!(exclusion_preview(6, &f.snapshot, &[rule]).entries, 0);
    }

    #[test]
    fn parent_after_child_order_is_handled() {
        // Build a snapshot where a child folder is stored before its parent.
        let mut b = crate::testkit::SnapshotBuilder::new();
        let root = b.dir("/data", None);
        let child = b.dir("/data/skip/deep", Some(root));
        b.file(child, "x.txt", 1, 0);
        let mut snap = (*b.finish()).clone();
        let skip = DirId(snap.dirs.len() as u32);
        snap.dirs.paths.push(b"/data/skip");
        snap.dirs.parent.push(root);
        snap.dirs.volume.push(everythux_core::ids::VolumeId(0));
        snap.dirs.total_size.push(0);
        snap.dirs.mtime_ns.push(0);
        snap.dirs.entry.push(everythux_core::ids::EntryId::NONE);
        snap.dirs.parent[child.index()] = skip;
        let p = exclusion_preview(7, &snap, &[ExclusionRule::new("skip", RuleKind::Dir)]);
        // x.txt, plus the folder entry of `deep` (its own folder inherits the exclusion)
        assert_eq!((p.entries, p.folders), (2, 2), "{p:?}");
    }

    #[test]
    fn counts_below_a_folder() {
        let f = Fixture::new();
        assert_eq!(count_below(&f.snapshot, Path::new("/home/me/proj")), (3, 2));
        assert_eq!(count_below(&f.snapshot, Path::new("/home/me/docs")), (2, 0));
        assert_eq!(count_below(&f.snapshot, Path::new("/nowhere")), (0, 0));
        assert_eq!(
            count_below(&f.snapshot, Path::new("/home/me/pro")),
            (0, 0),
            "prefix match must respect path separators"
        );
    }

    #[test]
    fn fs_meta_reads_a_real_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"hi").expect("write");
        let meta = read_fs_meta(&file).expect("meta");
        assert_eq!(meta.mode & 0o170_000, 0o100_000, "regular file");
        assert!(meta.modified_ns > 0);
        assert!(read_fs_meta(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn rules_roundtrip_through_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rules.json");
        let rules = vec![
            ExclusionRule::new("node_modules", RuleKind::Dir),
            ExclusionRule::new("bin", RuleKind::Dir).scoped("/home/*"),
        ];
        assert_eq!(export_rules(&path, &rules), Ok((path.clone(), 2)));
        assert_eq!(import_rules(&path), Ok(rules));
        assert!(import_rules(&dir.path().join("nope.json")).is_err());
    }

    #[test]
    fn rules_import_accepts_a_whole_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"exclusions":[{"pattern":"obj","kind":"dir"}]}"#).expect("write");
        let rules = import_rules(&path).expect("import");
        assert_eq!(rules, vec![ExclusionRule::new("obj", RuleKind::Dir)]);
    }
}
