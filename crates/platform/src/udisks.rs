//! UDisks2 over the system bus (ROADMAP Phase 7 "Volumes panel").
//!
//! Both functions are **blocking** (D-Bus round trips, and `mount` may wait for a polkit prompt).
//! Call them from a worker thread, never from the UI thread.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::time::Duration;

use zbus::blocking::Connection;
use zbus::blocking::connection::Builder;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use crate::PlatformError;

const SERVICE: &str = "org.freedesktop.UDisks2";
const ROOT_PATH: &str = "/org/freedesktop/UDisks2";
const BLOCK_IFACE: &str = "org.freedesktop.UDisks2.Block";
const FILESYSTEM_IFACE: &str = "org.freedesktop.UDisks2.Filesystem";
const OBJECT_MANAGER_IFACE: &str = "org.freedesktop.DBus.ObjectManager";

/// Listing is quick; if UDisks2 does not answer in this time we give up.
const LIST_TIMEOUT: Duration = Duration::from_secs(5);
/// Mounting may wait for the user to type a polkit password.
const MOUNT_TIMEOUT: Duration = Duration::from_secs(120);

/// A block device carrying a filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockDevice {
    /// D-Bus object path (needed to mount).
    pub object_path: String,
    /// Device node, e.g. `/dev/sdb1`.
    pub device: PathBuf,
    /// Filesystem UUID.
    pub fs_uuid: Option<String>,
    /// Filesystem type, e.g. `ext4`.
    pub fs_type: String,
    /// Label.
    pub label: Option<String>,
    /// Size in bytes.
    pub size: u64,
    /// Current mount points (empty = unmounted).
    pub mount_points: Vec<PathBuf>,
}

/// Lists filesystems of the given types known to UDisks2. `Err` if UDisks2 isn't available
/// (the UI then shows only mounted volumes from mountinfo).
///
/// An empty `fs_types` slice means "do not filter by type". Blocking: call from a worker thread.
pub fn list_filesystems(fs_types: &[String]) -> Result<Vec<BlockDevice>, PlatformError> {
    let conn = connect(LIST_TIMEOUT)?;
    let reply = conn
        .call_method(
            Some(SERVICE),
            ROOT_PATH,
            Some(OBJECT_MANAGER_IFACE),
            "GetManagedObjects",
            &(),
        )
        .map_err(PlatformError::dbus)?;
    let managed: ManagedObjects = reply.body().deserialize().map_err(PlatformError::dbus)?;
    Ok(parse_objects(&to_raw_objects(&managed), fs_types))
}

/// Mounts a filesystem via UDisks2 (polkit may prompt) and returns the mount point.
///
/// Blocking, possibly for as long as the user takes to authenticate: call from a worker thread.
pub fn mount(object_path: &str) -> Result<PathBuf, PlatformError> {
    let path = ObjectPath::try_from(object_path).map_err(PlatformError::dbus)?;
    let conn = connect(MOUNT_TIMEOUT)?;
    let options: HashMap<&str, Value<'_>> = HashMap::from([("auth.no_user_interaction", Value::Bool(false))]);
    let reply = conn
        .call_method(Some(SERVICE), path, Some(FILESYSTEM_IFACE), "Mount", &(options,))
        .map_err(PlatformError::dbus)?;
    let mount_path: String = reply.body().deserialize().map_err(PlatformError::dbus)?;
    Ok(PathBuf::from(mount_path))
}

fn connect(timeout: Duration) -> Result<Connection, PlatformError> {
    Builder::system()
        .and_then(|builder| builder.method_timeout(timeout).build())
        .map_err(PlatformError::dbus)
}

/// `GetManagedObjects` reply: object path → interface → property → value.
type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// `org.freedesktop.UDisks2.Block` properties we use, as plain Rust values.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct RawBlock {
    /// `Device`: NUL-terminated byte string.
    device: Vec<u8>,
    /// `PreferredDevice`: NUL-terminated byte string.
    preferred_device: Vec<u8>,
    id_uuid: String,
    id_type: String,
    id_label: String,
    size: u64,
    hint_ignore: bool,
}

/// `org.freedesktop.UDisks2.Filesystem` properties we use.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct RawFilesystem {
    /// `MountPoints` (`aay`): NUL-terminated byte strings.
    mount_points: Vec<Vec<u8>>,
}

/// One UDisks2 object with the interfaces we care about.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct RawObject {
    object_path: String,
    block: Option<RawBlock>,
    filesystem: Option<RawFilesystem>,
}

fn to_raw_objects(managed: &ManagedObjects) -> Vec<RawObject> {
    managed
        .iter()
        .map(|(path, interfaces)| RawObject {
            object_path: path.as_str().to_owned(),
            block: interfaces.get(BLOCK_IFACE).map(raw_block),
            filesystem: interfaces.get(FILESYSTEM_IFACE).map(raw_filesystem),
        })
        .collect()
}

