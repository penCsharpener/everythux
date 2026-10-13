# Keyboard and mouse reference

All keyboard shortcuts and the exclusion click can be changed in `config.json` under
`keybindings` (see [configuration.md](configuration.md)). The values below are the defaults.

## Keyboard

| Input | Context | Action |
|-------|---------|--------|
| Typing | anywhere | Search. The search box is focused on start |
| `Ctrl+F` | global | Focus the search box |
| `Ctrl+Shift+R` | global | Full reindex of all volumes (background, cancellable). Works while the search box has focus |
| `F5` | global | Quick rescan |
| `Ctrl+U` | global | Match against the full path instead of the name |
| `Ctrl+P` | global | Show or hide the preview pane |
| `Enter` | row | Open the file with its default application |
| `Ctrl+Enter` | row | Open the containing folder |
| `Ctrl+C` | row | Copy the full path |
| `Ctrl+Shift+C` | row | Copy the folder path |
| Arrow keys, `PageUp`, `PageDown`, `Home`, `End` | result list | Move the selection |

## Mouse

| Input | Context | Action |
|-------|---------|--------|
| Click on a column header | header | Sort by that column. Click again to reverse |
| Drag a header edge | header | Resize the column |
| Double-click | Name cell | Open the file with its default application |
| Double-click | Path cell | Open the containing folder in the file manager, with the file selected |
| Double-click | Size or Modified cell | Open the folder's properties |
| `Ctrl+Shift+Left-click` | Path cell | Add the path to the exclusions and open the exclusion popup |
| Right-click | row | Context menu: open, open containing folder, copy path, copy name, properties, exclude, re-index this folder |
| `Shift+Click` | row | Extend the selection (a range). `Ctrl+Shift+Click` is taken by the exclusion action |

## Notes

- **Folder properties** shows the properties of the containing folder for a file, and of the folder
  itself for a folder row. Set `ui.properties_target` to `"item"` to show the clicked item instead. If no file manager
  can show properties, Everythux opens its own Properties dialog.
- **Opening a folder with the file selected** needs a file manager that implements
  `org.freedesktop.FileManager1` (Nautilus, Dolphin, Nemo, Caja, Thunar do). Otherwise the folder is opened
  without a selection. See [troubleshooting.md](troubleshooting.md).
- **Binding format:** modifiers and a key joined by `+`, e.g. `"Ctrl+Shift+R"`, or a mouse button such as
  `"Ctrl+Shift+LeftClick"`. A binding that can't be parsed falls back to its default and a warning is logged.
