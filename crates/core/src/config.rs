//! The JSON configuration (`config.json`, see ROADMAP §5).
//!
//! Every struct derives `Default` and uses `#[serde(default)]`, so files written by older or newer
//! versions still load: missing fields take defaults, unknown fields are ignored. Breaking changes
//! bump [`CURRENT_VERSION`] and add a step to [`migrate`].

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::exclusion::{ExclusionRule, default_rules};

/// Config schema version written by this build.
pub const CURRENT_VERSION: u32 = 2;

/// Errors loading or saving the configuration.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The file exists but couldn't be read or written.
    #[error("cannot access config file {path:?}")]
    Io {
        /// The config file.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The file isn't valid JSON for this schema.
    #[error("invalid config file {path:?} (line {line}, column {column}): {message}")]
    Parse {
        /// The config file.
        path: PathBuf,
        /// 1-based line of the error.
        line: usize,
        /// 1-based column of the error.
        column: usize,
        /// Human-readable description.
        message: String,
    },
}

/// Root of `config.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Schema version; see [`CURRENT_VERSION`].
    pub version: u32,
    /// Which volumes to index.
    pub volumes: VolumesConfig,
    /// Exclusion rules, in evaluation order.
    pub exclusions: Vec<ExclusionRule>,
    /// UI settings.
    pub ui: UiConfig,
    /// Key and mouse bindings.
    pub keybindings: Keybindings,
    /// Indexer behaviour.
    pub indexer: IndexerConfig,
    /// Search defaults.
    pub search: SearchConfig,
    /// Bookmarked searches, in menu order.
    pub bookmarks: Vec<Bookmark>,
}

/// A bookmarked search: a name, the text of each search bar and the switches it was saved with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Bookmark {
    /// Menu label.
    pub name: String,
    /// The text of each search bar, top to bottom.
    pub texts: Vec<String>,
    /// Match case.
    pub case_sensitive: bool,
    /// Match path.
    pub match_path: bool,
    /// Regular expression.
    pub regex: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            volumes: VolumesConfig::default(),
            exclusions: default_rules(),
            ui: UiConfig::default(),
            keybindings: Keybindings::default(),
            indexer: IndexerConfig::default(),
            search: SearchConfig::default(),
            bookmarks: Vec::new(),
        }
    }
}

/// Volume discovery settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VolumesConfig {
    /// Filesystem types discovered automatically from `/proc/self/mountinfo`.
    pub auto_discover_fs_types: Vec<String>,
    /// Extra roots to index even if their filesystem type isn't auto-discovered.
    pub include: Vec<PathBuf>,
    /// Filesystem UUIDs the user disabled.
    pub disabled_uuids: Vec<String>,
}

impl Default for VolumesConfig {
    fn default() -> Self {
        Self {
            auto_discover_fs_types: vec!["ext4".to_owned()],
            include: Vec::new(),
            disabled_uuids: Vec::new(),
        }
    }
}

/// A result-table column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnId {
    /// File name.
    Name,
    /// Containing folder.
    Path,
    /// File type (from the extension).
    Type,
    /// Date modified.
    Modified,
    /// Size.
    Size,
}

impl ColumnId {
    /// All columns in default order.
    pub const ALL: [Self; 5] = [Self::Name, Self::Path, Self::Type, Self::Modified, Self::Size];

    /// Header label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Path => "Path",
            Self::Type => "Type",
            Self::Modified => "Date Modified",
            Self::Size => "Size",
        }
    }
}

/// Width and visibility of one column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnConfig {
    /// Which column.
    pub id: ColumnId,
    /// Width in logical pixels.
    pub width: f32,
    /// Whether it's shown.
    pub visible: bool,
}

/// Sort order of the result table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SortConfig {
    /// Sort column.
    pub column: ColumnId,
    /// Ascending (`true`) or descending.
    pub ascending: bool,
}

impl Default for SortConfig {
    fn default() -> Self {
        Self {
            column: ColumnId::Name,
            ascending: true,
        }
    }
}

