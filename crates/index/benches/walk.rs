//! `walk/synthetic`: raw walker throughput on a seeded synthetic tree (ROADMAP §9: ≥ 50k entries/s).
//!
//! The tree (200k files in 20k folders) is created once under `$EVERYTHUX_BENCH_DIR` (default:
//! the system temp dir) and walked with a warm page cache.

use std::path::PathBuf;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use everythux_index::test_utils::generate_tree;
use everythux_index::walker::{WalkControl, WalkOptions, dry_run};

const FILES: usize = 200_000;
const DIRS: usize = 20_000;

fn bench_tree() -> tempfile::TempDir {
    let base = std::env::var_os("EVERYTHUX_BENCH_DIR").map(PathBuf::from);
    let tmp = match base {
        Some(dir) => tempfile::Builder::new().prefix("everythux-bench").tempdir_in(dir),
        None => tempfile::tempdir(),
    }
    .expect("create bench dir");
    generate_tree(tmp.path(), FILES, DIRS, 1).expect("generate tree");
    tmp
}

fn walk(c: &mut Criterion) {
    let tree = bench_tree();
    let opts = WalkOptions::default();
    let mut group = c.benchmark_group("walk");
    group.throughput(Throughput::Elements((FILES + DIRS) as u64));
    group.sample_size(10);
    for (name, threads) in [("synthetic", 0), ("synthetic_1_thread", 1)] {
        group.bench_function(name, |b| {
            b.iter(|| {
                let summary = dry_run(tree.path(), &opts, &WalkControl::new(), threads).expect("walk");
                assert_eq!(summary.entries as usize, FILES + DIRS);
                std::hint::black_box(summary)
            });
        });
    }
    group.finish();
}

criterion_group!(benches, walk);
criterion_main!(benches);
