//! Text terms: substring, wildcard and regex matchers, compiled once per query.

use everythux_core::text::fold;
use memchr::memmem::Finder;
use regex::bytes::{Regex, RegexBuilder};

use crate::query::QueryError;

/// What a term is matched against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// The file name.
    Name,
    /// The full path (`dir/name`). Used by `path:` and by the "match path" option.
    Path,
}

/// How a term matches text.
#[derive(Debug, Clone)]
pub(crate) enum Matcher {
    /// Substring.
    Sub(Box<Finder<'static>>),
    /// `*` / `?` pattern that must match the whole text.
    Glob(Glob),
    /// Regular expression (unanchored).
    Regex(Regex),
}

/// A compiled text term.
#[derive(Debug, Clone)]
pub(crate) struct Term {
    /// The matcher.
    pub matcher: Matcher,
    /// Matches against folded text (case-insensitive terms other than regexes).
    pub fold: bool,
    /// Name or full path.
    pub scope: Scope,
}

impl Term {
    /// Compiles a plain word or phrase: a wildcard pattern if it contains `*` or `?`, else a
    /// substring. Returns `None` when the text matches everything (only `*`s) so callers can drop it.
    pub(crate) fn text(text: &str, case_sensitive: bool, scope: Scope) -> Result<Option<Self>, QueryError> {
        let fold_it = !case_sensitive;
        let bytes = if fold_it {
            fold(text.as_bytes())
        } else {
            text.to_owned()
        }
        .into_bytes();
        let matcher = if bytes.iter().any(|&b| b == b'*' || b == b'?') {
            match Glob::compile(&bytes)? {
                GlobShape::Everything => return Ok(None),
                GlobShape::Substring(needle) => Matcher::Sub(Box::new(Finder::new(&needle).into_owned())),
                GlobShape::Glob(g) => Matcher::Glob(g),
            }
        } else {
            Matcher::Sub(Box::new(Finder::new(&bytes).into_owned()))
        };
        Ok(Some(Self {
            matcher,
            fold: fold_it,
            scope,
        }))
    }

    /// Compiles a regular expression term.
    pub(crate) fn regex(pattern: &str, case_sensitive: bool, scope: Scope) -> Result<Self, QueryError> {
        let re = RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|e| QueryError::Regex(e.to_string()))?;
        Ok(Self {
            matcher: Matcher::Regex(re),
            fold: false,
            scope,
        })
    }
}

/// A `*`/`?` pattern that matches a whole string.
#[derive(Debug, Clone)]
pub(crate) enum Glob {
    /// Only `*` wildcards: anchored prefix and suffix plus ordered middle literals.
    Stars(Box<StarGlob>),
    /// Contains `?`: compiled to an anchored regex.
    Regex(Regex),
}

/// Result of analysing a wildcard pattern.
enum GlobShape {
    /// Only stars.
    Everything,
    /// `*literal*`.
    Substring(Vec<u8>),
    /// General pattern.
    Glob(Glob),
}

/// Glob with only `*`: `prefix * middle[0] * ... * suffix`.
#[derive(Debug, Clone)]
pub(crate) struct StarGlob {
    prefix: Vec<u8>,
    suffix: Vec<u8>,
    middle: Vec<Finder<'static>>,
    /// No star at all: the text must equal `prefix`.
    exact: bool,
    min_len: usize,
    /// The longest literal of the pattern, for a bulk pre-scan.
    prefilter: Option<Finder<'static>>,
}

impl Glob {
    fn compile(pattern: &[u8]) -> Result<GlobShape, QueryError> {
        if pattern.contains(&b'?') {
            return Self::compile_regex(pattern).map(|g| GlobShape::Glob(Self::Regex(g)));
        }
        let parts: Vec<&[u8]> = pattern.split(|&b| b == b'*').collect();
        if parts.len() == 1 {
            return Ok(GlobShape::Glob(Self::Stars(Box::new(StarGlob {
                prefix: pattern.to_vec(),
                suffix: Vec::new(),
                middle: Vec::new(),
                exact: true,
                min_len: pattern.len(),
                prefilter: None,
            }))));
        }
        let (prefix, suffix) = (parts[0], parts[parts.len() - 1]);
        let middle: Vec<&[u8]> = parts[1..parts.len() - 1]
            .iter()
            .copied()
            .filter(|p| !p.is_empty())
            .collect();
        match (prefix.is_empty(), suffix.is_empty(), middle.as_slice()) {
            (true, true, []) => Ok(GlobShape::Everything),
            (true, true, [only]) => Ok(GlobShape::Substring(only.to_vec())),
            _ => {
                let longest = middle
                    .iter()
                    .copied()
                    .chain([prefix, suffix])
                    .max_by_key(|lit| lit.len());
                Ok(GlobShape::Glob(Self::Stars(Box::new(StarGlob {
                    prefix: prefix.to_vec(),
                    suffix: suffix.to_vec(),
                    min_len: prefix.len() + suffix.len() + middle.iter().map(|m| m.len()).sum::<usize>(),
                    middle: middle.iter().map(|m| Finder::new(m).into_owned()).collect(),
                    exact: false,
                    prefilter: longest
                        .filter(|lit| !lit.is_empty())
                        .map(|lit| Finder::new(lit).into_owned()),
                }))))
            }
        }
    }

