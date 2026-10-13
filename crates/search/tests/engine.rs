//! End-to-end tests: parse a query, run it against a small hand-built snapshot.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{FixedOffset, TimeZone};
use everythux_core::config::ColumnId;
use everythux_core::ids::EntryId;
use everythux_core::model::EntryKind;
use everythux_core::snapshot::IndexSnapshot;
use everythux_search::synthetic::SnapshotBuilder;
use everythux_search::{QueryError, QueryOptions, SearchIndex, Sort, parse_at};

const KB: u64 = 1024;
const MB: u64 = 1024 * KB;
const DAY_NS: i64 = 86_400 * 1_000_000_000;

/// 2026-03-18 12:00 UTC, a Wednesday.
fn now() -> chrono::DateTime<FixedOffset> {
    FixedOffset::east_opt(0)
        .expect("UTC offset")
        .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
        .single()
        .expect("valid time")
}

fn midnight_ns(y: i32, m: u32, d: u32) -> i64 {
    chrono::NaiveDate::from_ymd_opt(y, m, d)
        .expect("valid date")
        .and_hms_opt(0, 0, 0)
        .expect("valid time")
        .and_utc()
        .timestamp()
        * 1_000_000_000
}

struct Fixture {
    index: SearchIndex,
}

fn fixture() -> Fixture {
    let mut b = SnapshotBuilder::new();
    let root = b.volume("/", None);
    let data = b.volume("/mnt/data", Some("Backup"));
    b.dir("/", root);
    let home = b.dir("/home", root);
    let alice = b.dir("/home/alice", root);
    let docs = b.dir("/home/alice/Docs", root);
    let src = b.dir("/home/alice/src", root);
    let usr = b.dir("/usr", root);
    b.dir("/mnt/data", data);
    let photos = b.dir("/mnt/data/photos", data);
    let today = midnight_ns(2026, 3, 18) + 3600 * 1_000_000_000;
    let _ = (home, usr);

    b.file(docs, b"Report Final.PDF", 2 * MB, today);
    b.file(docs, b"report draft.docx", 300 * KB, today - DAY_NS);
    b.file(docs, b"notes.txt", 12, today - 3 * DAY_NS);
    b.file(docs, b"Notes.TXT", 20, midnight_ns(2026, 3, 9) + 5);
    b.file(docs, b"budget 2025.xlsx", 80 * KB, midnight_ns(2025, 11, 30));
    b.file(src, b"main.rs", 4 * KB, today);
    b.file(src, b"lib.rs", 2 * KB, today - 40 * DAY_NS);
    b.file(src, b"Makefile", KB, today - 400 * DAY_NS);
    b.file(src, b"README", 900, today - 2 * DAY_NS);
    b.file(src, b"a.tar.gz", 5 * MB, midnight_ns(2024, 6, 1));
    b.file(alice, b".bashrc", 300, midnight_ns(2020, 1, 1));
    b.file(alice, b"caf\xc3\xa9 Menu.txt", 50, today);
    b.file(alice, b"bad\xffname.txt", 7, today);
    b.file(photos, b"IMG_0001.jpg", 3 * MB, midnight_ns(2025, 12, 24));
    b.file(photos, b"img_0002.JPG", 4 * MB, midnight_ns(2025, 12, 25));
    b.file(photos, b"holiday.png", 10 * MB, midnight_ns(2026, 3, 1));
    b.entry(alice, b"link", EntryKind::Symlink, 4, today);
    Fixture {
        index: SearchIndex::build(Arc::new(b.finish())),
    }
}

impl Fixture {
    fn snap(&self) -> &IndexSnapshot {
        self.index.snapshot()
    }

    fn ids(&self, q: &str, options: QueryOptions, sort: Sort, limit: usize) -> Vec<EntryId> {
        let query = parse_at(q, options, &now()).unwrap_or_else(|e| panic!("{q:?}: {e}"));
        self.index
            .search(&query, sort, limit, &|| false)
            .expect("not cancelled")
    }

    /// Lossy names in name order.
    fn names(&self, q: &str) -> Vec<String> {
        self.names_with(q, QueryOptions::default())
    }

