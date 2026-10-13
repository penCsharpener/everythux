//! Exclusion popup and editor: state and pure helpers (Phase 6).
//!
//! The behaviour that touches the app lives in `actions`; widgets are in `crate::ui::exclusion_ui`.

mod actions;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::Pos2;
use everythux_core::exclusion::{ExclusionRule, ExclusionSet, RuleKind};
use everythux_core::snapshot::IndexSnapshot;
use everythux_index::{Job, ScanMode};

use crate::backend::ExclusionPreview;

/// Which kind of rule the popup builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ExclusionChoice {
    /// A `Path` rule for exactly this folder (and everything below it).
    #[default]
    ThisFolder,
    /// A `Dir` rule matching the folder name anywhere.
    AnyNamed,
    /// A `Dir` rule matching the folder name, limited to a scope.
    AnyNamedUnder,
    /// A user-written glob.
    Custom,
}

/// The popup that opens after `Ctrl+Shift+Click` on a Path cell.
#[derive(Debug)]
pub(crate) struct ExclusionPopup {
    pub(crate) folder: PathBuf,
    /// Last path component, if the folder isn't the root.
    pub(crate) name: Option<String>,
    pub(crate) choice: ExclusionChoice,
    pub(crate) scope: String,
    pub(crate) custom: String,
    /// "Remove matching entries from the index now" (default) vs "Keep until next rescan".
    pub(crate) remove_now: bool,
    /// The rule this popup put into the config (replaced when the choice changes).
    pub(crate) current_rule: ExclusionRule,
    /// Validation error of the candidate rule, if any.
    pub(crate) error: Option<String>,
    pub(crate) preview: Option<ExclusionPreview>,
    pub(crate) preview_request: u64,
    /// Snapshot the preview is computed against.
    pub(crate) snapshot: Arc<IndexSnapshot>,
    /// Where the pointer was when the popup opened.
    pub(crate) anchor: Option<Pos2>,
}

impl ExclusionPopup {
    pub(crate) fn new(folder: PathBuf, snapshot: Arc<IndexSnapshot>, anchor: Option<Pos2>) -> Self {
        let name = folder.file_name().map(|n| n.to_string_lossy().into_owned());
        let mut popup = Self {
            scope: default_scope(&folder),
            custom: folder.to_string_lossy().into_owned(),
            name,
            choice: ExclusionChoice::ThisFolder,
            remove_now: true,
            current_rule: ExclusionRule::new("", RuleKind::Path),
            error: None,
            preview: None,
            preview_request: 0,
            snapshot,
            anchor,
            folder,
        };
        popup.current_rule = popup.candidate();
        popup
    }

    /// The rule described by the current choice and text fields.
    pub(crate) fn candidate(&self) -> ExclusionRule {
        let name = self.name.as_deref().unwrap_or_default();
        match self.choice {
            ExclusionChoice::ThisFolder => {
                ExclusionRule::new(escape_glob(&self.folder.to_string_lossy()), RuleKind::Path)
            }
            ExclusionChoice::AnyNamed => ExclusionRule::new(escape_glob(name), RuleKind::Dir),
            ExclusionChoice::AnyNamedUnder => {
                ExclusionRule::new(escape_glob(name), RuleKind::Dir).scoped(self.scope.trim())
            }
            ExclusionChoice::Custom => ExclusionRule::new(self.custom.trim(), RuleKind::Any),
        }
    }
}

/// Editor window state.
#[derive(Debug, Default)]
pub(crate) struct ExclusionEditor {
    pub(crate) open: bool,
    /// Rules as they were when the editor opened (or after the last re-index offer was handled);
    /// removed or changed rules are measured against this.
    pub(crate) baseline: Vec<ExclusionRule>,
    pub(crate) test_path: String,
    pub(crate) test_is_dir: bool,
    pub(crate) transfer_path: String,
    pub(crate) message: Option<String>,
    pub(crate) confirm_restore: bool,
}

