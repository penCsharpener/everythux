# CLAUDE.md

Everythux is an "Everything"-style instant file search for Linux. It is written in Rust, focused on ext4,
and persists its index in SQLite. Read **`ROADMAP.md`** before starting work: it holds the architecture,
the SQLite schema, the JSON config format and the phase plan. Keep it up to date when a decision changes.

## Status

v0.1 to v0.4 of the roadmap are implemented. The open items are listed in the "Implementation status"
section at the top of `ROADMAP.md`. When you finish a roadmap task, tick its checkbox in `ROADMAP.md`
in the same commit.

## Workspace layout

```
crates/core      domain types, config (JSON), exclusion rules, errors
crates/index     volume discovery, walker, SQLite store, migrations, rescans
crates/search    query parser, in-memory search index, sorting
crates/platform  D-Bus FileManager1, xdg-open, portals, thumbnails, udisks2
crates/preview   preview providers (feature-gated)
crates/app       eframe/egui UI          -> binary `everythux`
crates/cli       headless CLI            -> binary `everythux-cli`
```

Dependency direction: `app`/`cli` → `search`, `index`, `platform`, `preview` → `core`.
`core` depends on no other workspace crate. Only `app` depends on egui.

## Commands

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
dbus-run-session -- cargo test --workspace --all-features   # D-Bus mock tests need a session bus
cargo test --workspace --all-features -- --ignored        # root-only: loop-mounted ext4, D-Bus
cargo bench -p everythux-search          # criterion benches (perf-sensitive changes)
cargo deny check                         # licenses + advisories
cargo run -p everythux-app               # the UI
cargo run -p everythux-cli -- --help
```

Run fmt, clippy and tests before every commit. The `rust-verify` skill has the full gate.

## Skills in this repo

Load the matching skill before doing the work it covers:

- `.claude/skills/modern-rust`: idioms, API design, error handling, edition-2024 features, `unsafe` policy.
- `.claude/skills/rust-performance`: measuring, allocation, data layout, parallelism, SQLite and egui budgets.
- `.claude/skills/rust-testing`: unit/integration/property/snapshot/UI tests and benchmarks.
- `.claude/skills/rust-verify`: the pre-commit gate and the workspace lint configuration.
- `.claude/skills/version-bump`: after each fix or feature, bump the workspace version (feature = minor,
  fix = build number), update `ROADMAP.md`, and commit it.

## Project rules

These are specific to Everythux and override general habits.

- **Paths are bytes.** Linux filenames can be invalid UTF-8. Use `OsStr`/`Path`/`&[u8]`
  (`std::os::unix::ffi::OsStrExt`) end to end. Convert to `String` only for display, with
  `to_string_lossy()`. Never use the lossy string to open, select or delete a file.
- **Never block the UI thread.** No filesystem, SQLite, D-Bus or decoding work inside `eframe::App::update`.
  Send it to a worker over a channel and call `ctx.request_repaint()` when results arrive.
- **One SQLite writer.** All writes go through the single writer thread in `index`. Readers use their own
  connections (WAL mode). Batch inserts in transactions of about 10k rows, and use cached prepared
  statements (`prepare_cached`).
- **The index must survive crashes.** Full rebuilds write to `index.db.building` and atomically rename it
  over `index.db`. Config writes go to a temp file, then `fsync`, then rename.
- **Config compatibility.** Every config struct derives `Default` and uses `#[serde(default)]`. Don't use
  `deny_unknown_fields`. Bump `version` and add a migration for breaking changes.
- **Stay on one filesystem.** The walker never crosses `st_dev` and never follows symlinks unless the
  config says so.
- **Exclusions prune.** Match exclusions before descending into a directory, never by filtering afterwards.
- **Performance budgets** (ROADMAP §9) are acceptance criteria. A change that touches a hot path
  (walker, search, sort, table rendering) comes with a benchmark number in the commit or PR description.
- **Desktop integration** goes through `platform`. Use the D-Bus `org.freedesktop.FileManager1` first,
  then portals, then `xdg-open`. Other crates never shell out directly.

## Conventions

- Edition 2024, stable toolchain pinned in `rust-toolchain.toml`. Workspace-level `[workspace.dependencies]`
  and `[workspace.lints]`, which every crate inherits.
- Errors: `thiserror` enums in library crates, `anyhow` only in `app` and `cli`. No `unwrap()`/`expect()`
  in library code except for documented invariants (`expect("reason the invariant holds")`).
- Logging: `tracing` macros with structured fields (`tracing::info!(volume = %uuid, entries, "scan done")`).
  Never use `println!` outside `cli`.
- `unsafe` only in `index` (raw syscalls) and `platform` (FFI), and every block needs a `// SAFETY:` comment.
- Commit messages: imperative subject of 72 characters or fewer, with a body explaining *why*.