    fn names_with(&self, q: &str, options: QueryOptions) -> Vec<String> {
        self.ids(q, options, Sort::default(), 0)
            .into_iter()
            .map(|id| String::from_utf8_lossy(self.snap().name(id)).into_owned())
            .collect()
    }
}

#[test]
fn empty_query_returns_everything_in_name_order() {
    let f = fixture();
    let all = f.ids("", QueryOptions::default(), Sort::default(), 0);
    assert_eq!(all.len(), f.snap().len());
    let names: Vec<String> = all
        .iter()
        .map(|&id| String::from_utf8_lossy(f.snap().name(id)).to_lowercase())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

#[test]
fn substring_is_case_insensitive_by_default() {
    let f = fixture();
    assert_eq!(f.names("REPORT"), ["report draft.docx", "Report Final.PDF"]);
    assert_eq!(f.names("pdf"), ["Report Final.PDF"]);
    assert_eq!(f.names("zzz"), Vec::<String>::new());
}

#[test]
fn terms_are_anded() {
    let f = fixture();
    assert_eq!(f.names("report final"), ["Report Final.PDF"]);
    assert_eq!(f.names("report notes"), Vec::<String>::new());
}

#[test]
fn or_binds_tighter_than_and() {
    let f = fixture();
    assert_eq!(
        f.names("notes|report"),
        ["Notes.TXT", "notes.txt", "report draft.docx", "Report Final.PDF"]
    );
    assert_eq!(f.names("txt notes|report"), ["Notes.TXT", "notes.txt"]);
    assert_eq!(
        f.names("report|budget docx|xlsx"),
        ["budget 2025.xlsx", "report draft.docx"]
    );
    assert_eq!(
        f.names("report | budget"),
        ["budget 2025.xlsx", "report draft.docx", "Report Final.PDF"]
    );
}

#[test]
fn not_excludes() {
    let f = fixture();
    assert_eq!(f.names("report !final"), ["report draft.docx"]);
    assert_eq!(
        f.names("notes !notes.txt"),
        Vec::<String>::new(),
        "folded: both spellings are excluded"
    );
    let sensitive = QueryOptions {
        case_sensitive: true,
        ..QueryOptions::default()
    };
    assert_eq!(f.names_with("Notes !notes", sensitive), ["Notes.TXT"]);
    assert_eq!(f.names("!!report final"), ["Report Final.PDF"]);
}

#[test]
fn phrases_keep_spaces() {
    let f = fixture();
    assert_eq!(f.names("\"report final\""), ["Report Final.PDF"]);
    assert_eq!(f.names("\"final report\""), Vec::<String>::new());
    assert_eq!(f.names("\"2025.x\""), ["budget 2025.xlsx"]);
}

#[test]
fn wildcards_match_the_whole_name() {
    let f = fixture();
    assert_eq!(
        f.names("*.txt"),
        [
            "bad\u{fffd}name.txt",
            "caf\u{e9} Menu.txt",
            "Notes.TXT",
            "notes.txt"
        ]
    );
    assert_eq!(f.names("main.*"), ["main.rs"]);
    assert_eq!(f.names("main.r"), ["main.rs"], "no wildcard: substring");
    assert_eq!(f.names("main.r*s"), ["main.rs"]);
    assert_eq!(f.names("?ib.rs"), ["lib.rs"]);
    assert_eq!(f.names("???.rs"), ["lib.rs"]);
    assert_eq!(f.names("?.rs"), Vec::<String>::new());
    assert_eq!(f.names("note*"), ["Notes.TXT", "notes.txt"]);
    assert_eq!(f.names("*otes*"), ["Notes.TXT", "notes.txt"]);
    assert_eq!(f.names("report*.docx"), ["report draft.docx"]);
    assert_eq!(
        f.names("report*final"),
        Vec::<String>::new(),
        "wildcard patterns are anchored"
    );
    assert_eq!(f.names("report*final*"), ["Report Final.PDF"]);
    assert_eq!(f.names("report*final*pdf"), ["Report Final.PDF"]);
    assert_eq!(f.names("caf\u{e9}*"), ["caf\u{e9} Menu.txt"]);
    assert_eq!(
        f.names("\"c?f\u{e9} *\""),
        ["caf\u{e9} Menu.txt"],
        "? matches one character, not one byte"
    );
}

#[test]
fn case_sensitivity() {
    let f = fixture();
    let sensitive = QueryOptions {
        case_sensitive: true,
        ..QueryOptions::default()
    };
    assert_eq!(f.names_with("Notes", sensitive), ["Notes.TXT"]);
    assert_eq!(f.names_with("notes", sensitive), ["notes.txt"]);
    assert_eq!(f.names_with("*.TXT", sensitive), ["Notes.TXT"]);
    assert_eq!(f.names("case:Notes"), ["Notes.TXT"]);
    assert_eq!(f.names("case:notes"), ["notes.txt"]);
    assert_eq!(
        f.names("case: Notes"),
        ["Notes.TXT"],
        "bare case: switches the whole query"
    );
    assert_eq!(f.names("case:IMG"), ["IMG_0001.jpg"]);
    assert_eq!(f.names("img"), ["IMG_0001.jpg", "img_0002.JPG"]);
}

#[test]
fn ext_filter() {
    let f = fixture();
    assert_eq!(f.names("ext:pdf"), ["Report Final.PDF"]);
    assert_eq!(f.names("ext:PDF;docx"), ["report draft.docx", "Report Final.PDF"]);
    assert_eq!(
        f.names("ext:jpg|png"),
        ["holiday.png", "IMG_0001.jpg", "img_0002.JPG"]
    );
    assert_eq!(f.names("ext:gz"), ["a.tar.gz"]);
    assert_eq!(f.names("ext:tar"), Vec::<String>::new());
    assert_eq!(f.names("ext:nonexistent"), Vec::<String>::new());
    assert_eq!(f.names("report !ext:pdf"), ["report draft.docx"]);
    assert_eq!(f.names("ext:txt notes"), ["Notes.TXT", "notes.txt"]);
}

#[test]
fn size_filter() {
    let f = fixture();
    assert_eq!(f.names("file: size:>4mb"), ["a.tar.gz", "holiday.png"]);
    assert_eq!(
        f.names("file: size:>=4mb"),
        ["a.tar.gz", "holiday.png", "img_0002.JPG"]
    );
    assert_eq!(
        f.names("size:<=12 file:"),
        ["bad\u{fffd}name.txt", "link", "notes.txt"]
    );
    assert_eq!(f.names("size:2mb"), ["Report Final.PDF"]);
    assert_eq!(
        f.names("file: size:2mb..3mb"),
        ["IMG_0001.jpg", "Report Final.PDF"]
    );
    assert_eq!(f.names("size:1kb..2kb ext:rs"), ["lib.rs"]);
    assert_eq!(f.names("size:>1gb"), Vec::<String>::new());
    assert_eq!(f.names("size:300"), [".bashrc"]);
    assert_eq!(f.names("size:<1kb size:>299"), [".bashrc", "README"]);
    assert_eq!(
        f.names("ext:txt size:>1"),
        [
            "bad\u{fffd}name.txt",
            "caf\u{e9} Menu.txt",
            "Notes.TXT",
            "notes.txt"
        ]
    );
}

#[test]
fn folder_sizes_are_recursive() {
    let f = fixture();
    // /home/alice/Docs holds 2mb + 300kb + 12 + 20 + 80kb
    let docs_size = 2 * MB + 300 * KB + 12 + 20 + 80 * KB;
    assert_eq!(f.names(&format!("folder: size:{docs_size}")), ["Docs"]);
}

#[test]
fn modified_filter() {
    let f = fixture();
    assert_eq!(
        f.names("dm:today"),
        [
            "bad\u{fffd}name.txt",
            "caf\u{e9} Menu.txt",
            "link",
            "main.rs",
            "Report Final.PDF"
        ]
    );
    assert_eq!(f.names("dm:yesterday"), ["report draft.docx"]);
    assert_eq!(
        f.names("dm:today|yesterday ext:docx|pdf"),
        ["report draft.docx", "Report Final.PDF"]
    );
    assert_eq!(
        f.names("dm:thisweek ext:docx|pdf|txt"),
        [
            "bad\u{fffd}name.txt",
            "caf\u{e9} Menu.txt",
            "report draft.docx",
            "Report Final.PDF"
        ]
    );
    assert_eq!(f.names("dm:lastweek"), ["Notes.TXT", "notes.txt"]);
    assert_eq!(
        f.names("dm:thismonth ext:png|pdf"),
        ["holiday.png", "Report Final.PDF"]
    );
    assert_eq!(f.names("dm:lastmonth"), ["lib.rs"]);
    assert_eq!(f.names("dm:2025-12"), ["IMG_0001.jpg", "img_0002.JPG"]);
    assert_eq!(f.names("dm:2025-12-25"), ["img_0002.JPG"]);
    assert_eq!(
        f.names("dm:2025-12-24..2025-12-25"),
        ["IMG_0001.jpg", "img_0002.JPG"]
    );
    assert_eq!(
        f.names("dm:2025 ext:xlsx|jpg"),
        ["budget 2025.xlsx", "IMG_0001.jpg", "img_0002.JPG"]
    );
    assert_eq!(
        f.names("dm:lastyear"),
        ["budget 2025.xlsx", "IMG_0001.jpg", "img_0002.JPG", "Makefile"]
    );
    assert_eq!(f.names("file: dm:<2021"), [".bashrc"]);
    assert_eq!(f.names("dm:2024 ext:gz"), ["a.tar.gz"]);
    assert_eq!(
        f.names("!dm:thisyear ext:rs|gz|xlsx"),
        ["a.tar.gz", "budget 2025.xlsx"]
    );
}

#[test]
fn file_and_folder_kind_filters() {
    let f = fixture();
    assert_eq!(
        f.names("folder:"),
        ["alice", "Docs", "home", "photos", "src", "usr"]
    );
    assert_eq!(f.names("folder:doc"), ["Docs"]);
    assert_eq!(f.names("folder:s*"), ["src"]);
    assert_eq!(f.names("file:src"), Vec::<String>::new(), "src is a folder");
    assert_eq!(f.names("file:main"), ["main.rs"]);
    assert_eq!(f.names("!file: a"), f.names("folder: a"));
    assert!(!f.names("file:").contains(&"Docs".to_owned()));
    assert!(
        f.names("file:").contains(&"link".to_owned()),
        "symlinks count as files"
    );
}

#[test]
fn path_filter_matches_folders_and_names() {
    let f = fixture();
    assert_eq!(f.names("path:alice/docs ext:txt"), ["Notes.TXT", "notes.txt"]);
    assert_eq!(
        f.names("path:/home/alice/src"),
        ["a.tar.gz", "lib.rs", "main.rs", "Makefile", "README", "src"]
    );
    assert_eq!(f.names("path:src/main"), ["main.rs"]);
    assert_eq!(
        f.names("path:src/"),
        ["a.tar.gz", "lib.rs", "main.rs", "Makefile", "README"]
    );
    assert_eq!(f.names("path:photos holiday"), ["holiday.png"]);
    assert_eq!(
        f.names("path:photos !holiday"),
        ["IMG_0001.jpg", "img_0002.JPG", "photos"]
    );
    assert_eq!(
        f.names("path:mnt/data/photos/img_"),
        ["IMG_0001.jpg", "img_0002.JPG"]
    );
    assert_eq!(
        f.names("path:data/photos"),
        ["holiday.png", "IMG_0001.jpg", "img_0002.JPG", "photos"]
    );
    assert_eq!(
        f.names("path:Docs/Report"),
        ["report draft.docx", "Report Final.PDF"]
    );
    assert_eq!(f.names("path:\"alice/docs/report f\""), ["Report Final.PDF"]);
    assert_eq!(
        f.names("path:alice*txt"),
        [
            "bad\u{fffd}name.txt",
            "caf\u{e9} Menu.txt",
            "Notes.TXT",
            "notes.txt"
        ]
    );
    assert_eq!(f.names("path:zzz"), Vec::<String>::new());
}

#[test]
fn path_wildcards_and_case() {
    let f = fixture();
    assert_eq!(f.names("path:*/photos/*.png"), ["holiday.png"]);
    assert_eq!(f.names("path:/mnt/*/IMG_0001.jpg"), ["IMG_0001.jpg"]);
    let sensitive = QueryOptions {
        case_sensitive: true,
        ..QueryOptions::default()
    };
    assert_eq!(f.names_with("path:Docs ext:pdf", sensitive), ["Report Final.PDF"]);
    assert_eq!(f.names_with("path:docs ext:pdf", sensitive), Vec::<String>::new());
    assert_eq!(f.names("path:docs ext:pdf"), ["Report Final.PDF"]);
}

#[test]
fn match_path_option_applies_to_plain_terms() {
    let f = fixture();
    let opts = QueryOptions {
        match_path: true,
        ..QueryOptions::default()
    };
    assert_eq!(f.names_with("photos holiday", opts), ["holiday.png"]);
    assert_eq!(f.names_with("alice/src/lib", opts), ["lib.rs"]);
    assert_eq!(
        f.names_with("photos", opts),
        ["holiday.png", "IMG_0001.jpg", "img_0002.JPG", "photos"]
    );
    assert_eq!(f.names_with("photos", QueryOptions::default()), ["photos"]);
    assert_eq!(f.names_with("*/photos/*.png", opts), ["holiday.png"]);
    assert_eq!(f.names_with("regex:alice.src.*rs$", opts), ["lib.rs", "main.rs"]);
}

#[test]
fn regex_filter_and_option() {
    let f = fixture();
    assert_eq!(
        f.names("regex:^img_\\d+\\.jpg$"),
        ["IMG_0001.jpg", "img_0002.JPG"]
    );
    assert_eq!(f.names("regex:^[a-z]+\\.rs$"), ["lib.rs", "main.rs"]);
    assert_eq!(f.names("regex:rs$ !lib"), ["main.rs"]);
    let sensitive = QueryOptions {
        case_sensitive: true,
        ..QueryOptions::default()
    };
    assert_eq!(f.names_with("regex:^img_", sensitive), ["img_0002.JPG"]);
    let all_regex = QueryOptions {
        regex: true,
        ..QueryOptions::default()
    };
    assert_eq!(
        f.names_with("^(main|lib)\\.rs$", all_regex),
        ["lib.rs", "main.rs"]
    );
    assert_eq!(f.names_with("a b", all_regex), Vec::<String>::new());
    assert!(matches!(
        parse_at("(", all_regex, &now()),
        Err(QueryError::Regex(_))
    ));
}

#[test]
fn volume_filter() {
    let f = fixture();
    assert_eq!(
        f.names("vol:data"),
        ["holiday.png", "IMG_0001.jpg", "img_0002.JPG", "photos"]
    );
    assert_eq!(
        f.names("vol:backup ext:png"),
        ["holiday.png"],
        "label matches too"
    );
    assert_eq!(f.names("vol:mnt ext:jpg").len(), 2);
    assert_eq!(f.names("vol:nothere"), Vec::<String>::new());
    assert!(f.names("!vol:data").contains(&"main.rs".to_owned()));
    assert!(!f.names("!vol:data").contains(&"holiday.png".to_owned()));
}

#[test]
fn invalid_utf8_names_are_searchable() {
    let f = fixture();
    assert_eq!(f.names("bad"), ["bad\u{fffd}name.txt"]);
    assert_eq!(f.names("name.txt"), ["bad\u{fffd}name.txt"]);
    assert_eq!(f.names("bad*txt"), ["bad\u{fffd}name.txt"]);
    let sensitive = QueryOptions {
        case_sensitive: true,
        ..QueryOptions::default()
    };
    assert_eq!(f.names_with("name.txt", sensitive), ["bad\u{fffd}name.txt"]);
    assert_eq!(
        f.names("bad?name*"),
        ["bad\u{fffd}name.txt"],
        "folded names hold U+FFFD, which ? matches"
    );
    assert_eq!(f.names("bad ext:txt"), ["bad\u{fffd}name.txt"]);
}

#[test]
fn unicode_folding_matches_indexed_names() {
    let f = fixture();
    assert_eq!(f.names("CAF\u{c9}"), ["caf\u{e9} Menu.txt"]);
    assert_eq!(f.names("menu"), ["caf\u{e9} Menu.txt"]);
}

fn sorted(f: &Fixture, column: ColumnId, ascending: bool, q: &str) -> Vec<EntryId> {
    f.ids(q, QueryOptions::default(), Sort { column, ascending }, 0)
}

#[test]
fn every_column_sorts_ascending_and_descending() {
    let f = fixture();
    let s = f.snap();
    for q in ["", "txt|rs|jpg|pdf", "!zzz"] {
        for column in ColumnId::ALL {
            let asc = sorted(&f, column, true, q);
            let mut desc = sorted(&f, column, false, q);
            desc.reverse();
            assert_eq!(asc, desc, "{column:?} {q:?}");
            let key = |id: EntryId| -> (Vec<u8>, i64) {
                let i = id.index();
                match column {
                    ColumnId::Name => (s.entries.names_fold.get(i).to_vec(), 0),
                    ColumnId::Path => (s.dirs.paths.get(s.entries.dir[i].index()).to_ascii_lowercase(), 0),
                    ColumnId::Type => (s.extension(id).as_bytes().to_vec(), 0),
                    ColumnId::Size => (Vec::new(), s.entries.size[i].cast_signed()),
                    ColumnId::Modified => (Vec::new(), s.entries.mtime_ns[i]),
                }
            };
            for pair in asc.windows(2) {
                assert!(
                    key(pair[0]) <= key(pair[1]),
                    "{column:?} {q:?}: {:?} before {:?}",
                    s.name(pair[0]),
                    s.name(pair[1])
                );
            }
        }
    }
}

#[test]
fn ties_are_broken_by_name() {
    let f = fixture();
    let s = f.snap();
    let by_type = sorted(&f, ColumnId::Type, true, "ext:txt");
    let names: Vec<String> = by_type
        .iter()
        .map(|&id| String::from_utf8_lossy(s.name(id)).into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "bad\u{fffd}name.txt",
            "caf\u{e9} Menu.txt",
            "Notes.TXT",
            "notes.txt"
        ]
    );
    let by_path = sorted(&f, ColumnId::Path, true, "ext:txt");
    let first_dir = s.entries.dir[by_path[0].index()];
    assert_eq!(
        s.dirs.paths.get(first_dir.index()),
        b"/home/alice",
        "alice sorts before alice/Docs"
    );
}

