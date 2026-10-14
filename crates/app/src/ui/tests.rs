//! End-to-end UI tests: the real `App` rendered by `egui_kittest` with a `FakeBackend`.
//! They assert on effects (backend calls, state), not on pixels.

use egui::{Key, Modifiers, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use everythux_core::config::{ColumnId, Config};
use everythux_index::{IndexEvent, Job, JobId};

use crate::app::App;
use crate::backend::fake::{FakeBackend, FakeHandle};
use crate::backend::{BackendEvent, PlatformAction};
use crate::testkit::Fixture;

struct Rig {
    harness: Harness<'static, App>,
    fake: FakeHandle,
    fixture: Fixture,
}

fn rig() -> Rig {
    let (backend, fake, events) = FakeBackend::new();
    let app = App::new(
        egui::Context::default(),
        Box::new(backend),
        events,
        Config::default(),
        Vec::new(),
    );
    let harness = Harness::builder()
        .with_size(Vec2::new(1200.0, 600.0))
        .with_step_dt(0.01)
        .build_ui_state(|ui, app: &mut App| app.show(ui), app);
    Rig {
        harness,
        fake,
        fixture: Fixture::new(),
    }
}

impl Rig {
    /// Shows `ids` of the fixture and renders a few frames.
    fn show(&mut self, ids: &[everythux_core::ids::EntryId]) {
        let generation = self.harness.state().search.latest_generation;
        self.fake
            .send_results(generation, &self.fixture.snapshot, ids, None);
        self.harness.run_steps(3);
    }

    fn show_default(&mut self) {
        let f = &self.fixture;
        let ids = [f.report, f.notes, f.junk];
        self.show(&ids);
    }

    fn click_cell(&mut self, label: &str, modifiers: Modifiers) {
        self.harness.get_by_label(label).click_modifiers(modifiers);
        self.harness.run_steps(3);
    }

    fn double_click_cell(&mut self, label: &str) {
        let node = self.harness.get_by_label(label);
        node.click();
        node.click();
        self.harness.run_steps(3);
    }
}

#[test]
fn typing_in_the_search_box_submits_a_search() {
    let mut r = rig();
    r.harness.run_steps(3);
    let input = r.harness.get_by_role(egui::accesskit::Role::TextInput);
    input.focus();
    r.harness.run_steps(2);
    r.harness
        .get_by_role(egui::accesskit::Role::TextInput)
        .type_text("report");
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().search.text, "report");
    assert_eq!(r.fake.searches().last().expect("a search").text, "report");
}

#[test]
fn ctrl_shift_r_reindexes_even_while_the_search_box_has_focus() {
    let mut r = rig();
    r.harness.run_steps(3);
    r.harness.get_by_role(egui::accesskit::Role::TextInput).focus();
    r.harness.run_steps(2);
    r.harness
        .get_by_role(egui::accesskit::Role::TextInput)
        .type_text("abc");
    r.harness.run_steps(2);
    r.harness
        .key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::R);
    r.harness.run_steps(3);
    assert_eq!(r.fake.jobs(), [Job::FullReindex]);
    assert_eq!(r.harness.state().search.text, "abc", "the box kept its text");
}

#[test]
fn f5_and_ctrl_p_work_globally() {
    let mut r = rig();
    r.harness.run_steps(3);
    r.harness.key_press(Key::F5);
    r.harness.run_steps(3);
    assert_eq!(r.fake.jobs(), [Job::RescanAll(everythux_index::ScanMode::Quick)]);
    r.harness.key_press_modifiers(Modifiers::CTRL, Key::P);
    r.harness.run_steps(3);
    assert!(r.harness.state().config.ui.preview_pane.enabled);
}

#[test]
fn ctrl_shift_r_during_a_running_job_asks_for_confirmation() {
    let mut r = rig();
    r.fake.send(BackendEvent::Index(IndexEvent::Started {
        job: JobId(1),
        description: "Indexing /home".into(),
    }));
    r.harness.run_steps(3);
    r.harness
        .key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::R);
    r.harness.run_steps(3);
    assert!(r.fake.jobs().is_empty(), "nothing before the user confirms");
    assert!(
        r.harness
            .query_by_label("A reindex is in progress. Restart it?")
            .is_some()
    );

    r.harness.get_by_label("Restart").click();
    r.harness.run_steps(3);
    assert_eq!(r.fake.jobs(), [Job::FullReindex]);
    assert!(
        r.harness
            .query_by_label("A reindex is in progress. Restart it?")
            .is_none()
    );
}

#[test]
fn status_bar_counts_objects() {
    let mut r = rig();
    r.show_default();
    assert!(r.harness.query_by_label("3 of 10 objects").is_some());
}

#[test]
fn double_click_on_the_name_cell_opens_the_file() {
    let mut r = rig();
    r.show_default();
    r.double_click_cell("report.pdf");
    assert_eq!(
        r.fake.platform_actions(),
        [PlatformAction::OpenDefault("/home/me/docs/report.pdf".into())]
    );
}

#[test]
fn double_click_on_the_path_cell_shows_the_item_in_the_file_manager() {
    let mut r = rig();
    r.show_default();
    // Two rows share this folder; the first Path cell belongs to report.pdf.
    let node = r
        .harness
        .get_all_by_label("/home/me/docs")
        .next()
        .expect("path cell");
    node.click();
    node.click();
    r.harness.run_steps(3);
    assert_eq!(
        r.fake.platform_actions(),
        [PlatformAction::ShowItem("/home/me/docs/report.pdf".into())]
    );
}

#[test]
fn double_click_on_size_shows_the_containing_folder_properties() {
    let mut r = rig();
    r.show_default();
    r.double_click_cell("2.0 KiB");
    let actions = r.fake.platform_actions();
    let [PlatformAction::ShowProperties(target)] = actions.as_slice() else {
        panic!("expected one ShowProperties, got {actions:?}");
    };
    assert_eq!(target.path, std::path::Path::new("/home/me/docs"));
}