    fn compile_regex(pattern: &[u8]) -> Result<Regex, QueryError> {
        let text = String::from_utf8_lossy(pattern);
        let mut re = String::with_capacity(text.len() * 2 + 8);
        re.push_str("^(?:");
        let mut buf = [0u8; 4];
        for ch in text.chars() {
            match ch {
                '*' => re.push_str("(?s-u:.)*"),
                '?' => re.push_str("(?s:.)"),
                c => re.push_str(&regex::escape(c.encode_utf8(&mut buf))),
            }
        }
        re.push_str(")$");
        Regex::new(&re).map_err(|e| QueryError::Regex(e.to_string()))
    }

    /// Whether the whole of `text` matches the pattern.
    pub(crate) fn is_match(&self, text: &[u8]) -> bool {
        match self {
            Self::Stars(g) => g.is_match(text),
            Self::Regex(re) => re.is_match(text),
        }
    }

    /// A literal every match must contain, usable to pre-filter with one bulk scan.
    pub(crate) fn prefilter(&self) -> Option<&Finder<'static>> {
        match self {
            Self::Stars(g) => g.prefilter.as_ref(),
            Self::Regex(_) => None,
        }
    }
}

impl StarGlob {
    fn is_match(&self, text: &[u8]) -> bool {
        if self.exact {
            return text == self.prefix;
        }
        if text.len() < self.min_len || !text.starts_with(&self.prefix) || !text.ends_with(&self.suffix) {
            return false;
        }
        let mut rest = &text[self.prefix.len()..text.len() - self.suffix.len()];
        for finder in &self.middle {
            match finder.find(rest) {
                Some(at) => rest = &rest[at + finder.needle().len()..],
                None => return false,
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glob_matches(pattern: &str, text: &str) -> bool {
        let term = Term::text(pattern, true, Scope::Name).unwrap().unwrap();
        match term.matcher {
            Matcher::Glob(g) => g.is_match(text.as_bytes()),
            Matcher::Sub(f) => f.find(text.as_bytes()).is_some(),
            Matcher::Regex(_) => unreachable!(),
        }
    }

    #[test]
    fn star_patterns_match_whole_names() {
        for (pattern, text, expected) in [
            ("*.txt", "a.txt", true),
            ("*.txt", "a.txt.bak", false),
            ("foo*", "foobar", true),
            ("foo*", "afoo", false),
            ("a*b*c", "a-b-c", true),
            ("a*b*c", "acb", false),
            ("a*b*c", "abc", true),
            ("a*a", "a", false),
            ("a*a", "aa", true),
            ("exact*", "exact", true),
            ("*a*b*", "xxaxxbxx", true),
            ("*a*b*", "xxbxxaxx", false),
        ] {
            assert_eq!(glob_matches(pattern, text), expected, "{pattern} vs {text}");
        }
    }

    #[test]
    fn question_mark_matches_one_character() {
        for (pattern, text, expected) in [
            ("?.txt", "a.txt", true),
            ("?.txt", "ab.txt", false),
            ("?.txt", "\u{e9}.txt", true),
            ("f?o*", "foobar", true),
            ("f?o*", "fo", false),
            ("a?b", "a.b", true),
            ("a.b?", "axbc", false),
        ] {
            assert_eq!(glob_matches(pattern, text), expected, "{pattern} vs {text}");
        }
    }

    #[test]
    fn only_stars_match_everything() {
        assert!(Term::text("***", true, Scope::Name).unwrap().is_none());
        assert!(Term::text("*", false, Scope::Name).unwrap().is_none());
    }

    #[test]
    fn star_literal_star_is_a_substring() {
        let term = Term::text("*Foo*", false, Scope::Name).unwrap().unwrap();
        assert!(matches!(&term.matcher, Matcher::Sub(f) if f.needle() == b"foo"));
    }

    #[test]
    fn invalid_regex_is_reported() {
        assert!(matches!(
            Term::regex("(", false, Scope::Name),
            Err(QueryError::Regex(_))
        ));
    }
}