/// Preview pane settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent user toggles, stored flat in JSON"
)]
pub struct PreviewPaneConfig {
    /// Whether the pane is shown.
    pub enabled: bool,
    /// Pane width in logical pixels. The pane always has exactly this width (unless the window is
    /// too narrow for it); the previewed item never changes it.
    pub width: f32,
    /// Whether the pane edge can be dragged to change `width`. Off by default: the width is set
    /// in View → Preview pane.
    pub resizable: bool,
    /// Wrap long lines of text previews at the pane width (otherwise they scroll sideways).
    pub wrap_text: bool,
    /// Show line numbers next to text previews.
    pub line_numbers: bool,
    /// Files larger than this aren't previewed (bytes).
    pub max_file_size: u64,
}

impl Default for PreviewPaneConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            width: 360.0,
            resizable: false,
            wrap_text: true,
            line_numbers: false,
            max_file_size: 512 * 1024 * 1024,
        }
    }
}

/// Which folder a Size/Modified double-click shows properties for. (The context menu entry
/// "Properties…" always shows the item itself.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertiesTarget {
    /// The containing folder of the clicked row (folders: the folder itself).
    #[default]
    Parent,
    /// The clicked item itself.
    Item,
}

/// How sizes are formatted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeFormat {
    /// KiB/MiB/GiB (1024-based).
    #[default]
    Binary,
    /// kB/MB/GB (1000-based).
    Si,
    /// Exact byte count.
    Bytes,
}

/// UI theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Follow the desktop preference.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

/// What the right-click menu of a result shows besides the built-in entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextMenuConfig {
    /// "Open with" applications for the file type.
    pub open_with: bool,
    /// File-type specific actions installed by other programs (KDE service menus, Nemo and Thunar
    /// actions).
    pub service_menus: bool,
    /// Entry ids hidden from the menu (menu → Hide an entry).
    pub hidden: Vec<String>,
    /// Entry ids shown first, in this order; entries not listed keep their natural order after them.
    pub order: Vec<String>,
}

impl Default for ContextMenuConfig {
    fn default() -> Self {
        Self {
            open_with: true,
            service_menus: true,
            hidden: Vec::new(),
            order: Vec::new(),
        }
    }
}

/// UI settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Columns in display order.
    pub columns: Vec<ColumnConfig>,
    /// Keep the column widths as saved: the header borders can no longer be dragged. Path (or the
    /// last visible column if Path is hidden) still follows the window width.
    pub lock_column_widths: bool,
    /// Sort order.
    pub sort: SortConfig,
    /// Preview pane.
    pub preview_pane: PreviewPaneConfig,
    /// Extra entries of the row context menu.
    pub context_menu: ContextMenuConfig,
    /// Target of the "properties" double-click.
    pub properties_target: PropertiesTarget,
    /// Size formatting.
    pub size_format: SizeFormat,
    /// `chrono` strftime format for dates.
    pub date_format: String,
    /// Theme.
    pub theme: Theme,
    /// Start an inline rename when the already-selected Name cell is clicked again (slowly, not as
    /// part of a double-click). Off by default so double-click-to-open never turns into a rename.
    pub rename_on_slow_click: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        let col = |id, width| ColumnConfig {
            id,
            width,
            visible: true,
        };
        Self {
            columns: vec![
                col(ColumnId::Name, 280.0),
                col(ColumnId::Path, 380.0),
                col(ColumnId::Type, 140.0),
                col(ColumnId::Modified, 150.0),
                col(ColumnId::Size, 90.0),
            ],
            lock_column_widths: false,
            sort: SortConfig::default(),
            preview_pane: PreviewPaneConfig::default(),
            context_menu: ContextMenuConfig::default(),
            properties_target: PropertiesTarget::default(),
            size_format: SizeFormat::default(),
            date_format: "%Y-%m-%d %H:%M".to_owned(),
            theme: Theme::default(),
            rename_on_slow_click: false,
        }
    }
}

