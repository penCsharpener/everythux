# Everythux v0.5 Roadmap

Four features requested on 2026-10-14:

| # | Feature | Phase | Size |
|---|---------|-------|------|
| F1 | Rename a file or folder directly in the result list | Phase D | L |
| F2 | Several search bars (`+` adds one, `x` closes one) that combine with AND; `Tab` switches between the list and the newest bar | Phase C | M |
| F3 | Text files previewed reliably | Phase B | M |
| F4 | A fixed preview pane width set by the user, never driven by the previewed item | Phase A | S |

This file plans them on top of `main` at `1d4d3ae` (v0.4). It covers the current state, the design,
tasks, tests and acceptance criteria for each feature, plus the open questions. The conventions in
`CLAUDE.md` and the skills under `.claude/skills/` apply throughout.

## Build order and why

The phases go smallest and most self-contained first. F1 goes last because it is the only feature
that writes to the user's filesystem and changes the index.

| Phase | Feature | Depends on | Crates touched |
|-------|---------|------------|----------------|
| A | F4 fixed pane width | none | `core` (config), `app` |
| B | F3 text preview | A (text is laid out inside the fixed width) | `preview`, `app` |
| C | F2 multiple search bars | none | `search` (combine queries), `app` |
| D | F1 inline rename | none, but reuses C's focus handling for `Tab`/`Esc` | `platform` (rename syscall), `index` (rename job), `app` |

Each phase is its own commit series with a green gate (`rust-verify` skill) and its own entry in the
"Implementation status" section of `ROADMAP.md`. B and C can run in parallel: they share no files
except `crates/app/src/app.rs` state wiring.

---

## Phase A (F4): Fixed preview pane width

### Current state
- `crates/app/src/ui/preview_pane.rs` shows a resizable right `SidePanel` (`resizable(true)`,
  `size_range(160..=1000)`). Since `26b6741` the panel fills its configured width
  (`take_available_width`) and saves a new width once, after an edge drag.
- `config.ui.preview_pane.width` (default 360) is the only width setting. There is no way to stop
  the edge from being dragged, and no way to type an exact width.
- Content can still overflow inside the pane. A long file name in the header wraps. A wide image is
  shrunk by `shrink_to_fit`, but the pane has no hard `max_width` guard for every content kind.

### Design
- New config fields in `PreviewPaneConfig` (`crates/core/src/config.rs`). They use `#[serde(default)]`,
  so no version bump is needed:
  - `width: f32`: the existing field, now the authoritative fixed width.
  - `resizable: bool`, default **`false`**: the user asked for a fixed width. When `false` the panel
    is built with `exact_width(width)` and `resizable(false)`, so there is no drag handle and no
    resize cursor.
- **Setting the width** (View → Preview pane):
  - Presets: Narrow 280 px, Medium 360 px, Wide 520 px, Extra wide 720 px.
  - "Custom…" opens a small window with a `DragValue` (200–1200 px, step 10) and live feedback.
    The value is saved on change; saves are already debounced 500 ms.
  - A checkbox "Allow resizing by dragging" sets `resizable = true`. The edge drag then behaves as
    it does today and saves once on release.
- **Hard width guarantee.** Inside the panel:
  - Every `PreviewContent` branch renders in a child `Ui` with `set_max_width(width)` and
    `set_min_width(width)`. No branch may call `auto_shrink([true, _])` horizontally.
  - Header: the file name is truncated with an ellipsis (`Label::truncate()`), with the full name in
    a tooltip. Size, date and path go on a single truncated line.
  - Images, PDF pages, covers and video thumbnails are scaled to the pane width (`fit_to_exact_size`
    keeping the aspect ratio). Small images are never upscaled.
  - Media metadata grid: the value column truncates, with a tooltip.
  - Text: see Phase B, which covers wrap and horizontal scroll inside the pane.
