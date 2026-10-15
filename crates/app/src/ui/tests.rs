//! End-to-end UI tests: the real `App` rendered by `egui_kittest` with a `FakeBackend`.
//! They assert on effects (backend calls, state), not on pixels.

use egui::{Key, Modifiers, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::NodeT as _;
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
    assert_eq!(r.harness.state().search.first().text, "report");
    assert_eq!(r.fake.searches().last().expect("a search").texts, ["report"]);
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
    assert_eq!(
        r.harness.state().search.first().text,
        "abc",
        "the box kept its text"
    );
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
    assert_eq!(r.harness.state().search.first().text, "");
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
            content: everythux_preview::PreviewContent::text("hello from the preview pane"),
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
        PreviewContent::text(format!("{}\nshort line\n", "0123456789".repeat(1000))),
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
    // Wide text without wrapping (and with line numbers) scrolls sideways inside the pane.
    let pane = &mut r.harness.state_mut().config.ui.preview_pane;
    pane.wrap_text = false;
    pane.line_numbers = true;
    r.fake.send(BackendEvent::Preview(PreviewResult {
        request: id,
        path,
        content: PreviewContent::text("0123456789".repeat(1000)),
    }));
    r.harness.run_steps(3);
    assert_pane_width(&r, 360.0, "unwrapped text");
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

// --- Text preview (v0.5 Phase B) -------------------------------------------------------------

/// Clipboard writes of the next frame.
fn copied_next_frame(r: &mut Rig) -> Vec<String> {
    r.harness.step();
    r.harness
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The selection and preview request state did not change (text previews have their own labels,
/// unlike `assert_preview_untouched`).
fn assert_still_previewing_notes(r: &Rig, what: &str) {
    let app = r.harness.state();
    assert_eq!(
        app.selection.rows().collect::<Vec<_>>(),
        [1],
        "{what}: selection changed"
    );
    assert!(preview_calls(r).is_empty(), "{what}: new preview request");
    assert!(
        app.preview.shown.as_ref().is_some_and(|s| s.text.is_some()),
        "{what}: text preview dropped"
    );
}

/// A rig previewing `notes.txt` with `content` (request id and path filled in).
fn text_rig(content: everythux_preview::PreviewContent) -> Rig {
    let mut r = preview_rig();
    let (path, id) = r.harness.state().preview.requested.clone().expect("requested");
    r.fake
        .send(BackendEvent::Preview(everythux_preview::PreviewResult {
            request: id,
            path,
            content,
        }));
    r.harness.run_steps(3);
    r.fake.clear();
    r
}

#[test]
fn ctrl_c_copies_the_selected_preview_text_not_the_path() {
    let mut r = text_rig(everythux_preview::PreviewContent::text(
        "first line\nsecond line\n",
    ));
    r.harness.get_by_label("first line").click();
    r.harness.run_steps(2);
    assert!(
        r.harness
            .ctx
            .memory(|m| m.has_focus(crate::ui::text_view::text_view_id())),
        "clicking the text focuses it"
    );
    r.harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    r.harness.run_steps(1);
    r.harness.event(egui::Event::Copy);
    assert_eq!(copied_next_frame(&mut r), ["first line\nsecond line\n"]);
    // The same through the key event (backends that do not synthesise `Event::Copy`), sent as
    // one event so the copy lands in the frame `copied_next_frame` looks at.
    r.harness.event(egui::Event::Key {
        key: Key::C,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    });
    assert_eq!(copied_next_frame(&mut r), ["first line\nsecond line\n"]);
}

#[test]
fn right_click_menu_selects_all_and_copies_the_preview_text() {
    let mut r = text_rig(everythux_preview::PreviewContent::text(
        "first line\nsecond line\n",
    ));
    r.harness.get_by_label("first line").click_secondary();
    r.harness.run_steps(4);
    r.harness.get_by_label("Select all").click();
    r.harness.run_steps(3);
    assert!(r.harness.state().preview_text_selected());
    r.harness.get_by_label("first line").click_secondary();
    r.harness.run_steps(4);
    // The toolbar has a "Copy" button too; the menu entry is the one drawn last.
    r.harness
        .query_all_by_label("Copy")
        .last()
        .expect("menu entry")
        .click();
    assert_eq!(copied_next_frame(&mut r), ["first line\nsecond line\n"]);
}

#[test]
fn toolbar_copy_button_copies_the_selection() {
    let mut r = text_rig(everythux_preview::PreviewContent::text(
        "first line\nsecond line\n",
    ));
    assert!(
        r.harness.get_by_label("Copy").accesskit_node().is_disabled(),
        "nothing selected yet"
    );
    r.harness.get_by_label("first line").click();
    r.harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    r.harness.run_steps(2);
    r.harness.get_by_label("Copy").click();
    assert_eq!(copied_next_frame(&mut r), ["first line\nsecond line\n"]);
}

#[test]
fn ctrl_c_with_the_table_focused_still_copies_the_path() {
    let mut r = text_rig(everythux_preview::PreviewContent::text(
        "first line\nsecond line\n",
    ));
    r.harness.get_by_label("first line").click();
    r.harness.run_steps(1);
    r.harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    r.harness.run_steps(1);
    assert!(r.harness.state().preview_text_selected());
    // Back to the table: the click ends the text selection and takes the focus away.
    r.harness
        .query_all_by_label("notes.txt")
        .min_by(|a, b| a.rect().min.x.total_cmp(&b.rect().min.x))
        .expect("the table cell")
        .click();
    r.harness.run_steps(2);
    // As in `copy_event_copies_the_selected_paths`: a real window frees the search box focus.
    r.harness
        .ctx
        .memory_mut(|m| m.surrender_focus(crate::app::search_id()));
    r.harness.run_steps(1);
    assert!(!r.harness.state().preview_text_selected());
    r.harness.event(egui::Event::Copy);
    assert_eq!(copied_next_frame(&mut r), ["/home/me/docs/notes.txt"]);
}

#[test]
fn dragging_over_the_text_selects_it() {
    let mut r = text_rig(everythux_preview::PreviewContent::text(
        "alpha beta\ngamma delta\n",
    ));
    let first = r.harness.get_by_label("alpha beta").rect();
    let second = r.harness.get_by_label("gamma delta").rect();
    let start = egui::pos2(first.min.x + 1.0, first.center().y);
    let end = egui::pos2(second.max.x - 1.0, second.center().y);
    r.harness.hover_at(start);
    r.harness.run_steps(1);
    r.harness.drag_at(start);
    r.harness.run_steps(1);
    for step in 1..=5 {
        r.harness.hover_at(start.lerp(end, step as f32 / 5.0));
        r.harness.run_steps(1);
    }
    r.harness.drop_at(end);
    r.harness.run_steps(2);
    let doc = r
        .harness
        .state()
        .preview
        .shown
        .as_ref()
        .and_then(|s| s.text.clone());
    let selected = doc.as_ref().and_then(|d| d.selected_text().map(str::to_owned));
    assert_eq!(selected.as_deref(), Some("alpha beta\ngamma delta"));
    assert_still_previewing_notes(&r, "selecting text");
}

#[test]
fn wrap_and_line_number_toggles_are_persisted() {
    let mut r = text_rig(everythux_preview::PreviewContent::text("x\n"));
    assert!(
        r.harness.state().config.ui.preview_pane.wrap_text,
        "wrap is the default"
    );
    r.harness.get_by_label("Wrap").click();
    r.harness.run_steps(2);
    r.harness.get_by_label("Line numbers").click();
    r.harness.run_steps(2);
    let pane = &r.harness.state().config.ui.preview_pane;
    assert!(!pane.wrap_text && pane.line_numbers);
    let saved = r.fake.last_saved_config().expect("saved");
    assert!(!saved.ui.preview_pane.wrap_text && saved.ui.preview_pane.line_numbers);
}

#[test]
fn header_names_the_encoding_and_show_more_loads_the_next_range() {
    use everythux_preview::{PreviewContent, PreviewResult, TEXT_MORE_BYTES, TextEncoding};

    let head = "line\n".repeat(13_107); // 65 535 bytes
    let mut r = text_rig(PreviewContent::Text {
        text: head.clone(),
        truncated: true,
        encoding: TextEncoding::Utf16Le,
        total_bytes: 1_258_291,
        end_offset: 131_072,
    });
    assert!(
        r.harness
            .query_by_label("UTF-16 LE · 1.2 MiB · showing first 128 KiB")
            .is_some()
    );
    r.harness.get_by_label("Show more (next 256 KiB)").click();
    r.harness.run_steps(2);
    let path = std::path::PathBuf::from("/home/me/docs/notes.txt");
    let range_calls: Vec<_> = r
        .fake
        .calls()
        .into_iter()
        .filter(|c| matches!(c, crate::backend::fake::Call::RequestTextRange { .. }))
        .collect();
    assert_eq!(
        range_calls,
        [crate::backend::fake::Call::RequestTextRange {
            path: path.clone(),
            offset: 131_072,
            len: TEXT_MORE_BYTES,
            encoding: TextEncoding::Utf16Le,
        }]
    );
    assert!(r.harness.query_by_label("Loading more…").is_some());
    let id = r.harness.state().preview.more_request.expect("pending");
    r.fake.send(BackendEvent::Preview(PreviewResult {
        request: id,
        path,
        content: PreviewContent::Text {
            text: "the rest\n".to_owned(),
            truncated: false,
            encoding: TextEncoding::Utf16Le,
            total_bytes: 1_258_291,
            end_offset: 1_258_291,
        },
    }));
    r.harness.run_steps(3);
    let app = r.harness.state();
    let doc = app
        .preview
        .shown
        .as_ref()
        .and_then(|s| s.text.as_ref())
        .expect("text");
    assert_eq!(doc.rows().len(), 13_107 + 2, "appended (plus the empty last row)");
    assert!(
        r.harness.query_by_label("UTF-16 LE · 1.2 MiB").is_some(),
        "complete now"
    );
    assert!(r.harness.query_by_label_contains("Show more").is_none());
    assert_still_previewing_notes(&r, "show more");
}

#[test]
fn large_text_renders_only_the_rows_in_view() {
    let line = "2026-10-14 12:00:00 INFO worker finished a job in 3 ms\n";
    let text = line.repeat(8 * 1024 * 1024 / line.len());
    let r = text_rig(everythux_preview::PreviewContent::text(text));
    let labels = r.harness.query_all_by_label_contains("worker finished").count();
    assert!(
        (5..200).contains(&labels),
        "{labels} rows laid out; only the visible ones should be"
    );
}

#[test]
#[ignore = "timing report: cargo test --release -p everythux-app -- --ignored --nocapture text_preview_timing"]
fn text_preview_timing() {
    use std::time::Instant;

    use everythux_preview::TextEncoding;

    let line = "2026-10-14 12:00:00 INFO worker-7 finished job “import” in 3 ms — ok\n";
    let text = line.repeat(8 * 1024 * 1024 / line.len());
    let len = text.len() as u64;
    let start = Instant::now();
    let mut doc = crate::text_doc::TextDoc::new(text.clone(), TextEncoding::Utf8, len, len);
    let index = start.elapsed();
    let start = Instant::now();
    doc.layout(45);
    let wrap = start.elapsed();
    let start = Instant::now();
    doc.layout(crate::text_doc::MAX_ROW_COLUMNS);
    let no_wrap = start.elapsed();

    let mut r = text_rig(everythux_preview::PreviewContent::text(text));
    r.harness.run_steps(5);
    let frames = 100;
    let start = Instant::now();
    for i in 0..frames {
        // Scroll a page down on every other frame, so rows are laid out afresh.
        if i % 2 == 0 {
            r.harness.hover_at(egui::pos2(WINDOW_WIDTH - 150.0, 300.0));
            r.harness.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Page,
                delta: egui::vec2(0.0, -1.0),
                modifiers: Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            });
        }
        r.harness.step();
    }
    let per_frame = start.elapsed() / frames;
    eprintln!(
        "8 MiB text ({} lines): index {index:?}, wrap layout {wrap:?}, no-wrap layout {no_wrap:?}, \
         frame while scrolling {per_frame:?} (kittest, includes AccessKit)",
        doc.line_count()
    );
}

// --- Inline rename ------------------------------------------------------------------------

mod rename {
    use std::path::PathBuf;

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
        r.harness.state_mut().search.first_mut().text = "abc".to_owned();
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
            r.harness.state().search.first().text,
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
        r.harness.state_mut().search.first_mut().text = "abc".to_owned();
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
        assert_eq!(
            app.search.first().text,
            "abc",
            "typing did not go to the search box"
        );
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
        assert_eq!(
            r.harness.state().search.first().text,
            "txt",
            "the search box got no text"
        );
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

mod search_bars {
    use everythux_search::{QueryOptions, SearchError, SearchIndex, Sort, parse_many};

    use super::*;
    use crate::app::bar_widget_id;
    use crate::search_bars::{FIRST_BAR, MAX_SEARCH_BARS};
    use crate::ui::rename_editor::editor_id;

    const ADD: &str = "Add a search bar";
    const CLOSE: &str = "Close this search bar";

    fn ids(r: &Rig) -> Vec<u32> {
        r.harness.state().search.bars.iter().map(|b| b.id).collect()
    }

    fn texts(r: &Rig) -> Vec<String> {
        r.harness.state().search.texts()
    }

    fn focused_bar(r: &Rig) -> Option<u32> {
        let ctx = &r.harness.ctx;
        ids(r)
            .into_iter()
            .find(|&id| ctx.memory(|m| m.has_focus(bar_widget_id(id))))
    }

    fn nothing_focused(r: &Rig) -> bool {
        r.harness.ctx.memory(|m| m.focused().is_none())
    }

    fn focus_bar(r: &mut Rig, id: u32) {
        r.harness.ctx.memory_mut(|m| m.request_focus(bar_widget_id(id)));
        r.harness.run_steps(2);
        assert_eq!(focused_bar(r), Some(id));
    }

    fn type_text(r: &mut Rig, text: &str) {
        r.harness.event(egui::Event::Text(text.to_owned()));
        r.harness.run_steps(2);
    }

    const CTRL_SHIFT: Modifiers = Modifiers::CTRL.plus(Modifiers::SHIFT);

    fn press(r: &mut Rig, modifiers: Modifiers, key: Key) {
        r.harness.key_press_modifiers(modifiers, key);
        r.harness.run_steps(3);
    }

    fn click_plus(r: &mut Rig) {
        r.harness.get_by_label(ADD).click();
        r.harness.run_steps(3);
    }

    /// The app with `n` bars, via the `+` button.
    fn with_bars(n: usize) -> Rig {
        let mut r = rig();
        r.harness.run_steps(3);
        for _ in 1..n {
            click_plus(&mut r);
        }
        assert_eq!(ids(&r).len(), n);
        r
    }

    fn bar_rect(r: &Rig, id: u32) -> egui::Rect {
        r.harness
            .ctx
            .read_response(bar_widget_id(id))
            .expect("the bar is drawn")
            .rect
    }

    #[test]
    fn plus_adds_a_bar_below_and_focuses_it() {
        let mut r = with_bars(1);
        assert_eq!(
            focused_bar(&r),
            Some(FIRST_BAR),
            "the first bar is focused on startup"
        );
        r.fake.clear();
        click_plus(&mut r);
        assert_eq!(ids(&r), [0, 1]);
        assert_eq!(focused_bar(&r), Some(1), "the new bar has the focus");
        let (first, second) = (bar_rect(&r, 0), bar_rect(&r, 1));
        assert!(second.top() >= first.bottom(), "{first:?} then {second:?}");
        assert!((second.right() - first.right()).abs() < 0.5, "same right edge");
        assert_eq!(
            r.fake.searches().last().expect("re-run").texts,
            ["", ""],
            "the search re-runs with the empty bar"
        );
        type_text(&mut r, "pdf");
        assert_eq!(texts(&r), ["", "pdf"], "typing goes to the new bar");
    }

    #[test]
    fn closing_a_middle_bar_keeps_the_others_and_their_focus_ids() {
        let mut r = with_bars(3);
        for (id, text) in [(0, "a"), (1, "b"), (2, "c")] {
            focus_bar(&mut r, id);
            type_text(&mut r, text);
        }
        let closes: Vec<_> = r.harness.get_all_by_label(CLOSE).collect();
        assert_eq!(closes.len(), 2);
        closes[0].click(); // the middle bar's ×
        r.harness.run_steps(3);
        assert_eq!(ids(&r), [0, 2]);
        assert_eq!(texts(&r), ["a", "c"]);
        assert_eq!(focused_bar(&r), Some(FIRST_BAR), "the bar above takes the focus");
        assert_eq!(r.fake.searches().last().expect("re-run").texts, ["a", "c"]);
        // The bottom bar kept its widget id, so focus requests and typing still reach it.
        assert!(r.harness.ctx.read_response(bar_widget_id(2)).is_some());
        focus_bar(&mut r, 2);
        type_text(&mut r, "d");
        assert_eq!(texts(&r), ["a", "cd"]);
    }

    #[test]
    fn first_bar_has_no_close_button() {
        let mut r = with_bars(1);
        assert!(r.harness.query_by_label(CLOSE).is_none());
        assert!(r.harness.query_by_label(ADD).is_some());
        click_plus(&mut r);
        assert_eq!(
            r.harness.get_all_by_label(CLOSE).count(),
            1,
            "only the extra bar has one"
        );
        assert_eq!(
            r.harness.get_all_by_label(ADD).count(),
            1,
            "only the first bar has +"
        );
    }

    #[test]
    fn bars_combine_with_and() {
        let mut r = with_bars(2);
        focus_bar(&mut r, 0);
        type_text(&mut r, "r");
        focus_bar(&mut r, 1);
        type_text(&mut r, "t");
        let request = r.fake.searches().last().expect("a search").clone();
        assert_eq!(request.texts, ["r", "t"]);

        // Answer like the real service would.
        let index = SearchIndex::build(std::sync::Arc::clone(&r.fixture.snapshot));
        let query = parse_many(&request.texts, QueryOptions::default()).expect("parses");
        let hits = index
            .search(&query, Sort::default(), 0, &|| false)
            .expect("not cancelled");
        r.show(&hits);
        let snap = std::sync::Arc::clone(&r.fixture.snapshot);
        let names: Vec<String> = hits
            .iter()
            .map(|&id| String::from_utf8_lossy(snap.name(id)).into_owned())
            .collect();
        assert!(
            names.iter().all(|n| n.contains('r') && n.contains('t')),
            "{names:?}"
        );
        assert!(names.contains(&"report.pdf".to_owned()), "{names:?}");
        assert!(
            !names.contains(&"notes.txt".to_owned()),
            "no r in notes.txt: {names:?}"
        );
        assert_eq!(
            r.harness.state().results.as_ref().expect("results").len(),
            hits.len()
        );
        assert!(r.harness.query_by_label("report.pdf").is_some());
        assert!(r.harness.query_by_label("notes.txt").is_none());
    }

    #[test]
    fn parse_error_is_shown_under_the_failing_bar_only() {
        let mut r = with_bars(3);
        r.show_default();
        let generation = r.harness.state().search.latest_generation;
        let ids_before = r.harness.state().results.as_ref().expect("results").ids.clone();
        r.fake.send_results_with(
            generation,
            &r.fixture.snapshot,
            &ids_before,
            Some(SearchError {
                bar: 1,
                message: "unterminated quote".to_owned(),
            }),
        );
        r.harness.run_steps(3);
        let errors: Vec<bool> = r
            .harness
            .state()
            .search
            .bars
            .iter()
            .map(|b| b.error.is_some())
            .collect();
        assert_eq!(errors, [false, true, false]);
        let rect = r.harness.get_by_label("unterminated quote").rect();
        let (middle, bottom) = (bar_rect(&r, 1), bar_rect(&r, 2));
        assert!(
            middle.bottom() <= rect.top() && rect.bottom() <= bottom.top(),
            "the message sits between the failing bar and the next: {rect:?}"
        );
        assert_eq!(
            r.harness.state().results.as_ref().expect("results").len(),
            3,
            "results stay"
        );
    }

    #[test]
    fn tab_toggles_between_table_and_newest_bar() {
        let mut r = with_bars(3);
        r.show_default();
        r.click_cell("notes.txt", Modifiers::NONE);
        assert!(
            nothing_focused(&r),
            "clicking the list leaves no text field focused"
        );
        assert_eq!(r.harness.state().selection.cursor(), Some(1));

        press(&mut r, Modifiers::NONE, Key::Tab);
        assert_eq!(focused_bar(&r), Some(2), "the newest bar");
        press(&mut r, Modifiers::NONE, Key::Tab);
        assert!(nothing_focused(&r), "back to the list");
        assert_eq!(r.harness.state().selection.cursor(), Some(1), "selection kept");

        focus_bar(&mut r, 1);
        press(&mut r, Modifiers::NONE, Key::Tab);
        assert!(nothing_focused(&r), "from bar 1 to the list");
        assert_eq!(r.harness.state().selection.cursor(), Some(1));

        press(&mut r, Modifiers::SHIFT, Key::Tab);
        assert_eq!(focused_bar(&r), Some(2), "Shift+Tab toggles too");
        press(&mut r, Modifiers::SHIFT, Key::Tab);
        assert!(nothing_focused(&r));

        // Keys typed after Tab reach the list again.
        press(&mut r, Modifiers::NONE, Key::ArrowDown);
        assert_eq!(r.harness.state().selection.cursor(), Some(2));
    }

    #[test]
    fn tab_from_a_bar_without_a_selection_selects_the_first_row() {
        let mut r = with_bars(1);
        r.show_default();
        assert!(r.harness.state().selection.is_empty());
        assert_eq!(focused_bar(&r), Some(FIRST_BAR));
        press(&mut r, Modifiers::NONE, Key::Tab);
        assert!(nothing_focused(&r));
        assert_eq!(r.harness.state().selection.cursor(), Some(0));
        press(&mut r, Modifiers::NONE, Key::Enter);
        assert_eq!(
            r.fake.platform_actions(),
            [PlatformAction::OpenDefault("/home/me/docs/report.pdf".into())]
        );
    }

    #[test]
    fn tab_does_not_cycle_through_table_cells() {
        let mut r = with_bars(2);
        r.show_default();
        r.click_cell("report.pdf", Modifiers::NONE);
        for i in 0..6 {
            press(&mut r, Modifiers::NONE, Key::Tab);
            let focused = r.harness.ctx.memory(egui::Memory::focused);
            let expected = (i % 2 == 0).then(|| bar_widget_id(1));
            assert_eq!(focused, expected, "Tab #{i} only toggles list and newest bar");
        }
        for i in 0..4 {
            press(&mut r, Modifiers::SHIFT, Key::Tab);
            let focused = r.harness.ctx.memory(egui::Memory::focused);
            assert!(
                focused.is_none() || focused == Some(bar_widget_id(1)),
                "Shift+Tab #{i} focused {focused:?}"
            );
        }
        assert_eq!(r.harness.state().selection.cursor(), Some(0));
    }

    #[test]
    fn ctrl_shift_t_and_w_add_and_close_bars() {
        let mut r = with_bars(1);
        type_text(&mut r, "a");
        press(&mut r, CTRL_SHIFT, Key::T);
        assert_eq!(ids(&r), [0, 1]);
        assert_eq!(focused_bar(&r), Some(1));
        type_text(&mut r, "b");
        press(&mut r, CTRL_SHIFT, Key::T);
        type_text(&mut r, "c");
        assert_eq!(texts(&r), ["a", "b", "c"]);

        press(&mut r, CTRL_SHIFT, Key::W);
        assert_eq!(ids(&r), [0, 1]);
        assert_eq!(focused_bar(&r), Some(1), "the bar above");
        press(&mut r, CTRL_SHIFT, Key::W);
        assert_eq!(ids(&r), [0]);
        assert_eq!(focused_bar(&r), Some(FIRST_BAR));
        press(&mut r, CTRL_SHIFT, Key::W);
        assert_eq!(texts(&r), ["a"], "Ctrl+Shift+W never closes the first bar");
        assert_eq!(r.fake.searches().last().expect("a search").texts, ["a"]);

        // Ctrl+Shift+T also works from the list.
        r.show_default();
        r.click_cell("notes.txt", Modifiers::NONE);
        press(&mut r, CTRL_SHIFT, Key::T);
        assert_eq!(ids(&r), [0, 3]);
        assert_eq!(focused_bar(&r), Some(3));
    }

    #[test]
    fn ctrl_t_opens_an_empty_tab_and_ctrl_w_closes_it() {
        let mut r = with_bars(1);
        type_text(&mut r, "a");
        press(&mut r, Modifiers::CTRL, Key::T);
        assert_eq!(r.harness.state().tab_count(), 2);
        assert_eq!(r.harness.state().active_tab, 1);
        assert_eq!(texts(&r), [""], "the new tab has one empty bar");
        assert_eq!(focused_bar(&r), Some(FIRST_BAR));
        type_text(&mut r, "b");

        press(&mut r, Modifiers::CTRL, Key::Tab);
        assert_eq!(r.harness.state().active_tab, 0);
        assert_eq!(texts(&r), ["a"]);
        press(&mut r, CTRL_SHIFT, Key::Tab);
        assert_eq!(texts(&r), ["b"], "Ctrl+Shift+Tab goes back");

        press(&mut r, Modifiers::CTRL, Key::W);
        assert_eq!(r.harness.state().tab_count(), 1);
        assert_eq!(texts(&r), ["a"]);
        press(&mut r, Modifiers::CTRL, Key::W);
        assert_eq!(r.harness.state().tab_count(), 1, "the last tab stays");
    }

    #[test]
    fn eight_bar_limit_disables_plus() {
        let mut r = with_bars(1);
        for _ in 0..MAX_SEARCH_BARS + 2 {
            press(&mut r, CTRL_SHIFT, Key::T);
        }
        assert_eq!(ids(&r).len(), MAX_SEARCH_BARS, "Ctrl+Shift+T stops at the limit");
        let plus = r.harness.get_by_label(ADD);
        assert!(plus.accesskit_node().is_disabled(), "+ is disabled");
        plus.click();
        r.harness.run_steps(3);
        assert_eq!(
            ids(&r).len(),
            MAX_SEARCH_BARS,
            "clicking a disabled + does nothing"
        );
        // Closing one enables it again. (The disabled button let the click through to the
        // first bar's field, so focus the newest bar first.)
        let newest = r.harness.state().search.newest();
        focus_bar(&mut r, newest);
        press(&mut r, CTRL_SHIFT, Key::W);
        assert!(!r.harness.get_by_label(ADD).accesskit_node().is_disabled());
    }

    #[test]
    fn tab_while_renaming_commits_the_rename_and_keeps_the_bars() {
        let mut r = with_bars(2);
        r.show_default();
        r.click_cell("notes.txt", Modifiers::NONE);
        press(&mut r, Modifiers::NONE, Key::F2);
        assert!(r.harness.ctx.memory(|m| m.has_focus(editor_id())), "editing");
        // Bar shortcuts are suspended while the editor is open.
        press(&mut r, Modifiers::CTRL, Key::T);
        press(&mut r, CTRL_SHIFT, Key::T);
        assert_eq!(ids(&r), [0, 1], "no bar was added");
        assert_eq!(r.harness.state().tab_count(), 1, "no tab was opened");
        type_text(&mut r, "todo");
        press(&mut r, Modifiers::NONE, Key::Tab);
        assert_eq!(
            r.fake.platform_actions(),
            [PlatformAction::Rename {
                from: "/home/me/docs/notes.txt".into(),
                new_name: "todo.txt".into(),
                undo: false,
            }],
            "Tab committed the rename"
        );
        assert!(r.harness.state().rename.is_none());
        assert_eq!(focused_bar(&r), None, "Tab did not also jump to a bar");
        assert_eq!(texts(&r), ["", ""], "no text leaked into the bars");
        // The next Tab is a focus switch again.
        press(&mut r, Modifiers::NONE, Key::Tab);
        assert_eq!(focused_bar(&r), Some(1));
    }

    #[test]
    fn status_bar_and_title_show_the_combined_query() {
        let mut r = with_bars(1);
        type_text(&mut r, "rust");
        assert_eq!(
            r.harness.state().search.shown_title.as_deref(),
            Some("rust - Everythux")
        );
        press(&mut r, CTRL_SHIFT, Key::T);
        type_text(&mut r, "path:src");
        r.show_default();
        // Shown in the status bar and, truncated to the tab label, in the tab strip.
        assert!(r.harness.query_all_by_label("rust AND path:src").count() >= 1);
        assert_eq!(
            r.harness.state().search.shown_title.as_deref(),
            Some("rust AND path:src - Everythux")
        );
    }

    #[test]
    fn escape_clears_the_focused_bar() {
        let mut r = with_bars(2);
        focus_bar(&mut r, 0);
        type_text(&mut r, "a");
        focus_bar(&mut r, 1);
        type_text(&mut r, "b");
        press(&mut r, Modifiers::NONE, Key::Escape);
        assert_eq!(texts(&r), ["a", ""], "only the focused bar is cleared");
        assert_eq!(ids(&r), [0, 1], "Escape doesn't close bars");
    }
}

// --- Context menu: open with, file actions, copy file, trash ------------------------------

mod context_menu {
    use std::path::PathBuf;
    use std::sync::Arc;

    use everythux_platform::actions::{ActionCatalog, XdgDirs};

    use super::*;

    const NOTES: &str = "/home/me/docs/notes.txt";

    /// A catalog with Kate (text) and a service menu with a top-level and a submenu action.
    fn catalog() -> Arc<ActionCatalog> {
        let tmp = tempfile::tempdir().expect("tempdir");
        let write = |rel: &str, text: &str| {
            let path = tmp.path().join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
            std::fs::write(path, text).expect("write");
        };
        write(
            "data/applications/kate.desktop",
            "[Desktop Entry]\nType=Application\nName=Kate\nExec=kate %F\nMimeType=text/plain;\n",
        );
        write(
            "data/kio/servicemenus/text.desktop",
            "[Desktop Entry]\nMimeType=text/plain;\nActions=count;\n[Desktop Action count]\nName=Count words\nExec=wc -w %f\n",
        );
        write(
            "data/kio/servicemenus/hashes.desktop",
            "[Desktop Entry]\nMimeType=text/plain;\nActions=hash;\nX-KDE-Submenu=Hashes\n[Desktop Action hash]\nName=Checksum\nExec=sha256sum %f\n",
        );
        let dirs = XdgDirs {
            config_home: tmp.path().join("config"),
            data_home: tmp.path().join("none"),
            data_dirs: vec![tmp.path().join("data")],
            ..XdgDirs::default()
        };
        Arc::new(ActionCatalog::load(&dirs))
    }

    fn open_menu(r: &mut Rig) {
        r.fake.send(BackendEvent::Actions(catalog()));
        r.harness.run_steps(2);
        r.harness.get_by_label("notes.txt").click_secondary();
        r.harness.run_steps(4);
    }

    #[test]
    fn open_with_lists_the_apps_for_the_type_and_launches_one() {
        let mut r = rig();
        r.show_default();
        open_menu(&mut r);
        r.harness.get_by_label_contains("Open with").click();
        r.harness.run_steps(3);
        r.harness.get_by_label("Kate").click();
        r.harness.run_steps(3);
        let actions = r.fake.platform_actions();
        let [PlatformAction::Launch { launcher, paths }] = actions.as_slice() else {
            panic!("expected one launch, got {actions:?}");
        };
        assert_eq!(launcher.name, "Kate");
        assert_eq!(paths, &[PathBuf::from(NOTES)]);
    }

    #[test]
    fn file_actions_appear_with_submenus_and_run_on_the_selection() {
        let mut r = rig();
        r.show_default();
        open_menu(&mut r);
        r.harness.get_by_label("Count words").click();
        r.harness.run_steps(3);
        let actions = r.fake.platform_actions();
        let [PlatformAction::Launch { launcher, paths }] = actions.as_slice() else {
            panic!("expected one launch, got {actions:?}");
        };
        assert_eq!(launcher.exec, "wc -w %f");
        assert_eq!(paths, &[PathBuf::from(NOTES)]);
        // The grouped action sits under its submenu.
        r.harness.get_by_label("notes.txt").click_secondary();
        r.harness.run_steps(4);
        assert!(r.harness.query_by_label("Checksum").is_none());
        r.harness.get_by_label_contains("Hashes").click();
        r.harness.run_steps(3);
        assert!(r.harness.query_by_label("Checksum").is_some());
    }

    #[test]
    fn move_to_trash_and_copy_file_act_on_the_row() {
        let mut r = rig();
        r.show_default();
        r.harness.get_by_label("notes.txt").click_secondary();
        r.harness.run_steps(4);
        r.harness.get_by_label("Copy file").click();
        r.harness.run_steps(3);
        r.harness.get_by_label("notes.txt").click_secondary();
        r.harness.run_steps(4);
        r.harness.get_by_label_contains("Move to Trash").click();
        r.harness.run_steps(3);
        assert_eq!(
            r.fake.platform_actions(),
            [
                PlatformAction::CopyFiles(vec![NOTES.into()]),
                PlatformAction::Trash(vec![NOTES.into()]),
            ]
        );
    }

    #[test]
    fn cut_and_paste_here_on_a_folder() {
        let mut r = rig();
        let ids = [r.fixture.docs_entry, r.fixture.notes];
        r.show(&ids);
        r.harness.get_by_label("notes.txt").click_secondary();
        r.harness.run_steps(4);
        assert!(
            r.harness.query_by_label("Paste here").is_none(),
            "only folders paste"
        );
        r.harness.get_by_label("Cut").click();
        r.harness.run_steps(3);
        r.harness.get_by_label("docs").click_secondary();
        r.harness.run_steps(4);
        r.harness.get_by_label("Paste here").click();
        r.harness.run_steps(3);
        assert_eq!(
            r.fake.platform_actions(),
            [
                PlatformAction::CutFiles(vec![NOTES.into()]),
                PlatformAction::Paste {
                    dest: "/home/me/docs".into()
                },
            ]
        );
        r.fake.clear();
        r.fake.send(BackendEvent::Pasted {
            dest: "/home/me/docs".into(),
            cut: true,
            results: Ok(vec![(
                "/home/me/proj/a.txt".into(),
                Ok("/home/me/docs/a.txt".into()),
            )]),
        });
        r.harness.run_steps(2);
        assert!(
            r.harness
                .state()
                .toasts
                .iter()
                .any(|t| t.text.contains("Moved 1 item(s) into docs"))
        );
        assert!(r.fake.jobs().iter().any(|j| matches!(
            j,
            Job::RescanPath { path, .. } if path == &PathBuf::from("/home/me/docs")
        )));
    }

    #[test]
    fn trash_outcome_toasts_and_rescans_the_folder() {
        let mut r = rig();
        r.show_default();
        r.fake.clear();
        r.fake.send(BackendEvent::Trashed(vec![(NOTES.into(), Ok(()))]));
        r.harness.run_steps(2);
        let app = r.harness.state();
        assert!(
            app.toasts
                .iter()
                .any(|t| t.text.contains("Moved notes.txt to the trash"))
        );
        assert!(r.fake.jobs().iter().any(|j| matches!(
            j,
            Job::RescanPath { path, mode: everythux_index::ScanMode::Quick }
                if path == &PathBuf::from("/home/me/docs")
        )));
    }

    #[test]
    fn failed_trash_reports_the_reason() {
        let mut r = rig();
        r.show_default();
        r.fake.send(BackendEvent::Trashed(vec![(
            NOTES.into(),
            Err("permission denied".into()),
        )]));
        r.harness.run_steps(2);
        assert!(
            r.harness
                .state()
                .toasts
                .iter()
                .any(|t| t.text.contains("Cannot move notes.txt") && t.text.contains("permission denied"))
        );
    }

    #[test]
    fn shift_delete_asks_then_deletes_permanently() {
        let mut r = rig();
        r.show_default();
        r.harness.get_by_label("notes.txt").click();
        r.harness.run_steps(3);
        r.fake.clear();
        r.harness.key_press_modifiers(Modifiers::SHIFT, Key::Delete);
        r.harness.run_steps(3);
        assert!(r.harness.state().windows.confirm_delete.is_some());
        assert!(
            r.fake.platform_actions().is_empty(),
            "nothing before confirmation"
        );
        r.harness.get_by_label("Delete").click();
        r.harness.run_steps(3);
        assert_eq!(
            r.fake.platform_actions(),
            [PlatformAction::Delete(vec![NOTES.into()])]
        );
        assert!(r.harness.state().windows.confirm_delete.is_none());
    }

    #[test]
    fn delete_key_moves_the_selection_to_the_trash() {
        let mut r = rig();
        r.show_default();
        r.harness.get_by_label("notes.txt").click();
        r.harness.run_steps(3);
        r.fake.clear();
        r.harness.key_press(Key::Delete);
        r.harness.run_steps(3);
        assert_eq!(
            r.fake.platform_actions(),
            [PlatformAction::Trash(vec![NOTES.into()])]
        );
    }

    #[test]
    fn permanent_delete_can_be_cancelled_and_reports_the_outcome() {
        let mut r = rig();
        r.show_default();
        r.harness.get_by_label("notes.txt").click_secondary();
        r.harness.run_steps(4);
        r.harness.get_by_label_contains("Delete permanently").click();
        r.harness.run_steps(3);
        r.fake.clear();
        r.harness.get_by_label("Cancel").click();
        r.harness.run_steps(3);
        assert_eq!(r.fake.platform_actions(), Vec::<PlatformAction>::new());
        assert!(r.harness.state().windows.confirm_delete.is_none());

        r.fake.send(BackendEvent::Deleted(vec![(NOTES.into(), Ok(()))]));
        r.harness.run_steps(2);
        assert!(
            r.harness
                .state()
                .toasts
                .iter()
                .any(|t| t.text.contains("Deleted notes.txt"))
        );
        r.fake.send(BackendEvent::Deleted(vec![(
            NOTES.into(),
            Err("permission denied".into()),
        )]));
        r.harness.run_steps(2);
        assert!(
            r.harness
                .state()
                .toasts
                .iter()
                .any(|t| t.text.contains("Cannot delete notes.txt"))
        );
    }

    #[test]
    fn hiding_an_entry_persists_and_removes_it_from_the_next_menu() {
        let mut r = rig();
        r.show_default();
        open_menu(&mut r);
        r.harness.get_by_label_contains("Hide an entry").click();
        r.harness.run_steps(3);
        // The entry is listed twice now: in the menu and in the "Hide an entry" submenu.
        r.harness
            .query_all_by_label("Count words")
            .last()
            .expect("entry in the submenu")
            .click();
        r.harness.run_steps(3);
        let saved = r.fake.last_saved_config().expect("saved");
        assert_eq!(saved.ui.context_menu.hidden, ["service:text.desktop:count"]);
        r.harness.get_by_label("notes.txt").click_secondary();
        r.harness.run_steps(4);
        assert!(r.harness.query_by_label("Count words").is_none());
    }

    #[test]
    fn the_catalog_is_requested_at_startup() {
        let r = rig();
        assert!(
            r.fake
                .calls()
                .iter()
                .any(|c| matches!(c, crate::backend::fake::Call::LoadActions))
        );
    }
}

#[test]
fn toolbar_buttons_run_their_commands() {
    let mut r = rig();
    r.show_default();
    r.harness.get_by_label("Open JSON config").click();
    r.harness.run_steps(3);
    assert_eq!(
        r.fake.platform_actions(),
        [PlatformAction::OpenDefault(
            "/home/me/.config/everythux/config.json".into()
        )]
    );
    r.harness.get_by_label_contains("Reindex all").click();
    r.harness.run_steps(3);
    assert_eq!(r.fake.jobs(), [Job::FullReindex]);
}

#[test]
fn history_dropdown_opens_picks_and_forgets_with_shift_delete() {
    let mut r = rig();
    r.show_default();
    let now = 1_700_000_000;
    {
        let app = r.harness.state_mut();
        app.history.touch(&["report".to_owned()], now);
        app.history.touch(&["notes".to_owned()], now + 10);
    }
    r.harness.run_steps(2);
    r.harness.get_by_label("Search history").click();
    r.harness.run_steps(3);
    assert!(r.harness.query_by_label("notes").is_some());
    assert!(r.harness.query_by_label("report").is_some());

    // Newest first: the first line is "notes". Move to "report" and forget it.
    r.harness.key_press(Key::ArrowDown);
    r.harness.run_steps(2);
    r.harness.key_press_modifiers(Modifiers::SHIFT, Key::Delete);
    r.harness.run_steps(3);
    let labels: Vec<String> = r
        .harness
        .state()
        .history
        .entries
        .iter()
        .map(everythux_core::history::HistoryEntry::label)
        .collect();
    assert_eq!(labels, ["notes"]);

    // Enter runs the highlighted search.
    r.harness.key_press(Key::Enter);
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().search.texts(), ["notes"]);
}

#[test]
fn toolbar_star_bookmarks_the_current_search() {
    let mut r = rig();
    r.show_default();
    r.harness.state_mut().set_bar_text(0, "report");
    r.harness.run_steps(2);
    r.harness
        .get_by_label_contains("Add the current search to the bookmarks")
        .click();
    r.harness.run_steps(3);
    assert!(
        r.harness.query_by_label("Add current search").is_some(),
        "the bookmarks window is open"
    );
    assert_eq!(r.harness.state().windows.bookmarks.name, "report");
    r.harness.run_steps(3);
    r.harness.get_by_label("Add current search").click();
    r.harness.run_steps(6);
    assert_eq!(r.harness.state().config.bookmarks.len(), 1);
    assert_eq!(r.harness.state().config.bookmarks[0].name, "report");
}
