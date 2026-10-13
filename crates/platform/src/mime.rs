//! File type descriptions for the Type column.

/// Human-readable type for a lowercase extension, e.g. `"pdf"` → `"PDF document"`, `""` → `"File"`.
/// Uses shared-mime-info (`/usr/share/mime/<type>.xml` `<comment>`) when present, otherwise a
/// built-in table / `"<EXT> file"`. Results are cached; cheap to call per visible row.
pub fn describe_extension(ext: &str) -> std::sync::Arc<str> {
    let _ = ext;
    todo!()
}

/// Broad category used for icons and quick filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// Folders.
    Folder,
    /// Images.
    Image,
    /// Audio.
    Audio,
    /// Video.
    Video,
    /// Documents (pdf, office, text).
    Document,
    /// Archives.
    Archive,
    /// Source code.
    Code,
    /// Executables / libraries.
    Executable,
    /// Anything else.
    Other,
}

/// Category for a lowercase extension (folders are handled by the caller).
pub fn category(ext: &str) -> Category {
    let _ = ext;
    todo!()
}
