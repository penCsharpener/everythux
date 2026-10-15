# Everythux Roadmap

> An "Everything"-style instant file search for Linux, written in Rust, optimised for ext4.

This document plans the work from an empty repository to a 1.0 release. It covers
the architecture, the data model, the phases in build order (each with tasks and
acceptance criteria), and the risks and open questions.

## Implementation status (2026-10-13)

**v0.5** (released as 0.5.0) adds inline rename, multiple AND-combined search bars, more robust text previews and a
fixed preview pane width. The plan is in [ROADMAP-v0.5.md](ROADMAP-v0.5.md). Done:
- Phase D (inline rename).
- Phase A (fixed preview pane width): presets and a Custom… width in View → Preview pane, dragging
  only when `ui.preview_pane.resizable` is on, and every preview kind laid out inside the width.
- Phase B (text preview): UTF-8/16/32 and Windows-1252 detection, more extensions and known file
  names, "Show more" up to 8 MiB, a virtualised selectable text view with Wrap and line-number
  toggles, and `Ctrl+C` copying selected preview text. Syntax highlighting is still deferred.
- Phase C (several search bars combined with AND, `SearchRequest.texts`).
- 0.6.0: tabs (`Ctrl+T`, each with its own bars, list and selection), an empty search lists nothing and
  `*` lists everything. Search bar shortcuts moved to `Ctrl+Shift+T`/`Ctrl+Shift+W`; config version 2.
  Window resizes only change the Path column, with View → Columns → Lock column widths
  (`ui.lock_column_widths`); the text preview has a Copy button and a right-click menu.
- 0.7.0: richer row context menu. **Open with ▸** lists the applications for the file type (default
  first, from `mimeapps.list`); file-type actions installed by other programs are shown too: KDE service
  menus (`kio/servicemenus`), Nemo actions and Thunar custom actions (`uca.xml`), grouped into their
  submenus. Also **Copy file** (clipboard as `text/uri-list`, needs `wl-copy` or `xclip`) and
  **Move to Trash** (freedesktop Trash spec, never copies across filesystems). `ui.context_menu` can turn
  the extra entries off, hide entries (menu → Hide an entry; View → Context menu restores them) and
  order them (`order`: list of entry ids). Not covered: plugin-only Dolphin entries (Ark, share, git),
  Nemo actions with `Conditions=`, entry icons, and a graphical editor for the order.

- 0.8.0: query matches in the Name and Path columns are drawn white and bold (plain words, phrases and
  wildcard literals; filters, negated terms and regex mode are not highlighted).
- 0.9.0: icon toolbar under the menu bar: Match case, Match path, Regex, Reindex all, Quick rescan,
  Open JSON config (default application) and Show preview, each with hover text and shortcut.
- 0.10.0: **Show only duplicates** (toolbar button and Search menu): keeps the files that share name and size
  with another match, in the current sort order; the result cap applies after the filter. Not persisted.
- 0.11.0: zoomable image previews: Ctrl+wheel, pinch or the −/+/Fit buttons (up to 16×), drag to pan,
  double-click resets; the zoom is remembered per file.
- 0.12.0: **Cut** and **Paste here** in the row menu (system clipboard, needs wl-clipboard or xclip): paste
  copies or moves into the right-clicked folder, never overwrites (` (copy)` suffix), then rescans.
- 0.12.1: zoomed image previews load the full-resolution image (up to the GPU texture size) and stay in a
  fixed-size frame; wheel zoom centres on the pointer.
- 0.12.2: PDF previews page through the whole document (◀ ▶ buttons or the wheel over the page).
- 0.12.3: zoomed image previews are panned by dragging, with the grab / grabbing hand cursors.
- 0.13.0: **search history**: every search is remembered indefinitely in `history.json` (the search is the
  key; reuse moves its date to now). Drop-down on the first search bar (chevron button or Alt+Down), newest
  first; Enter/click runs a search, Shift+Delete forgets it. A search counts as used when a result is opened.
- 0.14.0: **bookmarked searches** (Bookmarks menu, `bookmarks` in config.json): a name plus the bar texts and the
  match case / path / regex switches; run from the menu, managed in a small window (add, rename, search,
  delete). Not covered yet: sort order and filters per bookmark, reordering.
- 0.14.1: bookmarks can be imported from and exported to a JSON file (bookmarks window; import adds only
  the ones that are not there yet and also reads a whole config.json).
- 0.14.2: a star button in the toolbar bookmarks the current search (opens the bookmarks window with the name
  filled in); fixes the Add button being clipped out of the bookmarks window.
Phases 0–9 are implemented apart from the open checkboxes below; Phase 10 and 11 are partly done.
Still open, in rough priority order:

- **Incremental RAM-index patching** (Phase 7): every index change reloads the whole snapshot, so on
  multi-million-entry indexes live updates show up seconds late instead of within 1 s (Phase 9 target).
- **Privileged fanotify helper** (Phase 9): only the inotify user mode exists.
- **SQLite search fallback + FTS5 benchmark gate / ADR** (Phase 3): the in-memory engine meets the
  latency targets (≈8 ms substring query on 2M entries), so this is documentation work.
