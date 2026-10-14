//! MIME types of files for the context menu: guessed from the name, with `shared-mime-info`
//! subclass relations (`text/x-rust` is a `text/plain`) loaded from the XDG data directories.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The MIME type used for folders.
pub const DIRECTORY: &str = "inode/directory";

/// Best-effort MIME type of `path` (folders are `inode/directory`).
#[must_use]
pub fn mime_of(path: &Path, is_dir: bool) -> String {
    if is_dir {
        return DIRECTORY.to_owned();
    }
    mime_guess::from_path(path).first().map_or_else(
        || "application/octet-stream".to_owned(),
        |m| m.essence_str().to_owned(),
    )
}

/// Parent relations between MIME types.
#[derive(Debug, Clone, Default)]
pub struct MimeTree {
    parents: HashMap<String, Vec<String>>,
}

impl MimeTree {
    /// Reads `<dir>/mime/subclasses` for every data directory (missing files are skipped).
    #[must_use]
    pub fn load(data_dirs: &[PathBuf]) -> Self {
        let mut tree = Self::default();
        for dir in data_dirs {
            if let Ok(text) = std::fs::read_to_string(dir.join("mime/subclasses")) {
                tree.add_subclasses(&text);
            }
        }
        tree
    }

    /// Adds `child parent` lines (the `subclasses` file format).
    pub fn add_subclasses(&mut self, text: &str) {
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            if let (Some(child), Some(parent), None) = (parts.next(), parts.next(), parts.next())
                && !child.starts_with('#')
            {
                self.parents
                    .entry(child.to_owned())
                    .or_default()
                    .push(parent.to_owned());
            }
        }
    }

    /// `mime` and everything it is a kind of, most specific first.
    #[must_use]
    pub fn ancestors_of(&self, mime: &str) -> Vec<String> {
        self.ancestors(mime)
    }

    /// Whether `mime` is `app_mime` or a kind of it (an application's `MimeType=` entry).
    #[must_use]
    pub fn matches_exact_or_parent(&self, mime: &str, app_mime: &str) -> bool {
        self.ancestors(mime).iter().any(|m| m == app_mime)
    }

    /// `mime` and everything it is a kind of.
    fn ancestors(&self, mime: &str) -> Vec<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut order = Vec::new();
        let mut stack = vec![mime.to_owned()];
        while let Some(current) = stack.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            if let Some(parents) = self.parents.get(&current) {
                stack.extend(parents.iter().cloned());
            }
            order.push(current);
        }
        // Implicit relations of the spec: any text type is text/plain; any file is a stream of bytes.
        if mime.starts_with("text/") && !seen.contains("text/plain") {
            order.push("text/plain".to_owned());
        }
        if mime != DIRECTORY && !seen.contains("application/octet-stream") {
            order.push("application/octet-stream".to_owned());
        }
        order
    }

    /// Whether a file of type `mime` matches the service-menu `pattern`: an exact type, a
    /// `group/*` wildcard, `all/all`, `all/allfiles` (any non-folder) or a parent type.
    #[must_use]
    pub fn matches(&self, mime: &str, pattern: &str) -> bool {
        let is_dir = mime == DIRECTORY;
        match pattern {
            "all/all" | "*" | "*/*" => return true,
            "all/allfiles" | "application/octet-stream" => return !is_dir,
            _ => {}
        }
        if let Some(group) = pattern.strip_suffix("/*") {
            return self
                .ancestors(mime)
                .iter()
                .any(|m| m.split_once('/').is_some_and(|(g, _)| g == group));
        }
        self.ancestors(mime).iter().any(|m| m == pattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guesses_from_the_extension() {
        assert_eq!(mime_of(Path::new("/x/a.png"), false), "image/png");
        assert_eq!(mime_of(Path::new("/x/a.png"), true), DIRECTORY);
        assert_eq!(mime_of(Path::new("/x/noext"), false), "application/octet-stream");
    }

    #[test]
    fn subclasses_and_wildcards() {
        let mut tree = MimeTree::default();
        tree.add_subclasses("text/x-rust text/plain\ntext/markdown text/x-readme\n# c d\n");
        assert!(tree.matches("text/x-rust", "text/plain"));
        assert!(tree.matches("text/x-rust", "text/*"));
        assert!(tree.matches("image/png", "image/*"));
        assert!(!tree.matches("image/png", "text/*"));
        assert!(tree.matches("image/png", "all/allfiles"));
        assert!(!tree.matches(DIRECTORY, "all/allfiles"));
        assert!(tree.matches(DIRECTORY, "inode/directory"));
        assert!(tree.matches(DIRECTORY, "all/all"));
        assert!(tree.matches("text/csv", "text/plain"), "implicit text/plain");
    }
}
