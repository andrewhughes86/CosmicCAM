//! The one set of camera controls. Every canvas with a camera calls [`handle`].
//!
//! - Left drag: orbit
//! - Middle drag, right drag or Shift + left drag: pan
//! - Scroll wheel or pinch: zoom about the cursor

use CosmicCAM_geom::Vec2;
use CosmicCAM_render::OrbitCamera;
use egui::{PointerButton, Response, Ui};

/// What a canvas lets the user do with its camera.
#[derive(Clone, Copy, Debug)]
pub struct NavCaps {
    pub orbit: bool,
    pub pan: bool,
    pub zoom: bool,
}

impl NavCaps {
    pub const FULL: Self = Self { orbit: true, pan: true, zoom: true };
}

/// Applies this frame's pointer input on `response` to `camera`. Returns true if it moved.
pub fn handle(ui: &Ui, response: &Response, camera: &mut OrbitCamera, caps: NavCaps) -> bool {
    let rect = response.rect;
    if !(rect.width() > 1.0 && rect.height() > 1.0) {
        return false;
    }
    let before = *camera;
    let shift = ui.input(|i| i.modifiers.shift);
    let delta = response.drag_delta();
    let delta = Vec2::new(delta.x, delta.y);

    let panning = response.dragged_by(PointerButton::Middle)
        || response.dragged_by(PointerButton::Secondary)
        || (shift && response.dragged_by(PointerButton::Primary));
    if panning && caps.pan {
        camera.pan(delta, rect.height());
    } else if response.dragged_by(PointerButton::Primary) && caps.orbit {
        camera.orbit(delta);
    }

    if caps.zoom && response.hovered() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let mut factor = (-scroll * 0.0025).exp();
        if pinch.is_finite() && pinch > 0.0 {
            factor /= pinch;
        }
        if factor.is_finite() && (factor - 1.0).abs() > 1.0e-5 {
            let at = ui
                .input(|i| i.pointer.hover_pos())
                .map(|p| {
                    let x = (p.x - rect.center().x) / (rect.width() * 0.5);
                    let y = -(p.y - rect.center().y) / (rect.height() * 0.5);
                    Vec2::new(x, y)
                })
                .unwrap_or(Vec2::ZERO);
            camera.zoom_at(factor, at, rect.aspect_ratio());
        }
    }
    *camera = camera.sanitized();
    *camera != before
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Camera input is read only here: other modules must not hand-roll drag or scroll handling.
    #[test]
    fn no_hand_rolled_camera_input() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.file_name().is_some_and(|n| n == "navigation.rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                for needle in ["drag_delta", "scroll_delta", "zoom_delta", ".orbit(", ".pan(", ".zoom_at("] {
                    if text.contains(needle) {
                        offenders.push(format!("{} uses {needle}", path.display()));
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "camera input outside navigation.rs:\n{}", offenders.join("\n"));
    }
}
