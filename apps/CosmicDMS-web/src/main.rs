//! CosmicDMS in the browser: the same app as the desktop, drawn into a canvas with wgpu
//! (WebGPU, or WebGL 2 where WebGPU is missing). Built with `trunk` from `index.html`.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
#![allow(non_snake_case)]

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;

    eframe::WebLogger::init(log::LevelFilter::Warn).ok();
    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window().and_then(|w| w.document());
        let canvas = document
            .as_ref()
            .and_then(|d| d.get_element_by_id("cosmicdms-canvas"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok());
        let Some(canvas) = canvas else {
            log::error!("CosmicDMS: no <canvas id=\"cosmicdms-canvas\"> on the page");
            return;
        };
        let started = CosmicDMS_ui_egui::run_web(canvas, CosmicDMS_ui_egui::Session::new()).await;
        // Replace the loading note with the outcome.
        if let Some(note) = document.as_ref().and_then(|d| d.get_element_by_id("cosmicdms-loading")) {
            match &started {
                Ok(()) => note.remove(),
                Err(e) => note.set_text_content(Some(&format!(
                    "CosmicDMS could not start: {e}. It needs a browser with WebGPU or WebGL 2."
                ))),
            }
        }
        if let Err(e) = started {
            log::error!("CosmicDMS could not start: {e}");
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("CosmicDMS-web is the browser build; build it with `trunk build` in apps/CosmicDMS-web.");
}