/// Key/mouse bindings as strings like `"Ctrl+Shift+R"` or `"Ctrl+Shift+LeftClick"`.
/// The UI parses them; unparsable bindings fall back to the default with a warning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keybindings {
    /// Full reindex of all volumes. With automatic indexing off, a binding shared with
    /// `update_index` belongs to `update_index`.
    pub reindex_all: String,
    /// Update the index on request: re-list the folders the app's file operations changed (see
    /// `indexer.automatic`), or quick-rescan all volumes when there are none. Shares its default
    /// with `reindex_all`: `Ctrl+Shift+R` runs this while automatic indexing is off and the
    /// full reindex while it is on.
    pub update_index: String,
    /// Quick rescan of all volumes.
    pub quick_rescan: String,
    /// Click modifier that adds a path to the exclusions.
    pub exclude_path_click: String,
    /// Toggle the preview pane.
    pub toggle_preview: String,
    /// Focus the search box.
    pub focus_search: String,
    /// Toggle matching against the full path.
    pub toggle_match_path: String,
    /// Open a new tab with an empty search.
    pub new_tab: String,
    /// Close the current tab (the last tab can't be closed).
    pub close_tab: String,
    /// Select the next tab, wrapping around. With Shift added it selects the previous one.
    pub next_tab: String,
    /// Add a search bar below the others (its query is combined with theirs by AND).
    pub add_search_bar: String,
    /// Close the focused extra search bar.
    pub close_search_bar: String,
    /// Move the keyboard focus between the result list and the newest search bar. With Shift
    /// added it does the same in reverse.
    pub switch_focus: String,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            reindex_all: "Ctrl+Shift+R".to_owned(),
            update_index: "Ctrl+Shift+R".to_owned(),
            quick_rescan: "F5".to_owned(),
            exclude_path_click: "Ctrl+Shift+LeftClick".to_owned(),
            toggle_preview: "Ctrl+P".to_owned(),
            focus_search: "Ctrl+F".to_owned(),
            toggle_match_path: "Ctrl+U".to_owned(),
            new_tab: "Ctrl+T".to_owned(),
            close_tab: "Ctrl+W".to_owned(),
            next_tab: "Ctrl+Tab".to_owned(),
            add_search_bar: "Ctrl+Shift+T".to_owned(),
            close_search_bar: "Ctrl+Shift+W".to_owned(),
            switch_focus: "Tab".to_owned(),
        }
    }
}

/// What to do on startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupRescan {
    /// Use the existing index as is.
    None,
    /// Re-list only folders whose mtime changed.
    #[default]
    Quick,
    /// Rescan everything.
    Full,
}

/// Indexer behaviour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent user toggles, stored flat in JSON"
)]
pub struct IndexerConfig {
    /// Automatic indexing. When off, the indexer does no background work of its own: no startup
    /// rescan (an empty database is still built once), no scheduled rescans and no inotify live
    /// updates, whatever `rescan_on_startup`, `scheduled_rescan_minutes` and `live_updates` say.
    /// The index is then refreshed on request only (`Ctrl+Shift+R`, F5 or the Index menu). The
    /// other fields keep their values, so switching back restores the previous behaviour.
    pub automatic: bool,
    /// Walker threads; `0` = number of CPUs.
    pub threads: usize,
    /// Follow symlinks (off by default; loops are detected either way).
    pub follow_symlinks: bool,
    /// Never cross into other filesystems.
    pub one_filesystem: bool,
    /// Startup behaviour.
    pub rescan_on_startup: StartupRescan,
    /// Periodic quick rescan interval; `0` disables it.
    pub scheduled_rescan_minutes: u32,
    /// Watch for changes with inotify while running (user mode).
    pub live_updates: bool,
    /// Roots watched with inotify when `live_updates` is on; empty = the user's home directory.
    pub watch_roots: Vec<PathBuf>,
}

