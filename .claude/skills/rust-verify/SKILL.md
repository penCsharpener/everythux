---
name: rust-verify
description: The verification gate every everythux change must pass before commit or push (fmt, clippy with workspace lints, tests, docs, cargo-deny, benches for hot paths), plus the canonical workspace Cargo.toml lint, profile and toolchain configuration. Use before committing or pushing Rust changes, when setting up or changing the workspace manifests or CI, or when clippy or CI fails.
---

# Verify before you commit

Run the gate from the workspace root. Stop at the first failure, fix it, and re-run from the top.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace --all-features   # or: cargo test --workspace --all-features
cargo test --workspace --doc
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
cargo deny check                                # if deny.toml exists
```

Additionally:
- **Hot path touched** (walker, search, sort, table, SQLite queries): run the relevant `cargo bench`
  and put the before and after numbers in the commit message.
- **Dependency added or changed:** run `cargo deny check` and `cargo tree -d` to look for duplicate
  versions. Commit `Cargo.lock`.
- **Public API changed:** update the doc comments and `ROADMAP.md` if a decision changed.
- **Finished a roadmap task:** tick its checkbox in `ROADMAP.md`.

Then re-read your own diff as a hostile reviewer would. Look for leftover `dbg!`/`println!`, `unwrap()`
in library code, commented-out code, TODOs without an issue, and accidental `pub`.

## Fixing clippy properly

- Fix the code. Don't silence the lint.
- If a lint is genuinely wrong for a spot, use `#[expect(clippy::lint_name, reason = "why it's fine here")]`
  on the smallest possible item. Never `#[allow]` at crate level, and never add `-A` flags to the CI command.
- `clippy::pedantic` findings that conflict with readability can be turned off workspace-wide in
  `[workspace.lints.clippy]`, with a comment explaining why. That's a team decision, so mention it in the
  commit message.

## Canonical workspace configuration

Use this when creating or adjusting the root `Cargo.toml` (Phase 0). Every member crate inherits it with
`[lints] workspace = true`, and package metadata with `edition.workspace = true` and so on.

```toml
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
edition = "2024"
rust-version = "1.88"          # bump deliberately. Let chains and Vec::extract_if need ≥ 1.88
license = "MIT OR Apache-2.0"
repository = "https://github.com/penCsharpener/everythux"

[workspace.lints.rust]
unsafe_code = "deny"           # index/platform: `#![expect(unsafe_code, reason = "…")]` on the one module that needs it
unsafe_op_in_unsafe_fn = "deny"
missing_debug_implementations = "warn"
unreachable_pub = "warn"
unused_qualifications = "warn"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
# pedantic opt-outs (each needs a reason):
module_name_repetitions = "allow"  # `index::IndexError` reads fine
must_use_candidate = "allow"       # too noisy. Add #[must_use] by hand where ignoring is a bug
missing_errors_doc = "allow"       # enforced in review for public APIs instead
missing_panics_doc = "allow"       # same
cast_possible_truncation = "allow" # u32 ids / u64->i64 timestamps are pervasive and bounded
cast_sign_loss = "allow"
cast_precision_loss = "allow"      # display-only float maths (sizes, percentages)
# restriction lints we want:
unwrap_used = "warn"               # allowed in tests via clippy.toml (below)
dbg_macro = "warn"
print_stdout = "warn"              # cli: `#![expect(clippy::print_stdout, reason = "CLI output")]` in its output module
todo = "warn"
undocumented_unsafe_blocks = "warn"
allow_attributes = "warn"          # use #[expect] instead
rc_buffer = "warn"

[profile.release]
lto = "thin"
codegen-units = 1
panic = "abort"
debug = "line-tables-only"

[profile.profiling]
inherits = "release"
debug = true

[profile.dev.package."*"]
opt-level = 2                  # fast deps (egui, image, sqlite) in debug builds
```

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "1.99.0"   # exact version: CI and local get the same lints
components = ["rustfmt", "clippy"]
```

Bump the pinned version deliberately, in its own commit, after fixing whatever new clippy lints it
brings. A floating `stable` lets CI pick up new lints on its own and go red without a code change.

Member crates can't override individual lints when they use `[lints] workspace = true`. Use a scoped
`#[expect(...)]` on the module that needs the exception instead.

`clippy.toml`:
```toml
allow-unwrap-in-tests = true
allow-expect-in-tests = true
allow-print-in-tests = true
doc-valid-idents = ["SQLite", "FileManager1", "UDisks2", "DBus", "PipeWire", "GStreamer", "OpenURI", "XDG", ".."]
```

`rustfmt.toml` stays minimal: `edition = "2024"` and `max_width = 110`. Don't add nightly-only options.

## CI mirror

CI (`.github/workflows/ci.yml`) runs the same gate on `ubuntu-latest`. It uses `Swatinem/rust-cache`,
installs `cargo-nextest` and `cargo-deny` via `taiki-e/install-action`, and has an extra job for the
root-only ext4 loop-mount tests (`--run-ignored only`). If the gate passes locally and fails in CI, the
cause is an environment difference. Fix the cause and don't retry blindly.
