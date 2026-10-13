//! Finds the volumes to index by parsing `/proc/self/mountinfo` (ROADMAP Phase 1).
//!
//! The pipeline is: parse the mount table, keep mounts whose filesystem type is configured,
//! drop mount points that aren't directories (containers bind-mount single files such as
//! `/etc/hosts` from ext4), deduplicate bind mounts per filesystem, then attach a stable identity
//! (filesystem UUID, label) to every survivor.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use everythux_core::config::VolumesConfig;

use crate::IndexError;

/// A mounted filesystem selected for indexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredVolume {
    /// Filesystem UUID (`/dev/disk/by-uuid`), or `dev:<major>:<minor>` if none.
    pub fs_uuid: String,
    /// Block device (mount source), e.g. `/dev/nvme0n1p2`.
    pub device: PathBuf,
    /// Shortest mount point of this filesystem root (bind mounts are deduplicated).
    pub mount_point: PathBuf,
    /// `st_dev` of the mount point, used to stay on one filesystem.
    pub dev: u64,
    /// Filesystem type, e.g. `ext4`.
    pub fs_type: String,
    /// Label from `/dev/disk/by-label`, if any.
    pub label: Option<String>,
}

/// One line of `/proc/self/mountinfo`, see `proc_pid_mountinfo(5)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountInfoLine {
    /// Unique mount id.
    pub mount_id: u32,
    /// Id of the parent mount.
    pub parent_id: u32,
    /// Device major number (`st_dev` of files on this filesystem).
    pub major: u32,
    /// Device minor number.
    pub minor: u32,
    /// Root of the mount within its filesystem (`/` unless this is a bind mount of a subtree).
    pub root: PathBuf,
    /// Where it is mounted.
    pub mount_point: PathBuf,
    /// Per-mount options, e.g. `rw,relatime`.
    pub mount_options: String,
    /// Optional fields such as `shared:1` (everything between the options and the ` - ` separator).
    pub optional_fields: Vec<String>,
    /// Filesystem type, e.g. `ext4`.
    pub fs_type: String,
    /// Mount source, e.g. `/dev/nvme0n1p2`.
    pub source: String,
    /// Per-superblock options.
    pub super_options: String,
}

/// Decodes the `\040`-style octal escapes the kernel uses for spaces, tabs, newlines and
/// backslashes in mountinfo paths.
fn unescape_octal(field: &str) -> OsString {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && let Some(v) = octal3(&bytes[i + 1..i + 4])
        {
            out.push(v);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    OsString::from_vec(out)
}

fn octal3(digits: &[u8]) -> Option<u8> {
    let mut v: u16 = 0;
    for &d in digits {
        if !(b'0'..=b'7').contains(&d) {
            return None;
        }
        v = v * 8 + u16::from(d - b'0');
    }
    u8::try_from(v).ok()
}

/// Parses the text of `/proc/self/mountinfo`. Malformed lines are skipped.
pub fn parse_mountinfo(text: &str) -> Vec<MountInfoLine> {
    text.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<MountInfoLine> {
    let mut it = line.split(' ');
    let mount_id = it.next()?.parse().ok()?;
    let parent_id = it.next()?.parse().ok()?;
    let (major, minor) = it.next()?.split_once(':')?;
    let root = unescape_octal(it.next()?);
    let mount_point = unescape_octal(it.next()?);
    let mount_options = it.next()?.to_owned();
    let mut optional_fields = Vec::new();
    loop {
        let field = it.next()?;
        if field == "-" {
            break;
        }
        optional_fields.push(field.to_owned());
    }
    let fs_type = it.next()?.to_owned();
    let source = unescape_octal(it.next()?).to_string_lossy().into_owned();
    let super_options = it.next().unwrap_or_default().to_owned();
    Some(MountInfoLine {
        mount_id,
        parent_id,
        major: major.parse().ok()?,
        minor: minor.parse().ok()?,
        root: PathBuf::from(root),
        mount_point: PathBuf::from(mount_point),
        mount_options,
        optional_fields,
        fs_type,
        source,
        super_options,
    })
}

/// Picks the mounts to index: matching filesystem type, mount point accepted by `is_dir`, one per
/// filesystem (`major:minor`). Among the mounts of one filesystem the one with the shortest
/// filesystem root wins (it covers the most), then the shortest mount point.
fn select_mounts<'a>(
    lines: &'a [MountInfoLine],
    fs_types: &[String],
    is_dir: impl Fn(&Path) -> bool,
) -> Vec<&'a MountInfoLine> {
    let mut best: HashMap<(u32, u32), &MountInfoLine> = HashMap::new();
    for line in lines {
        if !fs_types.contains(&line.fs_type) || !is_dir(&line.mount_point) {
            continue;
        }
        let key = (line.major, line.minor);
        let rank = |l: &MountInfoLine| (l.root.as_os_str().len(), l.mount_point.as_os_str().len());
        match best.get(&key) {
            Some(current) if rank(current) <= rank(line) => {}
            _ => {
                best.insert(key, line);
            }
        }
    }
    let mut selected: Vec<_> = best.into_values().collect();
    selected.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    selected
}

