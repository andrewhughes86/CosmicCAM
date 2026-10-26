//! The main 3D viewport pane.

mod gpu;
mod overlay;
mod sim;

use std::panic::{AssertUnwindSafe, catch_unwind};

use CosmicCAM_engine::session::Entity;
use CosmicCAM_engine::{Display, Session, Theme};
use CosmicCAM_geom::{Aabb, FaceKind, Vec3};
use CosmicCAM_render::{OrbitCamera, SceneGeometry, TriadStyle, scene};
use egui::{Color32, Pos2, Rect, Sense, Ui, pos2};
use egui_wgpu::RenderState;
use serde_json::Value;

use crate::Request;
use crate::navigation::{self, NavCaps};
use crate::theme::{self, Tokens};

/// What a click may pick while a picking tool is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accept {
    /// Geometry that gives a direction or meets others: an edge near the pointer, else a
    /// plane or cylinder face.
    Axis,
    /// A point: a circular edge (its centre) near the pointer, else a point on a face,
    /// snapped to a nearby corner.
    Point,
    /// Plain selection with no tool open: a vertex, else an edge, else any face. Clicks
    /// run `selection.set`.
    Select,
}

/// How close to an edge, in screen points, the pointer must be to pick it.
const EDGE_PICK_PX: f32 = 8.0;

/// Set by a picking tool each frame it is open.
#[derive(Clone, Debug, PartialEq)]
pub struct PickMode {
    /// What a click picks; None while the tool needs nothing more.
    pub accept: Option<Accept>,
    /// Faces already picked, drawn highlighted: (model id, face index).
    pub picked: Vec<(String, usize)>,
    /// Edges already picked, drawn highlighted: (model id, edge index).
    pub picked_edges: Vec<(String, usize)>,
    /// Points already picked, drawn as dots.
    pub picked_points: Vec<Vec3>,
    /// The WCS the picks would make: origin and X, Y, Z axes.
    pub preview: Option<(Vec3, [Vec3; 3])>,
    /// Plane normals a new plane must be independent of (one plane per axis): it may not
    /// be parallel to one of them, nor lie along the line two of them share.
    pub independent_of: Vec<Vec3>,
    /// A toolpath being edited, drawn over the scene: its start and moves.
    pub path: Option<(Vec3, std::sync::Arc<Vec<CosmicCAM_engine::toolpath::Move>>)>,
}

/// Viewport colour class of a toolpath move.
fn path_kind(k: CosmicCAM_engine::toolpath::MoveKind) -> CosmicCAM_render::PathKind {
    use CosmicCAM_engine::toolpath::MoveKind as M;
    use CosmicCAM_render::PathKind as P;
    match k {
        M::Rapid => P::Rapid,
        M::Link => P::Link,
        M::Plunge => P::Plunge,
        M::Lead => P::Lead,
        M::Cut => P::Cut,
    }
}

/// Whether `n` points along a new axis, given the normals already picked.
pub fn independent(n: Vec3, picked: &[Vec3]) -> bool {
    const MIN: f32 = 0.25;
    match picked {
        [] => true,
        [a] => n.cross(*a).length() > MIN,
        [a, b, ..] => n.cross(*a).length() > MIN && n.cross(*b).length() > MIN && a.cross(*b).dot(n).abs() > MIN,
    }
}

/// A face or point the user clicked in pick mode.
#[derive(Clone, Debug, PartialEq)]
pub struct PickHit {
    pub model: String,
    /// The face clicked, unless an edge was picked.
    pub face: Option<usize>,
    /// The edge picked.
    pub edge: Option<usize>,
    /// The point clicked (snapped to a corner when one is close).
    pub point: Vec3,
    /// The plane's outward normal when the face is a plane.
    pub normal: Option<Vec3>,
}

/// Inputs the highlight geometry was last built from.
type HighlightKey = (Vec<(String, usize)>, Option<(usize, usize, bool)>, u64, Theme);

