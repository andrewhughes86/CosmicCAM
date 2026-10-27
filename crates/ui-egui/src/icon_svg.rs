//! Command icons: the SVG masters in `assets/icons/ui/design/svg/`, embedded at build time,
//! coloured from the active theme and rasterised on demand.
//!
//! Masters use colour-role placeholders (`#INK`, `#FACE`, `#SHADE`, `#ACCENT`, `#OK`, `#STOP`,
//! `#WARN`, `#BG`) — see `assets/icons/ui/STYLE_GUIDE.md`.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use CosmicDMS_engine::CommandSpec;
use egui::{Color32, ColorImage, TextureHandle, TextureId, TextureOptions};
use resvg::{tiny_skia, usvg};

use crate::theme::Tokens;

include!(concat!(env!("OUT_DIR"), "/icons.rs"));

/// Master grid size.
const MASTER: f32 = 32.0;
/// Largest raster made, in pixels.
const MAX_PX: u32 = 256;

pub fn source(name: &str) -> Option<&'static str> {
    ICONS.binary_search_by(|(n, _)| (*n).cmp(name)).ok().and_then(|i| ICONS.get(i)).map(|(_, svg)| *svg)
}

/// The icon a command shows: its explicit `icon`, else the file named after its id
/// (`toolpath.pocket` → `toolpath_pocket`).
pub fn name_for(spec: &CommandSpec) -> String {
    spec.icon.map_or_else(|| spec.id.replace(['.', '-'], "_"), str::to_owned)
}

fn hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// Replaces the colour roles with the theme's colours.
pub fn colourise(svg: &str, t: &Tokens) -> String {
    let roles = [
        ("#ACCENT", t.accent),
        ("#SHADE", t.icon_shade),
        ("#FACE", t.icon_face),
        ("#STOP", t.error),
        ("#WARN", t.warning),
        ("#INK", t.text),
        ("#OK", t.success),
        ("#BG", t.panel_bg),
    ];
    let mut out = svg.to_owned();
    for (role, colour) in roles {
        out = out.replace(role, &hex(colour));
    }
    out
}

/// Renders `svg` (already coloured) to a `px` × `px` image.
pub fn rasterise(svg: &str, px: u32) -> Result<ColorImage, String> {
    let px = px.clamp(1, MAX_PX);
    catch_unwind(AssertUnwindSafe(|| {
        let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).map_err(|e| e.to_string())?;
        let mut pixmap = tiny_skia::Pixmap::new(px, px).ok_or("could not allocate the icon raster")?;
        let scale = px as f32 / MASTER;
        resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        Ok(ColorImage::from_rgba_premultiplied([px as usize, px as usize], pixmap.data()))
    }))
    .unwrap_or_else(|_| Err("the SVG renderer failed".into()))
}

/// Rasterised icons per (name, theme, pixel size). Failures are remembered so a bad
/// master is reported once and drawn as a placeholder.
#[derive(Clone, Default)]
pub struct IconCache {
    textures: HashMap<(String, bool, u32), Option<TextureHandle>>,
}

impl IconCache {
    pub fn get(&mut self, ctx: &egui::Context, name: &str, tokens: &Tokens, px: u32) -> Option<TextureId> {
        let px = px.clamp(1, MAX_PX);
        let key = (name.to_owned(), tokens.dark, px);
        if let Some(entry) = self.textures.get(&key) {
            return entry.as_ref().map(TextureHandle::id);
        }
        let handle = source(name).and_then(|svg| match rasterise(&colourise(svg, tokens), px) {
            Ok(image) => Some(ctx.load_texture(format!("icon-{name}-{px}"), image, TextureOptions::LINEAR)),
            Err(e) => {
                log::warn!("icon {name}: {e}");
                None
            }
        });
        let id = handle.as_ref().map(TextureHandle::id);
        self.textures.insert(key, handle);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{DARK, LIGHT};

    #[test]
    fn every_master_renders_in_both_themes() {
        assert!(!ICONS.is_empty());
        for (name, svg) in ICONS {
            for t in [&DARK, &LIGHT] {
                let coloured = colourise(svg, t);
                assert!(!coloured.contains("\"#") || !coloured.contains("#INK"), "{name}");
                let image = rasterise(&coloured, 32);
                assert!(image.is_ok(), "{name}: {image:?}");
                let painted = image.map(|i| i.pixels.iter().filter(|p| p.a() > 0).count()).unwrap_or(0);
                assert!(painted > 40, "{name} is nearly empty ({painted} px)");
            }
        }
    }

    #[test]
    fn masters_use_only_colour_roles() {
        let roles = ["#INK", "#FACE", "#SHADE", "#ACCENT", "#OK", "#STOP", "#WARN", "#BG"];
        for (name, svg) in ICONS {
            assert!(svg.contains("viewBox=\"0 0 32 32\""), "{name} is not on the 32 grid");
            for (i, _) in svg.match_indices('#') {
                let rest = &svg[i..];
                assert!(
                    roles.iter().any(|r| rest.starts_with(r)),
                    "{name} has a literal colour near {}",
                    &rest[..rest.len().min(8)]
                );
            }
        }
    }

    #[test]
    fn every_ribbon_command_has_an_icon() {
        let missing: Vec<String> = CosmicDMS_engine::cmd::all()
            .filter(|c| c.placement.is_some())
            .map(name_for)
            .filter(|n| source(n).is_none())
            .collect();
        assert!(missing.is_empty(), "ribbon commands without icons: {missing:?}");
    }

    /// `ICON_SHEET=/path/sheet.png cargo test -p CosmicDMS-ui-egui icon_sheet` writes a review
    /// sheet: each icon at 64, 32 and 16 px, dark theme above light.
    #[test]
    fn icon_sheet() {
        let Ok(path) = std::env::var("ICON_SHEET") else { return };
        let (cw, ch, cols) = (128usize, 72usize, 8usize);
        let rows_per_theme = ICONS.len().div_ceil(cols);
        let width = cols * cw;
        let mut sheet = ColorImage::filled([width, rows_per_theme * 2 * ch], Color32::BLACK);
        for (half, t) in [&DARK, &LIGHT].into_iter().enumerate() {
            for (i, (_, svg)) in ICONS.iter().enumerate() {
                let (x0, y0) = ((i % cols) * cw, (i / cols + half * rows_per_theme) * ch);
                for y in 0..ch {
                    for x in 0..cw {
                        sheet.pixels[(y0 + y) * width + x0 + x] = t.panel_bg;
                    }
                }
                for (size, ox) in [(64usize, 4usize), (32, 74), (16, 108)] {
                    let Ok(img) = rasterise(&colourise(svg, t), size as u32) else { continue };
                    for y in 0..size {
                        for x in 0..size {
                            let src = img.pixels[y * size + x];
                            let idx = (y0 + 4 + y) * width + x0 + ox + x;
                            let dst = sheet.pixels[idx];
                            let keep = 255 - src.a() as u32;
                            let mix = |s: u8, d: u8| (s as u32 + d as u32 * keep / 255).min(255) as u8;
                            sheet.pixels[idx] =
                                Color32::from_rgb(mix(src.r(), dst.r()), mix(src.g(), dst.g()), mix(src.b(), dst.b()));
                        }
                    }
                }
            }
        }
        let _ = crate::automation::save_png(&sheet, std::path::Path::new(&path));
    }
}
