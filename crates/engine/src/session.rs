//! The session: all engine state, changed only through [`Session::execute`].

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use CosmicCAM_geom::{Aabb, EdgeKind, EdgeMap, FaceKind, FaceMap, Mesh, Vec3};
use CosmicCAM_io::{ModelFormat, Units};
use CosmicCAM_render::OrbitCamera;
use serde_json::{Value, json};

use crate::cmd::{self, CommandSpec, RibbonTab};
use crate::error::{EngineError, EngineResult};

/// Most log entries kept; older ones are dropped.
pub const MAX_LOG: usize = 1000;
pub const DEFAULT_FPS_LIMIT: u32 = 60;
pub const MAX_FPS_LIMIT: u32 = 240;
/// Larger command results are summarised in the log instead of stored.
const MAX_LOGGED_RESULT_BYTES: usize = 4096;
/// Most UI requests queued between frames.
const MAX_UI_REQUESTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    pub fn id(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }
}

/// Which viewport elements are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Display {
    pub grid: bool,
    /// The WCS triad drawn in the scene.
    pub wcs: bool,
    pub view_cube: bool,
    /// The screen-corner orientation triad.
    pub orientation_triad: bool,
    pub stock: bool,
    /// Model outline edges.
    pub edges: bool,
    /// Projection and frame-rate readout.
    pub readout: bool,
}

impl Display {
    pub const KEYS: &'static [&'static str] =
        &["grid", "wcs", "view_cube", "orientation_triad", "stock", "edges", "readout"];

    pub fn get(&self, key: &str) -> Option<bool> {
        Some(match key {
            "grid" => self.grid,
            "wcs" => self.wcs,
            "view_cube" => self.view_cube,
            "orientation_triad" => self.orientation_triad,
            "stock" => self.stock,
            "edges" => self.edges,
            "readout" => self.readout,
            _ => return None,
        })
    }

    pub fn set(&mut self, key: &str, on: bool) -> bool {
        let slot = match key {
            "grid" => &mut self.grid,
            "wcs" => &mut self.wcs,
            "view_cube" => &mut self.view_cube,
            "orientation_triad" => &mut self.orientation_triad,
            "stock" => &mut self.stock,
            "edges" => &mut self.edges,
            "readout" => &mut self.readout,
            _ => return false,
        };
        *slot = on;
        true
    }

    pub fn to_json(&self) -> Value {
        Value::Object(Self::KEYS.iter().filter_map(|k| Some(((*k).to_owned(), json!(self.get(k)?)))).collect())
    }
}

impl Default for Display {
    fn default() -> Self {
        Self {
            grid: true,
            wcs: true,
            view_cube: true,
            orientation_triad: true,
            stock: true,
            edges: true,
            readout: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Preferences {
    pub theme: Theme,
    /// Units lengths and feeds are shown in (the API stays in millimetres).
    pub units: crate::units::DisplayUnits,
    /// Bumped when a saved preference changes, so the app knows to save.
    pub revision: u64,
    /// Upper bound on viewport frames per second while animating.
    pub fps_limit: u32,
    pub display: Display,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            units: crate::units::DisplayUnits::default(),
            revision: 0,
            fps_limit: DEFAULT_FPS_LIMIT,
            display: Display::default(),
        }
    }
}

/// Workspace panes the UI can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    Viewport,
    Explorer,
    Inspector,
    Console,
    /// Machines and the active machine's tools.
    Library,
}

impl Pane {
    pub const ALL: [Self; 5] = [Self::Viewport, Self::Explorer, Self::Inspector, Self::Console, Self::Library];
    pub const IDS: &'static [&'static str] = &["viewport", "explorer", "inspector", "console", "library"];

    pub fn id(self) -> &'static str {
        match self {
            Self::Viewport => "viewport",
            Self::Explorer => "explorer",
            Self::Inspector => "inspector",
            Self::Console => "console",
            Self::Library => "library",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }
}

/// Something only the UI can carry out (window and pane changes). Commands queue these;
/// the UI drains them each frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UiRequest {
    ShowPane(Pane),
    HidePane(Pane),
    FocusPane(Pane),
    WindowSize { width: f32, height: f32 },
    Fullscreen(bool),
    Maximized(bool),
    Quit,
}

/// UI state the engine keeps so commands and the API can read and change it.
#[derive(Clone, Debug)]
pub struct UiState {
    pub ribbon_tab: RibbonTab,
    /// Panes currently open, reported by the UI each frame.
    pub open_panes: Vec<Pane>,
    requests: Vec<UiRequest>,
}

impl Default for UiState {
    fn default() -> Self {
        Self { ribbon_tab: RibbonTab::Home, open_panes: Pane::ALL.to_vec(), requests: Vec::new() }
    }
}

impl UiState {
    pub(crate) fn request(&mut self, r: UiRequest) {
        if self.requests.len() < MAX_UI_REQUESTS {
            self.requests.push(r);
        }
    }
}

