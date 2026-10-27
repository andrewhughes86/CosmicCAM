//! CAD mode's panels, docked at the right of the window. The sketch panel draws on a plane by
//! clicking in the viewport (lines chained end to end, rectangles, circles, arcs, polygons),
//! snapping to the grid and to points already drawn, with every number editable in a list;
//! OK runs `cad.sketch`. The feature panel sets up an extrusion or revolution, adding to or
//! cutting from the part, and previews the result live; OK runs `cad.extrude` or
//! `cad.revolve`.

use std::sync::Arc;

use CosmicDMS_engine::Session;
use CosmicDMS_engine::cad::{self, MAX_ENTITIES};
use CosmicDMS_geom::{Mesh, Vec3};
use CosmicDMS_kernel::Plane;
use egui::{RichText, Ui};
use glam::{DVec2, DVec3};
use serde_json::{Map, Value, json};

use crate::Request;
use crate::theme::Tokens;
use crate::toolpath_panel::ok_cancel;
use crate::ui_kit::{Panel, Surface};
use crate::viewport::{Accept, PickHit, PickMode};

/// A sketch tool: what clicks on the plane draw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Line,
    Rectangle,
    Circle,
    Arc,
    Polygon,
}

const TOOLS: &[(Tool, &str, &str)] = &[
    (Tool::Line, "Line", "Click points: each line starts where the last ended; click the first point to close"),
    (Tool::Rectangle, "Rectangle", "Click two opposite corners"),
    (Tool::Circle, "Circle", "Click the centre, then a point on the circle"),
    (Tool::Arc, "Arc", "Click the centre, the start, then the end (counter-clockwise)"),
    (Tool::Polygon, "Polygon", "Click the centre, then a corner"),
];

impl Tool {
    /// The tool a sketch command starts.
    pub fn from_command(id: &str) -> Option<Self> {
        Some(match id {
            "cad.sketch.line" => Self::Line,
            "cad.sketch.rectangle" => Self::Rectangle,
            "cad.sketch.circle" => Self::Circle,
            "cad.sketch.arc" => Self::Arc,
            "cad.sketch.polygon" => Self::Polygon,
            _ => return None,
        })
    }

    /// Clicks it takes.
    fn clicks(self) -> usize {
        match self {
            Tool::Line | Tool::Rectangle | Tool::Circle | Tool::Polygon => 2,
            Tool::Arc => 3,
        }
    }
}

fn pt(p: DVec2) -> Value {
    json!([round(p.x), round(p.y)])
}

/// Rounds away floating-point noise (to a millionth of a millimetre).
fn round(v: f64) -> f64 {
    (v * 1.0e6).round() / 1.0e6
}

fn read_pt(v: &Value, key: &str) -> Option<DVec2> {
    let a = v.get(key)?.as_array()?;
    Some(DVec2::new(a.first()?.as_f64()?, a.get(1)?.as_f64()?))
}

/// The points worth snapping to on an entity: its ends, corners and centre.
fn key_points(e: &Value) -> Vec<DVec2> {
    let mut out: Vec<DVec2> =
        ["from", "to", "corner", "opposite", "center", "start", "end"].iter().filter_map(|k| read_pt(e, k)).collect();
    if let (Some(a), Some(c)) = (read_pt(e, "corner"), read_pt(e, "opposite")) {
        out.push(DVec2::new(c.x, a.y));
        out.push(DVec2::new(a.x, c.y));
    }
    out
}

pub struct SketchPanel {
    editing: Option<String>,
    name: String,
    plane: &'static str,
    offset: f64,
    entities: Vec<Value>,
    tool: Tool,
    /// Clicks so far for the entity being drawn (plane coordinates).
    clicks: Vec<DVec2>,
    /// Where the current chain of lines began, to close it.
    chain_start: Option<DVec2>,
    grid: f64,
    sides: usize,
    error: Option<String>,
    /// Commands to run now without closing the panel (the camera turning to the plane).
    requests: Vec<Request>,
    /// Open the extrude panel on this sketch once it is saved.
    pub extrude_after: bool,
}

