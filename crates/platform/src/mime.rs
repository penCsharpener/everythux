//! File type descriptions for the Type column.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

/// Upper bound on cached extensions; protects against trees with endless distinct extensions.
const CACHE_LIMIT: usize = 8192;

type Cache = RwLock<HashMap<Box<str>, Arc<str>>>;

static CACHE: LazyLock<Cache> = LazyLock::new(|| RwLock::new(HashMap::new()));
static DATA_DIRS: LazyLock<Vec<PathBuf>> = LazyLock::new(|| {
    data_dirs_from(
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("XDG_DATA_DIRS"),
        std::env::var_os("HOME"),
    )
});

/// Human-readable type for a lowercase extension, e.g. `"pdf"` → `"PDF document"`, `""` → `"File"`.
/// Uses shared-mime-info (`/usr/share/mime/<type>.xml` `<comment>`) when present, otherwise a
/// built-in table / `"<EXT> file"`. Results are cached; cheap to call per visible row.
///
/// The first call for an extension reads a small XML file, so a cold call may touch the
/// filesystem; later calls are a read-lock plus an `Arc` clone.
pub fn describe_extension(ext: &str) -> Arc<str> {
    if let Some(hit) = CACHE.read().unwrap_or_else(PoisonError::into_inner).get(ext) {
        return Arc::clone(hit);
    }
    let described: Arc<str> = describe_uncached(ext, &DATA_DIRS).into();
    let mut cache = CACHE.write().unwrap_or_else(PoisonError::into_inner);
    if cache.len() < CACHE_LIMIT {
        cache.insert(ext.into(), Arc::clone(&described));
    }
    described
}

/// Computes the description without the cache, looking for shared-mime-info in `data_dirs`.
fn describe_uncached(ext: &str, data_dirs: &[PathBuf]) -> String {
    if ext.is_empty() {
        return "File".to_owned();
    }
    let lower = ext.to_ascii_lowercase();
    if let Some(mime) = mime_guess::from_ext(&lower).first()
        && let Some(comment) = data_dirs
            .iter()
            .find_map(|dir| shared_mime_comment(dir, mime.essence_str()))
    {
        return comment;
    }
    if let Some(name) = builtin_description(&lower) {
        return name.to_owned();
    }
    format!("{} file", ext.to_ascii_uppercase())
}

/// Splits `$XDG_DATA_HOME` and `$XDG_DATA_DIRS` into the ordered search list.
fn data_dirs_from(
    data_home: Option<OsString>,
    data_dirs: Option<OsString>,
    home: Option<OsString>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match data_home.filter(|v| !v.is_empty()) {
        Some(dir) => dirs.push(PathBuf::from(dir)),
        None => {
            if let Some(home) = home.filter(|v| !v.is_empty()) {
                dirs.push(Path::new(&home).join(".local/share"));
            }
        }
    }
    let system = data_dirs
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| OsString::from("/usr/local/share:/usr/share"));
    dirs.extend(std::env::split_paths(&system).filter(|p| !p.as_os_str().is_empty()));
    dirs
}

/// Reads the untranslated `<comment>` from `<data_dir>/mime/<mime>.xml`.
fn shared_mime_comment(data_dir: &Path, mime: &str) -> Option<String> {
    // `mime` comes from mime_guess's static table, so it cannot contain `..` or extra separators.
    let path = data_dir.join("mime").join(format!("{mime}.xml"));
    let xml = std::fs::read_to_string(path).ok()?;
    let comment = untranslated_comment(&xml)?;
    (!comment.is_empty()).then_some(comment)
}

/// Finds the `<comment>` element without an `xml:lang` attribute and unescapes its text.
fn untranslated_comment(xml: &str) -> Option<String> {
    const OPEN: &str = "<comment";
    const CLOSE: &str = "</comment>";
    let mut rest = xml;
    while let Some(start) = rest.find(OPEN) {
        rest = &rest[start + OPEN.len()..];
        let tag_end = rest.find('>')?;
        let attrs = &rest[..tag_end];
        // Reject `<comments>` and similar; accept `<comment>` and `<comment attr=..>`.
        if !(attrs.is_empty() || attrs.starts_with(char::is_whitespace)) {
            continue;
        }
        let body = &rest[tag_end + 1..];
        let end = body.find(CLOSE)?;
        if !attrs.contains("xml:lang") {
            return Some(unescape(body[..end].trim()));
        }
        rest = &body[end + CLOSE.len()..];
    }
    None
}

