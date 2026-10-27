//! L5 desktop UI on egui + wgpu. Swappable: everything below talks only to the engine.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
#![allow(non_snake_case)]

mod app;
mod automation;
mod cad_panel;
#[cfg(not(target_arch = "wasm32"))]
pub mod control;
#[cfg(target_arch = "wasm32")]
#[path = "control_web.rs"]
pub mod control;
mod dock;
pub mod icon_svg;
pub mod navigation;
mod panes;
mod param_form;
mod ribbon;
mod setup_panel;
pub mod theme;
mod tool_view;
mod toolpath_panel;
mod tools;
mod trim_panel;
pub mod ui_kit;
mod viewport;
mod wcs_tool;

pub use CosmicDMS_engine::Session;
pub use app::{CosmicApp, LaunchOptions};
pub use automation::Automation;
pub use panes::parse_console_line;

/// A command for the app to run after this frame: id and JSON parameters.
pub type Request = (String, serde_json::Value);

pub const APP_ID: &str = "CosmicDMS";

/// The application icon's master (`assets/icons/app/cosmicdms.svg`, a 256-unit square).
pub const APP_ICON_SVG: &str = include_str!("../../../assets/icons/app/cosmicdms.svg");

/// The application icon for the window and the taskbar, rendered at `px` pixels; None if it
/// cannot be drawn (the window then has the system's default).
pub fn app_icon(px: u32) -> Option<egui::IconData> {
    use resvg::{tiny_skia, usvg};
    let px = px.clamp(16, 1024);
    std::panic::catch_unwind(|| {
        let tree = usvg::Tree::from_str(APP_ICON_SVG, &usvg::Options::default()).ok()?;
        let mut pixmap = tiny_skia::Pixmap::new(px, px)?;
        let scale = px as f32 / 256.0;
        resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        // The window system wants straight (not premultiplied) alpha.
        let rgba = pixmap.pixels().iter().flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        });
        Some(egui::IconData { rgba: rgba.collect(), width: px, height: px })
    })
    .ok()
    .flatten()
}

/// Native window configuration: wgpu renderer, sensible default and minimum sizes.
#[cfg(not(target_arch = "wasm32"))]
pub fn native_options() -> eframe::NativeOptions {
    let viewport = egui::ViewportBuilder::default();
    let viewport = match app_icon(256) {
        Some(icon) => viewport.with_icon(std::sync::Arc::new(icon)),
        None => viewport,
    };
    eframe::NativeOptions {
        viewport: viewport
            .with_title("CosmicDMS")
            // The ribbon's tab strip is the title bar, with its own window controls.
            .with_decorations(false)
            .with_app_id(APP_ID)
            .with_inner_size([1600.0, 1000.0])
            .with_min_inner_size([960.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        // The viewport renders offscreen with its own depth and MSAA targets.
        depth_buffer: 0,
        multisampling: 0,
        centered: true,
        ..Default::default()
    }
}

/// Opens the desktop window and runs until it closes.
#[cfg(not(target_arch = "wasm32"))]
pub fn run(session: Session, options: LaunchOptions) -> eframe::Result {
    eframe::run_native(APP_ID, native_options(), Box::new(move |cc| Ok(Box::new(CosmicApp::new(cc, session, options)))))
}

/// Starts the app in a browser page, drawing into the canvas with id `canvas_id`.
#[cfg(target_arch = "wasm32")]
pub async fn run_web(canvas: eframe::web_sys::HtmlCanvasElement, session: Session) -> Result<(), String> {
    let options = eframe::WebOptions::default();
    eframe::WebRunner::new()
        .start(
            canvas,
            options,
            Box::new(move |cc| {
                Ok(Box::new(CosmicApp::new(cc, session, LaunchOptions { demo: true, ..LaunchOptions::default() })))
            }),
        )
        .await
        .map_err(|e| format!("{e:?}"))
}

#[cfg(test)]
mod app_icon_tests {
    #[test]
    fn the_app_icon_renders() {
        let icon = super::app_icon(64);
        assert!(icon.as_ref().is_some_and(|i| i.width == 64 && i.rgba.len() == 64 * 64 * 4));
        // Opaque in the middle, clear in the corners (a rounded tile).
        let px = |x: usize, y: usize| icon.as_ref().map_or(0, |i| i.rgba[(y * 64 + x) * 4 + 3]);
        assert_eq!(px(32, 32), 255);
        assert_eq!(px(0, 0), 0);
    }
}