impl SketchPanel {
    /// A new sketch on `plane` (top, front, right), or `editing` an existing one; `tool` is
    /// the first tool.
    pub fn new(session: &Session, editing: Option<&str>, plane: Option<&str>, tool: Tool) -> Self {
        let mut panel = Self {
            editing: None,
            name: String::new(),
            plane: "top",
            offset: 0.0,
            entities: Vec::new(),
            tool,
            clicks: Vec::new(),
            chain_start: None,
            // A round step in the units shown: 1 mm, or 0.05 in.
            grid: if session.prefs.units == CosmicDMS_engine::units::DisplayUnits::Inches { 1.27 } else { 1.0 },
            sides: 6,
            error: None,
            requests: Vec::new(),
            extrude_after: false,
        };
        if let Some(f) = editing.and_then(|id| session.job.part.find(id)).filter(|f| f.kind == "sketch") {
            panel.editing = Some(f.id.clone());
            panel.name = f.name.clone();
            panel.set_plane(f.params.get("plane").and_then(Value::as_str).unwrap_or("top"));
            panel.offset = f.params.get("offset").and_then(Value::as_f64).unwrap_or(0.0);
            panel.entities = f.params.get("entities").and_then(Value::as_array).cloned().unwrap_or_default();
        } else {
            panel.set_plane(plane.unwrap_or("top"));
        }
        panel.requests.push(("view.camera.preset".into(), json!({ "preset": panel.plane })));
        panel
    }

    fn set_plane(&mut self, p: &str) {
        self.plane = match p {
            "front" => "front",
            "right" => "right",
            _ => "top",
        };
    }

    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    pub fn fail(&mut self, message: String) {
        self.error = Some(message);
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.clicks.clear();
        self.chain_start = None;
    }

    fn work_plane(&self) -> Plane {
        cad::plane_of(&json!({ "plane": self.plane, "offset": self.offset })).unwrap_or(Plane {
            origin: DVec3::ZERO,
            x: DVec3::X,
            y: DVec3::Y,
        })
    }

    fn local(&self, p: Vec3) -> DVec2 {
        let pl = self.work_plane();
        let d = p.as_dvec3() - pl.origin;
        DVec2::new(d.dot(pl.x), d.dot(pl.y))
    }