- Preview extras: syntax highlighting, PDF paging, audio playback (needs ALSA at build time).
- `follow_symlinks` is accepted in the config but symlinks are never followed.
- Manual file-manager test checklist, extra columns, search history/bookmarks, drag-and-drop, tray icon,
  `.rpm`/AppImage/Flatpak packaging, changelog automation.

---

## 1. Goals and non-goals

### Goals (the requested feature set)

| # | Requirement | Where it lands |
|---|-------------|----------------|
| R1 | By default, index every available ext4 partition | Phase 1 |
| R2 | A simple, native Rust UI | Phase 4 |
| R3 | Columns: **Name, Path, Type, Date Modified, Size** | Phase 4 |
| R4 | SQLite backend | Phase 2 |
| R5 | `Ctrl+Shift+R` rebuilds the database | Phase 5 |
| R6 | Double-clicking a **Path** cell opens the folder with the file selected | Phase 5 |
| R7 | Double-clicking a **Size** or **Modified** cell opens the folder's properties | Phase 5 |
| R8 | Optional preview pane for PDFs, images, audio and video | Phase 8 |
| R9 | `Ctrl+Shift+Left-click` on a path adds it to the exclusion list and opens an editor menu | Phase 6 |
| R10 | Exclusions support patterns (`node_modules`, `obj`, `bin`, `.var`, …) | Phase 6 |
| R11 | Partial re-index of one drive or one folder | Phase 7 |
| R12 | Configuration is stored as JSON | Phase 0 |

### Product qualities to aim for
- **Instant search:** results update as you type, with no visible lag on 2–5 million entries.
- **Fast first index:** a full index of a typical desktop (about 1M files) finishes in under a minute on SSD.
- **Small footprint:** under about 300 MB RAM for 2M entries, and an idle CPU when nothing is happening.
- **No surprises:** the index never silently goes stale, and exclusions are easy to see and undo.

### Non-goals for 1.0
- Searching file contents. Everything doesn't do this by default either; it may come later as an opt-in.
- Filesystems other than ext4 as a first-class target. The walker is generic, so btrfs and xfs will
  mostly work, but they won't be tested or tuned before 1.0.
- Network shares (SMB/NFS), Windows/macOS ports, and multi-user server mode.

---

## 2. Interpretations and open questions

These are assumptions the plan makes. Each one is cheap to change if it's wrong.

1. **"Modified by"** is read as **Date Modified**, which is the Everything column. ext4 doesn't record
   *who* last modified a file, only the owner (`uid`). An optional **Owner** column is in Phase 10.
2. **"Folder properties" on a Size/Modified double-click:** for a file, this opens the properties of the
   **containing folder**. For a folder row, it opens that folder's properties. The JSON setting
   `ui.properties_target: "parent" | "item"` lets you change this.
3. **Ctrl+Shift+Click** is normally "add range to selection" in list views. Here it is bound to
   "exclude path" as requested. Range selection moves to `Shift+Click` only, and the binding can be
   changed in the config.
4. **"All ext4 partitions available"** means *mounted* ext4 filesystems by default. Unmounted ext4
   partitions are listed in the UI with an "offer to mount" action (Phase 7). Reading raw block
   devices needs root and is a stretch goal (Phase 12).
5. **Privileges:** 1.0 runs as the desktop user and indexes everything that user can read. An optional
   privileged helper, needed for full-disk indexing and fanotify, is designed in Phase 9 but isn't
   required.

---

## 3. Architecture

```
┌──────────────────────────── everythux (single binary) ────────────────────────────┐
│                                                                                     │
│  UI thread (eframe/egui)                                                            │
│   ├─ search box ─► Query parser ─► SearchEngine (RAM index, rayon) ─► result ids    │
│   ├─ virtual table (Name | Path | Type | Modified | Size), sortable                  │
│   ├─ preview pane (optional)       ├─ exclusion editor      ├─ status bar/progress  │
│   └─ actions ─► Platform (D-Bus FileManager1, xdg-open, portals, thumbnails)        │
│                                                                                     │
│  Indexer threads                                                                    │
│   Volume discovery (/proc/self/mountinfo, udisks2)                                  │
│        │                                                                            │
│        ▼                                                                            │
│   Parallel walker (per volume, one-filesystem, exclusion-pruned)                    │
│        │  bounded channel (batches of ~10k rows)                                    │
│        ▼                                                                            │
│   SQLite writer (single connection, WAL, batched transactions) ──► index.db         │
│        │                                                                            │
│        └─ "index changed" event ─► SearchEngine reload/patch ─► UI repaint           │
│                                                                                     │
│  Watcher (Phase 9): fanotify (privileged) / inotify (user) ─► incremental updates   │
└─────────────────────────────────────────────────────────────────────────────────────┘
        ▲                                   ▲
        │ config.json (serde, hot-reload)    │ CLI: everythux-cli search/reindex/exclude
```

### Key design decisions

