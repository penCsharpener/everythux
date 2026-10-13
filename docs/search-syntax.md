# Search syntax

Type in the search box and results update as you type. The same syntax works in
`everythux-cli search`.

An empty query matches everything.

## Terms

| Input | Meaning |
|-------|---------|
| `report` | Name contains `report` (substring, case-insensitive) |
| `report 2026` | Space means AND: the name contains both |
| `jpg\|png` | `\|` means OR: contains `jpg` or `png` |
| `!tmp` | A leading `!` means NOT: the name does not contain `tmp` |
| `"annual report"` | Quotes keep spaces inside one term |
| `*.pdf` | `*` matches any run of characters |
| `img_??.png` | `?` matches exactly one character |

Details:

- `\|` binds tighter than AND. `a b\|c` means `a AND (b OR c)`.
- A term with `*` or `?` must match the **whole name**, not just part of it. `*.pdf` finds names ending in `.pdf`;
  `pdf*` finds names starting with `pdf`; `*pdf*` finds names containing `pdf`.
- A term without wildcards matches anywhere in the name.
- Matching ignores case by default. Non-ASCII letters are case-folded too (`ä` matches `Ä`).
- Names are matched as text. A file name that is not valid UTF-8 is matched with the invalid bytes shown
  as U+FFFD, but the real bytes are used when you open or copy it.

## Filters

Filters have the form `name:value`. Combine them with terms and with each other; they are ANDed like
everything else.

| Filter | Meaning | Examples |
|--------|---------|----------|
| `ext:` | Extension. Separate several with `;`. No dot, case-insensitive | `ext:pdf`, `ext:jpg;png;gif` |
| `size:` | File size. Units are `b`, `kb`, `mb`, `gb` | `size:>100mb`, `size:1mb..10mb` |
| `dm:` | Date modified (see below) | `dm:today`, `dm:2026-01-01..2026-03-01` |
| `path:` | The full path contains this text | `path:src/main` |
| `file:` | Only files (not folders) | `file: report` |
| `folder:` | Only folders | `folder: backup` |
| `regex:` | The name matches this regular expression | `regex:^img_\d+\.jpe?g$` |
| `case:` | This one term is case-sensitive | `case:README` |
| `vol:` | Only volumes whose mount point contains this text | `vol:home` |

### Size

- `size:>100mb` is larger than 100 MB; `size:1mb..10mb` is a range, ends included.
- Sizes are binary (1 kb = 1024 bytes).
- Folders have the total size of everything below them. Use `file:` to leave them out.

### Dates

`dm:` accepts:

- `today`, `yesterday`
- `thisweek`, `lastweek`
- `thismonth`, `thisyear`
- a day: `dm:2026-01-01`
- a range of days: `dm:2026-01-01..2026-03-01`

Days are in your local time zone.

## Switches

These are buttons or settings in the UI and flags in the CLI. They apply to the whole query.

| Switch | UI | CLI | Config key |
|--------|----|-----|------------|
| Case-sensitive | toggle in the search bar | `--case` | `search.case_sensitive` |
| Match full path | `Ctrl+U` | `--match-path` | `search.match_path` |
| Regular expression | toggle in the search bar | `--regex` | `search.regex` |

- **Match full path** matches every term against the whole path (`/home/me/src/app/main.rs`) instead of
  only the name. Use it for queries like `src app main` or `/home/me/*.rs`.
- **Regular expression** treats the whole input as one regular expression (Rust `regex` syntax) and
  disables the operators above. An invalid expression is reported and the previous results stay.

## Errors

If a query can't be parsed (an unknown filter, an unclosed quote, a bad regular expression), the search
box shows the reason and keeps the previous results. `everythux-cli search` prints the error and exits
with status 1.

## Examples

```text
ext:pdf invoice dm:thisyear           PDFs with "invoice" in the name, modified this year
size:>1gb file:                       files larger than 1 GB
folder: node_modules                  folders named like node_modules (if not excluded)
"my photo" jpg|png !thumb             "my photo" in a .jpg/.png name, not containing "thumb"
path:projects ext:rs !test            Rust files below a "projects" folder, excluding "test" names
case:Makefile                         exactly "Makefile", not "makefile"
vol:backup dm:lastweek                changed last week on a volume mounted below "backup"
```

## From the command line

```sh
everythux-cli search ext:pdf invoice --sort modified --desc --limit 20
everythux-cli search 'size:>1gb file:' --json
everythux-cli search --null '*.tmp' | xargs -0 ls -l
```

Quote queries that contain `|`, `!`, `*`, `>` or spaces so your shell passes them through unchanged.
Several words after `search` are joined with spaces, so `search foo bar` is the query `foo bar`.