#[test]
fn unsupported_properties_open_the_builtin_window() {
    let mut r = rig();
    r.fake.set_properties_unsupported(true);
    r.show_default();
    r.double_click_cell("2.0 KiB");
    r.harness.run_steps(4);
    assert_eq!(r.harness.state().windows.properties.len(), 1);
    assert!(r.harness.query_by_label("Properties: docs").is_some());
}

#[test]
fn ctrl_shift_click_on_a_path_cell_opens_the_prefilled_exclusion_popup() {
    let mut r = rig();
    r.show_default();
    let node = r
        .harness
        .get_all_by_label("/home/me/proj/node_modules")
        .next()
        .expect("path cell of junk.js");
    node.click_modifiers(Modifiers::CTRL | Modifiers::SHIFT);
    r.harness.run_steps(4);
    let app = r.harness.state();
    let popup = app.windows.exclusion_popup.as_ref().expect("popup open");
    assert_eq!(popup.folder, std::path::Path::new("/home/me/proj/node_modules"));
    assert!(
        app.config
            .exclusions
            .iter()
            .any(|rule| rule.pattern == "/home/me/proj/node_modules"),
        "the rule exists before the popup is dismissed"
    );
    assert!(
        r.harness
            .query_by_label("Remove matching entries from the index now")
            .is_some()
    );
    assert!(r.harness.query_by_label("This folder only").is_some());
    // No file manager action happened.
    assert!(
        r.fake.platform_actions().is_empty(),
        "{:?}",
        r.fake.platform_actions()
    );
}

#[test]
fn popup_undo_removes_the_rule() {
    let mut r = rig();
    r.show_default();
    let before = r.harness.state().config.exclusions.clone();
    let node = r
        .harness
        .get_all_by_label("/home/me/docs")
        .next()
        .expect("path cell");
    node.click_modifiers(Modifiers::CTRL | Modifiers::SHIFT);
    r.harness.run_steps(4);
    assert_ne!(r.harness.state().config.exclusions, before);
    r.harness.get_by_label("Undo").click();
    r.harness.run_steps(4);
    assert_eq!(r.harness.state().config.exclusions, before);
    assert!(r.harness.state().windows.exclusion_popup.is_none());
}

#[test]
fn header_click_sorts_and_toggles_direction() {
    let mut r = rig();
    r.show_default();
    r.fake.clear();
    r.harness.get_by_label("Size").click();
    r.harness.run_steps(3);
    r.harness.get_by_label("Size").click();
    r.harness.run_steps(3);
    let sorts: Vec<_> = r
        .fake
        .searches()
        .iter()
        .map(|s| (s.sort.column, s.sort.ascending))
        .collect();
    assert_eq!(sorts, [(ColumnId::Size, true), (ColumnId::Size, false)]);
}

#[test]
fn single_click_selects_and_enter_opens() {
    let mut r = rig();
    r.show_default();
    r.click_cell("notes.txt", Modifiers::NONE);
    assert_eq!(r.harness.state().selection.cursor(), Some(1));
    r.harness.key_press(Key::Enter);
    r.harness.run_steps(3);
    assert_eq!(
        r.fake.platform_actions(),
        [PlatformAction::OpenDefault("/home/me/docs/notes.txt".into())]
    );
}

#[test]
fn arrow_keys_move_the_selection_while_typing_in_the_search_box() {
    let mut r = rig();
    r.show_default();
    r.harness.get_by_role(egui::accesskit::Role::TextInput).focus();
    r.harness.run_steps(2);
    r.harness.key_press(Key::ArrowDown);
    r.harness.run_steps(2);
    r.harness.key_press(Key::ArrowDown);
    r.harness.run_steps(2);
    assert_eq!(r.harness.state().selection.cursor(), Some(1));
    r.harness.key_press_modifiers(Modifiers::CTRL, Key::Enter);
    r.harness.run_steps(2);
    assert_eq!(
        r.fake.platform_actions(),
        [PlatformAction::ShowItem("/home/me/docs/notes.txt".into())]
    );
}

#[test]
fn escape_clears_the_search_text() {
    let mut r = rig();
    r.harness.run_steps(3);
    r.harness
        .get_by_role(egui::accesskit::Role::TextInput)
        .type_text("abc");
    r.harness.run_steps(2);
    r.harness.key_press(Key::Escape);
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().search.text, "");
}

#[test]
fn copy_event_copies_the_selected_paths() {
    let mut r = rig();
    r.show_default();
    r.click_cell("notes.txt", Modifiers::NONE);
    // Free the keyboard from the search box first, as clicking a row does in a real window.
    r.harness
        .ctx
        .memory_mut(|m| m.surrender_focus(crate::app::search_id()));
    r.harness.run_steps(1);
    r.harness.event(egui::Event::Copy);
    r.harness.run_steps(3);
    assert!(
        r.harness
            .state()
            .toasts
            .iter()
            .any(|t| t.text == "Copied to clipboard"),
        "{:?}",
        r.harness.state().toasts
    );
}

#[test]
fn volumes_window_lists_indexed_volumes_and_degrades_without_udisks() {
    let mut r = rig();
    r.fake.send(BackendEvent::Index(IndexEvent::VolumesChanged(
        r.fixture.snapshot.volumes.clone(),
    )));
    r.harness.run_steps(2);
    r.harness.state_mut().open_volumes_window();
    r.fake.send(BackendEvent::Devices(Err("no system bus".into())));
    r.harness.run_steps(4);
    assert!(r.harness.query_by_label("UDisks2 not available").is_some());
    assert!(r.harness.query_by_label("/dev/sda1").is_some());
    r.harness.get_by_label("Full rescan").click();
    r.harness.run_steps(3);
    assert_eq!(
        r.fake.jobs(),
        [Job::RescanVolume {
            volume: "uuid-root".into(),
            mode: everythux_index::ScanMode::Full
        }]
    );
}

