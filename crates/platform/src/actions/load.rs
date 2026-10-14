//! Reading the catalog from the XDG directories.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::{
    ActionCatalog, ActionDef, AppDef, Count, Kinds, Launcher, Matcher, MenuEntry, Source, program_exists,
    unique_ids,
};
use crate::desktop_entry::{DesktopFile, Group, locale_candidates};
use crate::exec::Dialect;
use crate::mimetype::MimeTree;

/// The directories of the XDG base-directory spec plus the desktop and locale settings.
#[derive(Debug, Clone, Default)]
pub struct XdgDirs {
    /// `$XDG_CONFIG_HOME`.
    pub config_home: PathBuf,
    /// `$XDG_CONFIG_DIRS`.
    pub config_dirs: Vec<PathBuf>,
    /// `$XDG_DATA_HOME`.
    pub data_home: PathBuf,
    /// `$XDG_DATA_DIRS`.
    pub data_dirs: Vec<PathBuf>,
    /// `$XDG_CURRENT_DESKTOP`, split at `:`, e.g. `["KDE"]`.
    pub desktops: Vec<String>,
    /// Locale candidates for `Name[xx]` keys, most specific first.
    pub locales: Vec<String>,
}

impl XdgDirs {
    /// Reads the process environment, with the spec's defaults.
    #[must_use]
    pub fn from_env() -> Self {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let home = var("HOME").unwrap_or_else(|| PathBuf::from("/"));
        let list = |name: &str, default: &str| -> Vec<PathBuf> {
            std::env::var_os(name).filter(|v| !v.is_empty()).map_or_else(
                || default.split(':').map(PathBuf::from).collect(),
                |v| std::env::split_paths(&v).collect(),
            )
        };
        let lang = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|n| std::env::var(n).ok().filter(|v| !v.is_empty()))
            .unwrap_or_default();
        Self {
            config_home: var("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")),
            config_dirs: list("XDG_CONFIG_DIRS", "/etc/xdg"),
            data_home: var("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")),
            data_dirs: list("XDG_DATA_DIRS", "/usr/local/share:/usr/share"),
            desktops: std::env::var("XDG_CURRENT_DESKTOP")
                .unwrap_or_default()
                .split(':')
                .filter(|d| !d.is_empty())
                .map(str::to_owned)
                .collect(),
            locales: locale_candidates(&lang),
        }
    }

    /// Data directories in priority order (user first).
    fn data_search(&self) -> Vec<PathBuf> {
        std::iter::once(self.data_home.clone())
            .chain(self.data_dirs.iter().cloned())
            .collect()
    }

    fn shown_here(&self, group: &Group) -> bool {
        let only = group.list("OnlyShowIn");
        let not = group.list("NotShowIn");
        if !only.is_empty() && !only.iter().any(|d| self.desktops.contains(d)) {
            return false;
        }
        !not.iter().any(|d| self.desktops.contains(d))
    }
}

pub(super) fn load(dirs: &XdgDirs) -> ActionCatalog {
    let search = dirs.data_search();
    let mut catalog = ActionCatalog {
        tree: MimeTree::load(&search),
        ..ActionCatalog::default()
    };
    load_apps(dirs, &search, &mut catalog);
    load_mimeapps(dirs, &search, &mut catalog);
    load_service_menus(dirs, &search, &mut catalog);
    load_nemo(dirs, &search, &mut catalog);
    load_thunar(dirs, &mut catalog);
    let mut seen = HashSet::new();
    for action in &mut catalog.actions {
        action.entry.id = unique_ids(&mut seen, &action.entry.id);
    }
    catalog
}

