//! The trim panel: docked at the right of the window while toolpaths are trimmed. Draw a box,
//! a polygon or a circle on the toolpath's work plane in the viewport, or pick faces and closed
//! edges, then choose the side to keep and how the cutter meets the boundary. The trimmed
//! toolpaths preview live in place of the originals; OK runs `toolpath.trim`.

use std::sync::Arc;

use CosmicDMS_engine::Session;
use CosmicDMS_engine::session::Entity;
use CosmicDMS_engine::toolpath::{Move, MoveKind};
use CosmicDMS_geom::Vec3;
use egui::{RichText, Ui};
use serde_json::{Map, Value, json};

use crate::Request;
use crate::theme::Tokens;
use crate::toolpath_panel::ok_cancel;
use crate::ui_kit::{Panel, Surface};
use crate::viewport::{Accept, PickHit, PickMode};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Box,
    Polygon,
    Circle,
    Faces,
    Stock,
    Model,
    Heights,
}

const SHAPES: &[(Shape, &str, &str)] = &[
    (Shape::Box, "Box", "Two opposite corners"),
    (Shape::Polygon, "Polygon", "Any outline, corner by corner"),
    (Shape::Circle, "Circle", "A centre and a point on the circle"),
    (Shape::Faces, "Faces", "The outline of model faces or closed edges"),
    (Shape::Stock, "Stock", "The stock's outline"),
    (Shape::Model, "Model", "The model's outline"),
    (Shape::Heights, "Heights", "Only between heights, no outline"),
];

impl Shape {
    fn id(self) -> &'static str {
        match self {
            Self::Box => "box",
            Self::Polygon => "polygon",
            Self::Circle => "circle",
            Self::Faces => "selection",
            Self::Stock => "stock",
            Self::Model => "model",
            Self::Heights => "none",
        }
    }

    fn from_id(id: &str) -> Self {
        match id {
            "polygon" => Self::Polygon,
            "circle" => Self::Circle,
            "selection" => Self::Faces,
            "stock" => Self::Stock,
            "model" => Self::Model,
            "none" => Self::Heights,
            _ => Self::Box,
        }
    }

    /// Whether it is drawn by clicking points on the work plane.
    fn drawn(self) -> bool {
        matches!(self, Self::Box | Self::Polygon | Self::Circle)
    }
}

/// The trimmed toolpaths (each one's start and moves) and a summary, or why they cannot be made.
type Preview = Result<(Vec<(Vec3, Arc<Vec<Move>>)>, String), String>;

/// Runs `toolpath.trim` on `trial` (a copy of the session) and takes what it makes.
fn compute(mut trial: Session, p: &Value) -> Preview {
    let result = trial.execute("toolpath.trim", p).map_err(|e| e.to_string())?;
    let made: Vec<&Value> =
        result["trimmed"].as_array().into_iter().chain(result["split"].as_array()).flatten().collect();
    let mut paths = Vec::new();
    let mut moves = 0;
    let mut cut = 0.0;
    for v in &made {
        let Some(tp) = trial.job.toolpaths.iter().find(|t| Some(t.id.as_str()) == v["id"].as_str()) else { continue };
        moves += tp.moves.len();
        cut += v["cut_length"].as_f64().unwrap_or(0.0);
        paths.push((tp.start, Arc::clone(&tp.moves)));
    }
    let units = trial.prefs.units;
    let what = if made.len() == 1 { "1 toolpath".to_owned() } else { format!("{} toolpaths", made.len()) };
    Ok((paths, format!("{what} · {moves} moves · cutting {}", units.fmt_len(cut))))
}

pub struct TrimPanel {
    targets: Vec<String>,
    /// The trim being changed (1 is the first), or None for a new one.
    index: Option<usize>,
    shape: Shape,
    /// Points clicked on the work plane (world mm).
    points: Vec<Vec3>,
    /// The polygon is finished: clicks no longer add corners.
    closed: bool,
    geometry: Vec<String>,
    keep: &'static str,
    split: bool,
    containment: &'static str,
    link: &'static str,
    /// Offset, top and bottom as typed, in display units; empty for none.
    offset: String,
    top: String,
    bottom: String,
    /// The work plane: a point on it, its normal (the tool axis) and its X and Y directions.
    plane: (Vec3, Vec3, Vec3, Vec3),
    preview: Preview,
    pending: Option<std::sync::mpsc::Receiver<Preview>>,
    dirty: bool,
    error: Option<String>,
    /// Commands to run now without closing the panel (removing a trim, a camera view).
    requests: Vec<Request>,
}

