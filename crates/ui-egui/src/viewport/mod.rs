//! The main 3D viewport pane.

mod gpu;
mod overlay;

use std::panic::{AssertUnwindSafe, catch_unwind};

use CosmicCAM_engine::{Display, Session, Theme};
use CosmicCAM_geom::{Aabb, Vec3};
use CosmicCAM_render::{OrbitCamera, SceneGeometry, scene};
use egui::{Color32, Pos2, Rect, Sense, Ui, pos2};
use egui_wgpu::RenderState;
use serde_json::Value;

use crate::Request;
use crate::navigation::{self, NavCaps};
use crate::theme::{self, Tokens};

pub struct Viewport {
    gpu: Option<gpu::ViewportGpu>,
    gpu_error: Option<String>,
    /// The camera on screen; eases towards the session's camera.
    shown: OrbitCamera,
    /// Theme, job geometry revision and display flags the uploaded scene was built for.
    scene_key: Option<(Theme, u64, Display)>,
    scene_bounds: Aabb,
    fit_pending: bool,
    last_render_time: Option<f64>,
    fps: f32,
    cube_hover: Option<overlay::CubeHover>,
    /// Scene built before the GPU existed, uploaded once it does.
    pending_upload: Option<(SceneGeometry, SceneGeometry)>,
    animating: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Self::new()
    }
}

impl Viewport {
    pub fn new() -> Self {
        Self {
            gpu: None,
            gpu_error: None,
            shown: OrbitCamera::default(),
            scene_key: None,
            scene_bounds: Aabb::EMPTY,
            fit_pending: true,
            last_render_time: None,
            fps: 0.0,
            cube_hover: None,
            pending_upload: None,
            animating: false,
        }
    }

    /// True while the shown camera is still easing towards the session camera.
    pub fn is_animating(&self) -> bool {
        self.animating
    }

    /// Smoothed frames per second of the viewport.
    pub fn fps(&self) -> f32 {
        self.fps
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        tokens: &Tokens,
        render_state: Option<&RenderState>,
        requests: &mut Vec<Request>,
    ) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        if !(rect.width() >= 2.0 && rect.height() >= 2.0) {
            return;
        }
        session.set_viewport_aspect(rect.aspect_ratio());
        self.rebuild_scene_if_needed(session, tokens);
        if self.fit_pending && self.scene_key.is_some() {
            self.fit_pending = false;
            requests.push(("view.fit".into(), Value::Null));
        }

        // Interaction starts from what is on screen, cancelling any animation.
        let mut cam = self.shown;
        cam.projection = session.view.camera.projection;
        if navigation::handle(ui, &response, &mut cam, NavCaps::FULL) {
            session.view.camera = cam;
            self.shown = cam;
        }
        self.animate(ui, session);

        let painter = ui.painter_at(rect);
        match self.render(ui, rect, tokens, render_state) {
            Some(texture) => {
                painter.image(texture, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                painter.rect_filled(rect, 0.0, tokens.viewport_bottom);
                let msg = self.gpu_error.as_deref().unwrap_or("3D viewport needs the wgpu renderer");
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    msg,
                    egui::FontId::proportional(14.0),
                    tokens.text_muted,
                );
            }
        }

