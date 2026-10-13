//! Prints min / median / p95 latency per query on a synthetic corpus, plus build and column re-sort
//! times and memory. The minimum is robust against a busy machine, the p95 is the ROADMAP §9 metric.
//!
//! `cargo run --release -p everythux-search --example latency -- [entries] [runs]`

#![expect(clippy::print_stdout, reason = "this example reports its results on stdout")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use everythux_core::config::ColumnId;
use everythux_search::synthetic::generate;
use everythux_search::{QueryOptions, SearchIndex, Sort, parse};

fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("VmRSS:"))
                .and_then(|v| v.split_whitespace().next().and_then(|n| n.parse::<u64>().ok()))
        })
        .map_or(0, |kb| kb / 1024)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn main() {
    let mut args = std::env::args().skip(1);
    let entries: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(2_000_000);
    let runs: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(40);

    let started = Instant::now();
    let snapshot = Arc::new(generate(entries, 42));
    println!(
        "generated {entries} entries in {:.0} ms, snapshot heap {} MB",
        ms(started.elapsed()),
        snapshot.heap_size() >> 20
    );
    let rss_snapshot = rss_mb();

    let mut best_build = Duration::MAX;
    let mut index = None;
    for _ in 0..3 {
        let started = Instant::now();
        let built = SearchIndex::build(Arc::clone(&snapshot));
        best_build = best_build.min(started.elapsed());
        index = Some(built);
    }
    let index = index.expect("built at least once");
    println!(
        "build (name order): best of 3 = {:.0} ms, RSS {} MB -> {} MB",
        ms(best_build),
        rss_snapshot,
        rss_mb()
    );

    for column in [ColumnId::Size, ColumnId::Modified, ColumnId::Type, ColumnId::Path] {
        let mut best = Duration::MAX;
        for _ in 0..3 {
            let fresh = SearchIndex::build(Arc::clone(&snapshot));
            let started = Instant::now();
            let _ = fresh.permutation(column);
            best = best.min(started.elapsed());
        }
        println!("first sort by {column:?}: best of 3 = {:.0} ms", ms(best));
    }
    index.warm();
    println!(
        "permutations: {} MB, RSS with all columns {} MB",
        index.permutation_heap_size() >> 20,
        rss_mb()
    );

    let queries: &[(&str, &str)] = &[
        ("substring", "report"),
        ("rare substring", "qzx"),
        ("1-char substring", "e"),
        ("multi term", "holiday draft"),
        ("or + not", "photo|invoice !copy"),
        ("wildcard", "img_*0.jpg"),
        ("suffix wildcard", "*.tar.gz"),
        ("ext + size", "ext:pdf size:>1mb"),
        ("substring + ext", "report ext:jpg;png"),
        ("path:", "path:projects/src draft"),
        ("vol:", "vol:data report"),
        ("regex", "regex:^img_\\d{3}0\\.jpg$"),
        ("dm:", "dm:2025 report"),
        ("match all", ""),
    ];
    println!(
        "\n{:<18} {:>9} {:>8} {:>8} {:>8}",
        "query", "results", "min ms", "p50 ms", "p95 ms"
    );
    for sort in [
        Sort::default(),
        Sort {
            column: ColumnId::Size,
            ascending: false,
        },
    ] {
        println!("-- sort {:?} ascending={}", sort.column, sort.ascending);
        for (label, text) in queries {
            let query = parse(text, QueryOptions::default()).expect("query parses");
            let mut times = Vec::with_capacity(runs);
            let mut hits = 0;
            for _ in 0..runs {
                let started = Instant::now();
                hits = index
                    .search(&query, sort, 0, &|| false)
                    .map_or(0, |ids| ids.len());
                times.push(started.elapsed());
            }
            times.sort_unstable();
            let p95 = times[(times.len() * 95).div_ceil(100).saturating_sub(1)];
            println!(
                "{label:<18} {hits:>9} {:>8.2} {:>8.2} {:>8.2}",
                ms(times[0]),
                ms(times[times.len() / 2]),
                ms(p95)
            );
        }
    }
}