    /// A point on the plane snapped to a point already drawn when one is near, else to the grid.
    fn snap(&self, p: DVec2) -> DVec2 {
        let reach = (self.grid * 0.6).max(0.4);
        let mut best: Option<(f64, DVec2)> = None;
        let mut candidates: Vec<DVec2> = self.entities.iter().flat_map(key_points).collect();
        candidates.extend(self.clicks.iter().copied());
        candidates.extend(self.chain_start);
        candidates.push(DVec2::ZERO);
        for c in candidates {
            let d = c.distance(p);
            if d < reach && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, c));
            }
        }
        match best {
            Some((_, c)) => c,
            None if self.grid > 0.0 => (p / self.grid).round() * self.grid,
            None => p,
        }
    }

    /// The entity the clicks so far (and `last`) make, when they are enough.
    fn entity(&self, clicks: &[DVec2]) -> Option<Value> {
        Some(match (self.tool, clicks) {
            (Tool::Line, [a, b, ..]) if a.distance(*b) > 1.0e-6 => {
                json!({ "type": "line", "from": pt(*a), "to": pt(*b) })
            }
            (Tool::Rectangle, [a, b, ..]) if (a.x - b.x).abs() > 1.0e-6 && (a.y - b.y).abs() > 1.0e-6 => {
                json!({ "type": "rectangle", "corner": pt(*a), "opposite": pt(*b) })
            }
            (Tool::Circle, [c, r, ..]) if c.distance(*r) > 1.0e-6 => {
                json!({ "type": "circle", "center": pt(*c), "radius": round(c.distance(*r)) })
            }
            (Tool::Arc, [c, s, e, ..])
                if c.distance(*s) > 1.0e-6 && c.distance(*e) > 1.0e-6 && s.distance(*e) > 1.0e-6 =>
            {
                json!({ "type": "arc", "center": pt(*c), "start": pt(*s), "end": pt(*e) })
            }
            (Tool::Polygon, [c, k, ..]) if c.distance(*k) > 1.0e-6 => {
                let d = *k - *c;
                json!({ "type": "polygon", "center": pt(*c), "radius": round(d.length()), "sides": self.sides, "angle": round(d.y.atan2(d.x).to_degrees()) })
            }
            _ => return None,
        })
    }

    /// A click on the plane.
    pub fn picked(&mut self, hit: PickHit) {
        if !hit.model.is_empty() || self.entities.len() >= MAX_ENTITIES {
            return;
        }
        let p = self.snap(self.local(hit.point));
        self.error = None;
        if self.tool == Tool::Line {
            match self.clicks.first().copied() {
                None => {
                    self.clicks = vec![p];
                    self.chain_start = Some(p);
                }
                Some(a) => {
                    if let Some(e) = self.entity(&[a, p]) {
                        self.entities.push(e);
                    }
                    // Back at the chain's start: the loop is closed.
                    if self.chain_start.is_some_and(|s| s.distance(p) < 1.0e-9) {
                        self.clicks.clear();
                        self.chain_start = None;
                    } else {
                        self.clicks = vec![p];
                    }
                }
            }
            return;
        }
        self.clicks.push(p);
        if self.clicks.len() >= self.tool.clicks() {
            if let Some(e) = self.entity(&self.clicks) {
                self.entities.push(e);
            }
            self.clicks.clear();
        }
    }

    /// What the viewport picks and draws: points on the plane, the entities and the one being
    /// drawn up to the pointer (`hover`).
    pub fn mode(&self, hover: Option<Vec3>) -> PickMode {
        let pl = self.work_plane();
        let world = |p: DVec2| pl.world(p).as_vec3();
        let mut lines: Vec<Vec<Vec3>> = Vec::new();
        let mut all = self.entities.clone();
        if let Some(h) = hover.map(|h| self.snap(self.local(h))) {
            let mut clicks = self.clicks.clone();
            if !clicks.is_empty() {
                clicks.push(h);
                all.extend(self.entity(&clicks));
            }
        }
        for e in &all {
            if let Ok(segs) = cad::entity(e) {
                for s in segs {
                    lines.push(cad::segment_points(&s).into_iter().map(world).collect());
                }
            }
        }
        // The plane's outline around what is drawn.
        let span =
            self.entities.iter().flat_map(key_points).map(|p| p.abs().max_element()).fold(20.0f64, f64::max) * 1.3;
        let corners = [
            DVec2::new(-span, -span),
            DVec2::new(span, -span),
            DVec2::new(span, span),
            DVec2::new(-span, span),
            DVec2::new(-span, -span),
        ];
        lines.push(corners.iter().map(|c| world(*c)).collect());
        let n = pl.normal();
        PickMode {
            accept: Some(Accept::Plane),
            plane: Some((pl.origin.as_vec3(), n.as_vec3())),
            picked_points: self.clicks.iter().map(|c| world(*c)).chain(self.chain_start.map(world)).collect(),
            lines,
            ..PickMode::default()
        }
    }

    fn params(&self) -> Value {
        let mut p = Map::new();
        if let Some(id) = &self.editing {
            p.insert("id".into(), json!(id));
        }
        p.insert("plane".into(), json!(self.plane));
        if self.offset.abs() > 1.0e-12 {
            p.insert("offset".into(), json!(self.offset));
        }
        p.insert("entities".into(), json!(self.entities));
        Value::Object(p)
    }

    /// Draws the panel. Returns `cad.sketch` to run on OK, and whether the panel stays open.
    pub fn show(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens) -> (Option<Request>, bool) {
        let units = session.prefs.units;
        if !ui.ctx().egui_wants_keyboard_input() {
            let (esc, enter) = ui.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Enter)));
            if esc || enter {
                self.clicks.clear();
                self.chain_start = None;
            }
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        let mut result = (None, true);
        Panel::right("sketch-panel", Surface::SidePanel, 340.0).show(ui, tokens, |ui| {
            let title = if self.editing.is_some() { format!("Edit {}", self.name) } else { "New sketch".to_owned() };
            ui.label(RichText::new(title).heading());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Plane");
                for (id, label) in [("top", "Top"), ("front", "Front"), ("right", "Right")] {
                    if ui.selectable_label(self.plane == id, label).clicked() && self.plane != id {
                        self.set_plane(id);
                        self.requests.push(("view.camera.preset".into(), json!({ "preset": id })));
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Offset").on_hover_text("Move the plane along its normal");
                let mut v = units.from_mm(self.offset);
                if ui.add(egui::DragValue::new(&mut v).speed(0.5).suffix(format!(" {}", units.length()))).changed() {
                    self.offset = units.to_mm(v);
                }
                ui.label("Grid").on_hover_text("Clicks snap to this spacing (0 for none)");
                let mut g = units.from_mm(self.grid);
                if ui.add(egui::DragValue::new(&mut g).speed(0.1).range(0.0..=1000.0)).changed() {
                    self.grid = units.to_mm(g).max(0.0);
                }
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                for (tool, label, help) in TOOLS {
                    if ui.selectable_label(self.tool == *tool, *label).on_hover_text(*help).clicked() {
                        self.set_tool(*tool);
                    }
                }
            });
            if self.tool == Tool::Polygon {
                ui.horizontal(|ui| {
                    ui.label("Sides");
                    ui.add(egui::DragValue::new(&mut self.sides).range(3..=360));
                });
            }
            let hint = TOOLS.iter().find(|(t, _, _)| *t == self.tool).map_or("", |(_, _, h)| *h);
            ui.label(RichText::new(format!("{hint}. Esc stops.")).small().color(tokens.text_muted));
            ui.separator();
            let body = (ui.available_height() - 110.0).max(80.0);
            let mut remove = None;
            egui::ScrollArea::vertical().max_height(body).auto_shrink([false, false]).show(ui, |ui| {
                if self.entities.is_empty() {
                    ui.label(RichText::new("Nothing drawn yet: click in the viewport.").color(tokens.text_muted));
                }
                for (i, e) in self.entities.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let kind = e.get("type").and_then(Value::as_str).unwrap_or("?").to_owned();
                        ui.label(RichText::new(format!("{} {kind}", i)).strong());
                        if ui.small_button("×").on_hover_text("Delete").clicked() {
                            remove = Some(i);
                        }
                    });
                    entity_fields(ui, e, units);
                }
            });
            if let Some(i) = remove {
                self.entities.remove(i);
            }
            ui.separator();
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(tokens.error));
            } else {
                let profile = cad::sketch_of(&self.params()).and_then(|s| {
                    CosmicDMS_kernel::loops(&s.segments)
                        .map_err(|e| CosmicDMS_engine::error::EngineError::InvalidParams(e.0))
                });
                match profile {
                    Ok(l) => ui.label(
                        RichText::new(format!("{} closed profile{}", l.len(), if l.len() == 1 { "" } else { "s" }))
                            .color(tokens.success),
                    ),
                    Err(_) if self.entities.is_empty() => ui.label(""),
                    Err(e) => ui.label(RichText::new(e.to_string()).small().color(tokens.warning)),
                };
            }
            ui.horizontal(|ui| {
                if ui
                    .button("OK, then extrude")
                    .on_hover_text("Save the sketch and go straight on to extrude it")
                    .clicked()
                {
                    self.extrude_after = true;
                    result.0 = Some(("cad.sketch".to_owned(), self.params()));
                }
            });
            ui.add_space(4.0);
            let (ok, cancel) = ok_cancel(ui, true);
            if ok {
                result.0 = Some(("cad.sketch".to_owned(), self.params()));
            }
            if cancel {
                result.1 = false;
            }
        });
        result
    }
}