impl TrimPanel {
    /// A panel trimming `targets` (toolpath ids); one that holds trims lists them for editing.
    pub fn new(session: &Session, targets: Vec<String>) -> Self {
        let plane = work_plane(session, targets.first().map(String::as_str));
        let mut panel = Self {
            targets,
            index: None,
            shape: Shape::Box,
            points: Vec::new(),
            closed: false,
            geometry: Vec::new(),
            keep: "inside",
            split: false,
            containment: "center",
            link: "auto",
            offset: String::new(),
            top: String::new(),
            bottom: String::new(),
            plane,
            preview: Err(String::new()),
            pending: None,
            dirty: true,
            error: None,
            requests: Vec::new(),
        };
        // A face or edge selected in the viewport starts a Faces boundary.
        if let Some(sel) = session.view.selection.as_deref()
            && session.job.entity(sel).is_some_and(|(_, e)| !matches!(e, Entity::Vertex(_)))
        {
            panel.shape = Shape::Faces;
            panel.geometry.push(sel.to_owned());
        }
        panel
    }

    pub fn fail(&mut self, message: String) {
        self.error = Some(message);
    }

    /// Commands the panel asks for that do not close it.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// Loads trim `index` (1 is the first) of the first toolpath into the panel to change it.
    fn edit(&mut self, settings: &Value, index: usize, session: &Session) {
        let units = session.prefs.units;
        let s = |k: &str| settings.get(k).and_then(Value::as_str);
        let num = |k: &str| settings.get(k).and_then(Value::as_f64);
        let points = |k: &str| -> Vec<Vec3> {
            settings
                .get(k)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    let c: Vec<f32> = p.as_array()?.iter().filter_map(Value::as_f64).map(|v| v as f32).collect();
                    (c.len() == 3).then(|| Vec3::new(c[0], c[1], c[2]))
                })
                .collect()
        };
        self.index = Some(index);
        self.shape = Shape::from_id(s("boundary").unwrap_or(if settings.get("points").is_some() {
            "polygon"
        } else if settings.get("center").is_some() {
            "circle"
        } else if settings.get("geometry").is_some() {
            "selection"
        } else if settings.get("corners").is_some() {
            "box"
        } else {
            "none"
        }));
        self.points = match self.shape {
            Shape::Polygon => points("points"),
            Shape::Circle => {
                let c = settings.get("center").and_then(Value::as_array).and_then(|a| {
                    let v: Vec<f32> = a.iter().filter_map(Value::as_f64).map(|v| v as f32).collect();
                    (v.len() == 3).then(|| Vec3::new(v[0], v[1], v[2]))
                });
                c.map(|c| vec![c, c + self.plane.2 * num("radius").unwrap_or(1.0) as f32]).unwrap_or_default()
            }
            _ => points("corners"),
        };
        self.closed = self.shape == Shape::Polygon;
        self.geometry = settings
            .get("geometry")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|g| g.as_str().map(str::to_owned))
            .collect();
        let pick = |v: Option<&str>, of: &[&'static str], default: &'static str| -> &'static str {
            v.and_then(|v| of.iter().copied().find(|o| *o == v)).unwrap_or(default)
        };
        self.keep = pick(s("keep"), &["inside", "outside"], "inside");
        self.containment = pick(s("containment"), &["center", "tool_within", "tool_over"], "center");
        self.link = pick(s("link"), &["auto", "retract", "clearance", "feed"], "auto");
        self.split = false;
        let text = |v: Option<f64>| v.map(|v| units.plain(v)).unwrap_or_default();
        self.offset = text(num("offset"));
        self.top = text(num("top"));
        self.bottom = text(num("bottom"));
        self.dirty = true;
    }

    /// The boundary is complete enough to trim with.
    fn ready(&self) -> bool {
        match self.shape {
            Shape::Box | Shape::Circle => self.points.len() >= 2,
            Shape::Polygon => self.points.len() >= 3,
            Shape::Faces => !self.geometry.is_empty(),
            Shape::Stock | Shape::Model => true,
            Shape::Heights => !self.top.trim().is_empty() || !self.bottom.trim().is_empty(),
        }
    }

    fn params(&self, session: &Session) -> Value {
        let units = session.prefs.units;
        let mut p = Map::new();
        p.insert("ids".into(), json!(self.targets));
        p.insert("boundary".into(), json!(self.shape.id()));
        let arr = |v: &Vec3| json!([v.x, v.y, v.z]);
        match self.shape {
            Shape::Box => {
                p.insert("corners".into(), json!(self.points.iter().take(2).map(arr).collect::<Vec<_>>()));
            }
            Shape::Polygon => {
                p.insert("points".into(), json!(self.points.iter().map(arr).collect::<Vec<_>>()));
            }
            Shape::Circle => {
                if let [c, rim, ..] = self.points.as_slice() {
                    p.insert("center".into(), arr(c));
                    p.insert("radius".into(), json!(f64::from(flat(self.plane.1, *rim - *c).length()).max(0.001)));
                }
            }
            Shape::Faces => {
                p.insert("geometry".into(), json!(self.geometry));
            }
            _ => {}
        }
        p.insert("keep".into(), json!(self.keep));
        p.insert("containment".into(), json!(self.containment));
        p.insert("link".into(), json!(self.link));
        if self.split {
            p.insert("split".into(), json!(true));
        }
        for (key, text) in [("offset", &self.offset), ("top", &self.top), ("bottom", &self.bottom)] {
            if let Ok(v) = text.trim().parse::<f64>()
                && v.is_finite()
            {
                p.insert(key.into(), json!(units.to_mm(v)));
            }
        }
        if let Some(i) = self.index {
            p.insert("index".into(), json!(i));
        }
        Value::Object(p)
    }

    /// Works the trim out again on a copy of the session, in the background where threads exist.
    fn refresh(&mut self, session: &Session) {
        self.dirty = false;
        self.error = None;
        if !self.ready() {
            self.pending = None;
            self.preview = Err(String::new());
            return;
        }
        let p = self.params(session);
        let trial = session.clone();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new().name("trim-preview".into()).spawn(move || {
                let _ = tx.send(compute(trial, &p));
            });
            match spawned {
                Ok(_) => self.pending = Some(rx),
                Err(e) => self.preview = Err(format!("could not start the preview: {e}")),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.preview = compute(trial, &p);
        }
    }

    /// Takes a finished background preview; true while one is still being worked out.
    fn poll(&mut self) -> bool {
        let Some(rx) = &self.pending else { return false };
        match rx.try_recv() {
            Ok(preview) => {
                self.preview = preview;
                self.pending = None;
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                self.preview = Err("the preview stopped unexpectedly".into());
                false
            }
        }
    }

    /// What the viewport picks and draws while the panel is open; `hover` is the point on
    /// the work plane under the pointer.
    pub fn mode(&self, session: &Session, hover: Option<Vec3>) -> PickMode {
        let (origin, normal, _, _) = self.plane;
        let drawing = self.shape.drawn() && !self.complete();
        let mut mode = PickMode {
            accept: if drawing {
                Some(Accept::Plane)
            } else if self.shape == Shape::Faces {
                Some(Accept::Axis)
            } else {
                None
            },
            plane: Some((origin, normal)),
            picked_points: self.points.clone(),
            lines: self.outline(hover.filter(|_| drawing)),
            ..PickMode::default()
        };
        if let Ok((paths, _)) = &self.preview {
            mode.paths = paths.clone();
            mode.hide = self.targets.clone();
        }
        if let Some(tp) = self.targets.first().and_then(|id| session.job.toolpaths.iter().find(|t| &t.id == id)) {
            mode.wcs.extend(tp.params.get("orientation").and_then(Value::as_str).map(str::to_owned).or(tp.wcs.clone()));
        }
        for r in &self.geometry {
            match session.job.entity(r) {
                Some((m, Entity::Face(f))) => mode.picked.push((m.id.clone(), f)),
                Some((m, Entity::Edge(e))) => mode.picked_edges.push((m.id.clone(), e)),
                _ => {}
            }
        }
        mode
    }

    /// No more points are needed for the drawn shape.
    fn complete(&self) -> bool {
        match self.shape {
            Shape::Box | Shape::Circle => self.points.len() >= 2,
            Shape::Polygon => self.closed,
            _ => true,
        }
    }

    /// The boundary as lines on the work plane, with the pointer's next point while drawing.
    fn outline(&self, hover: Option<Vec3>) -> Vec<Vec<Vec3>> {
        let (_, n, x, y) = self.plane;
        let mut pts = self.points.clone();
        let rubber = !self.complete();
        if rubber {
            pts.extend(hover);
        }
        match self.shape {
            Shape::Box => match pts.as_slice() {
                [a, b, ..] => {
                    let d = *b - *a;
                    let (dx, dy) = (x * d.dot(x), y * d.dot(y));
                    vec![vec![*a, *a + dx, *a + dx + dy, *a + dy, *a]]
                }
                _ => Vec::new(),
            },
            Shape::Circle => match pts.as_slice() {
                [c, rim, ..] => {
                    let r = flat(n, *rim - *c).length();
                    let ring = (0..=96)
                        .map(|i| {
                            let a = std::f32::consts::TAU * i as f32 / 96.0;
                            *c + (x * a.cos() + y * a.sin()) * r
                        })
                        .collect();
                    vec![ring, vec![*c, *rim]]
                }
                _ => Vec::new(),
            },
            Shape::Polygon if pts.len() >= 2 => {
                if self.closed || (rubber && pts.len() >= 3) {
                    pts.extend(pts.first().copied());
                }
                vec![pts]
            }
            _ => Vec::new(),
        }
    }

    /// A click in the viewport: a point on the work plane, or a face or edge.
    pub fn picked(&mut self, hit: PickHit, session: &Session) {
        if self.shape == Shape::Faces {
            let reference = match (hit.edge, hit.face) {
                (Some(e), _) => Entity::Edge(e).reference(&hit.model),
                (None, Some(f)) => Entity::Face(f).reference(&hit.model),
                _ => return,
            };
            if let Some(i) = self.geometry.iter().position(|g| *g == reference) {
                self.geometry.remove(i);
            } else {
                self.geometry.push(reference);
            }
            self.dirty = true;
            return;
        }
        if !self.shape.drawn() || self.complete() || !hit.model.is_empty() {
            return;
        }
        // Clicking near the first corner closes the polygon.
        if self.shape == Shape::Polygon
            && self.points.len() >= 3
            && self.points.first().is_some_and(|p| p.distance(hit.point) < self.snap_distance(session))
        {
            self.closed = true;
        } else {
            self.points.push(hit.point);
        }
        self.dirty = true;
    }

    /// How near the first corner a click closes the polygon: 2% of the job's size.
    fn snap_distance(&self, session: &Session) -> f32 {
        let b = session.job.part_bounds();
        if b.is_empty() { 1.0 } else { ((b.max - b.min).length() * 0.02).max(0.5) }
    }

    /// Draws the panel. Returns `toolpath.trim` to run on OK, and whether the panel stays open.
    pub fn show(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens) -> (Option<Request>, bool) {
        self.targets.retain(|id| session.job.toolpaths.iter().any(|t| &t.id == id));
        if self.targets.is_empty() {
            return (None, false);
        }
        // Enter finishes a polygon; Backspace takes back the last corner.
        if self.shape.drawn() && !ui.ctx().egui_wants_keyboard_input() {
            let (enter, back) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Backspace)));
            if enter && self.shape == Shape::Polygon && self.points.len() >= 3 && !self.closed {
                self.closed = true;
                self.dirty = true;
            }
            if back && self.points.pop().is_some() {
                self.closed = false;
                self.dirty = true;
            }
        }
        if self.dirty {
            self.refresh(session);
        }
        let working = self.poll();
        if working || (self.shape.drawn() && !self.complete()) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        }
        let mut result = (None, true);
        let names: Vec<String> = self
            .targets
            .iter()
            .filter_map(|id| session.job.toolpaths.iter().find(|t| &t.id == id))
            .map(|t| format!("[T{}] {}", t.tool, t.name))
            .collect();
        Panel::right("trim-panel", Surface::SidePanel, 360.0).show(ui, tokens, |ui| {
            let title = if self.index.is_some() { "Edit trim" } else { "Trim toolpaths" };
            ui.label(RichText::new(title).heading());
            ui.label(RichText::new(names.join(", ")).color(tokens.text_muted));
            ui.add_space(4.0);
            ui.separator();
            let footer = 120.0;
            let body = (ui.available_height() - footer).max(120.0);
            egui::ScrollArea::vertical()
                .max_height(body)
                .min_scrolled_height(body)
                .auto_shrink([false, false])
                .show(ui, |ui| self.settings(ui, session, tokens));
            ui.separator();
            ui.vertical_centered(|ui| match (&self.error, &self.preview) {
                _ if working => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label("Trimming…");
                    });
                }
                (Some(e), _) => {
                    ui.label(RichText::new(e).color(tokens.error));
                }
                (None, Ok((_, summary))) => {
                    ui.label(summary);
                }
                (None, Err(e)) if !e.is_empty() => {
                    ui.label(RichText::new(e).color(tokens.warning));
                }
                _ => {
                    ui.label(RichText::new(self.hint()).color(tokens.text_muted));
                }
            });
            ui.add_space(8.0);
            let (ok, cancel) = ok_cancel(ui, self.preview.is_ok() && !working);
            if ok {
                result.0 = Some(("toolpath.trim".to_owned(), self.params(session)));
            }
            if cancel {
                result.1 = false;
            }
        });
        result
    }

    /// What to do next, while the boundary is not complete.
    fn hint(&self) -> String {
        match self.shape {
            Shape::Box => format!("Click two opposite corners in the viewport ({} of 2).", self.points.len()),
            Shape::Polygon => {
                "Click the corners in the viewport; click the first corner or press Enter to finish.".into()
            }
            Shape::Circle if self.points.is_empty() => "Click the circle's centre in the viewport.".into(),
            Shape::Circle => "Click a point on the circle.".into(),
            Shape::Faces => "Click faces or closed edges of the model.".into(),
            Shape::Heights => "Give a top height, a bottom height or both.".into(),
            Shape::Stock | Shape::Model => String::new(),
        }
    }

    fn settings(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens) {
        let units = session.prefs.units;
        let mut changed = false;
        ui.label(RichText::new("Boundary").strong());
        ui.horizontal_wrapped(|ui| {
            for (shape, label, help) in SHAPES {
                if ui.selectable_label(self.shape == *shape, *label).on_hover_text(*help).clicked()
                    && self.shape != *shape
                {
                    self.shape = *shape;
                    self.points.clear();
                    self.closed = false;
                    changed = true;
                }
            }
        });
        if self.shape.drawn() {
            ui.horizontal(|ui| {
                if ui.button("Start over").on_hover_text("Clear the points and draw again").clicked() {
                    self.points.clear();
                    self.closed = false;
                    changed = true;
                }
                if self.shape == Shape::Polygon
                    && !self.closed
                    && ui.add_enabled(self.points.len() >= 3, egui::Button::new("Finish")).clicked()
                {
                    self.closed = true;
                    changed = true;
                }
                if ui.button("Top view").on_hover_text("Look straight down the tool axis to draw").clicked() {
                    self.requests.push(("view.camera.preset".into(), json!({ "preset": "top" })));
                }
            });
            ui.label(
                RichText::new(
                    "Drawn on the toolpath's work plane, seen along the tool axis. Backspace removes the last point.",
                )
                .small()
                .color(tokens.text_muted),
            );
        }
        ui.add_space(8.0);
        egui::Grid::new("trim-settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Keep").on_hover_text("The side of the boundary that stays");
            ui.horizontal(|ui| {
                for (keep, split, label, help) in [
                    ("inside", false, "Inside", "Keep what lies inside the boundary"),
                    ("outside", false, "Outside", "Keep what lies outside the boundary"),
                    ("inside", true, "Both", "Keep the inside, and add a copy of the toolpath that keeps the outside"),
                ] {
                    if ui
                        .selectable_label(self.keep == keep && self.split == split, label)
                        .on_hover_text(help)
                        .clicked()
                    {
                        self.keep = keep;
                        self.split = split;
                        changed = true;
                    }
                }
            });
            ui.end_row();
            if self.shape != Shape::Heights {
                ui.label("Tool").on_hover_text("What the boundary holds");
                ui.horizontal(|ui| {
                    for (id, label, help) in [
                        ("center", "Centre", "The tool's centre stops at the boundary"),
                        ("tool_within", "Within", "The whole cutter stays on the kept side"),
                        ("tool_over", "Over", "The cutter may reach over the boundary by its radius"),
                    ] {
                        if ui.selectable_label(self.containment == id, label).on_hover_text(help).clicked() {
                            self.containment = id;
                            changed = true;
                        }
                    }
                });
                ui.end_row();
                ui.label("Offset").on_hover_text("Grow the boundary by this much (negative shrinks it)");
                ui.horizontal(|ui| {
                    changed |= ui
                        .add(egui::TextEdit::singleline(&mut self.offset).desired_width(72.0).hint_text("0"))
                        .changed();
                    ui.label(units.length());
                });
                ui.end_row();
            }
            ui.label("Top").on_hover_text("Keep only moves at or below this height (WCS Z); empty for no limit");
            ui.horizontal(|ui| {
                changed |=
                    ui.add(egui::TextEdit::singleline(&mut self.top).desired_width(72.0).hint_text("none")).changed();
                ui.label(units.length());
            });
            ui.end_row();
            ui.label("Bottom").on_hover_text("Keep only moves at or above this height (WCS Z); empty for no limit");
            ui.horizontal(|ui| {
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut self.bottom).desired_width(72.0).hint_text("none"))
                    .changed();
                ui.label(units.length());
            });
            ui.end_row();
            ui.label("Links").on_hover_text("How the tool gets from one piece left to the next");
            let label = |id: &str| match id {
                "retract" => "Retract height",
                "clearance" => "Clearance height",
                "feed" => "Feed across",
                _ => "Auto",
            };
            egui::ComboBox::from_id_salt("trim-link").selected_text(label(self.link)).width(150.0).show_ui(ui, |ui| {
                for (id, help) in [
                    ("auto", "Feed across short gaps that stay on the kept side; retract elsewhere"),
                    ("retract", "Rise to the retract height between pieces"),
                    ("clearance", "Rise to the clearance height between pieces"),
                    ("feed", "Always feed straight across"),
                ] {
                    if ui.selectable_label(self.link == id, label(id)).on_hover_text(help).clicked() {
                        self.link = id;
                        changed = true;
                    }
                }
            });
            ui.end_row();
        });
        // The first toolpath's trims, to change or take off.
        let trims = self
            .targets
            .first()
            .and_then(|id| session.job.toolpaths.iter().find(|t| &t.id == id))
            .map(|t| (t.id.clone(), t.trims.clone()))
            .filter(|(_, trims)| !trims.is_empty());
        if let Some((id, trims)) = trims.filter(|_| self.targets.len() == 1) {
            ui.add_space(12.0);
            ui.label(RichText::new("Trims on this toolpath").strong());
            for (i, t) in trims.iter().enumerate() {
                let index = i + 1;
                ui.horizontal(|ui| {
                    let editing = self.index == Some(index);
                    let text = format!("{index}. {}", CosmicDMS_engine::toolpath::trim::describe(t));
                    ui.label(if editing { RichText::new(text).color(tokens.accent) } else { RichText::new(text) });
                    if !editing && ui.small_button("Edit").clicked() {
                        self.edit(t, index, session);
                    }
                    if ui.small_button("Remove").on_hover_text("Give back what this trim cut away").clicked() {
                        self.requests.push(("toolpath.trim.remove".into(), json!({ "id": id, "index": index })));
                        if self.index == Some(index) {
                            self.index = None;
                        }
                        changed = true;
                    }
                });
            }
            if self.index.is_some() && ui.small_button("Add a new trim instead").clicked() {
                self.index = None;
                changed = true;
            }
        }
        self.dirty |= changed;
    }
}

