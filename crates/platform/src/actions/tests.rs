use std::fs;
use std::path::{Path, PathBuf};

use super::*;

/// A fake XDG tree under a temp dir.
struct Fixture {
    tmp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            tmp: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.tmp.path().join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(path, text).expect("write");
    }

    fn dirs(&self) -> XdgDirs {
        XdgDirs {
            config_home: self.tmp.path().join("config"),
            config_dirs: Vec::new(),
            data_home: self.tmp.path().join("home-data"),
            data_dirs: vec![self.tmp.path().join("sys-data")],
            desktops: vec!["KDE".to_owned()],
            locales: Vec::new(),
        }
    }

    fn catalog(&self) -> ActionCatalog {
        ActionCatalog::load(&self.dirs())
    }
}

fn file(path: &str) -> Target {
    Target {
        path: PathBuf::from(path),
        is_dir: false,
    }
}

fn dir(path: &str) -> Target {
    Target {
        path: PathBuf::from(path),
        is_dir: true,
    }
}

fn labels(entries: &[MenuEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.label.as_str()).collect()
}

fn app(name: &str, mime: &str, extra: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={name}\nExec={} %F\nMimeType={mime}\n{extra}",
        name.to_lowercase()
    )
}

#[test]
fn open_with_lists_matching_apps_default_first() {
    let fx = Fixture::new();
    fx.write(
        "sys-data/applications/kate.desktop",
        &app("Kate", "text/plain;", ""),
    );
    fx.write(
        "sys-data/applications/gedit.desktop",
        &app("Gedit", "text/plain;", ""),
    );
    fx.write(
        "sys-data/applications/gwenview.desktop",
        &app("Gwenview", "image/png;", ""),
    );
    fx.write(
        "config/mimeapps.list",
        "[Default Applications]\ntext/plain=kate.desktop;\n",
    );
    let catalog = fx.catalog();
    let entries = catalog.open_with(&[file("/home/me/notes.txt")]);
    assert_eq!(labels(&entries), ["Kate", "Gedit"]);
    assert_eq!(entries[0].id, "open-with:kate.desktop");
    assert_eq!(
        labels(&catalog.open_with(&[file("/home/me/pic.png")])),
        ["Gwenview"]
    );
    assert_eq!(
        labels(&catalog.open_with(&[file("/home/me/data.xyz")])),
        Vec::<&str>::new()
    );
}

#[test]
fn open_with_needs_every_selected_type_to_be_handled() {
    let fx = Fixture::new();
    fx.write(
        "sys-data/applications/kate.desktop",
        &app("Kate", "text/plain;", ""),
    );
    let catalog = fx.catalog();
    assert_eq!(
        labels(&catalog.open_with(&[file("/a.txt"), file("/b.txt")])),
        ["Kate"]
    );
    assert_eq!(
        labels(&catalog.open_with(&[file("/a.txt"), file("/b.png")])),
        Vec::<&str>::new()
    );
}

#[test]
fn hidden_terminal_and_other_desktop_apps_are_left_out() {
    let fx = Fixture::new();
    fx.write(
        "sys-data/applications/a.desktop",
        &app("Hidden", "text/plain;", "NoDisplay=true\n"),
    );
    fx.write(
        "sys-data/applications/b.desktop",
        &app("Term", "text/plain;", "Terminal=true\n"),
    );
    fx.write(
        "sys-data/applications/c.desktop",
        &app("Gnome", "text/plain;", "OnlyShowIn=GNOME;\n"),
    );
    fx.write(
        "sys-data/applications/d.desktop",
        &app("Broken", "text/plain;", "TryExec=/nonexistent/x\n"),
    );
    fx.write(
        "sys-data/applications/e.desktop",
        &app("Shown", "text/plain;", "OnlyShowIn=KDE;\n"),
    );
    assert_eq!(labels(&fx.catalog().open_with(&[file("/a.txt")])), ["Shown"]);
}

#[test]
fn user_entries_shadow_system_ones_and_removed_associations_hide_apps() {
    let fx = Fixture::new();
    fx.write(
        "sys-data/applications/kate.desktop",
        &app("Kate", "text/plain;", ""),
    );
    fx.write(
        "home-data/applications/kate.desktop",
        &app("My Kate", "text/plain;", ""),
    );
    fx.write(
        "sys-data/applications/gedit.desktop",
        &app("Gedit", "text/plain;", ""),
    );
    fx.write(
        "config/mimeapps.list",
        "[Removed Associations]\ntext/plain=gedit.desktop;\n",
    );
    assert_eq!(labels(&fx.catalog().open_with(&[file("/a.txt")])), ["My Kate"]);
}

