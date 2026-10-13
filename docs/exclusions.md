# Exclusions

Exclusion rules keep files and folders out of the index. Typical uses: build output, package caches and
temporary files. Rules are stored in `config.json` (see [configuration.md](configuration.md)) and
are applied while scanning, so an excluded folder is never read.

## Rule fields

```json
{ "pattern": "bin", "kind": "dir", "scope": "/home/*", "enabled": true }
```

| Field | Meaning |
|-------|---------|
| `pattern` | A glob, or `re:<regex>` for a regular expression |
| `kind` | `dir`, `file`, `path` or `any` (default `any`) |
| `scope` | Optional glob over absolute paths: the rule applies only below matching paths |
| `enabled` | `false` keeps the rule in the list but ignores it |

## How a rule matches

- **Glob syntax:** `*` matches within one path component (not `/`), `**` matches across components,
  `?` matches one character, `[abc]` a set, `{a,b}` alternatives. Matching is case-sensitive. Use `\` to
  escape a special character.
- **Name or path:** a pattern **without** `/` is matched against the entry's **name** (its last component), wherever
  it is. A pattern **with** `/` is matched against the entry's **absolute path**.
- **kind:**
  - `dir`: only folders.
  - `file`: only things that are not folders (files, symlinks, ...).
  - `any`: both.
  - `path`: the pattern is an absolute path (or path glob), and the rule excludes that path **and everything
    below it**. A `path` pattern must start with `/`.
- **scope:** the rule applies only to entries whose absolute path matches `<scope>/**`. The scope
  `/home/*` covers everything below any direct child of `/home` (that is, below users' home
  directories), but not `/home/<user>` itself and not anything outside `/home`.
- **Regular expressions:** `re:` followed by a Rust regex, matched anywhere in the name (or, for `kind: path`, in the
  absolute path). Anchor with `^` and `$` to match the whole name: `re:^\.cache-[0-9]+$`. Regexes work on raw
  bytes, so they can match non-UTF-8 names.
- **First match wins:** when several rules match, the first one in the list is the one reported by `exclude test`.
  Order doesn't change what is excluded.
- **Subtrees:** because excluded folders are pruned before the scan descends into them, excluding a folder
  excludes everything below it.
- **Disabled rules** are ignored.

## Defaults

| Pattern | Kind | Scope | Why |
|---------|------|-------|-----|
| `node_modules` | dir | | JavaScript dependencies |
| `.var` | dir | `/home/*` | Flatpak app data |
| `.cache` | dir | `/home/*` | User caches |
| `__pycache__` | dir | | Python bytecode |
| `.venv` | dir | | Python virtual environments |
| `target` | dir | `/home/*` | Rust build output |
| `obj` | dir | `/home/*` | .NET and C build output |
| `bin` | dir | `/home/*` | .NET build output |
| `/proc`, `/sys`, `/dev`, `/run`, `/tmp` | path | | Virtual and temporary filesystems |
| `/var/lib/docker`, `/var/lib/containers` | path | | Container storage |
| `lost+found` | dir | | ext4 recovery folder |

All of them can be edited, disabled or deleted. `everythux-cli config reset --yes` brings the defaults back.

## Scoping pitfalls

- **`bin` without a scope removes `/usr/bin`, `/bin` and every other folder of that name.** That is why the
  default `bin`, `obj`, `target`, `.cache` and `.var` rules are scoped to `/home/*`. Keep the scope when you
  add similar rules for names that also exist in the system.
- `/home/*` does not match `/home` itself or other mount points. If your home is elsewhere (for example
  `/data/home`), scope to that: `/data/home/*`.
- A scope of `/home/*/src/**` limits a rule to projects below each user's `src` folder.
- A bare name rule such as `target` with kind `any` also hides **files** called `target`. Use `dir`.
- Rules match **names**, so `*.tmp` with kind `file` hides files only. A folder named `x.tmp` stays.
- Case matters: `Node_Modules` is not matched by `node_modules`.
- The default `/tmp` rule means a test tree created below `/tmp` is not indexed. Disable that rule if you
  want to index it.

## Adding a rule in the UI: Ctrl+Shift+Click

`Ctrl+Shift+Left-click` on a **Path** cell adds a `path` rule for that folder immediately and opens the
exclusion popup next to the cursor. In the popup you can:

- switch between **this exact folder**, **any folder with this name**, **any folder with this name under a scope**, and a **custom glob**;
- see a live preview such as "matches 12,345 indexed entries in 87 folders";
- choose **Remove matching entries from the index now** (default) or **Keep until next rescan**;
- **Undo** (removes the rule) or open the full **exclusion editor**.

Before you confirm a broad rule, check the preview count. A rule that matches a large share of the index is
usually too broad.

The exclusion editor (Settings, Exclusions) lists all rules with enable checkboxes, and has a test box: type a
path to see whether it is excluded and by which rule.

## From the command line

```sh
everythux-cli exclude list
everythux-cli exclude add node_modules --kind dir
everythux-cli exclude add bin --kind dir --scope '/home/*'
everythux-cli exclude add 're:^\.cache-[0-9]+$' --kind dir
everythux-cli exclude test /home/me/proj/bin        # which rule matches?
everythux-cli exclude disable 3
everythux-cli exclude remove 3
```

Rule numbers (starting at 0) are those shown by `exclude list`. Rules are validated before they are saved.
Changing rules from the CLI edits `config.json` only; run `everythux-cli reindex --quick` to update the
index. The desktop UI applies changes immediately and reloads `config.json` when it changes on disk.

## Removing a rule

Entries that were excluded are not in the index, so removing a rule does not bring them back by itself. Re-index
the affected folder (context menu, **Re-index this folder**, or `everythux-cli reindex --path <folder>`).
