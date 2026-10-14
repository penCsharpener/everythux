//! Search benchmarks on a synthetic 2M-entry corpus (ROADMAP §9: p95 <= 50 ms per query,
//! column re-sort <= 100 ms).

use std::hint::black_box;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use everythux_core::config::ColumnId;
use everythux_core::snapshot::IndexSnapshot;
use everythux_search::synthetic::generate;
use everythux_search::{QueryOptions, SearchIndex, Sort, parse, parse_many};

const ENTRIES: usize = 2_000_000;

static SNAPSHOT: LazyLock<Arc<IndexSnapshot>> = LazyLock::new(|| Arc::new(generate(ENTRIES, 42)));

static INDEX: LazyLock<SearchIndex> = LazyLock::new(|| {
    let index = SearchIndex::build(Arc::clone(&SNAPSHOT));
    index.warm();
    index
});

fn bench_query(c: &mut Criterion, name: &str, text: &str, options: QueryOptions, sort: Sort, limit: usize) {
    let index = &*INDEX;
    let query = parse(text, options).expect("benchmark query parses");
    let hits = index
        .search(&query, sort, limit, &|| false)
        .map_or(0, |ids| ids.len());
    eprintln!("{name}: {text:?} -> {hits} results");
    c.bench_function(name, |b| {
        b.iter(|| black_box(index.search(black_box(&query), sort, limit, &|| false)));
    });
}

fn search(c: &mut Criterion) {
    let opts = QueryOptions::default();
    let by_name = Sort::default();
    bench_query(c, "search/substring_2m", "report", opts, by_name, 0);
    bench_query(c, "search/rare_substring_2m", "qzx", opts, by_name, 0);
    bench_query(c, "search/common_substring_2m", "e", opts, by_name, 0);
    bench_query(c, "search/multi_term_2m", "holiday draft", opts, by_name, 0);
    bench_query(c, "search/or_not_2m", "photo|invoice !copy", opts, by_name, 0);
    bench_query(c, "search/wildcard_2m", "img_*0.jpg", opts, by_name, 0);
    bench_query(c, "search/suffix_wildcard_2m", "*.tar.gz", opts, by_name, 0);
    bench_query(c, "search/ext_size_2m", "ext:pdf size:>1mb", opts, by_name, 0);
    bench_query(
        c,
        "search/ext_substring_2m",
        "report ext:jpg;png",
        opts,
        by_name,
        0,
    );
    bench_query(c, "search/path_2m", "path:projects/src draft", opts, by_name, 0);
    bench_query(c, "search/vol_2m", "vol:data report", opts, by_name, 0);
    bench_query(
        c,
        "search/regex_2m",
        "regex:^img_\\d{3}0\\.jpg$",
        opts,
        by_name,
        0,
    );
    bench_query(
        c,
        "search/match_path_2m",
        "projects draft",
        QueryOptions {
            match_path: true,
            ..opts
        },
        by_name,
        0,
    );
    bench_query(c, "search/limit_1000_2m", "e", opts, by_name, 1000);
    let by_size_desc = Sort {
        column: ColumnId::Size,
        ascending: false,
    };
    bench_query(c, "search/match_all_sorted_2m", "", opts, by_size_desc, 0);
    bench_query(
        c,
        "search/substring_sorted_size_2m",
        "report",
        opts,
        by_size_desc,
        0,
    );
    let by_path = Sort {
        column: ColumnId::Path,
        ascending: true,
    };
    bench_query(c, "search/substring_sorted_path_2m", "report", opts, by_path, 0);
}

/// Several search bars against the same terms typed into one bar (budget: two or three bars add
/// at most 10 % latency). Parsing is inside the loop for every variant, because the service
/// parses each request, and bars parse once per bar.
fn multi_bar(c: &mut Criterion) {
    let index = &*INDEX;
    let opts = QueryOptions::default();
    let sort = Sort::default();
    let cases: [(&str, &[&str]); 3] = [
        ("one_bar", &["report ext:jpg;png dm:2020-01-01..2030-01-01"]),
        ("two_bars", &["report", "ext:jpg;png dm:2020-01-01..2030-01-01"]),
        (
            "three_bars",
            &["report", "ext:jpg;png", "dm:2020-01-01..2030-01-01"],
        ),
    ];
    let mut group = c.benchmark_group("search/multi_bar_2m");
    for (name, texts) in cases {
        let query = parse_many(texts, opts).expect("benchmark bars parse");
        let hits = index
            .search(&query, sort, 0, &|| false)
            .map_or(0, |ids| ids.len());
        eprintln!("search/multi_bar_2m/{name}: {texts:?} -> {hits} results");
        group.bench_function(name, |b| {
            b.iter(|| {
                let query = parse_many(black_box(texts), opts).expect("benchmark bars parse");
                black_box(index.search(&query, sort, 0, &|| false))
            });
        });
    }
    group.finish();
}

fn sorting(c: &mut Criterion) {
    let mut group = c.benchmark_group("sort");
    group.sample_size(10).warm_up_time(Duration::from_millis(500));
    for (name, column) in [
        ("size_2m", ColumnId::Size),
        ("modified_2m", ColumnId::Modified),
        ("path_2m", ColumnId::Path),
        ("type_2m", ColumnId::Type),
    ] {
        group.bench_function(name, |b| {
            b.iter_batched(
                || SearchIndex::build(Arc::clone(&SNAPSHOT)),
                |index| black_box(index.permutation(column).len()),
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();
}

fn build(c: &mut Criterion) {
    let mut group = c.benchmark_group("build");
    group.sample_size(10).warm_up_time(Duration::from_millis(500));
    group.bench_function("2m", |b| {
        b.iter(|| black_box(SearchIndex::build(Arc::clone(&SNAPSHOT))));
    });
    group.finish();
    let index = &*INDEX;
    eprintln!(
        "snapshot heap: {} MB, permutations (all 5 columns + folded dir paths): {} MB",
        SNAPSHOT.heap_size() >> 20,
        index.permutation_heap_size() >> 20
    );
}

criterion_group!(benches, search, multi_bar, sorting, build);
criterion_main!(benches);