#[test]
fn exclusion_editor_lists_rules_and_explains_a_path() {
    let mut r = rig();
    r.harness.state_mut().open_exclusion_editor();
    r.harness.run_steps(4);
    assert!(r.harness.query_by_label("Restore defaults").is_some());
    r.harness.state_mut().windows.exclusion_editor.test_path = "/home/me/proj/bin/x".to_owned();
    r.harness.run_steps(3);
    assert!(
        r.harness
            .query_by_label_contains("Excluded by rule #8: bin")
            .is_some(),
        "the default `bin` rule scoped to /home/*"
    );
    r.harness.state_mut().windows.exclusion_editor.test_path = "/usr/bin/ls".to_owned();
    r.harness.run_steps(3);
    assert!(r.harness.query_by_label("Not excluded").is_some());
}

// --- Preview pane interaction -------------------------------------------------------------

const WINDOW_WIDTH: f32 = 1200.0;

fn preview_calls(r: &Rig) -> Vec<std::path::PathBuf> {
    r.fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            crate::backend::fake::Call::RequestPreview(p) => Some(p),
            _ => None,
        })
        .collect()
}

fn saved_widths(r: &Rig) -> Vec<f32> {
    r.fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            crate::backend::fake::Call::SaveConfig(c) => Some(c.ui.preview_pane.width),
            _ => None,
        })
        .collect()
}

/// A rig with the preview pane open, `notes.txt` selected and its text preview delivered.
fn preview_rig() -> Rig {
    let mut r = rig();
    r.harness.state_mut().config.ui.preview_pane.enabled = true;
    r.show_default();
    r.click_cell("notes.txt", Modifiers::NONE);
    r.harness.run_steps(15);
    let (path, id) = r
        .harness
        .state()
        .preview
        .requested
        .clone()
        .expect("a preview was requested after the debounce");
    assert_eq!(path, std::path::Path::new("/home/me/docs/notes.txt"));
    r.fake
        .send(BackendEvent::Preview(everythux_preview::PreviewResult {
            request: id,
            path,
            content: everythux_preview::PreviewContent::Text {
                text: "hello from the preview pane".to_owned(),
                truncated: false,
            },
        }));
    r.harness.run_steps(3);
    assert!(r.harness.query_by_label("hello from the preview pane").is_some());
    r.fake.clear();
    r
}

/// Vertical centre of the table row showing `name`.
fn row_y(r: &Rig, name: &str) -> f32 {
    r.harness.get_by_label(name).rect().center().y
}

fn assert_preview_untouched(r: &Rig, what: &str) {
    let app = r.harness.state();
    assert_eq!(app.selection.cursor(), Some(1), "{what}: cursor moved");
    assert_eq!(
        app.selection.rows().collect::<Vec<_>>(),
        [1],
        "{what}: selection changed"
    );
    assert!(
        preview_calls(r).is_empty(),
        "{what}: new preview request: {:?}",
        preview_calls(r)
    );
    assert!(app.preview.shown.is_some(), "{what}: preview dropped");
    assert!(
        r.harness.query_by_label("hello from the preview pane").is_some(),
        "{what}: preview content no longer shown"
    );
}

#[test]
fn clicking_inside_the_preview_pane_keeps_selection_and_preview() {
    let mut r = preview_rig();
    let pane_x = WINDOW_WIDTH - 100.0;
    for y in [150.0, 300.0, 500.0] {
        let pos = egui::pos2(pane_x, y);
        r.harness.hover_at(pos);
        r.harness.run_steps(1);
        r.harness.drag_at(pos);
        r.harness.run_steps(2);
        r.harness.drop_at(pos);
        r.harness.run_steps(15);
        assert_preview_untouched(&r, &format!("click at y={y}"));
    }
    // Clicking the text itself (a selectable label) is part of the same promise.
    r.harness.get_by_label("hello from the preview pane").click();
    r.harness.run_steps(15);
    assert_preview_untouched(&r, "click on the preview text");
}

#[test]
fn resizable_mode_keeps_todays_drag_behaviour() {
    let mut r = preview_rig();
    r.harness.state_mut().config.ui.preview_pane.resizable = true;
    r.harness.run_steps(2);
    let start_width = r.harness.state().config.ui.preview_pane.width;
    let edge_x = WINDOW_WIDTH - start_width;
    let y = 300.0;
    r.harness.hover_at(egui::pos2(edge_x, y));
    r.harness.run_steps(2);
    r.harness.drag_at(egui::pos2(edge_x, y));
    r.harness.run_steps(2);
    for step in 1..=10 {
        r.harness.hover_at(egui::pos2(edge_x - 10.0 * step as f32, y));
        r.harness.run_steps(1);
        assert_preview_untouched(&r, &format!("mid-drag step {step}"));
    }
    assert_eq!(
        saved_widths(&r),
        Vec::<f32>::new(),
        "no config writes while dragging"
    );
    r.harness.drop_at(egui::pos2(edge_x - 100.0, y));
    r.harness.run_steps(15);
    assert_preview_untouched(&r, "after the drag");

    let width = r.harness.state().config.ui.preview_pane.width;
    assert!(
        (width - (start_width + 100.0)).abs() < 12.0,
        "width {width} should follow the pointer from {start_width} (minus the drag threshold)"
    );
    let saved = saved_widths(&r);
    assert_eq!(saved.len(), 1, "persisted exactly once: {saved:?}");
    assert!((saved[0] - width).abs() < 1.0, "persisted the final width");
}

#[test]
fn redelivered_identical_results_keep_the_preview_selection() {
    let mut r = preview_rig();
    let f = &r.fixture;
    let ids = [f.report, f.notes, f.junk];
    r.show(&ids);
    r.harness.run_steps(15);
    assert_preview_untouched(&r, "identical results re-delivered");
}

#[test]
fn clicking_on_the_preview_pane_edge_keeps_selection_and_preview() {
    let mut r = preview_rig();
    let edge_x = WINDOW_WIDTH - r.harness.state().config.ui.preview_pane.width;
    for dx in [-4.0, -2.0, 0.0, 2.0, 4.0] {
        let pos = egui::pos2(edge_x + dx, row_y(&r, "report.pdf"));
        r.harness.hover_at(pos);
        r.harness.run_steps(1);
        r.harness.drag_at(pos);
        r.harness.run_steps(2);
        r.harness.drop_at(pos);
        r.harness.run_steps(15);
        assert_preview_untouched(&r, &format!("click at edge{dx:+}"));
    }
}