#[derive(Clone, Debug)]
pub struct ViewState {
    /// Where the camera should be. The viewport animates its shown camera towards it.
    pub camera: OrbitCamera,
    /// Bounds of everything in the scene, for fitting and clip planes.
    pub scene_bounds: Aabb,
    /// Width / height of the main viewport, reported by the UI.
    pub aspect: f32,
    /// Bumped by `view.layout.reset`; the UI rebuilds its dock layout when it changes.
    pub layout_generation: u64,
    /// Selected explorer node id.
    pub selection: Option<String>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            camera: OrbitCamera::default(),
            scene_bounds: Aabb::EMPTY,
            aspect: 1.6,
            layout_generation: 0,
            selection: None,
        }
    }
}

/// Most models one job holds.
pub const MAX_MODELS: usize = 256;

/// An imported model. The mesh is shared, so snapshotting the session stays cheap.
#[derive(Clone, Debug)]
pub struct Model {
    /// Outline id, `model.<n>`.
    pub id: String,
    pub name: String,
    /// The file it was imported from.
    pub source: String,
    /// None for the built-in sample part.
    pub format: Option<ModelFormat>,
    /// Unit the file was read in; the mesh itself is in millimetres.
    pub units: Units,
    pub mesh: Arc<Mesh>,
    /// The planes, cylinders and freeform patches recognised on the mesh.
    pub faces: Arc<FaceMap>,
    /// The straight and circular edges recognised on the mesh.
    pub edges: Arc<EdgeMap>,
}

impl Model {
    /// Face and edge recognition for a newly loaded mesh.
    pub fn recognise(mesh: &Mesh) -> (Arc<FaceMap>, Arc<EdgeMap>) {
        (Arc::new(CosmicCAM_geom::faces::segment(mesh)), Arc::new(CosmicCAM_geom::edges::extract(mesh)))
    }
}

/// `<model id>/edge.<n>`, as commands name edges.
pub fn edge_ref(model: &str, edge: usize) -> String {
    format!("{model}/edge.{edge}")
}

/// `<model id>/vertex.<n>`, as commands name vertices.
pub fn vertex_ref(model: &str, vertex: usize) -> String {
    format!("{model}/vertex.{vertex}")
}

/// A vertex as the API reports it.
pub fn vertex_json(model: &Model, vertex: usize) -> Value {
    model.edges.vertices.get(vertex).map_or(Value::Null, |p| {
        json!({ "vertex": vertex_ref(&model.id, vertex), "model": model.id, "kind": "vertex", "position": p.to_array() })
    })
}

/// A face, edge or vertex of a model, as a selection or pick names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entity {
    Face(usize),
    Edge(usize),
    Vertex(usize),
}

impl Entity {
    pub fn reference(self, model: &str) -> String {
        match self {
            Self::Face(i) => face_ref(model, i),
            Self::Edge(i) => edge_ref(model, i),
            Self::Vertex(i) => vertex_ref(model, i),
        }
    }
}

/// An edge as the API reports it.
pub fn edge_json(model: &Model, edge: usize) -> Value {
    let Some(e) = model.edges.edges.get(edge) else {
        return Value::Null;
    };
    let mut v = json!({
        "edge": edge_ref(&model.id, edge),
        "model": model.id,
        "kind": e.kind.id(),
        "length": e.length,
        "direction": e.kind.direction().to_array(),
    });
    match e.kind {
        EdgeKind::Line { start, end } => {
            v["start"] = json!(start.to_array());
            v["end"] = json!(end.to_array());
        }
        EdgeKind::Circle { center, normal, radius } => {
            v["center"] = json!(center.to_array());
            v["normal"] = json!(normal.to_array());
            v["radius"] = json!(radius);
        }
    }
    v
}

/// Outline id of the stock.
pub const STOCK_ID: &str = "stock";

/// The stock as the API reports it.
pub fn stock_json(b: &Aabb) -> Value {
    json!({ "shape": "box", "min": b.min.to_array(), "max": b.max.to_array(), "size": b.size().to_array() })
}

/// Outline id of the built-in sample part.
pub const SAMPLE_MODEL_ID: &str = "model.sample";

/// Where a ray met a model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    /// Index into `JobState::models`.
    pub model: usize,
    pub triangle: usize,
    pub face: usize,
    pub point: Vec3,
    pub distance: f32,
}

/// `<model id>/face.<n>`, as commands name faces.
pub fn face_ref(model: &str, face: usize) -> String {
    format!("{model}/face.{face}")
}

