//! Volumes panel (Phase 7): indexed volumes merged with what UDisks2 knows about.

use std::collections::HashSet;
use std::path::PathBuf;

use everythux_core::model::{VolumeInfo, VolumeState};
use everythux_index::{Job, ScanMode};
use everythux_platform::udisks::BlockDevice;

use crate::app::{App, PopupId};
use crate::status::ToastKind;

/// What UDisks2 reported.
#[derive(Debug, Default)]
pub(crate) enum DevicesState {
    #[default]
    NotRequested,
    Loading,
    Loaded(Vec<BlockDevice>),
    /// UDisks2 isn't reachable; only indexed volumes are listed.
    Unavailable(String),
}

/// Window state.
#[derive(Debug, Default)]
pub(crate) struct VolumesWindow {
    pub(crate) open: bool,
    pub(crate) devices: DevicesState,
    /// Object paths currently being mounted.
    pub(crate) mounting: HashSet<String>,
    pub(crate) message: Option<String>,
}

/// Display state of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowState {
    Online,
    Offline,
    Scanning,
    Error,
    /// A partition UDisks2 knows that isn't mounted (and so isn't indexed).
    Unmounted,
    /// Mounted but not part of the index.
    NotIndexed,
}

impl RowState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Offline => "offline",
            Self::Scanning => "scanning",
            Self::Error => "error",
            Self::Unmounted => "not mounted",
            Self::NotIndexed => "not indexed",
        }
    }
}

/// One line of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VolumeRow {
    pub(crate) mount_point: Option<PathBuf>,
    pub(crate) device: PathBuf,
    pub(crate) label: Option<String>,
    pub(crate) uuid: Option<String>,
    pub(crate) fs_type: String,
    pub(crate) state: RowState,
    pub(crate) last_scan_ns: Option<i64>,
    pub(crate) last_scan_ms: Option<u64>,
    /// Whether the volume is part of the index (and so can be rescanned / disabled).
    pub(crate) indexed: bool,
    pub(crate) enabled: bool,
    /// UDisks2 object path when the device can be mounted from here.
    pub(crate) mountable: Option<String>,
}

fn state_of(state: VolumeState) -> RowState {
    match state {
        VolumeState::Online => RowState::Online,
        VolumeState::Offline => RowState::Offline,
        VolumeState::Scanning => RowState::Scanning,
        VolumeState::Error => RowState::Error,
    }
}

/// Merges indexed volumes with UDisks2 block devices. Indexed volumes come first, in index order;
/// devices the index doesn't know follow.
pub(crate) fn merge_rows(
    volumes: &[VolumeInfo],
    devices: &[BlockDevice],
    disabled_uuids: &[String],
) -> Vec<VolumeRow> {
    let mut rows: Vec<VolumeRow> = volumes
        .iter()
        .map(|v| {
            let device = devices
                .iter()
                .find(|d| d.fs_uuid.as_deref() == Some(v.fs_uuid.as_str()));
            VolumeRow {
                mount_point: (v.state != VolumeState::Offline).then(|| v.mount_point.clone()),
                device: v.device.clone(),
                label: v.label.clone(),
                uuid: Some(v.fs_uuid.clone()),
                fs_type: v.fs_type.clone(),
                state: state_of(v.state),
                last_scan_ns: v.last_full_scan_ns,
                last_scan_ms: v.last_scan_ms,
                indexed: true,
                enabled: !disabled_uuids.contains(&v.fs_uuid),
                mountable: device
                    .filter(|d| d.mount_points.is_empty())
                    .map(|d| d.object_path.clone()),
            }
        })
        .collect();
    for d in devices {
        let known = d
            .fs_uuid
            .as_ref()
            .is_some_and(|u| volumes.iter().any(|v| &v.fs_uuid == u));
        if known {
            continue;
        }
        let unmounted = d.mount_points.is_empty();
        rows.push(VolumeRow {
            mount_point: d.mount_points.first().cloned(),
            device: d.device.clone(),
            label: d.label.clone(),
            uuid: d.fs_uuid.clone(),
            fs_type: d.fs_type.clone(),
            state: if unmounted {
                RowState::Unmounted
            } else {
                RowState::NotIndexed
            },
            last_scan_ns: None,
            last_scan_ms: None,
            indexed: false,
            enabled: !d.fs_uuid.as_ref().is_some_and(|u| disabled_uuids.contains(u)),
            mountable: unmounted.then(|| d.object_path.clone()),
        });
    }
    rows
}

