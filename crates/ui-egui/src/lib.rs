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
pub mod ui_kit;
mod viewport;
mod wcs_tool;

pub use CosmicCAM_engine::Session;
pub use app::{CosmicApp, LaunchOptions};
pub use automation::Automation;
pub use panes::parse_console_line;

/// A command for the app to run after this frame: id and JSON parameters.
pub type Request = (String, serde_json::Value);

pub const APP_ID: &str = "CosmicCAM";

/// Native window configuration: wgpu renderer, sensible default and minimum sizes.
#[cfg(not(target_arch = "wasm32"))]
pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("CosmicCAM")
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