#[test]
fn dragging_from_either_side_of_the_edge_keeps_selection_and_preview() {
    for dx in [-4.0, -2.0, 2.0, 4.0] {
        let mut r = preview_rig();
        let edge_x = WINDOW_WIDTH - r.harness.state().config.ui.preview_pane.width;
        let y = row_y(&r, "report.pdf");
        r.harness.hover_at(egui::pos2(edge_x + dx, y));
        r.harness.run_steps(2);
        r.harness.drag_at(egui::pos2(edge_x + dx, y));
        r.harness.run_steps(2);
        for step in 1..=10 {
            r.harness
                .hover_at(egui::pos2(edge_x + dx - 10.0 * step as f32, y));
            r.harness.run_steps(1);
        }
        r.harness.drop_at(egui::pos2(edge_x + dx - 100.0, y));
        r.harness.run_steps(15);
        assert_preview_untouched(&r, &format!("drag from edge{dx:+}"));
    }
}

#[test]
fn preview_pane_opens_at_the_configured_width_and_does_not_rewrite_it() {
    let mut r = rig();
    r.harness.state_mut().config.ui.preview_pane.enabled = true;
    r.harness.state_mut().config.ui.preview_pane.width = 400.0;
    r.harness.run_steps(5);
    r.fake.clear();
    r.show_default();
    r.click_cell("notes.txt", Modifiers::NONE);
    r.harness.run_steps(15);
    let width = r.harness.state().config.ui.preview_pane.width;
    assert!((width - 400.0).abs() < f32::EPSILON, "width drifted to {width}");
    assert!(
        saved_widths(&r).is_empty(),
        "nothing to save: {:?}",
        saved_widths(&r)
    );
    let panel = r.harness.get_by_label("Open").rect();
    assert!(
        panel.max.x <= WINDOW_WIDTH && panel.min.x >= WINDOW_WIDTH - 400.0,
        "pane content must sit inside the 400px pane: {panel:?}"
    );
}

#[test]
fn shrinking_the_window_does_not_overwrite_the_preferred_pane_width() {
    let mut r = rig();
    r.harness.state_mut().config.ui.preview_pane.enabled = true;
    r.harness.run_steps(5);
    r.harness.set_size(Vec2::new(420.0, 600.0));
    r.harness.run_steps(5);
    r.harness.set_size(Vec2::new(WINDOW_WIDTH, 600.0));
    r.harness.run_steps(5);
    let width = r.harness.state().config.ui.preview_pane.width;
    assert!((width - 360.0).abs() < f32::EPSILON, "width drifted to {width}");
    assert_eq!(saved_widths(&r), Vec::<f32>::new());
}

#[test]
fn a_background_reindex_keeps_the_selected_entry_and_its_preview() {
    let mut r = preview_rig();
    // Same query, freshly built snapshot, the selected entry now sits in another row.
    let fresh = Fixture::new();
    let generation = r.harness.state().search.latest_generation;
    r.fake.send_results(
        generation,
        &fresh.snapshot,
        &[fresh.junk, fresh.report, fresh.notes],
        None,
    );
    r.harness.run_steps(15);
    let app = r.harness.state();
    assert_eq!(
        app.selection.rows().collect::<Vec<_>>(),
        [2],
        "followed notes.txt"
    );
    assert!(preview_calls(&r).is_empty(), "{:?}", preview_calls(&r));
    assert!(app.preview.shown.is_some(), "preview dropped");
    assert!(r.harness.query_by_label("hello from the preview pane").is_some());
}

// --- Fixed preview pane width (v0.5 Phase A) ------------------------------------------------

fn pane_width(r: &Rig) -> f32 {
    r.harness.state().preview.pane_rect.expect("pane shown").width()
}

fn assert_pane_width(r: &Rig, expected: f32, what: &str) {
    let app = r.harness.state();
    let pane = app.preview.pane_rect.expect("pane shown");
    let content = app.preview.content_rect.expect("pane content");
    assert!(
        (pane.width() - expected).abs() <= 1.0,
        "{what}: pane is {} px wide, expected {expected}",
        pane.width()
    );
    let screen = r.harness.ctx.content_rect();
    assert!(
        pane.max.x <= screen.max.x + 1.0,
        "{what}: the pane {pane:?} was pushed out of the window {screen:?}"
    );
    assert!(
        content.min.x >= pane.min.x - 1.0 && content.max.x <= pane.max.x + 1.0,
        "{what}: content {content:?} overflows the pane {pane:?}"
    );
}

fn rgba(width: u32, height: u32) -> everythux_preview::ImageData {
    everythux_preview::ImageData {
        width,
        height,
        rgba: vec![200; (width * height * 4) as usize],
    }
}

#[test]
fn fixed_width_pane_has_exact_width_for_every_content_kind() {
    use everythux_preview::{MediaMeta, PreviewContent, PreviewResult};

    let mut r = rig();
    r.harness.state_mut().config.ui.preview_pane.enabled = true;
    let long_name = format!("{}.txt", "a-very-long-file-name-".repeat(14));
    let mut b = crate::testkit::SnapshotBuilder::new();
    let dir = b.dir(&format!("/home/me/{}", "deeply-nested-folder/".repeat(12)), None);
    let entry = b.file(dir, &long_name, 1234, 1_700_000_000_000_000_000);
    let snapshot = b.finish();
    let generation = r.harness.state().search.latest_generation;
    r.fake.send_results(generation, &snapshot, &[entry], None);
    r.harness.run_steps(3);
    r.harness.state_mut().selection.select_only(0);
    r.harness.run_steps(20);
    let (path, id) = r.harness.state().preview.requested.clone().expect("requested");
    assert_pane_width(&r, 360.0, "loading");

    let long_meta = MediaMeta {
        title: Some("A title that goes on and on ".repeat(20)),
        artist: Some("Someone with a remarkably long name ".repeat(10)),
        album: None,
        duration: Some(std::time::Duration::from_secs(4000)),
        extra: vec![("Codec".to_owned(), "x".repeat(500))],
    };
    let contents = [
        PreviewContent::Image {
            image: rgba(2000, 40),
            original_size: Some((2000, 40)),
        },
        PreviewContent::Pdf {
            page: rgba(2000, 80),
            pages: Some(1),
        },
        PreviewContent::Text {
            text: format!("{}\nshort line\n", "0123456789".repeat(1000)),
            truncated: false,
        },
        PreviewContent::Audio {
            meta: long_meta.clone(),
            cover: Some(rgba(2000, 2000)),
        },
        PreviewContent::Video {
            meta: long_meta,
            thumbnail: Some(rgba(2000, 100)),
        },
        PreviewContent::Folder { children: Some(12) },
        PreviewContent::Unavailable {
            hint: "A hint that is much longer than the pane is wide ".repeat(10),
        },
    ];
    for content in contents {
        let what = format!("{content:?}").chars().take(40).collect::<String>();
        r.fake.send(BackendEvent::Preview(PreviewResult {
            request: id,
            path: path.clone(),
            content,
        }));
        r.harness.run_steps(3);
        assert_pane_width(&r, 360.0, &what);
    }
}

