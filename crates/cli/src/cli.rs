//! Command-line definition (clap derive).

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use everythux_core::config::ColumnId;
use everythux_core::exclusion::RuleKind;
use everythux_index::{Job, ScanMode};

/// Instant file search for Linux: headless interface.
#[derive(Debug, Parser)]
#[command(name = "everythux-cli", version, about, long_about = None)]
pub(crate) struct Cli {
    /// Index database (default: `$XDG_DATA_HOME/everythux/index.db`, or `$EVERYTHUX_DB`).
    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) db: Option<PathBuf>,

    /// Configuration file (default: `$XDG_CONFIG_HOME/everythux/config.json`, or `$EVERYTHUX_CONFIG`).
    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) config: Option<PathBuf>,

    /// More logging on stderr (-v info, -vv debug, -vvv trace). Without it, `RUST_LOG` applies.
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub(crate) verbose: u8,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Show which volumes would be indexed.
    Scan(ScanArgs),
    /// List known and currently mounted volumes.
    Volumes,
    /// Rebuild or refresh (part of) the index.
    Reindex(ReindexArgs),
    /// Search the index and print the matching paths.
    Search(SearchArgs),
    /// Print entry counts, database size and per-volume numbers.
    Stats,
    /// View and edit the exclusion rules in config.json.
    Exclude {
        #[command(subcommand)]
        action: ExcludeAction,
    },
    /// Inspect or reset config.json.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Debug, Args)]
pub(crate) struct ScanArgs {
    /// Only discover and print volumes; nothing is read or written. Required: use `reindex` to scan.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Restrict to one volume (filesystem UUID or mount point).
    #[arg(long, value_name = "UUID|MOUNTPOINT")]
    pub(crate) volume: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ReindexArgs {
    /// Rebuild from scratch (for everything: build a new database and swap it in atomically).
    #[arg(long, conflicts_with = "quick")]
    pub(crate) full: bool,

    /// Re-list only folders whose mtime changed (the default).
    #[arg(long)]
    pub(crate) quick: bool,

    /// Only this volume (filesystem UUID or mount point).
    #[arg(long, value_name = "UUID|MOUNTPOINT", conflicts_with = "path")]
    pub(crate) volume: Option<String>,

    /// Only this folder and everything below it.
    #[arg(long, value_name = "PATH")]
    pub(crate) path: Option<PathBuf>,
}

impl ReindexArgs {
    /// The indexer job these flags ask for.
    ///
    /// `--full` without a target is the same as `Ctrl+Shift+R` in the UI ([`Job::FullReindex`]);
    /// with a target it is a full-mode rescan of that target.
    pub(crate) fn job(&self) -> anyhow::Result<Job> {
        let mode = if self.full {
            ScanMode::Full
        } else {
            ScanMode::Quick
        };
        if let Some(path) = &self.path {
            let path = std::path::absolute(path)
                .map_err(|e| anyhow::anyhow!("cannot resolve {}: {e}", path.display()))?;
            return Ok(Job::RescanPath { path, mode });
        }
        if let Some(volume) = &self.volume {
            return Ok(Job::RescanVolume {
                volume: volume.clone(),
                mode,
            });
        }
        Ok(if self.full {
            Job::FullReindex
        } else {
            Job::RescanAll(mode)
        })
    }
}

/// Sortable columns, as spelled on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SortKey {
    Name,
    Path,
    Type,
    Modified,
    Size,
}

impl From<SortKey> for ColumnId {
    fn from(key: SortKey) -> Self {
        match key {
            SortKey::Name => Self::Name,
            SortKey::Path => Self::Path,
            SortKey::Type => Self::Type,
            SortKey::Modified => Self::Modified,
            SortKey::Size => Self::Size,
        }
    }
}

#[derive(Debug, Args)]
#[expect(clippy::struct_excessive_bools, reason = "independent command-line switches")]
pub(crate) struct SearchArgs {
    /// Search terms (joined with spaces). See docs/search-syntax.md.
    #[arg(required = true, value_name = "QUERY")]
    pub(crate) query: Vec<String>,

    /// Print at most N results (0 = all).
    #[arg(short = 'n', long, default_value_t = 0, value_name = "N")]
    pub(crate) limit: usize,

    /// Sort column.
    #[arg(long, value_enum, default_value_t = SortKey::Name)]
    pub(crate) sort: SortKey,

    /// Reverse the sort order.
    #[arg(long)]
    pub(crate) desc: bool,

    /// Case-sensitive matching.
    #[arg(short = 'c', long = "case")]
    pub(crate) case_sensitive: bool,

    /// Match terms against the full path instead of just the name.
    #[arg(short = 'p', long)]
    pub(crate) match_path: bool,

    /// Treat the whole query as one regular expression.
    #[arg(short = 'r', long)]
    pub(crate) regex: bool,

