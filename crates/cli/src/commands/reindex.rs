//! `reindex`: run one indexer job to completion, with progress on stderr.

use std::io::IsTerminal as _;

use anyhow::{Context as _, Result};
use everythux_core::config::StartupRescan;
use everythux_index::{Indexer, IndexerSettings};

use crate::cli::ReindexArgs;
use crate::context::Env;
use crate::progress::{Reporter, wait_for_job};
use crate::{output, render};

pub(crate) fn run(env: &Env, args: &ReindexArgs) -> Result<()> {
    let job = args.job()?;
    let config = env.load_config()?;
    if let Some(parent) = env.db.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    }

    // The CLI runs exactly the job it was asked for: no startup rescan, no inotify watcher.
    // (An empty database still gets its initial full scan; `wait_for_job` copes with that.)
    let mut indexer = config.indexer;
    indexer.rescan_on_startup = StartupRescan::None;
    indexer.live_updates = false;
    let settings = IndexerSettings {
        volumes: config.volumes,
        exclusions: config.exclusions,
        indexer,
    };

    let (handle, events) = Indexer::start(&env.db, settings)
        .with_context(|| format!("cannot start the indexer for {}", env.db.display()))?;
    let id = handle.submit(job);
    tracing::info!(?id, "job submitted");

    let stderr = std::io::stderr();
    let tty = stderr.is_terminal();
    let stats = wait_for_job(&events, id, &mut Reporter::new(stderr, tty))?;
    output::text(&render::render_job_stats(&stats));
    Ok(())
}
