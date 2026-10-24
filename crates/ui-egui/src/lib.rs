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
pub mod control;
mod dock;
pub mod icon_svg;
pub mod navigation;
mod panes;
mod ribbon;
pub mod theme;
pub mod ui_kit;
mod viewport;

pub use CosmicCAM_engine::Session;
pub use app::{CosmicApp, LaunchOptions};
pub use automation::Automation;
pub use panes::parse_console_line;

/// A command for the app to run after this frame: id and JSON parameters.
pub type Request = (String, serde_json::Value);

pub const APP_ID: &str = "CosmicCAM";

/// Native window configuration: wgpu renderer, sensible default and minimum sizes.
pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("CosmicCAM")
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
pub fn run(session: Session, options: LaunchOptions) -> eframe::Result {
    eframe::run_native(APP_ID, native_options(), Box::new(move |cc| Ok(Box::new(CosmicApp::new(cc, session, options)))))
}