/// Files with `extension` under `dir`, at most three levels deep, sorted for stable results.
fn files_with(dir: &Path, extension: &str, depth: u8) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                out.extend(files_with(&path, extension, depth - 1));
            }
        } else if path.extension() == Some(OsStr::new(extension)) {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// Desktop file ID: the path below `applications/` with `/` turned into `-`.
fn desktop_id(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('/', "-")
}

fn load_apps(dirs: &XdgDirs, search: &[PathBuf], catalog: &mut ActionCatalog) {
    let mut seen: HashSet<String> = HashSet::new();
    for data in search {
        let root = data.join("applications");
        for file in files_with(&root, "desktop", 2) {
            let id = desktop_id(&root, &file);
            // An earlier (user) directory shadows later ones, even if it hides the entry.
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(parsed) = DesktopFile::read(&file) else {
                continue;
            };
            let Some(entry) = parsed.group("Desktop Entry") else {
                continue;
            };
            let shown = entry.get("Type") == Some("Application")
                && !entry.flag("NoDisplay")
                && !entry.flag("Hidden")
                && !entry.flag("Terminal")
                && dirs.shown_here(entry)
                && entry.get("TryExec").is_none_or(program_exists);
            let (Some(exec), Some(name)) = (entry.get("Exec"), entry.localized("Name", &dirs.locales)) else {
                continue;
            };
            if !shown {
                continue;
            }
            catalog.apps.push(AppDef {
                id,
                launcher: Launcher {
                    dialect: Dialect::DesktopEntry,
                    exec: exec.to_owned(),
                    name: name.to_owned(),
                    icon: entry.get("Icon").map(str::to_owned),
                    desktop_file: Some(file),
                },
                mimes: entry.list("MimeType"),
            });
        }
    }
}

fn load_mimeapps(dirs: &XdgDirs, search: &[PathBuf], catalog: &mut ActionCatalog) {
    let mut files: Vec<PathBuf> = Vec::new();
    let config: Vec<&PathBuf> = std::iter::once(&dirs.config_home)
        .chain(&dirs.config_dirs)
        .collect();
    for base in config {
        for desktop in &dirs.desktops {
            files.push(base.join(format!("{}-mimeapps.list", desktop.to_lowercase())));
        }
        files.push(base.join("mimeapps.list"));
    }
    for data in search {
        files.push(data.join("applications/mimeapps.list"));
    }
    for file in files {
        let Some(parsed) = DesktopFile::read(&file) else {
            continue;
        };
        for (group, target) in [
            ("Default Applications", 0),
            ("Added Associations", 1),
            ("Removed Associations", 2),
        ] {
            let Some(group) = parsed.group(group) else {
                continue;
            };
            for (mime, ids) in group.pairs() {
                for id in ids.split(';').filter(|i| !i.is_empty()) {
                    let pair = (mime.to_owned(), id.to_owned());
                    match target {
                        0 => catalog.defaults.push(pair),
                        1 => catalog.added.push(pair),
                        _ => catalog.removed.push(pair),
                    }
                }
            }
        }
    }
}

fn load_service_menus(dirs: &XdgDirs, search: &[PathBuf], catalog: &mut ActionCatalog) {
    for data in search {
        for sub in [
            "kio/servicemenus",
            "kservices5/ServiceMenus",
            "kservices6/ServiceMenus",
        ] {
            for file in files_with(&data.join(sub), "desktop", 1) {
                if let Some(parsed) = DesktopFile::read(&file) {
                    service_menu(dirs, &file, &parsed, catalog);
                }
            }
        }
    }
}

fn service_menu(dirs: &XdgDirs, file: &Path, parsed: &DesktopFile, catalog: &mut ActionCatalog) {
    let Some(entry) = parsed.group("Desktop Entry") else {
        return;
    };
    if entry.flag("Hidden") || entry.flag("NoDisplay") || !dirs.shown_here(entry) {
        return;
    }
    // Only local files are offered; menus for other protocols (`X-KDE-Protocols=smb,…`) are not.
    let protocols = entry.list("X-KDE-Protocols");
    if !protocols.is_empty() && !protocols.iter().any(|p| p == "file") {
        return;
    }
    let mut mimes = entry.list("MimeType");
    mimes.extend(
        entry
            .list("X-KDE-ServiceTypes")
            .into_iter()
            .chain(entry.list("ServiceTypes"))
            .filter(|t| !t.starts_with("KonqPopupMenu")),
    );
    if mimes.is_empty() {
        return;
    }
    let count = match entry
        .get("X-KDE-RequiredNumberOfUrls")
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(n) if n > 0 => Count { min: n, max: n },
        _ => Count::ANY,
    };
    let submenu = entry
        .localized("X-KDE-Submenu", &dirs.locales)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let file_name = file
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    for action_id in entry.list("Actions") {
        let Some(action) = parsed.group(&format!("Desktop Action {action_id}")) else {
            continue;
        };
        let (Some(exec), Some(name)) = (action.get("Exec"), action.localized("Name", &dirs.locales)) else {
            continue;
        };
        if action.get("TryExec").is_some_and(|t| !program_exists(t)) {
            continue;
        }
        let icon = action
            .get("Icon")
            .or_else(|| entry.get("Icon"))
            .map(str::to_owned);
        let launcher = Launcher {
            dialect: Dialect::DesktopEntry,
            exec: exec.to_owned(),
            name: name.to_owned(),
            icon,
            desktop_file: Some(file.to_path_buf()),
        };
        catalog.actions.push(ActionDef {
            entry: MenuEntry {
                id: format!("service:{file_name}:{action_id}"),
                label: name.to_owned(),
                submenu: submenu.clone(),
                source: Source::ServiceMenu,
                launcher,
            },
            matcher: Matcher {
                mimes: mimes.clone(),
                extensions: Vec::new(),
                globs: Vec::new(),
                kinds: Kinds::default(),
                count,
            },
        });
    }
}