/// Escapes glob metacharacters so a literal name or path can be used as a pattern.
pub(crate) fn escape_glob(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '*' | '?' | '[' | ']' | '{' | '}' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Reverses [`escape_glob`]. Returns `None` if the pattern contains an unescaped wildcard, i.e.
/// doesn't denote a single literal path.
pub(crate) fn unescape_glob(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => out.push(chars.next()?),
            '*' | '?' | '[' | ']' | '{' | '}' => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

/// Default scope for "any folder named X under ...": the home pattern `/home/*` for folders below
/// `/home`, otherwise the folder's grandparent.
pub(crate) fn default_scope(folder: &Path) -> String {
    if folder.starts_with("/home") && folder != Path::new("/home") {
        return "/home/*".to_owned();
    }
    folder
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| "/".to_owned(), |p| p.to_string_lossy().into_owned())
}

/// Why a rule can't be used, if it can't: empty patterns and patterns that don't compile.
pub(crate) fn rule_error(rule: &ExclusionRule) -> Option<String> {
    if rule.pattern.trim().is_empty() {
        return Some("The pattern is empty".to_owned());
    }
    rule.validate().err().map(|e| e.to_string())
}

/// Copies of `rules` where uncompilable rules are disabled, so indices stay aligned with the config
/// while the set can still be built.
pub(crate) fn compile_lenient(rules: &[ExclusionRule]) -> ExclusionSet {
    let usable: Vec<ExclusionRule> = rules
        .iter()
        .map(|r| {
            let mut r = r.clone();
            if rule_error(&r).is_some() {
                r.enabled = false;
            }
            r
        })
        .collect();
    ExclusionSet::compile(&usable).unwrap_or_default()
}

/// Why a path is excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Explanation {
    /// Index of the rule in the list.
    pub(crate) rule_index: usize,
    /// The path (the tested one or one of its ancestors) that the rule matched.
    pub(crate) matched: PathBuf,
}

/// Which rule excludes `path`, considering that an excluded ancestor folder prunes everything
/// below it. The shallowest match wins, as the walker would stop there.
pub(crate) fn explain(rules: &[ExclusionRule], path: &Path, is_dir: bool) -> Option<Explanation> {
    let set = compile_lenient(rules);
    let ancestors: Vec<&Path> = path.ancestors().skip(1).filter(|a| *a != Path::new("")).collect();
    for ancestor in ancestors.into_iter().rev() {
        if let Some(rule_index) = set.matching_rule(ancestor, true) {
            return Some(Explanation {
                rule_index,
                matched: ancestor.to_owned(),
            });
        }
    }
    set.matching_rule(path, is_dir).map(|rule_index| Explanation {
        rule_index,
        matched: path.to_owned(),
    })
}

/// Enabled rules present in `baseline` but no longer (unchanged and enabled) in `current`.
pub(crate) fn removed_rules(baseline: &[ExclusionRule], current: &[ExclusionRule]) -> Vec<ExclusionRule> {
    let mut remaining: Vec<&ExclusionRule> = current.iter().filter(|r| r.enabled).collect();
    let mut removed = Vec::new();
    for rule in baseline.iter().filter(|r| r.enabled) {
        if let Some(pos) = remaining.iter().position(|r| *r == rule) {
            remaining.swap_remove(pos);
        } else {
            removed.push(rule.clone());
        }
    }
    removed
}

