# Troubleshooting

## Files are missing from the results

**Unreadable folders.** Everythux runs as your user and indexes what that user can read. Folders such as `/root`
or other users' home directories are skipped, and the error is counted rather than stopping the scan.
The count appears as "errors" in the summary printed by `everythux-cli reindex`, and `-v`/`-vv` show more detail.
Running as root would index everything, but is not recommended for the desktop app.

**An exclusion rule matches.** Check with `everythux-cli exclude test /path/to/file`. A rule such as `bin`
without a scope also hides `/usr/bin`. See [exclusions.md](exclusions.md).

**The volume isn't indexed.** `everythux-cli volumes` lists known and mounted volumes. By default only
mounted **ext4** filesystems are indexed. Add other mount points under `volumes.include`, or check
that the volume's UUID is not in `volumes.disabled_uuids`. `everythux-cli scan --dry-run` shows what would be indexed.

**The index is stale.** Run `everythux-cli reindex --quick` (or press `F5`). A quick rescan only
re-lists folders whose modification time changed, so it can miss files whose size or time changed inside an
otherwise unchanged folder. Use `reindex --full` (or `Ctrl+Shift+R`) for a complete rescan, or
`reindex --path <folder>` for one folder.

**Entries from a disk that is not connected** stay in the index and show as offline. They come back online when
the disk is mounted again, even at a different mount point.

## Inotify watch limit

With `indexer.live_updates` on, Everythux watches folders with inotify (by default below your home
directory). Linux limits how many watches a user may have. When the limit is reached, changes in
unwatched folders are not noticed until the next quick rescan.

Check and raise the limit:

```sh
cat /proc/sys/fs/inotify/max_user_watches
echo 'fs.inotify.max_user_watches=524288' | sudo tee /etc/sysctl.d/60-inotify.conf
sudo sysctl --system
```

Each watch uses a small amount of kernel memory (about 1 KiB). Alternatively reduce `indexer.watch_roots`
to the folders that change often and rely on `scheduled_rescan_minutes` for the rest.

## "Open containing folder" doesn't select the file

Selecting a file needs a file manager that implements the D-Bus interface `org.freedesktop.FileManager1`.
Nautilus, Dolphin, Nemo, Caja and Thunar do. Without one, Everythux falls back to the OpenURI portal and then to
`xdg-open`, which opens the folder but can't select anything. Check that a file manager is running and
that the session bus is available:

```sh
gdbus introspect --session --dest org.freedesktop.FileManager1 --object-path /org/freedesktop/FileManager1
```

If that fails, install or start a file manager that supports it. Folder **properties** (double-click on Size or
Modified) work the same way (`ShowItemProperties`); if the file manager lacks it, Everythux shows its own Properties dialog.

## No previews

The preview pane (`Ctrl+P`) uses optional tools and degrades to file information when they are missing:

| Type | Needs |
|------|-------|
| Images, text | nothing extra |
| PDF | `poppler-utils` (`pdftoppm`) |
| Video, audio cover art | `ffmpeg` (or a thumbnailer service) |

Install them with your package manager, for example `sudo apt install poppler-utils ffmpeg` or
`sudo pacman -S poppler ffmpeg`. Files larger than `ui.preview_pane.max_file_size` are never previewed.
Thumbnails that your file manager already generated (`~/.cache/thumbnails`) are used first.

### Text files

Text previews detect UTF-8 (with or without BOM), UTF-16 and UTF-32 (with a BOM, or without one
for mostly-ASCII text) and fall back to Windows-1252 for legacy 8-bit files. The header under the
file name shows the detected encoding. Files are recognised by about 160 extensions, by names such
as `Dockerfile`, `Jenkinsfile`, `LICENSE` or `.bashrc`, and otherwise by their content.

- **"Binary file" / "No preview available for this file type":** the content has NUL bytes or too
  many invalid characters for any of these encodings. Other legacy encodings (Shift-JIS, KOI8-R, …)
  are not detected.
- **Only part of the file is shown:** the preview loads the first 64 KiB. **Show more** loads 256 KiB
  at a time, up to 8 MiB; open the file for the rest.
- **`Ctrl+C` copies the path instead of the selected text:** click into the preview text first, or
  select with the mouse; a click in the result list ends the text selection on purpose.

## Rebuilding a corrupt or unwanted index

The index is only a cache of what is on disk, so it is always safe to rebuild:

1. Try a full reindex first: press `Ctrl+Shift+R`, or run `everythux-cli reindex --full`. The new
   database is built beside the old one and swapped in when finished, so the old index stays searchable
   and survives an interrupted run.
2. If Everythux reports a database error or the schema is newer than the program, close the app and delete the
   database files, then start it again (an empty database triggers a full scan):

   ```sh
   rm -f ~/.local/share/everythux/index.db ~/.local/share/everythux/index.db-wal ~/.local/share/everythux/index.db-shm
   everythux-cli reindex --full
   ```

   If you moved the database with `EVERYTHUX_DB`, delete that file instead.

Your settings and exclusions are in `config.json` and are not touched by this.

## The configuration file is rejected

The error names the line and column of the JSON problem. Fix the file, or move it away and run
`everythux-cli config reset --yes` to start from the defaults. `everythux-cli config show` prints what is
in effect.

## Wayland

- The app runs natively on Wayland and falls back to X11 (XWayland) if there is no Wayland display. Force
  X11 with `WAYLAND_DISPLAY= everythux`.
- Wayland doesn't let an application place or raise its own windows, so the window position is not
  restored on some compositors, and a file manager window opened by Everythux may not come to the front.
  Everythux passes an activation token to the file manager where the compositor supports it.
- Global shortcuts are not available on Wayland without a portal. `Ctrl+Shift+R` and the other shortcuts
  work while the Everythux window is focused.
- If the window doesn't open, run `everythux` from a terminal with `RUST_LOG=debug` and check for graphics errors
  (missing OpenGL/EGL drivers). On X11-only setups, make sure `DISPLAY` is set.

## Logs

The desktop app writes to `$XDG_STATE_HOME/everythux/` (`~/.local/state/everythux/`). Set `RUST_LOG=debug`
for more detail. `everythux-cli` logs to stderr: `-v` for info, `-vv` for debug.
