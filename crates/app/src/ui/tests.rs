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
    assert!(r.fake.platform_actions().is_empty());
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