/// Index jobs that bring back entries hidden by `removed` rules: `Path` rules re-index their
/// folder, anything else needs a full rescan of everything.
pub(crate) fn reindex_jobs(removed: &[ExclusionRule]) -> Vec<Job> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for rule in removed {
        let literal = (rule.kind == RuleKind::Path && !rule.pattern.starts_with("re:"))
            .then(|| unescape_glob(rule.pattern.trim()))
            .flatten();
        match literal {
            Some(p) => {
                let path = PathBuf::from(p.trim_end_matches('/'));
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
            None => {
                return vec![Job::RescanAll(ScanMode::Full)];
            }
        }
    }
    paths
        .into_iter()
        .map(|path| Job::RescanPath {
            path,
            mode: ScanMode::Full,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(p: &str, k: RuleKind) -> ExclusionRule {
        ExclusionRule::new(p, k)
    }

    #[test]
    fn escaping_roundtrips() {
        for s in ["plain", "a[b]c", "star*", "{x}", "back\\slash", "q?"] {
            assert_eq!(unescape_glob(&escape_glob(s)).as_deref(), Some(s), "{s}");
        }
        assert_eq!(unescape_glob("/home/*/x"), None);
    }

    #[test]
    fn escaped_names_match_literally() {
        let r = rule(&escape_glob("a[1]"), RuleKind::Dir);
        let set = ExclusionSet::compile(&[r]).expect("compiles");
        assert!(set.is_excluded(Path::new("/x/a[1]"), true));
        assert!(!set.is_excluded(Path::new("/x/a1"), true));
    }

    #[test]
    fn default_scope_prefers_home_pattern() {
        assert_eq!(default_scope(Path::new("/home/me/proj/bin")), "/home/*");
        assert_eq!(default_scope(Path::new("/srv/data/cache/tmp")), "/srv/data");
        assert_eq!(default_scope(Path::new("/data")), "/");
    }

    #[test]
    fn popup_builds_the_four_kinds_of_rules() {
        let snap = Arc::new(IndexSnapshot::empty());
        let mut popup = ExclusionPopup::new("/home/me/proj/node_modules".into(), snap, None);
        assert_eq!(
            popup.candidate(),
            rule("/home/me/proj/node_modules", RuleKind::Path)
        );
        popup.choice = ExclusionChoice::AnyNamed;
        assert_eq!(popup.candidate(), rule("node_modules", RuleKind::Dir));
        popup.choice = ExclusionChoice::AnyNamedUnder;
        assert_eq!(
            popup.candidate(),
            rule("node_modules", RuleKind::Dir).scoped("/home/*")
        );
        popup.choice = ExclusionChoice::Custom;
        popup.custom = "**/node_modules".to_owned();
        assert_eq!(popup.candidate(), rule("**/node_modules", RuleKind::Any));
    }

    #[test]
    fn explain_names_the_rule_and_the_excluded_ancestor() {
        let rules = [
            rule("node_modules", RuleKind::Dir),
            rule("bin", RuleKind::Dir).scoped("/home/*"),
        ];
        let e = explain(&rules, Path::new("/home/me/proj/bin/x"), false).expect("excluded");
        assert_eq!(e.rule_index, 1);
        assert_eq!(e.matched, PathBuf::from("/home/me/proj/bin"));
        assert_eq!(
            explain(&rules, Path::new("/usr/bin/ls"), false),
            None,
            "scope protects /usr/bin"
        );
        let e = explain(&rules, Path::new("/a/node_modules"), true).expect("excluded");
        assert_eq!(e.rule_index, 0);
        assert_eq!(e.matched, PathBuf::from("/a/node_modules"));
    }

    #[test]
    fn explain_ignores_invalid_and_disabled_rules() {
        let mut off = rule("tmp", RuleKind::Dir);
        off.enabled = false;
        let rules = [rule("[", RuleKind::Any), off, rule("cache", RuleKind::Dir)];
        assert_eq!(explain(&rules, Path::new("/x/tmp"), true), None);
        assert_eq!(
            explain(&rules, Path::new("/x/cache"), true).map(|e| e.rule_index),
            Some(2)
        );
    }

    #[test]
    fn removed_rules_compares_against_the_baseline() {
        let a = rule("a", RuleKind::Dir);
        let b = rule("/b", RuleKind::Path);
        let mut b_off = b.clone();
        b_off.enabled = false;
        assert!(
            removed_rules(&[a.clone(), b.clone()], &[a.clone(), b.clone()]).is_empty(),
            "{:?}",
            removed_rules(&[a.clone(), b.clone()], &[a.clone(), b.clone()])
        );
        assert_eq!(
            removed_rules(&[a.clone(), b.clone()], std::slice::from_ref(&a)),
            vec![b.clone()]
        );
        assert_eq!(
            removed_rules(&[a.clone(), b.clone()], &[a.clone(), b_off]),
            vec![b.clone()]
        );
        let mut a_edited = a.clone();
        a_edited.pattern = "aa".into();
        assert_eq!(removed_rules(std::slice::from_ref(&a), &[a_edited]), vec![a]);
        assert!(
            removed_rules(&[], std::slice::from_ref(&b)).is_empty(),
            "added rules need no re-index"
        );
    }

    #[test]
    fn reindex_jobs_for_path_rules_and_others() {
        assert_eq!(
            reindex_jobs(&[
                rule("/home/me/old", RuleKind::Path),
                rule("/home/me/old", RuleKind::Path)
            ]),
            vec![Job::RescanPath {
                path: "/home/me/old".into(),
                mode: ScanMode::Full
            }]
        );
        assert_eq!(
            reindex_jobs(&[
                rule("/home/me/old", RuleKind::Path),
                rule("node_modules", RuleKind::Dir)
            ]),
            vec![Job::RescanAll(ScanMode::Full)]
        );
        assert_eq!(
            reindex_jobs(&[rule("/home/*/old", RuleKind::Path)]),
            vec![Job::RescanAll(ScanMode::Full)],
            "a path glob can't be rescanned as one folder"
        );
        assert!(reindex_jobs(&[]).is_empty(), "{:?}", reindex_jobs(&[]));
        assert_eq!(
            reindex_jobs(&[rule(&escape_glob("/data/a[1]"), RuleKind::Path)]),
            vec![Job::RescanPath {
                path: "/data/a[1]".into(),
                mode: ScanMode::Full
            }]
        );
    }
}
