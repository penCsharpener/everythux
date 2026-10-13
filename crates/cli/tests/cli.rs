//! Black-box tests: run the built binary against a temporary config and database.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use tempfile::TempDir;

struct Sandbox {
    dir: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("create temp dir"),
        }
    }

    fn config(&self) -> PathBuf {
        self.dir.path().join("config.json")
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("index.db")
    }

    /// Runs `everythux-cli --db … --config … <args>` and returns the raw output.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_everythux-cli"))
            .arg("--db")
            .arg(self.db())
            .arg("--config")
            .arg(self.config())
            .args(args)
            .env_remove("RUST_LOG")
            .env("NO_COLOR", "1")
            .output()
            .expect("run everythux-cli")
    }

    /// Like [`Self::run`], but the command must succeed; returns stdout.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?} failed: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("stdout is UTF-8")
    }

    fn read_config(&self) -> Value {
        let text = fs::read_to_string(self.config()).expect("config file exists");
        serde_json::from_str(&text).expect("config file is JSON")
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn exclude_add_list_test_remove_round_trip() {
    let sb = Sandbox::new();
    let listing = sb.ok(&["exclude", "list"]);
    assert!(listing.contains("node_modules"), "defaults are listed: {listing}");
    let default_count = everythux_core::config::Config::default().exclusions.len();

    let added = sb.ok(&[
        "exclude",
        "add",
        "build-out",
        "--kind",
        "dir",
        "--scope",
        "/home/*",
    ]);
    assert!(added.contains(&format!("Added rule #{default_count}")), "{added}");
    assert!(added.contains("reindex --quick"), "apply hint: {added}");
    assert_eq!(
        sb.read_config()["exclusions"].as_array().unwrap().len(),
        default_count + 1
    );
    assert!(sb.ok(&["exclude", "list"]).contains("build-out"));

    let index = default_count.to_string();
    let hit = sb.ok(&["exclude", "test", "/home/me/proj/build-out"]);
    assert!(hit.contains(&format!("excluded by rule #{index}")), "{hit}");
    let scoped_out = sb.ok(&["exclude", "test", "/srv/build-out"]);
    assert!(
        scoped_out.contains("not excluded"),
        "scope protects other places: {scoped_out}"
    );

    sb.ok(&["exclude", "disable", &index]);
    assert_eq!(
        sb.read_config()["exclusions"][default_count]["enabled"],
        json!(false)
    );
    assert!(
        sb.ok(&["exclude", "test", "/home/me/proj/build-out"])
            .contains("not excluded")
    );

    sb.ok(&["exclude", "enable", &index]);
    assert!(
        sb.ok(&["exclude", "test", "/home/me/proj/build-out"])
            .contains("excluded by rule")
    );

    sb.ok(&["exclude", "remove", &index]);
    assert!(!sb.ok(&["exclude", "list"]).contains("build-out"));
    assert_eq!(
        sb.read_config()["exclusions"].as_array().unwrap().len(),
        default_count
    );
}

#[test]
fn exclude_test_uses_the_real_file_type_when_the_path_exists() {
    let sb = Sandbox::new();
    let tmp = tempfile::tempdir().unwrap();
    let as_file = tmp.path().join("bin");
    fs::write(&as_file, b"x").unwrap();
    sb.ok(&["exclude", "add", tmp.path().to_str().unwrap(), "--kind", "path"]);
    let out = sb.ok(&["exclude", "test", as_file.to_str().unwrap()]);
    assert!(
        out.contains("(file)") && out.contains("excluded by rule"),
        "{out}"
    );
}

#[test]
fn exclude_rejects_invalid_rules_and_bad_indexes() {
    let sb = Sandbox::new();
    let bad = sb.run(&["exclude", "add", "["]);
    assert!(!bad.status.success());
    assert!(stderr(&bad).contains("invalid"), "{}", stderr(&bad));
    assert!(!sb.config().exists(), "nothing is written on failure");

    let relative = sb.run(&["exclude", "add", "rel/dir", "--kind", "path"]);
    assert!(!relative.status.success());
    assert!(stderr(&relative).contains("absolute"), "{}", stderr(&relative));

    let missing = sb.run(&["exclude", "remove", "9999"]);
    assert!(!missing.status.success());
    assert!(stderr(&missing).contains("no rule #9999"), "{}", stderr(&missing));
}

#[test]
fn config_path_show_and_reset() {
    let sb = Sandbox::new();
    assert_eq!(sb.ok(&["config", "path"]).trim(), sb.config().to_str().unwrap());

    let shown: Value = serde_json::from_str(&sb.ok(&["config", "show"])).unwrap();
    assert_eq!(shown["version"], json!(1));
    assert_eq!(shown["volumes"]["auto_discover_fs_types"], json!(["ext4"]));
    assert!(!sb.config().exists(), "show doesn't create the file");

    let refused = sb.run(&["config", "reset"]);
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("--yes"), "{}", stderr(&refused));
    assert!(!sb.config().exists());

    fs::write(sb.config(), r#"{"search": {"regex": true}}"#).unwrap();
    assert_eq!(sb.read_config()["search"]["regex"], json!(true));
    sb.ok(&["config", "reset", "--yes"]);
    assert_eq!(sb.read_config()["search"]["regex"], json!(false));
}

#[test]
fn broken_config_is_reported_with_its_location() {
    let sb = Sandbox::new();
    fs::write(sb.config(), "{ not json").unwrap();
    let out = sb.run(&["config", "show"]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("config.json") && err.contains("line 1"), "{err}");
}

#[test]
fn search_and_stats_without_an_index_point_to_reindex() {
    let sb = Sandbox::new();
    for args in [&["stats"][..], &["search", "foo"]] {
        let out = sb.run(args);
        assert!(!out.status.success(), "{args:?}");
        assert!(
            stderr(&out).contains("reindex --full"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
    assert!(
        !sb.db().exists(),
        "read-only commands must not create the database"
    );
}

#[test]
fn scan_requires_dry_run() {
    let sb = Sandbox::new();
    let out = sb.run(&["scan"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("--dry-run"), "{}", stderr(&out));
}

#[test]
fn help_lists_every_subcommand() {
    let sb = Sandbox::new();
    let help = sb.ok(&["--help"]);
    for name in [
        "scan", "volumes", "reindex", "search", "stats", "exclude", "config",
    ] {
        assert!(help.contains(name), "{name} missing from help:\n{help}");
    }
}

/// A config that indexes exactly `root` (no auto-discovery, no exclusions: the default rules would
/// drop everything under `/tmp`).
fn write_tree_config(sb: &Sandbox, root: &Path) {
    let config = json!({
        "volumes": { "auto_discover_fs_types": [], "include": [root] },
        "exclusions": [],
        "indexer": { "live_updates": false, "rescan_on_startup": "none" },
    });
    fs::write(sb.config(), config.to_string()).expect("write config");
}

#[test]
fn reindex_then_search_end_to_end() {
    let sb = Sandbox::new();
    let tree = tempfile::tempdir().unwrap();
    let root = tree.path();
    fs::create_dir_all(root.join("docs/deep")).unwrap();
    fs::write(root.join("docs/report-final.txt"), vec![b'x'; 2048]).unwrap();
    fs::write(root.join("docs/deep/Notes.TXT"), b"n").unwrap();
    fs::write(root.join("docs/deep/image.png"), b"p").unwrap();
    write_tree_config(&sb, root);

    let summary = sb.ok(&["reindex", "--full"]);
    assert!(summary.contains("Done in"), "{summary}");

    let plain = sb.ok(&["search", "ext:txt", "--sort", "name"]);
    let lines: Vec<&str> = plain.lines().collect();
    assert_eq!(lines.len(), 2, "{plain}");
    assert!(lines[0].ends_with("docs/deep/Notes.TXT"), "{plain}");
    assert!(lines[1].ends_with("docs/report-final.txt"), "{plain}");

    let json_out = sb.ok(&["search", "report-final", "--json"]);
    let row: Value = serde_json::from_str(json_out.lines().next().unwrap()).unwrap();
    assert_eq!(row["name"], json!("report-final.txt"));
    assert_eq!(row["size"], json!(2048));
    assert_eq!(row["kind"], json!("file"));
    assert!(row["modified"].as_str().unwrap().ends_with('Z'));
    assert!(
        row["full_path"]
            .as_str()
            .unwrap()
            .ends_with("docs/report-final.txt")
    );
    assert_eq!(Path::new(row["path"].as_str().unwrap()), root.join("docs"));

    let nul = sb.run(&["search", "image", "--null"]);
    assert!(nul.status.success());
    assert!(nul.stdout.ends_with(b"image.png\0"), "{:?}", nul.stdout);

    let limited = sb.ok(&["search", "txt", "--limit", "1", "--sort", "size", "--desc"]);
    assert_eq!(limited.lines().count(), 1);
    assert!(limited.contains("report-final.txt"), "largest first: {limited}");

    let case = sb.ok(&["search", "NOTES", "--case"]);
    assert!(case.trim().is_empty(), "--case is case-sensitive: {case}");
    assert!(sb.ok(&["search", "NOTES"]).contains("Notes.TXT"));

    let stats = sb.ok(&["stats"]);
    assert!(stats.contains("Entries:"), "{stats}");
}

#[test]
fn partial_reindex_picks_up_new_files_below_a_path() {
    let sb = Sandbox::new();
    let tree = tempfile::tempdir().unwrap();
    let root = tree.path();
    fs::create_dir_all(root.join("a")).unwrap();
    fs::create_dir_all(root.join("b")).unwrap();
    write_tree_config(&sb, root);
    sb.ok(&["reindex", "--full"]);

    fs::write(root.join("a/new-in-a.txt"), b"1").unwrap();
    fs::write(root.join("b/new-in-b.txt"), b"1").unwrap();
    sb.ok(&["reindex", "--full", "--path", root.join("a").to_str().unwrap()]);

    assert!(sb.ok(&["search", "new-in-a"]).contains("new-in-a.txt"));
    assert!(
        sb.ok(&["search", "new-in-b"]).trim().is_empty(),
        "b was not rescanned"
    );
}

#[test]
fn exclusions_keep_matching_entries_out_of_the_index() {
    let sb = Sandbox::new();
    let tree = tempfile::tempdir().unwrap();
    let root = tree.path();
    fs::create_dir_all(root.join("keep")).unwrap();
    fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    fs::write(root.join("keep/file.txt"), b"k").unwrap();
    fs::write(root.join("node_modules/pkg/file.txt"), b"x").unwrap();
    write_tree_config(&sb, root);
    sb.ok(&["exclude", "add", "node_modules", "--kind", "dir"]);
    sb.ok(&["reindex", "--full"]);

    let found = sb.ok(&["search", "file.txt"]);
    assert_eq!(found.lines().count(), 1, "{found}");
    assert!(found.contains("keep/file.txt"));
}
