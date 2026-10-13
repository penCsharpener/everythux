//! Exclusion rules (ROADMAP Phase 6) and their compiled matcher.
//!
//! # Rule semantics
//!
//! - `pattern` is a glob (`globset` syntax, case-sensitive, `*` doesn't cross `/`, `**` does) or, with
//!   a `re:` prefix, a regular expression.
//! - A pattern **without** `/` is matched against the entry's **name** (final component).
//!   A pattern **with** `/` is matched against the entry's **absolute path**.
//! - [`RuleKind::Dir`] applies only to folders, [`RuleKind::File`] only to non-folders,
//!   [`RuleKind::Any`] to both.
//! - [`RuleKind::Path`]: `pattern` is an absolute path (or path glob); it excludes that path and
//!   everything below it.
//! - `scope` (optional glob over absolute paths): the rule only applies to entries whose absolute
//!   path matches `<scope>/**`. E.g. `bin` with scope `/home/*` excludes `/home/me/proj/bin` but
//!   not `/usr/bin`.
//! - Disabled rules are ignored.
//!
//! Because the walker prunes excluded folders before descending, matching a folder is enough to
//! drop its whole subtree.

use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;

use globset::{Candidate, Glob, GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use regex::bytes::RegexSet;
use serde::{Deserialize, Serialize};

/// What a rule applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// Folders only.
    Dir,
    /// Non-folders only.
    File,
    /// An exact path (or path glob) and everything below it.
    Path,
    /// Folders and files.
    #[default]
    Any,
}

/// One exclusion rule as stored in `config.json`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExclusionRule {
    /// Glob, or `re:<regex>`.
    pub pattern: String,
    /// What it applies to.
    #[serde(default)]
    pub kind: RuleKind,
    /// Optional path glob limiting where the rule applies.
    #[serde(default)]
    pub scope: Option<String>,
    /// Whether the rule is active.
    #[serde(default = "enabled_default")]
    pub enabled: bool,
}

const fn enabled_default() -> bool {
    true
}

impl ExclusionRule {
    /// A rule with no scope, enabled.
    pub fn new(pattern: impl Into<String>, kind: RuleKind) -> Self {
        Self {
            pattern: pattern.into(),
            kind,
            scope: None,
            enabled: true,
        }
    }

    /// Builder: limit the rule to `scope`.
    #[must_use]
    pub fn scoped(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    /// Checks that the pattern and scope compile (for live feedback in the editor).
    pub fn validate(&self) -> Result<(), ExclusionError> {
        ExclusionSet::compile(std::slice::from_ref(self)).map(|_| ())
    }
}

/// The rules shipped by default. `obj`, `bin` and `.var` are scoped to home directories, because
/// excluding `bin` globally would drop `/usr/bin`.
pub fn default_rules() -> Vec<ExclusionRule> {
    vec![
        ExclusionRule::new("node_modules", RuleKind::Dir),
        ExclusionRule::new(".var", RuleKind::Dir).scoped("/home/*"),
        ExclusionRule::new(".cache", RuleKind::Dir).scoped("/home/*"),
        ExclusionRule::new("__pycache__", RuleKind::Dir),
        ExclusionRule::new(".venv", RuleKind::Dir),
        ExclusionRule::new("target", RuleKind::Dir).scoped("/home/*"),
        ExclusionRule::new("obj", RuleKind::Dir).scoped("/home/*"),
        ExclusionRule::new("bin", RuleKind::Dir).scoped("/home/*"),
        ExclusionRule::new("/proc", RuleKind::Path),
        ExclusionRule::new("/sys", RuleKind::Path),
        ExclusionRule::new("/dev", RuleKind::Path),
        ExclusionRule::new("/run", RuleKind::Path),
        ExclusionRule::new("/tmp", RuleKind::Path),
        ExclusionRule::new("/var/lib/docker", RuleKind::Path),
        ExclusionRule::new("/var/lib/containers", RuleKind::Path),
        ExclusionRule::new("lost+found", RuleKind::Dir),
    ]
}

/// A rule that failed to compile.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ExclusionError {
    /// Invalid glob in `pattern` or `scope`.
    #[error("invalid glob {glob:?}: {message}")]
    Glob {
        /// The offending glob.
        glob: String,
        /// Parser message.
        message: String,
    },
    /// Invalid regular expression after `re:`.
    #[error("invalid regex {regex:?}: {message}")]
    Regex {
        /// The offending regex.
        regex: String,
        /// Parser message.
        message: String,
    },
    /// A [`RuleKind::Path`] pattern that isn't absolute.
    #[error("path rule {0:?} must be an absolute path")]
    RelativePath(String),
}