/// UUIDs and labels of block devices, keyed by `(major, minor)` of the device node.
#[derive(Debug, Default)]
pub(crate) struct DiskIds {
    uuids: HashMap<(u32, u32), String>,
    labels: HashMap<(u32, u32), String>,
}

impl DiskIds {
    /// Reads `/dev/disk/by-uuid` and `/dev/disk/by-label`. Both directories are matched by the
    /// `st_rdev` of what the symlinks point to, which is more robust than comparing device names
    /// (`/dev/mapper/x` vs `/dev/dm-0`).
    pub(crate) fn scan() -> Self {
        Self {
            uuids: scan_symlink_dir(Path::new("/dev/disk/by-uuid"), false),
            labels: scan_symlink_dir(Path::new("/dev/disk/by-label"), true),
        }
    }

    fn uuid(&self, major: u32, minor: u32) -> Option<&str> {
        self.uuids.get(&(major, minor)).map(String::as_str)
    }

    fn label(&self, major: u32, minor: u32) -> Option<&str> {
        self.labels.get(&(major, minor)).map(String::as_str)
    }
}

fn scan_symlink_dir(dir: &Path, udev_unescape: bool) -> HashMap<(u32, u32), String> {
    let mut map = HashMap::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return map;
    };
    for entry in read.flatten() {
        // `metadata` follows the symlink to the device node.
        let Ok(meta) = std::fs::metadata(entry.path()) else {
            continue;
        };
        let rdev = meta.rdev();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let name = if udev_unescape {
            unescape_udev(&name)
        } else {
            name.into_owned()
        };
        map.insert((rustix::fs::major(rdev), rustix::fs::minor(rdev)), name);
    }
    map
}