fn raw_block(props: &HashMap<String, OwnedValue>) -> RawBlock {
    RawBlock {
        device: bytes_prop(props, "Device"),
        preferred_device: bytes_prop(props, "PreferredDevice"),
        id_uuid: string_prop(props, "IdUUID"),
        id_type: string_prop(props, "IdType"),
        id_label: string_prop(props, "IdLabel"),
        size: props.get("Size").and_then(|v| u64::try_from(v).ok()).unwrap_or(0),
        hint_ignore: props
            .get("HintIgnore")
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false),
    }
}

fn raw_filesystem(props: &HashMap<String, OwnedValue>) -> RawFilesystem {
    let mount_points = props
        .get("MountPoints")
        .and_then(|v| Vec::<Vec<u8>>::try_from(v.clone()).ok())
        .unwrap_or_default();
    RawFilesystem { mount_points }
}

fn bytes_prop(props: &HashMap<String, OwnedValue>, name: &str) -> Vec<u8> {
    props
        .get(name)
        .and_then(|v| Vec::<u8>::try_from(v.clone()).ok())
        .unwrap_or_default()
}

fn string_prop(props: &HashMap<String, OwnedValue>, name: &str) -> String {
    props
        .get(name)
        .and_then(|v| <&str>::try_from(v).ok())
        .unwrap_or_default()
        .to_owned()
}

/// Strips the trailing NUL (and anything after it) from a UDisks2 byte string.
fn c_bytes_to_path(bytes: &[u8]) -> PathBuf {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    PathBuf::from(OsStr::from_bytes(&bytes[..end]))
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_owned())
}