/// All enabled rules compiled for fast matching (one `GlobSet` / `RegexSet` per match target).
///
/// Cheap to clone (shares compiled matchers) and `Send + Sync`, so walker threads can share it.
#[derive(Debug, Clone, Default)]
pub struct ExclusionSet {
    inner: Arc<Compiled>,
}

#[derive(Debug, Default)]
struct Compiled {
    /// Globs matched against the entry name; `name_owner[i]` is the rule of glob `i`.
    name_globs: GlobSet,
    name_owner: Vec<usize>,
    /// Globs matched against the absolute path.
    path_globs: GlobSet,
    path_owner: Vec<usize>,
    /// Regexes matched against the name / the absolute path.
    name_regex: Option<RegexSet>,
    name_regex_owner: Vec<usize>,
    path_regex: Option<RegexSet>,
    path_regex_owner: Vec<usize>,
    /// Per original rule index: its kind and compiled scope (if any). `None` for disabled rules.
    rules: Vec<Option<CompiledRule>>,
}

#[derive(Debug)]
struct CompiledRule {
    kind: RuleKind,
    scope: Option<GlobMatcher>,
}

impl CompiledRule {
    fn applies(&self, path: &Path, is_dir: bool) -> bool {
        let kind_ok = match self.kind {
            RuleKind::Dir => is_dir,
            RuleKind::File => !is_dir,
            RuleKind::Path | RuleKind::Any => true,
        };
        kind_ok && self.scope.as_ref().is_none_or(|s| s.is_match(path))
    }
}

fn glob(pattern: &str) -> Result<Glob, ExclusionError> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .backslash_escape(true)
        .build()
        .map_err(|e| ExclusionError::Glob {
            glob: pattern.to_owned(),
            message: e.kind().to_string(),
        })
}

fn regex_set(patterns: &[String]) -> Result<Option<RegexSet>, ExclusionError> {
    if patterns.is_empty() {
        return Ok(None);
    }
    RegexSet::new(patterns).map(Some).map_err(|e| {
        // Report the first pattern that fails on its own.
        let bad = patterns
            .iter()
            .find(|p| regex::bytes::Regex::new(p).is_err())
            .cloned()
            .unwrap_or_default();
        ExclusionError::Regex {
            regex: bad,
            message: e.to_string(),
        }
    })
}

impl ExclusionSet {
    /// A set that excludes nothing.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Compiles the enabled rules. Errors name the first invalid rule.
    pub fn compile(rules: &[ExclusionRule]) -> Result<Self, ExclusionError> {
        let mut name_globs = GlobSetBuilder::new();
        let mut path_globs = GlobSetBuilder::new();
        let mut c = Compiled::default();
        let (mut name_res, mut path_res) = (Vec::new(), Vec::new());

        for (idx, rule) in rules.iter().enumerate() {
            if !rule.enabled {
                c.rules.push(None);
                continue;
            }
            let scope = match rule.scope.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(s) => Some(glob(&format!("{}/**", s.trim_end_matches('/')))?.compile_matcher()),
                None => None,
            };
            let pattern = rule.pattern.trim();
            if let Some(re) = pattern.strip_prefix("re:") {
                if rule.kind == RuleKind::Path {
                    path_res.push(re.to_owned());
                    c.path_regex_owner.push(idx);
                } else {
                    name_res.push(re.to_owned());
                    c.name_regex_owner.push(idx);
                }
            } else if rule.kind == RuleKind::Path {
                if !pattern.starts_with('/') {
                    return Err(ExclusionError::RelativePath(pattern.to_owned()));
                }
                let base = if pattern == "/" {
                    ""
                } else {
                    pattern.trim_end_matches('/')
                };
                if !base.is_empty() {
                    path_globs.add(glob(base)?);
                    c.path_owner.push(idx);
                }
                path_globs.add(glob(&format!("{base}/**"))?);
                c.path_owner.push(idx);
            } else if pattern.contains('/') {
                path_globs.add(glob(pattern.trim_end_matches('/'))?);
                c.path_owner.push(idx);
            } else {
                name_globs.add(glob(pattern)?);
                c.name_owner.push(idx);
            }
            c.rules.push(Some(CompiledRule {
                kind: rule.kind,
                scope,
            }));
        }

