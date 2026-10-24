//! The session: all engine state, changed only through [`Session::execute`].

use std::panic::{AssertUnwindSafe, catch_unwind};

use CosmicCAM_geom::Aabb;
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
    /// Upper bound on viewport frames per second while animating.
    pub fps_limit: u32,
    pub display: Display,
}

impl Default for Preferences {
    fn default() -> Self {
        Self { theme: Theme::Dark, fps_limit: DEFAULT_FPS_LIMIT, display: Display::default() }
    }
}

/// Workspace panes the UI can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    Viewport,
    Explorer,
    Inspector,
    Console,
}

impl Pane {
    pub const ALL: [Self; 4] = [Self::Viewport, Self::Explorer, Self::Inspector, Self::Console];
    pub const IDS: &'static [&'static str] = &["viewport", "explorer", "inspector", "console"];

    pub fn id(self) -> &'static str {
        match self {
            Self::Viewport => "viewport",
            Self::Explorer => "explorer",
            Self::Inspector => "inspector",
            Self::Console => "console",
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

#[derive(Clone, Debug)]
pub struct JobState {
    pub name: String,
    /// The Phase 1 placeholder workpiece is shown.
    pub sample_loaded: bool,
}

impl Default for JobState {
    fn default() -> Self {
        Self { name: "Untitled".into(), sample_loaded: false }
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

#[derive(Clone, Debug, Default)]
pub struct Session {
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
        s
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
            "job": { "name": self.job.name, "sample_loaded": self.job.sample_loaded },
            "theme": self.prefs.theme.id(),
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
        if self.job.sample_loaded {
            let size = self.view.scene_bounds.size();
            models.push(leaf(
                "model.sample",
                "Sample part",
                vec![
                    ("Source".into(), "Built-in placeholder".into()),
                    ("Extents".into(), format!("{:.1} × {:.1} × {:.1} mm", size.x, size.y, size.z)),
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
            properties: vec![("Name".into(), self.job.name.clone()), ("Units".into(), "Millimetres".into())],
            children: vec![
                group("models", "Models", models),
                group("setups", "Setups", Vec::new()),
                group("tools", "Tools", Vec::new()),
                group("toolpaths", "Toolpaths", Vec::new()),
            ],
        }]
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
