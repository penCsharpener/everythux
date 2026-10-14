//! Small LRU cache of finished previews, so flipping back and forth between files is instant.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::{PreviewContent, PreviewLimits};

/// Entries kept at most.
pub(crate) const CAPACITY: usize = 32;
/// Approximate pixel/text payload kept at most (decoded images are large).
pub(crate) const BYTE_BUDGET: usize = 96 * 1024 * 1024;

/// Identity of a preview: the file as it is on disk plus the limits it was rendered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CacheKey {
    path: PathBuf,
    mtime: SystemTime,
    size: u64,
    limits: PreviewLimits,
}

impl CacheKey {
    /// `None` when the file cannot be stat'ed (then the result is not cached).
    pub(crate) fn of(path: &Path, limits: PreviewLimits) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            path: path.to_path_buf(),
            mtime: meta.modified().ok()?,
            size: meta.len(),
            limits,
        })
    }
}

#[derive(Debug)]
struct Entry {
    key: CacheKey,
    content: PreviewContent,
    bytes: usize,
}

/// Least-recently-used cache; the most recent entry is last. Linear scans are fine at this size.
#[derive(Debug)]
pub(crate) struct PreviewCache {
    entries: Vec<Entry>,
    capacity: usize,
    byte_budget: usize,
    bytes: usize,
}

impl PreviewCache {
    pub(crate) fn new(capacity: usize, byte_budget: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            capacity,
            byte_budget,
            bytes: 0,
        }
    }

    /// Cached content, marking it most recently used.
    pub(crate) fn get(&mut self, key: &CacheKey) -> Option<&PreviewContent> {
        let index = self.entries.iter().position(|e| e.key == *key)?;
        let entry = self.entries.remove(index);
        self.entries.push(entry);
        self.entries.last().map(|e| &e.content)
    }

    pub(crate) fn insert(&mut self, key: CacheKey, content: PreviewContent) {
        if let Some(index) = self.entries.iter().position(|e| e.key == key) {
            self.bytes -= self.entries.remove(index).bytes;
        }
        let bytes = content_bytes(&content);
        self.bytes += bytes;
        self.entries.push(Entry { key, content, bytes });
        // Evict from the least recently used end, but never the entry just added.
        while self.entries.len() > 1 && (self.entries.len() > self.capacity || self.bytes > self.byte_budget)
        {
            self.bytes -= self.entries.remove(0).bytes;
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Whether a result is worth remembering. Failures and degraded results may be transient
/// (a tool was missing or timed out), so they are recomputed.
pub(crate) fn is_cacheable(content: &PreviewContent) -> bool {
    !matches!(
        content,
        PreviewContent::Unavailable { .. } | PreviewContent::Video { thumbnail: None, .. }
    )
}

fn content_bytes(content: &PreviewContent) -> usize {
    const BASE: usize = 256;
    BASE + match content {
        PreviewContent::Image { image, .. } => image.rgba.len(),
        PreviewContent::Pdf { page, .. } => page.rgba.len(),
        PreviewContent::Text { text, .. } => text.len(),
        PreviewContent::Audio { cover, .. } => cover.as_ref().map_or(0, |c| c.rgba.len()),
        PreviewContent::Video { thumbnail, .. } => thumbnail.as_ref().map_or(0, |c| c.rgba.len()),
        PreviewContent::Folder { .. } | PreviewContent::Unavailable { .. } => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::ImageData;

    fn key(name: &str, size: u64) -> CacheKey {
        CacheKey {
            path: PathBuf::from(name),
            mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            size,
            limits: PreviewLimits::default(),
        }
    }

    fn text(s: &str) -> PreviewContent {
        PreviewContent::text(s)
    }

    #[test]
    fn evicts_least_recently_used() {
        let mut cache = PreviewCache::new(2, usize::MAX);
        cache.insert(key("a", 1), text("a"));
        cache.insert(key("b", 1), text("b"));
        assert!(
            cache.get(&key("a", 1)).is_some(),
            "touch a so b becomes the oldest"
        );
        cache.insert(key("c", 1), text("c"));
        assert_eq!(cache.len(), 2);
        assert!(cache.get(&key("b", 1)).is_none(), "b was least recently used");
        assert!(cache.get(&key("a", 1)).is_some());
        assert!(cache.get(&key("c", 1)).is_some());
    }

    #[test]
    fn key_covers_size_mtime_and_limits() {
        let mut cache = PreviewCache::new(8, usize::MAX);
        cache.insert(key("a", 1), text("old"));
        assert!(cache.get(&key("a", 2)).is_none(), "size changed");
        let mut newer = key("a", 1);
        newer.mtime += Duration::from_secs(1);
        assert!(cache.get(&newer).is_none(), "mtime changed");
        let mut other_limits = key("a", 1);
        other_limits.limits.max_dim = 64;
        assert!(cache.get(&other_limits).is_none(), "limits changed");
    }

    #[test]
    fn reinserting_replaces_without_double_counting() {
        let mut cache = PreviewCache::new(4, usize::MAX);
        cache.insert(key("a", 1), text("one"));
        cache.insert(key("a", 1), text("two"));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&key("a", 1)), Some(&text("two")));
    }

    #[test]
    fn byte_budget_evicts_big_entries_but_keeps_the_newest() {
        let image = |n: usize| PreviewContent::Image {
            image: ImageData {
                width: 1,
                height: 1,
                rgba: vec![0; n],
            },
            original_size: None,
        };
        let mut cache = PreviewCache::new(10, 1000);
        cache.insert(key("a", 1), image(600));
        cache.insert(key("b", 1), image(600));
        assert!(cache.get(&key("a", 1)).is_none());
        assert!(cache.get(&key("b", 1)).is_some());
        cache.insert(key("huge", 1), image(5000));
        assert_eq!(cache.len(), 1, "an entry over the budget is still kept alone");
    }

    #[test]
    fn failures_are_not_cacheable() {
        assert!(is_cacheable(&text("x")));
        assert!(is_cacheable(&PreviewContent::Folder { children: Some(1) }));
        assert!(!is_cacheable(&PreviewContent::Unavailable { hint: "x".into() }));
        assert!(!is_cacheable(&PreviewContent::Video {
            meta: crate::MediaMeta::default(),
            thumbnail: None
        }));
    }
}
