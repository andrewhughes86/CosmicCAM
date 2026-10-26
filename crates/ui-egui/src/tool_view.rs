//! A to-scale side view of a cutting tool, as the tool dialogs and the tool library show it.
//!
//! The outline is the engine's [`Tool::profile`], the same one the simulation draws and the
//! multi-axis collision check uses, so the picture is the tool's real geometry: the fluted part
//! in the accent colour, the neck and shank above it, and the holder (when it has one) above the
//! holder face. Dimension lines give the cutting diameter, the flute length, the stick-out and
//! what the type adds (corner radius, point angle, reach).

use CosmicCAM_engine::library::{Holder, Tool};
use CosmicCAM_engine::units::DisplayUnits;
use egui::{Align2, Color32, FontId, Mesh, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

use crate::theme::Tokens;

/// Fills the outline `points` (screen positions of one side, in order along the axis), mirrored
/// about `cx`, as a strip of trapezoids, each coloured by `colour(its middle's screen y)`.
fn fill(painter: &egui::Painter, points: &[Pos2], cx: f32, colour: impl Fn(f32) -> Color32) {
    let mut mesh = Mesh::default();
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        if (a.y - b.y).abs() < 1.0e-3 {
            continue;
        }
        let c = colour((a.y + b.y) / 2.0);
        let base = mesh.vertices.len() as u32;
        for p in [pos2(cx - (a.x - cx), a.y), pos2(a.x, a.y), pos2(b.x, b.y), pos2(cx - (b.x - cx), b.y)] {
            mesh.colored_vertex(p, c);
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(Shape::mesh(mesh));
}

/// The closed outline of `points` mirrored about `cx`.
fn outline(points: &[Pos2], cx: f32) -> Vec<Pos2> {
    let mut out: Vec<Pos2> = points.to_vec();
    out.extend(points.iter().rev().map(|p| pos2(cx - (p.x - cx), p.y)));
    out
}

/// Draws `tool` (and `holder`) filling `size` (in points), tip down, holder above.
pub fn draw(
    ui: &mut Ui,
    tool: Option<&Tool>,
    holder: Option<&Holder>,
    units: DisplayUnits,
    tokens: &Tokens,
    size: egui::Vec2,
) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, tokens.app_bg);
    let Some(t) = tool.filter(|t| t.validate().is_ok()) else {
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
    let profile = t.profile();
    let holder_profile = holder.map(Holder::profile).unwrap_or_default();
    let holder_height = holder.map_or(0.0, Holder::height);
    // Show only the bottom of the holder, so it does not shrink the tool to nothing.
    let shown_holder = holder_height.min(t.overall_length * 0.35);

    // One scale for both directions, leaving room for the dimension labels.
    let widest = profile
        .points
        .iter()
        .map(|p| p[0])
        .chain(holder_profile.iter().filter(|p| p[1] <= shown_holder).map(|p| p[0]))
        .fold(t.diameter / 2.0, f64::max);
    let area = Rect::from_min_max(rect.min + vec2(18.0, 22.0), rect.max - vec2(78.0, 26.0));
    let total = t.overall_length + shown_holder;
    let scale = (f64::from(area.height()) / total).min(f64::from(area.width()) / (2.0 * widest)) as f32;
    let px = |mm: f64| mm as f32 * scale;
    let cx = area.center().x;
    let tip = area.top() + (area.height() + px(total)) / 2.0;
    let face = tip - px(t.overall_length);
    let flute_top = tip - px(profile.flute_top);
    let to_screen = |p: &[f64; 2]| pos2(cx + px(p[0]), tip - px(p[1]));

    // The holder, cut off at what is shown, then the tool: flutes in the accent colour.
    if holder.is_some() {
        let pts: Vec<Pos2> =
            holder_profile.iter().map(|p| pos2(cx + px(p[0]), face - px(p[1].min(shown_holder)))).collect();
        fill(&painter, &pts, cx, |_| tokens.icon_shade);
        painter.add(Shape::closed_line(outline(&pts, cx), dim));
    } else {
        painter.add(Shape::dashed_line(
            &[pos2(cx - px(widest) - 14.0, face), pos2(cx + px(widest) + 14.0, face)],
            dim,
            4.0,
            3.0,
        ));
    }
    let pts: Vec<Pos2> = profile.points.iter().map(to_screen).collect();
    let accent = tokens.accent.gamma_multiply(0.85);
    let shank = tokens.icon_face;
    fill(&painter, &pts, cx, |mid| if mid >= flute_top - 0.5 { accent } else { shank });
    painter.add(Shape::closed_line(outline(&pts, cx), ink));

    // Dimensions: Ø below the tip, flute length and stick-out on the right, type extras left.
    let label = |mm: f64| units.plain(mm) + " " + units.length();
    let r = px(t.diameter / 2.0);
    let dia_y = tip + 12.0;
    arrow_line(&painter, pos2(cx - r, dia_y), pos2(cx + r, dia_y), dim);
    painter.text(
        pos2(cx, dia_y + 2.0),
        Align2::CENTER_TOP,
        format!("Ø {}", label(t.diameter)),
        font.clone(),
        tokens.text,
    );
    // Dimensions go beside the tool, not its (often wider) holder.
    let tool_widest = profile.points.iter().map(|p| p[0]).fold(t.diameter / 2.0, f64::max);
    let x1 = cx + px(tool_widest) + 16.0;
    arrow_line(&painter, pos2(x1, flute_top), pos2(x1, tip), Stroke::new(1.0, tokens.accent));
    painter.text(
        pos2(x1 + 5.0, (flute_top + tip) / 2.0),
        Align2::LEFT_CENTER,
        label(t.flute_length),
        font.clone(),
        tokens.accent,
    );
    let x2 = x1 + 44.0;
    arrow_line(&painter, pos2(x2, face), pos2(x2, tip), dim);
    painter.text(
        pos2(x2 + 5.0, (face + tip) / 2.0),
        Align2::LEFT_CENTER,
        label(t.overall_length),
        font.clone(),
        tokens.text,
    );
    let mut notes: Vec<String> = Vec::new();
    if t.corner_radius > 0.0 && !t.ball_ended() {
        notes.push(format!("R {}", label(t.corner_radius)));
    }
    if t.tip_angle > 0.0 {
        notes.push(format!("{}°", trim(t.tip_angle)));
    }
    if t.taper_angle > 0.0 {
        notes.push(format!("{}° taper", trim(t.taper_angle)));
    }
    if t.neck_diameter > 0.0 {
        notes.push(format!("neck Ø {}", label(t.neck_diameter)));
    }
    if t.thread_pitch > 0.0 {
        notes.push(format!("P {}", label(t.thread_pitch)));
    }
    if t.shoulder_length > 0.0 {
        notes.push(format!("reach {}", label(t.shoulder_length)));
    }
    for (i, n) in notes.iter().enumerate() {
        painter.text(
            rect.left_top() + vec2(6.0, 6.0 + 13.0 * i as f32),
            Align2::LEFT_TOP,
            n,
            font.clone(),
            tokens.text_muted,
        );
    }
    if let Some(h) = holder {
        painter.text(rect.right_top() + vec2(-6.0, 6.0), Align2::RIGHT_TOP, h.name.as_str(), font, tokens.text_muted);
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
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