#[test]
fn size_and_modified_orders() {
    let f = fixture();
    let by_size: Vec<String> = sorted(&f, ColumnId::Size, false, "ext:rs|png|jpg|pdf")
        .iter()
        .map(|&id| String::from_utf8_lossy(f.snap().name(id)).into_owned())
        .collect();
    assert_eq!(
        by_size,
        [
            "holiday.png",
            "img_0002.JPG",
            "IMG_0001.jpg",
            "Report Final.PDF",
            "main.rs",
            "lib.rs"
        ]
    );
    let by_mtime: Vec<String> = sorted(&f, ColumnId::Modified, true, "ext:png|jpg")
        .iter()
        .map(|&id| String::from_utf8_lossy(f.snap().name(id)).into_owned())
        .collect();
    assert_eq!(by_mtime, ["IMG_0001.jpg", "img_0002.JPG", "holiday.png"]);
}

#[test]
fn limit_keeps_the_first_matches_in_sort_order() {
    let f = fixture();
    let all = f.ids("txt", QueryOptions::default(), Sort::default(), 0);
    assert_eq!(all.len(), 4);
    for limit in 1..=5 {
        let got = f.ids("txt", QueryOptions::default(), Sort::default(), limit);
        assert_eq!(got, all[..limit.min(all.len())], "limit {limit}");
    }
    let desc = Sort {
        column: ColumnId::Name,
        ascending: false,
    };
    let all_desc = f.ids("txt", QueryOptions::default(), desc, 0);
    assert_eq!(f.ids("txt", QueryOptions::default(), desc, 2), all_desc[..2]);
    // match-all fast path
    let everything = f.ids("", QueryOptions::default(), Sort::default(), 0);
    assert_eq!(
        f.ids("", QueryOptions::default(), Sort::default(), 3),
        everything[..3]
    );
    assert_eq!(f.ids("", QueryOptions::default(), desc, 3).len(), 3);
    assert_eq!(
        f.ids("", QueryOptions::default(), Sort::default(), 10_000).len(),
        everything.len()
    );
}

