//! Context-menu actions of the desktop: "Open with" applications and the file-type specific
//! actions that other programs install (KDE service menus, Nemo actions, Thunar custom actions).
//!
//! Dolphin's own menu cannot be embedded (its entries are in-process Qt plugins), but the data
//! those menus are built from is plain files, which [`ActionCatalog`] reads. Loading touches the
//! disk (hundreds of small files): do it on a worker thread. Filtering a loaded catalog for a
//! selection is in-memory and cheap.

mod load;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::PlatformError;
use crate::exec::{self, Dialect, ExecContext};
use crate::mimetype::{self, MimeTree};

pub use load::XdgDirs;

/// Where a menu entry comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Source {
    /// An installed application that handles the file type.
    OpenWith,
    /// A KDE service menu (`kio/servicemenus/*.desktop`).
    ServiceMenu,
    /// A Nemo action (`nemo/actions/*.nemo_action`).
    Nemo,
    /// A Thunar custom action (`Thunar/uca.xml`).
    Thunar,
}

/// What to run for a menu entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    /// Which field codes `exec` uses.
    pub dialect: Dialect,
    /// The command line, with field codes.
    pub exec: String,
    /// Display name (`%c`).
    pub name: String,
    /// Icon name from the theme, if any.
    pub icon: Option<String>,
    /// The file the entry was read from (`%k`).
    pub desktop_file: Option<PathBuf>,
}

/// One entry for the right-click menu of a selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    /// Stable identifier, used to hide or reorder entries in the config.
    pub id: String,
    /// Text shown in the menu.
    pub label: String,
    /// Submenu the entry belongs to, if any.
    pub submenu: Option<String>,
    /// Where it came from.
    pub source: Source,
    /// What running it does.
    pub launcher: Launcher,
}

/// A selected file or folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Absolute path.
    pub path: PathBuf,
    /// Whether it is a folder.
    pub is_dir: bool,
}

/// How many selected items an action accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Count {
    pub(crate) min: usize,
    pub(crate) max: usize,
}

impl Count {
    pub(crate) const ANY: Self = Self {
        min: 1,
        max: usize::MAX,
    };
}

/// Which items an action applies to.
#[derive(Debug, Clone)]
pub(crate) struct Matcher {
    /// MIME patterns (`image/*`, `inode/directory`, `all/allfiles`, …).
    pub(crate) mimes: Vec<String>,
    /// Lowercase extensions without the dot; `any`, `dir` and `none` are Nemo keywords.
    pub(crate) extensions: Vec<String>,
    /// Case-insensitive name globs (`*.txt`).
    pub(crate) globs: Vec<String>,
    /// Thunar kind switches; all false means "everything".
    pub(crate) kinds: Kinds,
    pub(crate) count: Count,
}

/// Thunar's `<directories/>`, `<text-files/>`, … switches as a bit set.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Kinds(u8);

impl Kinds {
    pub(crate) const DIRECTORIES: u8 = 1;
    pub(crate) const TEXT: u8 = 2;
    pub(crate) const IMAGE: u8 = 4;
    pub(crate) const AUDIO: u8 = 8;
    pub(crate) const VIDEO: u8 = 16;
    pub(crate) const OTHER: u8 = 32;

    pub(crate) fn with(self, flag: u8, on: bool) -> Self {
        Self(if on { self.0 | flag } else { self.0 })
    }

