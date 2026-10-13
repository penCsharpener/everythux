---
name: rust-performance
description: How to write and verify fast Rust in everythux. Covers measuring first, allocation, data layout, parallelism, filesystem I/O, SQLite throughput and the egui frame budget. Use when working on hot paths (walker, indexer, search, sorting, table rendering, preview decoding), when a performance target in ROADMAP §9 is at stake, or when asked to optimise, profile or benchmark.
---

# Performance in everythux

Everythux's value is speed: about 50 ms from keystroke to results on 2M entries, and a full index in
under 60 s. Treat the budgets in `ROADMAP.md` §9 as tests.

## 1. Measure, then change

1. **Reproduce with a benchmark first.** Use `criterion` benches in `crates/<crate>/benches/`, on the
   synthetic corpus (1M files / 100k dirs, from the generator in `index`'s test utilities). Wrap inputs
   and outputs in `std::hint::black_box`.
2. **Profile in a release build with debug info.** The workspace sets `[profile.release] debug = "line-tables-only"`
   and a `[profile.profiling]` that inherits release with `debug = true`.
   - CPU: `cargo flamegraph --profile profiling -p everythux-cli -- search foo`, or `perf record -g` and `perf report`.
   - Allocations: `heaptrack`, or `dhat` (the `dhat` crate behind a `dhat-heap` feature).
   - Syscalls and I/O: `strace -c -f`, and `perf trace`.
3. **Change one thing, re-run the bench, and record the before and after numbers** in the commit message.
   No "should be faster" without a number.
4. Compare against a cold cache when it matters: `sync; echo 3 | sudo tee /proc/sys/vm/drop_caches`.

## 2. Allocation discipline

- No allocation per entry in the walker or search loop. Reuse buffers: call `clear()` and refill rather
  than creating new ones.
- `Vec::with_capacity` / `String::with_capacity` when the size is known or estimable. Use `reserve` before
  bulk `extend`.
- Store many small strings as **one arena**: a `Vec<u8>` of concatenated names plus a `Vec<u32>` of offsets.
  This is the `MemSearch` layout. It uses about 3× less memory than `Vec<String>` and is cache friendly.
- `SmallVec`/`ArrayVec` for small, bounded collections on hot paths. `Box<str>`/`Box<[T]>` for immutable data
  (no capacity field).
- Avoid `format!` in hot loops. Use `write!` into a reused `String`, or `itoa`/`ryu` for numbers.
- Return iterators or write into `&mut Vec<T>` out-parameters instead of allocating and returning a new
  `Vec` each call.

## 3. Data layout

- **Struct of arrays** for columns that are scanned (`names`, `sizes`, `mtimes`, `dir_ids`, `kinds`).
  The search loop touches only the arrays it needs.
- Use the smallest correct type: `u32` IDs (4 billion entries is enough), `u8`-backed enums
  (`#[repr(u8)]`), and `i64` nanoseconds for times instead of `SystemTime`.
- Sort permutations as `Vec<u32>`, precomputed per column. Re-sorting then becomes a filtered walk.
- `FxHashMap`/`ahash` for internal hash maps with non-adversarial keys. Keep SipHash (the std default)
  where keys come from outside.
- Check sizes of hot types with `const _: () = assert!(size_of::<Entry>() <= 24);`.

## 4. Search hot path

- Fold case once at index time (`name_fold`), not per query.
- `memchr::memmem::Finder`, built once per query and reused across chunks. For multi-term queries, test
  the rarest term first.
- Parallelise over chunks with `rayon` (`par_chunks`, chunk size of about 64k entries), collecting into a
  bitset (`fixedbitset`) or per-chunk `Vec<u32>`s that are concatenated afterwards.
- Cancellation: check an `AtomicU64` generation counter once per chunk, and bail out if it changed.
- Only materialise the visible page of results for the UI (row text, formatted size and date). Never
  format 2M rows.

## 5. Filesystem I/O (walker)

- Parallel directory traversal (`jwalk` first, then a `rustix` walker: `openat` + `getdents64` +
  `statx(AT_STATX_DONT_SYNC)` relative to the directory fd, which avoids path resolution).
- Use `d_type` from `getdents64` to skip `stat` for type detection. Only `stat` when size or mtime are needed.
- Stat children in **inode order**. It is a large win on HDDs and harmless on SSDs.
- Prune excluded directories before opening them.
- Bounded channel to the writer, so memory can't grow without limit when SQLite is slower than the walker.

## 6. SQLite throughput (`rusqlite`)

- `PRAGMA journal_mode=WAL; synchronous=NORMAL; temp_store=MEMORY; cache_size=-65536;` (64 MB) on the
  writer, and `mmap_size` on readers.
- Insert inside explicit transactions of about 10k rows. Use `prepare_cached` and re-bind parameters,
  never build SQL strings.
- Bulk build: create secondary indexes **after** loading, then run `ANALYZE`.
- Use `EXPLAIN QUERY PLAN` to confirm every query in a hot path uses an index. Add the plan to the PR when
  you introduce a query.
- Store blobs and integers rather than formatted text. Read with `row.get_ref()` to avoid copying when you
  only inspect the value.

## 7. UI frame budget (egui)

- `update()` must finish in under 4 ms at 2M results. Everything heavy happens on workers (see CLAUDE.md).
- Virtualised table: `TableBuilder::body().rows(row_height, total, |row| …)` renders only visible rows.
- Cache formatted cell strings for the visible window. Invalidate only when the page or results change.
- Upload textures (previews, icons) once and keep the `TextureHandle`. Never re-upload per frame.
- Call `ctx.request_repaint()` only when new data arrived. No continuous repaint while idle (target
  about 0% idle CPU).

## 8. Build configuration

```toml
[profile.release]
lto = "thin"
codegen-units = 1
panic = "abort"        # binaries only; fine since we don't catch panics
debug = "line-tables-only"

[profile.profiling]
inherits = "release"
debug = true
```

Don't enable `target-cpu=native` in committed config, because release builds must run on any x86-64-v2
machine. Use it locally only to experiment.

## 9. Anti-patterns to flag in review

- `.collect::<Vec<_>>()` immediately followed by `.iter()`.
- `clone()` of large `Vec`s or `String`s inside loops.
- Locking a `Mutex` per item in a parallel loop. Use thread-local accumulation and merge instead.
- `to_string_lossy().to_string()` on hot paths. Keep bytes.
- Sorting the whole result set when only the top page is needed (use `select_nth_unstable` or the
  permutation approach).
- `std::fs::metadata` on a full path when you already hold the directory fd.