impl Default for IndexerConfig {
    fn default() -> Self {
        Self {
            automatic: true,
            threads: 0,
            follow_symlinks: false,
            one_filesystem: true,
            rescan_on_startup: StartupRescan::default(),
            scheduled_rescan_minutes: 0,
            live_updates: true,
            watch_roots: Vec::new(),
        }
    }
}

impl IndexerConfig {
    /// The startup rescan that actually runs: [`StartupRescan::None`] when `automatic` is off.
    #[must_use]
    pub const fn effective_startup_rescan(&self) -> StartupRescan {
        if self.automatic {
            self.rescan_on_startup
        } else {
            StartupRescan::None
        }
    }

    /// The periodic rescan interval that actually runs; `0` (off) when `automatic` is off.
    #[must_use]
    pub const fn effective_scheduled_rescan_minutes(&self) -> u32 {
        if self.automatic {
            self.scheduled_rescan_minutes
        } else {
            0
        }
    }

    /// Whether the inotify watcher should run: `automatic` and `live_updates` are both on.
    #[must_use]
    pub const fn effective_live_updates(&self) -> bool {
        self.automatic && self.live_updates
    }
}

/// Search defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct SearchConfig {
    /// Case-sensitive matching.
    pub case_sensitive: bool,
    /// Match terms against the full path instead of just the name.
    pub match_path: bool,
    /// Treat the whole query as a regular expression.
    pub regex: bool,
    /// Cap on results; `0` = unlimited.
    pub max_results: usize,
}

impl Config {
    /// Loads the config from `path`. A missing file yields [`Config::default`].
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Io {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        Self::from_json(&text, path)
    }

    /// Parses config JSON (`origin` is only used in error messages) and applies migrations.
    pub fn from_json(text: &str, origin: &Path) -> Result<Self, ConfigError> {
        let mut value: serde_json::Value = serde_json::from_str(text).map_err(|e| parse_error(&e, origin))?;
        migrate(&mut value);
        serde_json::from_value(value).map_err(|e| parse_error(&e, origin))
    }

    /// Serialises to pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("Config serialisation is infallible")
    }

    /// Writes the config atomically: temp file in the same folder, `fsync`, rename.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let io = |source| ConfigError::Io {
            path: path.to_owned(),
            source,
        };
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir).map_err(io)?;
        let mut tmp = tempfile_in(dir).map_err(io)?;
        tmp.1.write_all(self.to_json().as_bytes()).map_err(io)?;
        tmp.1.write_all(b"\n").map_err(io)?;
        tmp.1.sync_all().map_err(io)?;
        drop(tmp.1);
        std::fs::rename(&tmp.0, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp.0);
            io(e)
        })
    }
}