/// Resolves the five predefined XML entities and numeric character references.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let resolved = rest.find(';').and_then(|semi| {
            let entity = &rest[1..semi];
            let ch = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => entity
                    .strip_prefix('#')
                    .and_then(|num| {
                        num.strip_prefix(['x', 'X'])
                            .map_or_else(|| num.parse().ok(), |hex| u32::from_str_radix(hex, 16).ok())
                    })
                    .and_then(char::from_u32),
            };
            ch.map(|c| (c, semi + 1))
        });
        if let Some((ch, consumed)) = resolved {
            out.push(ch);
            rest = &rest[consumed..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Names for common extensions, used when shared-mime-info is missing or has no entry.
fn builtin_description(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "pdf" => "PDF document",
        "txt" | "text" | "log" => "Plain text document",
        "md" | "markdown" => "Markdown document",
        "rtf" => "Rich text document",
        "doc" | "docx" => "Word document",
        "xls" | "xlsx" => "Excel spreadsheet",
        "ppt" | "pptx" => "PowerPoint presentation",
        "odt" => "OpenDocument text document",
        "ods" => "OpenDocument spreadsheet",
        "odp" => "OpenDocument presentation",
        "csv" => "CSV document",
        "json" => "JSON document",
        "xml" => "XML document",
        "html" | "htm" => "HTML document",
        "png" => "PNG image",
        "jpg" | "jpeg" => "JPEG image",
        "gif" => "GIF image",
        "bmp" => "BMP image",
        "webp" => "WebP image",
        "svg" => "SVG image",
        "tif" | "tiff" => "TIFF image",
        "ico" => "Icon image",
        "mp3" => "MP3 audio",
        "flac" => "FLAC audio",
        "ogg" | "oga" => "Ogg audio",
        "wav" => "WAV audio",
        "m4a" => "MPEG-4 audio",
        "mp4" | "m4v" => "MPEG-4 video",
        "mkv" => "Matroska video",
        "avi" => "AVI video",
        "webm" => "WebM video",
        "mov" => "QuickTime video",
        "zip" => "ZIP archive",
        "tar" => "Tar archive",
        "gz" | "tgz" => "Gzip archive",
        "bz2" => "Bzip2 archive",
        "xz" => "XZ archive",
        "zst" => "Zstandard archive",
        "7z" => "7-zip archive",
        "rar" => "RAR archive",
        "deb" => "Debian package",
        "rpm" => "RPM package",
        "iso" => "Disc image",
        "rs" => "Rust source code",
        "py" => "Python script",
        "js" => "JavaScript source code",
        "ts" => "TypeScript source code",
        "c" => "C source code",
        "h" => "C header",
        "cpp" | "cc" | "cxx" => "C++ source code",
        "java" => "Java source code",
        "go" => "Go source code",
        "sh" | "bash" => "Shell script",
        "toml" => "TOML document",
        "yaml" | "yml" => "YAML document",
        "sql" => "SQL script",
        "so" => "Shared library",
        "exe" => "Windows executable",
        "appimage" => "AppImage application",
        "bin" => "Binary file",
        _ => return None,
    })
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
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "tif" | "tiff" | "ico" | "heic"
        | "heif" | "avif" | "raw" | "cr2" | "nef" | "arw" | "dng" | "psd" | "xcf" => Category::Image,
        "mp3" | "flac" | "ogg" | "oga" | "opus" | "wav" | "m4a" | "aac" | "wma" | "aiff" | "mid" | "midi" => {
            Category::Audio
        }
        "mp4" | "m4v" | "mkv" | "avi" | "webm" | "mov" | "wmv" | "flv" | "mpg" | "mpeg" | "ogv" | "3gp" => {
            Category::Video
        }
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp" | "rtf" | "txt"
        | "text" | "md" | "markdown" | "epub" | "djvu" | "csv" | "tex" => Category::Document,
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "lz4" | "lzma" | "cab"
        | "iso" | "deb" | "rpm" | "jar" | "war" => Category::Archive,
        "rs" | "py" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "c" | "h" | "cc" | "cpp" | "cxx" | "hpp"
        | "hh" | "java" | "kt" | "go" | "rb" | "php" | "cs" | "swift" | "lua" | "sh" | "bash" | "zsh"
        | "fish" | "pl" | "sql" | "html" | "htm" | "css" | "scss" | "json" | "toml" | "yaml" | "yml"
        | "xml" | "zig" | "hs" | "ml" | "ex" | "exs" | "dart" | "vue" | "svelte" => Category::Code,
        "so" | "a" | "o" | "dll" | "exe" | "msi" | "bin" | "appimage" | "elf" | "ko" | "out" | "run"
        | "bat" | "com" | "dylib" => Category::Executable,
        _ => Category::Other,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn fake_data_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp dir");
        for (rel, content) in files {
            let path = dir.path().join("mime").join(rel);
            fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
            fs::write(path, content).expect("write");
        }
        dir
    }

    const PDF_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<mime-type xmlns="http://www.freedesktop.org/standards/shared-mime-info" type="application/pdf">
  <!--Created automatically by update-mime-database. DO NOT EDIT!-->
  <comment xml:lang="de">PDF-Dokument</comment>
  <comment>Fake &amp; &lt;PDF&gt; &#65;&#x42;</comment>
  <comment xml:lang="fr">Document PDF</comment>
  <acronym>PDF</acronym>
