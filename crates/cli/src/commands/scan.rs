//! `scan --dry-run`: show what discovery would index.

use anyhow::{Context as _, Result, bail};
use everythux_core::exclusion::ExclusionSet;
use everythux_index::{DiscoveredVolume, discover_volumes};

use crate::cli::ScanArgs;
use crate::context::Env;
use crate::{output, render};

pub(crate) fn run(env: &Env, args: &ScanArgs) -> Result<()> {
    if !args.dry_run {
        bail!("`scan` only supports --dry-run; use `everythux-cli reindex` to update the index");
    }
    let config = env.load_config()?;
    ExclusionSet::compile(&config.exclusions)
        .context("the configuration contains an invalid exclusion rule")?;
    let mut volumes = discover_volumes(&config.volumes).context("cannot discover volumes")?;
    if let Some(wanted) = &args.volume {
        volumes.retain(|v| volume_matches(v, wanted));
        if volumes.is_empty() {
            bail!("no discovered volume matches {wanted:?} (see `everythux-cli volumes`)");
        }
    }
    let active = config.exclusions.iter().filter(|r| r.enabled).count();
    output::text(&render::render_discovered(
        &volumes,
        active,
        config.exclusions.len(),
    ));
    Ok(())
}

/// Whether `wanted` names `volume` by filesystem UUID or mount point (trailing `/` ignored).
pub(crate) fn volume_matches(volume: &DiscoveredVolume, wanted: &str) -> bool {
    let trimmed = if wanted.len() > 1 {
        wanted.trim_end_matches('/')
    } else {
        wanted
    };
    volume.fs_uuid == wanted || volume.mount_point.as_os_str() == trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(uuid: &str, mount: &str) -> DiscoveredVolume {
        DiscoveredVolume {
            fs_uuid: uuid.to_owned(),
            device: "/dev/sda1".into(),
            mount_point: mount.into(),
            dev: 1,
            fs_type: "ext4".to_owned(),
            label: None,
        }
    }

    #[test]
    fn volumes_match_by_uuid_or_mount_point() {
        let v = volume("1234-abcd", "/home");
        assert!(volume_matches(&v, "1234-abcd"));
        assert!(volume_matches(&v, "/home"));
        assert!(volume_matches(&v, "/home/"));
        assert!(!volume_matches(&v, "/hom"));
        assert!(volume_matches(&volume("u", "/"), "/"));
    }
}
