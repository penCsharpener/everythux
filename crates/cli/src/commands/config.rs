//! `config path | show | reset`.

use anyhow::{Result, bail};
use everythux_core::config::Config;

use crate::cli::ConfigAction;
use crate::context::Env;
use crate::output;

pub(crate) fn run(env: &Env, action: &ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Path => output::line(&env.config.display().to_string()),
        ConfigAction::Show => output::line(&env.load_config()?.to_json()),
        ConfigAction::Reset { yes } => {
            if !yes {
                bail!(
                    "this overwrites {} with the defaults (your rules and settings are lost); pass --yes to confirm",
                    env.config.display()
                );
            }
            env.save_config(&Config::default())?;
            output::line(&format!("Wrote defaults to {}", env.config.display()));
        }
    }
    Ok(())
}
