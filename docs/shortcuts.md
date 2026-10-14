# Keyboard and mouse reference

All keyboard shortcuts and the exclusion click can be changed in `config.json` under
`keybindings` (see [configuration.md](configuration.md)). The values below are the defaults.

## Keyboard

| Input | Context | Action |
|-------|---------|--------|
| Typing | anywhere | Search. The search box is focused on start |
| `Ctrl+F` | global | Focus the first search bar |
| `Ctrl+T` | global | Open a new tab with an empty search and focus it (at most 32 tabs) |
| `Ctrl+W` | global | Close the current tab. The last tab stays |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | global | Next / previous tab, wrapping around |
| `Ctrl+Shift+T` | global | Add a search bar below the others and focus it. Its query is combined with AND (at most 8 bars) |
| `Ctrl+Shift+W` | extra search bar | Close the focused search bar and focus the one above. Does nothing in the first bar |
| `Tab` / `Shift+Tab` | global | From the result list (or with nothing focused): focus the newest, bottom-most search bar. From any search bar: focus the result list, keeping the selection (or selecting the first row). `Tab` never walks through the table cells |
| `Esc` | search bar | Clear the focused search bar (the first bar when no bar has the focus) |
| `Ctrl+Shift+R` | global | Full reindex of all volumes (background, cancellable). Works while the search box has focus |
| `F5` | global | Quick rescan |
| `Ctrl+U` | global | Match against the full path instead of the name |
| `Ctrl+P` | global | Show or hide the preview pane |
| `Enter` | row | Open the file with its default application |
| `Ctrl+Enter` | row | Open the containing folder |
| `F2` | row | Rename the selected file or folder inline. The name without its extension is preselected |
| `Enter` or `Tab` | rename editor | Commit the new name. A second `Enter` is needed after a warning (changed extension, leading or trailing space) |
| `Esc` | rename editor | Cancel the rename |
| `Ctrl+C` | row | Copy the full path (only while no text is selected in the preview and no text field has the focus) |
| `Ctrl+Shift+C` | row | Copy the folder path |
| Arrow keys, `PageUp`, `PageDown`, `Home`, `End` | result list | Move the selection |
| `Ctrl+A` | preview text (clicked) | Select all loaded text of the preview |
| `Ctrl+C` | preview text (clicked or with a selection) | Copy the selected preview text |

## Mouse

| Input | Context | Action |
|-------|---------|--------|
| Click `+` | first search bar | Add a search bar (AND). Disabled once there are 8 bars |
| Click `×` | extra search bar | Close that search bar |
| Click on a column header | header | Sort by that column. Click again to reverse |
| Drag a header edge | header | Resize the column |
| Drag the preview pane edge | preview pane | Resize the pane. Only when View → Preview pane → **Allow resizing by dragging** is on; otherwise the width is fixed and set in the same menu |
| Double-click | Name cell | Open the file with its default application |
| Double-click | Path cell | Open the containing folder in the file manager, with the file selected |
| Double-click | Size or Modified cell | Open the folder's properties |
| `Ctrl+Shift+Left-click` | Path cell | Add the path to the exclusions and open the exclusion popup |
| Right-click | row | Context menu: open, open containing folder, copy path, copy name, rename, properties, exclude, re-index this folder |
| Click | selected Name cell | Rename inline, when `ui.rename_on_slow_click` is `true` (default `false`). Needs a click more than 0.5 s after the row was selected that is not part of a double-click |
| `Shift+Click` | row | Extend the selection (a range). `Ctrl+Shift+Click` is taken by the exclusion action |

## Notes

- **Preview pane text:** click into the text to give it the keyboard, drag to select, `Shift+Click` to
  extend. While it has the focus or a selection, `Ctrl+A` and `Ctrl+C` act on the text; clicking a row
  in the result list ends the selection, and `Ctrl+C` copies paths again. The header shows the
  encoding and how much of the file is loaded; **Show more** loads the next 256 KiB, up to 8 MiB.
  **Wrap** and **Line numbers** in the header are saved in the config.

- **Renaming** works on one entry at a time, on online volumes only. Clicking elsewhere also commits
  the name; an unchanged name does nothing. Names that are empty, `.`/`..`, contain `/` or are longer than
  255 bytes are refused inline. An existing file is never overwritten. While the editor is open, typed keys
  stay in it: `F5`, `Ctrl+C`, the arrow keys and the other global shortcuts are suspended, except
  `Ctrl+Shift+R`. After a rename, the status bar offers **Undo** for 10 seconds.
  `Tab` in the editor commits the name; it doesn't move the focus to a search bar.
- **Several search bars** are combined with AND; see [search-syntax.md](search-syntax.md#several-search-bars).
  The arrow keys, `PageUp`, `PageDown`, `Enter` and `F2` work from any search bar as they do from the first.

- **Folder properties** shows the properties of the containing folder for a file, and of the folder
  itself for a folder row. Set `ui.properties_target` to `"item"` to show the clicked item instead. If no file manager
  can show properties, Everythux opens its own Properties dialog.
- **Opening a folder with the file selected** needs a file manager that implements
  `org.freedesktop.FileManager1` (Nautilus, Dolphin, Nemo, Caja, Thunar do). Otherwise the folder is opened
  without a selection. See [troubleshooting.md](troubleshooting.md).
- **Binding format:** modifiers and a key joined by `+`, e.g. `"Ctrl+Shift+R"`, or a mouse button such as
  `"Ctrl+Shift+LeftClick"`. A binding that can't be parsed falls back to its default and a warning is logged.
