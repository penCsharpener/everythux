# everythux

Instant file search for Linux, inspired by [Everything](https://www.voidtools.com/) on Windows.
Written in Rust, focused on ext4, backed by SQLite.

- Indexes every mounted ext4 filesystem by default (about 300k entries per second on SSD, warm cache) and
  keeps the index fresh with quick rescans and inotify.
- Results update as you type: an in-memory engine answers substring queries over 2 million entries in
  about 10 ms, with Everything-style syntax (`foo bar|baz !qux ext:pdf;docx size:>10mb dm:thisweek`).
- Five columns: Name, Path, Type, Date Modified, Size (folders show their recursive size).
- Double-click a **Path** cell to open the folder with the file selected. Double-click **Size** or
  **Date Modified** to open the folder's properties. Double-click **Name** to open the file.
- `F2` (or Right-click → Rename…) renames the selected file or folder right in the list; the status bar offers Undo.
- `Ctrl+Shift+R` rebuilds the index. `F5` runs a quick rescan. You can also re-index a single volume or folder.
- `Ctrl+Shift+Click` on a path excludes it and opens a popup to refine the rule. Exclusions support globs,
  regexes and scopes (`node_modules`, `bin` under `/home/*`, …).
- Optional preview pane (`Ctrl+P`) for images, text, PDFs, audio and video.
- Configuration is a JSON file (`~/.config/everythux/config.json`).

**Status:** v0.1 to v0.4 of the [roadmap](ROADMAP.md) are implemented. See its status section for what's
still open.

## Build and run

Requires stable Rust (1.88 or newer).

```sh
cargo build --release
./target/release/everythux                 # desktop UI
./target/release/everythux-cli --help      # headless: reindex, search, stats, exclusions, config
```

The version string includes the git commit the binary was built from, for example `0.5.0-dev (ef0366a, modified)`
(`modified` means uncommitted changes). It is shown in Help > About, logged at startup and printed by
`everythux-cli --version`, so you can tell a stale `target/release` build from a fresh one.

Optional runtime tools: `poppler-utils` (PDF previews), `ffmpeg` (video previews), `xdg-utils` (fallback
for opening files). Opening a folder with the file selected needs a file manager that implements
`org.freedesktop.FileManager1` (Nautilus, Dolphin, Nemo, Caja, Thunar).

Examples:

```sh
everythux-cli reindex --full
everythux-cli search '*.pdf' 'dm:thisyear' --sort size --desc --limit 20
everythux-cli exclude add node_modules --kind dir
```

### Release build

`./build-release.sh` builds the binaries with the `dist` Cargo profile (release, no debug info, stripped)
and writes a tarball, a `.deb` (xz-compressed) and `SHA256SUMS` to `dist/`. The artefact names match the
CI release workflow. The `.deb` is about 7 MB (about 25 MB installed) instead of 25 MB / 124 MB with the
plain release profile.

```sh
./build-release.sh                           # fmt + clippy, build, tarball, .deb, SHA256SUMS in dist/
./build-release.sh --skip-checks --out /tmp/out
./build-release.sh --no-deb --no-tarball     # just the stripped binaries in target/dist/
```

Options: `--out DIR` (default `dist/`), `--target TRIPLE` (default: host), `--no-deb`, `--no-tarball`,
`--skip-checks` (skip `cargo fmt --check` and clippy), `--test` (also run the tests),
`--install-tools` (run `cargo install cargo-deb --locked` if cargo-deb is missing; otherwise the script
prints that command and exits), `--help`.

## Documentation

- [Search syntax](docs/search-syntax.md)
- [Keyboard and mouse shortcuts](docs/shortcuts.md)
- [Exclusions](docs/exclusions.md)
- [Configuration](docs/configuration.md)
- [Troubleshooting](docs/troubleshooting.md)

## Development

The workspace layout and coding rules are in [CLAUDE.md](CLAUDE.md). Before committing, run:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
dbus-run-session -- cargo test --workspace --all-features
```