#[test]
fn fixed_width_pane_has_no_resize_handle() {
    let mut r = preview_rig();
    let edge_x = WINDOW_WIDTH - 360.0;
    let y = 300.0;
    r.harness.hover_at(egui::pos2(edge_x, y));
    r.harness.run_steps(2);
    r.harness.drag_at(egui::pos2(edge_x, y));
    r.harness.run_steps(2);
    for step in 1..=10 {
        r.harness.hover_at(egui::pos2(edge_x - 10.0 * step as f32, y));
        r.harness.run_steps(1);
        assert_pane_width(&r, 360.0, &format!("drag step {step}"));
    }
    r.harness.drop_at(egui::pos2(edge_x - 100.0, y));
    r.harness.run_steps(15);
    assert_pane_width(&r, 360.0, "after the drag");
    assert!((r.harness.state().config.ui.preview_pane.width - 360.0).abs() < f32::EPSILON);
    assert_eq!(saved_widths(&r), Vec::<f32>::new(), "nothing saved");
    assert_preview_untouched(&r, "after the drag");
}

#[test]
fn choosing_a_preset_sets_and_saves_the_width_once() {
    let mut r = preview_rig();
    r.harness.get_by_label("View").click();
    r.harness.run_steps(3);
    r.harness.get_by_label("Preview pane ⏵").click();
    r.harness.run_steps(3);
    r.harness.get_by_label("Wide (520 px)").click();
    r.harness.run_steps(5);
    assert!((r.harness.state().config.ui.preview_pane.width - 520.0).abs() < f32::EPSILON);
    assert_eq!(saved_widths(&r), [520.0], "saved exactly once");
    assert_pane_width(&r, 520.0, "after choosing Wide");
}

#[test]
fn a_preset_also_applies_in_resizable_mode() {
    let mut r = preview_rig();
    r.harness.state_mut().config.ui.preview_pane.resizable = true;
    r.harness.run_steps(3);
    r.harness
        .state_mut()
        .run_command(crate::app::Command::SetPreviewWidth(720));
    r.harness.run_steps(3);
    assert_pane_width(&r, 720.0, "resizable pane follows the menu");
}

#[test]
fn custom_width_window_opens_and_closes_with_escape() {
    let mut r = preview_rig();
    r.harness
        .state_mut()
        .run_command(crate::app::Command::CustomPreviewWidth);
    r.harness.run_steps(3);
    assert!(r.harness.query_by_label("Preview pane width").is_some());
    r.harness.key_press(Key::Escape);
    r.harness.run_steps(3);
    assert!(!r.harness.state().preview.width_window);
    assert!(r.harness.query_by_label("Preview pane width").is_none());
    assert_eq!(
        saved_widths(&r),
        Vec::<f32>::new(),
        "opening and closing changes nothing"
    );
}

#[test]
fn allow_resizing_checkbox_toggles_and_persists() {
    let mut r = rig();
    r.harness.run_steps(2);
    r.harness.get_by_label("View").click();
    r.harness.run_steps(3);
    r.harness.get_by_label("Preview pane ⏵").click();
    r.harness.run_steps(3);
    r.harness.get_by_label("Allow resizing by dragging").click();
    r.harness.run_steps(3);
    assert!(r.harness.state().config.ui.preview_pane.resizable);
    assert!(
        r.fake
            .last_saved_config()
            .expect("saved")
            .ui
            .preview_pane
            .resizable
    );
}

#[test]
fn narrow_window_shrinks_the_pane_without_changing_config() {
    let mut r = preview_rig();
    r.harness.set_size(Vec2::new(600.0, 600.0));
    r.harness.run_steps(5);
    let squeezed = pane_width(&r);
    assert!(
        (250.0..=300.0).contains(&squeezed),
        "a 600 px window keeps 300 px for the table, pane is {squeezed}"
    );
    // Clicking around while squeezed must not adopt the squeezed width either.
    r.harness
        .query_all_by_label("notes.txt")
        .min_by(|a, b| a.rect().min.x.total_cmp(&b.rect().min.x))
        .expect("the table cell")
        .click();
    r.harness.run_steps(3);
    r.harness.set_size(Vec2::new(WINDOW_WIDTH, 600.0));
    r.harness.run_steps(5);
    assert_pane_width(&r, 360.0, "back to the fixed width");
    assert!((r.harness.state().config.ui.preview_pane.width - 360.0).abs() < f32::EPSILON);
    assert_eq!(saved_widths(&r), Vec::<f32>::new());
}

// --- Inline rename ------------------------------------------------------------------------

mod rename {
    use std::path::PathBuf;

    use egui_kittest::kittest::NodeT as _;

    use super::*;
    use crate::ui::rename_editor::editor_id;

    const NOTES: &str = "/home/me/docs/notes.txt";

