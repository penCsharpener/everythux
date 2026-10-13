//! Property tests: the parser never panics, and parse + match agrees with a naive reference.

use std::sync::Arc;

use everythux_core::config::ColumnId;
use everythux_core::ids::{DirId, EntryId};
use everythux_search::synthetic::SnapshotBuilder;
use everythux_search::{QueryOptions, SearchIndex, Sort, parse};
use proptest::prelude::*;

fn options(case_sensitive: bool, match_path: bool, regex: bool) -> QueryOptions {
    QueryOptions {
        case_sensitive,
        match_path,
        regex,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn parser_never_panics_on_arbitrary_text(input in any::<String>(), cs in any::<bool>(), mp in any::<bool>(), re in any::<bool>()) {
        let _ = parse(&input, options(cs, mp, re));
    }

    #[test]
    fn parser_never_panics_on_syntax_soup(
        input in r#"[a-zA-Z0-9:*?!|"\\.<>=;, ()\[\]^$+-]{0,48}"#,
        cs in any::<bool>(),
        mp in any::<bool>(),
        re in any::<bool>(),
    ) {
        let _ = parse(&input, options(cs, mp, re));
    }

    #[test]
    fn parser_never_panics_on_filters(
        name in prop::sample::select(vec!["ext", "size", "dm", "path", "file", "folder", "regex", "case", "vol", "EXT", "Size"]),
        value in r#"[a-zA-Z0-9:*?!|"\\.<>=;, ()-]{0,24}"#,
    ) {
        let _ = parse(&format!("{name}:{value}"), QueryOptions::default());
        let _ = parse(&format!("!{name}:{value} {name}:{value}|{value}"), QueryOptions::default());
    }
}

/// A query as data, so the reference implementation doesn't need a parser.
#[derive(Debug, Clone)]
struct SimpleQuery {
    /// AND of ORs of (negated, term).
    groups: Vec<Vec<(bool, String)>>,
}

impl SimpleQuery {
    fn render(&self) -> String {
        self.groups
            .iter()
            .map(|alts| {
                alts.iter()
                    .map(|(neg, term)| format!("{}{term}", if *neg { "!" } else { "" }))
                    .collect::<Vec<_>>()
                    .join("|")
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn matches(&self, name: &str, case_sensitive: bool) -> bool {
        let hay = if case_sensitive {
            name.to_owned()
        } else {
            name.to_lowercase()
        };
        self.groups.iter().all(|alts| {
            alts.iter().any(|(neg, term)| {
                let needle = if case_sensitive {
                    term.clone()
                } else {
                    term.to_lowercase()
                };
                hay.contains(&needle) != *neg
            })
        })
    }
}

fn simple_query() -> impl Strategy<Value = SimpleQuery> {
    let term = "[a-cA-C._]{1,3}";
    let alt = (any::<bool>(), term);
    prop::collection::vec(prop::collection::vec(alt, 1..4), 1..4).prop_map(|groups| SimpleQuery { groups })
}

fn snapshot_of(names: &[String]) -> SearchIndex {
    let mut b = SnapshotBuilder::new();
    let v = b.volume("/", None);
    // add_dir creates no folder entry, so the snapshot holds exactly `names`
    let root = b.add_dir(b"/", DirId::NONE, v);
    let sub = b.add_dir(b"/sub", root, v);
    for (i, n) in names.iter().enumerate() {
        b.file(
            if i % 3 == 0 { sub } else { root },
            n.as_bytes(),
            i as u64 * 7 % 13,
            (i64::try_from(i).expect("small") * 31) % 17,
        );
    }
    SearchIndex::build(Arc::new(b.finish()))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn matches_agree_with_a_naive_reference(
        names in prop::collection::vec("[a-cA-C._ -]{1,8}", 1..80),
        q in simple_query(),
        case_sensitive in any::<bool>(),
    ) {
        let index = snapshot_of(&names);
        let text = q.render();
        let query = parse(&text, options(case_sensitive, false, false)).unwrap();
        let got = index.search(&query, Sort::default(), 0, &|| false).unwrap();
        let mut got: Vec<usize> = got.iter().map(|id| id.index()).collect();
        got.sort_unstable();
        let expected: Vec<usize> = (0..names.len())
            .filter(|&i| q.matches(&names[i], case_sensitive))
            .collect();
        prop_assert_eq!(got, expected, "query {:?}", text);
    }

    #[test]
    fn match_path_sees_folder_names(
        names in prop::collection::vec("[a-c._]{1,6}", 1..40),
        term in "[a-c/._]{1,4}",
    ) {
        let index = snapshot_of(&names);
        let query = parse(&term, options(false, true, false)).unwrap();
        let got = index.search(&query, Sort::default(), 0, &|| false).unwrap();
        let snap = index.snapshot();
        let mut got: Vec<usize> = got.iter().map(|id| id.index()).collect();
        got.sort_unstable();
        // Full paths: the root folder is "/", the other folder "/sub".
        let expected: Vec<usize> = (0..snap.len())
            .filter(|&i| {
                let path = snap.full_path(EntryId(i as u32));
                path.to_string_lossy().to_lowercase().contains(&term)
            })
            .collect();
        prop_assert_eq!(got, expected, "term {:?}", term);
    }

    #[test]
    fn sorted_outputs_are_ordered_and_limits_are_prefixes(
        entries in prop::collection::vec((0u64..20, -5i64..5), 1..120),
        limit in 0usize..40,
        ascending in any::<bool>(),
    ) {
        let mut b = SnapshotBuilder::new();
        let v = b.volume("/", None);
        let root = b.dir("/", v);
        for (i, (size, mtime)) in entries.iter().enumerate() {
            b.file(root, format!("f{i:03}.txt").as_bytes(), *size, *mtime);
        }
        let index = SearchIndex::build(Arc::new(b.finish()));
        let query = parse("txt", QueryOptions::default()).unwrap();
        for column in [ColumnId::Size, ColumnId::Modified, ColumnId::Name] {
            let sort = Sort { column, ascending };
            let full = index.search(&query, sort, 0, &|| false).unwrap();
            prop_assert_eq!(full.len(), entries.len());
            let key = |id: &EntryId| match column {
                ColumnId::Size => entries[id.index()].0.cast_signed(),
                ColumnId::Modified => entries[id.index()].1,
                _ => i64::try_from(id.index()).expect("small"), // names f000.. are in index order
            };
            for pair in full.windows(2) {
                if ascending {
                    prop_assert!(key(&pair[0]) <= key(&pair[1]));
                } else {
                    prop_assert!(key(&pair[0]) >= key(&pair[1]));
                }
            }
            let limited = index.search(&query, sort, limit, &|| false).unwrap();
            let take = if limit == 0 { full.len() } else { limit.min(full.len()) };
            prop_assert_eq!(&limited[..], &full[..take]);
        }
    }
}

/// Reference wildcard matcher over characters (`*` any run, `?` any single character).
fn glob_reference(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|skip| glob_reference(rest, &text[skip..])),
        Some(('?', rest)) => text.split_first().is_some_and(|(_, t)| glob_reference(rest, t)),
        Some((c, rest)) => text
            .split_first()
            .is_some_and(|(t, tail)| t == c && glob_reference(rest, tail)),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn wildcards_agree_with_a_reference_matcher(
        names in prop::collection::vec("[a-c\u{e9}.]{1,7}", 1..60),
        pattern in "[a-c\u{e9}.*?]{1,6}",
        extra in prop::option::of("[a-c]"),
    ) {
        let index = snapshot_of(&names);
        let text = match &extra {
            Some(term) => format!("{pattern} {term}"),
            None => pattern.clone(),
        };
        let query = parse(&text, QueryOptions::default()).unwrap();
        let got = index.search(&query, Sort::default(), 0, &|| false).unwrap();
        let mut got: Vec<usize> = got.iter().map(|id| id.index()).collect();
        got.sort_unstable();
        let pat: Vec<char> = pattern.chars().collect();
        let expected: Vec<usize> = (0..names.len())
            .filter(|&i| {
                let chars: Vec<char> = names[i].chars().collect();
                let hit = if pattern.contains(['*', '?']) {
                    glob_reference(&pat, &chars)
                } else {
                    names[i].contains(pattern.as_str())
                };
                hit && extra.as_ref().is_none_or(|t| names[i].contains(t.as_str()))
            })
            .collect();
        prop_assert_eq!(got, expected, "query {:?}", text);
    }
}
