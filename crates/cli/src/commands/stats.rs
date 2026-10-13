//! `stats`: numbers about the index.

use anyhow::{Context as _, Result};

use crate::context::Env;
use crate::{output, render};

pub(crate) fn run(env: &Env) -> Result<()> {
    let stats = env
        .open_existing_store()?
        .stats()
        .context("cannot read the index statistics")?;
    output::text(&render::render_stats(&stats));
    Ok(())
}