</mime-type>"#;

    #[test]
    fn reads_untranslated_comment_from_injected_dir() {
        let dir = fake_data_dir(&[("application/pdf.xml", PDF_XML)]);
        let dirs = [dir.path().to_path_buf()];
        assert_eq!(describe_uncached("pdf", &dirs), "Fake & <PDF> AB");
        assert_eq!(
            describe_uncached("PDF", &dirs),
            "Fake & <PDF> AB",
            "case-insensitive"
        );
    }

    #[test]
    fn first_data_dir_with_the_type_wins() {
        let a = fake_data_dir(&[]);
        let b = fake_data_dir(&[("application/pdf.xml", PDF_XML)]);
        let dirs = [a.path().to_path_buf(), b.path().to_path_buf()];
        assert_eq!(describe_uncached("pdf", &dirs), "Fake & <PDF> AB");
    }

    #[test]
    fn falls_back_to_builtin_table_then_uppercase_ext() {
        let empty = fake_data_dir(&[]);
        let dirs = [empty.path().to_path_buf()];
        assert_eq!(describe_uncached("pdf", &dirs), "PDF document");
        assert_eq!(describe_uncached("xyzzy", &dirs), "XYZZY file");
        assert_eq!(describe_uncached("", &dirs), "File");
    }

    #[test]
    fn comment_without_lang_missing_falls_back() {
        let xml = r#"<mime-type><comment xml:lang="de">Nur Deutsch</comment></mime-type>"#;
        let dir = fake_data_dir(&[("application/pdf.xml", xml)]);
        let dirs = [dir.path().to_path_buf()];
        assert_eq!(describe_uncached("pdf", &dirs), "PDF document");
    }

    #[test]
    fn comment_parser_edge_cases() {
        assert_eq!(untranslated_comment("<comments>x</comments>"), None);
        assert_eq!(untranslated_comment("<comment>unterminated"), None);
        assert_eq!(
            untranslated_comment("<comment >  spaced  </comment>").as_deref(),
            Some("spaced")
        );
        assert_eq!(unescape("a &unknown; & b &amp"), "a &unknown; & b &amp");
    }

    #[test]
    fn uses_real_shared_mime_info_when_installed() {
        let pdf = Path::new("/usr/share/mime/application/pdf.xml");
        if !pdf.exists() {
            return;
        }
        let dirs = [PathBuf::from("/usr/share")];
        assert_eq!(describe_uncached("pdf", &dirs), "PDF document");
        let png = describe_uncached("png", &dirs);
        assert!(png.to_lowercase().contains("png"), "{png}");
    }

    #[test]
    fn describe_extension_is_cached_and_shared() {
        let first = describe_extension("zzcachetest");
        let second = describe_extension("zzcachetest");
        assert_eq!(&*first, "ZZCACHETEST file");
        assert!(
            Arc::ptr_eq(&first, &second),
            "second call must reuse the cached Arc"
        );
        assert_eq!(&*describe_extension(""), "File");
    }

    #[test]
    fn data_dirs_follow_xdg_rules() {
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(
            data_dirs_from(None, None, os("/home/u")),
            [
                PathBuf::from("/home/u/.local/share"),
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share")
            ]
        );
        assert_eq!(
            data_dirs_from(os("/h"), os("/a::/b"), os("/home/u")),
            [PathBuf::from("/h"), PathBuf::from("/a"), PathBuf::from("/b")]
        );
        assert_eq!(
            data_dirs_from(os(""), os(""), None),
            [PathBuf::from("/usr/local/share"), PathBuf::from("/usr/share")]
        );
    }

    #[test]
    fn categories() {
        for (ext, want) in [
            ("png", Category::Image),
            ("flac", Category::Audio),
            ("mkv", Category::Video),
            ("pdf", Category::Document),
            ("docx", Category::Document),
            ("odt", Category::Document),
            ("txt", Category::Document),
            ("md", Category::Document),
            ("zip", Category::Archive),
            ("rs", Category::Code),
            ("so", Category::Executable),
            ("appimage", Category::Executable),
            ("exe", Category::Executable),
            ("bin", Category::Executable),
            ("weird", Category::Other),
            ("", Category::Other),
        ] {
            assert_eq!(category(ext), want, "{ext}");
        }
    }
}