pub struct Viewport {
    /// The open picking tool's request, if any; set by the app before each frame.
    pub pick_mode: Option<PickMode>,
    /// This frame's mode: the tool's, or plain selection built from the session.
    mode: Option<PickMode>,
    /// (model index, vertex) under the pointer in select mode.
    hover_vertex: Option<(usize, usize)>,
    picked: Option<PickHit>,
    /// (model index, face, acceptable) under the pointer.
    hover: Option<(usize, usize, bool)>,
    /// (model index, edge, acceptable) under the pointer in edge modes.
    hover_edge: Option<(usize, usize, bool)>,
    hover_point: Option<Vec3>,
    last_pointer: Option<Pos2>,
    /// What the last hover test looked for; a change forces a fresh test.
    last_accept: Option<Accept>,
    highlight_key: Option<HighlightKey>,
    /// The overlay (WCS triads) without the pick preview, and the triad size it used.
    base_overlay: SceneGeometry,
    triad_size: f32,
    /// Triads in the scene, for their axis labels: origin, X/Y/Z axes, arrow length, style.
    triads: Vec<(Vec3, [Vec3; 3], f32, TriadStyle)>,
    preview_shown: Option<(Vec3, [Vec3; 3])>,
    /// Identity of the preview path last uploaded (its moves' address and count).
    path_shown: Option<(usize, usize)>,
    gpu: Option<gpu::ViewportGpu>,
    gpu_error: Option<String>,
    /// The camera on screen; eases towards the session's camera.
    shown: OrbitCamera,
    /// Theme, job geometry revision, WCS revision and display flags the uploaded scene
    /// was built for.
    scene_key: Option<(Theme, u64, u64, Display, u64)>,
    scene_bounds: Aabb,
    fit_pending: bool,
    last_render_time: Option<f64>,
    fps: f32,
    cube_hover: Option<overlay::CubeHover>,
    /// Stock being defined, drawn as a see-through box over the scene.
    pub stock_preview: Option<CosmicCAM_geom::Aabb>,
    /// What the simulated stock was last drawn from: (cut revision, marks revision, theme,
    /// playing).
    sim_stock_key: Option<(u64, u64, Theme, bool)>,
    /// When the simulated stock was last rebuilt, seconds of app time, and how long to wait
    /// before rebuilding it while playing (longer for a bigger mesh).
    sim_stock_built: f64,
    sim_stock_interval: f64,
    /// Where the simulated tool was last drawn.
    sim_tool_key: Option<[i64; 3]>,
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
            pick_mode: None,
            mode: None,
            hover_vertex: None,
            picked: None,
            hover: None,
            hover_edge: None,
            hover_point: None,
            last_pointer: None,
            last_accept: None,
            highlight_key: None,
            base_overlay: SceneGeometry::default(),
            triad_size: 20.0,
            triads: Vec::new(),
            preview_shown: None,
            path_shown: None,
            gpu: None,
            gpu_error: None,
            shown: OrbitCamera::default(),
            scene_key: None,
            scene_bounds: Aabb::EMPTY,
            fit_pending: true,
            last_render_time: None,
            fps: 0.0,
            cube_hover: None,
            stock_preview: None,
            sim_stock_key: None,
            sim_stock_built: f64::NEG_INFINITY,
            sim_stock_interval: 0.08,
            sim_tool_key: None,
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
        self.mode = Some(self.pick_mode.clone().unwrap_or_else(|| select_mode(session)));
        self.handle_picking(ui, &response, rect, session, requests);
        self.update_highlight(session, tokens);
        self.update_sim(ui, session, tokens);

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
                let (yaw, pitch) = hit.view_degrees();
                requests.push(("view.camera.set".into(), serde_json::json!({ "yaw": yaw, "pitch": pitch })));
            }
            if self.cube_hover.is_some() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            overlay::paint_view_cube(&painter, cube_rect, &self.shown, tokens, self.cube_hover);
        } else {
            self.cube_hover = None;
        }
        if let Some(stock) = self.stock_preview {
            self.paint_stock_preview(&painter, rect, stock, tokens);
        }
        self.paint_edges(&painter, rect, session, tokens);
        if display.wcs {
            self.paint_wcs_labels(&painter, rect, session, tokens);
        }
        view_tools(ui, rect, session, tokens, requests);
        if let Some(sim) = session.sim.as_deref() {
            playback_bar(ui, rect, sim, session, tokens, requests);
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

    /// A click in pick mode, taken once.
    pub fn take_pick(&mut self) -> Option<PickHit> {
        self.picked.take()
    }

    /// The camera ray through screen point `p`: (origin, unit direction).
    fn ray(&self, rect: Rect, p: Pos2) -> Option<(Vec3, Vec3)> {
        let cam = self.shown;
        let radius = self.scene_bounds.radius() + cam.target.distance(self.scene_bounds.center());
        let inv = cam.view_proj(rect.aspect_ratio(), radius).inverse();
        let x = (p.x - rect.left()) / rect.width() * 2.0 - 1.0;
        let y = 1.0 - (p.y - rect.top()) / rect.height() * 2.0;
        let unproject = |z: f32| {
            let h = inv * CosmicCAM_geom::Vec4::new(x, y, z, 1.0);
            (h.w.abs() > 1.0e-9).then(|| h.truncate() / h.w)
        };
        let (near, far) = (unproject(0.0)?, unproject(1.0)?);
        let dir = (far - near).normalize_or_zero();
        (near.is_finite() && dir != Vec3::ZERO).then_some((near, dir))
    }

    fn handle_picking(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        rect: Rect,
        session: &Session,
        requests: &mut Vec<Request>,
    ) {
        let Some(mode) = &self.mode else {
            self.hover = None;
            self.hover_edge = None;
            self.last_pointer = None;
            return;
        };
        if mode.accept != self.last_accept {
            self.last_accept = mode.accept;
            self.last_pointer = None;
        }
        let Some(accept) = mode.accept else {
            self.hover = None;
            self.hover_edge = None;
            return;
        };
        // No hover tests while dragging the camera.
        let pointer = response.hover_pos().filter(|_| !ui.input(|i| i.pointer.any_down()));
        if pointer != self.last_pointer {
            self.last_pointer = pointer;
            self.hover = None;
            self.hover_edge = None;
            self.hover_vertex = None;
            self.hover_point = None;
            if let Some(p) = pointer
                && let (Some((o, d)), Some((o2, d2))) =
                    (self.ray(rect, p), self.ray(rect, p + egui::vec2(EDGE_PICK_PX, 0.0)))
            {
                let hit = session.job.pick(o, d);
                // The pick radius in mm: how far apart two rays EDGE_PICK_PX apart are at
                // the depth of the surface under the pointer.
                let depth = hit.map_or(self.shown.distance, |h| h.distance);
                let radius = ((o2 + d2 * depth) - (o + d * depth)).length().max(1.0e-4);
                let vertex = if accept == Accept::Select { session.job.pick_vertex(o, d, radius) } else { None };
                let edge = session.job.pick_edge(o, d, radius).filter(|(mi, ei)| {
                    // Points come from circle centres only; a straight edge has no single point.
                    accept != Accept::Point
                        || session
                            .job
                            .models
                            .get(*mi)
                            .and_then(|m| m.edges.edges.get(*ei))
                            .is_some_and(|e| e.kind.id() == "circle")
                });
                if let Some(v) = vertex {
                    self.hover_vertex = Some(v);
                } else if let Some((mi, ei)) = edge {
                    self.hover_edge = Some((mi, ei, true));
                } else if let Some(hit) = hit
                    && let Some(model) = session.job.models.get(hit.model)
                {
                    let ok = match (accept, model.faces.faces.get(hit.face).map(|f| f.kind)) {
                        (Accept::Point | Accept::Select, _) => true,
                        (Accept::Axis, Some(FaceKind::Plane { normal, .. })) => {
                            independent(normal, &mode.independent_of)
                        }
                        (Accept::Axis, Some(FaceKind::Cylinder { .. })) => true,
                        (Accept::Axis, _) => false,
                    };
                    self.hover = Some((hit.model, hit.face, ok));
                    self.hover_point = Some(self.snap(rect, pointer, model, hit.triangle).unwrap_or(hit.point));
                }
            }
        }
        if accept != Accept::Select && (self.hover.is_some() || self.hover_edge.is_some()) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if !response.clicked() {
            return;
        }
        if accept == Accept::Select {
            // A click selects what is under the pointer; a click on nothing clears the selection.
            let model_id = |mi: usize| session.job.models.get(mi).map(|m| m.id.clone());
            let target = if let Some((mi, v)) = self.hover_vertex {
                model_id(mi).map(|m| Entity::Vertex(v).reference(&m))
            } else if let Some((mi, e, _)) = self.hover_edge {
                model_id(mi).map(|m| Entity::Edge(e).reference(&m))
            } else if let Some((mi, f, _)) = self.hover {
                model_id(mi).map(|m| Entity::Face(f).reference(&m))
            } else {
                None
            };
            let params = target.map_or_else(|| serde_json::json!({}), |id| serde_json::json!({ "id": id }));
            requests.push(("selection.set".into(), params));
            return;
        }
        if let Some((mi, edge, true)) = self.hover_edge
            && let Some(model) = session.job.models.get(mi)
        {
            self.picked = Some(PickHit {
                model: model.id.clone(),
                face: None,
                edge: Some(edge),
                point: Vec3::ZERO,
                normal: None,
            });
        } else if let Some((mi, face, true)) = self.hover
            && let Some(model) = session.job.models.get(mi)
        {
            let normal = match model.faces.faces.get(face).map(|f| f.kind) {
                Some(FaceKind::Plane { normal, .. }) => Some(normal),
                _ => None,
            };
            self.picked = Some(PickHit {
                model: model.id.clone(),
                face: Some(face),
                edge: None,
                point: self.hover_point.unwrap_or_default(),
                normal,
            });
        }
    }

    /// Draws picked edges and the edge under the pointer as thick lines over the scene.
    fn paint_edges(&self, painter: &egui::Painter, rect: Rect, session: &Session, tokens: &Tokens) {
        let picked = self.mode.as_ref().map(|m| m.picked_edges.clone()).unwrap_or_default();
        let mut lines: Vec<(usize, usize, Color32, f32)> = picked
            .iter()
            .filter_map(|(model, e)| session.job.models.iter().position(|m| &m.id == model).map(|mi| (mi, *e)))
            .map(|(mi, e)| (mi, e, tokens.accent, 3.5))
            .collect();
        if let Some((mi, e, ok)) = self.hover_edge {
            lines.push((mi, e, if ok { tokens.accent.gamma_multiply(0.7) } else { tokens.error }, 3.0));
        }
        let to_screen = self.projector(rect);
        let mut dots: Vec<(Vec3, Color32)> = self
            .mode
            .as_ref()
            .map(|m| m.picked_points.iter().map(|p| (*p, tokens.accent)).collect())
            .unwrap_or_default();
        if self.mode.as_ref().is_some_and(|m| m.accept == Some(Accept::Point)) && self.hover_edge.is_none() {
            dots.extend(self.hover_point.map(|p| (p, tokens.accent)));
        }
        if let Some((mi, v)) = self.hover_vertex
            && let Some(p) = session.job.models.get(mi).and_then(|m| m.edges.vertices.get(v))
        {
            dots.push((*p, tokens.accent.gamma_multiply(0.7)));
        }
        for (p, colour) in dots.iter().filter_map(|(p, c)| to_screen(*p).map(|s| (s, *c))) {
            painter.circle(p, 4.5, colour, egui::Stroke::new(1.5, tokens.panel_bg));
        }
        for (mi, e, colour, width) in lines {
            let Some(edge) = session.job.models.get(mi).and_then(|m| m.edges.edges.get(e)) else { continue };
            let pts: Vec<Option<Pos2>> = edge.points.iter().map(|p| to_screen(*p)).collect();
            for pair in pts.windows(2) {
                if let [Some(a), Some(b)] = pair {
                    painter.line_segment([*a, *b], egui::Stroke::new(width, colour));
                }
            }
        }
    }

    /// World point to screen point with the camera on screen; None off screen or behind.
    fn projector(&self, rect: Rect) -> impl Fn(Vec3) -> Option<Pos2> + use<> {
        let cam = self.shown;
        let radius = self.scene_bounds.radius() + cam.target.distance(self.scene_bounds.center());
        let view_proj = cam.view_proj(rect.aspect_ratio(), radius);
        move |p: Vec3| {
            let clip = view_proj * p.extend(1.0);
            if !(clip.w.abs() > 1.0e-6 && clip.is_finite()) {
                return None;
            }
            let ndc = clip.truncate() / clip.w;
            if ndc.z < 0.0 || ndc.z > 1.0 || ndc.x.abs() > 1.2 || ndc.y.abs() > 1.2 {
                return None;
            }
            Some(pos2(
                rect.left() + (ndc.x + 1.0) * 0.5 * rect.width(),
                rect.top() + (1.0 - ndc.y) * 0.5 * rect.height(),
            ))
        }
    }

    /// Uploads the simulated stock when the cut has changed (at most ~12 times a second while
    /// it plays) and the tool when it has moved; clears both when there is no simulation.
    fn update_sim(&mut self, ui: &Ui, session: &Session, tokens: &Tokens) {
        let Some(gpu) = &mut self.gpu else { return };
        let Some(sim) = session.sim.as_deref() else {
            if self.sim_stock_key.take().is_some() || self.sim_tool_key.take().is_some() {
                let empty = SceneGeometry::default();
                let _ = catch_unwind(AssertUnwindSafe(|| {
                    gpu.set_sim_stock(&empty);
                    gpu.set_sim_tool(&empty);
                }));
            }
            return;
        };
        let palette = tokens.scene_palette();
        let colours = sim::SimColours {
            stock: theme::linear(mix(tokens.part, tokens.warning, 0.35)),
            cut: palette.part,
            gouge: theme::linear(tokens.error),
            flutes: theme::linear(tokens.accent),
            shank: theme::linear(tokens.text_muted),
        };
        let now = ui.input(|i| i.time);
        let key = (sim.stock_revision(), sim.marks_revision, session.prefs.theme, sim.playing);
        let due = !sim.playing || now - self.sim_stock_built > self.sim_stock_interval;
        if self.sim_stock_key != Some(key) && due {
            let g = sim::stock(sim, &colours);
            // A big mesh takes a while to build and upload: rebuild it less often while playing.
            self.sim_stock_interval = 0.08 + g.triangles.len() as f64 / 6.0e6;
            let _ = catch_unwind(AssertUnwindSafe(|| gpu.set_sim_stock(&g)));
            self.sim_stock_key = Some(key);
            self.sim_stock_built = now;
        }
        if let Some((tip, axis, tool)) = sim.pose_at(sim.time) {
            let tool_key = ((tip + axis * 1000.0) * 1000.0).as_i64vec3().to_array();
            if self.sim_tool_key != Some(tool_key) {
                let g = sim::tool(&sim.frame, tip, axis, tool, &colours);
                let _ = catch_unwind(AssertUnwindSafe(|| gpu.set_sim_tool(&g)));
                self.sim_tool_key = Some(tool_key);
            }
        }
        if sim.playing {
            ui.ctx().request_repaint();
        }
    }

    /// The stock being defined: its faces that face the viewer, filled see-through so the
    /// part shows inside it, with its edges drawn over them.
    fn paint_stock_preview(&self, painter: &egui::Painter, rect: Rect, b: CosmicCAM_geom::Aabb, tokens: &Tokens) {
        let cam = self.shown;
        let radius = self.scene_bounds.radius().max(b.radius()) + cam.target.distance(self.scene_bounds.center());
        let view_proj = cam.view_proj(rect.aspect_ratio(), radius);
        let project = |p: Vec3| -> Option<Pos2> {
            let clip = view_proj * p.extend(1.0);
            if !(clip.w > 1.0e-6 && clip.is_finite()) {
                return None;
            }
            let ndc = clip.truncate() / clip.w;
            Some(pos2(
                rect.left() + (ndc.x + 1.0) * 0.5 * rect.width(),
                rect.top() + (1.0 - ndc.y) * 0.5 * rect.height(),
            ))
        };
        let (lo, hi) = (b.min, b.max);
        let corner = |x: bool, y: bool, z: bool| {
            Vec3::new(if x { hi.x } else { lo.x }, if y { hi.y } else { lo.y }, if z { hi.z } else { lo.z })
        };
        // Each face: outward normal and its corners in order.
        let faces = [
            (
                Vec3::Z,
                [
                    corner(false, false, true),
                    corner(true, false, true),
                    corner(true, true, true),
                    corner(false, true, true),
                ],
            ),
            (
                Vec3::NEG_Z,
                [
                    corner(false, false, false),
                    corner(false, true, false),
                    corner(true, true, false),
                    corner(true, false, false),
                ],
            ),
            (
                Vec3::X,
                [
                    corner(true, false, false),
                    corner(true, true, false),
                    corner(true, true, true),
                    corner(true, false, true),
                ],
            ),
            (
                Vec3::NEG_X,
                [
                    corner(false, false, false),
                    corner(false, false, true),
                    corner(false, true, true),
                    corner(false, true, false),
                ],
            ),
            (
                Vec3::Y,
                [
                    corner(false, true, false),
                    corner(false, true, true),
                    corner(true, true, true),
                    corner(true, true, false),
                ],
            ),
            (
                Vec3::NEG_Y,
                [
                    corner(false, false, false),
                    corner(true, false, false),
                    corner(true, false, true),
                    corner(false, false, true),
                ],
            ),
        ];
        let eye = cam.eye();
        let perspective = cam.projection == CosmicCAM_render::Projection::Perspective;
        let fill = tokens.accent.gamma_multiply(0.22);
        let stroke = egui::Stroke::new(1.5, tokens.accent);
        for (normal, pts) in faces {
            let centre = (pts[0] + pts[2]) * 0.5;
            let towards = if perspective { eye - centre } else { cam.eye_dir() };
            if normal.dot(towards) <= 0.0 {
                continue;
            }
            let Some(screen) = pts.iter().map(|p| project(*p)).collect::<Option<Vec<Pos2>>>() else { continue };
            painter.add(egui::Shape::convex_polygon(screen, fill, stroke));
        }
    }

    /// The hit triangle's corner within 10 px of the pointer, if any.
    fn snap(
        &self,
        rect: Rect,
        pointer: Option<Pos2>,
        model: &CosmicCAM_engine::Model,
        triangle: usize,
    ) -> Option<Vec3> {
        let pointer = pointer?;
        let t = *model.mesh.triangles().get(triangle)?;
        let cam = self.shown;
        let radius = self.scene_bounds.radius() + cam.target.distance(self.scene_bounds.center());
        let vp = cam.view_proj(rect.aspect_ratio(), radius);
        model
            .mesh
            .corners(t)
            .into_iter()
            .filter_map(|c| {
                let clip = vp * c.extend(1.0);
                (clip.w.abs() > 1.0e-9).then(|| {
                    let ndc = clip.truncate() / clip.w;
                    let at = pos2(
                        rect.left() + (ndc.x + 1.0) * 0.5 * rect.width(),
                        rect.top() + (1.0 - ndc.y) * 0.5 * rect.height(),
                    );
                    (c, at.distance(pointer))
                })
            })
            .filter(|(_, d)| *d <= 10.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(c, _)| c)
    }

    /// Rebuilds the highlighted faces and the preview triad when they change.
    fn update_highlight(&mut self, session: &Session, tokens: &Tokens) {
        if self.gpu.is_none() {
            return; // built once the GPU exists
        }
        let picked = self.mode.as_ref().map(|m| m.picked.clone()).unwrap_or_default();
        let key: HighlightKey = (picked.clone(), self.hover, session.job.revision, session.prefs.theme);
        if self.highlight_key.as_ref() != Some(&key) {
            let mut g = SceneGeometry::default();
            let mut paint = |mi: usize, face: usize, color: [f32; 4]| {
                let Some(m) = session.job.models.get(mi) else { return };
                for t in m.faces.triangles_of(face) {
                    if let Some(&tri) = m.mesh.triangles().get(t) {
                        let [a, b, c] = m.mesh.corners(tri);
                        g.push_triangle(a, b, c, color);
                    }
                }
            };
            let accent = theme::linear(tokens.accent);
            for (model, face) in &picked {
                if let Some(mi) = session.job.models.iter().position(|m| &m.id == model) {
                    paint(mi, *face, accent);
                }
            }
            let axis_mode = self.mode.as_ref().is_some_and(|m| matches!(m.accept, Some(Accept::Axis | Accept::Select)));
            if let Some((mi, face, ok)) = self.hover
                && axis_mode
                && !picked.iter().any(|(m, f)| session.job.models.get(mi).is_some_and(|mm| &mm.id == m) && *f == face)
            {
                let base = if ok { tokens.accent } else { tokens.error };
                paint(mi, face, theme::linear(base.gamma_multiply(0.45)));
            }
            if let Some(gpu) = &mut self.gpu {
                let _ = catch_unwind(AssertUnwindSafe(|| gpu.set_highlight(&g)));
            }
            self.highlight_key = Some(key);
        }
        let preview = self.mode.as_ref().and_then(|m| m.preview);
        let path = self.mode.as_ref().and_then(|m| m.path.clone());
        let path_id = path.as_ref().map(|(_, moves)| (std::sync::Arc::as_ptr(moves) as usize, moves.len()));
        if preview != self.preview_shown || path_id != self.path_shown {
            let mut overlay = self.base_overlay.clone();
            if let Some((origin, axes)) = preview {
                overlay.push_triad(origin, axes, self.triad_size * 1.2, &tokens.scene_palette(), TriadStyle::Full);
            }
            if let Some((start, moves)) = &path {
                overlay.push_path(*start, moves.iter().map(|m| (path_kind(m.kind), m.to)), &tokens.scene_palette());
            }
            self.path_shown = path_id;
            if let Some(gpu) = &mut self.gpu {
                let _ = catch_unwind(AssertUnwindSafe(|| gpu.set_overlay(&overlay)));
            }
            self.preview_shown = preview;
        }
    }

    /// Labels the X, Y and Z arrows of every triad and names each WCS next to its origin,
    /// projected with the camera on screen.
    fn paint_wcs_labels(&self, painter: &egui::Painter, rect: Rect, session: &Session, tokens: &Tokens) {
        let to_screen = self.projector(rect);
        let mut triads = self.triads.clone();
        if let Some((origin, axes)) = self.preview_shown {
            triads.push((origin, axes, scene::triad_length(self.triad_size * 1.2, TriadStyle::Full), TriadStyle::Full));
        }
        for (origin, axes, length, style) in triads {
            let Some(o) = to_screen(origin) else { continue };
            for ((axis, name), colour) in
                axes.iter().zip(["X", "Y", "Z"]).zip([tokens.axis_x, tokens.axis_y, tokens.axis_z])
            {
                let Some(tip) = to_screen(origin + *axis * length) else { continue };
                let d = tip - o;
                // An axis pointing at the viewer has no room for a label.
                if d.length() < 6.0 {
                    continue;
                }
                let colour = mix(tokens.text_muted, colour, style.colour_share());
                let size = match style {
                    TriadStyle::Full => 13.0,
                    TriadStyle::Dim => 11.0,
                    TriadStyle::Faint => 10.0,
                };
                let font = egui::FontId::new(size, egui::FontFamily::Monospace);
                painter.text(tip + d.normalized() * 9.0, egui::Align2::CENTER_CENTER, name, font, colour);
            }
        }
        let active = session.job.active_wcs.as_deref();
        for w in &session.job.wcs {
            let Some(at) = to_screen(w.origin) else { continue };
            let is_active = active == Some(w.id.as_str());
            let color = if is_active { tokens.text } else { tokens.text_muted };
            painter.text(
                at + egui::vec2(8.0, 8.0),
                egui::Align2::LEFT_TOP,
                &w.name,
                egui::FontId::proportional(if is_active { 12.0 } else { 11.0 }),
                color,
            );
        }
    }

    fn rebuild_scene_if_needed(&mut self, session: &mut Session, tokens: &Tokens) {
        let display = session.prefs.display;
        // The grid follows the units, so a units change counts as a setup change.
        let units = u64::from(session.prefs.units == CosmicCAM_engine::units::DisplayUnits::Inches);
        // Only the selected toolpath and those switched on are drawn.
        let selected = session.view.selection.as_deref();
        let shown: Vec<&CosmicCAM_engine::toolpath::Toolpath> =
            session.job.toolpaths.iter().filter(|t| t.visible || selected == Some(t.id.as_str())).collect();
        let shown_key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for t in &shown {
                t.id.hash(&mut h);
            }
            h.finish()
        };
        // While simulating, the cut stock stands in for the part, its box and the toolpaths.
        let simulating = session.sim.is_some();
        let shown_key = shown_key ^ u64::from(simulating);
        let key =
            (session.prefs.theme, session.job.revision, session.job.setup_revision ^ (units << 63), display, shown_key);
        if self.scene_key == Some(key) {
            return;
        }
        let palette = tokens.scene_palette();
        let mut content = SceneGeometry::default();
        for tp in shown.into_iter().filter(|_| !simulating) {
            content.push_path(tp.start, tp.moves.iter().map(|m| (path_kind(m.kind), m.to)), &palette);
            if tp.multi_axis() {
                push_tool_axes(&mut content, tp, palette.path_lead);
            }
        }
        if let Some(stock) = session.job.stock {
            if display.stock && !simulating {
                content.push_box_edges(&stock, palette.stock_edge);
            }
            content.bounds.include(stock.min);
            content.bounds.include(stock.max);
        }
        let edge = display.edges.then_some(palette.part_edge);
        for model in session.job.models.iter().filter(|m| m.visible && !simulating) {
            content.push_mesh(&model.mesh, palette.part, edge);
        }
        let mut world =
            if display.grid { scene::grid(&content.bounds, &palette, units == 1) } else { SceneGeometry::default() };
        world.triangles.extend_from_slice(&content.triangles);
        world.lines.extend_from_slice(&content.lines);
        let triad_size = (content.bounds.radius() * 0.4).clamp(15.0, 200.0);
        // The world (model) origin always has a triad: in full colour until a WCS exists,
        // then faint. Each WCS gets one too, the active one in full colour.
        let mut triad = SceneGeometry::default();
        self.triads.clear();
        if display.wcs {
            let world = if session.job.wcs.is_empty() { TriadStyle::Full } else { TriadStyle::Faint };
            let mut frames = vec![(Vec3::ZERO, [Vec3::X, Vec3::Y, Vec3::Z], world)];
            let active = session.job.active_wcs.as_deref();
            for w in &session.job.wcs {
                let style = if active == Some(w.id.as_str()) { TriadStyle::Full } else { TriadStyle::Dim };
                frames.push((w.origin, [w.x_axis, w.y_axis(), w.z_axis], style));
            }
            for (origin, axes, style) in frames {
                triad.push_triad(origin, axes, triad_size, &palette, style);
                self.triads.push((origin, axes, scene::triad_length(triad_size, style), style));
            }
        }

        // Re-frame the view when the geometry changes, not when a WCS moves.
        let job_changed = self.scene_key.map(|(_, revision, _, _, _)| revision) != Some(key.1);
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
        self.base_overlay = triad.clone();
        self.triad_size = triad_size;
        self.preview_shown = None;
        self.path_shown = None;
        self.highlight_key = None;
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
        if let Some(e) = gpu.error() {
            self.fail(&format!("the GPU reported an error: {e}"));
            return None;
        }
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

/// Plain selection: what the session has selected, drawn as picked.
fn select_mode(session: &Session) -> PickMode {
    let mut mode = PickMode {
        accept: Some(Accept::Select),
        picked: Vec::new(),
        picked_edges: Vec::new(),
        picked_points: Vec::new(),
        preview: None,
        independent_of: Vec::new(),
        path: None,
    };
    if let Some((m, entity)) = session.view.selection.as_deref().and_then(|id| session.job.entity(id)) {
        match entity {
            Entity::Face(f) => mode.picked.push((m.id.clone(), f)),
            Entity::Edge(e) => mode.picked_edges.push((m.id.clone(), e)),
            Entity::Vertex(v) => mode.picked_points.extend(m.edges.vertices.get(v).copied()),
        }
    }
    mode
}

/// `share` of `colour` over `base`, as the scene greys inactive triads.
fn mix(base: Color32, colour: Color32, share: f32) -> Color32 {
    let t = share.clamp(0.0, 1.0);
    let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgb(m(base.r(), colour.r()), m(base.g(), colour.g()), m(base.b(), colour.b()))
}

/// Size of one floating view tool button.
const TOOL_BUTTON: f32 = 38.0;
/// Gap between the tool row and the viewport's bottom edge.
const TOOL_MARGIN: f32 = 10.0;
/// Space between two tool buttons.
const TOOL_GAP: f32 = 4.0;

/// The row of view tools floating at the bottom centre of the viewport.
fn view_tools(ui: &Ui, viewport: Rect, session: &Session, tokens: &Tokens, requests: &mut Vec<Request>) {
    let tools = CosmicCAM_engine::cmd::VIEWPORT_TOOLS;
    let width = tools.len() as f32 * (TOOL_BUTTON + TOOL_GAP) - TOOL_GAP;
    let left = viewport.center().x - width / 2.0;
    let centre_y = viewport.bottom() - TOOL_MARGIN - TOOL_BUTTON / 2.0;
    for (n, (id, menu)) in tools.iter().enumerate() {
        let Some(spec) = CosmicCAM_engine::cmd::find(id) else {
            continue;
        };
        let button = Rect::from_center_size(
            pos2(left + TOOL_BUTTON / 2.0 + n as f32 * (TOOL_BUTTON + TOOL_GAP), centre_y),
            egui::vec2(TOOL_BUTTON, TOOL_BUTTON),
        );
        if button.left() < viewport.left() + 4.0 || button.right() > viewport.right() - 4.0 {
            continue;
        }
        let enabled = session.is_enabled(spec);
        let response = ui.interact(button, ui.id().with(("view-tool", *id)), Sense::click());
        let checked = crate::ribbon::is_checked(id, session);
        let fill = if response.is_pointer_button_down_on() {
            tokens.accent.gamma_multiply(0.3)
        } else if checked {
            tokens.accent.gamma_multiply(0.22)
        } else if response.hovered() {
            tokens.hover_bg
        } else {
            tokens.panel_bg.gamma_multiply(0.85)
        };
        let stroke = egui::Stroke::new(1.0, if response.hovered() || checked { tokens.accent } else { tokens.border });
        ui.painter().rect(button, 4.0, fill, stroke, egui::StrokeKind::Inside);
        crate::ribbon::paint_icon(ui, button.shrink(6.0), spec, tokens, enabled);
        let response = crate::ribbon::with_tooltip(response, spec, enabled);
        if response.clicked() && enabled {
            requests.push(((*id).to_owned(), Value::Null));
        }
        if !menu.is_empty() {
            crate::ui_kit::Menu::below(&response).context(tokens, |ui| {
                for item in menu.iter().filter_map(|m| CosmicCAM_engine::cmd::find(m)) {
                    if ui.button(item.label).clicked() {
                        requests.push((item.id.to_owned(), Value::Null));
                    }
                }
            });
        }
    }
}

/// Minutes and seconds.
fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// The simulation's controls, floating above the view tools: play or pause, step, a scrubber,
/// the speed, where it is and what it has found, and close.
fn playback_bar(
    ui: &mut Ui,
    viewport: Rect,
    sim: &CosmicCAM_engine::sim::Sim,
    session: &Session,
    tokens: &Tokens,
    requests: &mut Vec<Request>,
) {
    let size = egui::vec2(640.0_f32.min(viewport.width() - 16.0), 44.0);
    let at = pos2(viewport.center().x - size.x / 2.0, viewport.bottom() - TOOL_MARGIN - TOOL_BUTTON - 10.0 - size.y);
    let bar = Rect::from_min_size(at, size);
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::left_to_right(egui::Align::Center)));
    crate::ui_kit::Container::new(crate::ui_kit::Surface::Overlay).frame(tokens).show(&mut child, |ui| {
        ui.set_min_width(size.x - 16.0);
        ui.spacing_mut().item_spacing.x = 8.0;
        let icon_button = |ui: &mut Ui, icon: &str, tip: &str| -> bool {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), Sense::click());
            if response.hovered() {
                ui.painter().rect_filled(rect, 4.0, tokens.hover_bg);
            }
            crate::ribbon::paint_named_icon(ui, rect.shrink(3.0), icon, tokens, true);
            response.on_hover_text(tip).clicked()
        };
        let (icon, tip, command) = if sim.playing {
            ("sim_pause", "Pause (Space)", "sim.pause")
        } else {
            ("sim_play", "Play (Space)", "sim.play")
        };
        if icon_button(ui, icon, tip) {
            requests.push((command.into(), Value::Null));
        }
        if icon_button(ui, "sim_step", "Next move") {
            requests.push(("sim.step".into(), Value::Null));
        }
        let mut time = sim.time;
        let slider = egui::Slider::new(&mut time, 0.0..=sim.duration.max(1.0e-6)).show_value(false);
        if ui.add_sized(egui::vec2(220.0, 20.0), slider).changed() {
            requests.push(("sim.seek".into(), serde_json::json!({ "time": time })));
        }
        ui.label(mono(format!("{} / {}", clock(sim.time), clock(sim.duration)), tokens));
        if ui.button(format!("{}×", trim_speed(sim.speed))).on_hover_text("Playback speed: click for faster").clicked()
        {
            requests.push(("sim.speed".into(), Value::Null));
        }
        let current = sim.current_toolpath().map_or(String::new(), |(_, name)| name.clone());
        ui.label(egui::RichText::new(current).color(tokens.text_muted));
        if !sim.events.is_empty() {
            let n = sim.events.len();
            ui.label(
                egui::RichText::new(format!("{n} collision{}", if n == 1 { "" } else { "s" })).color(tokens.warning),
            )
            .on_hover_text("Rapids through material or cuts deeper than the flutes; run Collisions for the list");
        }
        if let Some(n) = sim.gouges.as_ref().map(Vec::len).or(sim.voxel_gouges.as_ref().map(Vec::len)) {
            let colour = if n == 0 { tokens.success } else { tokens.error };
            let text = if n == 0 { "No gouges".to_owned() } else { format!("{n} gouged cells") };
            ui.label(egui::RichText::new(text).color(colour));
        }
        let _ = session;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("×").on_hover_text("Close the simulation").clicked() {
                requests.push(("sim.stop".into(), Value::Null));
            }
        });
    });
}

