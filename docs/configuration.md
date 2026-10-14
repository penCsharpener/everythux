# Configuration

Everythux stores its settings in one JSON file.

| What | Default location | Override |
|------|------------------|----------|
| Config file | `$XDG_CONFIG_HOME/everythux/config.json` (`~/.config/everythux/config.json`) | `EVERYTHUX_CONFIG=<file>`, or `--config` in the CLI |
| Index database | `$XDG_DATA_HOME/everythux/index.db` (`~/.local/share/everythux/index.db`) | `EVERYTHUX_DB=<file>`, or `--db` in the CLI |
| Log files | `$XDG_STATE_HOME/everythux/` (`~/.local/state/everythux/`) | |

`everythux-cli config path` prints the config file location, `config show` prints the effective settings and
`config reset --yes` overwrites the file with the defaults.

## How the file is read

- Every field is optional. A missing field takes its default and unknown fields are ignored, so you only need
  to write what you change, and files from older or newer versions still load.
- `version` is the schema version (currently `1`). Older files are upgraded automatically when loaded.
- The file must be valid JSON (no comments, no trailing commas). If it isn't, Everythux reports the
  line and column and does not overwrite your file.
- The UI saves changes atomically (temp file, then rename). The desktop app reloads the file when it changes on disk.

## `volumes`

| Field | Default | Meaning |
|-------|---------|---------|
| `auto_discover_fs_types` | `["ext4"]` | Filesystem types indexed automatically. Mounted filesystems of these types are found through `/proc/self/mountinfo`; bind mounts are indexed once |
| `include` | `[]` | Extra roots to index even if their filesystem type is not auto-discovered, e.g. `["/mnt/usb"]` |
| `disabled_uuids` | `[]` | Filesystem UUIDs to skip |

## `exclusions`

A list of rules, evaluated in order. Each rule has `pattern`, `kind` (`dir`, `file`, `path`, `any`), an optional `scope`
and `enabled`. See [exclusions.md](exclusions.md) for the semantics and the default list.

## `ui`

| Field | Default | Meaning |
|-------|---------|---------|
| `columns` | all five, visible | List of `{ "id", "width", "visible" }` in display order. `id` is `name`, `path`, `type`, `modified` or `size`; `width` is in logical pixels. Defaults: name 280, path 420, type 140, modified 150, size 90 |
| `sort` | `{ "column": "name", "ascending": true }` | Sort order of the result table |
| `preview_pane` | `{ "enabled": false, "width": 360, "resizable": false, "max_file_size": 536870912 }` | Preview pane. `width` is the fixed pane width in logical pixels, set in View → Preview pane (presets or Custom…, 200–1200). The previewed file never changes it; a window too narrow for it shrinks the pane for the moment without changing the setting. `resizable: true` also lets you drag the pane edge, which saves the new width. Files larger than `max_file_size` bytes (512 MiB) are not previewed |
| `properties_target` | `"parent"` | What a Size or Modified double-click shows properties for: `"parent"` (containing folder) or `"item"` |
| `rename_on_slow_click` | `false` | Start an inline rename with a slow second click on the selected row's name (F2 and the context menu always work) |
| `size_format` | `"binary"` | `"binary"` (KiB, MiB), `"si"` (kB, MB) or `"bytes"` (exact) |
| `date_format` | `"%Y-%m-%d %H:%M"` | [`chrono` strftime](https://docs.rs/chrono/latest/chrono/format/strftime/index.html) format for the Date Modified column |
| `theme` | `"system"` | `"system"`, `"light"` or `"dark"` |

## `keybindings`

Strings made of modifiers and a key (or mouse button) joined with `+`. A binding that can't be parsed
falls back to its default, with a warning in the log.

| Field | Default | Action |
|-------|---------|--------|
| `reindex_all` | `"Ctrl+Shift+R"` | Full reindex of all volumes |
| `quick_rescan` | `"F5"` | Quick rescan |
| `exclude_path_click` | `"Ctrl+Shift+LeftClick"` | Click on a Path cell that adds the path to the exclusions |
| `toggle_preview` | `"Ctrl+P"` | Show or hide the preview pane |
| `focus_search` | `"Ctrl+F"` | Focus the first search bar |
| `toggle_match_path` | `"Ctrl+U"` | Match against the full path |
| `add_search_bar` | `"Ctrl+T"` | Add a search bar below the others; its query is combined with AND |
| `close_search_bar` | `"Ctrl+W"` | Close the focused extra search bar (the first bar can't be closed) |
| `switch_focus` | `"Tab"` | Move the focus between the result list and the newest search bar. The same key with `Shift` added does the same |

## `indexer`

| Field | Default | Meaning |
|-------|---------|---------|
| `threads` | `0` | Walker threads. `0` means one per CPU |
| `follow_symlinks` | `false` | Follow symbolic links while scanning. Loops are detected either way |
| `one_filesystem` | `true` | Never descend into another filesystem. Nested mounts are separate volumes |
| `rescan_on_startup` | `"quick"` | `"none"` (use the index as it is), `"quick"` (re-list folders whose modification time changed) or `"full"`. An empty database always gets a full scan |
| `scheduled_rescan_minutes` | `0` | Run a quick rescan every N minutes. `0` turns it off |
| `live_updates` | `true` | Watch for changes with inotify while the app runs |
| `watch_roots` | `[]` | Folders watched with inotify. Empty means your home directory. Mind the [watch limit](troubleshooting.md#inotify-watch-limit) |

## `search`

Defaults for the search switches in the UI. See [search-syntax.md](search-syntax.md).

| Field | Default | Meaning |
|-------|---------|---------|
| `case_sensitive` | `false` | Case-sensitive matching |
| `match_path` | `false` | Match terms against the full path instead of the name |
| `regex` | `false` | Treat the whole query as a regular expression |
| `max_results` | `0` | Maximum number of results. `0` means unlimited |

`everythux-cli search` does not read these; it uses only its own flags (`--case`, `--match-path`, `--regex`, `--limit`).

## Example

A minimal file that adds an external disk, disables live updates and shows sizes in SI units:

```json
{
  "volumes": { "include": ["/mnt/archive"] },
  "indexer": { "live_updates": false },
  "ui": { "size_format": "si" }
}
```

Note that `exclusions` is not listed, so the default rules apply. If you write an `exclusions` array, it
replaces the defaults entirely.