    fn has(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    fn any(self) -> bool {
        self.0 != 0
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ActionDef {
    pub(crate) entry: MenuEntry,
    pub(crate) matcher: Matcher,
}

#[derive(Debug, Clone)]
pub(crate) struct AppDef {
    /// Desktop file ID, e.g. `org.kde.kate.desktop`.
    pub(crate) id: String,
    pub(crate) launcher: Launcher,
    pub(crate) mimes: Vec<String>,
}

/// Everything installed that can appear in a context menu.
#[derive(Debug, Clone, Default)]
pub struct ActionCatalog {
    pub(crate) apps: Vec<AppDef>,
    pub(crate) actions: Vec<ActionDef>,
    /// `(mime, desktop id)` defaults in priority order.
    pub(crate) defaults: Vec<(String, String)>,
    pub(crate) added: Vec<(String, String)>,
    pub(crate) removed: Vec<(String, String)>,
    pub(crate) tree: MimeTree,
}

/// Most "Open with" entries offered at once.
const MAX_OPEN_WITH: usize = 12;

impl ActionCatalog {
    /// Reads the catalog from `dirs`. Blocking; unreadable files are skipped.
    #[must_use]
    pub fn load(dirs: &XdgDirs) -> Self {
        load::load(dirs)
    }

    /// Reads the catalog from the environment's XDG directories. Blocking.
    #[must_use]
    pub fn load_default() -> Self {
        Self::load(&XdgDirs::from_env())
    }

    /// Number of applications and actions known (for logging).
    #[must_use]
    pub fn len(&self) -> usize {
        self.apps.len() + self.actions.len()
    }

    /// Whether nothing was found.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Applications that can open every item of `targets`, the default one first.
    #[must_use]
    pub fn open_with(&self, targets: &[Target]) -> Vec<MenuEntry> {
        if targets.is_empty() {
            return Vec::new();
        }
        let mimes: Vec<String> = targets
            .iter()
            .map(|t| mimetype::mime_of(&t.path, t.is_dir))
            .collect();
        let default = self.default_app(&mimes[0]);
        let mut entries: Vec<(bool, &AppDef)> = self
            .apps
            .iter()
            .filter(|app| mimes.iter().all(|m| self.app_handles(app, m)))
            .map(|app| (default == Some(app.id.as_str()), app))
            .collect();
        entries.sort_by(|a, b| {
            b.0.cmp(&a.0).then_with(|| {
                a.1.launcher
                    .name
                    .to_lowercase()
                    .cmp(&b.1.launcher.name.to_lowercase())
            })
        });
        entries
            .into_iter()
            .take(MAX_OPEN_WITH)
            .map(|(_, app)| MenuEntry {
                id: format!("open-with:{}", app.id),
                label: app.launcher.name.clone(),
                submenu: None,
                source: Source::OpenWith,
                launcher: app.launcher.clone(),
            })
            .collect()
    }

    /// File-type specific actions (service menus, Nemo and Thunar actions) for `targets`.
    #[must_use]
    pub fn actions_for(&self, targets: &[Target]) -> Vec<MenuEntry> {
        if targets.is_empty() {
            return Vec::new();
        }
        let mimes: Vec<String> = targets
            .iter()
            .map(|t| mimetype::mime_of(&t.path, t.is_dir))
            .collect();
        self.actions
            .iter()
            .filter(|a| a.matcher.accepts(&self.tree, targets, &mimes))
            .map(|a| a.entry.clone())
            .collect()
    }

    fn default_app(&self, mime: &str) -> Option<&str> {
        let candidates = self.tree.ancestors_of(mime);
        candidates.iter().find_map(|m| {
            self.defaults
                .iter()
                .find(|(dm, id)| dm == m && self.apps.iter().any(|a| &a.id == id))
                .map(|(_, id)| id.as_str())
        })
    }

    fn app_handles(&self, app: &AppDef, mime: &str) -> bool {
        let removed = self
            .removed
            .iter()
            .any(|(m, id)| id == &app.id && self.tree.matches(mime, m));
        if removed {
            return false;
        }
        app.mimes
            .iter()
            .any(|m| self.tree.matches_exact_or_parent(mime, m))
            || self
                .added
                .iter()
                .any(|(m, id)| id == &app.id && self.tree.matches_exact_or_parent(mime, m))
    }
}

impl Matcher {
    fn accepts(&self, tree: &MimeTree, targets: &[Target], mimes: &[String]) -> bool {
        let n = targets.len();
        if n < self.count.min || n > self.count.max {
            return false;
        }
        targets
            .iter()
            .zip(mimes)
            .all(|(target, mime)| self.accepts_one(tree, target, mime))
    }

    fn accepts_one(&self, tree: &MimeTree, target: &Target, mime: &str) -> bool {
        let name = target
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !self.mimes.is_empty() && !self.mimes.iter().any(|p| tree.matches(mime, p)) {
            return false;
        }
        if !self.extensions.is_empty() {
            let ext = Path::new(&name)
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_default();
            let ok = self.extensions.iter().any(|e| match e.as_str() {
                "any" => !target.is_dir,
                "dir" => target.is_dir,
                "none" => !target.is_dir && ext.is_empty(),
                other => !target.is_dir && other == ext,
            });
            if !ok {
                return false;
            }
        }
        if !self.globs.is_empty() && !self.globs.iter().any(|g| glob_match(g, &name)) {
            return false;
        }
        if self.kinds.any() {
            let k = self.kinds;
            let ok = if target.is_dir {
                k.has(Kinds::DIRECTORIES)
            } else {
                (k.has(Kinds::TEXT) && tree.matches(mime, "text/*"))
                    || (k.has(Kinds::IMAGE) && tree.matches(mime, "image/*"))
                    || (k.has(Kinds::AUDIO) && tree.matches(mime, "audio/*"))
                    || (k.has(Kinds::VIDEO) && tree.matches(mime, "video/*"))
                    || (k.has(Kinds::OTHER)
                        && !["text/*", "image/*", "audio/*", "video/*"]
                            .iter()
                            .any(|p| tree.matches(mime, p)))
            };
            if !ok {
                return false;
            }
        }
        true
    }
}

/// `*` and `?` wildcards; both sides are already lowercase.
fn glob_match(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('*', rest)) => (0..=t.len()).any(|i| go(rest, &t[i..])),
            Some(('?', rest)) => t.split_first().is_some_and(|(_, tr)| go(rest, tr)),
            Some((c, rest)) => t.split_first().is_some_and(|(tc, tr)| tc == c && go(rest, tr)),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    go(&p, &t)
}

/// Starts `launcher` for `paths` without waiting for it. Returns how many processes were started.
///
/// Arguments are passed as an argument vector (no shell). The working directory is the folder of
/// the first path, as file managers do. Blocking only for the `fork`/`exec`; call from a worker.
///
/// # Errors
///
/// [`PlatformError::Spawn`] if the program cannot be started, [`PlatformError::Unsupported`] if the
/// command line is empty.
pub fn launch(launcher: &Launcher, paths: &[PathBuf]) -> Result<usize, PlatformError> {
    let ctx = ExecContext {
        name: &launcher.name,
        icon: launcher.icon.as_deref(),
        desktop_file: launcher.desktop_file.as_deref(),
    };
    let argvs = exec::expand(launcher.dialect, &launcher.exec, paths, &ctx);
    let cwd = paths
        .first()
        .and_then(|p| p.parent())
        .filter(|p| p.is_dir())
        .map(Path::to_path_buf);
    let mut started = 0;
    for argv in argvs {
        let Some((program, rest)) = argv.split_first() else {
            return Err(PlatformError::Unsupported("the command line is empty"));
        };
        let mut command = Command::new(program);
        command
            .args(rest)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(dir) = &cwd {
            command.current_dir(dir);
        }
        let child = command.spawn().map_err(|source| PlatformError::Spawn {
            program: program.to_string_lossy().into_owned(),
            source,
        })?;
        started += 1;
        // Reap the child so it does not stay a zombie; the program itself keeps running detached.
        std::thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
        });
    }
    Ok(started)
}

/// Programs named by `TryExec`/`Dependencies` must exist on `PATH` (or be an executable path).
pub(crate) fn program_exists(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    let is_exec = |p: &Path| {
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if program.contains('/') {
        return is_exec(Path::new(program));
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| is_exec(&dir.join(program))))
}

pub(crate) fn unique_ids(seen: &mut HashSet<String>, id: &str) -> String {
    let mut candidate = id.to_owned();
    let mut n = 1u32;
    while !seen.insert(candidate.clone()) {
        n += 1;
        candidate = format!("{id}#{n}");
    }
    candidate
}

#[cfg(test)]
mod tests;
