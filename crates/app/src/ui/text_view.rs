//! The text preview: a virtualised, selectable monospace view inside the fixed-width pane.
//!
//! Only the rows in view are laid out and painted, so an 8 MiB preview costs the same per frame
//! as a short one. Selection is kept as a byte range of the whole text (not per label), which is
//! what lets `Ctrl+A` / `Ctrl+C` cover rows that are scrolled out of view.

use egui::text::CCursor;
use egui::{
    Align2, Button, Event, Id, Key, Label, Modifiers, Pos2, Rect, RichText, ScrollArea, Sense, TextStyle, Ui,
    Vec2, WidgetInfo, WidgetType,
};
use everythux_core::config::SizeFormat;
use everythux_preview::{TEXT_MAX_BYTES, TEXT_MORE_BYTES};

use crate::app::{App, Effect};
use crate::format::format_size;
use crate::text_doc::TextDoc;

/// Widget id of the text area; it takes keyboard focus when clicked.
pub(crate) fn text_view_id() -> Id {
    Id::new("everythux_preview_text")
}

/// What the user asked for in the text view this frame, applied once the view is drawn.
#[derive(Debug, Default)]
struct Actions {
    toggle_wrap: bool,
    toggle_line_numbers: bool,
    show_more: bool,
    copy: Option<String>,
}

/// Geometry of the laid-out text for this frame.
struct Layout {
    font: egui::FontId,
    row_height: f32,
    char_width: f32,
    gutter: f32,
    /// Top-left of row 0 in screen space.
    origin: Pos2,
}

impl Layout {
    fn row_at(&self, y: f32, rows: usize) -> usize {
        let row = ((y - self.origin.y) / self.row_height).floor().max(0.0) as usize;
        row.min(rows.saturating_sub(1))
    }

    fn galley(&self, ui: &Ui, text: &str) -> std::sync::Arc<egui::Galley> {
        let color = ui.visuals().text_color();
        ui.ctx()
            .fonts_mut(|f| f.layout_no_wrap(text.to_owned(), self.font.clone(), color))
    }

    /// Byte offset of the character under `pos`.
    fn offset_at(&self, ui: &Ui, doc: &TextDoc, pos: Pos2) -> usize {
        let rows = doc.rows();
        let Some(&row) = rows.get(self.row_at(pos.y, rows.len())) else {
            return 0;
        };
        let text = doc.row_text(row);
        let galley = self.galley(ui, text);
        let x = pos.x - self.origin.x - self.gutter;
        let chars = galley
            .cursor_from_pos(Vec2::new(x, self.row_height * 0.5))
            .index
            .0;
        row.start as usize + text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i)
    }
}

/// "UTF-16 LE · 1.2 MiB · showing first 64 KiB".
fn summary(doc: &TextDoc, format: SizeFormat) -> String {
    let mut text = format!("{} · {}", doc.encoding, format_size(doc.total_bytes, format));
    if doc.end_offset < doc.total_bytes {
        text.push_str(" · showing first ");
        text.push_str(&format_size(doc.end_offset, format));
    }
    text
}

fn digits(mut n: usize) -> usize {
    let mut d = 1;
    while n >= 10 {
        n /= 10;
        d += 1;
    }
    d
}

impl App {
    /// Whether the text preview holds a non-empty selection (then `Ctrl+C` copies it, not paths).
    pub(crate) fn preview_text_selected(&self) -> bool {
        self.preview
            .shown
            .as_ref()
            .and_then(|s| s.text.as_ref())
            .is_some_and(|doc| doc.selection().is_some())
    }