impl App {
    /// Index → Volumes…: shows the window and asks UDisks2 for devices.
    pub(crate) fn open_volumes_window(&mut self) {
        self.windows.volumes.open = true;
        self.refresh_devices();
        self.open_popup(PopupId::Volumes);
    }

    pub(crate) fn refresh_devices(&mut self) {
        self.windows.volumes.devices = DevicesState::Loading;
        self.backend
            .list_devices(self.config.volumes.auto_discover_fs_types.clone());
    }

    pub(crate) fn on_devices(&mut self, result: Result<Vec<BlockDevice>, String>) {
        self.windows.volumes.devices = match result {
            Ok(devices) => DevicesState::Loaded(devices),
            Err(e) => {
                tracing::info!(error = %e, "UDisks2 not available");
                DevicesState::Unavailable(e)
            }
        };
    }

    /// "Mount and index".
    pub(crate) fn mount_and_index(&mut self, object_path: String) {
        self.windows.volumes.message = None;
        if self.windows.volumes.mounting.insert(object_path.clone()) {
            self.backend.mount(object_path);
        }
    }

    pub(crate) fn on_mounted(&mut self, object_path: &str, result: Result<PathBuf, String>) {
        self.windows.volumes.mounting.remove(object_path);
        match result {
            Ok(mount_point) => {
                self.windows.volumes.message = Some(format!("Mounted at {}", mount_point.display()));
                self.backend.submit_job(Job::RescanAll(ScanMode::Full));
                if self.windows.volumes.open {
                    self.refresh_devices();
                }
            }
            Err(e) => {
                self.windows.volumes.message = Some(format!("Mount failed: {e}"));
                self.toast(format!("Mount failed: {e}"), ToastKind::Error);
            }
        }
    }

    /// The enabled checkbox: volumes in `disabled_uuids` are skipped by the indexer.
    pub(crate) fn set_volume_enabled(&mut self, uuid: &str, enabled: bool) {
        let disabled = &mut self.config.volumes.disabled_uuids;
        let present = disabled.iter().any(|u| u == uuid);
        if enabled && present {
            disabled.retain(|u| u != uuid);
        } else if !enabled && !present {
            disabled.push(uuid.to_owned());
        } else {
            return;
        }
        self.settings_changed();
    }

