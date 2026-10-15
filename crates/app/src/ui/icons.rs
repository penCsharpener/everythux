//! Tiny vector icons per file category and for small icon buttons, painted directly so no font
//! glyphs or image assets are needed.

use egui::{Color32, Pos2, Rect, Shape, Stroke, Ui, Vec2, pos2, vec2};
use everythux_platform::mime::Category;

/// Edge length of an icon in logical pixels.
pub(crate) const ICON_SIZE: f32 = 14.0;

/// The tint of a category.
pub(crate) fn category_color(category: Category) -> Color32 {
    match category {
        Category::Folder => Color32::from_rgb(0xE0, 0xA2, 0x30),
        Category::Image => Color32::from_rgb(0x3E, 0xA6, 0x6B),
        Category::Audio => Color32::from_rgb(0x9B, 0x6B, 0xD1),
        Category::Video => Color32::from_rgb(0xD1, 0x5B, 0x5B),
        Category::Document => Color32::from_rgb(0x4B, 0x8B, 0xD6),
        Category::Archive => Color32::from_rgb(0xB0, 0x7A, 0x4A),
        Category::Code => Color32::from_rgb(0x2E, 0xA8, 0xA8),
        Category::Executable => Color32::from_rgb(0x6C, 0x7A, 0x89),
        Category::Other => Color32::from_rgb(0x8C, 0x8C, 0x8C),
    }
}

/// Allocates an icon-sized rect and paints the icon for `category` in it.
pub(crate) fn icon(ui: &mut Ui, category: Category, dimmed: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(ICON_SIZE), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let mut color = category_color(category);
    if dimmed {
        color = color.gamma_multiply(0.45);
    }
    paint(ui.painter(), rect, category, color);
}

/// A glyph for a small icon button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Glyph {
    /// `+`: add something.
    Plus,
    /// `×`: close something.
    Cross,
    /// A downward chevron: opens a drop-down list.
    ChevronDown,
    /// `Aa`: case-sensitive matching.
    MatchCase,
    /// A path with a slash: match the full path.
    MatchPath,
    /// `.*`: regular expressions.
    Regex,
    /// Two overlapping pages: duplicate files.
    Duplicates,
    /// Circular arrows: rebuild the whole index.
    Reindex,
    /// A single arrow with a bar: rescan the changed folders.
    QuickRescan,
    /// `{}`: the JSON config file.
    Config,
    /// A window with a side pane: the preview pane.
    Preview,
}

/// A square, frameless button showing a painted `glyph`, highlighted on hover like the other
/// small toggles. `label` is its accessible name; disabled buttons are greyed out and never
/// report clicks.
pub(crate) fn glyph_button(ui: &mut Ui, glyph: Glyph, label: &str, enabled: bool) -> egui::Response {
    icon_button(ui, glyph, label, enabled, false)
}

/// Like [`glyph_button`], with a `selected` state drawn as a filled background (toggles).
pub(crate) fn icon_button(
    ui: &mut Ui,
    glyph: Glyph,
    label: &str,
    enabled: bool,
    selected: bool,
) -> egui::Response {
    let side = ui.spacing().interact_size.y;
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), sense);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, selected, label));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.style().interact(&response);
    let color = if enabled {
        if selected {
            ui.painter().rect_filled(
                rect.shrink(1.0),
                visuals.corner_radius,
                ui.visuals().selection.bg_fill,
            );
        } else if response.hovered() {
            ui.painter()
                .rect_filled(rect.shrink(1.0), visuals.corner_radius, visuals.weak_bg_fill);
        }
        if selected {
            ui.visuals().selection.stroke.color
        } else {
            visuals.fg_stroke.color
        }
    } else {
        ui.visuals().weak_text_color().gamma_multiply(0.6)
    };
    paint_glyph(ui, rect, glyph, color);
    response
}