| Decision | Choice | Why |
|----------|--------|-----|
| UI toolkit | **eframe/egui** + `egui_extras::TableBuilder` | Pure Rust, fast to iterate on, and the table virtualises rows so millions of them scroll smoothly. Double-click and modifier detection are built in (`Response::double_clicked()`, `ctx.input(|i| i.modifiers)`). Slint or iced are alternatives if a more native look matters later; GTK4 (`gtk4-rs`) if desktop integration matters more than "pure Rust". |
| Persistence | **SQLite via `rusqlite` (feature `bundled`)** | Required (R4). Gives atomic, crash-safe storage and a stable on-disk format. |
| Query path | **RAM search index loaded from SQLite**, with a SQLite-only fallback | A `LIKE '%foo%'` scan over 2M rows takes about 200–500 ms. That's too slow for search-as-you-type. A struct-of-arrays index in RAM with parallel `memchr::memmem` takes about 10–30 ms. SQLite stays the source of truth. Phase 3 has a benchmark gate that compares this against FTS5 `trigram`. |
| Walker | Start with **`jwalk`** (parallel, prunes in `process_read_dir`). Later, a custom **`rustix`** walker (`openat`/`getdents64`/`statx`) | Ships quickly, with a known way to optimise later. |
| Exclusion patterns | **`globset`** (plus an optional `regex`) | Fast to match many patterns at once, and the syntax is familiar. |
| Desktop integration | **`zbus`** → `org.freedesktop.FileManager1` (`ShowItems`, `ShowItemProperties`), with an `xdg-open` fallback | Nautilus, Dolphin, Nemo, Caja and Thunar implement it. It gives the "folder opens with the file selected" behaviour. |
| Config | **`serde` + `serde_json`**, `directories` for XDG paths, `notify` for hot reload | Required (R12). |
| Concurrency | `crossbeam-channel` and `rayon`. No async runtime in the core | Simple design: only D-Bus needs async, and `zbus` blocking mode covers that. |
| Logging | `tracing` + `tracing-subscriber` (file in `$XDG_STATE_HOME`) | |

### Cargo workspace layout

```
everythux/
├─ Cargo.toml                 # workspace
├─ crates/
│  ├─ core/        # domain types, config (JSON), exclusion rules, errors
│  ├─ index/       # volume discovery, walker, SQLite store, migrations, rescans
│  ├─ search/      # query parser, RAM index, sorting, filters
│  ├─ platform/    # D-Bus FileManager1, xdg-open, portals, thumbnails, udisks2
│  ├─ preview/     # image/pdf/audio/video/text preview providers (feature-gated)
│  ├─ app/         # eframe UI (binary: `everythux`)
│  └─ cli/         # headless CLI (binary: `everythux-cli`)
├─ docs/
└─ packaging/      # .desktop, icons, deb/rpm/AUR/Flatpak manifests
```

---

## 4. Data model (SQLite)

Database file: `$XDG_DATA_HOME/everythux/index.db` (default `~/.local/share/everythux/index.db`).

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous  = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);  -- schema_version, created_at, …

CREATE TABLE volumes (
  id              INTEGER PRIMARY KEY,
  fs_uuid         TEXT NOT NULL UNIQUE,  -- stable identity across reboots / mount point changes
  device          TEXT,                  -- /dev/nvme0n1p2
  mount_point     TEXT,                  -- last known mount point
  label           TEXT,
  fs_type         TEXT NOT NULL,         -- 'ext4'
  state           TEXT NOT NULL,         -- 'online' | 'offline' | 'scanning' | 'error'
  last_full_scan  INTEGER,               -- unix ns
  last_scan_ms    INTEGER
);

CREATE TABLE dirs (
  id          INTEGER PRIMARY KEY,
  volume_id   INTEGER NOT NULL REFERENCES volumes(id) ON DELETE CASCADE,
  parent_id   INTEGER REFERENCES dirs(id) ON DELETE CASCADE,
  path        BLOB NOT NULL,             -- absolute path as raw bytes (Linux paths are not UTF-8!)
  inode       INTEGER NOT NULL,
  mtime_ns    INTEGER NOT NULL,          -- used for quick incremental rescans (1 ns off when racy)
  total_size  INTEGER,                   -- recursive size, filled after scan (folder Size column)
  UNIQUE (volume_id, path)
);

CREATE TABLE entries (
  id          INTEGER PRIMARY KEY,
  dir_id      INTEGER NOT NULL REFERENCES dirs(id) ON DELETE CASCADE,  -- containing folder
  name        BLOB NOT NULL,             -- raw bytes
  name_fold   TEXT NOT NULL,             -- lossy UTF-8, Unicode-lowercased, for search
  kind        INTEGER NOT NULL,          -- 0 file, 1 dir, 2 symlink, 3 other
  ext         TEXT,                      -- lowercased extension, NULL if none
  size        INTEGER NOT NULL,
  mtime_ns    INTEGER NOT NULL,
  inode       INTEGER NOT NULL,
  uid         INTEGER,
  mode        INTEGER
);