/// A face as the API reports it.
pub fn face_json(model: &Model, face: usize) -> Value {
    let Some(f) = model.faces.faces.get(face) else {
        return Value::Null;
    };
    let mut v = json!({
        "face": face_ref(&model.id, face),
        "model": model.id,
        "kind": f.kind.id(),
        "area": f.area,
        "centroid": f.centroid.to_array(),
        "triangles": f.triangles,
    });
    match f.kind {
        FaceKind::Plane { normal, point } => {
            v["normal"] = json!(normal.to_array());
            v["point"] = json!(point.to_array());
        }
        FaceKind::Cylinder { base, axis, radius, length, hole } => {
            v["base"] = json!(base.to_array());
            v["axis"] = json!(axis.to_array());
            v["radius"] = json!(radius);
            v["diameter"] = json!(radius * 2.0);
            v["length"] = json!(length);
            v["hole"] = json!(hole);
        }
        FaceKind::Freeform => {}
    }
    v
}

impl Model {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "source": self.source,
            "format": self.format.map_or("builtin", ModelFormat::id),
            "units": self.units.id(),
            "triangles": self.mesh.triangles().len(),
            "vertices": self.mesh.positions().len(),
            "faces": self.faces.faces.len(),
            "edges": self.edges.edges.len(),
            "bounds": bounds_json(&self.mesh.bounds()),
        })
    }

    /// The built-in sample part as a model, so it can be picked like an imported one.
    pub fn sample() -> Self {
        let c = [0.5; 4];
        let palette = CosmicCAM_render::ScenePalette {
            grid_minor: c,
            grid_major: c,
            axis_x: c,
            axis_y: c,
            axis_z: c,
            part: c,
            part_edge: c,
            stock_edge: c,
            path_cut: c,
            path_rapid: c,
            path_plunge: c,
            path_lead: c,
            path_link: c,
        };
        let g = CosmicCAM_render::scene::sample_part(&palette, false, false);
        let soup = g.triangles.as_chunks::<3>().0.iter().map(|t| t.map(|v| Vec3::from(v.pos)));
        let mesh = Mesh::from_soup(soup).unwrap_or_default();
        let (faces, edges) = Self::recognise(&mesh);
        Self {
            id: SAMPLE_MODEL_ID.into(),
            name: "Sample part".into(),
            source: "Built-in sample part".into(),
            format: None,
            units: Units::Millimetres,
            mesh: Arc::new(mesh),
            faces,
            edges,
        }
    }
}

/// Most work coordinate systems one job holds.
pub const MAX_WCS: usize = 64;

/// A work coordinate system: the machine's part zero and axis directions, in world
/// millimetres. `x_axis` and `z_axis` are unit length and perpendicular.
#[derive(Clone, Debug, PartialEq)]
pub struct Wcs {
    /// Outline id, `wcs.<n>`.
    pub id: String,
    pub name: String,
    pub origin: Vec3,
    pub x_axis: Vec3,
    pub z_axis: Vec3,
}

impl Wcs {
    pub fn y_axis(&self) -> Vec3 {
        self.z_axis.cross(self.x_axis)
    }

    /// A point given in this WCS, in world coordinates.
    pub fn to_world(&self, local: Vec3) -> Vec3 {
        self.origin + self.x_axis * local.x + self.y_axis() * local.y + self.z_axis * local.z
    }

