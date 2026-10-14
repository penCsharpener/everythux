//! `Exec` lines: splitting and field-code expansion for desktop entries, Nemo and Thunar actions.
//!
//! Commands are never run through a shell: the line is split into an argument vector and paths are
//! inserted as separate arguments, so file names cannot inject anything.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use crate::uri::file_uri;

/// Which field-code vocabulary an `Exec` line uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// freedesktop Desktop Entry spec (`%f %F %u %U %i %c %k`), also used by KDE service menus.
    DesktopEntry,
    /// Nemo actions (`%F` paths, `%U` URIs, `%P` parent folder, `%f` first file name).
    Nemo,
    /// Thunar custom actions (`%f %F %d %D %n %N %u %U`).
    Thunar,
}

/// Values the non-file codes expand to.
#[derive(Debug, Clone, Default)]
pub struct ExecContext<'a> {
    /// Translated application or action name (`%c`).
    pub name: &'a str,
    /// Icon name (`%i` becomes `--icon NAME`).
    pub icon: Option<&'a str>,
    /// Location of the `.desktop` file (`%k`).
    pub desktop_file: Option<&'a Path>,
}

/// Splits an `Exec` string into arguments, honouring double quotes and backslash escapes inside them.
pub(crate) fn split(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quoted = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            '\\' if quoted => match chars.next() {
                Some(n @ ('"' | '`' | '$' | '\\')) => current.push(n),
                Some(n) => {
                    current.push('\\');
                    current.push(n);
                }
                None => current.push('\\'),
            },
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    args.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            c => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        args.push(current);
    }
    args
}

/// Command lines to run for `exec` with `paths`: one argument vector per process to start.
///
/// A line with a "single file" code (`%f`, `%u`, Thunar `%d %n`) runs once per path; a line with
/// only list codes (`%F`, `%U`) or none runs once. The first vector's first element is the program.
#[must_use]
pub fn expand(dialect: Dialect, exec: &str, paths: &[PathBuf], ctx: &ExecContext<'_>) -> Vec<Vec<OsString>> {
    let tokens = split(exec);
    let per_file = tokens.iter().any(|t| has_single_code(dialect, t));
    if per_file && !paths.is_empty() {
        paths
            .iter()
            .map(|p| build(dialect, &tokens, std::slice::from_ref(p), paths, ctx))
            .collect()
    } else {
        vec![build(dialect, &tokens, paths, paths, ctx)]
    }
}

fn is_single(dialect: Dialect, code: char) -> bool {
    match dialect {
        Dialect::DesktopEntry => matches!(code, 'f' | 'u'),
        Dialect::Nemo => matches!(code, 'f'),
        Dialect::Thunar => matches!(code, 'f' | 'u' | 'd' | 'n'),
    }
}

fn has_single_code(dialect: Dialect, token: &str) -> bool {
    codes(token).any(|c| is_single(dialect, c))
}

/// The field-code letters in a token (`%%` is not a code).
fn codes(token: &str) -> impl Iterator<Item = char> + '_ {
    let mut chars = token.chars();
    std::iter::from_fn(move || {
        while let Some(c) = chars.next() {
            if c == '%' {
                match chars.next() {
                    Some('%') | None => {}
                    Some(code) => return Some(code),
                }
            }
        }
        None
    })
}

/// One argument vector. `files` are the paths the single codes see, `all` the full selection.
fn build(
    dialect: Dialect,
    tokens: &[String],
    files: &[PathBuf],
    all: &[PathBuf],
    ctx: &ExecContext<'_>,
) -> Vec<OsString> {
    let mut argv: Vec<OsString> = Vec::new();
    for token in tokens {
        // A token that is exactly a list code expands to several arguments (or none).
        if let Some(list) = whole_list_code(dialect, token, all) {
            argv.extend(list);
            continue;
        }
        if token == "%i" && dialect == Dialect::DesktopEntry {
            if let Some(icon) = ctx.icon {
                argv.push("--icon".into());
                argv.push(icon.into());
            }
            continue;
        }
        let value = substitute(dialect, token, files, all, ctx);
        // An argument made only of codes that expanded to nothing disappears.
        if !(value.is_empty() && token.contains('%')) {
            argv.push(value);
        }
    }
    argv
}

fn whole_list_code(dialect: Dialect, token: &str, all: &[PathBuf]) -> Option<Vec<OsString>> {
    let list_code = match (dialect, token) {
        (Dialect::DesktopEntry | Dialect::Thunar | Dialect::Nemo, "%F") => true,
        (_, "%U") => false,
        (Dialect::Thunar, "%D" | "%N") => {
            return Some(
                all.iter()
                    .map(|p| {
                        if token == "%D" {
                            p.parent().unwrap_or(Path::new("/")).as_os_str().to_owned()
                        } else {
                            p.file_name().unwrap_or_default().to_owned()
                        }
                    })
                    .collect(),
            );
        }
        _ => return None,
    };
    Some(
        all.iter()
            .map(|p| {
                if list_code {
                    p.as_os_str().to_owned()
                } else {
                    OsString::from(file_uri(p))
                }
            })
            .collect(),
    )
}