CREATE INDEX entries_dir   ON entries(dir_id);
CREATE INDEX entries_ext   ON entries(ext);
CREATE INDEX entries_size  ON entries(size);
CREATE INDEX entries_mtime ON entries(mtime_ns);
CREATE INDEX dirs_parent   ON dirs(parent_id);
```

Notes:
- The **Path** column is the containing folder (`dirs.path`). Folders make up only about 10% of entries,
  so storing their full paths costs little and makes path searches and subtree deletes simple:
  `WHERE path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0')`.
- Names are stored **as bytes**, because Linux filenames can be invalid UTF-8. The UI displays them with
  lossy conversion, and actions (open, select) always use the original bytes.
- **Type** is not stored. It is derived from `ext` through a cached lookup in shared-mime-info
  (for example `pdf` → "PDF document"). Indexing never reads file contents.
- Schema migrations use a `schema_version` in `meta` with numbered SQL files (`refinery`, or a small
  hand-rolled runner).

---

## 5. Configuration (JSON)

File: `$XDG_CONFIG_HOME/everythux/config.json` (default `~/.config/everythux/config.json`).
Writes are atomic (write to a temp file, `fsync`, then rename). Every field has a `#[serde(default)]`,
so older and newer files both load. A top-level `version` drives migrations, and external edits are
hot-reloaded.

```json
{
  "version": 1,
  "volumes": {
    "auto_discover_fs_types": ["ext4"],
    "include": [],
    "disabled_uuids": []
  },
  "exclusions": [
    { "pattern": "node_modules", "kind": "dir", "scope": null, "enabled": true },
    { "pattern": ".var",         "kind": "dir", "scope": "/home/*", "enabled": true },
    { "pattern": "obj",          "kind": "dir", "scope": "/home/*", "enabled": true },
    { "pattern": "bin",          "kind": "dir", "scope": "/home/*/src/**", "enabled": true },
    { "pattern": "/home/me/Downloads/old", "kind": "path", "scope": null, "enabled": true },
    { "pattern": "*.tmp",        "kind": "file", "scope": null, "enabled": true },
    { "pattern": "re:^\\.cache-[0-9]+$", "kind": "dir", "scope": null, "enabled": false }
  ],
  "ui": {
    "columns": [
      { "id": "name", "width": 280, "visible": true },
      { "id": "path", "width": 380, "visible": true },
      { "id": "type", "width": 140, "visible": true },
      { "id": "modified", "width": 150, "visible": true },
      { "id": "size", "width": 90, "visible": true }
    ],
    "sort": { "column": "name", "ascending": true },
    "preview_pane": { "enabled": false, "width": 360 },
    "properties_target": "parent",
    "size_format": "binary",
    "date_format": "%Y-%m-%d %H:%M",
    "theme": "system"
  },
  "keybindings": {
    "reindex_all": "Ctrl+Shift+R",
    "exclude_path_click": "Ctrl+Shift+LeftClick",
    "toggle_preview": "Ctrl+P",
    "focus_search": "Ctrl+F"
  },
  "indexer": {
    "threads": 0,
    "follow_symlinks": false,
    "one_filesystem": true,
    "rescan_on_startup": "quick",
    "scheduled_rescan_minutes": 0
  },
  "search": {
    "case_sensitive": false,
    "match_path": false,
    "regex": false,
    "max_results": 0
  }
}
```

---

## 6. Phases

Rough sizes assume one developer working part-time (about 10–15 h/week). **v0.1** (Phases 0–5) is the first
usable release and covers R1–R7 and R12.

### Phase 0: Foundations *(≈1 week)*
- [x] Cargo workspace with the crates above, `rust-toolchain.toml` (stable), `rustfmt`, `clippy -D warnings`.
- [x] CI on GitHub Actions: build, test, clippy, `cargo deny` (licenses/advisories), cache.
- [x] `core::config`: typed `Config` struct, load/save JSON with atomic writes, defaults, versioned migration,
      and validation errors shown to the user instead of panics.
- [x] XDG paths through `directories`. `tracing` logs to `$XDG_STATE_HOME/everythux/everythux.log`.
- [x] Error type strategy: `thiserror` in libraries, `anyhow` in binaries.

**Done when:** `cargo test` is green in CI, and `config.json` round-trips and survives unknown or missing fields.

### Phase 1: Volume discovery and scanner *(≈2 weeks)* covers R1
- [x] Parse `/proc/self/mountinfo`, filter `fstype == ext4`, and resolve the filesystem UUID
      (`/dev/disk/by-uuid` symlinks, or `libblkid` through FFI if needed).
- [x] **Dedupe bind mounts:** index each `(device, fs root)` pair once, at its shortest mount point.
- [x] One-filesystem traversal: never cross `st_dev` boundaries. Nested mounts get their own volume.
- [x] Parallel walker (own `rayon` + `rustix` `getdents64`/`statat` walker; `jwalk` was not needed) with:
  - exclusion pruning *before* descending (a `node_modules` folder is never read),
  - `lstat` semantics, so symlinks are not followed by default,
  - permission errors counted and logged per volume, not fatal,
  - cancellation token and progress reporting (dirs/s, entries, current path).
