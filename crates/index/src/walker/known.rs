//! What the database already knows about a folder tree, for incremental walks.

use std::collections::HashMap;

/// One folder that is already stored in the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownDir {
    /// Database id of the folder.
    pub id: i64,
    /// Id of the containing folder (`None` for a volume root).
    pub parent: Option<i64>,
    /// Folder mtime (ns) at the last scan.
    pub mtime_ns: i64,
    /// Names (final path components) of the known sub-folders.
    pub children: Vec<Box<[u8]>>,
}

/// A row of the `dirs` table, as the input for [`KnownTree::build`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownRow {
    /// Database id.
    pub id: i64,
    /// Parent folder id.
    pub parent: Option<i64>,
    /// Absolute path, raw bytes.
    pub path: Vec<u8>,
    /// Folder mtime (ns).
    pub mtime_ns: i64,
}

/// `(path of the parent, name of the child)`.
type Link = (Box<[u8]>, Box<[u8]>);

/// Known folders keyed by absolute path (raw bytes).
#[derive(Debug, Default)]
pub struct KnownTree {
    dirs: HashMap<Box<[u8]>, KnownDir>,
}

impl KnownTree {
    /// An empty tree: everything the walker finds is new.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the tree and links every folder to its known children.
    pub fn build(rows: Vec<KnownRow>) -> Self {
        let mut dirs: HashMap<Box<[u8]>, KnownDir> = HashMap::with_capacity(rows.len());
        let mut path_of: HashMap<i64, Box<[u8]>> = HashMap::with_capacity(rows.len());
        for row in rows {
            let path: Box<[u8]> = row.path.into_boxed_slice();
            path_of.insert(row.id, path.clone());
            dirs.insert(
                path,
                KnownDir {
                    id: row.id,
                    parent: row.parent,
                    mtime_ns: row.mtime_ns,
                    children: Vec::new(),
                },
            );
        }
        let links: Vec<Link> = dirs
            .iter()
            .filter_map(|(path, dir)| {
                let parent_path = path_of.get(&dir.parent?)?;
                let name = path.rsplit(|&b| b == b'/').next().unwrap_or_default();
                Some((parent_path.clone(), Box::from(name)))
            })
            .collect();
        for (parent_path, name) in links {
            if let Some(parent) = dirs.get_mut(&parent_path) {
                parent.children.push(name);
            }
        }
        Self { dirs }
    }

    /// The known folder at absolute `path`.
    pub fn get(&self, path: &[u8]) -> Option<&KnownDir> {
        self.dirs.get(path)
    }

    /// Number of known folders.
    pub fn len(&self) -> usize {
        self.dirs.len()
    }

    /// Whether the tree is empty.
    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_children_by_name() {
        let row = |id, parent, path: &str, mtime_ns| KnownRow {
            id,
            parent,
            path: path.as_bytes().to_vec(),
            mtime_ns,
        };
        let tree = KnownTree::build(vec![
            row(1, None, "/", 10),
            row(2, Some(1), "/a", 20),
            row(3, Some(1), "/b", 30),
            row(4, Some(2), "/a/c", 40),
        ]);
        assert_eq!(tree.len(), 4);
        let mut root_children: Vec<_> = tree.get(b"/").map(|d| d.children.clone()).unwrap_or_default();
        root_children.sort();
        assert_eq!(root_children, [Box::from(&b"a"[..]), Box::from(&b"b"[..])]);
        assert_eq!(tree.get(b"/a").map(|d| d.children.len()), Some(1));
        assert_eq!(tree.get(b"/a/c").map(|d| d.parent), Some(Some(2)));
        assert!(tree.get(b"/zzz").is_none());
    }
}