- **Narrow windows.** If the window is narrower than `width + 300` px (the table's minimum), the pane
  is shown at `window_width - 300` for that frame only. The config is **not** changed, so the
  pane returns to its fixed width when the window grows. This logic already exists for the drag
  case (`track_preview_width` only adopts widths while the pointer is down); extend it to the
  fixed-width case.

### Tasks
- [x] `core`: add `PreviewPaneConfig.resizable` (default `false`), plus docs in `docs/configuration.md`.
- [x] `app/ui/preview_pane.rs`: when not resizable, build the panel with
      `exact_width(clamped width)`; keep the drag path for `resizable = true`.
- [x] `app/ui/menus.rs`: View → Preview pane submenu (enable toggle, presets, Custom…,
      "Allow resizing by dragging").
- [x] Width-capped rendering for every content kind (header truncation, image fit, grid truncation).
- [x] Narrow-window clamp that never writes config.

### Tests (kittest, `crates/app/src/ui/tests.rs`)
- `fixed_width_pane_has_exact_width_for_every_content_kind`: render each `PreviewContent`
  variant with oversized content (a 4000 px wide image, a 300-character file name, a 10 000-column
  text line, long metadata values) and assert the panel rect width equals the configured width ± 1 px.
- `fixed_width_pane_has_no_resize_handle`: dragging the edge changes nothing and saves nothing.
- `choosing_a_preset_sets_and_saves_the_width_once`.
- `narrow_window_shrinks_the_pane_without_changing_config`.
- `resizable_mode_keeps_todays_drag_behaviour`: the existing drag tests stay green with
  `resizable = true`.

### Done when
The pane width only changes through the View menu (or a drag, when enabled). Selecting any file,
including huge images, long names and wide text, never moves the pane edge by a pixel.

---

## Phase B (F3): Text preview

### Current state (verified)
Text preview already works on `main` for the common cases. On 2026-10-14 the real app under Xvfb
previewed `main.rs`, `readme.txt`, `Makefile` and `.gitconfig`: monospace text, correct content,
selectable. The provider is `crates/preview/src/textfile.rs`, and the dispatch is
`crates/preview/src/kind.rs` (about 120 known extensions; unknown ones are content-sniffed).

**The request most likely comes from one of these gaps.** Each was found by reading the code; the
reporter should confirm which files failed for them (see Open questions).
1. **UTF-16 or UTF-32 files**, common for files saved by Windows tools, PowerShell and Visual Studio.
   The sniff rejects them because of NUL bytes, and known extensions like `.txt` get decoded as
   UTF-8, which produces garbage.
2. **Legacy 8-bit encodings** (Windows-1252, Latin-1) show `�` for every umlaut or accent.
3. **Missing extensions**, mostly .NET and build files: `csproj`, `fsproj`, `vbproj`, `sln`, `slnx`,
   `props`, `targets`, `xaml`, `axaml`, `razor`, `cshtml`, `resx`, `nuspec`, `ruleset`,
   `runsettings`, `http`, `bicep`, `hcl`, `dockerfile`, `containerfile`, `jenkinsfile`.
   Known extension-less names should also count: `Dockerfile`, `Containerfile`, `Jenkinsfile`,
   `Vagrantfile`, `Gemfile`, `Rakefile`, `LICENSE`, `README`, `CHANGELOG`, `AUTHORS`, `COPYING`.
   These are sniffed today, but sniffing fails on UTF-16 and on files that start with binary-looking
   bytes.
4. **Only the first 64 KiB** is shown, and the only hint is "… (truncated)". There is no way to read
   further.
5. **Ctrl+C conflict.** With text selected in the preview, `Ctrl+C` copies the *selected row's
   path*, because the global shortcut wins, not the selected text.
6. **Long lines** need horizontal scrolling, and there is no wrap option. With Phase A's fixed width,
   wrapping becomes the better default.

### Design
- **Encoding detection** (`preview/src/textfile.rs`), with no new dependency for the common cases:
  1. BOM: UTF-8 BOM (strip it), UTF-16 LE/BE (decode with `char::decode_utf16`), UTF-32 LE/BE.
  2. No BOM, but NUL bytes at every second position in the first 8 KiB: heuristic UTF-16 LE/BE.
  3. Valid UTF-8 (allowing a truncated final sequence at the 64 KiB cut): use it as is.
  4. Otherwise, if no NUL bytes and under 30 % invalid sequences: decode as Windows-1252 with a
     32-entry table for 0x80–0x9F plus Latin-1 for the rest. This is a small hand-written module
     with tests; `encoding_rs` is only worth adding if more encodings are requested.
  5. Otherwise it is binary, and the preview is `Unavailable { hint: "Binary file" }`.
- `PreviewContent::Text` gains `encoding: TextEncoding` (an enum whose `name()` is the display
  string), `total_bytes: u64` and `end_offset: u64` (where the next chunk starts), so the header can show
  "UTF-16 LE · 1.2 MiB · showing first 64 KiB". `PreviewContent` is `#[non_exhaustive]`, but its
  fields are matched in the app, so the app is updated in the same commit.
- **Recognition:** add the extensions in gap 3, plus a `KNOWN_TEXT_FILENAMES` table matched on
  the whole name (case-insensitive). The sniff accepts UTF-16 per step 2.
- **More content:** a "Show more" button under truncated text requests the next 256 KiB, through a
  new `PreviewService::request_range(path, offset, len, encoding)` on the preview worker, and
  appends it. It stops at a hard cap of 8 MiB per file. Large text is split into display rows once
  (`app/text_doc.rs`) and only the rows in view are laid out (`ScrollArea::show_viewport`), so the
  frame budget holds (ROADMAP §9: `update()` under 4 ms). Selection is a byte range of the whole
  text, so `Ctrl+A`/`Ctrl+C` also cover rows scrolled out of view.
- **Rendering** (`app/ui/preview_pane.rs`):
  - Monospace, a tab width of 4 spaces, an optional line-number gutter, and a **Wrap** toggle in the
    pane header (default on; saved as `ui.preview_pane.wrap_text`).
  - With wrap off, the text area scrolls horizontally *inside* the fixed-width pane.
  - `Ctrl+C` / `Ctrl+A` act on the preview text while it has focus or a selection. The global
    "copy path" shortcut only fires when the table has focus. That needs a check in
    `app/frame.rs` before consuming `Ctrl+C`.
- **Syntax highlighting (optional, stretch):** `syntect` behind a `highlight` cargo feature of
  `everythux-preview`, off by default until binary size is measured (target: under 2 MB added to
  the stripped binary). It returns spans (`Vec<(Range<usize>, Style)>`) so the crate stays
  UI-agnostic. Skip it if the size or startup cost is too high.

### Tasks
- [x] Ask the reporter which files didn't preview, and add each one as a regression fixture.
      Not asked; instead generated fixtures cover every gap above (UTF-16 LE/BE with and without
      BOM, UTF-32, Windows-1252, `.csproj`, `Dockerfile`, extension-less names, a 5 MB log) in
      `crates/preview/tests/end_to_end.rs`. Add the reporter's files when they answer.
- [x] `preview/textfile.rs`: BOM, UTF-16/32, UTF-8 and Windows-1252 decoding, with the improved
      binary check.
- [x] `preview/kind.rs`: the new extensions and the `KNOWN_TEXT_FILENAMES` table.
- [x] `PreviewContent::Text { encoding, total_bytes }` and the header line in the app.
- [x] "Show more" range requests with the 8 MiB cap; `show_rows` rendering for large text.
- [x] Wrap toggle and line numbers; `Ctrl+C`/`Ctrl+A` routed to preview text when it has focus.
- [ ] (Stretch) `highlight` feature with `syntect`; measure the binary size delta before enabling.
      Deferred: not part of the v0.5 Phase B implementation.
- [x] Docs: update the Preview pane section in `docs/shortcuts.md` and `docs/troubleshooting.md`.

### Tests
- `preview` unit tests with generated fixtures:
  - UTF-8 with and without BOM, UTF-16 LE/BE with and without BOM, UTF-32, and Windows-1252
    (`"Grüße €"` round-trips).
  - A truncated multi-byte sequence at the 64 KiB cut.
  - Binary files (PNG bytes, an ELF header, random bytes) are rejected.
  - Every new extension and known filename classifies as `Kind::Text`.
- `preview` end-to-end: a 1 MiB text file previews the first chunk, `request_range` returns the next
  one, and the cap is enforced.
- `app` kittest: with preview text selected, `Ctrl+C` puts the selected text on the clipboard, not
  the path; with the table focused it copies the path, as today. The wrap toggle is persisted.
- Visual check under Xvfb: UTF-16 `.txt`, `.csproj`, `Dockerfile`, a 5 MB log file.

### Done when
Every text file a developer typically has (source, configs, logs, .NET project files, Windows UTF-16
text) shows readable text within 150 ms (ROADMAP §9 Phase 8 target). Binary files never show
garbage, and copying text from the preview works.

---

## Phase C (F2): Multiple search bars (AND)

### Current state
- One search box: `crates/app/src/ui/search_bar.rs` (35 lines). Its state is in
  `crates/app/src/app/search.rs`.
- `SearchRequest { text: String, … }` (`crates/search/src/service.rs`) is parsed by
  `everythux_search::parse`. In the query language, whitespace already means AND, and `|` binds
  tighter than AND.
- `Tab` currently moves egui focus between table cells, because cells are focusable.

### Design
- **Model.** `SearchState.bars: Vec<SearchBar>`, where `SearchBar { id: u32, text: String,
  error: Option<String> }`.
  - Bar 0 always exists and can't be closed.
  - Ids are stable, so egui widget ids and focus survive closing a bar in the middle.
  - There is a soft limit of 8 bars, after which the `+` button is disabled with a tooltip.
- **Semantics.** Each bar is parsed **on its own**, and the resulting queries are combined with AND.
  The texts are never concatenated, so an unterminated quote or a trailing `|` in one bar can't leak
  into another. Empty bars are ignored.
  - `search` crate: add `Query::all(Vec<Query>) -> Query` (the AND of sub-queries, keeping the
    existing cost-based clause ordering), and `parse_many(&[&str], QueryOptions) ->
    Result<Query, (usize, QueryError)>`, which reports the index of the failing bar.
  - `SearchRequest.text: String` becomes `texts: Vec<String>`. This is a contract change: update
    `everythux-cli search`, which passes one element, and its tests.
  - The match options (case, path, regex) stay global and apply to every bar. Regex mode: each bar
    is its own regex.
  - A parse error is shown under **that** bar; results keep the previous list (today's behaviour).
- **UI** (`ui/search_bar.rs`):
  - Bar 0 is a full-width text field with a `+` button inside its right edge, styled like the
    existing toggles and using a painted icon from `ui/icons.rs`. The tooltip reads "Add a search
    bar (AND) · Ctrl+T".
  - Each extra bar sits directly underneath with the same width, a small "AND" label on the left and
    an `×` button on the right (tooltip "Close this search bar · Ctrl+W").
  - Clicking `+` or pressing `Ctrl+T` appends a bar, focuses it and re-runs the search (an empty
    bar changes nothing).
  - Clicking `×` or pressing `Ctrl+W` in a focused extra bar removes it, moves focus to the bar
    above, and re-runs the search.
- **Focus and keyboard.** "Newest bar" means the last bar in the list, which is the most recently
  added one.
  - `Tab` with the table focused (or nothing focused) focuses the newest bar.
  - `Tab` with any search bar focused focuses the table, keeping the selection (or selecting the
    first row if there is none).
  - `Shift+Tab` does the same, in reverse.
  - `Ctrl+F` focuses bar 0, as today.
  - The Up/Down/PageUp/PageDown keys typed in a search bar keep working as they do today (they move
    the table selection).
  - `Tab` is consumed globally in `app/frame.rs` *before* widgets, so egui's default focus cycling
    through table cells no longer happens. Cells stay clickable.
  - All new bindings (`add_search_bar = "Ctrl+T"`, `close_search_bar = "Ctrl+W"`,
    `switch_focus = "Tab"`) go into `Keybindings` with defaults, and are documented.
- **Persistence.** Extra bars are kept for the session only. On startup there is one empty bar, the
  same as today. A `ui.remember_search_bars` option can follow if wanted (see Open questions).
- **Status bar and window title** show the combined query, for example `rust AND path:src AND
  dm:thisweek`, so it's clear which bars are active.

### Tasks
- [x] `search`: `Query::all`, `parse_many` with per-bar error index; `SearchRequest.texts`; service
      and CLI updated; proptest that `parse_many([a, b])` matches the same entries as
      `parse("(a) (b)")` wherever the single-string form is well defined.
- [x] `core`: new `Keybindings` fields with defaults; docs in `docs/configuration.md`.
- [x] `app/app/search.rs`: the `bars` model with add, close and edit, building `texts` for the
      request; a result that comes back with a bar error is attached to that bar.
- [x] `app/ui/search_bar.rs`: the bar stack, `+` and `×` buttons, the AND labels and per-bar errors.
- [x] `app/app/frame.rs`: global `Tab`/`Shift+Tab`/`Ctrl+T`/`Ctrl+W` handling and focus routing;
      the newest-bar tracking.
- [x] `docs/search-syntax.md` (multiple bars) and `docs/shortcuts.md`.

As built: `parse_many` returns `Result<Query, BarQueryError { bar, error }>` instead of a tuple, and
`SearchResults.error` became `Option<SearchError { bar, message }>`. The query language has no
parentheses, so the proptest compares against the bars joined with a space. `Ctrl+W` is also consumed
(as a no-op) in the first bar, so the text field doesn't treat it as "delete word". `Esc` clears the
focused bar. The status bar shows the combined query once there are two or more bars; the window title
always shows it.

### Tests
- `search`: `Query::all` semantics (empty list matches everything, single bar equals plain parse);
  error index reporting; property test against a naive AND of the sub-queries.
- `app` kittest:
  - `plus_adds_a_bar_below_and_focuses_it`.
  - `closing_a_middle_bar_keeps_the_others_and_their_focus_ids`.
  - `first_bar_has_no_close_button`.
  - `bars_combine_with_and`: typing `a` in bar 0 and `b` in bar 1 sends `texts == ["a", "b"]`, and
    the fake results are filtered accordingly.
  - `parse_error_is_shown_under_the_failing_bar_only`.
  - `tab_toggles_between_table_and_newest_bar`: with three bars, `Tab` from the table focuses
    bar 3; from bar 1 it focuses the table; the selection is kept.
  - `tab_does_not_cycle_through_table_cells`.
  - `ctrl_t_and_ctrl_w_add_and_close_bars`.
  - `eight_bar_limit_disables_plus`.

### Done when
A user can type a broad query in bar 1 (`ext:pdf`), narrow it in bar 2 (`invoice`) and bar 3
(`dm:thisyear`), close any bar but the first, and move between the list and the newest bar with
`Tab`. The results always equal the AND of the non-empty bars.

---

## Phase D (F1): Inline rename in the result list

### Current state
- No file operations exist yet. `platform` only opens files and folders and shows properties.
- Name cell: double-click opens the file, single click selects.
- The index learns about renames only through inotify (in watched roots) or rescans. A
  renamed *folder* would make `RefreshDirs` delete the old subtree and re-walk the new one. That is
  correct but slow for big folders.
- Selection remapping after a snapshot change is by full path (`26b6741`), so a renamed entry
  would lose its selection.

### Design
- **Starting a rename** (only on online volumes, one entry at a time):
  - `F2` on the selected row; Everything uses F2 too.
  - Context menu → **Rename…**.
  - Optional: a slow second click on the Name cell of the already-selected row (more than 500 ms
    after the selecting click, and not part of a double-click). This is behind
    `ui.rename_on_slow_click`, default `false`, so double-click-to-open never turns into an
    accidental rename (see Open questions).
- **The editor:**
  - The Name cell turns into a single-line `TextEdit` with the same font and row height, so the row
    doesn't jump.
  - It opens with the **stem** selected (the name without its extension; folders get the whole
    name), the way file managers do.
  - `Enter` or `Tab` commits; `Esc` cancels.
  - Clicking elsewhere commits, the same as Windows Explorer. If the name is unchanged, nothing
    happens.
  - While editing, the global shortcuts that would steal typed keys (`Ctrl+C` path copy, `F5`,
    `Tab` focus switching, typing into the search bar) are suspended. `Ctrl+Shift+R` still works.
- **Validation, before calling the filesystem.** Errors appear inline, as red text under the cell or
  a tooltip, and the editor stays open:
  - The name is empty, `.` or `..`, or contains `/` or NUL.
  - The name is longer than 255 bytes (ext4 `NAME_MAX`).
  - The original name isn't valid UTF-8 and the user left the replacement characters in. The
    warning reads "This name contains bytes that can't be shown; type the full new name".
  - Leading or trailing whitespace gets a warning and needs a second `Enter`.
  - A changed extension gets the warning "Change the extension from .txt to .md?", also needing a
    second `Enter` (as in Nautilus and Explorer).
- **Doing the rename** (`platform::fileops`, a new module):
  - `pub fn rename_in_place(path: &Path, new_name: &OsStr) -> Result<PathBuf, RenameError>`.
  - It uses `rustix::fs::renameat_with(dirfd, old, dirfd, new, RenameFlags::NOREPLACE)`. This is
    atomic and never overwrites an existing file, which ext4 supports. On `EINVAL` (a filesystem
    without `NOREPLACE`) it falls back to "check that the target doesn't exist, then `renameat`"
    and documents the race.
  - `RenameError` covers `AlreadyExists`, `PermissionDenied`, `NotFound` (it vanished meanwhile),
    `Busy` (a mount point), `ReadOnlyFilesystem`, `InvalidName` and `Other(io::Error)`, each with a
    clear message.
  - It runs on the platform worker thread, never on the UI thread.
  - This adds `rustix` (workspace, feature `fs`) to `everythux-platform`. There is no `unsafe`.
- **Updating the index right away**, without waiting for inotify or a rescan:
  - New `Job::Renamed { from: PathBuf, to: PathBuf }` in `index`. It is applied as one transaction
    in `Store::rename_path`:
    - Files: update `entries.name`, `name_fold` and `ext`.
    - Folders: update the folder's entry name, then rewrite the path prefix of every descendant in
      `dirs` (`UPDATE dirs SET path = :to || substr(path, length(:from) + 1) WHERE path = :from OR
      path >= :from || '/' AND path < :from || '0'`).
  - The job publishes a snapshot like any other job. The cost is O(descendant folders) in SQLite
    instead of a re-walk.
  - If the watcher also reports the rename, the following `RefreshDirs` finds nothing to change.
- **Selection and preview follow the file.** The app records `pending_reselect = new path` and the
  snapshot remap (`app/search.rs`) prefers it, so the renamed row stays selected even if it moves
  in the sort order. The table scrolls to it.
- **Undo.** After a successful rename, a toast reads "Renamed to `x` · Undo". Undo renames back with
  the same `NOREPLACE` call while the toast is visible (10 s).
- **Not in scope for v0.5:** batch rename of several files, moving to another folder, and renaming
  across filesystems (impossible with `rename(2)` anyway).

### Tasks
- [ ] `platform/fileops.rs`: `rename_in_place` + `RenameError`, `NOREPLACE` with fallback; unit tests
      on temp dirs (success, exists, missing source, permission denied when not root, non-UTF-8
      names, 255-byte limit).
- [ ] `index`: `Job::Renamed`, `Store::rename_path` for files and folders (prefix rewrite),
      `total_size` unchanged for the subtree; integration tests including a renamed folder with
      10 000 descendants (no re-walk: check `dirs_listed == 0` in `JobStats`), and one test that a
      watcher event after the job is a no-op.
- [ ] `app`: rename state (`editing: Option<RenameEdit { entry, buffer, warning, confirmed }>`),
      the editor in `ui/table.rs` Name cell, `F2` and the context-menu entry, validation, the
      platform action and result event, the `Job::Renamed` submit, `pending_reselect`, and the
      Undo toast.
- [ ] Shortcut suspension while editing (`app/frame.rs`); `Tab` commits instead of switching focus.
- [ ] Optional `ui.rename_on_slow_click` (default off).
- [ ] Docs: `docs/shortcuts.md` (F2, Enter, Esc), README feature list.

### Tests
- `platform`: the cases listed in the tasks; `NOREPLACE` never overwrites (create the target first,
  then rename and expect `AlreadyExists`, with the target content unchanged).
- `index`: file rename, folder rename with deep descendants (paths, `parent`, `total_size`,
  snapshot invariants via `check_invariants`), renaming onto a name in another folder is rejected,
  a rename of an excluded name (for example, to `node_modules`) removes the subtree from the index.
- `app` kittest:
  - `f2_opens_the_editor_with_the_stem_selected`.
  - `enter_commits_and_submits_platform_rename`.
  - `esc_cancels_without_side_effects`.
  - `invalid_names_show_inline_errors`.
  - `extension_change_needs_confirmation`.
  - `renamed_row_stays_selected_after_snapshot`.
  - `undo_toast_renames_back`.
  - `typing_in_the_editor_does_not_trigger_global_shortcuts`.
  - `rename_disabled_on_offline_volumes`.
- Manual check on a real ext4 volume: rename a file, a folder with many children, a file in a
  read-only folder (error), and a file whose name collides with an existing one (error, nothing
  overwritten).

### Done when
Pressing F2, typing a new name and pressing Enter renames the file on disk atomically without ever
overwriting another file. The list shows the new name at once and keeps it selected, and Undo
restores the old name. A failed rename explains why and changes nothing.

---

## Cross-cutting

- **Performance budgets** (ROADMAP §9) stay acceptance criteria. Multiple bars must not add more
  than 10 % to search latency on 2M entries for two or three bars (`search/multi_bar_2m` bench).
  Rename jobs must not block searching.
- **Contract changes:**
  - `SearchRequest.texts` (Phase C).
  - `PreviewContent::Text` fields (Phase B).
  - `Job::Renamed` (Phase D).
  - New `Keybindings` and `PreviewPaneConfig` fields, all defaulted, so old configs still load.
- **Docs:** `docs/shortcuts.md`, `docs/search-syntax.md`, `docs/configuration.md` and the README
  feature list are updated in the phase that changes them.
- **Release:** after D, bump the workspace version to 0.5.0 and tag `v0.5.0` (the release workflow
  and `build-release.sh` are already in place).

## Open questions for the reporter

1. **Text preview:** which files didn't preview for you (extension, and if you know it, the encoding,
   for example a file saved from Windows)? `main` previews `.rs`, `.txt`, `Makefile` and `.gitconfig`
   correctly, so Phase B's fixes target UTF-16, legacy encodings and .NET or extension-less names.
   Real examples would make sure the right gap is fixed.
2. **Rename trigger:** is `F2` plus the context menu enough, or should a slow second click on the
   selected name also start a rename? The slow click is planned behind an option, off by default,
   to avoid accidental renames.
3. **Rename on focus loss:** should clicking elsewhere commit the rename (as in Windows Explorer,
   planned) or cancel it (as in some Linux file managers)?
4. **Search bars across restarts:** should the extra bars be restored on next start, or always start
   with one empty bar (planned)?
5. **"Latest search bar":** the plan reads it as the newest, bottom-most bar. Should `Tab` instead
   return to the bar you were last typing in?
6. **Fixed pane width:** is a fixed width with presets and a custom value enough, with dragging
   available as an opt-in, or should dragging be removed completely?