- [x] Bounded channel to the writer, so the walker can't outrun SQLite and use unbounded RAM.
- [x] Benchmark harness: a synthetic tree generator (1M files, 100k dirs) plus `criterion` benches.

**Done when:** `everythux-cli scan --dry-run` lists all mounted ext4 volumes and walks them, with throughput
of at least 50k entries/s on SSD (warm cache).

### Phase 2: SQLite store *(≈1.5 weeks)* covers R4
- [x] Schema plus migration runner. WAL mode, prepared statements, a single writer thread.
- [x] **Bulk-load path for full scans:** write into `index.db.building` with secondary indexes dropped,
      insert in transactions of about 10k rows, build the indexes at the end, compute `dirs.total_size`
      bottom-up, `ANALYZE`, then atomically rename over `index.db`. The UI keeps searching the old index
      until the swap.
- [x] **Subtree replace path** for partial scans: `DELETE` the subtree and insert fresh rows in one transaction.
- [x] Integrity: `PRAGMA quick_check` on startup. If it fails, offer a rebuild.
- [x] `everythux-cli stats` prints entry counts, DB size and last scan per volume.

**Done when:** a 1M-entry index builds in under 60 s end-to-end, the DB is about 150 B/entry, and killing the
process mid-scan leaves the previous index intact.

### Phase 3: Search engine *(≈2 weeks)*
- [ ] `SearchEngine` trait with two implementations:
  - `SqliteSearch`: `LIKE`/`GLOB` on `name_fold`, used as reference and fallback.
  - `MemSearch`: struct-of-arrays loaded from SQLite (one contiguous `Vec<u8>` of folded names plus
    offsets, along with `dir_id`, `size`, `mtime`, `kind` and `ext_id` arrays). Matching runs in parallel
    with `rayon` and `memchr::memmem`.
- [x] **Pre-sorted permutations** for each sortable column (name, path, type, size, modified). The result
  set is a bitset, and sorted output comes from walking the permutation and keeping matches. Clicking a
  column header re-sorts in O(n) with no per-query sort. This is how Everything does it.
- [x] Query cancellation: each keystroke bumps a generation counter, stale queries stop early, and input
  is debounced by about 30 ms.
- [x] Basic syntax: space means AND, `|` means OR, `!` means NOT, `"quoted phrase"`, `*`/`?` wildcards,
  and a case-sensitivity toggle.
- [ ] **Benchmark gate:** compare `MemSearch` against SQLite FTS5 (`tokenize='trigram'`) on 2M entries for
  latency, RAM and DB size. Document the result in `docs/adr/0001-search-engine.md` and choose the default.

**Done when:** p95 latency from keystroke to first page is under 50 ms on 2M entries, and RAM stays under 300 MB.

### Phase 4: UI MVP *(≈2 weeks)* covers R2, R3
- [x] eframe window: search box (focused on start), result table, status bar
      ("1,234 of 2,345,678 objects · indexing /home 43%").
- [x] Virtualised table (`TableBuilder::body().rows(row_h, n, …)`), with only visible rows materialised from
      the engine.
- [x] Columns **Name | Path | Type | Date Modified | Size**: resizable, sortable by header click, with an
      ascending/descending indicator. Visibility and widths are saved to JSON.
- [x] File-type icons by MIME category (a small embedded icon set; freedesktop icon theme lookup comes later).
- [x] Human-readable sizes (binary or SI, configurable) and locale-aware dates (`chrono`).
- [x] Keyboard navigation: arrows, PageUp/PageDown, Home/End, Enter opens, `Ctrl+C` copies the full path,
      `Ctrl+Shift+C` copies the folder.
- [x] Right-click context menu: Open, Open containing folder, Copy path, Copy name, Properties,
      Exclude…, Re-index this folder.
- [x] Light/dark theme following the system preference.
- [x] Remember window size and position.

**Done when:** scrolling 2M results stays at 60 fps, and every column sorts in under 100 ms.

### Phase 5: Core interactions *(≈1 week)* covers R5, R6, R7. **Release v0.1**
- [x] **`Ctrl+Shift+R` triggers a full reindex.** It is consumed globally before widgets, so the search
      box doesn't swallow it. It runs in the background with progress and cancel in the status bar, and the
      old index stays searchable until the atomic swap (Phase 2). Pressing it again while running asks
      "Restart reindex?".
- [x] **Double-click on a Path cell opens the folder with the file selected.**
  1. D-Bus `org.freedesktop.FileManager1.ShowItems([file://…], startup_id)` through `zbus`.
  2. In a Flatpak/sandbox, `org.freedesktop.portal.OpenURI.OpenDirectory`.
  3. Otherwise, `xdg-open <parent dir>` (opens the folder, but nothing is selected).
- [x] **Double-click on a Size or Modified cell opens the folder's properties.**
  1. D-Bus `org.freedesktop.FileManager1.ShowItemProperties([file://<folder>], startup_id)`.
  2. If no file manager implements it, a **built-in Properties dialog** shows path, size (from
     `dirs.total_size`), file/folder counts from the index, owner, permissions, and mtime/ctime.
     It also has "Open folder" and "Re-index" buttons.
