//! A to-scale side view of a cutting tool, as the tool dialogs and the tool library show it:
//! the shank, the fluted length in the accent colour, and dimension lines for the cutting
//! diameter, the flute length and the overall length in the user's units.

use CosmicCAM_engine::units::DisplayUnits;
use egui::{Align2, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

use crate::theme::Tokens;

/// The dimensions a drawing needs, in millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolShape {
    pub diameter: f64,
    pub shank_diameter: f64,
    pub flute_length: f64,
    pub overall_length: f64,
    pub flutes: u32,
}

impl ToolShape {
    /// True when the numbers make a drawable tool.
    fn drawable(&self) -> bool {
        [self.diameter, self.shank_diameter, self.flute_length, self.overall_length]
            .iter()
            .all(|v| v.is_finite() && *v > 0.0 && *v < 1.0e5)
            && self.flute_length <= self.overall_length
    }
}

/// Draws `tool` filling `size` (in points), tip down, holder face at the top.
pub fn draw(ui: &mut Ui, tool: Option<ToolShape>, units: DisplayUnits, tokens: &Tokens, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, tokens.app_bg);
    let Some(t) = tool.filter(ToolShape::drawable) else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "No tool to draw",
            FontId::proportional(12.0),
            tokens.text_muted,
        );
        return;
    };
    let ink = Stroke::new(1.5, tokens.text);
    let dim = Stroke::new(1.0, tokens.text_muted);
    let font = FontId::proportional(11.0);

    // One scale for both directions, leaving room for the dimension labels.
    let width = t.diameter.max(t.shank_diameter);
    let area = Rect::from_min_max(rect.min + vec2(18.0, 22.0), rect.max - vec2(78.0, 26.0));
    let scale = (f64::from(area.height()) / t.overall_length).min(f64::from(area.width()) / width) as f32;
    let px = |mm: f64| mm as f32 * scale;
    let cx = area.center().x;
    let top = area.top() + (area.height() - px(t.overall_length)) / 2.0;
    let tip = top + px(t.overall_length);
    let flute_top = tip - px(t.flute_length);
    let (r, rs) = (px(t.diameter) / 2.0, px(t.shank_diameter) / 2.0);

    // Holder face, then the shank, then the fluted length in the accent colour.
    painter.add(Shape::dashed_line(&[pos2(cx - rs - 14.0, top), pos2(cx + rs + 14.0, top)], dim, 4.0, 3.0));
    let shank = Rect::from_min_max(pos2(cx - rs, top), pos2(cx + rs, flute_top));
    painter.rect(shank, 0.0, tokens.icon_face, ink, egui::StrokeKind::Middle);
    let flutes = Rect::from_min_max(pos2(cx - r, flute_top), pos2(cx + r, tip));
    painter.rect(flutes, 0.0, tokens.accent.gamma_multiply(0.85), ink, egui::StrokeKind::Middle);
    // Helix strokes across the flutes, one band per flute, as on a drawing.
    let pitch = (r * 2.0).max(6.0) * 1.2;
    let mut y = tip - 2.0;
    let bands = (t.flutes.clamp(1, 8) as usize).max(1);
    let mut guard = 0;
    while y - r * 0.7 > flute_top && guard < 400 {
        for k in 0..bands {
            let off = pitch * k as f32 / bands as f32;
            let (y0, y1) = (y - off, y - off - r * 0.7);
            if y1 > flute_top + 1.0 {
                painter
                    .line_segment([pos2(cx - r + 1.5, y0), pos2(cx + r - 1.5, y1)], Stroke::new(1.0, tokens.panel_bg));
            }
        }
        y -= pitch;
        guard += 1;
    }

    // Dimensions: Ø below the tip, flute length and overall length on the right.
    let label = |mm: f64| units.plain(mm) + " " + units.length();
    let dia_y = tip + 12.0;
    arrow_line(&painter, pos2(cx - r, dia_y), pos2(cx + r, dia_y), dim);
    painter.text(
        pos2(cx, dia_y + 2.0),
        Align2::CENTER_TOP,
        format!("Ø {}", label(t.diameter)),
        font.clone(),
        tokens.text,
    );
    let x1 = cx + width.max(t.diameter) as f32 * scale / 2.0 + 16.0;
    arrow_line(&painter, pos2(x1, flute_top), pos2(x1, tip), Stroke::new(1.0, tokens.accent));
    painter.text(
        pos2(x1 + 5.0, (flute_top + tip) / 2.0),
        Align2::LEFT_CENTER,
        label(t.flute_length),
        font.clone(),
        tokens.accent,
    );
    let x2 = x1 + 44.0;
    arrow_line(&painter, pos2(x2, top), pos2(x2, tip), dim);
    painter.text(pos2(x2 + 5.0, (top + tip) / 2.0), Align2::LEFT_CENTER, label(t.overall_length), font, tokens.text);
}

/// A dimension line with arrow heads at both ends.
fn arrow_line(painter: &egui::Painter, a: Pos2, b: Pos2, stroke: Stroke) {
    painter.line_segment([a, b], stroke);
    let d = (b - a).normalized() * 5.0;
    let n = vec2(-d.y, d.x) * 0.5;
    for (p, dir) in [(a, d), (b, -d)] {
        painter.add(Shape::convex_polygon(vec![p, p + dir + n, p + dir - n], stroke.color, Stroke::NONE));
    }
}