        let display = session.prefs.display;
        if display.view_cube {
            let cube_rect = overlay::cube_rect(rect);
            let cube_response = ui.interact(cube_rect, ui.id().with("view_cube"), Sense::click());
            self.cube_hover = cube_response.hover_pos().and_then(|p| overlay::cube_hit(&self.shown, cube_rect, p));
            if cube_response.clicked()
                && let Some(hit) = self.cube_hover
            {
                requests.push(("view.camera.preset".into(), serde_json::json!({ "preset": hit.preset_id() })));
            }
            if self.cube_hover.is_some() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            overlay::paint_view_cube(&painter, cube_rect, &self.shown, tokens, self.cube_hover);
        } else {
            self.cube_hover = None;
        }
        if display.orientation_triad {
            overlay::paint_triad(&painter, overlay::triad_anchor(rect), &self.shown, tokens);
        }
        if display.readout {
            overlay::paint_readout(
                &painter,
                Pos2::new(rect.right() - 10.0, rect.bottom() - 8.0),
                &self.shown,
                self.fps,
                session.prefs.fps_limit,
                tokens,
            );
        }
    }

    fn rebuild_scene_if_needed(&mut self, session: &mut Session, tokens: &Tokens) {
        let display = session.prefs.display;
        let key = (session.prefs.theme, session.job.revision, display);
        if self.scene_key == Some(key) {
            return;
        }
        let palette = tokens.scene_palette();
        let mut content = SceneGeometry::default();
        if session.job.sample_loaded {
            content = scene::sample_part(&palette, display.edges, display.stock);
        }
        let edge = display.edges.then_some(palette.part_edge);
        for model in &session.job.models {
            content.push_mesh(&model.mesh, palette.part, edge);
        }
        let mut world = if display.grid { scene::grid(&content.bounds, &palette) } else { SceneGeometry::default() };
        world.triangles.extend_from_slice(&content.triangles);
        world.lines.extend_from_slice(&content.lines);
        let triad_size = (content.bounds.radius() * 0.4).clamp(15.0, 200.0);
        let triad =
            if display.wcs { scene::wcs_triad(Vec3::ZERO, triad_size, &palette) } else { SceneGeometry::default() };

        let job_changed = self.scene_key.map(|(_, s, _)| s) != Some(key.1);
        session.set_scene_bounds(if content.bounds.is_empty() {
            Aabb::new(Vec3::new(-50.0, -50.0, 0.0), Vec3::new(50.0, 50.0, 20.0))
        } else {
            content.bounds
        });
        self.scene_bounds = Aabb::from_points(world.lines.iter().chain(&world.triangles).map(|v| Vec3::from(v.pos)));
        self.scene_bounds.include(content.bounds.min);
        self.scene_bounds.include(content.bounds.max);
        if job_changed {
            self.fit_pending = true;
        }
        if let Some(gpu) = &mut self.gpu {
            let r = catch_unwind(AssertUnwindSafe(|| {
                gpu.set_scene(&world);
                gpu.set_overlay(&triad);
            }));
            if r.is_err() {
                self.fail("uploading the scene to the GPU failed");
                return;
            }
        } else {
            self.pending_upload = Some((world, triad));
        }
        self.scene_key = Some(key);
    }

    fn animate(&mut self, ui: &Ui, session: &Session) {
        let goal = session.view.camera.sanitized();
        self.animating = !self.shown.approx_eq(&goal);
        if !self.animating {
            self.shown = goal;
            return;
        }
        let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1);
        self.shown = self.shown.lerp(&goal, 1.0 - (-dt * 14.0).exp());
        // Frame-rate limiter: animate at most at the preferred rate.
        let fps = session.prefs.fps_limit.max(1) as f32;
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f32(1.0 / fps));
    }

    fn render(
        &mut self,
        ui: &Ui,
        rect: Rect,
        tokens: &Tokens,
        render_state: Option<&RenderState>,
    ) -> Option<egui::TextureId> {
        if self.gpu_error.is_some() {
            return None;
        }
        let render_state = render_state?;
        if self.gpu.is_none() {
            match catch_unwind(AssertUnwindSafe(|| gpu::ViewportGpu::new(render_state))) {
                Ok(mut g) => {
                    if let Some((world, triad)) = self.pending_upload.take() {
                        g.set_scene(&world);
                        g.set_overlay(&triad);
                    }
                    self.gpu = Some(g);
                }
                Err(_) => {
                    self.fail("could not create the 3D viewport on this GPU");
                    return None;
                }
            }
        }
        let ppp = ui.ctx().pixels_per_point();
        let size_px = [(rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32];
        let cam = self.shown;
        let b = cam.basis();
        let radius = self.scene_bounds.radius() + cam.target.distance(self.scene_bounds.center());
        let params = gpu::FrameParams {
            size_px,
            view_proj: cam.view_proj(rect.aspect_ratio(), radius),
            light_dir: -b.forward + b.up * 0.35 + b.right * 0.25,
            bg_top: theme::linear(tokens.viewport_top),
            bg_bottom: theme::linear(tokens.viewport_bottom),
        };
        let gpu = self.gpu.as_mut()?;
        match catch_unwind(AssertUnwindSafe(|| gpu.render(&params))) {
            Ok(id) => {
                let now = ui.input(|i| i.time);
                if let Some(last) = self.last_render_time {
                    let dt = (now - last) as f32;
                    if dt > 0.0 && dt < 1.0 {
                        let inst = 1.0 / dt;
                        self.fps = if self.fps > 0.0 { self.fps * 0.9 + inst * 0.1 } else { inst };
                    }
                }
                self.last_render_time = Some(now);
                id
            }
            Err(_) => {
                self.fail("the GPU rejected a viewport frame");
                None
            }
        }
    }

    fn fail(&mut self, message: &str) {
        log::error!("viewport: {message}");
        self.gpu = None;
        self.gpu_error = Some(format!("3D viewport unavailable: {message}"));
    }
}