    /// Draws the text preview of the shown file (header line, toggles, text, "Show more").
    pub(crate) fn text_preview(&mut self, ui: &mut Ui) {
        let pane = &self.config.ui.preview_pane;
        let (wrap, line_numbers) = (pane.wrap_text, pane.line_numbers);
        let size_format = self.config.ui.size_format;
        let more_pending = self.preview.more_request.is_some();
        let more_error = self.preview.more_error.clone();
        let Some(doc) = self.preview.shown.as_mut().and_then(|s| s.text.as_mut()) else {
            return;
        };
        let mut actions = Actions::default();
        // Presses anywhere in the pane (the toolbar buttons) keep the selection.
        let pane = ui.max_rect();

        ui.add(Label::new(RichText::new(summary(doc, size_format)).weak()).truncate());
        ui.horizontal(|ui| {
            let wrap_button = ui
                .add(Button::selectable(wrap, "Wrap"))
                .on_hover_text("Wrap long lines at the pane width");
            actions.toggle_wrap = wrap_button.clicked();
            actions.toggle_line_numbers = ui.add(Button::selectable(line_numbers, "Line numbers")).clicked();
            let copy = ui
                .add_enabled(doc.selection().is_some(), Button::new("Copy"))
                .on_hover_text("Copy the selected text (Ctrl+C)")
                .on_disabled_hover_text("Select text in the preview to copy it");
            if copy.clicked() {
                actions.copy = doc.selected_text().map(str::to_owned);
            }
        });

        let has_footer = doc.end_offset < doc.total_bytes;
        let footer_height = if has_footer {
            ui.spacing().interact_size.y + ui.spacing().item_spacing.y
        } else {
            0.0
        };
        let max_height = (ui.available_height() - footer_height).max(48.0);
        let scroll = if wrap {
            ScrollArea::vertical()
        } else {
            ScrollArea::both()
        };
        scroll
            .id_salt("everythux_preview_text_scroll")
            .auto_shrink([false, false])
            .max_height(max_height)
            .show_viewport(ui, |ui, viewport| {
                text_body(ui, viewport, pane, doc, wrap, line_numbers, &mut actions);
            });

        if has_footer {
            ui.horizontal(|ui| {
                if more_pending {
                    ui.spinner();
                    ui.weak("Loading more…");
                } else if let Some(error) = &more_error {
                    ui.add(Label::new(RichText::new(error).weak()).truncate());
                } else if doc.more_available() {
                    let label = format!("Show more (next {})", format_size(TEXT_MORE_BYTES, size_format));
                    actions.show_more = ui.button(label).clicked();
                } else {
                    ui.add(
                        Label::new(
                            RichText::new(format!(
                                "Text previews stop at {}; open the file to read the rest.",
                                format_size(TEXT_MAX_BYTES, size_format)
                            ))
                            .weak(),
                        )
                        .truncate(),
                    );
                }
            });
        }

        if actions.toggle_wrap {
            self.config.ui.preview_pane.wrap_text = !wrap;
            self.config_changed();
        }
        if actions.toggle_line_numbers {
            self.config.ui.preview_pane.line_numbers = !line_numbers;
            self.config_changed();
        }
        if actions.show_more {
            self.request_more_text();
        }
        if let Some(text) = actions.copy {
            self.apply_effects(vec![Effect::CopyText(text)]);
        }
    }
}

/// The scrolled content: rows in view, selection, pointer and keyboard handling.
fn text_body(
    ui: &mut Ui,
    viewport: Rect,
    pane: Rect,
    doc: &mut TextDoc,
    wrap: bool,
    line_numbers: bool,
    actions: &mut Actions,
) {
    let font = TextStyle::Monospace.resolve(ui.style());
    let (row_height, char_width) = ui
        .ctx()
        .fonts_mut(|f| (f.row_height(&font), f.glyph_width(&font, '0')));
    let char_width = char_width.max(1.0);
    let gutter = if line_numbers {
        (digits(doc.line_count()) + 1) as f32 * char_width
    } else {
        0.0
    };
    let width = ui.available_width();
    let columns = if wrap {
        ((width - gutter) / char_width).floor().max(1.0) as usize
    } else {
        usize::MAX
    };
    doc.layout(columns);
    let rows = doc.rows().len();
    let content_width = if wrap {
        width
    } else {
        (gutter + (doc.max_columns() + 1) as f32 * char_width).max(width)
    };
    let origin = ui.max_rect().min;
    let content = Rect::from_min_size(origin, Vec2::new(content_width, rows as f32 * row_height));
    ui.allocate_rect(content, Sense::hover());
    let layout = Layout {
        font,
        row_height,
        char_width,
        gutter,
        origin,
    };

    let response = ui.interact(content, text_view_id(), Sense::click_and_drag());
    handle_pointer(ui, &response, pane, doc, &layout);
    handle_keys(ui, &response, doc, actions);
    response.context_menu(|ui| context_menu(ui, doc, actions));

    let first = ((viewport.min.y / row_height).floor().max(0.0) as usize).min(rows);
    let last = ((viewport.max.y / row_height).ceil() as usize + 1).min(rows);
    paint_rows(ui, doc, &layout, content_width, first..last, line_numbers);
}

/// Right-click menu of the text: copy the selection, or select everything.
fn context_menu(ui: &mut Ui, doc: &mut TextDoc, actions: &mut Actions) {
    let has_selection = doc.selection().is_some();
    if ui
        .add_enabled(has_selection, Button::new("Copy"))
        .on_disabled_hover_text("Select text first")
        .clicked()
    {
        actions.copy = doc.selected_text().map(str::to_owned);
        ui.close();
    }
    if ui.button("Select all").clicked() {
        doc.select_all();
        ui.close();
    }
}

