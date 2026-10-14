//! Inline rename: the editor state and the name validation. Pure logic, no widgets and no I/O.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;
use std::path::PathBuf;

/// Longest file name ext4 accepts, in bytes (`NAME_MAX`).
pub(crate) const NAME_MAX: usize = 255;

/// The text shown next to the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Message {
    /// The name can't be used; the editor stays open.
    Error(String),
    /// The name can be used after a second `Enter`.
    Warning(String),
}

impl Message {
    /// The message text.
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Error(t) | Self::Warning(t) => t,
        }
    }
}

/// What a commit attempt should do with the current text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Same name as before: nothing to do.
    Unchanged,
    /// Not a valid name.
    Invalid(String),
    /// Valid, but the user must confirm with a second `Enter`.
    Confirm(String),
    /// Valid; rename to this.
    Rename(OsString),
}

/// Splits a name into stem and extension. A leading dot (`.bashrc`) or a trailing dot is not an
/// extension separator.
pub(crate) fn split_extension(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], Some(&name[i + 1..])),
        _ => (name, None),
    }
}

/// The first problem that makes `name` unusable, if any. `original_lossy` says the old name had
/// bytes that are not UTF-8 (so the text field shows replacement characters).
pub(crate) fn name_error(name: &str, original_lossy: bool) -> Option<String> {
    if name.is_empty() {
        return Some("The name can't be empty".to_owned());
    }
    if name == "." || name == ".." {
        return Some(format!("\"{name}\" is not a valid name"));
    }
    if name.contains('/') {
        return Some("A name can't contain \"/\"".to_owned());
    }
    if name.contains('\0') {
        return Some("A name can't contain NUL characters".to_owned());
    }
    if name.len() > NAME_MAX {
        return Some(format!(
            "The name is too long ({} bytes, the limit is {NAME_MAX})",
            name.len()
        ));
    }
    if original_lossy && name.contains('\u{FFFD}') {
        return Some("This name contains bytes that can't be shown; type the full new name".to_owned());
    }
    None
}

/// What the editor must do with focus on its next draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusRequest {
    /// Nothing.
    None,
    /// Just opened: take focus and select the stem.
    SelectStem,
    /// A commit was refused: take focus again, keeping the selection.
    Refocus,
}

/// State of the inline editor in the Name cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenameEdit {
    /// Full path of the entry being renamed (rows can move; the path identifies the entry).
    pub(crate) path: PathBuf,
    /// Row the editor is drawn in; kept in step with result changes by the app.
    pub(crate) row: usize,
    /// The name as stored on disk.
    pub(crate) original: OsString,
    /// What the editor showed initially (the lossy rendering of `original`).
    pub(crate) original_text: String,
    /// `original` is not valid UTF-8.
    pub(crate) lossy: bool,
    pub(crate) is_dir: bool,
    /// The text being edited.
    pub(crate) buffer: String,
    /// Inline feedback.
    pub(crate) message: Option<Message>,
    /// The text for which a confirmation warning is showing; a second `Enter` on the same text
    /// commits.
    pub(crate) warned_for: Option<String>,
    /// Focus work for the next draw.
    pub(crate) focus: FocusRequest,
    /// The editor was drawn at least once.
    pub(crate) shown: bool,
}

impl RenameEdit {
    /// Starts editing the entry at `path` whose on-disk name is `name`.
    pub(crate) fn new(path: PathBuf, row: usize, name: Vec<u8>, is_dir: bool) -> Self {
        let original = OsString::from_vec(name);
        let original_text = original.to_string_lossy().into_owned();
        let lossy = original.to_str().is_none();
        Self {
            path,
            row,
            original,
            buffer: original_text.clone(),
            original_text,
            lossy,
            is_dir,
            message: None,
            warned_for: None,
            focus: FocusRequest::SelectStem,
            shown: false,
        }
    }

    /// Number of characters to select initially: the stem for files, the whole name for folders.
    pub(crate) fn stem_chars(&self) -> usize {
        let text = &self.buffer;
        if self.is_dir {
            return text.chars().count();
        }
        split_extension(text).0.chars().count()
    }

    /// Called after the user changed the text: shows hard errors live and drops stale warnings.
    pub(crate) fn on_edited(&mut self) {
        if self.warned_for.as_deref() != Some(self.buffer.as_str()) {
            self.warned_for = None;
        }
        self.message = if self.buffer == self.original_text {
            None
        } else {
            name_error(&self.buffer, self.lossy).map(Message::Error)
        };
    }