        let build = |b: GlobSetBuilder| {
            b.build().map_err(|e| ExclusionError::Glob {
                glob: String::new(),
                message: e.to_string(),
            })
        };
        c.name_globs = build(name_globs)?;
        c.path_globs = build(path_globs)?;
        c.name_regex = regex_set(&name_res)?;
        c.path_regex = regex_set(&path_res)?;
        Ok(Self { inner: Arc::new(c) })
    }

    /// Whether no enabled rules are present.
    pub fn is_empty(&self) -> bool {
        self.inner.rules.iter().all(Option::is_none)
    }

    /// Whether the entry at absolute `path` is excluded. `is_dir` says whether it's a folder.
    ///
    /// Hot path: called for every entry during a scan. Doesn't allocate unless some glob matches.
    pub fn is_excluded(&self, path: &Path, is_dir: bool) -> bool {
        self.matching_rule(path, is_dir).is_some()
    }

    /// Index (into the slice given to [`Self::compile`]) of the first rule excluding `path`.
    pub fn matching_rule(&self, path: &Path, is_dir: bool) -> Option<usize> {
        let c = &*self.inner;
        let name = path.file_name().unwrap_or(path.as_os_str());
        let applies = |rule: usize| c.rules[rule].as_ref().is_some_and(|r| r.applies(path, is_dir));
        let mut best: Option<usize> = None;
        let mut consider = |rule: usize| {
            if best.is_none_or(|b| rule < b) && applies(rule) {
                best = Some(rule);
            }
        };

        if !c.name_globs.is_empty() {
            let cand = Candidate::new(name);
            if c.name_globs.is_match_candidate(&cand) {
                for g in c.name_globs.matches_candidate(&cand) {
                    consider(c.name_owner[g]);
                }
            }
        }
        if !c.path_globs.is_empty() {
            let cand = Candidate::new(path);
            if c.path_globs.is_match_candidate(&cand) {
                for g in c.path_globs.matches_candidate(&cand) {
                    consider(c.path_owner[g]);
                }
            }
        }
        if let Some(set) = &c.name_regex {
            let bytes = name.as_bytes();
            if set.is_match(bytes) {
                for r in &set.matches(bytes) {
                    consider(c.name_regex_owner[r]);
                }
            }
        }
        if let Some(set) = &c.path_regex {
            let bytes = path.as_os_str().as_bytes();
            if set.is_match(bytes) {
                for r in &set.matches(bytes) {
                    consider(c.path_regex_owner[r]);
                }
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(rules: &[ExclusionRule]) -> ExclusionSet {
        ExclusionSet::compile(rules).unwrap()
    }

    #[test]
    fn name_dir_rules_and_scopes() {
        let s = set(&[
            ExclusionRule::new("node_modules", RuleKind::Dir),
            ExclusionRule::new("bin", RuleKind::Dir).scoped("/home/*"),
        ]);
        for (path, is_dir, excluded) in [
            ("/home/a/proj/node_modules", true, true),
            ("/opt/x/node_modules", true, true),
            ("/home/a/node_modules", false, false), // a file named node_modules
            ("/home/a/node_modules.txt", false, false),
            ("/home/a/proj/bin", true, true),
            ("/usr/bin", true, false),  // scoping protects system dirs
            ("/home/bin", true, false), // `/home/*/**` needs a component below /home
        ] {
            assert_eq!(s.is_excluded(Path::new(path), is_dir), excluded, "{path}");
        }
    }

    #[test]
    fn path_rules_cover_subtree() {
        let s = set(&[ExclusionRule::new("/home/me/Downloads/old/", RuleKind::Path)]);
        assert!(s.is_excluded(Path::new("/home/me/Downloads/old"), true));
        assert!(s.is_excluded(Path::new("/home/me/Downloads/old/a/b.txt"), false));
        assert!(!s.is_excluded(Path::new("/home/me/Downloads/older"), true));
        assert!(matches!(
            ExclusionSet::compile(&[ExclusionRule::new("rel/path", RuleKind::Path)]),
            Err(ExclusionError::RelativePath(_))
        ));
    }

    #[test]
    fn file_globs_regex_and_path_globs() {
        let s = set(&[
            ExclusionRule::new("*.tmp", RuleKind::File),
            ExclusionRule::new(r"re:^\.cache-[0-9]+$", RuleKind::Dir),
            ExclusionRule::new("/home/*/.local/share/Trash", RuleKind::Any),
        ]);
        assert!(s.is_excluded(Path::new("/x/a.tmp"), false));
        assert!(!s.is_excluded(Path::new("/x/a.tmp"), true));
        assert!(s.is_excluded(Path::new("/x/.cache-42"), true));
        assert!(!s.is_excluded(Path::new("/x/.cache-x"), true));
        assert!(s.is_excluded(Path::new("/home/me/.local/share/Trash"), true));
        assert!(!s.is_excluded(Path::new("/home/me/x/.local/share/Trash"), true));
    }

    #[test]
    fn disabled_rules_and_rule_indices() {
        let mut off = ExclusionRule::new("a", RuleKind::Any);
        off.enabled = false;
        let s = set(&[
            off,
            ExclusionRule::new("b", RuleKind::Any),
            ExclusionRule::new("*", RuleKind::File),
        ]);
        assert_eq!(s.matching_rule(Path::new("/x/a"), true), None);
        assert_eq!(s.matching_rule(Path::new("/x/b"), false), Some(1));
        assert_eq!(s.matching_rule(Path::new("/x/c"), false), Some(2));
        assert!(!s.is_empty());
        assert!(ExclusionSet::empty().is_empty());
        assert!(!ExclusionSet::empty().is_excluded(Path::new("/x"), true));
    }

    #[test]
    fn invalid_utf8_names_match_bytewise() {
        use std::ffi::OsStr;
        let s = set(&[ExclusionRule::new(r"re:^bad\xFF", RuleKind::Any)]);
        let p = Path::new("/x").join(OsStr::from_bytes(b"bad\xffname"));
        // `\xFF` in a bytes regex with Unicode on means U+00FF; disable unicode for raw bytes.
        let s2 = set(&[ExclusionRule::new(r"re:(?-u)^bad\xFF", RuleKind::Any)]);
        assert!(!s.is_excluded(&p, false) || s2.is_excluded(&p, false));
        assert!(s2.is_excluded(&p, false));
    }

    #[test]
    fn defaults_compile_and_keep_system_dirs() {
        let s = set(&default_rules());
        assert!(!s.is_excluded(Path::new("/usr/bin"), true));
        assert!(!s.is_excluded(Path::new("/usr/lib/obj"), true));
        assert!(s.is_excluded(Path::new("/home/me/.var"), true));
        assert!(s.is_excluded(Path::new("/home/me/src/app/node_modules"), true));
        assert!(s.is_excluded(Path::new("/proc/1/status"), false));
    }

    #[test]
    fn validate_reports_bad_patterns() {
        assert!(ExclusionRule::new("[", RuleKind::Any).validate().is_err());
        assert!(ExclusionRule::new("re:(", RuleKind::Any).validate().is_err());
        assert!(ExclusionRule::new("ok*", RuleKind::Any).validate().is_ok());
    }
}