fn handle_pointer(ui: &Ui, response: &egui::Response, pane: Rect, doc: &mut TextDoc, layout: &Layout) {
    let (pressed, shift, press_origin) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.modifiers.shift,
            i.pointer.press_origin(),
        )
    });
    let visible = ui.clip_rect().intersect(response.rect);
    if pressed && press_origin.is_some_and(|p| !visible.contains(p)) {
        // Clicking outside the pane (the result list, the search box) ends the selection, so
        // Ctrl+C goes back to copying paths. The pane's own buttons keep it.
        if press_origin.is_some_and(|p| !pane.contains(p)) {
            doc.clear_selection();
        }
        return;
    }
    let Some(pos) = response.interact_pointer_pos() else {
        return;
    };
    if (pressed && response.contains_pointer()) || (response.clicked() && !response.dragged()) {
        response.request_focus();
        let at = layout.offset_at(ui, doc, pos);
        if shift {
            doc.select_to(at);
        } else if pressed || doc.selection().is_none() {
            doc.select_from(at);
        }
    } else if response.dragged() {
        let at = layout.offset_at(ui, doc, pos);
        doc.select_to(at);
    }
}

fn handle_keys(ui: &Ui, response: &egui::Response, doc: &mut TextDoc, actions: &mut Actions) {
    let focused = response.has_focus();
    // With a selection but no focus, only act while no other widget (e.g. the search box) has
    // the keyboard: that widget copies its own text.
    let other_focused = !focused && ui.ctx().memory(|m| m.focused().is_some());
    if !(focused || (doc.selection().is_some() && !other_focused)) {
        return;
    }
    ui.ctx().input_mut(|i| {
        if i.consume_key(Modifiers::COMMAND, Key::A) {
            doc.select_all();
        }
        // Native backends turn Ctrl+C into `Event::Copy` and drop the key event.
        let copy_event = i.events.iter().any(|e| matches!(e, Event::Copy));
        let copy_key = i.consume_key(Modifiers::COMMAND, Key::C);
        if copy_event || copy_key {
            i.events.retain(|e| !matches!(e, Event::Copy));
            actions.copy = doc.selected_text().map(str::to_owned);
        }
    });
}

fn paint_rows(
    ui: &mut Ui,
    doc: &TextDoc,
    layout: &Layout,
    content_width: f32,
    range: std::ops::Range<usize>,
    line_numbers: bool,
) {
    let selection = doc.selection();
    let selection_fill = ui.visuals().selection.bg_fill;
    let text_color = ui.visuals().text_color();
    let weak_color = ui.visuals().weak_text_color();
    for index in range {
        let row = doc.rows()[index];
        let text = doc.row_text(row);
        let y = layout.origin.y + index as f32 * layout.row_height;
        let text_pos = Pos2::new(layout.origin.x + layout.gutter, y);
        let galley = layout.galley(ui, text);

        if let Some(sel) = &selection
            && sel.start <= row.end as usize
            && sel.end > row.start as usize
        {
            let from = sel.start.max(row.start as usize) - row.start as usize;
            let to = sel.end.min(row.end as usize) - row.start as usize;
            let x = |byte: usize| {
                galley
                    .pos_from_cursor(CCursor::new(text[..byte].chars().count()))
                    .min
                    .x
            };
            // The line break itself is selected: show it as half a character.
            let tail = if sel.end > row.end as usize {
                layout.char_width * 0.5
            } else {
                0.0
            };
            let rect = Rect::from_min_max(
                Pos2::new(text_pos.x + x(from), y),
                Pos2::new(text_pos.x + x(to) + tail, y + layout.row_height),
            );
            ui.painter().rect_filled(rect, 0.0, selection_fill);
        }
        if line_numbers && let Some(number) = doc.line_number(row) {
            ui.painter().text(
                Pos2::new(text_pos.x - layout.char_width * 0.5, y),
                Align2::RIGHT_TOP,
                number.to_string(),
                layout.font.clone(),
                weak_color,
            );
        }
        ui.painter().galley(text_pos, galley, text_color);

        // Screen readers (and the UI tests) see each visible row as a label.
        let row_rect = Rect::from_min_size(
            Pos2::new(layout.origin.x, y),
            Vec2::new(content_width, layout.row_height),
        );
        ui.interact(row_rect, text_view_id().with(index), Sense::hover())
            .widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, text));
    }
}

#[cfg(test)]
mod tests {
    use everythux_preview::TextEncoding;

    use super::*;

    #[test]
    fn summary_says_how_much_is_shown() {
        let doc = TextDoc::new("x".to_owned(), TextEncoding::Utf16Le, 1_258_291, 65_536);
        assert_eq!(
            summary(&doc, SizeFormat::Binary),
            "UTF-16 LE · 1.2 MiB · showing first 64.0 KiB"
        );
        let whole = TextDoc::new("abc".to_owned(), TextEncoding::Utf8, 3, 3);
        assert_eq!(summary(&whole, SizeFormat::Bytes), "UTF-8 · 3 B");
    }

    #[test]
    fn digits_counts_decimal_digits() {
        assert_eq!([0, 9, 10, 99, 100, 123_456].map(digits), [1, 1, 2, 2, 3, 6]);
    }
}
