//! The search bars: the first one with a `+` button, extra ones below it with an "AND" label and
//! a `×` button, each with its own error line.

use egui::{Key, Margin, Rect, RichText, TextEdit, Ui, vec2};

use crate::app::{App, bar_widget_id};
use crate::keys::describe_shortcut;
use crate::search_bars::{FIRST_BAR, MAX_SEARCH_BARS, SearchBar};
use crate::ui::icons::{Glyph, glyph_button};

/// Width of the left gutter that holds the "AND" labels once there are several bars.
const AND_GUTTER: f32 = 34.0;

/// What the user did in the bar stack this frame.
#[derive(Debug, Default)]
struct BarsOutput {
    edited: bool,
    add: bool,
    close: Option<u32>,
}

/// Per-frame facts shared by every bar row.
struct RowParams {
    add_tip: String,
    close_tip: String,
    limit_tip: String,
    can_add: bool,
    /// More than one bar: reserve the "AND" gutter on every row so all fields line up.
    several: bool,
    focus_bar: Option<u32>,
}

impl App {
    pub(crate) fn search_bar(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        let params = RowParams {
            add_tip: format!(
                "Add a search bar (AND) · {}",
                describe_shortcut(self.bindings.add_search_bar)
            ),
            close_tip: format!(
                "Close this search bar · {}",
                describe_shortcut(self.bindings.close_search_bar)
            ),
            limit_tip: format!("At most {MAX_SEARCH_BARS} search bars"),
            can_add: self.search.can_add(),
            several: self.search.bars.len() > 1,
            focus_bar: self.search.focus_bar.take(),
        };
        let mut out = BarsOutput::default();
        for bar in &mut self.search.bars {
            bar_row(ui, bar, &params, &mut out);
        }
        ui.add_space(2.0);

        if out.edited {
            self.resubmit_search();
        }
        if let Some(id) = out.close {
            self.close_search_bar(id);
        }
        if out.add {
            self.add_search_bar();
        }
    }
}

/// One bar: the optional "AND" gutter, the text field with its button inside the right edge, and
/// the error line below.
fn bar_row(ui: &mut Ui, bar: &mut SearchBar, p: &RowParams, out: &mut BarsOutput) {
    let first = bar.id == FIRST_BAR;
    ui.horizontal(|ui| {
        if p.several {
            let size = vec2(
                AND_GUTTER - ui.spacing().item_spacing.x,
                ui.spacing().interact_size.y,
            );
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            if !first {
                ui.painter().text(
                    rect.left_center(),
                    egui::Align2::LEFT_CENTER,
                    "AND",
                    egui::TextStyle::Small.resolve(ui.style()),
                    ui.visuals().weak_text_color(),
                );
            }
        }
        let button = ui.spacing().interact_size.y;
        let response = ui.add(
            TextEdit::singleline(&mut bar.text)
                .id(bar_widget_id(bar.id))
                .hint_text(if first {
                    "Search files and folders"
                } else {
                    "Narrow the results further"
                })
                .margin(Margin {
                    left: 4,
                    right: 6 + button as i8,
                    top: 2,
                    bottom: 2,
                })
                .desired_width(f32::INFINITY),
        );
        // The button sits inside the right edge of the field.
        let slot = Rect::from_min_size(
            egui::pos2(response.rect.right() - button - 2.0, response.rect.top()),
            vec2(button, response.rect.height()),
        );
        if first {
            let plus = ui
                .put(slot, |ui: &mut Ui| {
                    glyph_button(ui, Glyph::Plus, "Add a search bar", p.can_add)
                })
                .on_hover_text(&p.add_tip)
                .on_disabled_hover_text(&p.limit_tip);
            out.add |= plus.clicked();
        } else {
            let cross = ui
                .put(slot, |ui: &mut Ui| {
                    glyph_button(ui, Glyph::Cross, "Close this search bar", true)
                })
                .on_hover_text(&p.close_tip);
            if cross.clicked() {
                out.close = Some(bar.id);
            }
        }
        if p.focus_bar == Some(bar.id) {
            response.request_focus();
        }
        out.edited |= response.changed();
        // Enter surrenders focus in a single-line editor; Enter is consumed earlier as "open" so
        // this only guards against the editor losing focus for other reasons.
        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            response.request_focus();
        }
    });
    if let Some(error) = &bar.error {
        ui.horizontal(|ui| {
            if p.several {
                ui.add_space(AND_GUTTER);
            }
            let muted = ui.visuals().error_fg_color.gamma_multiply(0.8);
            ui.label(RichText::new(error).small().color(muted));
        });
    }
}
