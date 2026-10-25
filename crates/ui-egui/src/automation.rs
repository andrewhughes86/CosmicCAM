//! Launch-time automation: run commands, then optionally save a screenshot and quit.
//! Used for review evidence and smoke tests; the control channel does the same at runtime.

use std::path::{Path, PathBuf};

use egui::ColorImage;

use crate::Request;

/// Frames to let layout settle before running steps.
const SETTLE_FRAMES: u32 = 5;
/// Frames between the last step and the screenshot, so camera animations finish.
const CAPTURE_DELAY_FRAMES: u32 = 90;

#[derive(Default)]
pub struct Automation {
    pub steps: Vec<Request>,
    pub screenshot: Option<PathBuf>,
    /// Ribbon tab to show, by name (case-insensitive).
    pub ribbon_tab: Option<String>,
    frame: u32,
    ran: bool,
}

/// What automation wants from the app this frame.
#[derive(Default)]
pub struct Step {
    pub requests: Vec<Request>,
    /// Capture the window to this file, then quit.
    pub capture: Option<PathBuf>,
}

impl Automation {
    pub fn new(steps: Vec<Request>, screenshot: Option<PathBuf>) -> Self {
        Self { steps, screenshot, ..Default::default() }
    }

    /// Called once per frame.
    pub fn tick(&mut self, ctx: &egui::Context) -> Step {
        if self.ran && self.screenshot.is_none() {
            return Step::default();
        }
        self.frame = self.frame.saturating_add(1);
        ctx.request_repaint();
        let mut step = Step::default();
        if !self.ran && self.frame >= SETTLE_FRAMES {
            self.ran = true;
            step.requests = std::mem::take(&mut self.steps);
        }
        if self.frame >= SETTLE_FRAMES + CAPTURE_DELAY_FRAMES {
            step.capture = self.screenshot.take();
        }
        step
    }
}

/// Writes `image` as an RGBA PNG.
pub fn save_png(image: &ColorImage, path: &Path) -> Result<(), String> {
    let [w, h] = image.size;
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    // Fast compression keeps screenshots and recordings from stalling the frame.
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
    writer.write_image_data(&bytes).map_err(|e| e.to_string())
}
