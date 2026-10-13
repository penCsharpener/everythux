//! `search`: query the index like Everything's `es.exe`.

use std::io::{BufWriter, Write as _};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use everythux_core::ids::EntryId;
use everythux_core::snapshot::IndexSnapshot;
use everythux_search::{QueryOptions, SearchIndex, Sort, parse};

use crate::cli::SearchArgs;
use crate::context::Env;
use crate::output::{self, ResultFormat};

pub(crate) fn run(env: &Env, args: &SearchArgs) -> Result<()> {
    // Open the store and parse before loading the snapshot, so a missing index or a syntax error
    // fails fast instead of after reading millions of rows.
    let store = env.open_existing_store()?;
    let query_text = args.query.join(" ");
    let options = QueryOptions {
        case_sensitive: args.case_sensitive,
        match_path: args.match_path,
        regex: args.regex,
    };
    let query = parse(&query_text, options).with_context(|| format!("invalid query {query_text:?}"))?;

    let snapshot = Arc::new(store.load_snapshot().context("cannot load the index")?);
    let index = SearchIndex::build(Arc::clone(&snapshot));
    let sort = Sort {
        column: args.sort.into(),
        ascending: !args.desc,
    };
    let Some(ids) = index.search(&query, sort, args.limit, &|| false) else {
        bail!("the search was cancelled");
    };

    write_stdout(&snapshot, &ids, format(args))
}

fn format(args: &SearchArgs) -> ResultFormat {
    if args.json {
        ResultFormat::Json
    } else if args.null {
        ResultFormat::Null
    } else {
        ResultFormat::Lines
    }
}

fn write_stdout(snapshot: &IndexSnapshot, ids: &[EntryId], format: ResultFormat) -> Result<()> {
    let mut out = BufWriter::new(std::io::stdout().lock());
    output::write_results(&mut out, snapshot, ids, format).context("cannot write results")?;
    out.flush().context("cannot write results")
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::*;
    use crate::cli::{Cli, Command};

    fn args(extra: &[&str]) -> SearchArgs {
        let cli = Cli::try_parse_from([&["everythux-cli", "search", "q"], extra].concat()).unwrap();
        let Command::Search(args) = cli.command else {
            panic!("not a search")
        };
        args
    }

    #[test]
    fn output_format_follows_flags() {
        assert_eq!(format(&args(&[])), ResultFormat::Lines);
        assert_eq!(format(&args(&["--json"])), ResultFormat::Json);
        assert_eq!(format(&args(&["-0"])), ResultFormat::Null);
    }
}