    pub fn to_json(&self, active: bool) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "active": active,
            "origin": self.origin.to_array(),
            "x_axis": self.x_axis.to_array(),
            "y_axis": self.y_axis().to_array(),
            "z_axis": self.z_axis.to_array(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct JobState {
    pub name: String,
    /// The Phase 1 placeholder workpiece is shown.
    pub sample_loaded: bool,
    pub models: Vec<Model>,
    /// Number used for the next model id.
    pub(crate) next_model: u64,
    /// Bumped whenever the job's geometry changes; never repeats within a run, so the
    /// viewport rebuilds its scene when it differs from what it last drew.
    pub revision: u64,
    pub wcs: Vec<Wcs>,
    /// Id of the WCS toolpaths are programmed in.
    pub active_wcs: Option<String>,
    pub(crate) next_wcs: u64,
    /// Bumped whenever a WCS or the stock changes; like `revision`, it never repeats within a run.
    pub setup_revision: u64,
    /// The raw material: a box in world millimetres, once defined.
    pub stock: Option<Aabb>,
    pub toolpaths: Vec<crate::toolpath::Toolpath>,
    pub(crate) next_toolpath: u64,
}

impl Default for JobState {
    fn default() -> Self {
        Self {
            name: "Untitled".into(),
            sample_loaded: false,
            models: Vec::new(),
            next_model: 1,
            revision: 0,
            wcs: Vec::new(),
            active_wcs: None,
            next_wcs: 1,
            setup_revision: 0,
            stock: None,
            toolpaths: Vec::new(),
            next_toolpath: 1,
        }
    }
}

impl JobState {
    pub(crate) fn geometry_changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn setup_changed(&mut self) {
        self.setup_revision = self.setup_revision.wrapping_add(1);
    }

    pub fn wcs_by_id(&self, id: &str) -> Option<&Wcs> {
        self.wcs.iter().find(|w| w.id == id)
    }

    pub fn active_wcs(&self) -> Option<&Wcs> {
        self.active_wcs.as_deref().and_then(|id| self.wcs_by_id(id))
    }

    /// Bounds of the part geometry WCS origins are placed on.
    pub fn part_bounds(&self) -> Aabb {
        self.models_bounds()
    }

    /// The nearest model face a ray hits.
    pub fn pick(&self, origin: Vec3, direction: Vec3) -> Option<Pick> {
        let mut best: Option<Pick> = None;
        for (mi, m) in self.models.iter().enumerate() {
            if let Some((distance, triangle)) = m.mesh.raycast(origin, direction)
                && best.is_none_or(|b| distance < b.distance)
            {
                best = Some(Pick {
                    model: mi,
                    triangle,
                    face: m.faces.face_of(triangle).unwrap_or(0),
                    point: origin + direction.normalize_or_zero() * distance,
                    distance,
                });
            }
        }
        best
    }

    /// The edge passing within `radius` of a ray and not hidden behind the first surface
    /// it hits: (model index, edge index).
    pub fn pick_edge(&self, origin: Vec3, direction: Vec3, radius: f32) -> Option<(usize, usize)> {
        let limit = self.pick(origin, direction).map_or(f32::INFINITY, |hit| hit.distance + radius * 2.0);
        let mut best: Option<(usize, usize, f32)> = None;
        for (mi, m) in self.models.iter().enumerate() {
            if let Some((edge, along)) = m.edges.nearest_to_ray(origin, direction, radius, limit)
                && best.is_none_or(|(_, _, d)| along < d)
            {
                best = Some((mi, edge, along));
            }
        }
        best.map(|(m, e, _)| (m, e))
    }

    /// The vertex within `radius` of a ray and not hidden behind the first surface it hits:
    /// (model index, vertex index).
    pub fn pick_vertex(&self, origin: Vec3, direction: Vec3, radius: f32) -> Option<(usize, usize)> {
        let limit = self.pick(origin, direction).map_or(f32::INFINITY, |hit| hit.distance + radius * 2.0);
        let mut best: Option<(usize, usize, f32)> = None;
        for (mi, m) in self.models.iter().enumerate() {
            if let Some((v, along)) = m.edges.nearest_vertex_to_ray(origin, direction, radius, limit)
                && best.is_none_or(|(_, _, d)| along < d)
            {
                best = Some((mi, v, along));
            }
        }
        best.map(|(m, v, _)| (m, v))
    }

    /// What a click would select along a ray: a vertex, else an edge (both within
    /// `radius`), else the face hit. (model index, entity).
    pub fn pick_entity(&self, origin: Vec3, direction: Vec3, radius: f32) -> Option<(usize, Entity)> {
        if let Some((m, v)) = self.pick_vertex(origin, direction, radius) {
            return Some((m, Entity::Vertex(v)));
        }
        if let Some((m, e)) = self.pick_edge(origin, direction, radius) {
            return Some((m, Entity::Edge(e)));
        }
        self.pick(origin, direction).map(|hit| (hit.model, Entity::Face(hit.face)))
    }

    /// Resolves `<model id>/face.<n>`, `/edge.<n>` or `/vertex.<n>`.
    pub fn entity(&self, reference: &str) -> Option<(&Model, Entity)> {
        if let Some((m, f)) = self.face(reference) {
            return Some((m, Entity::Face(f)));
        }
        if let Some((m, e)) = self.edge(reference) {
            return Some((m, Entity::Edge(e)));
        }
        let (model, v) = reference.rsplit_once("/vertex.")?;
        let v: usize = v.parse().ok()?;
        let m = self.model(model)?;
        (v < m.edges.vertices.len()).then_some((m, Entity::Vertex(v)))
    }

    /// Resolves `<model id>/edge.<n>`.
    pub fn edge(&self, reference: &str) -> Option<(&Model, usize)> {
        let (model, edge) = reference.rsplit_once("/edge.")?;
        let edge: usize = edge.parse().ok()?;
        let m = self.model(model)?;
        (edge < m.edges.edges.len()).then_some((m, edge))
    }

    /// Resolves `<model id>/face.<n>`.
    pub fn face(&self, reference: &str) -> Option<(&Model, usize)> {
        let (model, face) = reference.rsplit_once("/face.")?;
        let face: usize = face.parse().ok()?;
        let m = self.model(model)?;
        (face < m.faces.faces.len()).then_some((m, face))
    }

    pub fn model(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.id == id)
    }

    /// Bounds of all job geometry.
    pub fn models_bounds(&self) -> Aabb {
        let mut b = Aabb::EMPTY;
        for m in &self.models {
            let mb = m.mesh.bounds();
            b.include(mb.min);
            b.include(mb.max);
        }
        b
    }
}

#[derive(Clone, Debug)]
pub struct LogEntry {
    /// Increases by one per executed command; `console.log.get` pages with it.
    pub seq: u64,
    pub command: String,
    pub params: Value,
    pub outcome: Result<Value, EngineError>,
}

/// One node of the job outline shown by the explorer tree.
#[derive(Clone, Debug)]
pub struct OutlineNode {
    pub id: String,
    pub label: String,
    pub children: Vec<OutlineNode>,
    pub properties: Vec<(String, String)>,
}

impl OutlineNode {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "label": self.label,
            "properties": self.properties.iter().map(|(k, v)| json!({ "name": k, "value": v })).collect::<Vec<_>>(),
            "children": self.children.iter().map(OutlineNode::to_json).collect::<Vec<_>>(),
        })
    }
}