fn paint_glyph(ui: &Ui, rect: Rect, glyph: Glyph, color: Color32) {
    let painter = ui.painter();
    let stroke = Stroke::new(1.5, color);
    let side = rect.width();
    let r = Rect::from_center_size(rect.center(), Vec2::splat(side * 0.4));
    let big = Rect::from_center_size(rect.center(), Vec2::splat(side * 0.6));
    let text = |s: &str| {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            s,
            egui::FontId::monospace(side * 0.5),
            color,
        );
    };
    let line = |a: Pos2, b: Pos2| {
        painter.line_segment([a, b], stroke);
    };
    match glyph {
        Glyph::Plus => {
            line(pos2(r.center().x, r.top()), pos2(r.center().x, r.bottom()));
            line(pos2(r.left(), r.center().y), pos2(r.right(), r.center().y));
        }
        Glyph::Cross => {
            line(r.left_top(), r.right_bottom());
            line(r.right_top(), r.left_bottom());
        }
        Glyph::ChevronDown => {
            let c = r.center();
            line(pos2(r.left(), c.y - 2.0), pos2(c.x, c.y + 2.0));
            line(pos2(c.x, c.y + 2.0), pos2(r.right(), c.y - 2.0));
        }
        Glyph::MatchCase => text("Aa"),
        Glyph::Regex => text(".*"),
        Glyph::Config => text("{}"),
        Glyph::MatchPath => {
            // `/a/b`: two slashes with a baseline.
            line(
                pos2(big.left() + 2.0, big.bottom()),
                pos2(big.left() + 5.0, big.top()),
            );
            line(
                pos2(big.center().x + 1.0, big.bottom()),
                pos2(big.center().x + 4.0, big.top()),
            );
            line(pos2(big.left(), big.bottom()), pos2(big.right(), big.bottom()));
        }
        Glyph::Reindex => {
            let c = big.center();
            let radius = big.width() * 0.5;
            let arc = |from: f32, to: f32| {
                let points: Vec<Pos2> = (0..=10)
                    .map(|i| {
                        let a = from + (to - from) * (i as f32 / 10.0);
                        c + vec2(a.cos(), a.sin()) * radius
                    })
                    .collect();
                painter.add(Shape::line(points, stroke));
            };
            arc(0.3, 2.9);
            arc(3.44, 6.04);
            for (tip, dir) in [
                (c + vec2(2.9_f32.cos(), 2.9_f32.sin()) * radius, vec2(-1.0, -1.0)),
                (c + vec2(6.04_f32.cos(), 6.04_f32.sin()) * radius, vec2(1.0, 1.0)),
            ] {
                line(tip, tip + vec2(dir.x * 3.0, 0.0));
                line(tip, tip + vec2(0.0, dir.y * 3.0));
            }
        }
        Glyph::QuickRescan => {
            // A lightning bolt.
            let p = |fx: f32, fy: f32| pos2(big.left() + big.width() * fx, big.top() + big.height() * fy);
            painter.add(Shape::line(
                vec![
                    p(0.6, 0.0),
                    p(0.15, 0.58),
                    p(0.5, 0.58),
                    p(0.4, 1.0),
                    p(0.88, 0.38),
                    p(0.52, 0.38),
                    p(0.6, 0.0),
                ],
                stroke,
            ));
        }
        Glyph::Duplicates => {
            let back = Rect::from_center_size(rect.center() + vec2(1.5, -1.5), Vec2::splat(side * 0.42));
            let front = Rect::from_center_size(rect.center() + vec2(-1.5, 1.5), Vec2::splat(side * 0.42));
            painter.rect_stroke(back, 1.0, stroke, egui::StrokeKind::Inside);
            painter.rect_filled(front.expand(1.0), 0.0, ui.visuals().panel_fill);
            painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Inside);
        }
        Glyph::Preview => {
            let frame = Rect::from_center_size(rect.center(), vec2(side * 0.62, side * 0.5));
            painter.rect_stroke(frame, 1.5, stroke, egui::StrokeKind::Inside);
            let x = frame.right() - frame.width() * 0.36;
            line(pos2(x, frame.top()), pos2(x, frame.bottom()));
        }
    }
}

fn paint(painter: &egui::Painter, rect: Rect, category: Category, color: Color32) {
    let r = rect.shrink(1.0);
    let at = |fx: f32, fy: f32| pos2(r.min.x + r.width() * fx, r.min.y + r.height() * fy);
    let mark = Stroke::new(1.2, Color32::WHITE);
    if category == Category::Folder {
        let tab = Rect::from_min_max(at(0.0, 0.1), at(0.45, 0.35));
        painter.rect_filled(tab, 1.0, color);
        let body = Rect::from_min_max(at(0.0, 0.25), at(1.0, 0.95));
        painter.rect_filled(body, 1.5, color);
        return;
    }
    let page = Rect::from_min_max(at(0.12, 0.0), at(0.88, 1.0));
    painter.rect_filled(page, 2.0, color);
    let line = |a: Pos2, b: Pos2| {
        painter.line_segment([a, b], mark);
    };
    match category {
        Category::Image => {
            painter.add(Shape::convex_polygon(
                vec![
                    at(0.2, 0.85),
                    at(0.45, 0.45),
                    at(0.62, 0.7),
                    at(0.72, 0.58),
                    at(0.8, 0.85),
                ],
                Color32::WHITE,
                Stroke::NONE,
            ));
        }
        Category::Audio => {
            painter.circle_filled(at(0.4, 0.75), 1.8, Color32::WHITE);
            line(at(0.52, 0.72), at(0.52, 0.25));
            line(at(0.52, 0.25), at(0.72, 0.35));
        }
        Category::Video => {
            painter.add(Shape::convex_polygon(
                vec![at(0.35, 0.28), at(0.35, 0.72), at(0.7, 0.5)],
                Color32::WHITE,
                Stroke::NONE,
            ));
        }
        Category::Document => {
            for y in [0.3, 0.5, 0.7] {
                line(at(0.3, y), at(0.7, y));
            }
        }
        Category::Archive => {
            line(at(0.5, 0.1), at(0.5, 0.9));
            line(at(0.4, 0.3), at(0.6, 0.3));
            line(at(0.4, 0.55), at(0.6, 0.55));
        }
        Category::Code => {
            line(at(0.45, 0.3), at(0.28, 0.5));
            line(at(0.28, 0.5), at(0.45, 0.7));
            line(at(0.55, 0.3), at(0.72, 0.5));
            line(at(0.72, 0.5), at(0.55, 0.7));
        }
        Category::Executable => {
            painter.rect_stroke(
                Rect::from_center_size(r.center(), vec2(5.0, 5.0)),
                0.0,
                mark,
                egui::StrokeKind::Inside,
            );
        }
        Category::Folder | Category::Other => {}
    }
}