/// `v` without its part along `n`.
fn flat(n: Vec3, v: Vec3) -> Vec3 {
    v - n * v.dot(n)
}

/// The plane boundaries are drawn on for toolpath `id`: across its tool axis, at the top of
/// its cuts, with the X and Y of the WCS it is worked out in.
fn work_plane(session: &Session, id: Option<&str>) -> (Vec3, Vec3, Vec3, Vec3) {
    let Some(tp) = id.and_then(|id| session.job.toolpaths.iter().find(|t| t.id == id)) else {
        return (Vec3::ZERO, Vec3::Z, Vec3::X, Vec3::Y);
    };
    let frame = tp
        .params
        .get("orientation")
        .and_then(Value::as_str)
        .filter(|o| !o.is_empty())
        .or(tp.wcs.as_deref())
        .and_then(|w| session.job.wcs_by_id(w));
    let n = tp.axis.try_normalize().unwrap_or(Vec3::Z);
    let x = frame.map_or(Vec3::X, |w| w.x_axis);
    let x = flat(n, x).try_normalize().unwrap_or_else(|| n.any_orthonormal_vector());
    let y = n.cross(x);
    let cuts = tp.moves.iter().filter(|m| m.kind != MoveKind::Rapid);
    let top = cuts.clone().map(|m| m.to.dot(n)).fold(f32::NEG_INFINITY, f32::max);
    let origin = frame.map_or(Vec3::ZERO, |w| w.origin);
    let at = if top.is_finite() { origin + n * (top - origin.dot(n)) } else { origin };
    (at, n, x, y)
}