    fn rename_action(from: &str, to: &str, undo: bool) -> PlatformAction {
        PlatformAction::Rename {
            from: from.into(),
            new_name: to.into(),
            undo,
        }
    }

    /// Selects `notes.txt` and presses F2.
    fn editing() -> Rig {
        let mut r = rig();
        r.show_default();
        r.click_cell("notes.txt", Modifiers::NONE);
        r.harness.key_press(Key::F2);
        r.harness.run_steps(4);
        r
    }

    fn type_text(r: &mut Rig, text: &str) {
        r.harness.event(egui::Event::Text(text.to_owned()));
        r.harness.run_steps(3);
    }

    /// Replaces the whole text of the editor.
    fn replace_text(r: &mut Rig, text: &str) {
        r.harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        r.harness.run_steps(2);
        if text.is_empty() {
            press(r, Key::Backspace);
        } else {
            type_text(r, text);
        }
    }

    fn press(r: &mut Rig, key: Key) {
        r.harness.key_press(key);
        r.harness.run_steps(3);
    }

    fn buffer(r: &Rig) -> Option<String> {
        r.harness.state().rename.as_ref().map(|e| e.buffer.clone())
    }

    #[test]
    fn f2_opens_the_editor_with_the_stem_selected() {
        let r = editing();
        assert_eq!(buffer(&r).as_deref(), Some("notes.txt"));
        let ctx = &r.harness.ctx;
        assert!(ctx.memory(|m| m.has_focus(editor_id())), "the editor is focused");
        let state = egui::text_edit::TextEditState::load(ctx, editor_id()).expect("editor state");
        let range = state.cursor.char_range().expect("a selection");
        let (a, b) = (range.primary.index.0, range.secondary.index.0);
        assert_eq!(
            (a.min(b), a.max(b)),
            (0, "notes".len()),
            "only the stem is selected"
        );
        let editor = ctx.read_response(editor_id()).expect("editor response");
        assert!(
            editor.rect.height() <= crate::app::ROW_HEIGHT + 0.5,
            "the row does not grow: {}",
            editor.rect.height()
        );
    }

    #[test]
    fn enter_commits_and_submits_platform_rename() {
        let mut r = editing();
        type_text(&mut r, "todo");
        assert_eq!(
            buffer(&r).as_deref(),
            Some("todo.txt"),
            "typing replaces the selected stem"
        );
        press(&mut r, Key::Enter);
        assert_eq!(
            r.fake.platform_actions(),
            [rename_action(NOTES, "todo.txt", false)]
        );
        assert!(r.harness.state().rename.is_none(), "the editor closed");
        assert!(
            r.fake.platform_actions().len() == 1 && r.fake.jobs().is_empty(),
            "the index job waits for the disk result"
        );
    }

    #[test]
    fn tab_commits_too() {
        let mut r = editing();
        type_text(&mut r, "todo");
        press(&mut r, Key::Tab);
        assert_eq!(
            r.fake.platform_actions(),
            [rename_action(NOTES, "todo.txt", false)]
        );
    }

    #[test]
    fn esc_cancels_without_side_effects() {
        let mut r = editing();
        r.harness.state_mut().search.text = "abc".to_owned();
        type_text(&mut r, "todo");
        press(&mut r, Key::Escape);
        assert!(r.harness.state().rename.is_none());
        assert!(
            r.fake.platform_actions().is_empty(),
            "{:?}",
            r.fake.platform_actions()
        );
        assert_eq!(r.fake.jobs().len(), 0, "{:?}", r.fake.jobs());
        assert_eq!(
            r.harness.state().search.text,
            "abc",
            "Escape did not reach the search box"
        );
        assert!(
            r.harness.query_by_label("notes.txt").is_some(),
            "the old name is back"
        );
    }

    #[test]
    fn focus_loss_commits() {
        let mut r = editing();
        type_text(&mut r, "todo");
        let path_cell = r
            .harness
            .get_all_by_label("/home/me/docs")
            .next()
            .expect("a path cell");
        path_cell.click();
        r.harness.run_steps(4);
        assert_eq!(
            r.fake.platform_actions(),
            [rename_action(NOTES, "todo.txt", false)]
        );
        assert!(r.harness.state().rename.is_none());
    }

    #[test]
    fn unchanged_name_is_a_no_op() {
        let mut r = editing();
        press(&mut r, Key::Enter);
        assert!(r.harness.state().rename.is_none());
        assert_eq!(
            r.fake.platform_actions().len(),
            0,
            "{:?}",
            r.fake.platform_actions()
        );

        // Also via focus loss.
        r.harness.key_press(Key::F2);
        r.harness.run_steps(4);
        assert!(r.harness.state().rename.is_some());
        r.harness
            .get_all_by_label("/home/me/docs")
            .next()
            .expect("path cell")
            .click();
        r.harness.run_steps(4);
        assert!(r.harness.state().rename.is_none());
        assert_eq!(
            r.fake.platform_actions().len(),
            0,
            "{:?}",
            r.fake.platform_actions()
        );
        assert_eq!(r.fake.jobs().len(), 0, "{:?}", r.fake.jobs());
    }

    #[test]
    fn invalid_names_show_inline_errors() {
        let mut r = editing();
        for (text, message) in [
            ("a/b", "A name can't contain \"/\""),
            ("..", "\"..\" is not a valid name"),
            ("", "The name can't be empty"),
        ] {
            replace_text(&mut r, text);
            assert!(
                r.harness.query_by_label(message).is_some(),
                "{text:?} should show {message:?}"
            );
            press(&mut r, Key::Enter);
            assert!(
                r.harness.state().rename.is_some(),
                "{text:?}: the editor stays open"
            );
            assert!(
                r.fake.platform_actions().is_empty(),
                "{text:?}: nothing submitted"
            );
        }
        replace_text(&mut r, &"x".repeat(256));
        assert!(
            r.harness
                .query_by_label_contains("The name is too long")
                .is_some()
        );
        replace_text(&mut r, "fine.txt");
        assert!(
            r.harness.query_by_label_contains("can't").is_none()
                && r.harness.query_by_label_contains("too long").is_none(),
            "the error goes away once the name is valid"
        );
    }

