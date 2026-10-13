//! Which provider handles a file, decided by its lowercase extension.

/// Provider family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Image,
    Text,
    Pdf,
    Audio,
    Video,
    /// `.ts` is TypeScript source or an MPEG transport stream; the content decides.
    TypeScriptOrVideo,
    /// Unknown extension: thumbnail cache, then content sniffing for text.
    Other,
}

pub(crate) fn classify(ext: Option<&str>) -> Kind {
    let Some(ext) = ext else {
        return Kind::Other;
    };
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "ico" => Kind::Image,
        "pdf" => Kind::Pdf,
        "mp3" | "flac" | "ogg" | "opus" | "m4a" | "aac" | "wav" | "wma" | "aiff" | "aif" => Kind::Audio,
        "mp4" | "mkv" | "webm" | "avi" | "mov" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg" => Kind::Video,
        "ts" => Kind::TypeScriptOrVideo,
        _ if is_text_extension(ext) => Kind::Text,
        _ => Kind::Other,
    }
}

/// Extensions that are shown as text without sniffing the content.
const TEXT_EXTENSIONS: &[&str] = &[
    "txt",
    "text",
    "md",
    "markdown",
    "rst",
    "adoc",
    "org",
    "tex",
    "rs",
    "toml",
    "lock",
    "json",
    "jsonc",
    "json5",
    "yaml",
    "yml",
    "xml",
    "html",
    "htm",
    "xhtml",
    "svg",
    "css",
    "scss",
    "sass",
    "less",
    "js",
    "mjs",
    "cjs",
    "jsx",
    "tsx",
    "vue",
    "svelte",
    "py",
    "pyi",
    "rb",
    "php",
    "pl",
    "pm",
    "lua",
    "r",
    "jl",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "bat",
    "c",
    "h",
    "cc",
    "cpp",
    "cxx",
    "hpp",
    "hh",
    "hxx",
    "m",
    "mm",
    "java",
    "kt",
    "kts",
    "scala",
    "groovy",
    "gradle",
    "cs",
    "fs",
    "vb",
    "swift",
    "dart",
    "go",
    "mod",
    "sum",
    "zig",
    "nim",
    "hs",
    "ml",
    "mli",
    "ex",
    "exs",
    "erl",
    "clj",
    "lisp",
    "el",
    "asm",
    "s",
    "d",
    "v",
    "vhd",
    "vhdl",
    "ini",
    "conf",
    "cfg",
    "config",
    "properties",
    "env",
    "desktop",
    "service",
    "rules",
    "log",
    "csv",
    "tsv",
    "sql",
    "diff",
    "patch",
    "cmake",
    "make",
    "mk",
    "ninja",
    "proto",
    "graphql",
    "gql",
    "tf",
    "nix",
    "srt",
    "vtt",
    "ass",
    "ics",
    "vcf",
    "gitignore",
    "gitattributes",
    "editorconfig",
];

fn is_text_extension(ext: &str) -> bool {
    TEXT_EXTENSIONS.contains(&ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension() {
        for (ext, kind) in [
            (Some("png"), Kind::Image),
            (Some("tif"), Kind::Image),
            (Some("pdf"), Kind::Pdf),
            (Some("flac"), Kind::Audio),
            (Some("mkv"), Kind::Video),
            (Some("ts"), Kind::TypeScriptOrVideo),
            (Some("rs"), Kind::Text),
            (Some("md"), Kind::Text),
            (Some("tsv"), Kind::Text),
            (Some("xyz"), Kind::Other),
            (None, Kind::Other),
        ] {
            assert_eq!(classify(ext), kind, "{ext:?}");
        }
    }
}