/// The numbers of one entity, editable in the display units.
fn entity_fields(ui: &mut Ui, e: &mut Value, units: CosmicDMS_engine::units::DisplayUnits) {
    let Some(o) = e.as_object_mut() else { return };
    ui.horizontal_wrapped(|ui| {
        for key in ["from", "to", "corner", "opposite", "center", "start", "end"] {
            let Some(arr) = o.get_mut(key).and_then(Value::as_array_mut) else { continue };
            ui.label(RichText::new(key).small());
            for slot in arr.iter_mut().take(2) {
                let mut v = units.from_mm(slot.as_f64().unwrap_or(0.0));
                if ui.add(egui::DragValue::new(&mut v).speed(0.1).max_decimals(4)).changed() {
                    *slot = json!(round(units.to_mm(v)));
                }
            }
        }
        if let Some(r) = o.get_mut("radius") {
            ui.label(RichText::new("radius").small());
            let mut v = units.from_mm(r.as_f64().unwrap_or(1.0));
            if ui.add(egui::DragValue::new(&mut v).speed(0.1).range(0.0001..=1.0e6).max_decimals(4)).changed() {
                *r = json!(round(units.to_mm(v)));
            }
        }
        if let Some(n) = o.get_mut("sides") {
            ui.label(RichText::new("sides").small());
            let mut v = n.as_f64().unwrap_or(6.0) as usize;
            if ui.add(egui::DragValue::new(&mut v).range(3..=360)).changed() {
                *n = json!(v);
            }
        }
    });
}