    /// Decides what committing the current text means.
    pub(crate) fn verdict(&self) -> Verdict {
        let text = self.buffer.as_str();
        if text == self.original_text {
            return Verdict::Unchanged;
        }
        if let Some(error) = name_error(text, self.lossy) {
            return Verdict::Invalid(error);
        }
        let mut warnings: Vec<String> = Vec::new();
        if text.trim() != text {
            warnings.push("The name starts or ends with a space.".to_owned());
        }
        if !self.is_dir && !self.lossy {
            let old = split_extension(&self.original_text).1;
            let new = split_extension(text).1;
            match (old, new) {
                (Some(o), Some(n)) if o != n => {
                    warnings.push(format!("Change the extension from .{o} to .{n}?"));
                }
                (Some(o), None) => warnings.push(format!("Remove the extension .{o}?")),
                (None, Some(n)) => warnings.push(format!("Add the extension .{n}?")),
                _ => {}
            }
        }
        if warnings.is_empty() {
            Verdict::Rename(OsString::from(text))
        } else {
            Verdict::Confirm(warnings.join(" "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(name: &str, is_dir: bool) -> RenameEdit {
        RenameEdit::new(
            PathBuf::from("/x").join(name),
            0,
            name.as_bytes().to_vec(),
            is_dir,
        )
    }

    #[test]
    fn stem_excludes_the_last_extension() {
        let mut e = edit("report.final.pdf", false);
        assert_eq!(e.stem_chars(), "report.final".len());
        e = edit(".bashrc", false);
        assert_eq!(e.stem_chars(), ".bashrc".len(), "dotfiles have no extension");
        e = edit("Makefile", false);
        assert_eq!(e.stem_chars(), 8);
        e = edit("ünï.txt", false);
        assert_eq!(e.stem_chars(), 3, "counted in characters, not bytes");
        e = edit("photos.2024", true);
        assert_eq!(e.stem_chars(), 11, "folders select the whole name");
    }

    #[test]
    fn hard_errors() {
        assert!(name_error("", false).is_some());
        assert!(name_error(".", false).is_some());
        assert!(name_error("..", false).is_some());
        assert!(name_error("a/b", false).is_some());
        assert!(name_error("a\0b", false).is_some());
        assert!(name_error(&"x".repeat(256), false).is_some());
        assert!(name_error(&"x".repeat(255), false).is_none());
        assert!(
            name_error(&"é".repeat(128), false).is_some(),
            "the limit counts bytes (256 here)"
        );
        assert!(name_error("a\u{FFFD}b", true).is_some());
        assert!(
            name_error("a\u{FFFD}b", false).is_none(),
            "a deliberate U+FFFD is fine"
        );
        assert!(name_error("...", false).is_none());
    }

    #[test]
    fn verdicts() {
        let mut e = edit("a.txt", false);
        assert_eq!(e.verdict(), Verdict::Unchanged);
        e.buffer = "b.txt".into();
        assert_eq!(e.verdict(), Verdict::Rename("b.txt".into()));
        e.buffer = "b.md".into();
        assert_eq!(
            e.verdict(),
            Verdict::Confirm("Change the extension from .txt to .md?".into())
        );
        e.buffer = "b".into();
        assert_eq!(e.verdict(), Verdict::Confirm("Remove the extension .txt?".into()));
        e.buffer = "b.txt ".into();
        assert!(matches!(e.verdict(), Verdict::Confirm(m) if m.contains("space") && m.contains("extension")));
        e.buffer = "a/b".into();
        assert!(matches!(e.verdict(), Verdict::Invalid(_)));

        let mut d = edit("dir.d", true);
        d.buffer = "other.x".into();
        assert_eq!(
            d.verdict(),
            Verdict::Rename("other.x".into()),
            "folders have no extension"
        );
    }

    #[test]
    fn non_utf8_names_keep_their_bytes_when_unchanged() {
        let mut e = RenameEdit::new("/x/bad\u{FFFD}".into(), 0, b"bad\xffname".to_vec(), false);
        assert!(e.lossy);
        assert_eq!(e.verdict(), Verdict::Unchanged);
        e.buffer = "bad\u{FFFD}name2".into();
        assert!(matches!(e.verdict(), Verdict::Invalid(m) if m.contains("can't be shown")));
        e.buffer = "good".into();
        assert_eq!(e.verdict(), Verdict::Rename("good".into()));
    }

    #[test]
    fn live_errors_follow_the_text() {
        let mut e = edit("a.txt", false);
        e.buffer = String::new();
        e.on_edited();
        assert!(matches!(e.message, Some(Message::Error(_))));
        e.buffer = "ok.txt".into();
        e.on_edited();
        assert_eq!(e.message, None);
        e.warned_for = Some("ok.txt".into());
        e.buffer = "ok2.txt".into();
        e.on_edited();
        assert_eq!(e.warned_for, None, "editing resets the confirmation");
    }
}