- [x] Double-click on a **Name** cell opens the file with its default application, the same as Everything
      (`xdg-open` or the portal `OpenFile`).
- [x] A per-cell hit test decides which action runs, using `ui.interact` on each cell rect.
- [ ] Integration tests against Nautilus, Dolphin, Nemo and Thunar, with a manual checklist in `docs/testing.md`.

**Done when:** all three double-click behaviours work on GNOME and KDE, and fall back gracefully elsewhere.

### Phase 6: Exclusions *(≈1.5 weeks)* covers R9, R10. **v0.2**
- [x] **Rule model** (`core::exclusion`):
  - `kind`: `dir` (matches folder names anywhere), `file`, `path` (an exact subtree), or `any`.
  - `pattern`: glob (`node_modules`, `*.o`, `**/target/debug`, `/home/*/.cache`) or `re:<regex>`.
  - `scope`: optional glob limiting where the rule applies (for example, `bin` only under `/home/*`).
  - `enabled`: true/false.
- [x] Compile all rules into one `GlobSet` and one `RegexSet` for fast matching, and apply them **during the walk**.
- [x] **Shipped defaults** (enabled, editable): `node_modules`, `.var` (Flatpak data, `~/.var`),
      `.cache`, `__pycache__`, `.venv`, `target`, plus `obj` and `bin` **scoped to `/home/*`**. Scoping
      matters: excluding `bin` globally would remove `/usr/bin`. The editor warns when a rule would exclude
      more than N% of the current index.
- [x] **`Ctrl+Shift+Left-click` on a Path cell** adds a `path` rule for that folder *immediately* and opens the
      **Exclusion popup** next to the cursor, prefilled. It lets you:
  - switch between "this exact folder", "any folder named `<name>`", "any folder named `<name>` under `<scope>`" and a custom glob,
  - see a **live preview**: "matches 12,345 indexed entries in 87 folders",
  - choose between *Remove matching entries from index now* (default) and *Keep until next rescan*,
  - **Undo** (removes the rule) or **Open full exclusion editor**.
- [x] **Exclusion editor window** (Settings → Exclusions): table of rules with enable checkboxes,
      add/edit/delete/reorder, a test box ("is `/home/me/proj/bin/x` excluded? by which rule?"),
      and import/export.
- [x] Removing an exclusion offers to **re-index only the affected subtrees** (this uses Phase 7).
- [x] Every change is persisted to `config.json` immediately.

**Done when:** after adding `node_modules`, matching entries disappear from results in under 1 s without a
full rescan, and the rule survives restarts.

### Phase 7: Partial index updates *(≈1.5 weeks)* covers R11. **v0.2**
- [x] **Re-index one volume:** from the context menu, *Index → Volumes*, or
      `everythux-cli reindex --volume <uuid|mountpoint>`.
- [x] **Re-index one folder (subtree):** from the context menu on any row or the folder Properties dialog, or
      `everythux-cli reindex --path /home/me/projects`.
- [x] **Quick rescan mode:** ext4 updates a directory's mtime whenever entries are added, removed or renamed.
      The quick mode re-lists only directories whose `mtime` changed since the last scan, so updating after a
      day away takes seconds. Full mode also catches size/mtime changes to files inside unchanged folders.
      Folders whose mtime falls within 2 s before a walk starts are "racy" (a change in the same timestamp
      tick keeps the mtime): they are stored with a validator 1 ns off, so the next quick scan re-lists them.
- [x] Startup policy: `rescan_on_startup` is `none`, `quick` or `full`. `scheduled_rescan_minutes` sets periodic quick rescans.
- [x] **Volumes panel:** lists all ext4 partitions (from udisks2 over D-Bus), with mounted/unmounted state,
      last scan, entry count, enable/disable toggle, and "Mount and index" for unmounted ones.
- [x] **Offline volumes:** entries of an unmounted or removed disk stay in the index, greyed out with a
      volume badge, keyed by filesystem UUID. They come back online when the disk is mounted again, even at a
      different path.
- [ ] The RAM index applies subtree patches incrementally instead of doing a full reload.

**Done when:** re-indexing a 50k-entry folder takes under 3 s and doesn't block searching.

### Phase 8: Preview pane *(≈2–3 weeks; optional features)* covers R8. **v0.3**
A toggleable right-hand pane (`Ctrl+P`) shows the selected file. Each provider sits behind a cargo feature
and runs on a worker thread with an LRU cache, so the UI never blocks.