fn load_nemo(dirs: &XdgDirs, search: &[PathBuf], catalog: &mut ActionCatalog) {
    for data in search {
        for file in files_with(&data.join("nemo/actions"), "nemo_action", 0) {
            let Some(parsed) = DesktopFile::read(&file) else {
                continue;
            };
            let Some(entry) = parsed.group("Nemo Action") else {
                continue;
            };
            // Conditions need Nemo's own evaluator (dbus, desktop, …): leave those actions out.
            if entry.get("Conditions").is_some() || entry.flag("Terminal") {
                continue;
            }
            if entry.list("Dependencies").iter().any(|d| !program_exists(d)) {
                continue;
            }
            let (Some(exec), Some(name)) = (entry.get("Exec"), entry.localized("Name", &dirs.locales)) else {
                continue;
            };
            let count = match entry.get("Selection").unwrap_or("s") {
                "s" => Count { min: 1, max: 1 },
                "m" => Count {
                    min: 2,
                    max: usize::MAX,
                },
                "any" | "notnone" => Count::ANY,
                other => other
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .map_or(Count { min: 1, max: 1 }, |n| Count { min: n, max: n }),
            };
            let file_name = file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            catalog.actions.push(ActionDef {
                entry: MenuEntry {
                    id: format!("nemo:{file_name}"),
                    label: name.replace('_', ""),
                    submenu: None,
                    source: Source::Nemo,
                    launcher: Launcher {
                        dialect: Dialect::Nemo,
                        exec: exec.to_owned(),
                        name: name.to_owned(),
                        icon: entry.get("Icon-Name").map(str::to_owned),
                        desktop_file: Some(file),
                    },
                },
                matcher: Matcher {
                    mimes: entry.list("Mimetypes"),
                    extensions: entry
                        .list("Extensions")
                        .iter()
                        .map(|e| e.to_lowercase())
                        .collect(),
                    globs: Vec::new(),
                    kinds: Kinds::default(),
                    count,
                },
            });
        }
    }
}

fn load_thunar(dirs: &XdgDirs, catalog: &mut ActionCatalog) {
    let Ok(text) = std::fs::read_to_string(dirs.config_home.join("Thunar/uca.xml")) else {
        return;
    };
    for block in blocks(&text, "action") {
        let (Some(name), Some(command)) = (tag_text(block, "name"), tag_text(block, "command")) else {
            continue;
        };
        let id = tag_text(block, "unique-id").unwrap_or_else(|| name.clone());
        let globs = tag_text(block, "patterns")
            .map(|p| {
                p.split(';')
                    .map(|g| g.trim().to_lowercase())
                    .filter(|g| !g.is_empty() && g != "*")
                    .collect()
            })
            .unwrap_or_default();
        let flag =
            |tag: &str| block.contains(&format!("<{tag}/>")) || block.contains(&format!("<{tag}></{tag}>"));
        let kinds = Kinds::default()
            .with(Kinds::DIRECTORIES, flag("directories"))
            .with(Kinds::TEXT, flag("text-files"))
            .with(Kinds::IMAGE, flag("image-files"))
            .with(Kinds::AUDIO, flag("audio-files"))
            .with(Kinds::VIDEO, flag("video-files"))
            .with(Kinds::OTHER, flag("other-files"));
        let label = name.replace('_', "");
        catalog.actions.push(ActionDef {
            entry: MenuEntry {
                id: format!("thunar:{id}"),
                label,
                submenu: tag_text(block, "submenu"),
                source: Source::Thunar,
                launcher: Launcher {
                    dialect: Dialect::Thunar,
                    exec: command,
                    name,
                    icon: tag_text(block, "icon"),
                    desktop_file: None,
                },
            },
            matcher: Matcher {
                mimes: Vec::new(),
                extensions: Vec::new(),
                globs,
                kinds,
                count: Count::ANY,
            },
        });
    }
}

/// The text between `<tag>` and `</tag>`, for every occurrence.
fn blocks<'a>(text: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        let body = &rest[start + open.len()..];
        let Some(end) = body.find(&close) else { break };
        out.push(&body[..end]);
        rest = &body[end + close.len()..];
    }
    out
}

fn tag_text(block: &str, tag: &str) -> Option<String> {
    blocks(block, tag)
        .first()
        .map(|t| xml_unescape(t.trim()))
        .filter(|t| !t.is_empty())
}

fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}