/// Decodes udev's `\xNN` escapes (used for spaces etc. in `/dev/disk/by-label` names).
fn unescape_udev(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && bytes.get(i + 1) == Some(&b'x')
            && let Some(hex) = name.get(i + 2..i + 4)
            && let Ok(v) = u8::from_str_radix(hex, 16)
        {
            out.push(v);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> IndexError + '_ {
    move |source| IndexError::io(path, source)
}

/// Discovers mounted volumes whose type is in `config.auto_discover_fs_types`, plus
/// `config.include` roots, minus `config.disabled_uuids`.
pub fn discover_volumes(config: &VolumesConfig) -> Result<Vec<DiscoveredVolume>, IndexError> {
    let path = Path::new("/proc/self/mountinfo");
    let raw = std::fs::read(path).map_err(io_err(path))?;
    let text = String::from_utf8_lossy(&raw);
    Ok(discover_from(config, &parse_mountinfo(&text), &DiskIds::scan()))
}

fn discover_from(config: &VolumesConfig, lines: &[MountInfoLine], ids: &DiskIds) -> Vec<DiscoveredVolume> {
    let mut volumes: Vec<DiscoveredVolume> = Vec::new();

    let is_dir = |p: &Path| std::fs::metadata(p).is_ok_and(|m| m.is_dir());
    for line in select_mounts(lines, &config.auto_discover_fs_types, is_dir) {
        let Ok(meta) = std::fs::metadata(&line.mount_point) else {
            continue;
        };
        volumes.push(DiscoveredVolume {
            fs_uuid: uuid_for(line, ids),
            device: PathBuf::from(&line.source),
            mount_point: line.mount_point.clone(),
            dev: meta.dev(),
            fs_type: line.fs_type.clone(),
            label: ids.label(line.major, line.minor).map(str::to_owned),
        });
    }

    for root in &config.include {
        if let Some(volume) = include_root(root, lines, ids) {
            let covered = volumes
                .iter()
                .any(|v| v.dev == volume.dev && volume.mount_point.starts_with(&v.mount_point));
            if covered {
                tracing::debug!(root = %root.display(), "include root already covered by a discovered volume");
            } else {
                volumes.push(volume);
            }
        } else {
            tracing::warn!(root = %root.display(), "include root is not a readable directory");
        }
    }

    volumes.retain(|v| !config.disabled_uuids.contains(&v.fs_uuid));
    volumes
}

fn uuid_for(line: &MountInfoLine, ids: &DiskIds) -> String {
    ids.uuid(line.major, line.minor)
        .map_or_else(|| format!("dev:{}:{}", line.major, line.minor), str::to_owned)
}

/// Builds a volume for an explicitly included directory. Directories that are not themselves a
/// mount point get a synthetic `dir:<path>` identity so several include roots on one filesystem
/// don't collide in the `volumes` table.
fn include_root(root: &Path, lines: &[MountInfoLine], ids: &DiskIds) -> Option<DiscoveredVolume> {
    let path = std::fs::canonicalize(root).ok()?;
    let meta = std::fs::metadata(&path).ok().filter(std::fs::Metadata::is_dir)?;
    let owner = lines
        .iter()
        .filter(|l| path.starts_with(&l.mount_point))
        .max_by_key(|l| (l.mount_point.as_os_str().len(), l.mount_id));
    let is_mount_root = owner.is_some_and(|l| l.mount_point == path);
    let fs_uuid = match owner {
        Some(l) if is_mount_root => uuid_for(l, ids),
        _ => format!("dir:{}", String::from_utf8_lossy(path.as_os_str().as_bytes())),
    };
    Some(DiscoveredVolume {
        fs_uuid,
        device: owner.map(|l| PathBuf::from(&l.source)).unwrap_or_default(),
        mount_point: path,
        dev: meta.dev(),
        fs_type: owner.map(|l| l.fs_type.clone()).unwrap_or_default(),
        label: owner.and_then(|l| ids.label(l.major, l.minor)).map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
28 1 254:0 / / rw,relatime shared:1 - ext4 /dev/vda rw,discard
30 28 254:32 / /opt/data\\040disk ro,nosuid,nodev,relatime - ext4 /dev/vdc ro,norecovery
31 28 254:0 /var/lib/docker/etc/hosts /etc/hosts rw,relatime - ext4 /dev/vda rw
32 28 254:0 /home /srv/home-bind rw,relatime master:3 shared:7 - ext4 /dev/vda rw
33 28 0:26 / /tmp rw,nosuid - tmpfs tmpfs rw,size=4k
34 28 254:48 / /mnt/b rw - ext4 /dev/disk/by\\134path rw
garbage line
";

    fn fs(types: &[&str]) -> Vec<String> {
        types.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_fields_optional_fields_and_escapes() {
        let lines = parse_mountinfo(SAMPLE);
        assert_eq!(lines.len(), 6, "garbage line is skipped");

        let root = &lines[0];
        assert_eq!((root.mount_id, root.parent_id), (28, 1));
        assert_eq!((root.major, root.minor), (254, 0));
        assert_eq!(root.root, Path::new("/"));
        assert_eq!(root.mount_point, Path::new("/"));
        assert_eq!(root.optional_fields, ["shared:1"]);
        assert_eq!(root.fs_type, "ext4");
        assert_eq!(root.source, "/dev/vda");
        assert_eq!(root.super_options, "rw,discard");

        assert_eq!(
            lines[1].mount_point,
            Path::new("/opt/data disk"),
            "\\040 is a space"
        );
        assert!(lines[1].optional_fields.is_empty());
        assert_eq!(lines[3].optional_fields, ["master:3", "shared:7"]);
        assert_eq!(lines[5].source, "/dev/disk/by\\path", "\\134 is a backslash");
    }

    #[test]
    fn escapes_may_encode_non_utf8_bytes() {
        let lines = parse_mountinfo("1 0 8:1 / /mnt/bad\\377name rw - ext4 /dev/sda1 rw\n");
        assert_eq!(lines[0].mount_point.as_os_str().as_bytes(), b"/mnt/bad\xffname");
    }

    #[test]
    fn dedupes_bind_mounts_per_filesystem() {
        let lines = parse_mountinfo(SAMPLE);
        let picked = select_mounts(&lines, &fs(&["ext4"]), |_| true);
        let points: Vec<_> = picked.iter().map(|l| l.mount_point.clone()).collect();
        // vda is mounted three times: `/` (root `/`) wins over the two bind mounts.
        assert_eq!(
            points,
            [
                PathBuf::from("/"),
                PathBuf::from("/mnt/b"),
                PathBuf::from("/opt/data disk")
            ]
        );
    }

    #[test]
    fn skips_non_directory_mount_points_before_deduping() {
        let lines = parse_mountinfo(SAMPLE);
        // Pretend `/` isn't visible as a directory: the shortest *remaining* root wins.
        let picked = select_mounts(&lines, &fs(&["ext4"]), |p| p != Path::new("/"));
        let vda: Vec<_> = picked.iter().filter(|l| l.minor == 0).collect();
        assert_eq!(vda.len(), 1);
        assert_eq!(
            vda[0].mount_point,
            Path::new("/srv/home-bind"),
            "root /home is shorter than the hosts file"
        );

        let none = select_mounts(&lines, &fs(&["ext4"]), |_| false);
        assert!(none.is_empty());
    }

    #[test]
    fn filters_by_fs_type() {
        let lines = parse_mountinfo(SAMPLE);
        assert!(select_mounts(&lines, &fs(&["btrfs"]), |_| true).is_empty());
        let tmp = select_mounts(&lines, &fs(&["tmpfs"]), |_| true);
        assert_eq!(tmp.len(), 1);
        assert_eq!(tmp[0].mount_point, Path::new("/tmp"));
    }

    #[test]
    fn same_root_prefers_shortest_mount_point() {
        let lines = parse_mountinfo(
            "1 0 8:1 / /mnt/long/path rw - ext4 /dev/sda1 rw\n2 0 8:1 / /mnt/a rw - ext4 /dev/sda1 rw\n",
        );
        let picked = select_mounts(&lines, &fs(&["ext4"]), |_| true);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].mount_point, Path::new("/mnt/a"));
    }

    #[test]
    fn udev_label_escapes() {
        assert_eq!(unescape_udev("My\\x20Disk"), "My Disk");
        assert_eq!(unescape_udev("plain"), "plain");
        assert_eq!(unescape_udev("bad\\xzz"), "bad\\xzz");
    }

    #[test]
    fn include_roots_get_synthetic_identity_and_respect_disabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = VolumesConfig {
            auto_discover_fs_types: vec![],
            include: vec![dir.path().to_path_buf()],
            disabled_uuids: vec![],
        };
        let found = discover_from(&config, &[], &DiskIds::default());
        assert_eq!(found.len(), 1);
        let canonical = dir.path().canonicalize().expect("canonicalize");
        assert_eq!(found[0].mount_point, canonical);
        assert!(found[0].fs_uuid.starts_with("dir:"), "{}", found[0].fs_uuid);
        assert_eq!(found[0].dev, std::fs::metadata(&canonical).expect("stat").dev());

        let disabled = VolumesConfig {
            disabled_uuids: vec![found[0].fs_uuid.clone()],
            ..config
        };
        assert!(discover_from(&disabled, &[], &DiskIds::default()).is_empty());
    }

    #[test]
    fn include_root_inside_discovered_volume_is_not_duplicated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let canonical = dir.path().canonicalize().expect("canonicalize");
        let meta = std::fs::metadata(&canonical).expect("stat");
        // Pretend the tempdir's filesystem is mounted at its parent and discovered automatically.
        let parent = canonical.parent().expect("parent");
        let mountinfo = format!(
            "1 0 {}:{} / {} rw - ext4 /dev/fake rw\n",
            rustix::fs::major(meta.dev()),
            rustix::fs::minor(meta.dev()),
            parent.display()
        );
        let lines = parse_mountinfo(&mountinfo);
        let config = VolumesConfig {
            auto_discover_fs_types: vec!["ext4".to_owned()],
            include: vec![canonical.clone()],
            disabled_uuids: vec![],
        };
        let found = discover_from(&config, &lines, &DiskIds::default());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].mount_point, parent);
        assert_eq!(found[0].fs_type, "ext4");
        assert!(found[0].fs_uuid.starts_with("dev:"));
    }
}
