//! Theme tokens: the only place UI colours are defined.
//!
//! Each palette is a set of semantic tokens; [`apply`] maps them onto `egui::Visuals`
//! (inheriting everything else from egui's own dark or light visuals) and
//! [`Tokens::scene_palette`] maps them into the 3D viewport.

use CosmicCAM_engine::Theme;
use CosmicCAM_render::ScenePalette;
use egui::{Color32, CornerRadius, Rgba, Stroke, Visuals};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub dark: bool,
    /// Window background behind every panel.
    pub app_bg: Color32,
    /// Panel and tab body surfaces.
    pub panel_bg: Color32,
    /// Raised surfaces: ribbon, tab bar, inputs, cards.
    pub raised_bg: Color32,
    pub hover_bg: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    /// Primary accent: selection, focus, active tab.
    pub accent: Color32,
    /// Secondary accent for emphasis (strong buttons, links).
    pub accent_strong: Color32,
    /// Text drawn on an accent fill.
    pub on_accent: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub viewport_top: Color32,
    pub viewport_bottom: Color32,
    pub grid_minor: Color32,
    pub grid_major: Color32,
    pub axis_x: Color32,
    pub axis_y: Color32,
    pub axis_z: Color32,
    pub part: Color32,
    pub part_edge: Color32,
    pub stock_edge: Color32,
    pub cube_face: Color32,
    pub cube_face_hover: Color32,
    pub cube_edge: Color32,
    pub cube_text: Color32,
    /// Lit faces of solids in icons (`#FACE`).
    pub icon_face: Color32,
    /// Faces turned from the light in icons (`#SHADE`).
    pub icon_shade: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Modern dark (default): deep slate with electric blue / cyan accents.
pub const DARK: Tokens = Tokens {
    dark: true,
    app_bg: hex(0x0A101C),
    panel_bg: hex(0x0F1726),
    raised_bg: hex(0x162133),
    hover_bg: hex(0x213049),
    border: hex(0x2A3A55),
    text: hex(0xE6ECF5),
    text_muted: hex(0x98A6BC),
    accent: hex(0x22D3EE),
    accent_strong: hex(0x3B82F6),
    on_accent: hex(0x04121A),
    success: hex(0x34D399),
    warning: hex(0xFBBF24),
    error: hex(0xF87171),
    viewport_top: hex(0x1C2840),
    viewport_bottom: hex(0x080D17),
    grid_minor: hex(0x1F2B40),
    grid_major: hex(0x33445F),
    axis_x: hex(0xF0525B),
    axis_y: hex(0x3DD68C),
    axis_z: hex(0x4C8DFF),
    part: hex(0x9FB0C8),
    part_edge: hex(0x0B1220),
    stock_edge: hex(0x22D3EE),
    cube_face: hex(0x3A5075),
    cube_face_hover: hex(0x0E7490),
    cube_edge: hex(0x9AA9C2),
    cube_text: hex(0xFFFFFF),
    icon_face: hex(0x3A4C6A),
    icon_shade: hex(0x22304A),
};

/// Light: off-white surfaces, white panels, charcoal type for bright shop floors.
pub const LIGHT: Tokens = Tokens {
    dark: false,
    app_bg: hex(0xE9EAE6),
    panel_bg: hex(0xFFFFFF),
    raised_bg: hex(0xF6F6F3),
    hover_bg: hex(0xE4E7EB),
    border: hex(0xC5CBD3),
    text: hex(0x1C2025),
    text_muted: hex(0x505A66),
    accent: hex(0x0369A1),
    accent_strong: hex(0x1D4ED8),
    on_accent: hex(0xFFFFFF),
    success: hex(0x047857),
    warning: hex(0xA16207),
    error: hex(0xB91C1C),
    viewport_top: hex(0xFAFBFC),
    viewport_bottom: hex(0xD5DBE3),
    grid_minor: hex(0xC9CFD8),
    grid_major: hex(0x9EA8B5),
    axis_x: hex(0xD12F3A),
    axis_y: hex(0x16924A),
    axis_z: hex(0x2563EB),
    part: hex(0x8D9BAE),
    part_edge: hex(0x27303B),
    stock_edge: hex(0x0369A1),
    cube_face: hex(0xF1F3F6),
    cube_face_hover: hex(0xBAE6FD),
    cube_edge: hex(0x4B5563),
    cube_text: hex(0x0B0D10),
    icon_face: hex(0xFFFFFF),
    icon_shade: hex(0xC4CDD8),
};

