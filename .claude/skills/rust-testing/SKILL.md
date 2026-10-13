---
name: rust-testing
description: Testing strategy and patterns for the everythux Rust workspace. Covers unit, integration, property, snapshot and egui UI tests, real-ext4 loop-mount tests, D-Bus mocks and criterion benchmarks. Use when adding a feature or fixing a bug (write the test first), when a test is flaky, or when deciding how to test filesystem, SQLite, UI or desktop-integration code.
---

# Testing in everythux

Every behaviour change ships with a test that fails before the change and passes after it. A bug fix
starts with a test that reproduces the bug.

## 1. Where tests go

| Kind | Location | Use for |
|------|----------|---------|
| Unit | `#[cfg(test)] mod tests` at the bottom of the file | Pure logic: query parser, exclusion matching, size/date formatting, config migration |
| Integration | `crates/<crate>/tests/*.rs` | Public API of a crate: walker + store on temp trees, search over a real index |
| Doc tests | `///` examples | Public API usage. Keep them short and make sure they compile |
| Benchmarks | `crates/<crate>/benches/*.rs` (`criterion`, `harness = false`) | Hot paths with budgets (ROADMAP §9) |
| Root-only | `#[ignore = "needs root: loop-mount ext4"]` | Real ext4 images, fanotify. Run in CI with `-- --ignored` |

## 2. Tooling

- `cargo nextest run --workspace` locally and in CI. It is faster, isolates tests per process and
  reports flaky ones. Fall back to `cargo test`.
- `tempfile::TempDir` for every filesystem test. Never touch `$HOME` or fixed paths.
- `proptest` for parsers and matchers: queries, exclusion globs, path normalisation, config round-trip.
- `insta` snapshots for complex outputs: parsed query ASTs, `EXPLAIN QUERY PLAN` text, rendered config
  JSON. Review snapshot changes with `cargo insta review`. Never accept them blindly.
- `egui_kittest` for UI behaviour tests.
- `rstest` for table-driven cases when a plain loop over an array gets unreadable.

## 3. Patterns

**Table-driven matching tests:**
```rust
#[test]
fn exclusion_matching() {
    let rules = Rules::compile(&[rule("node_modules", Kind::Dir), rule_scoped("bin", Kind::Dir, "/home/*")]).unwrap();
    for (path, excluded) in [
        ("/home/a/proj/node_modules", true),
        ("/home/a/proj/bin", true),
        ("/usr/bin", false),               // scoping protects system dirs
        ("/home/a/node_modules.txt", false),
    ] {
        assert_eq!(rules.is_excluded(Path::new(path), true), excluded, "{path}");
    }
}
```

**Filesystem fixtures:** use a small builder in `index`'s `test-utils` feature, such as
`TreeBuilder::new(&tmp).dir("a/b").file("a/b/c.txt", 10).symlink("loop", ".")`. Always include these
edge cases in walker tests:
- invalid UTF-8 names (`OsStr::from_bytes(b"bad\xffname")`),
- 255-byte names and deep nesting (more than 100 levels),
- symlink loops and dangling symlinks,
- directories without read or execute permission (skip when running as root: `if rustix::process::geteuid().is_root()`),
- files changing during the walk (created or deleted between `readdir` and `stat`).

**SQLite:** use `Connection::open_in_memory()` for store logic. Use a file in `TempDir` when testing WAL,
the atomic rename-swap or crash safety. Run migrations in tests the same way production does.

**Concurrency:** test cancellation and backpressure deterministically. Use bounded channels with capacity 1
and explicit barriers, not `sleep`. A test that needs `sleep` to pass is a flaky test.

**UI (`egui_kittest`):** drive the real `App` with a fake engine. Assert on effects, not pixels:
- a double-click on a Path cell emits `Action::ShowInFolder(path)`,
- a double-click on a Size or Modified cell emits `Action::ShowProperties(folder)`,
- `Ctrl+Shift+R` emits `Action::ReindexAll` even while the search box has focus,
- a Ctrl+Shift+click on a Path cell opens the exclusion popup prefilled with that path.

Platform side effects sit behind an `ActionSink` at the `platform` boundary. That is the one place where
a test fake is justified.

**D-Bus:** run a mock `org.freedesktop.FileManager1` on a private bus (`dbus-run-session -- cargo test -p everythux-platform`)
and assert that the received URIs are correctly percent-encoded `file://` URIs, including for non-UTF-8 names.

**Real ext4 (CI, root):**
```sh
truncate -s 64M ext4.img && mkfs.ext4 -q ext4.img
sudo mount -o loop ext4.img /mnt/evx && sudo chown "$USER" /mnt/evx
cargo nextest run -p everythux-index --run-ignored only
```
Use it to check discovery via mountinfo, UUID resolution, bind-mount dedupe and not crossing `st_dev`.

## 4. Benchmarks

- `criterion` with a fixed synthetic corpus (seeded RNG) so results are comparable across runs.
- Name benches after the budget they protect: `search/substring_2m`, `walk/synthetic_1m`, `sort/size_2m`.
- CI compares against the base branch and fails on a regression of more than 15%.

## 5. Rules

- Tests are deterministic, run in parallel, and leave no files behind.
- One behaviour per test. The name says what is checked: `quick_rescan_skips_unchanged_dirs`.
- Assert with messages that identify the failing case (`assert_eq!(a, b, "{path}")`).
- Never weaken, skip or `#[ignore]` a failing test to get green. Fix the code or the test's wrong assumption.
- Test the public behaviour, not private helpers, unless the helper is a pure algorithm worth testing directly.
