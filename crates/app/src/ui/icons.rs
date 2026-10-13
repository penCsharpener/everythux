//! Tiny vector icons per file category, painted directly so no font glyphs or image assets are
//! needed.

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
