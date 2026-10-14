//! A small reader for freedesktop `.desktop` / `.nemo_action` style INI files.
//!
//! Only what the context menu needs: groups, string/boolean/list values and localized keys.
//! Values are unescaped (`\s`, `\n`, `\t`, `\r`, `\\`); `Exec` quoting is handled by [`crate::exec`].

use std::path::Path;

/// One `[Group]` and its `key=value` pairs, in file order.
#[derive(Debug, Clone, Default)]
pub(crate) struct Group {
    pub(crate) name: String,
    entries: Vec<(String, String)>,
}

/// A parsed file.
#[derive(Debug, Clone, Default)]
pub(crate) struct DesktopFile {
    groups: Vec<Group>,
}

impl DesktopFile {
    pub(crate) fn parse(text: &str) -> Self {
        let mut groups: Vec<Group> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                groups.push(Group {
                    name: name.to_owned(),
                    entries: Vec::new(),
                });
            } else if let Some((key, value)) = line.split_once('=')
                && let Some(group) = groups.last_mut()
            {
                group
                    .entries
                    .push((key.trim().to_owned(), unescape(value.trim_start())));
            }
        }
        Self { groups }
    }

    /// Reads and parses `path`; unreadable or non-UTF-8 files yield `None`.
    pub(crate) fn read(path: &Path) -> Option<Self> {
        std::fs::read_to_string(path).ok().map(|text| Self::parse(&text))
    }

    pub(crate) fn group(&self, name: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.name == name)
    }
}

impl Group {
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// All `key`, `value` pairs in file order.
    pub(crate) fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// `Key[lang]` for the first matching locale candidate, else plain `Key`.
    pub(crate) fn localized(&self, key: &str, locales: &[String]) -> Option<&str> {
        locales
            .iter()
            .find_map(|l| self.get(&format!("{key}[{l}]")))
            .or_else(|| self.get(key))
    }

    pub(crate) fn flag(&self, key: &str) -> bool {
        self.get(key).is_some_and(|v| v == "true" || v == "1")
    }

    /// A `;`-separated list (`\;` escapes a literal semicolon). Empty items are dropped.
    pub(crate) fn list(&self, key: &str) -> Vec<String> {
        let Some(value) = self.get(key) else {
            return Vec::new();
        };
        let mut items = Vec::new();
        let mut current = String::new();
        let mut chars = value.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if chars.peek() == Some(&';') => {
                    current.push(';');
                    chars.next();
                }
                ';' => {
                    if !current.trim().is_empty() {
                        items.push(current.trim().to_owned());
                    }
                    current.clear();
                }
                other => current.push(other),
            }
        }
        if !current.trim().is_empty() {
            items.push(current.trim().to_owned());
        }
        items
    }
}

fn unescape(value: &str) -> String {
    if !value.contains('\\') {
        return value.to_owned();
    }
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') | None => out.push('\\'),
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            // Not a defined escape: keep it (the `\;` list escape is resolved by `Group::list`).
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

/// Locale names to try for `Key[...]`, most specific first: `de_DE@euro`, `de_DE`, `de@euro`, `de`.
pub(crate) fn locale_candidates(lang: &str) -> Vec<String> {
    let lang = lang.split('.').next().unwrap_or("");
    if lang.is_empty() || lang == "C" || lang == "POSIX" {
        return Vec::new();
    }
    let (rest, modifier) = match lang.split_once('@') {
        Some((rest, m)) => (rest, Some(m)),
        None => (lang, None),
    };
    let (language, country) = match rest.split_once('_') {
        Some((l, c)) => (l, Some(c)),
        None => (rest, None),
    };
    let mut out = Vec::new();
    if let (Some(c), Some(m)) = (country, modifier) {
        out.push(format!("{language}_{c}@{m}"));
    }
    if let Some(c) = country {
        out.push(format!("{language}_{c}"));
    }
    if let Some(m) = modifier {
        out.push(format!("{language}@{m}"));
    }
    out.push(language.to_owned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# comment
[Desktop Entry]
Name=Open in Kate
Name[de]=In Kate öffnen
Exec=kate %U
MimeType=text/plain;text/x-rust;
Terminal=false
Comment=a\\sb\\;c
";

    #[test]
    fn parses_groups_values_and_lists() {
        let file = DesktopFile::parse(SAMPLE);
        let entry = file.group("Desktop Entry").expect("group");
        assert_eq!(entry.get("Exec"), Some("kate %U"));
        assert_eq!(entry.list("MimeType"), ["text/plain", "text/x-rust"]);
        assert!(!entry.flag("Terminal"));
        assert_eq!(entry.get("Comment"), Some("a b\\;c"));
        assert!(file.group("Nope").is_none());
    }

    #[test]
    fn localized_names_prefer_the_most_specific_locale() {
        let file = DesktopFile::parse(SAMPLE);
        let entry = file.group("Desktop Entry").expect("group");
        assert_eq!(
            entry.localized("Name", &locale_candidates("de_DE.UTF-8")),
            Some("In Kate öffnen")
        );
        assert_eq!(
            entry.localized("Name", &locale_candidates("fr_FR")),
            Some("Open in Kate")
        );
        assert_eq!(entry.localized("Name", &[]), Some("Open in Kate"));
    }

    #[test]
    fn locale_candidates_cover_country_and_modifier() {
        assert_eq!(
            locale_candidates("sr_RS@latin.UTF-8"),
            ["sr_RS@latin", "sr_RS", "sr@latin", "sr"]
        );
        assert_eq!(locale_candidates("C"), Vec::<String>::new());
    }

    #[test]
    fn keys_before_any_group_are_ignored() {
        let file = DesktopFile::parse("Name=x\n[A]\nk=v\n");
        assert_eq!(file.group("A").and_then(|g| g.get("k")), Some("v"));
        assert!(file.group("A").and_then(|g| g.get("Name")).is_none());
    }
}