#[test]
fn descending_is_the_reverse_of_ascending() {
    let f = fixture();
    let asc = f.ids("e", QueryOptions::default(), Sort::default(), 0);
    let mut desc = f.ids(
        "e",
        QueryOptions::default(),
        Sort {
            column: ColumnId::Name,
            ascending: false,
        },
        0,
    );
    desc.reverse();
    assert_eq!(asc, desc);
}

#[test]
fn cancellation_returns_none() {
    let f = fixture();
    let q = parse_at("txt", QueryOptions::default(), &now()).unwrap();
    assert!(f.index.search(&q, Sort::default(), 0, &|| true).is_none());
    let everything = parse_at("", QueryOptions::default(), &now()).unwrap();
    assert!(
        f.index
            .search(&everything, Sort::default(), 0, &|| true)
            .is_none()
    );
}

#[test]
fn cancellation_is_polled_while_working() {
    let snap = Arc::new(everythux_search::synthetic::generate(300_000, 3));
    let index = SearchIndex::build(snap);
    let q = parse_at("report", QueryOptions::default(), &now()).unwrap();
    let polls = AtomicUsize::new(0);
    let result = index.search(&q, Sort::default(), 0, &|| {
        polls.fetch_add(1, Ordering::Relaxed) >= 3
    });
    assert!(result.is_none(), "cancelled after a few chunks");
    assert!(polls.load(Ordering::Relaxed) >= 4);
    let finished = index.search(&q, Sort::default(), 0, &|| false);
    assert!(finished.is_some_and(|ids| !ids.is_empty()));
}

