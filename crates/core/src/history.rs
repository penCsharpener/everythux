//! The search history: every search the user ran, kept indefinitely.
//!
//! The search itself is the key (the texts of its search bars); running it again only moves its
//! date forward. Lives in `history.json` next to the config, so a growing history never makes
//! the config file noisy.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One remembered search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HistoryEntry {
    /// The text of each search bar, top to bottom. Never empty, no blank texts.
    pub texts: Vec<String>,
    /// When it was last used, in seconds since the Unix epoch.
    pub last_used: i64,
}

impl HistoryEntry {
    /// The search as one line: the bar texts joined by `" AND "`.
    #[must_use]
    pub fn label(&self) -> String {
        self.texts.join(" AND ")
    }
}

/// Root of `history.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SearchHistory {
    /// Entries in no particular order; use [`Self::newest_first`].
    pub entries: Vec<HistoryEntry>,
}

/// The texts of a search with blank bars removed and surrounding whitespace trimmed, or `None`
/// when nothing is left (a blank search is not worth remembering).
#[must_use]
pub fn normalize(texts: &[String]) -> Option<Vec<String>> {
    let kept: Vec<String> = texts
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect();
    (!kept.is_empty()).then_some(kept)
}

impl SearchHistory {
    /// Records that `texts` was used at `now`: adds it, or moves an existing entry forward.
    /// Returns whether anything was recorded (blank searches are not).
    pub fn touch(&mut self, texts: &[String], now: i64) -> bool {
        let Some(texts) = normalize(texts) else {
            return false;
        };
        match self.entries.iter_mut().find(|e| e.texts == texts) {
            Some(entry) => entry.last_used = now,
            None => self.entries.push(HistoryEntry {
                texts,
                last_used: now,
            }),
        }
        true
    }

    /// Forgets the search with exactly these `texts`. Returns whether it was there.
    pub fn remove(&mut self, texts: &[String]) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.texts != texts);
        self.entries.len() != before
    }

    /// The entries, most recently used first.
    #[must_use]
    pub fn newest_first(&self) -> Vec<&HistoryEntry> {
        let mut sorted: Vec<&HistoryEntry> = self.entries.iter().collect();
        sorted.sort_by_key(|e| std::cmp::Reverse(e.last_used));
        sorted
    }

    /// Loads `path`. A missing file is an empty history.
    ///
    /// # Errors
    ///
    /// The I/O or JSON error as text; the caller should carry on with an empty history.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Writes the history atomically: temp file next to it, `fsync`, rename.
    ///
    /// # Errors
    ///
    /// The I/O error as text.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let io = |e: std::io::Error| format!("{}: {e}", path.display());
        let dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).map_err(io)?;
        let tmp: PathBuf = dir.join(format!(".history.json.{}.tmp", std::process::id()));
        let json = serde_json::to_string_pretty(self).expect("history serialisation is infallible");
        let mut file = std::fs::File::create(&tmp).map_err(io)?;
        file.write_all(json.as_bytes()).map_err(io)?;
        file.write_all(b"\n").map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            io(e)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn the_search_is_the_key_and_reuse_moves_it_forward() {
        let mut h = SearchHistory::default();
        assert!(h.touch(&s(&["report"]), 10));
        assert!(h.touch(&s(&["  ", "ext:pdf"]), 20));
        assert!(h.touch(&s(&["report"]), 30), "same search, newer date");
        assert_eq!(h.entries.len(), 2);
        let order: Vec<String> = h.newest_first().iter().map(|e| e.label()).collect();
        assert_eq!(order, ["report", "ext:pdf"]);
        assert!(!h.touch(&s(&["", " "]), 40), "blank searches are not kept");
    }

    #[test]
    fn remove_and_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.json");
        assert_eq!(SearchHistory::load(&path), Ok(SearchHistory::default()));
        let mut h = SearchHistory::default();
        h.touch(&s(&["a", "b"]), 1);
        h.touch(&s(&["c"]), 2);
        h.save(&path).expect("save");
        let mut loaded = SearchHistory::load(&path).expect("load");
        assert_eq!(loaded, h);
        assert!(loaded.remove(&s(&["a", "b"])));
        assert!(!loaded.remove(&s(&["a", "b"])));
        assert_eq!(loaded.entries.len(), 1);
    }
}