    #[test]
    fn extension_change_needs_confirmation() {
        let mut r = editing();
        replace_text(&mut r, "notes.md");
        press(&mut r, Key::Enter);
        assert!(
            r.fake.platform_actions().is_empty(),
            "the first Enter only warns: {:?}",
            r.fake.platform_actions()
        );
        assert!(r.harness.state().rename.is_some());
        assert!(
            r.harness
                .query_by_label_contains("Change the extension from .txt to .md?")
                .is_some()
        );
        press(&mut r, Key::Enter);
        assert_eq!(
            r.fake.platform_actions(),
            [rename_action(NOTES, "notes.md", false)]
        );
    }

    #[test]
    fn editing_after_a_warning_asks_again() {
        let mut r = editing();
        replace_text(&mut r, "notes.md");
        press(&mut r, Key::Enter);
        type_text(&mut r, "x");
        press(&mut r, Key::Enter);
        assert!(
            r.fake.platform_actions().is_empty(),
            "a changed text needs its own confirmation"
        );
        press(&mut r, Key::Enter);
        assert_eq!(
            r.fake.platform_actions(),
            [rename_action(NOTES, "notes.mdx", false)]
        );
    }

    #[test]
    fn focus_loss_does_not_confirm_a_warning() {
        let mut r = editing();
        replace_text(&mut r, " notes.txt");
        r.harness
            .get_all_by_label("/home/me/docs")
            .next()
            .expect("path cell")
            .click();
        r.harness.run_steps(4);
        assert_eq!(
            r.fake.platform_actions().len(),
            0,
            "{:?}",
            r.fake.platform_actions()
        );
        assert!(
            r.harness.state().rename.is_some(),
            "the editor stays open and warns"
        );
    }

    #[test]
    fn undo_toast_renames_back() {
        let mut r = editing();
        type_text(&mut r, "todo");
        press(&mut r, Key::Enter);
        r.fake.send(BackendEvent::Renamed {
            from: NOTES.into(),
            new_name: "todo.txt".into(),
            undo: false,
            result: Ok("/home/me/docs/todo.txt".into()),
        });
        r.harness.run_steps(3);
        assert!(r.harness.query_by_label("Renamed to todo.txt").is_some());
        r.harness.get_by_label("Undo").click();
        r.harness.run_steps(3);
        assert_eq!(
            r.fake.platform_actions().last(),
            Some(&rename_action("/home/me/docs/todo.txt", "notes.txt", true))
        );
        r.fake.send(BackendEvent::Renamed {
            from: "/home/me/docs/todo.txt".into(),
            new_name: "notes.txt".into(),
            undo: true,
            result: Ok(NOTES.into()),
        });
        r.harness.run_steps(3);
        assert_eq!(
            r.fake.jobs().last(),
            Some(&Job::Renamed {
                from: "/home/me/docs/todo.txt".into(),
                to: NOTES.into()
            })
        );
        assert!(r.harness.query_by_label("Renamed back to notes.txt").is_some());
    }

    #[test]
    fn typing_in_the_editor_does_not_trigger_global_shortcuts() {
        let mut r = editing();
        r.harness.state_mut().search.text = "abc".to_owned();
        let cursor_before = r.harness.state().selection.cursor();
        // F5 (quick rescan), Ctrl+P (preview), Ctrl+C / copy, arrows, Home/End, plain typing.
        press(&mut r, Key::F5);
        r.harness.key_press_modifiers(Modifiers::CTRL, Key::P);
        r.harness.run_steps(2);
        r.harness.event(egui::Event::Copy);
        r.harness.run_steps(2);
        press(&mut r, Key::ArrowDown);
        press(&mut r, Key::Home);
        type_text(&mut r, "z");
        let app = r.harness.state();
        assert!(r.fake.jobs().is_empty(), "F5 did not rescan: {:?}", r.fake.jobs());
        assert!(!app.config.ui.preview_pane.enabled, "Ctrl+P did nothing");
        assert!(app.toasts.is_empty(), "nothing was copied: {:?}", app.toasts);
        assert_eq!(
            app.selection.cursor(),
            cursor_before,
            "arrows stayed in the editor"
        );
        assert_eq!(app.search.text, "abc", "typing did not go to the search box");
        assert_eq!(
            buffer(&r).as_deref(),
            Some("znotes.txt"),
            "Home moved the caret, then `z` was typed"
        );
        assert!(
            r.fake.platform_actions().is_empty(),
            "Enter-like actions did not fire"
        );

        // Ctrl+Shift+R is the one global shortcut that still works.
        r.harness
            .key_press_modifiers(Modifiers::CTRL | Modifiers::SHIFT, Key::R);
        r.harness.run_steps(3);
        assert_eq!(r.fake.jobs(), [Job::FullReindex]);
        assert!(r.harness.state().rename.is_some(), "the editor is still open");
    }

    #[test]
    fn rename_disabled_on_offline_volumes() {
        let mut r = rig();
        let mut snap = (*r.fixture.snapshot).clone();
        snap.volumes[0].state = everythux_core::model::VolumeState::Offline;
        r.fixture.snapshot = std::sync::Arc::new(snap);
        r.show_default();
        r.click_cell("notes.txt", Modifiers::NONE);
        r.harness.key_press(Key::F2);
        r.harness.run_steps(3);
        assert!(r.harness.state().rename.is_none(), "F2 does nothing");
        assert!(
            r.harness
                .state()
                .toasts
                .iter()
                .any(|t| t.text.contains("offline")),
            "with a hint: {:?}",
            r.harness.state().toasts
        );

        // The context menu entry is there but greyed out.
        r.harness.get_by_label("notes.txt").click_secondary();
        r.harness.run_steps(4);
        let item = r.harness.get_by_label_contains("Rename…");
        assert!(item.accesskit_node().is_disabled(), "the menu item is disabled");
    }