impl Tokens {
    pub fn for_theme(theme: Theme) -> &'static Tokens {
        match theme {
            Theme::Dark => &DARK,
            Theme::Light => &LIGHT,
        }
    }

    pub fn scene_palette(&self) -> ScenePalette {
        ScenePalette {
            grid_minor: linear(self.grid_minor),
            grid_major: linear(self.grid_major),
            axis_x: linear(self.axis_x),
            axis_y: linear(self.axis_y),
            axis_z: linear(self.axis_z),
            part: linear(self.part),
            part_edge: linear(self.part_edge),
            stock_edge: linear(self.stock_edge.gamma_multiply(0.85)),
            path_cut: linear(self.accent_strong),
            path_rapid: linear(self.warning.gamma_multiply(0.8)),
            path_plunge: linear(self.error),
            path_lead: linear(self.success),
            path_link: linear(self.text_muted),
        }
    }
}

/// sRGB colour to linear RGBA for the GPU.
pub fn linear(c: Color32) -> [f32; 4] {
    Rgba::from(c).to_array()
}

/// Installs `tokens` as the egui visuals.
pub fn apply(ctx: &egui::Context, tokens: &Tokens) {
    ctx.set_visuals(visuals(tokens));
}

pub fn visuals(t: &Tokens) -> Visuals {
    let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
    let radius = CornerRadius::same(4);
    v.override_text_color = Some(t.text);
    v.weak_text_color = Some(t.text_muted);
    v.hyperlink_color = t.accent_strong;
    v.faint_bg_color = t.raised_bg;
    v.extreme_bg_color = t.app_bg;
    v.text_edit_bg_color = Some(t.app_bg);
    v.code_bg_color = t.raised_bg;
    v.warn_fg_color = t.warning;
    v.error_fg_color = t.error;
    v.panel_fill = t.panel_bg;
    v.window_fill = t.panel_bg;
    v.window_stroke = Stroke::new(1.0, t.border);
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = radius;
    v.selection.bg_fill = t.accent.gamma_multiply(if t.dark { 0.45 } else { 0.25 });
    v.selection.stroke = Stroke::new(1.0, t.accent);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.panel_bg;
    w.noninteractive.weak_bg_fill = t.panel_bg;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    w.noninteractive.corner_radius = radius;

    w.inactive.bg_fill = t.raised_bg;
    w.inactive.weak_bg_fill = t.raised_bg;
    w.inactive.bg_stroke = Stroke::new(1.0, t.border);
    w.inactive.fg_stroke = Stroke::new(1.0, t.text);
    w.inactive.corner_radius = radius;

    w.hovered.bg_fill = t.hover_bg;
    w.hovered.weak_bg_fill = t.hover_bg;
    w.hovered.bg_stroke = Stroke::new(1.0, t.accent);
    w.hovered.fg_stroke = Stroke::new(1.5, t.text);
    w.hovered.corner_radius = radius;

    // egui also draws strong text with the active foreground, so it stays the text colour.
    w.active.bg_fill = t.accent_strong.gamma_multiply(0.45);
    w.active.weak_bg_fill = t.accent_strong.gamma_multiply(0.45);
    w.active.bg_stroke = Stroke::new(1.0, t.accent);
    w.active.fg_stroke = Stroke::new(2.0, t.text);
    w.active.corner_radius = radius;

    w.open.bg_fill = t.hover_bg;
    w.open.weak_bg_fill = t.hover_bg;
    w.open.bg_stroke = Stroke::new(1.0, t.accent);
    w.open.fg_stroke = Stroke::new(1.0, t.text);
    w.open.corner_radius = radius;
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color32) -> f32 {
        let [r, g, b, _] = linear(c);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_meets_wcag_aa_on_every_surface() {
        for t in [&DARK, &LIGHT] {
            for bg in [t.app_bg, t.panel_bg, t.raised_bg, t.hover_bg] {
                assert!(contrast(t.text, bg) >= 7.0, "text on {bg:?}: {}", contrast(t.text, bg));
                assert!(contrast(t.text_muted, bg) >= 4.5, "muted on {bg:?}: {}", contrast(t.text_muted, bg));
            }
            assert!(contrast(t.on_accent, t.accent) >= 4.5, "on_accent: {}", contrast(t.on_accent, t.accent));
            assert!(contrast(t.cube_text, t.cube_face) >= 4.5);
        }
    }

    #[test]
    fn palettes_differ_where_it_matters() {
        const { assert!(DARK.dark && !LIGHT.dark) };
        assert!(luminance(DARK.panel_bg) < 0.05);
        assert!(luminance(LIGHT.panel_bg) > 0.9);
    }
}
