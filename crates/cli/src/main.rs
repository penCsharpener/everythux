//! everythux-cli: headless access to the everythux index.
//!
//! Like Everything's `es.exe`: search from scripts, trigger (partial) re-indexes, inspect the
//! index and edit the exclusion list without starting the desktop UI.

#![forbid(unsafe_code)]

mod cli;
mod commands;
mod context;
mod output;
mod progress;
mod render;

use std::process::ExitCode;

use clap::Parser as _;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    init_tracing(cli.verbose);
    match commands::run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if is_broken_pipe(&err) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// `-v` wins over `RUST_LOG`; without either only warnings are shown.
fn init_tracing(verbose: u8) {
    let filter = match verbose {
        0 => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        1 => EnvFilter::new("info"),
        2 => EnvFilter::new("debug"),
        _ => EnvFilter::new("trace"),
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}

/// `everythux-cli search … | head` closes the pipe early; that is not an error.
fn is_broken_pipe(err: &anyhow::Error) -> bool {
    err.chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
}