| Type | Approach | Effort |
|------|----------|--------|
| Any file with an existing thumbnail | Read the **freedesktop thumbnail cache** (`~/.cache/thumbnails/{normal,large,x-large}/<md5(uri)>.png`) first. It's free and covers whatever the file manager already generated. | Easy |
| Images (png/jpg/gif/webp/bmp/tiff/ico/avif) | `image` crate, downscaled, shown as an egui texture, with EXIF orientation (`kamadak-exif`) | Easy |
| Text/code/markdown | First 64 KB, syntax highlighting via `syntect` | Easy |
| PDF | Render page 1 (with paging later) through **`pdfium-render`** (dynamically loads `libpdfium`). Fallbacks: `pdftoppm -png -f 1 -l 1`, or the pure-Rust `hayro` renderer | Medium |
| Audio (mp3/flac/ogg/opus/m4a/wav) | Tags, duration and cover art via **`lofty`**. Play/pause/seek via **`rodio`** (Symphonia decoders) | Medium |
| Video (mp4/mkv/webm/avi) | Thumbnail at 10% duration via `ffmpegthumbnailer`/`ffmpeg` CLI (if installed) or the D-Bus thumbnailer `org.freedesktop.thumbnails.Thumbnailer1`. Metadata via `ffprobe`. A "Play" button opens the default player | Medium |
| Inline video playback | GStreamer (`gstreamer-rs`) rendering to an egui texture | Hard (post-1.0) |

- [x] Large-file guards: no preview over a configurable size, and decoding is cancelled when selection changes.
- [x] Degrades cleanly: when a dependency is missing, the pane shows metadata only, plus an "Install X for previews" hint.

**Done when:** previews for images, PDFs and audio appear within 150 ms of selection on typical files, with no UI jank.

### Phase 9: Live updates *(≈2–3 weeks)* **v0.4**
Keeps the index fresh without rescans.
- [ ] **Privileged mode (recommended):** a small `everythuxd` helper (systemd unit + polkit) using **fanotify**
      with `FAN_MARK_FILESYSTEM | FAN_REPORT_DFID_NAME` (Linux ≥ 5.9). One mark per ext4 filesystem receives
      create/delete/move/modify events for the whole disk, with no per-directory watch limits. It streams
      changes to the app over a Unix socket.
  - Security: the daemon serves **metadata only**. Results are filtered by the requesting user's
    read/execute permission on the containing folder, so private filenames don't leak to other users.
- [x] **User mode (fallback):** `inotify` on selected roots (for example `$HOME`), using the `notify` crate, with
      a watch-count budget. Anything not covered by watches gets periodic quick rescans.
- [x] Event coalescing (100–500 ms), with rename pairing via cookie or FID.
- [x] Overflow handling (`IN_Q_OVERFLOW` / `FAN_Q_OVERFLOW`) triggers a quick rescan of the affected volume.

**Done when:** a file created anywhere on an indexed ext4 volume appears in results within 1 s.

### Phase 10: Power search and polish *(≈2 weeks)*
- [x] Everything-style filters: `ext:pdf;docx`, `size:>100mb`, `size:1mb..10mb`, `dm:today`,
      `dm:lastweek`, `dm:2026-01-01..2026-03-01`, `path:src`, `folder:`, `file:`, `regex:`, `case:`, `vol:home`.