/// The camera as the API reports it: angles in degrees.
pub fn camera_json(c: &OrbitCamera) -> Value {
    json!({
        "target": c.target.to_array(),
        "distance": c.distance,
        "yaw": c.yaw.to_degrees(),
        "pitch": c.pitch.to_degrees(),
        "projection": c.projection.id(),
        "eye": c.eye().to_array(),
    })
}

pub fn bounds_json(b: &Aabb) -> Value {
    if b.is_empty() { Value::Null } else { json!({ "min": b.min.to_array(), "max": b.max.to_array() }) }
}

/// Prefix of paths naming a staged upload: `upload://<file name>`.
pub const UPLOAD_PREFIX: &str = "upload://";
/// Most uploads staged at once; the oldest is dropped beyond it.
const MAX_UPLOADS: usize = 8;

#[derive(Clone, Debug, Default)]
pub struct Session {
    /// Machines and their tool libraries; kept across jobs and saved by the app.
    pub library: crate::library::MachineLibrary,
    /// Files handed to the app without a path (browser uploads), by name, until imported.
    uploads: Vec<(String, Arc<Vec<u8>>)>,
    pub prefs: Preferences,
    pub view: ViewState,
    pub job: JobState,
    pub ui: UiState,
    log: Vec<LogEntry>,
    next_seq: u64,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// A session with the placeholder workpiece, framed in an isometric view.
    pub fn with_sample() -> Self {
        let mut s = Self::new();
        s.job.name = "Sample".into();
        s.job.sample_loaded = true;
        s.job.models.push(Model::sample());
        s.job.stock = Some(CosmicCAM_render::scene::sample_stock_box());
        s
    }

    /// Stages file bytes that have no path (a browser upload) and returns the path
    /// `model.import` takes for them: `upload://<name>`.
    pub fn stage_upload(&mut self, name: &str, bytes: Vec<u8>) -> String {
        let name: String = name.chars().filter(|c| !c.is_control()).take(256).collect();
        self.uploads.retain(|(n, _)| *n != name);
        if self.uploads.len() >= MAX_UPLOADS {
            self.uploads.remove(0);
        }
        self.uploads.push((name.clone(), Arc::new(bytes)));
        format!("{UPLOAD_PREFIX}{name}")
    }

    /// Takes a staged upload by its `upload://` path.
    pub(crate) fn take_upload(&mut self, path: &str) -> Option<(String, Arc<Vec<u8>>)> {
        let name = path.strip_prefix(UPLOAD_PREFIX)?;
        let i = self.uploads.iter().position(|(n, _)| n == name)?;
        Some(self.uploads.remove(i))
    }

    pub fn log(&self) -> &[LogEntry] {
        &self.log
    }

    pub(crate) fn log_mut(&mut self) -> &mut Vec<LogEntry> {
        &mut self.log
    }

    /// Runs a command by id. A panic inside the command restores the session and
    /// comes back as [`EngineError::Internal`].
    pub fn execute(&mut self, id: &str, params: &Value) -> EngineResult<Value> {
        let outcome = self.execute_inner(id, params);
        if self.log.len() >= MAX_LOG {
            let excess = self.log.len() + 1 - MAX_LOG;
            self.log.drain(..excess);
        }
        let logged = match &outcome {
            Ok(v) => {
                let size = v.to_string().len();
                Ok(if size > MAX_LOGGED_RESULT_BYTES { json!(format!("<{size} bytes>")) } else { v.clone() })
            }
            Err(e) => Err(e.clone()),
        };
        self.next_seq = self.next_seq.wrapping_add(1);
        self.log.push(LogEntry {
            seq: self.next_seq,
            command: id.chars().take(128).collect(),
            params: params.clone(),
            outcome: logged,
        });
        outcome
    }