fn substitute(
    dialect: Dialect,
    token: &str,
    files: &[PathBuf],
    all: &[PathBuf],
    ctx: &ExecContext<'_>,
) -> OsString {
    let first = files.first();
    let mut out: Vec<u8> = Vec::new();
    let mut chars = token.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        let Some(code) = chars.next() else {
            out.push(b'%');
            break;
        };
        let piece: Option<Vec<u8>> = match (dialect, code) {
            (_, '%') => Some(b"%".to_vec()),
            (Dialect::DesktopEntry, 'c') => Some(ctx.name.as_bytes().to_vec()),
            (Dialect::DesktopEntry, 'k') => ctx.desktop_file.map(|p| p.as_os_str().as_bytes().to_vec()),
            (_, 'f') if dialect != Dialect::Nemo => first.map(|p| p.as_os_str().as_bytes().to_vec()),
            (_, 'u') => first.map(|p| file_uri(p).into_bytes()),
            (_, 'F') => Some(join_paths(all)),
            (_, 'U') => Some(
                all.iter()
                    .map(|p| file_uri(p))
                    .collect::<Vec<_>>()
                    .join(" ")
                    .into_bytes(),
            ),
            (Dialect::Nemo, 'f') | (Dialect::Thunar, 'n') => {
                first.and_then(|p| p.file_name()).map(|n| n.as_bytes().to_vec())
            }
            (Dialect::Nemo, 'P') | (Dialect::Thunar, 'd') => first
                .and_then(|p| p.parent())
                .map(|p| p.as_os_str().as_bytes().to_vec()),
            (Dialect::Thunar, 'D') => Some(join_paths(
                &all.iter()
                    .filter_map(|p| p.parent().map(Path::to_path_buf))
                    .collect::<Vec<_>>(),
            )),
            (Dialect::Thunar, 'N') => Some(
                all.iter()
                    .filter_map(|p| p.file_name())
                    .map(|n| n.as_bytes().to_vec())
                    .collect::<Vec<_>>()
                    .join(&b' '),
            ),
            // Deprecated or unknown codes expand to nothing.
            _ => None,
        };
        if let Some(bytes) = piece {
            out.extend_from_slice(&bytes);
        }
    }
    OsString::from_vec(out)
}

fn join_paths(paths: &[PathBuf]) -> Vec<u8> {
    paths
        .iter()
        .map(|p| p.as_os_str().as_bytes().to_vec())
        .collect::<Vec<_>>()
        .join(&b' ')
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStrExt;

    use super::*;

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    fn strings(argvs: &[Vec<OsString>]) -> Vec<Vec<String>> {
        argvs
            .iter()
            .map(|a| a.iter().map(|s| s.to_string_lossy().into_owned()).collect())
            .collect()
    }

    #[test]
    fn splits_quotes_and_escapes() {
        assert_eq!(
            split(r#"sh -c "echo \"hi\" $HOME" x"#),
            ["sh", "-c", r#"echo "hi" $HOME"#, "x"]
        );
        assert_eq!(split("  a   b "), ["a", "b"]);
        assert_eq!(split(r#"cmd "" end"#), ["cmd", "", "end"]);
    }

    #[test]
    fn list_codes_run_once_with_every_path() {
        let ctx = ExecContext::default();
        let out = expand(Dialect::DesktopEntry, "kate %F", &paths(&["/a b/x", "/c"]), &ctx);
        assert_eq!(strings(&out), [["kate", "/a b/x", "/c"]]);
        let out = expand(Dialect::DesktopEntry, "app %U", &paths(&["/a b/x"]), &ctx);
        assert_eq!(strings(&out), [["app", "file:///a%20b/x"]]);
    }

    #[test]
    fn single_codes_run_once_per_path() {
        let ctx = ExecContext::default();
        let out = expand(
            Dialect::DesktopEntry,
            "viewer --file=%f",
            &paths(&["/a", "/b"]),
            &ctx,
        );
        assert_eq!(strings(&out), [["viewer", "--file=/a"], ["viewer", "--file=/b"]]);
    }

    #[test]
    fn no_code_runs_once_without_files() {
        let ctx = ExecContext::default();
        let out = expand(Dialect::DesktopEntry, "tool --flag", &paths(&["/a", "/b"]), &ctx);
        assert_eq!(strings(&out), [["tool", "--flag"]]);
    }

    #[test]
    fn icon_name_and_percent_codes() {
        let ctx = ExecContext {
            name: "My App",
            icon: Some("kate"),
            desktop_file: Some(Path::new("/x/app.desktop")),
        };
        let out = expand(
            Dialect::DesktopEntry,
            "app %i %c %k 100%% %d %f",
            &paths(&["/a"]),
            &ctx,
        );
        assert_eq!(
            strings(&out),
            [["app", "--icon", "kate", "My App", "/x/app.desktop", "100%", "/a"]]
        );
    }

    #[test]
    fn invalid_utf8_paths_are_passed_through_unchanged() {
        let ctx = ExecContext::default();
        let bad = PathBuf::from(OsString::from_vec(b"/tmp/bad\xffname".to_vec()));
        let out = expand(Dialect::DesktopEntry, "cat %f", std::slice::from_ref(&bad), &ctx);
        assert_eq!(out[0][1].as_bytes(), b"/tmp/bad\xffname");
    }

    #[test]
    fn nemo_and_thunar_codes() {
        let ctx = ExecContext::default();
        let files = paths(&["/home/me/a.txt", "/home/me/b.txt"]);
        let out = expand(Dialect::Nemo, "run %P %F", &files, &ctx);
        assert_eq!(
            strings(&out),
            [["run", "/home/me", "/home/me/a.txt", "/home/me/b.txt"]]
        );
        let out = expand(Dialect::Thunar, "tool %n in %d", &files, &ctx);
        assert_eq!(
            strings(&out),
            [
                ["tool", "a.txt", "in", "/home/me"],
                ["tool", "b.txt", "in", "/home/me"]
            ]
        );
    }

    #[test]
    fn deprecated_codes_vanish() {
        let ctx = ExecContext::default();
        let out = expand(Dialect::DesktopEntry, "app %m %n %F", &paths(&["/a"]), &ctx);
        assert_eq!(strings(&out), [["app", "/a"]]);
    }
}
