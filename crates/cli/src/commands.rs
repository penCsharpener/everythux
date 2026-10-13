//! One module per subcommand; [`run`] dispatches.

mod config;
mod exclude;
mod reindex;
mod scan;
mod search;
mod stats;
pub(crate) mod volumes;

use anyhow::Result;

use crate::cli::{Cli, Command};
use crate::context::Env;

pub(crate) fn run(cli: &Cli) -> Result<()> {
    let env = Env::resolve(cli.db.clone(), cli.config.clone())?;
    match &cli.command {
        Command::Scan(args) => scan::run(&env, args),
        Command::Volumes => volumes::run(&env),
        Command::Reindex(args) => reindex::run(&env, args),
        Command::Search(args) => search::run(&env, args),
        Command::Stats => stats::run(&env),
        Command::Exclude { action } => exclude::run(&env, action),
        Command::Config { action } => config::run(&env, action),
    }
}