fn tempfile_in(dir: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    let pid = std::process::id();
    for n in 0..100u32 {
        let p = dir.join(format!(".config.json.{pid}.{n}.tmp"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(f) => return Ok((p, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("cannot create temporary config file"))
}

fn parse_error(e: &serde_json::Error, origin: &Path) -> ConfigError {
    ConfigError::Parse {
        path: origin.to_owned(),
        line: e.line(),
        column: e.column(),
        message: e.to_string(),
    }
}

/// Upgrades an older config JSON value in place to [`CURRENT_VERSION`].
pub fn migrate(value: &mut serde_json::Value) {
    let Some(obj) = value.as_object_mut() else { return };
    let version = obj
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    // Version 0 (no field) -> 1: nothing to rename yet; just stamp the version.
    // Version 1 -> 2: Ctrl+T and Ctrl+W moved to tabs; the search bar bindings that still hold
    // the old defaults follow to their new ones, customised bindings are left alone.
    if version < 2
        && let Some(keys) = obj
            .get_mut("keybindings")
            .and_then(serde_json::Value::as_object_mut)
    {
        for (name, old, new) in [
            ("add_search_bar", "Ctrl+T", "Ctrl+Shift+T"),
            ("close_search_bar", "Ctrl+W", "Ctrl+Shift+W"),
        ] {
            if keys.get(name).and_then(serde_json::Value::as_str) == Some(old) {
                keys.insert(name.to_owned(), new.into());
            }
        }
    }
    if version < u64::from(CURRENT_VERSION) {
        obj.insert("version".to_owned(), CURRENT_VERSION.into());
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn version_1_search_bar_bindings_follow_the_tab_shortcuts() {
        let json =
            r#"{"version": 1, "keybindings": {"add_search_bar": "Ctrl+T", "close_search_bar": "Alt+W"}}"#;
        let config = Config::from_json(json, Path::new("c.json")).expect("loads");
        assert_eq!(config.version, CURRENT_VERSION);
        assert_eq!(config.keybindings.add_search_bar, "Ctrl+Shift+T");
        assert_eq!(
            config.keybindings.close_search_bar, "Alt+W",
            "custom binding kept"
        );
        assert_eq!(config.keybindings.new_tab, "Ctrl+T");
    }

    #[test]
    fn preview_pane_fields_default_when_missing() {
        let config = Config::from_json(
            r#"{ "ui": { "preview_pane": { "width": 400 } } }"#,
            Path::new("c.json"),
        )
        .expect("old config loads");
        let pane = &config.ui.preview_pane;
        assert!((pane.width - 400.0).abs() < f32::EPSILON);
        assert!(!pane.resizable, "fixed width unless asked for");
        assert!(pane.wrap_text && !pane.line_numbers);
    }

    #[test]
    fn column_widths_are_unlocked_when_missing() {
        let config = Config::from_json(r#"{ "ui": { "columns": [] } }"#, Path::new("c.json"))
            .expect("old config loads");
        assert!(!config.ui.lock_column_widths);
    }

    #[test]
    fn automatic_indexing_is_on_when_missing_from_an_old_config() {
        let old = r#"{ "version": 2, "indexer": { "live_updates": true, "scheduled_rescan_minutes": 30 } }"#;
        let config = Config::from_json(old, Path::new("c.json")).expect("old config loads");
        assert!(
            config.indexer.automatic,
            "existing configs keep indexing automatically"
        );
        assert!(config.indexer.effective_live_updates());
        assert_eq!(config.indexer.effective_scheduled_rescan_minutes(), 30);
        assert_eq!(config.indexer.effective_startup_rescan(), StartupRescan::Quick);
        assert!(Config::default().indexer.automatic);
        assert_eq!(config.version, CURRENT_VERSION, "no migration needed");
    }

    #[test]
    fn manual_mode_switches_off_every_background_trigger_but_keeps_the_values() {
        let json = r#"{ "indexer": { "automatic": false, "live_updates": true,
            "scheduled_rescan_minutes": 30, "rescan_on_startup": "full" } }"#;
        let config = Config::from_json(json, Path::new("c.json")).expect("loads");
        let indexer = &config.indexer;
        assert!(!indexer.automatic);
        assert!(!indexer.effective_live_updates());
        assert_eq!(indexer.effective_scheduled_rescan_minutes(), 0);
        assert_eq!(indexer.effective_startup_rescan(), StartupRescan::None);
        assert!(indexer.live_updates && indexer.scheduled_rescan_minutes == 30);
        assert_eq!(indexer.rescan_on_startup, StartupRescan::Full);
        let again = Config::from_json(&config.to_json(), Path::new("c.json")).expect("round trip");
        assert_eq!(again, config);
    }

    #[test]
    fn context_menu_defaults_when_missing() {
        let config = Config::from_json(
            r#"{ "ui": { "context_menu": { "hidden": ["x"] } } }"#,
            Path::new("c.json"),
        )
        .expect("loads");
        let menu = &config.ui.context_menu;
        assert!(menu.open_with && menu.service_menus);
        assert_eq!(menu.hidden, ["x"]);
        assert_eq!(Config::default().ui.context_menu.order, Vec::<String>::new());
    }
}