    fn execute_inner(&mut self, id: &str, params: &Value) -> EngineResult<Value> {
        let spec = cmd::find(id).ok_or_else(|| EngineError::UnknownCommand(id.chars().take(128).collect()))?;
        if !(spec.enabled)(self) {
            return Err(EngineError::Disabled(spec.id.into()));
        }
        spec.validate(params)?;
        let snapshot = self.clone();
        match catch_unwind(AssertUnwindSafe(|| (spec.run)(self, params))) {
            Ok(result) => {
                if result.is_err() {
                    // Commands validate before they change anything, but never leave half an edit.
                    *self = Session { log: std::mem::take(&mut self.log), next_seq: self.next_seq, ..snapshot };
                }
                result
            }
            Err(payload) => {
                let message = payload
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_owned())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "panic".into());
                *self = Session { log: std::mem::take(&mut self.log), next_seq: self.next_seq, ..snapshot };
                Err(EngineError::Internal(format!("{}: {message}", spec.id)))
            }
        }
    }

    pub fn is_enabled(&self, spec: &CommandSpec) -> bool {
        (spec.enabled)(self)
    }

    /// Reported by the viewport each frame; bad values are ignored.
    pub fn set_viewport_aspect(&mut self, aspect: f32) {
        if aspect.is_finite() && aspect > 1.0e-3 && aspect < 1.0e3 {
            self.view.aspect = aspect;
        }
    }

    pub fn set_scene_bounds(&mut self, bounds: Aabb) {
        self.view.scene_bounds = bounds;
    }

    /// Reported by the UI each frame.
    pub fn set_open_panes(&mut self, panes: Vec<Pane>) {
        self.ui.open_panes = panes;
    }

    /// Window and pane changes queued by commands since the last frame.
    pub fn take_ui_requests(&mut self) -> Vec<UiRequest> {
        std::mem::take(&mut self.ui.requests)
    }

    /// Everything a script needs to know about the session, as returned by `session.state.get`.
    pub fn state_json(&self) -> Value {
        let c = &self.view.camera;
        json!({
            "job": {
                "name": self.job.name,
                "sample_loaded": self.job.sample_loaded,
                "models": self.job.models.iter().map(Model::to_json).collect::<Vec<_>>(),
                "wcs": self.job.wcs.iter().map(|w| w.to_json(self.job.active_wcs.as_deref() == Some(&w.id))).collect::<Vec<_>>(),
                "active_wcs": self.job.active_wcs,
                "stock": self.job.stock.as_ref().map_or(Value::Null, stock_json),
            },
            "theme": self.prefs.theme.id(),
            "units": self.prefs.units.id(),
            "fps_limit": self.prefs.fps_limit,
            "display": self.prefs.display.to_json(),
            "camera": camera_json(c),
            "selection": self.view.selection,
            "scene_bounds": bounds_json(&self.view.scene_bounds),
            "ribbon_tab": self.ui.ribbon_tab.id(),
            "open_panes": self.ui.open_panes.iter().map(|p| p.id()).collect::<Vec<_>>(),
            "outline": self.outline().iter().map(OutlineNode::to_json).collect::<Vec<_>>(),
            "log_seq": self.next_seq,
        })
    }

    /// Starts a new job, keeping preferences, UI state and the command log.
    pub(crate) fn reset_job(&mut self, sample: bool) {
        let mut fresh = if sample { Session::with_sample() } else { Session::new() };
        fresh.prefs = self.prefs.clone();
        fresh.library = self.library.clone();
        fresh.job.revision = self.job.revision.wrapping_add(1);
        fresh.job.setup_revision = self.job.setup_revision.wrapping_add(1);
        fresh.view.layout_generation = self.view.layout_generation;
        fresh.view.aspect = self.view.aspect;
        fresh.view.camera.projection = self.view.camera.projection;
        fresh.ui = self.ui.clone();
        fresh.log = std::mem::take(&mut self.log);
        fresh.next_seq = self.next_seq;
        *self = fresh;
    }

    pub fn log_seq(&self) -> u64 {
        self.next_seq
    }

    /// Job outline for the explorer. Later phases fill in models, setups, tools and toolpaths.
    pub fn outline(&self) -> Vec<OutlineNode> {
        let leaf = |id: &str, label: &str, properties: Vec<(String, String)>| OutlineNode {
            id: id.into(),
            label: label.into(),
            children: Vec::new(),
            properties,
        };
        let mut models = Vec::new();
        let u = self.prefs.units;
        let extents = |b: Aabb| u.fmt_size(b.size().to_array());
        for m in &self.job.models {
            models.push(leaf(
                &m.id,
                &m.name,
                vec![
                    ("Source".into(), m.source.clone()),
                    ("Format".into(), m.format.map_or("Built-in", ModelFormat::label).into()),
                    ("File units".into(), m.units.id().into()),
                    ("Triangles".into(), m.mesh.triangles().len().to_string()),
                    ("Vertices".into(), m.mesh.positions().len().to_string()),
                    ("Faces".into(), face_summary(&m.faces)),
                    ("Edges".into(), m.edges.edges.len().to_string()),
                    ("Extents".into(), extents(m.mesh.bounds())),
                ],
            ));
        }
        let group = |id: &str, label: &str, children: Vec<OutlineNode>| OutlineNode {
            id: id.into(),
            label: label.into(),
            properties: vec![("Items".into(), children.len().to_string())],
            children,
        };
        vec![OutlineNode {
            id: "job".into(),
            label: format!("Job: {}", self.job.name),
            properties: vec![("Name".into(), self.job.name.clone()), ("Units".into(), u.label().into())],
            children: vec![
                group("models", "Models", models),
                group("setups", "Setups", self.setup_nodes()),
                group("tools", "Tools", Vec::new()),
                group("toolpaths", "Toolpaths", self.toolpath_nodes()),
            ],
        }]
    }

    fn toolpath_nodes(&self) -> Vec<OutlineNode> {
        let u = self.prefs.units;
        let rapid = self.library.active_machine().map_or(10_000.0, |m| m.max_feed.max(10_000.0));
        self.job
            .toolpaths
            .iter()
            .map(|t| {
                let (cut, rapid_len, time) = t.stats(rapid);
                let mut properties = vec![
                    ("Strategy".into(), "2D contour".into()),
                    ("Tool".into(), format!("T{}", t.tool)),
                    ("WCS".into(), t.wcs.clone().unwrap_or_else(|| "world".into())),
                    ("Moves".into(), t.moves.len().to_string()),
                    ("Cutting length".into(), u.fmt_len(cut)),
                    ("Rapid length".into(), u.fmt_len(rapid_len)),
                    ("Estimated time".into(), format!("{}:{:02}", (time / 60.0).floor(), (time % 60.0).round() as u32)),
                ];
                properties.extend(t.warnings.iter().map(|w| ("Warning".into(), w.clone())));
                OutlineNode { id: t.id.clone(), label: t.name.clone(), children: Vec::new(), properties }
            })
            .collect()
    }

    /// The stock, then each WCS.
    fn setup_nodes(&self) -> Vec<OutlineNode> {
        let u = self.prefs.units;
        let stock = self.job.stock.map(|b| {
            let size = b.size();
            OutlineNode {
                id: STOCK_ID.into(),
                label: "Stock".into(),
                children: Vec::new(),
                properties: vec![
                    ("Shape".into(), "Box".into()),
                    ("Size".into(), u.fmt_size(size.to_array())),
                    ("Min".into(), u.fmt_point(b.min.to_array())),
                    ("Max".into(), u.fmt_point(b.max.to_array())),
                ],
            }
        });
        stock.into_iter().chain(self.wcs_nodes()).collect()
    }

    fn wcs_nodes(&self) -> Vec<OutlineNode> {
        let u = self.prefs.units;
        let v = |p: Vec3| format!("{:.3}, {:.3}, {:.3}", p.x, p.y, p.z);
        self.job
            .wcs
            .iter()
            .map(|w| {
                let active = self.job.active_wcs.as_deref() == Some(&w.id);
                OutlineNode {
                    id: w.id.clone(),
                    label: if active { format!("{} (active)", w.name) } else { w.name.clone() },
                    children: Vec::new(),
                    properties: vec![
                        ("Name".into(), w.name.clone()),
                        ("Active".into(), if active { "Yes" } else { "No" }.into()),
                        ("Origin".into(), u.fmt_point(w.origin.to_array())),
                        ("X axis".into(), v(w.x_axis)),
                        ("Y axis".into(), v(w.y_axis())),
                        ("Z axis".into(), v(w.z_axis)),
                    ],
                }
            })
            .collect()
    }

    /// The selected item as the inspector shows it: an outline node, or a face, edge or
    /// vertex of a model.
    pub fn selection_node(&self, id: &str) -> Option<OutlineNode> {
        let u = self.prefs.units;
        if let Some(node) = self.outline_node(id) {
            return Some(node);
        }
        if let Some(node) = self.tool_node(id) {
            return Some(node);
        }
        let (m, entity) = self.job.entity(id)?;
        let v = |p: Vec3| format!("{:.3}, {:.3}, {:.3}", p.x, p.y, p.z);
        let (label, properties): (String, Vec<(String, String)>) = match entity {
            Entity::Face(f) => {
                let face = m.faces.faces.get(f)?;
                let mut props = vec![
                    ("Model".into(), m.name.clone()),
                    ("Kind".into(), face.kind.id().into()),
                    ("Area".into(), u.fmt_area(f64::from(face.area))),
                    ("Centre".into(), u.fmt_point(face.centroid.to_array())),
                ];
                match face.kind {
                    FaceKind::Plane { normal, .. } => props.push(("Normal".into(), v(normal))),
                    FaceKind::Cylinder { base, axis, radius, length, hole } => props.extend([
                        ("Type".into(), if hole { "Hole" } else { "Boss" }.into()),
                        ("Diameter".into(), u.fmt_len(f64::from(radius) * 2.0)),
                        ("Length".into(), u.fmt_len(f64::from(length))),
                        ("Axis".into(), v(axis)),
                        ("Axis base".into(), u.fmt_point(base.to_array())),
                    ]),
                    FaceKind::Freeform => {}
                }
                (format!("Face {f} ({})", face.kind.id()), props)
            }
            Entity::Edge(e) => {
                let edge = m.edges.edges.get(e)?;
                let mut props = vec![
                    ("Model".into(), m.name.clone()),
                    ("Kind".into(), edge.kind.id().into()),
                    ("Length".into(), u.fmt_len(f64::from(edge.length))),
                ];
                match edge.kind {
                    EdgeKind::Line { start, end } => props.extend([
                        ("Start".into(), u.fmt_point(start.to_array())),
                        ("End".into(), u.fmt_point(end.to_array())),
                        ("Direction".into(), v(edge.kind.direction())),
                    ]),
                    EdgeKind::Circle { center, normal, radius } => props.extend([
                        ("Centre".into(), u.fmt_point(center.to_array())),
                        ("Diameter".into(), u.fmt_len(f64::from(radius) * 2.0)),
                        ("Normal".into(), v(normal)),
                    ]),
                }
                (format!("Edge {e} ({})", edge.kind.id()), props)
            }
            Entity::Vertex(i) => {
                let p = m.edges.vertices.get(i)?;
                (
                    format!("Vertex {i}"),
                    vec![("Model".into(), m.name.clone()), ("Position".into(), u.fmt_point(p.to_array()))],
                )
            }
        };
        Some(OutlineNode { id: id.to_owned(), label, children: Vec::new(), properties })
    }

    /// The machine and tool a `<machine id>/tool.<n>` reference names.
    pub fn tool_by_ref(&self, reference: &str) -> Option<(&crate::library::Machine, &crate::library::Tool)> {
        let (machine, number) = reference.rsplit_once("/tool.")?;
        let m = self.library.machine(machine)?;
        let t = m.tool(number.parse().ok()?)?;
        Some((m, t))
    }

    /// A tool as the inspector shows it.
    fn tool_node(&self, id: &str) -> Option<OutlineNode> {
        let (m, t) = self.tool_by_ref(id)?;
        let u = self.prefs.units;
        let mm = |v: f64| u.fmt_len(v);
        let not_set = |v: f64, shown: String| if v > 0.0 { shown } else { "not set".into() };
        Some(OutlineNode {
            id: id.to_owned(),
            label: format!("T{} {}", t.number, t.name),
            children: Vec::new(),
            properties: vec![
                ("Machine".into(), m.name.clone()),
                ("Type".into(), t.kind.clone()),
                ("Diameter".into(), mm(t.diameter)),
                ("Flutes".into(), t.flutes.to_string()),
                ("Flute length".into(), mm(t.flute_length)),
                ("Overall length".into(), mm(t.overall_length)),
                ("Shank diameter".into(), mm(t.shank_diameter)),
                ("Material".into(), t.material.clone()),
                ("Spindle speed".into(), not_set(t.spindle_rpm, format!("{} rpm", t.spindle_rpm))),
                ("Feed".into(), not_set(t.feed, u.fmt_rate(t.feed))),
                ("Plunge feed".into(), not_set(t.plunge_feed, u.fmt_rate(t.plunge_feed))),
                ("Stepover".into(), not_set(t.stepover, u.fmt_len(t.stepover))),
                ("Stepdown".into(), not_set(t.stepdown, u.fmt_len(t.stepdown))),
                ("Offsets".into(), format!("H{} D{}", t.length_offset, t.diameter_offset)),
            ],
        })
    }

    /// The machine library as the app saves it.
    pub fn library_json(&self) -> Value {
        self.library.to_json()
    }

    /// Replaces the machine library with a saved one. Returns what was dropped as invalid.
    pub fn load_library(&mut self, saved: &Value) -> Result<Vec<String>, String> {
        let (mut library, dropped) = crate::library::MachineLibrary::from_json(saved)?;
        library.revision = self.library.revision;
        self.library = library;
        Ok(dropped)
    }

    /// Finds an outline node by id.
    pub fn outline_node(&self, id: &str) -> Option<OutlineNode> {
        fn walk(nodes: Vec<OutlineNode>, id: &str) -> Option<OutlineNode> {
            for n in nodes {
                if n.id == id {
                    return Some(n);
                }
                if let Some(found) = walk(n.children, id) {
                    return Some(found);
                }
            }
            None
        }
        walk(self.outline(), id)
    }
}

/// "12 (9 planes, 2 cylinders, 1 freeform)".
fn face_summary(map: &FaceMap) -> String {
    let count = |kind: &str| map.faces.iter().filter(|f| f.kind.id() == kind).count();
    format!(
        "{} ({} planes, {} cylinders, {} freeform)",
        map.faces.len(),
        count("plane"),
        count("cylinder"),
        count("freeform")
    )
}
