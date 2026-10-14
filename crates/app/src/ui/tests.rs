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
fn dragging_the_preview_pane_edge_keeps_selection_and_preview() {
    let mut r = preview_rig();
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
