//! `volumes`: known volumes (database) merged with the currently mounted ones.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use everythux_core::model::{VolumeInfo, VolumeState};
use everythux_index::{DiscoveredVolume, Store, discover_volumes};

use crate::context::Env;
use crate::{output, render};

/// One line of the `volumes` report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VolumeRow {
    /// `online`, `offline`, `scanning`, `error`, or `new` (mounted but not indexed yet).
    pub(crate) state: &'static str,
    pub(crate) uuid: String,
    pub(crate) mount_point: PathBuf,
    pub(crate) label: Option<String>,
    pub(crate) fs_type: String,
    pub(crate) last_scan_ns: Option<i64>,
}

pub(crate) fn run(env: &Env) -> Result<()> {
    let config = env.load_config()?;
    let known = if env.db.exists() {
        Store::open(&env.db)
            .and_then(|store| store.volumes())
            .context("cannot read the volumes from the index database")?
    } else {
        Vec::new()
    };
    let discovered = discover_volumes(&config.volumes).context("cannot discover volumes")?;
    output::text(&render::render_volumes(&merge_volumes(&known, &discovered)));
    Ok(())
}

/// Known volumes first (in database order), then mounted volumes that were never indexed.
///
/// A known volume that isn't mounted right now is `offline` whatever the database says; one that
/// is mounted keeps `scanning` / `error` and is otherwise `online`.
pub(crate) fn merge_volumes(known: &[VolumeInfo], discovered: &[DiscoveredVolume]) -> Vec<VolumeRow> {
    let mut rows: Vec<VolumeRow> = known
        .iter()
        .map(|info| {
            let mounted = discovered.iter().find(|d| d.fs_uuid == info.fs_uuid);
            let state = match (mounted, info.state) {
                (None, _) => "offline",
                (Some(_), VolumeState::Scanning) => "scanning",
                (Some(_), VolumeState::Error) => "error",
                (Some(_), VolumeState::Online | VolumeState::Offline) => "online",
            };
            VolumeRow {
                state,
                uuid: info.fs_uuid.clone(),
                mount_point: mounted.map_or_else(|| info.mount_point.clone(), |d| d.mount_point.clone()),
                label: mounted
                    .and_then(|d| d.label.clone())
                    .or_else(|| info.label.clone()),
                fs_type: info.fs_type.clone(),
                last_scan_ns: info.last_full_scan_ns,
            }
        })
        .collect();
    rows.extend(
        discovered
            .iter()
            .filter(|d| !known.iter().any(|k| k.fs_uuid == d.fs_uuid))
            .map(|d| VolumeRow {
                state: "new",
                uuid: d.fs_uuid.clone(),
                mount_point: d.mount_point.clone(),
                label: d.label.clone(),
                fs_type: d.fs_type.clone(),
                last_scan_ns: None,
            }),
    );
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(uuid: &str, mount: &str, state: VolumeState) -> VolumeInfo {
        VolumeInfo {
            fs_uuid: uuid.to_owned(),
            device: "/dev/x".into(),
            mount_point: mount.into(),
            label: Some("old".to_owned()),
            fs_type: "ext4".to_owned(),
            state,
            last_full_scan_ns: Some(5),
            last_scan_ms: None,
        }
    }

    fn mounted(uuid: &str, mount: &str) -> DiscoveredVolume {
        DiscoveredVolume {
            fs_uuid: uuid.to_owned(),
            device: "/dev/x".into(),
            mount_point: mount.into(),
            dev: 1,
            fs_type: "ext4".to_owned(),
            label: None,
        }
    }

    #[test]
    fn states_combine_database_and_mounts() {
        let rows = merge_volumes(
            &[
                known("a", "/a", VolumeState::Online),
                known("b", "/b", VolumeState::Online),
                known("c", "/c", VolumeState::Offline),
                known("d", "/d", VolumeState::Scanning),
            ],
            &[
                mounted("a", "/mnt/a"),
                mounted("c", "/c"),
                mounted("d", "/d"),
                mounted("n", "/new"),
            ],
        );
        let summary: Vec<(&str, &str, &str)> = rows
            .iter()
            .map(|r| (r.uuid.as_str(), r.state, r.mount_point.to_str().unwrap()))
            .collect();
        assert_eq!(
            summary,
            [
                ("a", "online", "/mnt/a"), // mounted elsewhere now
                ("b", "offline", "/b"),    // keeps its last known mount point
                ("c", "online", "/c"),     // was offline, is mounted again
                ("d", "scanning", "/d"),
                ("n", "new", "/new"),
            ]
        );
        assert_eq!(
            rows[0].label.as_deref(),
            Some("old"),
            "falls back to the stored label"
        );
        assert_eq!(rows[4].last_scan_ns, None);
    }
}