/// Extrude or revolve.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FeatureKind {
    Extrude,
    Revolve,
}

impl FeatureKind {
    pub fn from_command(id: &str) -> Option<(Self, &'static str)> {
        Some(match id {
            "cad.extrude" => (Self::Extrude, "join"),
            "cad.extrude.cut" => (Self::Extrude, "cut"),
            "cad.revolve" => (Self::Revolve, "join"),
            "cad.revolve.cut" => (Self::Revolve, "cut"),
            _ => return None,
        })
    }

    fn command(self) -> &'static str {
        match self {
            Self::Extrude => "cad.extrude",
            Self::Revolve => "cad.revolve",
        }
    }
}

/// The part as a feature would leave it (its mesh, a summary, and the part with the bodies it
/// worked out), or why it cannot be made.
type Preview = Result<(Arc<Mesh>, String, Box<cad::Part>), String>;

fn compute(mut trial: Session, command: &str, p: &Value) -> Preview {
    trial.execute(command, p).map_err(|e| e.to_string())?;
    let id = trial.job.part.model.clone().ok_or_else(|| "nothing to show".to_owned())?;
    let m = trial.job.models.iter().find(|m| m.id == id).ok_or_else(|| "nothing to show".to_owned())?;
    let mass = cad::mass_properties(&m.mesh);
    let units = trial.prefs.units;
    let per = units.mm_per_unit().powi(3);
    let volume = mass["volume"].as_f64().unwrap_or(0.0) / per;
    Ok((Arc::clone(&m.mesh), format!("Volume {volume:.3} {}³", units.length()), Box::new(trial.job.part.clone())))
}

pub struct FeaturePanel {
    kind: FeatureKind,
    editing: Option<String>,
    name: String,
    sketch: Option<String>,
    distance: f64,
    direction: &'static str,
    operation: &'static str,
    through_all: bool,
    axis: &'static str,
    line: usize,
    angle: f64,
    preview: Preview,
    pending: Option<std::sync::mpsc::Receiver<Preview>>,
    dirty: bool,
    error: Option<String>,
}

impl FeaturePanel {
    /// A new feature of `kind` doing `operation`, on `sketch` (default: the last one), or
    /// `editing` an existing one.
    pub fn new(
        session: &Session,
        kind: FeatureKind,
        operation: &str,
        editing: Option<&str>,
        sketch: Option<&str>,
    ) -> Self {
        let part = &session.job.part;
        let last = part.features.iter().rev().find(|f| f.kind == "sketch" && !f.suppressed).map(|f| f.id.clone());
        let mut panel = Self {
            kind,
            editing: None,
            name: String::new(),
            sketch: sketch.map(str::to_owned).or(last),
            distance: 10.0,
            direction: "normal",
            operation: if operation == "cut" { "cut" } else { "join" },
            through_all: false,
            axis: "y",
            line: 0,
            angle: 360.0,
            preview: Err(String::new()),
            pending: None,
            dirty: true,
            error: None,
        };
        if let Some(f) = editing.and_then(|id| part.find(id)) {
            let p = &f.params;
            let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
            panel.editing = Some(f.id.clone());
            panel.name = f.name.clone();
            panel.sketch = Some(s("sketch"));
            panel.distance = p.get("distance").and_then(Value::as_f64).unwrap_or(10.0);
            panel.direction = match s("direction").as_str() {
                "reverse" => "reverse",
                "symmetric" => "symmetric",
                _ => "normal",
            };
            panel.operation = match s("operation").as_str() {
                "cut" => "cut",
                "new" => "new",
                _ => "join",
            };
            panel.through_all = p.get("through_all").and_then(Value::as_bool).unwrap_or(false);
            panel.axis = match s("axis").as_str() {
                "x" => "x",
                "line" => "line",
                _ => "y",
            };
            panel.line = p.get("line").and_then(Value::as_f64).unwrap_or(0.0) as usize;
            panel.angle = p.get("angle").and_then(Value::as_f64).unwrap_or(360.0);
        }
        panel
    }