- [x] Toggle to match against the full path, not just the name (`Ctrl+U`).
- [ ] Optional extra columns: Owner, Permissions, Date Created (ext4 stores `crtime`, read via `statx`), Extension.
- [ ] Search history, saved searches/bookmarks, and quick filter buttons (Audio, Video, Documents, Pictures, Folders).
- [ ] Multi-select operations: copy paths, open, and drag-and-drop paths into other apps.
- [x] `everythux-cli search <query> [--json]` for scripting (like Everything's `es.exe`).
- [ ] Optional tray icon and single-instance with a global "show window" hotkey (portal `GlobalShortcuts` on Wayland).

### Phase 11: Packaging and release *(≈1–2 weeks)* **v1.0**
- [x] `.desktop` file, icon set, AppStream metainfo.
- [ ] Packages: `.deb` (`cargo-deb`), `.rpm` (`cargo-generate-rpm`), AUR `PKGBUILD`, static tarball, AppImage.
- [ ] Flatpak: documented with caveats. The sandbox needs `--filesystem=host:ro`, and fanotify is unavailable there.
- [ ] Release automation: tagging, changelog (`git-cliff`), GitHub Releases with checksums.
- [x] User docs: search syntax, shortcuts, exclusions guide, troubleshooting.

### Phase 12: Stretch goals (post-1.0)
- **Raw ext4 indexing:** read inode tables and directory blocks directly from the block device (like Everything
  reading the NTFS MFT), using a read-only ext4 parser (for example the `ext4-view` crate). This could make initial
  indexing several times faster, and would also work on unmounted partitions. It needs root and careful handling of
  journal state.
- First-class btrfs/xfs support, network shares, file-content indexing (opt-in, via `tantivy`), and an HTTP/ETP-like server.

---

## 7. Milestone summary

| Release | Phases | Contents | Requirements |
|---------|--------|----------|--------------|
| **v0.1, MVP** | 0–5 | Auto-discover ext4, full index to SQLite, instant search, 5-column table, Ctrl+Shift+R, double-click actions, JSON config | R1–R7, R12 |
| **v0.2** | 6–7 | Pattern exclusions with Ctrl+Shift+Click popup and editor, per-volume/per-folder re-index, quick rescan, offline volumes | R9–R11 |
| **v0.3** | 8 | Preview pane: images, text, PDF, audio, video thumbnails | R8 |
| **v0.4** | 9 | Live updates (fanotify helper / inotify fallback) | – |
| **v1.0** | 10–11 | Search filters, polish, packaging | – |

Cumulative effort at part-time pace: v0.1 in about 9–10 weeks, v0.2 at about 13 weeks, v1.0 at about 22–26 weeks.

---

## 8. Input reference (target behaviour)

| Input | Context | Action |
|-------|---------|--------|
| Typing | anywhere | Search (search box auto-focused) |
| `Ctrl+Shift+R` | global | Full reindex of all volumes (background, cancellable) |
| Double-click | Name cell | Open file with its default application |
| Double-click | Path cell | Open containing folder in the file manager, with the file selected |
| Double-click | Size / Modified cell | Open folder properties (file manager, or built-in dialog) |
| `Ctrl+Shift+Left-click` | Path cell | Add path to exclusions and open the exclusion popup |
| Right-click | row | Context menu (open, copy, properties, exclude, re-index folder) |
| `Ctrl+P` | global | Toggle preview pane |
| `Enter` / `Ctrl+Enter` | row | Open / open containing folder |
| `Ctrl+C` / `Ctrl+Shift+C` | row | Copy full path / copy folder path |
| `F5` | global | Quick rescan |

All bindings can be overridden in `config.json → keybindings`.

---

## 9. Performance targets and how they're measured

| Metric | Target | Measured by |
|--------|--------|-------------|
| Initial index, 1M entries, SSD | ≤ 60 s | `cargo bench -p index` + real-disk script |
| Quick rescan, 1M entries, no changes | ≤ 5 s | bench |
| Keystroke → first page, 2M entries | p95 ≤ 50 ms | `criterion` on the synthetic corpus |
| Column re-sort, 2M results | ≤ 100 ms | bench |
| RAM, 2M entries | ≤ 300 MB RSS | `/proc/self/status` in CI job |
| DB size | ≈ 150 B/entry | `stats` command |
| Idle CPU | ~0% | manual |

Tips to keep in mind: when stat-ing a directory's children, do it in **inode order**, which helps HDDs a lot on
ext4. Use `statx` with `AT_STATX_DONT_SYNC`. Avoid per-entry heap allocations in the walker.

---

## 10. Testing strategy
- **Unit:** config round-trip and migrations, exclusion matching (a table of paths against rules), query
  parser, size and date formatting.
- **Integration:** walker and store against `tempfile` trees, including invalid UTF-8 names, deep nesting,
  symlink loops, permission-denied dirs, and very long names (255 bytes).
- **Real ext4 in CI:** create an image file, `mkfs.ext4`, loop-mount it with `sudo` (available on GitHub Actions
  Ubuntu runners), then test discovery, UUID tracking and one-filesystem behaviour.
- **UI:** `egui_kittest` for snapshot and interaction tests: double-click routing per column, Ctrl+Shift+Click
  opening the exclusion popup, and the shortcut not being swallowed by the search box.
- **D-Bus:** a mock `FileManager1` service on a private session bus (`dbus-run-session`) to check that the
  `ShowItems`/`ShowItemProperties` calls carry the right URIs.
- **Benchmarks:** run on every PR against the synthetic corpus, failing on a >15% regression.

---

## 11. Risks and mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| SQLite substring search too slow for search-as-you-type | Core UX | RAM index (Phase 3) with a documented benchmark gate. FTS5 trigram as the alternative |
| Running as a user can't read everything (`/root`, other users' homes) | Incomplete index | Count and show skipped dirs in status. Optional privileged helper (Phase 9) |
| File manager lacks `FileManager1` or `ShowItemProperties` | R6/R7 degrade | Fall back to the portal, then `xdg-open`. Built-in Properties dialog |
| Over-broad exclusions (`bin` removing `/usr/bin`) | Missing results | Scoped defaults, an "affects N entries" preview, a warning threshold, one-click undo |
| inotify watch limits (`max_user_watches`) | Stale index | fanotify filesystem marks in privileged mode. Quick rescans as backstop |
| Non-UTF-8 filenames | Crashes or wrong actions | Store bytes, display lossy, act on the original bytes |
| Bind mounts / containers double-counting | Duplicate results | Dedupe by `(device, root)` from mountinfo |
| Preview dependencies (pdfium, ffmpeg) missing | Feature gaps | Feature flags, thumbnail-cache first, clear "install X" hints |
| Wayland restrictions (global hotkeys, window focus) | Polish | Use portals (`GlobalShortcuts`, activation tokens via `startup_id`) |

---

## 12. Immediate next steps
1. Confirm the interpretations in §2, especially "modified by" and the target for "folder properties".
2. Phase 0: set up the workspace, CI and config crate.
3. Spike, at most 2 days: walk `/` with `jwalk` into SQLite and measure. Prototype the `MemSearch` substring
   scan on the result. Together these validate the riskiest assumptions before building the UI.