    #[test]
    fn context_menu_rename_opens_the_editor() {
        let mut r = rig();
        r.show_default();
        r.harness.get_by_label("notes.txt").click_secondary();
        r.harness.run_steps(4);
        r.harness.get_by_label_contains("Rename…").click();
        r.harness.run_steps(4);
        let app = r.harness.state();
        assert_eq!(
            app.rename.as_ref().map(|e| e.path.clone()),
            Some(PathBuf::from(NOTES))
        );
        assert_eq!(app.selection.rows().collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn f2_with_several_rows_selected_does_nothing() {
        let mut r = rig();
        r.show_default();
        r.click_cell("report.pdf", Modifiers::NONE);
        r.click_cell("notes.txt", Modifiers::SHIFT);
        r.harness.key_press(Key::F2);
        r.harness.run_steps(3);
        assert!(r.harness.state().rename.is_none());
    }

    #[test]
    fn editor_survives_unrelated_frames_and_new_results() {
        let mut r = editing();
        type_text(&mut r, "to");
        // The same query is re-run against a newer snapshot with another order.
        let f = &r.fixture;
        let ids = [f.junk, f.notes, f.report];
        r.show(&ids);
        let edit = r.harness.state().rename.as_ref().expect("still editing");
        assert_eq!(edit.row, 1, "the editor followed its entry");
        assert_eq!(edit.buffer, "to.txt");
    }
    /// The docs folder after `notes.txt` became `todo.txt`: the new name sorts last.
    fn after_rename() -> (
        std::sync::Arc<everythux_core::snapshot::IndexSnapshot>,
        Vec<everythux_core::ids::EntryId>,
    ) {
        let mut b = crate::testkit::SnapshotBuilder::new();
        let home = b.dir("/home/me", None);
        let docs = b.dir("/home/me/docs", Some(home));
        let report = b.file(docs, "report.pdf", 2048, 1);
        let todo = b.file(docs, "todo.txt", 10, 2);
        let junk = b.file(docs, "aaa.tmp", 1, 1);
        (b.finish(), vec![report, junk, todo])
    }

    /// Delivers the worker's answer and the refreshed results, like the real backend does.
    fn finish_rename(r: &mut Rig) {
        r.fake.send(BackendEvent::Renamed {
            from: NOTES.into(),
            new_name: "todo.txt".into(),
            undo: false,
            result: Ok("/home/me/docs/todo.txt".into()),
        });
        r.harness.run_steps(3);
        assert_eq!(
            r.fake.jobs(),
            [Job::Renamed {
                from: NOTES.into(),
                to: "/home/me/docs/todo.txt".into()
            }],
            "the index is told about the rename"
        );
        let (snap, ids) = after_rename();
        let generation = r.harness.state().search.latest_generation;
        r.fake.send_results(generation, &snap, &ids, None);
        r.harness.run_steps(4);
    }

    fn assert_renamed_and_reselected(r: &Rig) {
        let app = r.harness.state();
        assert!(app.rename.is_none());
        assert_eq!(
            app.selection.rows().collect::<Vec<_>>(),
            [2],
            "todo.txt is selected"
        );
        assert!(r.harness.query_by_label("todo.txt").is_some(), "new name shown");
        assert!(r.harness.query_by_label("notes.txt").is_none(), "old name gone");
    }

    #[test]
    fn whole_flow_from_a_fresh_app_through_events_only() {
        let mut r = rig();
        r.show_default();
        r.click_cell("notes.txt", Modifiers::NONE);
        r.harness.key_press(Key::F2);
        r.harness.run_steps(4);
        assert!(r.harness.ctx.memory(|m| m.has_focus(editor_id())));
        type_text(&mut r, "todo");
        press(&mut r, Key::Enter);
        assert_eq!(
            r.fake.platform_actions(),
            [rename_action(NOTES, "todo.txt", false)],
            "Enter renamed and did not also open the file"
        );
        finish_rename(&mut r);
        assert_renamed_and_reselected(&r);
    }

    #[test]
    fn arrow_keys_then_f2_with_the_search_box_focused() {
        let mut r = rig();
        r.harness.run_steps(3);
        r.harness.get_by_role(egui::accesskit::Role::TextInput).focus();
        r.harness.run_steps(2);
        r.harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .type_text("txt");
        r.harness.run_steps(3);
        r.show_default();
        assert!(r.harness.ctx.memory(|m| m.has_focus(crate::app::search_id())));
        press(&mut r, Key::ArrowDown);
        press(&mut r, Key::ArrowDown);
        press(&mut r, Key::F2);
        assert_eq!(buffer(&r).as_deref(), Some("notes.txt"), "F2 opened the editor");
        assert!(r.harness.ctx.memory(|m| m.has_focus(editor_id())));
        type_text(&mut r, "todo");
        assert_eq!(r.harness.state().search.text, "txt", "the search box got no text");
        assert_eq!(buffer(&r).as_deref(), Some("todo.txt"));
        press(&mut r, Key::Enter);
        assert_eq!(
            r.fake.platform_actions(),
            [rename_action(NOTES, "todo.txt", false)]
        );
        finish_rename(&mut r);
        assert_renamed_and_reselected(&r);
    }

    #[test]
    fn context_menu_rename_then_typing_and_enter() {
        let mut r = rig();
        r.show_default();
        r.harness.get_by_label("notes.txt").click_secondary();
        r.harness.run_steps(4);
        r.harness.get_by_label_contains("Rename…").click();
        r.harness.run_steps(4);
        assert!(
            r.harness.ctx.memory(|m| m.has_focus(editor_id())),
            "editor focused"
        );
        type_text(&mut r, "todo");
        press(&mut r, Key::Enter);
        assert_eq!(
            r.fake.platform_actions(),
            [rename_action(NOTES, "todo.txt", false)]
        );
        finish_rename(&mut r);
        assert_renamed_and_reselected(&r);
    }

    #[test]
    fn escape_then_f2_again_works() {
        let mut r = editing();
        press(&mut r, Key::Escape);
        assert!(r.harness.state().rename.is_none());
        press(&mut r, Key::F2);
        assert!(r.harness.state().rename.is_some(), "reopened");
        assert!(r.harness.ctx.memory(|m| m.has_focus(editor_id())));
    }
}
