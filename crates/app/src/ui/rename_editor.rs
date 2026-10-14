//! The inline rename editor that replaces the label in the Name cell.

use egui::text::{CCursor, CCursorRange};
use egui::{Align, Id, Key, Margin, Modifiers, TextEdit, Ui, text_edit::TextEditState};

use crate::app::CommitTrigger;
use crate::rename::{FocusRequest, Message, RenameEdit};

/// Widget id of the editor, so tests and the input handling can find it.
pub(crate) fn editor_id() -> Id {
    Id::new("everythux_rename_editor")
}

/// What the user did in the editor this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorEvent {
    Commit(CommitTrigger),
    Cancel,
}

/// Draws the editor and reports Enter, Tab, Escape and focus loss.
///
/// Enter, Tab and Escape are consumed here, before the text field sees them, so that a single-line
/// editor doesn't turn Tab into focus traversal.
pub(crate) fn rename_editor(ui: &mut Ui, edit: &mut RenameEdit) -> Option<EditorEvent> {
    let id = editor_id();
    let mut event = None;
    if ui.memory(|m| m.has_focus(id)) {
        ui.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::Enter) {
                event = Some(EditorEvent::Commit(CommitTrigger::Enter));
            } else if i.consume_key(Modifiers::NONE, Key::Tab) {
                event = Some(EditorEvent::Commit(CommitTrigger::Tab));
            } else if i.consume_key(Modifiers::NONE, Key::Escape) {
                event = Some(EditorEvent::Cancel);
            }
        });
    }
    let select_stem = edit.focus == FocusRequest::SelectStem;
    if edit.focus != FocusRequest::None {
        ui.memory_mut(|m| m.request_focus(id));
        edit.focus = FocusRequest::None;
    }

    let output = TextEdit::singleline(&mut edit.buffer)
        .id(id)
        .desired_width(f32::INFINITY)
        .margin(Margin::symmetric(2, 1))
        .vertical_align(Align::Center)
        .show(ui);
    let response = output.response;
    // `TextEdit` leaves Tab and Escape to egui's focus handling, which would move or drop the focus
    // before this code sees the key. The filter set last wins, so set it after the widget's own.
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            id,
            egui::EventFilter {
                tab: true,
                escape: true,
                horizontal_arrows: true,
                vertical_arrows: true,
            },
        );
    });

    if select_stem {
        // Same row height, same font: only the selection differs from a label.
        let mut state: TextEditState = output.state;
        state.cursor.set_char_range(Some(CCursorRange::two(
            CCursor::new(0),
            CCursor::new(edit.stem_chars()),
        )));
        state.store(ui.ctx(), id);
        ui.ctx().request_repaint();
    }
    if response.changed() {
        edit.on_edited();
    }
    edit.shown = true;
    if event.is_none() && response.lost_focus() {
        event = Some(EditorEvent::Commit(CommitTrigger::FocusLost));
    }

    if let Some(message) = &edit.message {
        let color = match message {
            Message::Error(_) => ui.visuals().error_fg_color,
            Message::Warning(_) => ui.visuals().warn_fg_color,
        };
        egui::Area::new(Id::new("everythux_rename_message"))
            .order(egui::Order::Foreground)
            .fixed_pos(response.rect.left_bottom() + egui::vec2(0.0, 2.0))
            .interactable(false)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.colored_label(color, message.text());
                });
            });
    }
    event
}