    /// "Quick rescan" / "Full rescan".
    pub(crate) fn rescan_volume(&mut self, uuid: &str, mode: ScanMode) {
        self.backend.submit_job(Job::RescanVolume {
            volume: uuid.to_owned(),
            mode,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::TestApp;
    use crate::backend::BackendEvent;
    use crate::backend::fake::Call;

    fn volume(uuid: &str, mount: &str, state: VolumeState) -> VolumeInfo {
        VolumeInfo {
            fs_uuid: uuid.to_owned(),
            device: format!("/dev/{uuid}").into(),
            mount_point: mount.into(),
            label: Some(uuid.to_owned()),
            fs_type: "ext4".to_owned(),
            state,
            last_full_scan_ns: Some(1),
            last_scan_ms: Some(2),
        }
    }

    fn device(uuid: &str, mounts: &[&str]) -> BlockDevice {
        BlockDevice {
            object_path: format!("/org/freedesktop/UDisks2/block_devices/{uuid}"),
            device: format!("/dev/{uuid}").into(),
            fs_uuid: Some(uuid.to_owned()),
            fs_type: "ext4".to_owned(),
            label: None,
            size: 1 << 30,
            mount_points: mounts.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn merges_indexed_volumes_and_devices() {
        let volumes = [
            volume("root", "/", VolumeState::Online),
            volume("usb", "/media/usb", VolumeState::Offline),
        ];
        let devices = [
            device("root", &["/"]),
            device("usb", &[]),
            device("new", &[]),
            device("other", &["/mnt/other"]),
        ];
        let rows = merge_rows(&volumes, &devices, &["usb".to_owned()]);
        let summary: Vec<_> = rows
            .iter()
            .map(|r| {
                (
                    r.uuid.as_deref().unwrap_or(""),
                    r.state,
                    r.enabled,
                    r.mountable.is_some(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("root", RowState::Online, true, false),
                ("usb", RowState::Offline, false, true),
                ("new", RowState::Unmounted, true, true),
                ("other", RowState::NotIndexed, true, false),
            ]
        );
        assert_eq!(
            rows[1].mount_point, None,
            "offline volumes have no live mount point"
        );
        assert_eq!(
            rows[3].mount_point.as_deref(),
            Some(std::path::Path::new("/mnt/other"))
        );
    }

    #[test]
    fn without_udisks_only_indexed_volumes_are_listed() {
        let rows = merge_rows(&[volume("root", "/", VolumeState::Online)], &[], &[]);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].mountable.is_none());
    }

    #[test]
    fn opening_the_window_requests_devices_and_handles_errors() {
        let mut t = TestApp::new();
        t.app.open_volumes_window();
        assert!(
            t.fake
                .calls()
                .contains(&Call::ListDevices(vec!["ext4".to_owned()]))
        );
        assert!(matches!(t.app.windows.volumes.devices, DevicesState::Loading));
        t.fake.send(BackendEvent::Devices(Err("no bus".into())));
        t.pump();
        assert!(matches!(
            t.app.windows.volumes.devices,
            DevicesState::Unavailable(_)
        ));
    }

    #[test]
    fn mount_and_index_mounts_then_rescans_everything() {
        let mut t = TestApp::new();
        t.app.open_volumes_window();
        t.app.mount_and_index("/obj".into());
        t.app.mount_and_index("/obj".into());
        let mounts = t
            .fake
            .calls()
            .into_iter()
            .filter(|c| matches!(c, Call::Mount(_)))
            .count();
        assert_eq!(mounts, 1, "double click mounts once");
        t.fake.send(BackendEvent::Mounted {
            object_path: "/obj".into(),
            result: Ok("/media/usb".into()),
        });
        t.pump();
        assert_eq!(t.fake.jobs(), [Job::RescanAll(ScanMode::Full)]);
        assert!(t.app.windows.volumes.mounting.is_empty());
    }

    #[test]
    fn mount_failure_is_reported() {
        let mut t = TestApp::new();
        t.app.mount_and_index("/obj".into());
        t.fake.send(BackendEvent::Mounted {
            object_path: "/obj".into(),
            result: Err("denied".into()),
        });
        t.pump();
        assert!(t.fake.jobs().is_empty());
        assert!(
            t.app
                .windows
                .volumes
                .message
                .as_deref()
                .is_some_and(|m| m.contains("denied"))
        );
    }

    #[test]
    fn enable_checkbox_edits_disabled_uuids() {
        let mut t = TestApp::new();
        t.app.set_volume_enabled("abc", false);
        assert_eq!(t.app.config.volumes.disabled_uuids, ["abc"]);
        assert_eq!(
            t.fake.last_settings().expect("settings").volumes.disabled_uuids,
            ["abc"]
        );
        t.app.set_volume_enabled("abc", false);
        assert_eq!(t.app.config.volumes.disabled_uuids.len(), 1, "no duplicates");
        t.app.set_volume_enabled("abc", true);
        assert!(t.app.config.volumes.disabled_uuids.is_empty());
    }

    #[test]
    fn rescan_buttons_submit_volume_jobs() {
        let mut t = TestApp::new();
        t.app.rescan_volume("abc", ScanMode::Quick);
        t.app.rescan_volume("abc", ScanMode::Full);
        assert_eq!(
            t.fake.jobs(),
            [
                Job::RescanVolume {
                    volume: "abc".into(),
                    mode: ScanMode::Quick
                },
                Job::RescanVolume {
                    volume: "abc".into(),
                    mode: ScanMode::Full
                }
            ]
        );
    }
}
