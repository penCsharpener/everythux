//! `store/load_snapshot` (SQLite → in-memory index), `store/full_build` (walk + bulk load +
//! indexes + swap) and `index/quick_rescan_unchanged` (ROADMAP §9).

use std::path::PathBuf;
use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use everythux_core::config::{IndexerConfig, StartupRescan, VolumesConfig};
use everythux_index::test_utils::generate_tree;
use everythux_index::{IndexEvent, Indexer, IndexerSettings, Job, ScanMode, Store};

const FILES: usize = 200_000;
const DIRS: usize = 20_000;

fn bench_dir() -> tempfile::TempDir {
    let base = std::env::var_os("EVERYTHUX_BENCH_DIR").map(PathBuf::from);
    match base {
        Some(dir) => tempfile::Builder::new().prefix("everythux-bench").tempdir_in(dir),
        None => tempfile::tempdir(),
    }
    .expect("create bench dir")
}

fn settings(root: &std::path::Path) -> IndexerSettings {
    IndexerSettings {
        volumes: VolumesConfig {
            auto_discover_fs_types: vec![],
            include: vec![root.to_path_buf()],
            disabled_uuids: vec![],
        },
        exclusions: vec![],
        indexer: IndexerConfig {
            rescan_on_startup: StartupRescan::None,
            live_updates: false,
            ..IndexerConfig::default()
        },
    }
}

/// Waits for `Finished` of the first job (the automatic scan of an empty database).
fn wait_finished(rx: &crossbeam_channel::Receiver<IndexEvent>) {
    loop {
        match rx.recv_timeout(Duration::from_secs(600)).expect("indexer event") {
            IndexEvent::Finished { .. } => return,
            IndexEvent::Failed { error, .. } => panic!("{error}"),
            _ => {}
        }
    }
}

fn store_benches(c: &mut Criterion) {
    let tree = bench_dir();
    generate_tree(tree.path(), FILES, DIRS, 1).expect("generate tree");
    let work = bench_dir();
    let db = work.path().join("index.db");
    let entries = (FILES + DIRS) as u64;

    // Full build through the indexer: walk, bulk load, indexes, totals, ANALYZE, swap.
    let (indexer, rx) = Indexer::start(&db, settings(tree.path())).expect("start");
    wait_finished(&rx);

    let mut group = c.benchmark_group("store");
    group.throughput(Throughput::Elements(entries));
    group.sample_size(10);
    group.bench_function("full_build", |b| {
        b.iter(|| {
            let id = indexer.submit(Job::FullReindex);
            loop {
                match rx.recv_timeout(Duration::from_secs(600)).expect("indexer event") {
                    IndexEvent::Finished { job, .. } if job == id => break,
                    IndexEvent::Failed { error, .. } => panic!("{error}"),
                    _ => {}
                }
            }
        });
    });
    group.bench_function("quick_rescan_unchanged", |b| {
        b.iter(|| {
            let id = indexer.submit(Job::RescanAll(ScanMode::Quick));
            loop {
                match rx.recv_timeout(Duration::from_secs(600)).expect("indexer event") {
                    IndexEvent::Finished { job, .. } if job == id => break,
                    IndexEvent::Failed { error, .. } => panic!("{error}"),
                    _ => {}
                }
            }
        });
    });
    drop(indexer);

    let store = Store::open(&db).expect("open");
    eprintln!(
        "database: {} bytes for {entries} entries",
        store.stats().expect("stats").db_bytes
    );
    group.bench_function("load_snapshot", |b| {
        b.iter(|| std::hint::black_box(store.load_snapshot().expect("snapshot")));
    });
    group.finish();
}

criterion_group!(benches, store_benches);
criterion_main!(benches);