#[test]
fn empty_snapshot_is_fine() {
    let index = SearchIndex::build(Arc::new(IndexSnapshot::empty()));
    for q in ["", "a", "ext:x", "path:a"] {
        let query = parse_at(q, QueryOptions::default(), &now()).unwrap();
        assert_eq!(
            index.search(&query, Sort::default(), 0, &|| false),
            Some(Vec::new()),
            "{q:?}"
        );
    }
}

#[test]
fn large_snapshot_matches_a_naive_scan() {
    let snap = Arc::new(everythux_search::synthetic::generate(150_000, 11));
    let index = SearchIndex::build(Arc::clone(&snap));
    let opts = QueryOptions::default();
    for (q, naive) in [
        (
            "report",
            Box::new(|n: &str| n.contains("report")) as Box<dyn Fn(&str) -> bool>,
        ),
        (
            "img_ jpg",
            Box::new(|n: &str| n.contains("img_") && n.contains("jpg")),
        ),
        ("qzx", Box::new(|n: &str| n.contains("qzx"))),
        (
            "data|lib !x",
            Box::new(|n: &str| (n.contains("data") || n.contains("lib")) && !n.contains('x')),
        ),
        ("e", Box::new(|n: &str| n.contains('e'))),
        ("a", Box::new(|n: &str| n.contains('a'))),
        ("*.tar.gz", Box::new(|n: &str| n.ends_with(".tar.gz"))),
    ] {
        let query = parse_at(q, opts, &now()).unwrap();
        let got = index.search(&query, Sort::default(), 0, &|| false).unwrap();
        let mut expected: Vec<EntryId> = (0..snap.len())
            .map(|i| EntryId(i as u32))
            .filter(|&id| naive(&everythux_core::text::fold(snap.name(id))))
            .collect();
        let mut got_sorted = got.clone();
        got_sorted.sort();
        expected.sort();
        assert_eq!(got_sorted, expected, "{q}");
        assert!(!expected.is_empty() || q == "qzx", "{q} should match something");
    }
}