/// A speed without needless decimals: 0.5, 2, 25.
fn trim_speed(v: f64) -> String {
    if (v - v.round()).abs() < 1.0e-9 { format!("{v:.0}") } else { format!("{v:.1}") }
}

/// Monospaced text, for clocks that should not jitter as digits change.
fn mono(text: String, tokens: &Tokens) -> egui::RichText {
    egui::RichText::new(text).monospace().color(tokens.text)
}

/// Short lines up the tool axis along a multi-axis toolpath's cuts, spaced so about 800 show,
/// so the tilt reads without hiding the path.
fn push_tool_axes(content: &mut SceneGeometry, tp: &CosmicCAM_engine::toolpath::Toolpath, colour: scene::Rgba) {
    const LENGTH: f32 = 8.0;
    const MOST: usize = 1_000;
    let (cut, _, _) = tp.stats(10_000.0);
    let spacing = (cut as f32 / 800.0).max(4.0);
    let mut travelled = spacing;
    let mut at = tp.start;
    let mut drawn = 0;
    for (i, m) in tp.moves.iter().enumerate() {
        travelled += at.distance(m.to);
        at = m.to;
        if m.kind != CosmicCAM_engine::toolpath::MoveKind::Cut || travelled < spacing || drawn >= MOST {
            continue;
        }
        travelled = 0.0;
        drawn += 1;
        content.push_line(m.to, m.to + tp.axis_at(i) * LENGTH, colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_plane_must_give_a_new_axis() {
        assert!(independent(Vec3::Z, &[]));
        assert!(!independent(Vec3::NEG_Z, &[Vec3::Z]), "parallel");
        assert!(independent(Vec3::X, &[Vec3::Z]));
        assert!(independent(Vec3::NEG_Y, &[Vec3::Z, Vec3::NEG_X]));
        // A plane whose normal lies in the span of the first two cannot fix the corner.
        assert!(!independent(Vec3::new(1.0, 0.0, 1.0).normalize(), &[Vec3::Z, Vec3::X]));
    }
}