    pub fn fail(&mut self, message: String) {
        self.error = Some(message);
    }

    /// Hands the preview's worked-out bodies to `part`, so OK does not build them again.
    pub fn warm(&self, part: &mut cad::Part) {
        if let Ok((_, _, previewed)) = &self.preview {
            part.share_cache(previewed);
        }
    }

    fn params(&self) -> Value {
        let mut p = Map::new();
        if let Some(id) = &self.editing {
            p.insert("id".into(), json!(id));
        }
        if let Some(s) = &self.sketch {
            p.insert("sketch".into(), json!(s));
        }
        p.insert("operation".into(), json!(self.operation));
        match self.kind {
            FeatureKind::Extrude => {
                p.insert("distance".into(), json!(self.distance));
                p.insert("direction".into(), json!(self.direction));
                if self.through_all {
                    p.insert("through_all".into(), json!(true));
                }
            }
            FeatureKind::Revolve => {
                p.insert("axis".into(), json!(self.axis));
                if self.axis == "line" {
                    p.insert("line".into(), json!(self.line));
                }
                p.insert("angle".into(), json!(self.angle));
            }
        }
        Value::Object(p)
    }

    fn refresh(&mut self, session: &Session) {
        self.dirty = false;
        self.error = None;
        let trial = session.clone();
        let (command, p) = (self.kind.command(), self.params());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new().name("cad-preview".into()).spawn(move || {
                let _ = tx.send(compute(trial, command, &p));
            });
            match spawned {
                Ok(_) => self.pending = Some(rx),
                Err(e) => self.preview = Err(format!("could not start the preview: {e}")),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.preview = compute(trial, command, &p);
        }
    }