/// Turns raw UDisks2 objects into [`BlockDevice`]s: only objects with both a Block and a
/// Filesystem interface, not hinted as ignored, whose `IdType` is in `fs_types` (empty = all).
/// The result is sorted by device node for stable display.
fn parse_objects(objects: &[RawObject], fs_types: &[String]) -> Vec<BlockDevice> {
    let mut devices: Vec<BlockDevice> = objects
        .iter()
        .filter_map(|object| {
            let block = object.block.as_ref()?;
            let filesystem = object.filesystem.as_ref()?;
            if block.hint_ignore {
                return None;
            }
            if !fs_types.is_empty() && !fs_types.contains(&block.id_type) {
                return None;
            }
            let preferred = c_bytes_to_path(&block.preferred_device);
            let device = if preferred.as_os_str().is_empty() {
                c_bytes_to_path(&block.device)
            } else {
                preferred
            };
            let mount_points = filesystem
                .mount_points
                .iter()
                .map(|mp| c_bytes_to_path(mp))
                .filter(|mp| !mp.as_os_str().is_empty())
                .collect();
            Some(BlockDevice {
                object_path: object.object_path.clone(),
                device,
                fs_uuid: non_empty(&block.id_uuid),
                fs_type: block.id_type.clone(),
                label: non_empty(&block.id_label),
                size: block.size,
                mount_points,
            })
        })
        .collect();
    devices.sort_by(|a, b| {
        a.device
            .cmp(&b.device)
            .then_with(|| a.object_path.cmp(&b.object_path))
    });
    devices
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(device: &str, id_type: &str) -> RawBlock {
        RawBlock {
            device: [device.as_bytes(), b"\0"].concat(),
            preferred_device: Vec::new(),
            id_uuid: "1234-abcd".into(),
            id_type: id_type.into(),
            id_label: "Data".into(),
            size: 1 << 30,
            hint_ignore: false,
        }
    }

    fn object(path: &str, block: Option<RawBlock>, mounts: Option<&[&[u8]]>) -> RawObject {
        RawObject {
            object_path: path.into(),
            block,
            filesystem: mounts.map(|m| RawFilesystem {
                mount_points: m.iter().map(|b| b.to_vec()).collect(),
            }),
        }
    }

    fn types(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_unmounted_and_mounted_ext4() {
        let objects = [
            object(
                "/org/freedesktop/UDisks2/block_devices/sdb2",
                Some(block("/dev/sdb2", "ext4")),
                Some(&[b"/mnt/data\0", b"/srv/data\0"]),
            ),
            object(
                "/org/freedesktop/UDisks2/block_devices/sdb1",
                Some(block("/dev/sdb1", "ext4")),
                Some(&[]),
            ),
        ];
        let parsed = parse_objects(&objects, &types(&["ext4"]));
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].device, PathBuf::from("/dev/sdb1"));
        assert_eq!(parsed[0].mount_points, [] as [PathBuf; 0]);
        assert_eq!(parsed[0].fs_uuid.as_deref(), Some("1234-abcd"));
        assert_eq!(parsed[0].label.as_deref(), Some("Data"));
        assert_eq!(parsed[0].size, 1 << 30);
        assert_eq!(parsed[0].fs_type, "ext4");
        assert_eq!(
            parsed[1].mount_points,
            [PathBuf::from("/mnt/data"), PathBuf::from("/srv/data")]
        );
        assert_eq!(
            parsed[1].object_path,
            "/org/freedesktop/UDisks2/block_devices/sdb2"
        );
    }

    #[test]
    fn filters_by_type_ignored_and_missing_interfaces() {
        let mut ignored = block("/dev/sdc1", "ext4");
        ignored.hint_ignore = true;
        let objects = [
            object("/a", Some(block("/dev/sda1", "vfat")), Some(&[])),
            object("/b", Some(block("/dev/sda2", "ext4")), None),
            object("/c", None, Some(&[])),
            object("/d", Some(ignored), Some(&[])),
            object("/e", Some(block("/dev/sda5", "ext4")), Some(&[])),
        ];
        let ext4 = parse_objects(&objects, &types(&["ext4"]));
        assert_eq!(
            ext4.iter().map(|d| d.object_path.as_str()).collect::<Vec<_>>(),
            ["/e"]
        );
        let all = parse_objects(&objects, &[]);
        assert_eq!(
            all.iter().map(|d| d.object_path.as_str()).collect::<Vec<_>>(),
            ["/a", "/e"]
        );
        assert_eq!(
            parse_objects(&objects, &types(&["btrfs"])),
            [] as [BlockDevice; 0]
        );
    }

    #[test]
    fn empty_strings_become_none_and_preferred_device_wins() {
        let mut b = block("/dev/dm-0", "ext4");
        b.id_uuid.clear();
        b.id_label.clear();
        b.preferred_device = b"/dev/mapper/vg-root\0".to_vec();
        let parsed = parse_objects(&[object("/x", Some(b), Some(&[b"\0"]))], &types(&["ext4"]));
        assert_eq!(parsed[0].device, PathBuf::from("/dev/mapper/vg-root"));
        assert_eq!(parsed[0].fs_uuid, None);
        assert_eq!(parsed[0].label, None);
        assert!(parsed[0].mount_points.is_empty(), "bare NUL is not a mount point");
    }

    #[test]
    fn non_utf8_mount_point_survives() {
        let objects = [object(
            "/x",
            Some(block("/dev/sdb1", "ext4")),
            Some(&[b"/mnt/a\xffb\0"]),
        )];
        let parsed = parse_objects(&objects, &types(&["ext4"]));
        assert_eq!(parsed[0].mount_points[0].as_os_str().as_bytes(), b"/mnt/a\xffb");
    }

    #[test]
    fn nul_terminated_bytes_without_terminator_are_accepted() {
        assert_eq!(c_bytes_to_path(b"/dev/sda1"), PathBuf::from("/dev/sda1"));
        assert_eq!(c_bytes_to_path(b"/dev/sda1\0junk"), PathBuf::from("/dev/sda1"));
    }

    #[test]
    fn converts_zvariant_properties() {
        let mut props = HashMap::new();
        let ov = |v: Value<'_>| OwnedValue::try_from(v).expect("no fds");
        props.insert("Device".to_owned(), ov(Value::from(b"/dev/sdb1\0".to_vec())));
        props.insert("IdUUID".to_owned(), ov(Value::from("u-1")));
        props.insert("IdType".to_owned(), ov(Value::from("ext4")));
        props.insert("Size".to_owned(), ov(Value::from(42u64)));
        props.insert("HintIgnore".to_owned(), ov(Value::from(false)));
        let b = raw_block(&props);
        assert_eq!(b.device, b"/dev/sdb1\0");
        assert_eq!(
            (b.id_uuid.as_str(), b.id_type.as_str(), b.size),
            ("u-1", "ext4", 42)
        );
        assert!(!b.hint_ignore);

        let mut fs = HashMap::new();
        fs.insert(
            "MountPoints".to_owned(),
            ov(Value::from(vec![b"/mnt/x\0".to_vec()])),
        );
        assert_eq!(raw_filesystem(&fs).mount_points, [b"/mnt/x\0".to_vec()]);
    }

    #[test]
    fn invalid_object_path_is_rejected_before_connecting() {
        let err = mount("not an object path").expect_err("must fail");
        assert!(matches!(err, PlatformError::DBus(_)), "{err:?}");
    }

    #[test]
    fn without_a_system_bus_calls_fail_quickly() {
        // The container has no system bus; on a real desktop UDisks2 may answer, so only check
        // that a failure is reported as `DBus` and that we do not hang.
        let started = std::time::Instant::now();
        let result = list_filesystems(&types(&["ext4"]));
        if let Err(err) = result {
            assert!(matches!(err, PlatformError::DBus(_)), "{err:?}");
        }
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
