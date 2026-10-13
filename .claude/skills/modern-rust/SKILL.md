---
name: modern-rust
description: Idiomatic, modern (edition 2024) Rust for this workspace. Covers API design, error handling, ownership, traits, modules, dependencies and the unsafe policy. Use when writing or refactoring any Rust code in everythux, designing a public type or function, choosing between language features, or reviewing Rust for idiom and correctness.
---

# Modern Rust in everythux

Write code a strong Rust reviewer would accept without comments: simple, explicit, and taking advantage
of the type system. Prefer the standard library, then a well-known crate, then hand-rolled code.

## 1. Use current language features

Edition 2024 on stable. Prefer these over older patterns:

| Instead of | Write |
|------------|-------|
| `lazy_static!` / `once_cell::Lazy` | `std::sync::LazyLock` / `std::sync::OnceLock` |
| nested `if let` / `match` to bail early | `let Some(x) = opt else { return … };` (let-else) |
| `if let Some(a) = x { if cond(a) { … } }` | `if let Some(a) = x && cond(a) { … }` (let chains, edition 2024) |
| `#[allow(lint)]` | `#[expect(lint, reason = "…")]`, which warns when it's no longer needed |
| `Box<dyn Iterator>` returned from a trait | `fn iter(&self) -> impl Iterator<Item = T> + '_` (RPITIT) |
| `async-trait` crate | native `async fn` in traits (when async is needed at all) |
| `opt.map_or(true, f)` | `opt.is_none_or(f)` |
| `s.find(c).map(…)` to split once | `s.split_once(c)` |
| manual `retain`, then collect the removed items | `vec.extract_if(.., pred)` |
| `iter::repeat(x).take(n)` | `iter::repeat_n(x, n)` |
| hand-rolled grouping of adjacent items | `slice.chunk_by(pred)` |
| `.map_err(|e| { log(e); e })` | `.inspect_err(|e| …)` |

In edition 2024, `unsafe extern` blocks and `#[unsafe(no_mangle)]` are required, and
`unsafe_op_in_unsafe_fn` is on: every unsafe operation needs its own `unsafe {}` block, even
inside an `unsafe fn`.

## 2. Types carry meaning

- **Newtypes for IDs and units:** `struct EntryId(u32)`, `struct DirId(u32)`, `struct VolumeUuid(…)`. Never pass
  bare `u32`/`i64` across module boundaries when two of them could be swapped by accident.
- **Enums instead of booleans** for modes: `Rescan::Quick | Rescan::Full`, not `quick: bool`.
- **Make invalid states unrepresentable:** parse at the boundary (`ExclusionRule::parse(&str) -> Result<…>`),
  then pass the validated type. Don't re-validate deep inside.
- `#[non_exhaustive]` on public enums and structs that will grow (config, events, errors).
- Derive what's cheap and correct: `Debug` always; `Clone, Copy, PartialEq, Eq, Hash` where they make sense;
  `Default` for config.
- Prefer `&str`/`&[T]`/`&Path` parameters over `&String`/`&Vec<T>`/`&PathBuf`. Accept `impl AsRef<Path>`
  only at public entry points, and convert once.
- Return owned data when the caller needs ownership, and borrow otherwise. Use `Cow<'_, str>` when a value
  is usually borrowed but sometimes changed, like the output of `to_string_lossy`.

## 3. Error handling

- Library crates: one `thiserror` enum per crate (or per module when it is large), with source chains:
  ```rust
  #[derive(Debug, thiserror::Error)]
  #[non_exhaustive]
  pub enum IndexError {
      #[error("cannot read directory {path:?}")]
      ReadDir { path: PathBuf, #[source] source: std::io::Error },
      #[error(transparent)]
      Sqlite(#[from] rusqlite::Error),
  }
  ```
- Binaries (`app`, `cli`): use `anyhow::Result` and `.context("what we were doing")` at each layer.
- Use `?` everywhere. Don't use `unwrap()` in library code. `expect("why this can't fail")` is allowed
  only for real invariants.
- Recoverable per-item failures (a permission-denied directory) are counted and logged, not propagated.
  A single unreadable folder must never abort a scan.
- Never panic across an FFI boundary or in a worker thread whose panic would leave shared state
  inconsistent.

## 4. Ownership, borrowing and lifetimes

- Clone deliberately. A `.clone()` that only silences the borrow checker usually means the data flow
  should change, for example by splitting a struct or passing indices instead of references.
- Use indices (`u32` IDs into `Vec`s) rather than `Rc<RefCell<…>>` graphs for the file tree.
- Use `Arc<T>` for immutable shared state across threads. For a snapshot that readers swap atomically,
  use `ArcSwap` or `RwLock<Arc<T>>`. Keep lock scopes short and never hold a lock across I/O or a channel
  send.
- Name lifetimes only when the elision rules can't express the intent.

## 5. Traits and generics

- Generics (`impl Trait` / `<T: Trait>`) in hot paths. `dyn Trait` where heterogeneity or compile time
  matters more, for example preview providers in a `Vec<Box<dyn PreviewProvider>>`.
- Keep traits small and purposeful. Don't add a trait with a single implementation "for testing": test
  through the real type or a fake at the I/O boundary.
- Implement standard traits instead of ad-hoc methods: `From`/`TryFrom`, `Display`, `FromStr`,
  `IntoIterator`, `Default`.

## 6. Modules and visibility

- `pub` is a promise. Default to private, then `pub(crate)`. Re-export the public surface from `lib.rs`.
- Keep files focused, roughly under 500 lines. Split by responsibility, not by type kind (no `types.rs`
  dumping ground).
- No `mod.rs`: use `foo.rs` with a `foo/` directory.
- Doc comments (`///`) on every public item, saying *what* it does and *why* you'd use it, with an
  `# Errors` section for fallible functions and `# Panics` if it can panic.

## 7. Concurrency

- CPU-parallel work uses `rayon`. Pipelines use `crossbeam-channel` with **bounded** channels, so
  backpressure is mandatory.
- Long-running work takes a cancellation token (`Arc<AtomicBool>` or a generation counter) and checks it
  at loop boundaries.
- No async runtime in `core`, `index` or `search`. `zbus` is used in blocking mode in `platform`.
- Use `std::thread::scope` for borrowing work that must finish before returning.
- Choose atomics with the weakest correct ordering, and comment why if it isn't `SeqCst` or
  `Acquire`/`Release`.

## 8. `unsafe` policy

- Allowed only in `index` (raw `getdents64`/`statx`/fanotify via `rustix` where its safe API falls short)
  and `platform` (FFI). Every other crate has `#![forbid(unsafe_code)]`.
- Every `unsafe` block has a `// SAFETY:` comment naming the invariant and why it holds. Wrap it in a
  safe function with a narrow API.
- Prefer `rustix` (safe syscalls) over `libc` + `unsafe`.

## 9. Dependencies

- Add them to `[workspace.dependencies]` with a version, and reference them with `dep.workspace = true`.
- Disable default features you don't need (`default-features = false`) and enable features explicitly.
- Before adding a crate, check that it's maintained, widely used, and has a license allowed by `deny.toml`.
  Ask yourself whether 30 lines of std would do.

## 10. Style

- `rustfmt` defaults. Imports are grouped std, external, then crate (`group_imports = "StdExternalCrate"`
  is the target once it's stable; until then, keep that order by hand).
- Iterator chains when they read clearly. A `for` loop when there is early exit, mutation, or more than
  about four adapters.
- Comments explain *why*. If a comment explains *what*, rename things instead.
