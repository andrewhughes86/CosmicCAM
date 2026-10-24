//! Painted viewport overlays: the view cube, the orientation triad and the readout.

use CosmicCAM_geom::{Vec2 as GVec2, Vec3};
use CosmicCAM_render::OrbitCamera;
use CosmicCAM_render::view_cube::{self, CORNER_RADIUS, CubeHit};
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};

use crate::theme::Tokens;

const CUBE_BOX: f32 = 118.0;
const CUBE_MARGIN: f32 = 12.0;
/// Pixels per cube unit; the cube's projected extent never exceeds sqrt(3).
const CUBE_SCALE: f32 = CUBE_BOX * 0.5 / 1.9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CubeHover {
    pub corner: bool,
    preset: CosmicCAM_render::ViewPreset,
}

impl CubeHover {
    pub fn preset_id(self) -> &'static str {
        self.preset.id()
    }
}

pub fn cube_rect(viewport: Rect) -> Rect {
    Rect::from_min_size(
        pos2(viewport.right() - CUBE_BOX - CUBE_MARGIN, viewport.top() + CUBE_MARGIN),
        vec2(CUBE_BOX, CUBE_BOX),
    )
}

fn to_screen(rect: Rect, p: GVec2) -> Pos2 {
    rect.center() + vec2(p.x * CUBE_SCALE, -p.y * CUBE_SCALE)
}

fn to_cube(rect: Rect, p: Pos2) -> GVec2 {
    let d = p - rect.center();
    GVec2::new(d.x / CUBE_SCALE, -d.y / CUBE_SCALE)
}

pub fn cube_hit(camera: &OrbitCamera, rect: Rect, pointer: Pos2) -> Option<CubeHover> {
    view_cube::hit(camera, to_cube(rect, pointer))
        .map(|h| CubeHover { corner: matches!(h, CubeHit::Corner(_)), preset: h.preset() })
}

pub fn paint_view_cube(painter: &Painter, rect: Rect, camera: &OrbitCamera, t: &Tokens, hover: Option<CubeHover>) {
    let edge = Stroke::new(1.0, t.cube_edge);
    for face in view_cube::visible_faces(camera) {
        let hovered = hover.is_some_and(|h| !h.corner && h.preset == face.preset);
        let base = if hovered { t.cube_face_hover } else { t.cube_face };
        // Faces turned away from the viewer read a little darker.
        let fill = lerp_color(t.cube_edge, base, 0.55 + 0.45 * face.facing);
        let points: Vec<Pos2> = face.corners.iter().map(|&c| to_screen(rect, c)).collect();
        painter.add(Shape::convex_polygon(points.clone(), fill, edge));
        if face.facing > 0.3 {
            let centre = points.iter().fold(Vec2::ZERO, |acc, p| acc + p.to_vec2()) / 4.0;
            let size = 9.0 + 3.0 * face.facing;
            let text = lerp_color(fill, t.cube_text, face.facing.min(1.0));
            painter.text(centre.to_pos2(), Align2::CENTER_CENTER, face.label, FontId::proportional(size), text);
        }
    }
    for corner in view_cube::visible_corners(camera) {
        let hovered = hover.is_some_and(|h| h.corner && h.preset == corner.preset);
        let at = to_screen(rect, corner.at);
        let r = CORNER_RADIUS * CUBE_SCALE * 0.42;
        if hovered {
            painter.circle(at, r, t.cube_face_hover, Stroke::new(1.0, t.accent));
        } else {
            painter.circle_stroke(at, r * 0.55, Stroke::new(1.0, t.cube_edge));
        }
    }
}

pub fn triad_anchor(viewport: Rect) -> Pos2 {
    pos2(viewport.left() + 52.0, viewport.bottom() - 52.0)
}

/// Screen-aligned XYZ triad showing the world orientation.
pub fn paint_triad(painter: &Painter, at: Pos2, camera: &OrbitCamera, t: &Tokens) {
    const LEN: f32 = 34.0;
    let b = camera.basis();
    let mut axes = [(Vec3::X, "X", t.axis_x), (Vec3::Y, "Y", t.axis_y), (Vec3::Z, "Z", t.axis_z)];
    // Paint the axis pointing away from the viewer first.
    axes.sort_by(|a, z| z.0.dot(b.forward).total_cmp(&a.0.dot(b.forward)));
    painter.circle_filled(at, 3.0, t.text_muted);
    for (axis, label, color) in axes {
        let d = vec2(axis.dot(b.right), -axis.dot(b.up));
        let tip = at + d * LEN;
        let faded = d.length() < 0.15;
        let color = if faded { color.gamma_multiply(0.4) } else { color };
        painter.line_segment([at, tip], Stroke::new(2.5, color));
        painter.circle_filled(tip, 2.5, color);
        let label_at = at + d.normalized() * (LEN + 9.0);
        let label_at = if d.length() < 0.15 { at + vec2(9.0, -9.0) } else { label_at };
        painter.text(label_at, Align2::CENTER_CENTER, label, FontId::monospace(11.0), color);
    }
}

/// Projection and frame-rate readout in the corner.
pub fn paint_readout(painter: &Painter, at: Pos2, camera: &OrbitCamera, fps: f32, limit: u32, t: &Tokens) {
    let projection = match camera.projection {
        CosmicCAM_render::Projection::Orthographic => "Orthographic",
        CosmicCAM_render::Projection::Perspective => "Perspective",
    };
    let text = format!("{projection}  ·  {fps:.0} fps (limit {limit})");
    painter.text(at, Align2::RIGHT_BOTTOM, text, FontId::proportional(11.0), t.text_muted);
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()), mix(a.a(), b.a()))
}
