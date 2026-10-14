//! Which provider handles a file, decided by its name (a few well-known text files have no
//! extension) and then by its lowercase extension.

use everythux_core::text::extension;

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

/// Classifies a file by its name (the last path component, as bytes).
pub(crate) fn classify_file(name: &[u8]) -> Kind {
    if is_known_text_name(name) {
        return Kind::Text;
    }
    classify(extension(name).as_deref())
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
    // .NET, build and infrastructure files.
    "csproj",
    "fsproj",
    "vbproj",
    "vcxproj",
    "proj",
    "sln",
    "slnx",
    "props",
    "targets",
    "xaml",
    "axaml",
    "razor",
    "cshtml",
    "resx",
    "nuspec",
    "ruleset",
    "runsettings",
    "manifest",
    "http",
    "bicep",
    "hcl",
    "tfvars",
    "dockerfile",
    "containerfile",
    "jenkinsfile",
    "gemspec",
    "podspec",
    "plist",
    "reg",
    "psm1",
    "psd1",
    "cmd",
    "awk",
    "sed",
    "nfo",
    "readme",
];

/// Whole file names (compared case-insensitively) that are text without a telling extension.
/// Dot files are here too: `.bashrc` has no extension in the `foo.ext` sense.
const KNOWN_TEXT_FILENAMES: &[&str] = &[
    "dockerfile",
    "containerfile",
    "jenkinsfile",
    "vagrantfile",
    "gemfile",
    "rakefile",
    "podfile",
    "brewfile",
    "procfile",
    "justfile",
    "makefile",
    "gnumakefile",
    "license",
    "licence",
    "readme",
    "changelog",
    "changes",
    "authors",
    "contributors",
    "copying",
    "notice",
    "install",
    "todo",
    "news",
    "codeowners",
    ".gitignore",
    ".gitattributes",
    ".gitmodules",
    ".gitconfig",
    ".editorconfig",
    ".dockerignore",
    ".env",
    ".bashrc",
    ".bash_profile",
    ".bash_aliases",
    ".profile",
    ".zshrc",
    ".zprofile",
    ".vimrc",
    ".inputrc",
    ".npmrc",
    ".prettierrc",
    ".eslintrc",
];

fn is_known_text_name(name: &[u8]) -> bool {
    KNOWN_TEXT_FILENAMES
        .iter()
        .any(|known| name.eq_ignore_ascii_case(known.as_bytes()))
}

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

    #[test]
    fn dotnet_and_build_extensions_are_text() {
        for ext in [
            "csproj",
            "fsproj",
            "vbproj",
            "sln",
            "slnx",
            "props",
            "targets",
            "xaml",
            "axaml",
            "razor",
            "cshtml",
            "resx",
            "nuspec",
            "ruleset",
            "runsettings",
            "http",
            "bicep",
            "hcl",
            "dockerfile",
            "containerfile",
            "jenkinsfile",
        ] {
            assert_eq!(classify(Some(ext)), Kind::Text, "{ext}");
            assert_eq!(
                classify_file(format!("App.{}", ext.to_uppercase()).as_bytes()),
                Kind::Text,
                "{ext}"
            );
        }
    }

    #[test]
    fn known_file_names_are_text() {
        for name in [
            "Dockerfile",
            "Containerfile",
            "Jenkinsfile",
            "Vagrantfile",
            "Gemfile",
            "Rakefile",
            "LICENSE",
            "README",
            "CHANGELOG",
            "AUTHORS",
            "COPYING",
            "makefile",
            ".bashrc",
            ".gitignore",
        ] {
            assert_eq!(classify_file(name.as_bytes()), Kind::Text, "{name}");
        }
        assert_eq!(classify_file(b"Dockerfiles"), Kind::Other, "whole names only");
        assert_eq!(classify_file(b"photo.png"), Kind::Image);
        assert_eq!(classify_file(b"bad\xffname"), Kind::Other);
    }
}
