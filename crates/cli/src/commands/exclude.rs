//! `exclude list | add | remove | enable | disable | test`: edit `exclusions` in config.json.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use everythux_core::config::Config;
use everythux_core::exclusion::{ExclusionRule, ExclusionSet};

use crate::cli::ExcludeAction;
use crate::context::Env;
use crate::{output, render};

const APPLY_HINT: &str =
    "Run `everythux-cli reindex --quick` to apply this to the index (the desktop UI applies it immediately).";

pub(crate) fn run(env: &Env, action: &ExcludeAction) -> Result<()> {
    let mut config = env.load_config()?;
    match action {
        ExcludeAction::List => output::text(&render::render_exclusions(&config.exclusions)),
        ExcludeAction::Add { pattern, kind, scope } => {
            let mut rule = ExclusionRule::new(pattern.trim(), (*kind).into());
            rule.scope = scope.clone().filter(|s| !s.trim().is_empty());
            let index = add_rule(&mut config, rule)?;
            env.save_config(&config)?;
            output::line(&format!(
                "Added rule #{index}: {}",
                render::describe_rule(&config.exclusions[index])
            ));
            output::line(APPLY_HINT);
        }
        ExcludeAction::Remove { index } => {
            check_index(&config, *index)?;
            let rule = config.exclusions.remove(*index);
            env.save_config(&config)?;
            output::line(&format!(
                "Removed rule #{index}: {}",
                render::describe_rule(&rule)
            ));
            output::line(
                "Run `everythux-cli reindex --quick` to bring back entries that were excluded by it.",
            );
        }
        ExcludeAction::Enable { index } => set_enabled(env, &mut config, *index, true)?,
        ExcludeAction::Disable { index } => set_enabled(env, &mut config, *index, false)?,
        ExcludeAction::Test { path } => test_path(&config, path)?,
    }
    Ok(())
}

/// Validates `rule` and appends it unless an identical one exists. Returns the rule's index.
fn add_rule(config: &mut Config, rule: ExclusionRule) -> Result<usize> {
    if rule.pattern.trim().is_empty() {
        bail!("the pattern is empty");
    }
    rule.validate().context("invalid exclusion rule")?;
    if let Some(existing) = config.exclusions.iter().position(|r| *r == rule) {
        bail!("rule #{existing} is identical: {}", render::describe_rule(&rule));
    }
    config.exclusions.push(rule);
    Ok(config.exclusions.len() - 1)
}

fn check_index(config: &Config, index: usize) -> Result<()> {
    if index >= config.exclusions.len() {
        bail!(
            "there is no rule #{index} ({} rules, numbered from 0; see `exclude list`)",
            config.exclusions.len()
        );
    }
    Ok(())
}

fn set_enabled(env: &Env, config: &mut Config, index: usize, enabled: bool) -> Result<()> {
    check_index(config, index)?;
    let rule = &mut config.exclusions[index];
    rule.enabled = enabled;
    let described = render::describe_rule(rule);
    env.save_config(config)?;
    let verb = if enabled { "Enabled" } else { "Disabled" };
    output::line(&format!("{verb} rule #{index}: {described}"));
    Ok(())
}

fn test_path(config: &Config, path: &Path) -> Result<()> {
    let set = ExclusionSet::compile(&config.exclusions)
        .context("the configuration contains an invalid exclusion rule")?;
    let path = std::path::absolute(path).with_context(|| format!("cannot resolve {}", path.display()))?;
    let (is_dir, assumed) = match std::fs::symlink_metadata(&path) {
        Ok(meta) => (meta.is_dir(), false),
        Err(_) => (true, true),
    };
    let matched = set
        .matching_rule(&path, is_dir)
        .map(|index| (index, &config.exclusions[index]));
    output::text(&render::render_verdict(&path, is_dir, assumed, matched));
    Ok(())
}

#[cfg(test)]
mod tests {
    use everythux_core::exclusion::RuleKind;

    use super::*;

    #[test]
    fn add_rule_validates_and_rejects_duplicates() {
        let mut config = Config::default();
        let before = config.exclusions.len();
        let index = add_rule(&mut config, ExclusionRule::new("*.o", RuleKind::File)).unwrap();
        assert_eq!(index, before);

        let dup = add_rule(&mut config, ExclusionRule::new("*.o", RuleKind::File)).unwrap_err();
        assert!(dup.to_string().contains(&format!("#{index}")), "{dup}");

        let bad = add_rule(&mut config, ExclusionRule::new("[", RuleKind::Any)).unwrap_err();
        assert!(format!("{bad:#}").contains("invalid"), "{bad:#}");
        let relative = add_rule(&mut config, ExclusionRule::new("rel/dir", RuleKind::Path)).unwrap_err();
        assert!(format!("{relative:#}").contains("absolute"), "{relative:#}");
        assert!(add_rule(&mut config, ExclusionRule::new("  ", RuleKind::Any)).is_err());
        assert_eq!(
            config.exclusions.len(),
            before + 1,
            "failed adds leave the list untouched"
        );
    }

    #[test]
    fn index_checks_name_the_valid_range() {
        let config = Config::default();
        let n = config.exclusions.len();
        assert!(check_index(&config, n - 1).is_ok());
        let err = check_index(&config, n).unwrap_err().to_string();
        assert!(err.contains(&format!("no rule #{n}")), "{err}");
    }
}
