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
    /// The bar whose search field lost the keyboard focus: the search goes into the history.
    focus_lost: Option<u32>,
    add: bool,
    close: Option<u32>,
    /// The ▾ button of the first bar was clicked.
    toggle_history: bool,
    /// Screen rect of the first bar's text field, where the dropdown hangs.
    first_field: Option<Rect>,
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
    history_tip: String,
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
            history_tip: "Search history (Alt+Down)".to_owned(),
        };
        let mut out = BarsOutput::default();
        for bar in &mut self.search.bars {
            bar_row(ui, bar, &params, &mut out);
        }
        ui.add_space(2.0);

        if out.toggle_history {
            self.toggle_history();
        }
        if let (true, Some(field)) = (self.dropdown.open, out.first_field) {
            self.history_dropdown(ui, field);
        }
        if out.edited {
            self.resubmit_search();
        }
        if out.focus_lost.is_some() {
            self.record_history();
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
                    right: 6 + (if first { 2.0 * button } else { button }) as i8,
                    top: 2,
                    bottom: 2,
                })
                .desired_width(f32::INFINITY),
        );
        if first {
            out.first_field = Some(response.rect);
            let chevron_slot = Rect::from_min_size(
                egui::pos2(response.rect.right() - 2.0 * button - 2.0, response.rect.top()),
                vec2(button, response.rect.height()),
            );
            let chevron = ui
                .put(chevron_slot, |ui: &mut Ui| {
                    glyph_button(ui, Glyph::ChevronDown, "Search history", true)
                })
                .on_hover_text(&p.history_tip);
            out.toggle_history |= chevron.clicked();
        }
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
        if response.lost_focus() {
            out.focus_lost = Some(bar.id);
        }
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

impl App {
    /// The history list hanging under the first search bar: one line per remembered search with
    /// its last use on the right. Keys are handled in [`App::history_keys`].
    fn history_dropdown(&mut self, ui: &mut Ui, field: Rect) {
        let entries = self.history_entries();
        let date_format = self.config.ui.date_format.clone();
        let mut picked = None;
        let mut deleted = None;
        let mut hovered = None;
        let area = egui::Area::new(egui::Id::new("everythux_history_dropdown"))
            .order(egui::Order::Foreground)
            .fixed_pos(field.left_bottom())
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(field.width().max(240.0));
                    if entries.is_empty() {
                        ui.weak("No searches yet");
                        return;
                    }
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for (i, entry) in entries.iter().enumerate() {
                            let selected = i == self.dropdown.selected;
                            let when = crate::format::format_local(
                                entry.last_used.saturating_mul(1_000_000_000),
                                &date_format,
                            );
                            let row = ui.horizontal(|ui| {
                                let label = ui.selectable_label(selected, entry.label());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.weak(when);
                                });
                                label
                            });
                            let label = row.inner;
                            if label.clicked() {
                                picked = Some(entry.clone());
                            }
                            if label.hovered() {
                                hovered = Some(i);
                            }
                            label.context_menu(|ui| {
                                if ui.button("Forget this search").clicked() {
                                    deleted = Some(entry.clone());
                                    ui.close();
                                }
                            });
                            if selected {
                                label.scroll_to_me(None);
                            }
                        }
                    });
                    ui.separator();
                    ui.weak("Enter runs · Shift+Delete forgets · Esc closes");
                });
            });
        // Only a moving pointer takes the highlight, so the arrow keys are not undone by a
        // pointer that rests over a line.
        if let Some(i) = hovered.filter(|_| ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO)) {
            self.dropdown.selected = i;
        }
        // A click anywhere else closes it.
        let clicked_away = ui.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !area.response.rect.contains(p) && !field.contains(p))
        });
        if let Some(entry) = deleted {
            self.delete_history(&entry);
        } else if let Some(entry) = picked {
            self.pick_history(&entry);
        } else if clicked_away {
            self.dropdown.open = false;
        }
    }
}