#[test]
fn added_associations_extend_an_apps_types() {
    let fx = Fixture::new();
    fx.write(
        "sys-data/applications/viewer.desktop",
        &app("Viewer", "image/png;", ""),
    );
    fx.write(
        "config/mimeapps.list",
        "[Added Associations]\napplication/pdf=viewer.desktop;\n",
    );
    assert_eq!(labels(&fx.catalog().open_with(&[file("/a.pdf")])), ["Viewer"]);
}

#[test]
fn folders_are_opened_by_apps_that_list_inode_directory() {
    let fx = Fixture::new();
    fx.write(
        "sys-data/applications/files.desktop",
        &app("Files", "inode/directory;", ""),
    );
    fx.write(
        "sys-data/applications/kate.desktop",
        &app("Kate", "text/plain;", ""),
    );
    assert_eq!(labels(&fx.catalog().open_with(&[dir("/home/me")])), ["Files"]);
}

const SERVICE_MENU: &str = "[Desktop Entry]
Type=Service
MimeType=image/*;application/pdf;
Actions=resize;hash;
X-KDE-Submenu=Image tools
X-KDE-ServiceTypes=KonqPopupMenu/Plugin

[Desktop Action resize]
Name=Resize…
Exec=resizer --to 50 %F
Icon=transform-scale

[Desktop Action hash]
Name=Show checksum
Exec=sh -c 'sha256sum \"$1\"' _ %f
";

#[test]
fn service_menu_actions_match_by_mime_pattern_and_group_into_a_submenu() {
    let fx = Fixture::new();
    fx.write("sys-data/kio/servicemenus/images.desktop", SERVICE_MENU);
    let catalog = fx.catalog();
    let png = catalog.actions_for(&[file("/home/me/a.png")]);
    assert_eq!(labels(&png), ["Resize…", "Show checksum"]);
    assert_eq!(png[0].submenu.as_deref(), Some("Image tools"));
    assert_eq!(png[0].id, "service:images.desktop:resize");
    assert_eq!(png[0].launcher.icon.as_deref(), Some("transform-scale"));
    assert_eq!(png[0].source, Source::ServiceMenu);
    assert_eq!(labels(&catalog.actions_for(&[file("/home/me/a.pdf")])).len(), 2);
    assert_eq!(
        labels(&catalog.actions_for(&[file("/home/me/a.txt")])),
        Vec::<&str>::new()
    );
    assert_eq!(
        labels(&catalog.actions_for(&[dir("/home/me")])),
        Vec::<&str>::new()
    );
}

#[test]
fn service_menus_for_other_protocols_or_without_types_are_skipped() {
    let fx = Fixture::new();
    fx.write(
        "sys-data/kio/servicemenus/smb.desktop",
        "[Desktop Entry]\nMimeType=all/all;\nX-KDE-Protocols=smb\nActions=a;\n[Desktop Action a]\nName=A\nExec=a\n",
    );
    fx.write(
        "sys-data/kio/servicemenus/untyped.desktop",
        "[Desktop Entry]\nActions=a;\n[Desktop Action a]\nName=B\nExec=b\n",
    );
    assert_eq!(
        labels(&fx.catalog().actions_for(&[file("/a.txt")])),
        Vec::<&str>::new()
    );
}

#[test]
fn service_menu_for_folders_and_for_any_file() {
    let fx = Fixture::new();
    fx.write(
        "sys-data/kio/servicemenus/terminal.desktop",
        "[Desktop Entry]\nMimeType=inode/directory;\nActions=t;\n[Desktop Action t]\nName=Open terminal here\nExec=konsole --workdir %f\n",
    );
    fx.write(
        "sys-data/kio/servicemenus/everything.desktop",
        "[Desktop Entry]\nMimeType=all/allfiles;\nActions=e;\n[Desktop Action e]\nName=Any file\nExec=x %F\n",
    );
    let catalog = fx.catalog();
    assert_eq!(
        labels(&catalog.actions_for(&[dir("/home/me")])),
        ["Open terminal here"]
    );
    assert_eq!(
        labels(&catalog.actions_for(&[file("/home/me/a.bin")])),
        ["Any file"]
    );
}

#[test]
fn nemo_actions_respect_selection_extensions_and_conditions() {
    let fx = Fixture::new();
    fx.write(
        "sys-data/nemo/actions/one.nemo_action",
        "[Nemo Action]\nName=_Wipe\nExec=shred %F\nSelection=s\nExtensions=txt;log;\n",
    );
    fx.write(
        "sys-data/nemo/actions/many.nemo_action",
        "[Nemo Action]\nName=Bundle\nExec=zip out.zip %F\nSelection=m\nExtensions=any;\n",
    );
    fx.write(
        "sys-data/nemo/actions/cond.nemo_action",
        "[Nemo Action]\nName=Cond\nExec=c\nSelection=any\nConditions=dbus org.x\n",
    );
    let catalog = fx.catalog();
    let one = catalog.actions_for(&[file("/a.txt")]);
    assert_eq!(labels(&one), ["Wipe"]);
    assert_eq!(one[0].launcher.dialect, Dialect::Nemo);
    assert_eq!(
        labels(&catalog.actions_for(&[file("/a.png")])),
        Vec::<&str>::new()
    );
    assert_eq!(
        labels(&catalog.actions_for(&[file("/a.txt"), file("/b.png")])),
        ["Bundle"]
    );
}

#[test]
fn thunar_actions_match_patterns_and_kinds() {
    let fx = Fixture::new();
    fx.write(
        "config/Thunar/uca.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<actions>
<action>
	<icon>utilities-terminal</icon>
	<name>Open Terminal &amp;Here</name>
	<unique-id>1111</unique-id>
	<command>xterm -e &quot;cd %f&quot;</command>
	<description>Terminal</description>
	<patterns>*</patterns>
	<directories/>
</action>
<action>
	<name>Optimize</name>
	<unique-id>2222</unique-id>
	<command>optipng %F</command>
	<patterns>*.png;*.PNG</patterns>
	<image-files/>
</action>
</actions>
"#,
    );
    let catalog = fx.catalog();
    let folder = catalog.actions_for(&[dir("/home/me")]);
    assert_eq!(labels(&folder), ["Open Terminal &Here"]);
    assert_eq!(folder[0].launcher.exec, "xterm -e \"cd %f\"");
    assert_eq!(folder[0].id, "thunar:1111");
    assert_eq!(labels(&catalog.actions_for(&[file("/a.PNG")])), ["Optimize"]);
    assert_eq!(
        labels(&catalog.actions_for(&[file("/a.jpg")])),
        Vec::<&str>::new()
    );
}

#[test]
fn glob_matching() {
    assert!(glob_match("*.txt", "a.txt"));
    assert!(glob_match("a?c", "abc"));
    assert!(!glob_match("*.txt", "a.txt.bak"));
    assert!(glob_match("*", ""));
}

#[test]
fn launch_runs_the_command_per_file_without_a_shell() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("out.txt");
    let script = tmp.path().join("run.sh");
    fs::write(&script, "#!/bin/sh\necho \"$1\" >> \"$2\"\n").expect("script");
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let evil = tmp.path().join("a; touch pwned");
    fs::write(&evil, "").expect("file");
    let other = tmp.path().join("b");
    fs::write(&other, "").expect("file");
    let launcher = Launcher {
        dialect: Dialect::DesktopEntry,
        exec: format!("{} %f {}", script.display(), out.display()),
        name: "t".to_owned(),
        icon: None,
        desktop_file: None,
    };
    let started = launch(&launcher, &[evil.clone(), other.clone()]).expect("launched");
    assert_eq!(started, 2);
    // The children are detached: wait for both lines to appear.
    let mut text = String::new();
    for _ in 0..200 {
        text = fs::read_to_string(&out).unwrap_or_default();
        if text.lines().count() == 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let mut lines: Vec<&str> = text.lines().collect();
    lines.sort_unstable();
    assert_eq!(
        lines,
        [evil.to_str().expect("utf8"), other.to_str().expect("utf8")]
    );
    assert!(!Path::new("pwned").exists());
    assert!(
        !tmp.path().join("pwned").exists(),
        "no shell interpreted the name"
    );
}

#[test]
fn launch_reports_a_missing_program() {
    let launcher = Launcher {
        dialect: Dialect::DesktopEntry,
        exec: "/nonexistent/program %f".to_owned(),
        name: "t".to_owned(),
        icon: None,
        desktop_file: None,
    };
    let err = launch(&launcher, &[PathBuf::from("/a")]).expect_err("fails");
    assert!(matches!(err, PlatformError::Spawn { .. }), "{err:?}");
}