    fn poll(&mut self) -> bool {
        let Some(rx) = &self.pending else { return false };
        match rx.try_recv() {
            Ok(p) => {
                self.preview = p;
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

    /// The viewport shows the previewed part in place of the current one.
    pub fn mode(&self, session: &Session) -> PickMode {
        PickMode {
            solid: self.preview.as_ref().ok().map(|(m, _, _)| Arc::clone(m)),
            hide: if self.preview.is_ok() { session.job.part.model.iter().cloned().collect() } else { Vec::new() },
            ..PickMode::default()
        }
    }

    pub fn show(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens) -> (Option<Request>, bool) {
        if self.dirty && self.pending.is_none() {
            self.refresh(session);
        }
        let working = self.poll();
        if working {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        }
        let units = session.prefs.units;
        let mut result = (None, true);
        let sketches: Vec<(String, String)> = session
            .job
            .part
            .features
            .iter()
            .filter(|f| f.kind == "sketch" && !f.suppressed)
            .map(|f| (f.id.clone(), f.name.clone()))
            .collect();
        let lines: Vec<usize> = self
            .sketch
            .as_deref()
            .and_then(|id| session.job.part.find(id))
            .and_then(|f| f.params.get("entities").and_then(Value::as_array).cloned())
            .unwrap_or_default()
            .iter()
            .enumerate()
            .filter(|(_, e)| e.get("type").and_then(Value::as_str) == Some("line"))
            .map(|(i, _)| i)
            .collect();
        Panel::right("cad-feature-panel", Surface::SidePanel, 320.0).show(ui, tokens, |ui| {
            let what = match (self.kind, self.operation) {
                (FeatureKind::Extrude, "cut") => "Cut extrude",
                (FeatureKind::Revolve, "cut") => "Cut revolve",
                (FeatureKind::Extrude, _) => "Extrude",
                (FeatureKind::Revolve, _) => "Revolve",
            };
            let title = if self.editing.is_some() { format!("Edit {}", self.name) } else { what.to_owned() };
            ui.label(RichText::new(title).heading());
            ui.add_space(6.0);
            let mut changed = false;
            egui::Grid::new("cad-feature").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Sketch");
                let current = self
                    .sketch
                    .as_deref()
                    .and_then(|id| sketches.iter().find(|(i, _)| i == id))
                    .map_or("choose…".to_owned(), |(_, n)| n.clone());
                egui::ComboBox::from_id_salt("cad-sketch").selected_text(current).show_ui(ui, |ui| {
                    for (id, name) in &sketches {
                        if ui.selectable_label(self.sketch.as_deref() == Some(id), name).clicked() {
                            self.sketch = Some(id.clone());
                            changed = true;
                        }
                    }
                });
                ui.end_row();
                ui.label("Result");
                ui.horizontal(|ui| {
                    for (id, label, help) in [
                        ("join", "Add", "Join it to the part"),
                        ("cut", "Cut", "Take it away from the part"),
                        ("new", "New body", "Keep it as a separate body"),
                    ] {
                        if ui.selectable_label(self.operation == id, label).on_hover_text(help).clicked() {
                            self.operation = id;
                            changed = true;
                        }
                    }
                });
                ui.end_row();
                match self.kind {
                    FeatureKind::Extrude => {
                        ui.label("Distance");
                        ui.add_enabled_ui(!self.through_all, |ui| {
                            let mut v = units.from_mm(self.distance);
                            if ui
                                .add(
                                    egui::DragValue::new(&mut v)
                                        .speed(0.2)
                                        .range(0.0001..=1.0e6)
                                        .suffix(format!(" {}", units.length())),
                                )
                                .changed()
                            {
                                self.distance = units.to_mm(v);
                                changed = true;
                            }
                        });
                        ui.end_row();
                        ui.label("Direction");
                        ui.horizontal(|ui| {
                            for (id, label, help) in [
                                ("normal", "One way", "Along the sketch normal"),
                                ("reverse", "Flip", "Against the sketch normal"),
                                ("symmetric", "Mid-plane", "Half each way"),
                            ] {
                                if ui.selectable_label(self.direction == id, label).on_hover_text(help).clicked() {
                                    self.direction = id;
                                    changed = true;
                                }
                            }
                        });
                        ui.end_row();
                        ui.label("");
                        changed |= ui
                            .checkbox(&mut self.through_all, "Through all")
                            .on_hover_text("Right through the part, both ways")
                            .changed();
                        ui.end_row();
                    }
                    FeatureKind::Revolve => {
                        ui.label("Axis");
                        ui.horizontal(|ui| {
                            for (id, label) in [("x", "Sketch X"), ("y", "Sketch Y"), ("line", "A line")] {
                                if ui.selectable_label(self.axis == id, label).clicked() {
                                    self.axis = id;
                                    changed = true;
                                }
                            }
                        });
                        ui.end_row();
                        if self.axis == "line" {
                            ui.label("Line");
                            egui::ComboBox::from_id_salt("cad-axis-line")
                                .selected_text(format!("Line {}", self.line))
                                .show_ui(ui, |ui| {
                                    for i in &lines {
                                        if ui.selectable_label(self.line == *i, format!("Line {i}")).clicked() {
                                            self.line = *i;
                                            changed = true;
                                        }
                                    }
                                });
                            ui.end_row();
                        }
                        ui.label("Angle");
                        if ui
                            .add(egui::DragValue::new(&mut self.angle).speed(1.0).range(0.001..=360.0).suffix("°"))
                            .changed()
                        {
                            changed = true;
                        }
                        ui.end_row();
                    }
                }
            });
            self.dirty |= changed;
            ui.add_space(10.0);
            ui.separator();
            ui.vertical_centered(|ui| match (&self.error, &self.preview) {
                _ if working => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label("Building…");
                    });
                }
                (Some(e), _) => {
                    ui.label(RichText::new(e).color(tokens.error));
                }
                (None, Ok((_, summary, _))) => {
                    ui.label(summary);
                }
                (None, Err(e)) if !e.is_empty() => {
                    ui.label(RichText::new(e).color(tokens.warning));
                }
                _ => {}
            });
            ui.add_space(8.0);
            let (ok, cancel) = ok_cancel(ui, self.preview.is_ok() && !working);
            if ok {
                result.0 = Some((self.kind.command().to_owned(), self.params()));
            }
            if cancel {
                result.1 = false;
            }
        });
        result
    }
}