    /// One JSON object per line (name, path, `full_path`, size, modified, kind).
    #[arg(long, conflicts_with = "null")]
    pub(crate) json: bool,

    /// Separate paths with NUL instead of newline (for `xargs -0`).
    #[arg(short = '0', long)]
    pub(crate) null: bool,
}

/// How rule kinds are spelled on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum KindArg {
    Dir,
    File,
    Path,
    Any,
}

impl From<KindArg> for RuleKind {
    fn from(kind: KindArg) -> Self {
        match kind {
            KindArg::Dir => Self::Dir,
            KindArg::File => Self::File,
            KindArg::Path => Self::Path,
            KindArg::Any => Self::Any,
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum ExcludeAction {
    /// List the rules with their indexes.
    List,
    /// Append a rule.
    Add {
        /// Glob (`node_modules`, `*.o`, `/home/*/.cache`) or `re:<regex>`.
        pattern: String,
        /// What the rule applies to.
        #[arg(long, value_enum, default_value_t = KindArg::Any)]
        kind: KindArg,
        /// Only apply below paths matching this glob (e.g. `/home/*`).
        #[arg(long, value_name = "GLOB")]
        scope: Option<String>,
    },
    /// Delete the rule with this index.
    Remove {
        /// Index from `exclude list`.
        index: usize,
    },
    /// Enable the rule with this index.
    Enable {
        /// Index from `exclude list`.
        index: usize,
    },
    /// Disable the rule with this index.
    Disable {
        /// Index from `exclude list`.
        index: usize,
    },
    /// Show whether a path is excluded, and by which rule.
    Test {
        /// Path to check (it doesn't have to exist).
        path: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigAction {
    /// Print the config file location.
    Path,
    /// Print the effective configuration as JSON.
    Show,
    /// Overwrite config.json with the defaults.
    Reset {
        /// Confirm the overwrite.
        #[arg(long)]
        yes: bool,
    },
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory as _, Parser as _};

    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("everythux-cli").chain(args.iter().copied()))
    }

    #[test]
    fn clap_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn global_options_work_after_the_subcommand() {
        let cli = parse(&["stats", "--db", "/x/i.db", "--config", "/x/c.json", "-vv"]).unwrap();
        assert_eq!(cli.db.as_deref(), Some(std::path::Path::new("/x/i.db")));
        assert_eq!(cli.config.as_deref(), Some(std::path::Path::new("/x/c.json")));
        assert_eq!(cli.verbose, 2);
    }

    #[test]
    fn search_joins_terms_and_parses_flags() {
        let cli = parse(&[
            "search", "foo", "bar|baz", "-n", "5", "--sort", "size", "--desc", "--json",
        ])
        .unwrap();
        let Command::Search(args) = cli.command else {
            panic!("not a search command")
        };
        assert_eq!(args.query, ["foo", "bar|baz"]);
        assert_eq!(args.limit, 5);
        assert_eq!(args.sort, SortKey::Size);
        assert!(args.desc && args.json && !args.null);
    }

    #[test]
    fn search_needs_a_query_and_json_excludes_null() {
        assert!(parse(&["search"]).is_err());
        assert!(parse(&["search", "x", "--json", "--null"]).is_err());
    }

    #[test]
    fn reindex_flags_select_the_job() {
        let job = |args: &[&str]| {
            let cli = parse(&[&["reindex"], args].concat()).unwrap();
            let Command::Reindex(r) = cli.command else {
                panic!("not reindex")
            };
            r.job().unwrap()
        };
        assert_eq!(job(&[]), Job::RescanAll(ScanMode::Quick));
        assert_eq!(job(&["--quick"]), Job::RescanAll(ScanMode::Quick));
        assert_eq!(job(&["--full"]), Job::FullReindex);
        assert_eq!(
            job(&["--volume", "/home", "--full"]),
            Job::RescanVolume {
                volume: "/home".into(),
                mode: ScanMode::Full
            }
        );
        assert_eq!(
            job(&["--path", "/srv/data"]),
            Job::RescanPath {
                path: "/srv/data".into(),
                mode: ScanMode::Quick
            }
        );
    }

    #[test]
    fn reindex_rejects_conflicting_flags() {
        assert!(parse(&["reindex", "--full", "--quick"]).is_err());
        assert!(parse(&["reindex", "--volume", "/", "--path", "/x"]).is_err());
    }

    #[test]
    fn exclude_add_parses_kind_and_scope() {
        let cli = parse(&["exclude", "add", "bin", "--kind", "dir", "--scope", "/home/*"]).unwrap();
        let Command::Exclude {
            action: ExcludeAction::Add { pattern, kind, scope },
        } = cli.command
        else {
            panic!("not exclude add")
        };
        assert_eq!(pattern, "bin");
        assert_eq!(RuleKind::from(kind), RuleKind::Dir);
        assert_eq!(scope.as_deref(), Some("/home/*"));
    }
}
