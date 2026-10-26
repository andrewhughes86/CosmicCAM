//! The toolpath editor: a panel docked at the right of the window while a toolpath is being
//! created or edited. Its tabs (Tool, Multi-axis, Geometry, Heights, Passes, Linking) set
//! the parameters of the strategy's command (`toolpath.contour`, `toolpath.pocket`, …); the
//! result is previewed in the viewport as settings change, and OK runs the command.

use std::collections::HashMap;
use std::sync::Arc;

use CosmicCAM_engine::Session;
use CosmicCAM_engine::session::Entity;
use CosmicCAM_engine::toolpath::Move;
use CosmicCAM_engine::toolpath::contour::{self, COOLANTS, HEIGHTS, LEADS, RETRACTS};
use CosmicCAM_engine::units::DisplayUnits;
use CosmicCAM_geom::Vec3;
use egui::{RichText, Ui};
use serde_json::{Map, Value, json};

use crate::Request;
use crate::theme::Tokens;
use crate::ui_kit::{Panel, Surface};
use crate::viewport::{Accept, PickHit, PickMode};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Tool,
    MultiAxis,
    Geometry,
    Heights,
    Passes,
    Linking,
    Post,
}

const TABS: &[(Tab, &str)] = &[
    (Tab::Tool, "Tool"),
    (Tab::MultiAxis, "Multi-axis"),
    (Tab::Geometry, "Geometry"),
    (Tab::Heights, "Heights"),
    (Tab::Passes, "Passes"),
    (Tab::Linking, "Linking"),
    (Tab::Post, "Post"),
];

/// The strategy a panel edits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Strategy {
    Contour,
    Face,
    Pocket,
    Drill,
    Rough,
    Waterline,
    Raster,
    MultiAxis,
    Bore,
    Adaptive,
    Scallop,
    SteepShallow,
    Pencil,
    Spiral,
    Radial,
    Flat,
    Chamfer,
    Slot,
    Trace,
    Thread,
}

impl Strategy {
    /// The strategy a command starts, if it is a toolpath strategy.
    pub fn from_command(id: &str) -> Option<Self> {
        Some(match id {
            "toolpath.contour" => Self::Contour,
            "toolpath.face" => Self::Face,
            "toolpath.pocket" => Self::Pocket,
            "toolpath.drill" => Self::Drill,
            "toolpath.rough" => Self::Rough,
            "toolpath.waterline" => Self::Waterline,
            "toolpath.raster" => Self::Raster,
            "toolpath.multiaxis" => Self::MultiAxis,
            "toolpath.bore" => Self::Bore,
            "toolpath.adaptive" => Self::Adaptive,
            "toolpath.scallop" => Self::Scallop,
            "toolpath.steep_shallow" => Self::SteepShallow,
            "toolpath.pencil" => Self::Pencil,
            "toolpath.spiral" => Self::Spiral,
            "toolpath.radial" => Self::Radial,
            "toolpath.flat" => Self::Flat,
            "toolpath.chamfer" => Self::Chamfer,
            "toolpath.slot" => Self::Slot,
            "toolpath.trace" => Self::Trace,
            "toolpath.thread" => Self::Thread,
            _ => return None,
        })
    }

    /// The strategy a stored toolpath was made with.
    fn from_toolpath(strategy: &str) -> Self {
        match strategy {
            "face" => Self::Face,
            "pocket2d" => Self::Pocket,
            "drill" => Self::Drill,
            "rough3d" => Self::Rough,
            "waterline" => Self::Waterline,
            "raster" => Self::Raster,
            "multiaxis" => Self::MultiAxis,
            "bore" => Self::Bore,
            "adaptive" => Self::Adaptive,
            "scallop" => Self::Scallop,
            "steep_shallow" => Self::SteepShallow,
            "pencil" => Self::Pencil,
            "spiral" => Self::Spiral,
            "radial" => Self::Radial,
            "flat" => Self::Flat,
            "chamfer" => Self::Chamfer,
            "slot" => Self::Slot,
            "trace" => Self::Trace,
            "thread" => Self::Thread,
            _ => Self::Contour,
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::Contour => "toolpath.contour",
            Self::Face => "toolpath.face",
            Self::Pocket => "toolpath.pocket",
            Self::Drill => "toolpath.drill",
            Self::Rough => "toolpath.rough",
            Self::Waterline => "toolpath.waterline",
            Self::Raster => "toolpath.raster",
            Self::MultiAxis => "toolpath.multiaxis",
            Self::Bore => "toolpath.bore",
            Self::Adaptive => "toolpath.adaptive",
            Self::Scallop => "toolpath.scallop",
            Self::SteepShallow => "toolpath.steep_shallow",
            Self::Pencil => "toolpath.pencil",
            Self::Spiral => "toolpath.spiral",
            Self::Radial => "toolpath.radial",
            Self::Flat => "toolpath.flat",
            Self::Chamfer => "toolpath.chamfer",
            Self::Slot => "toolpath.slot",
            Self::Trace => "toolpath.trace",
            Self::Thread => "toolpath.thread",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Contour => "2D contour",
            Self::Face => "face",
            Self::Pocket => "pocket",
            Self::Drill => "drilling",
            Self::Rough => "3D rough",
            Self::Waterline => "waterline",
            Self::Raster => "raster",
            Self::MultiAxis => "multi-axis finish",
            Self::Bore => "helical bore",
            Self::Adaptive => "adaptive clearing",
            Self::Scallop => "scallop finish",
            Self::SteepShallow => "steep and shallow finish",
            Self::Pencil => "pencil finish",
            Self::Spiral => "spiral finish",
            Self::Radial => "radial finish",
            Self::Flat => "flat finish",
            Self::Chamfer => "chamfer",
            Self::Slot => "slot",
            Self::Trace => "trace",
            Self::Thread => "thread milling",
        }
    }

    /// The engine's strategy id, for its height defaults.
    fn engine_id(self) -> &'static str {
        use CosmicCAM_engine::toolpath::finish as fin;
        use CosmicCAM_engine::toolpath::strategies as st;
        use CosmicCAM_engine::toolpath::twod as tw;
        match self {
            Self::Contour => "contour2d",
            Self::Face => st::FACE,
            Self::Pocket => st::POCKET,
            Self::Drill => CosmicCAM_engine::toolpath::drill::DRILL,
            Self::Rough => st::ROUGH,
            Self::Waterline => st::WATERLINE,
            Self::Raster => st::RASTER,
            Self::MultiAxis => CosmicCAM_engine::toolpath::multiaxis::MULTIAXIS,
            Self::Bore => CosmicCAM_engine::toolpath::bore::BORE,
            Self::Adaptive => CosmicCAM_engine::toolpath::adaptive::ADAPTIVE,
            Self::Scallop => fin::SCALLOP,
            Self::SteepShallow => fin::STEEP_SHALLOW,
            Self::Pencil => fin::PENCIL,
            Self::Spiral => fin::SPIRAL,
            Self::Radial => fin::RADIAL,
            Self::Flat => fin::FLAT,
            Self::Chamfer => tw::CHAMFER.strategy,
            Self::Slot => tw::SLOT.strategy,
            Self::Trace => tw::TRACE.strategy,
            Self::Thread => tw::THREAD,
        }
    }

    fn tabs(self) -> Vec<(Tab, &'static str)> {
        TABS.iter()
            .copied()
            .filter(|(t, _)| match self {
                Self::Drill | Self::MultiAxis | Self::Bore | Self::Thread => *t != Tab::Linking,
                _ => true,
            })
            .collect()
    }

    /// The contour and the strategies built on it share its tabs.
    fn contour_family(self) -> bool {
        matches!(self, Self::Contour | Self::Chamfer | Self::Slot | Self::Trace)
    }

    /// The finishing strategies that drop the tool onto the model over an area.
    fn surface_finish(self) -> bool {
        matches!(self, Self::Scallop | Self::SteepShallow | Self::Pencil | Self::Spiral | Self::Radial)
    }

    /// Whether the strategy needs geometry picked before it can run.
    fn needs_geometry(self) -> bool {
        matches!(
            self,
            Self::Contour
                | Self::Pocket
                | Self::Drill
                | Self::Bore
                | Self::Chamfer
                | Self::Slot
                | Self::Trace
                | Self::Thread
        )
    }

    /// The tool types the strategy takes, best first.
    fn tool_kinds(self) -> &'static [&'static str] {
        use CosmicCAM_engine::tooling as tl;
        match self {
            Self::Contour | Self::Bore => &["flat", "bull", "ball"],
            Self::Face => &["face_mill", "flat", "bull"],
            Self::Pocket | Self::Rough | Self::Waterline | Self::Adaptive => &["flat", "bull"],
            Self::Drill => tl::HOLEMAKING,
            Self::Raster => &["ball", "flat", "tapered_ball", "lollipop"],
            Self::MultiAxis | Self::Pencil => tl::BALL_ENDED,
            Self::Scallop | Self::SteepShallow | Self::Spiral | Self::Radial => {
                CosmicCAM_engine::toolpath::finish::SURFACE_TOOLS
            }
            Self::Flat => &["flat", "bull", "face_mill"],
            Self::Chamfer => CosmicCAM_engine::toolpath::twod::CHAMFER.kinds,
            Self::Slot => CosmicCAM_engine::toolpath::twod::SLOT.kinds,
            Self::Trace => CosmicCAM_engine::toolpath::twod::TRACE.kinds,
            Self::Thread => &["thread_mill"],
        }
    }

    /// (prefix, reference, offset mm) for each height the strategy uses.
    fn heights(self) -> Vec<(&'static str, &'static str, f64)> {
        use CosmicCAM_engine::toolpath::strategies as st;
        match self {
            Self::Contour | Self::Chamfer | Self::Slot | Self::Trace => HEIGHTS.to_vec(),
            Self::Thread => CosmicCAM_engine::toolpath::twod::THREAD_HEIGHTS.to_vec(),
            Self::Face => st::FACE_HEIGHTS.to_vec(),
            Self::Pocket => st::POCKET_HEIGHTS.to_vec(),
            Self::Drill => CosmicCAM_engine::toolpath::drill::DRILL_HEIGHTS.to_vec(),
            Self::Bore => CosmicCAM_engine::toolpath::bore::BORE_HEIGHTS.to_vec(),
            Self::Rough | Self::Adaptive => st::ROUGH_HEIGHTS.to_vec(),
            Self::Waterline
            | Self::Raster
            | Self::MultiAxis
            | Self::Scallop
            | Self::SteepShallow
            | Self::Pencil
            | Self::Spiral
            | Self::Radial
            | Self::Flat => st::FINISH_HEIGHTS.to_vec(),
        }
    }
}

/// Labels for the height references, in `contour::HEIGHT_REFS` order.
const REF_LABELS: &[(&str, &str)] = &[
    ("selection", "Selected contour(s)"),
    ("model_top", "Model top"),
    ("model_bottom", "Model bottom"),
    ("stock_top", "Stock top"),
    ("stock_bottom", "Stock bottom"),
    ("origin", "Origin"),
];

/// The heights as the Heights tab shows them: (parameter prefix, label, icon, help).
const HEIGHT_ROWS: &[(&str, &str, &str, &str)] = &[
    (
        "clearance",
        "Clearance height",
        "toolpath_height_clearance",
        "Safe height for moving between toolpaths and contours",
    ),
    ("retract", "Retract height", "toolpath_height_retract", "The tool rises to this height between passes"),
    ("feed_height", "Feed height", "toolpath_height_feed", "Rapid moves stop here and the tool feeds down from here"),
    ("top", "Top height", "toolpath_height_top", "Where the material starts: the first depth is below this"),
    ("bottom", "Bottom height", "toolpath_height_bottom", "The final depth of the contour"),
];

/// A preview: the toolpath's start, moves and summary, or why it cannot be made.
type Preview = Result<(Vec3, Arc<Vec<Move>>, Value), String>;

/// Runs `command` on `trial` (a copy of the session) and takes the toolpath it makes.
fn compute(mut trial: Session, command: &str, p: &Value) -> Preview {
    let summary = trial.execute(command, p).map_err(|e| e.to_string())?;
    let id = summary["id"].as_str().unwrap_or_default().to_owned();
    trial
        .job
        .toolpaths
        .iter()
        .find(|t| t.id == id)
        .map(|t| (t.start, Arc::clone(&t.moves), summary.clone()))
        .ok_or_else(|| "no toolpath".to_owned())
}

pub struct ToolpathPanel {
    strategy: Strategy,
    /// The toolpath being edited, or None for a new one.
    editing: Option<String>,
    tab: Tab,
    /// Parameters set so far, in millimetres; anything not here uses the command's default.
    values: Map<String, Value>,
    /// Text being typed into number fields, by parameter.
    text: HashMap<String, String>,
    /// Selected edges and faces.
    geometry: Vec<String>,
    /// The toolpath the settings make, or why they cannot.
    preview: Preview,
    /// A preview being worked out in the background for the latest settings.
    pending: Option<std::sync::mpsc::Receiver<Preview>>,
    /// Heights as WCS Z values, for display next to each row.
    heights: Vec<(String, Option<f64>)>,
    dirty: bool,
    error: Option<String>,
    /// The next viewport click sets the entry point.
    picking_entry: bool,
}

impl ToolpathPanel {
    /// A new toolpath of `strategy`, or `editing` an existing one with its stored settings
    /// (its own strategy then).
    pub fn new(session: &Session, editing: Option<&str>, strategy: Strategy) -> Self {
        let mut values = Map::new();
        let mut geometry = Vec::new();
        let mut strategy = strategy;
        if let Some(tp) = editing.and_then(|id| session.job.toolpaths.iter().find(|t| t.id == id)) {
            strategy = Strategy::from_toolpath(&tp.strategy);
            if let Value::Object(params) = &tp.params {
                values = params.clone();
            }
            geometry = values
                .remove("geometry")
                .and_then(|g| g.as_array().cloned())
                .map(|a| a.into_iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
        } else {
            // A selected edge or face starts the selection.
            if let Some(sel) = session.view.selection.as_deref()
                && session
                    .job
                    .entity(sel)
                    .is_some_and(|(_, e)| strategy == Strategy::Drill || !matches!(e, Entity::Vertex(_)))
            {
                geometry.push(sel.to_owned());
            }
        }
        let mut panel = Self {
            strategy,
            editing: editing.map(str::to_owned),
            tab: if geometry.is_empty() && strategy.needs_geometry() { Tab::Geometry } else { Tab::Tool },
            values,
            text: HashMap::new(),
            geometry,
            preview: Err(String::new()),
            pending: None,
            heights: Vec::new(),
            dirty: true,
            error: None,
            picking_entry: false,
        };
        panel.refresh(session);
        panel
    }

    pub fn fail(&mut self, message: String) {
        self.error = Some(message);
    }

    fn params(&self) -> Value {
        let mut p = self.values.clone();
        p.insert("geometry".into(), json!(self.geometry));
        if let Some(id) = &self.editing {
            p.insert("id".into(), json!(id));
        }
        Value::Object(p)
    }

    /// Recomputes the preview by running the command on a copy of the session: in the
    /// background where threads exist, so slow strategies do not hold up the window.
    fn refresh(&mut self, session: &Session) {
        self.dirty = false;
        self.error = None;
        // Regenerating in place (with the id) keeps rest machining to the toolpaths before it.
        let p = self.params();
        self.heights = match self.strategy {
            s if s.contour_family() => contour::preview_heights(session, &p),
            other => CosmicCAM_engine::toolpath::strategies::preview_heights(other.engine_id(), session, &p),
        };
        let trial = session.clone();
        let command = self.strategy.command();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new().name("toolpath-preview".into()).spawn(move || {
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

    /// What the viewport picks and draws while the panel is open.
    pub fn mode(&self, session: &Session) -> PickMode {
        let entry = self.entry_point().filter(|_| self.tab == Tab::Linking && self.strategy == Strategy::Contour);
        let mut mode = PickMode {
            accept: if self.picking_entry {
                Some(Accept::Point)
            } else {
                (self.tab == Tab::Geometry && self.takes_geometry()).then_some(Accept::Axis)
            },
            picked: Vec::new(),
            picked_edges: Vec::new(),
            picked_points: entry.into_iter().collect(),
            preview: None,
            independent_of: Vec::new(),
            path: self.preview.as_ref().ok().map(|(start, moves, _)| (*start, Arc::clone(moves))),
        };
        for r in &self.geometry {
            match session.job.entity(r) {
                Some((m, Entity::Face(f))) => mode.picked.push((m.id.clone(), f)),
                Some((m, Entity::Edge(e))) => mode.picked_edges.push((m.id.clone(), e)),
                _ => {}
            }
        }
        mode
    }

    /// A click in the viewport (Geometry tab) adds the edge or face, or removes it if picked.
    pub fn picked(&mut self, hit: PickHit, session: &Session) {
        if self.picking_entry {
            self.picking_entry = false;
            self.values.insert("entry_from".into(), json!("selection"));
            self.values.insert("entry_point".into(), json!(hit.point.to_array()));
            self.refresh(session);
            return;
        }
        if !self.takes_geometry() {
            return;
        }
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
        self.refresh(session);
    }

    /// Draws the panel. Returns the command to run on OK, and whether the panel stays open.
    pub fn show(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens) -> (Option<Request>, bool) {
        if self.dirty {
            self.refresh(session);
        }
        let working = self.poll();
        if working {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        }
        let mut result = (None, true);
        let units = session.prefs.units;
        Panel::right("toolpath-panel", Surface::SidePanel, 360.0).show(ui, tokens, |ui| {
            let title = format!("{} {}", if self.editing.is_some() { "Edit" } else { "New" }, self.strategy.label());
            ui.label(RichText::new(title).heading());
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for (tab, label) in self.strategy.tabs() {
                    ui.selectable_value(&mut self.tab, tab, label);
                }
            });
            ui.separator();
            // The settings scroll; the summary and buttons stay at the bottom.
            let footer = 130.0;
            // Fill the panel so the buttons sit at its bottom.
            let body = (ui.available_height() - footer).max(120.0);
            egui::ScrollArea::vertical().max_height(body).min_scrolled_height(body).auto_shrink([false, false]).show(
                ui,
                |ui| {
                    let changed = match self.tab {
                        Tab::Tool => self.tool_tab(ui, session, tokens, units),
                        Tab::MultiAxis => self.multi_axis_tab(ui, session, tokens, units),
                        Tab::Geometry if !self.strategy.contour_family() => {
                            self.strategy_geometry_tab(ui, session, tokens, units)
                        }
                        Tab::Geometry => self.geometry_tab(ui, session, tokens),
                        Tab::Heights => self.heights_tab(ui, session, tokens, units),
                        Tab::Passes if !self.strategy.contour_family() => self.strategy_passes_tab(ui, units),
                        Tab::Passes => self.passes_tab(ui, units),
                        Tab::Linking if !self.strategy.contour_family() => self.strategy_linking_tab(ui, units),
                        Tab::Linking => self.linking_tab(ui, units),
                        Tab::Post => self.post_tab(ui, tokens),
                    };
                    self.dirty |= changed;
                },
            );
            ui.separator();
            // The summary and messages sit centred over the buttons.
            ui.vertical_centered(|ui| match (&self.error, &self.preview) {
                _ if working => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label("Working out the toolpath…");
                    });
                }
                (Some(e), _) => {
                    ui.label(RichText::new(e).color(tokens.error));
                }
                (None, Ok((_, moves, summary))) => {
                    let time = summary["time"].as_f64().unwrap_or(0.0);
                    ui.label(format!(
                        "{} moves · cutting {} · about {}:{:02}",
                        moves.len(),
                        units.fmt_len(summary["cut_length"].as_f64().unwrap_or(0.0)),
                        (time / 60.0).floor(),
                        (time % 60.0).round() as u32
                    ));
                    for w in summary["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                        ui.label(RichText::new(w).small().color(tokens.warning));
                    }
                }
                (None, Err(e)) if !e.is_empty() => {
                    ui.label(RichText::new(e).color(tokens.warning));
                }
                _ => {}
            });
            ui.add_space(8.0);
            let (ok, cancel) = ok_cancel(ui, self.preview.is_ok() && !working);
            if ok {
                result.0 = Some((self.strategy.command().to_owned(), self.params()));
            }
            if cancel {
                result.1 = false;
            }
        });
        result
    }

    /// The picked entry point, when the entry is set from a selection.
    fn entry_point(&self) -> Option<Vec3> {
        if self.values.get("entry_from").and_then(Value::as_str) != Some("selection") {
            return None;
        }
        let a = self.values.get("entry_point")?.as_array()?;
        let c: Vec<f32> = a.iter().filter_map(Value::as_f64).map(|v| v as f32).collect();
        match c.as_slice() {
            [x, y, z] => Some(Vec3::new(*x, *y, *z)),
            _ => None,
        }
    }

    // ---- field helpers -------------------------------------------------------------------

    /// A number typed in display units (lengths, feeds) or as is; empty means the default.
    /// `hint` shows the default. True when the value changed.
    fn number(&mut self, ui: &mut Ui, key: &str, hint: &str, scale: Option<DisplayUnits>, rate: bool) -> bool {
        let shown = self.values.get(key).and_then(Value::as_f64).map(|v| match scale {
            Some(u) if rate => u.plain_rate(v),
            Some(u) => u.plain(v),
            None => trim(v),
        });
        let entry = self.text.entry(key.to_owned()).or_insert_with(|| shown.unwrap_or_default());
        let response = ui.add(egui::TextEdit::singleline(entry).desired_width(72.0).hint_text(hint));
        if !response.changed() {
            return false;
        }
        let text = entry.trim().to_owned();
        if text.is_empty() {
            self.values.remove(key);
            return true;
        }
        match text.parse::<f64>() {
            Ok(v) if v.is_finite() => {
                let mm = scale.map_or(v, |u| u.to_mm(v));
                self.values.insert(key.to_owned(), json!(mm));
                true
            }
            _ => false,
        }
    }

    /// Text for a parameter; empty removes it. True when it changed.
    fn text(&mut self, ui: &mut Ui, key: &str, hint: &str, rows: usize) -> bool {
        let mut value = self.values.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
        let edit = if rows > 1 {
            egui::TextEdit::multiline(&mut value).desired_rows(rows).code_editor()
        } else {
            egui::TextEdit::singleline(&mut value)
        };
        let changed = ui.add(edit.desired_width(f32::INFINITY).hint_text(hint)).changed();
        if changed {
            if value.is_empty() {
                self.values.remove(key);
            } else {
                self.values.insert(key.to_owned(), json!(value));
            }
        }
        changed
    }

    /// What the post writes around this operation: a stop, a note, the user's own G-code.
    fn post_tab(&mut self, ui: &mut Ui, tokens: &Tokens) -> bool {
        let mut changed = false;
        ui.label(
            RichText::new("Written into the program by the post-processor around this operation.")
                .color(tokens.text_muted),
        );
        ui.add_space(6.0);
        changed |= self.check(ui, "post_stop", "Stop the machine before this operation (M0)", false);
        ui.add_space(6.0);
        ui.label("Note for the operator");
        changed |= self.text(ui, "post_comment", "e.g. Turn the part over and clamp on the finished face", 2);
        ui.add_space(6.0);
        ui.label("G-code before the operation");
        changed |= self.text(ui, "post_code_before", "Written as given, one block per line", 3);
        ui.add_space(6.0);
        ui.label("G-code after the operation");
        changed |= self.text(ui, "post_code_after", "Written as given, one block per line", 3);
        changed
    }

    fn check(&mut self, ui: &mut Ui, key: &str, label: &str, default: bool) -> bool {
        let mut on = self.values.get(key).and_then(Value::as_bool).unwrap_or(default);
        if ui.checkbox(&mut on, label).changed() {
            self.values.insert(key.to_owned(), json!(on));
            return true;
        }
        false
    }

    fn choice(&mut self, ui: &mut Ui, key: &str, options: &[(&str, &str)], default: &str) -> bool {
        let current = self.values.get(key).and_then(Value::as_str).unwrap_or(default).to_owned();
        let label = options.iter().find(|(id, _)| *id == current).map_or(current.as_str(), |(_, l)| *l).to_owned();
        let mut picked = None;
        egui::ComboBox::from_id_salt(("tp", key)).selected_text(label).width(150.0).show_ui(ui, |ui| {
            for (id, l) in options {
                if ui.selectable_label(*id == current, *l).clicked() {
                    picked = Some((*id).to_owned());
                }
            }
        });
        match picked {
            Some(id) if id != current => {
                self.values.insert(key.to_owned(), json!(id));
                true
            }
            _ => false,
        }
    }

    /// One labelled row in a two-column grid.
    fn row(ui: &mut Ui, label: &str, help: &str, add: impl FnOnce(&mut Ui) -> bool) -> bool {
        ui.label(label).on_hover_text(help);
        let changed = ui.horizontal(add).inner;
        ui.end_row();
        changed
    }

    /// Whether clicks in the viewport pick geometry for this strategy and its settings.
    fn takes_geometry(&self) -> bool {
        match self.strategy {
            Strategy::Rough | Strategy::Waterline => false,
            Strategy::Face
            | Strategy::Raster
            | Strategy::MultiAxis
            | Strategy::Adaptive
            | Strategy::Scallop
            | Strategy::SteepShallow
            | Strategy::Pencil
            | Strategy::Spiral
            | Strategy::Radial => self.values.get("boundary").and_then(Value::as_str) == Some("selection"),
            _ => true,
        }
    }

    // ---- tabs ----------------------------------------------------------------------------

    /// "Start from" a saved template of this strategy: its settings replace the panel's.
    fn template_row(&mut self, ui: &mut Ui, session: &Session) -> bool {
        let command = self.strategy.command();
        let templates: Vec<&CosmicCAM_engine::library::Template> =
            session.library.templates.iter().filter(|t| t.command == command).collect();
        if templates.is_empty() {
            return false;
        }
        let mut picked: Option<Map<String, Value>> = None;
        ui.horizontal(|ui| {
            ui.label("Start from").on_hover_text(
                "Fill in the settings from a saved template (save one from a toolpath's menu in the explorer)",
            );
            egui::ComboBox::from_id_salt("tp-template").selected_text("Template…").width(200.0).show_ui(ui, |ui| {
                for t in &templates {
                    if ui.selectable_label(false, t.name.as_str()).clicked() {
                        picked = t.params.as_object().cloned();
                    }
                }
            });
        });
        ui.add_space(4.0);
        match picked {
            Some(values) => {
                self.values = values;
                self.text.clear();
                true
            }
            None => false,
        }
    }

    fn tool_tab(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens, u: DisplayUnits) -> bool {
        if self.template_row(ui, session) {
            self.dirty = true;
        }
        let Some(machine) = session.library.active_machine() else {
            ui.label(RichText::new("There is no machine. Add one on the Machine tab.").color(tokens.error));
            return false;
        };
        let kinds = self.strategy.tool_kinds();
        let fitting: Vec<&CosmicCAM_engine::library::Tool> =
            machine.tools.iter().filter(|t| kinds.contains(&t.kind.as_str())).collect();
        if fitting.is_empty() {
            let names: Vec<&str> =
                kinds.iter().filter_map(|k| CosmicCAM_engine::tooling::tool_type(k)).map(|t| t.label).collect();
            ui.label(
                RichText::new(format!(
                    "{} has no tool this strategy takes ({}). Add one on the Tooling tab, or load the sample library.",
                    machine.name,
                    names.join(", ")
                ))
                .color(tokens.error),
            );
            return false;
        }
        let mut changed = false;
        let current =
            self.values.get("tool").and_then(Value::as_u64).map(|n| n as u32).or(fitting.first().map(|t| t.number));
        let tool = current.and_then(|n| machine.tool(n));
        ui.label(RichText::new(format!("Machine: {}", machine.name)).small().color(tokens.text_muted));
        egui::ComboBox::from_id_salt("tp-tool")
            .selected_text(tool.map_or("Choose a tool".to_owned(), |t| format!("T{}  {}", t.number, t.name)))
            .width(300.0)
            .show_ui(ui, |ui| {
                for t in &fitting {
                    let text = format!("T{}  {}  ({})", t.number, t.name, t.type_label().to_lowercase());
                    if ui.selectable_label(Some(t.number) == current, text).clicked() {
                        self.values.insert("tool".into(), json!(t.number));
                        self.values.remove("preset");
                        changed = true;
                    }
                }
            });
        if let Some(t) = tool {
            crate::tool_view::draw(ui, Some(t), session.library.holder_of(t), u, tokens, egui::vec2(300.0, 190.0));
            if !t.presets.is_empty() {
                let mut presets: Vec<(&str, &str)> = vec![("", "The tool's own")];
                presets.extend(t.presets.iter().map(|p| (p.name.as_str(), p.name.as_str())));
                egui::Grid::new("tp-preset").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    changed |= Self::row(ui, "Cutting data", "Speeds and feeds from one of the tool's presets", |ui| {
                        let c = self.choice(ui, "preset", &presets, "");
                        if self.values.get("preset").and_then(Value::as_str) == Some("") {
                            self.values.remove("preset");
                        }
                        c
                    });
                });
            }
            let hint_rate = |v: f64| if v > 0.0 { u.plain_rate(v) } else { "not set".into() };
            ui.label(RichText::new("Feeds and speeds (blank = the tool's)").strong());
            egui::Grid::new("tp-feeds").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                let rpm_hint = if t.spindle_rpm > 0.0 { trim(t.spindle_rpm) } else { "not set".into() };
                changed |= Self::row(ui, "Spindle speed", "rpm", |ui| {
                    let c = self.number(ui, "spindle_rpm", &rpm_hint, None, false);
                    ui.label("rpm");
                    c
                });
                let feeds: &[(&str, &str, f64)] = match self.strategy {
                    s if s.contour_family() => &[
                        ("feed", "Cutting feed", t.feed),
                        ("plunge_feed", "Plunge feed", t.plunge_feed),
                        ("lead_feed", "Lead-in / out feed", t.feed),
                    ],
                    Strategy::Drill => &[("feed", "Drilling feed", t.plunge_feed)],
                    Strategy::Bore => {
                        &[("feed", "Cutting feed", t.feed), ("plunge_feed", "Feed down to the hole", t.plunge_feed)]
                    }
                    Strategy::Thread => &[("feed", "Cutting feed", t.feed)],
                    _ => &[("feed", "Cutting feed", t.feed), ("plunge_feed", "Plunge feed", t.plunge_feed)],
                };
                for &(key, label, fallback) in feeds {
                    let hint = hint_rate(fallback);
                    changed |= Self::row(ui, label, "", |ui| {
                        let c = self.number(ui, key, &hint, Some(u), true);
                        ui.label(u.rate());
                        c
                    });
                }
                let coolants: Vec<(&str, &str)> = COOLANTS
                    .iter()
                    .map(|c| {
                        (
                            *c,
                            match *c {
                                "off" => "Off",
                                "flood" => "Flood",
                                "mist" => "Mist",
                                "air" => "Air blast",
                                _ => "Through tool",
                            },
                        )
                    })
                    .collect();
                changed |= Self::row(ui, "Coolant", "", |ui| self.choice(ui, "coolant", &coolants, "flood"));
            });
        }
        changed
    }

    /// A point or direction typed as three numbers (lengths in display units for points); all
    /// empty removes it. True when it changed.
    fn vector(&mut self, ui: &mut Ui, key: &str, hints: [&str; 3], scale: Option<DisplayUnits>) -> bool {
        let current: Vec<f64> = self
            .values
            .get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        let mut changed = false;
        let mut parts: Vec<Option<f64>> = Vec::with_capacity(3);
        for (k, hint) in hints.iter().enumerate() {
            let slot = format!("{key}.{k}");
            let shown = current.get(k).map(|v| scale.map_or_else(|| trim(*v), |u| u.plain(*v)));
            let entry = self.text.entry(slot).or_insert_with(|| shown.unwrap_or_default());
            changed |= ui.add(egui::TextEdit::singleline(entry).desired_width(48.0).hint_text(*hint)).changed();
            let typed = entry.trim().parse::<f64>().ok().filter(|v| v.is_finite());
            parts.push(typed.map(|v| scale.map_or(v, |u| u.to_mm(v))));
        }
        if !changed {
            return false;
        }
        match parts.as_slice() {
            [Some(x), Some(y), Some(z)] => {
                self.values.insert(key.to_owned(), json!([x, y, z]));
            }
            _ => {
                self.values.remove(key);
            }
        }
        true
    }

    /// Tool orientation: along the WCS's Z, or tilted to another WCS (3+2); for the multi-axis
    /// finish, how the tool leans as it goes.
    fn multi_axis_tab(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens, u: DisplayUnits) -> bool {
        let mut changed = false;
        let rotary = session.library.active_machine().and_then(|m| m.kinematics.as_ref()).map(|k| k.names().join(""));
        match &rotary {
            Some(axes) => {
                ui.label(
                    RichText::new(format!("The machine turns the tool with its {axes} axes.")).color(tokens.text_muted),
                );
            }
            None => {
                ui.label(
                    RichText::new("The active machine is a 3-axis mill: tilting the tool needs a 4- or 5-axis machine (Machine tab).")
                        .color(tokens.warning),
                );
            }
        }
        ui.add_space(6.0);
        let mut planes: Vec<(String, String)> = vec![(String::new(), "Along the WCS Z axis".into())];
        planes.extend(session.job.wcs.iter().map(|w| (w.id.clone(), format!("Along {}'s Z (3+2)", w.name))));
        let options: Vec<(&str, &str)> = planes.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        egui::Grid::new("tp-orientation").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            changed |= Self::row(
                ui,
                "Tool orientation",
                "3+2: work along another WCS's Z axis, programmed in this toolpath's WCS; the machine tilts the part or head to it",
                |ui| {
                    let c = self.choice(ui, "orientation", &options, "");
                    if self.values.get("orientation").and_then(Value::as_str) == Some("") {
                        self.values.remove("orientation");
                    }
                    c
                },
            );
        });
        if self.strategy != Strategy::MultiAxis {
            return changed;
        }
        ui.add_space(8.0);
        ui.label(RichText::new("Tool axis").strong());
        let rules = [
            ("tilt", "Tilted from Z"),
            ("normal", "Surface normal"),
            ("toward_point", "Towards a point"),
            ("away_from_point", "Away from a point"),
            ("toward_line", "Towards a line"),
            ("away_from_line", "Away from a line"),
            ("fixed", "Fixed direction"),
        ];
        let rule = self.values.get("tool_axis").and_then(Value::as_str).unwrap_or("tilt").to_owned();
        let deg = |ui: &mut Ui, me: &mut Self, key: &str, hint: &str| {
            let c = me.number(ui, key, hint, None, false);
            ui.label("°");
            c
        };
        egui::Grid::new("tp-tool-axis").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            changed |= Self::row(ui, "Lean", "How the tool axis is chosen at each point", |ui| {
                self.choice(ui, "tool_axis", &rules, "tilt")
            });
            if matches!(rule.as_str(), "tilt" | "normal") {
                let tilt_hint = if rule == "tilt" { "15" } else { "0" };
                changed |= Self::row(
                    ui,
                    "Side tilt",
                    "Positive leans the tool's top to the right of the row direction",
                    |ui| deg(ui, self, "tilt_angle", tilt_hint),
                );
                changed |= Self::row(ui, "Lead", "Positive leans the tool's top forward along the travel", |ui| {
                    deg(ui, self, "lead_angle", "0")
                });
            }
            if matches!(rule.as_str(), "toward_point" | "away_from_point" | "toward_line" | "away_from_line") {
                changed |= Self::row(ui, "Point", "In the WCS", |ui| {
                    let c = self.vector(ui, "axis_point", ["X", "Y", "Z"], Some(u));
                    ui.label(u.length());
                    c
                });
            }
            if matches!(rule.as_str(), "toward_line" | "away_from_line" | "fixed") {
                let label = if rule == "fixed" { "Tool axis" } else { "Line direction" };
                changed |= Self::row(ui, label, "A direction in the WCS", |ui| {
                    self.vector(ui, "axis_direction", ["I", "J", "K"], None)
                });
            }
            changed |= Self::row(ui, "Most tilt", "The tool never leans more than this from the toolpath's Z", |ui| {
                deg(ui, self, "max_tilt", "60")
            });
            changed |= Self::row(ui, "Axis rate", "Fastest the tool axis turns, degrees per mm of travel", |ui| {
                let c = self.number(ui, "max_axis_rate", "5", None, false);
                ui.label("°/mm");
                c
            });
            changed |= Self::row(ui, "", "", |ui| {
                self.check(ui, "avoid_collisions", "Lean back towards Z where the flutes or shank would touch", true)
            });
            changed |= Self::row(ui, "Shank clearance", "Gap kept around the shank", |ui| {
                let c = self.number(ui, "shank_clearance", &u.plain(0.5), Some(u), false);
                ui.label(u.length());
                c
            });
        });
        changed
    }

    fn geometry_tab(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens) -> bool {
        ui.label(
            RichText::new(
                "Click edges in the viewport to follow them, or a face to follow its outline. Click again to remove.",
            )
            .color(tokens.accent),
        );
        ui.add_space(4.0);
        let mut changed = self.geometry_list(ui, session, tokens);
        ui.add_space(8.0);
        egui::Grid::new("tp-geometry").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            if matches!(self.strategy, Strategy::Contour | Strategy::Chamfer) {
                let sides = [("outside", "Outside"), ("inside", "Inside"), ("on", "On the line")];
                changed |=
                    Self::row(ui, "Cut side", "Open contours: outside is the left of the edge's direction", |ui| {
                        self.choice(ui, "side", &sides, "outside")
                    });
            }
            changed |= Self::row(ui, "", "", |ui| self.check(ui, "reverse", "Reverse direction", false));
            changed |= self.wcs_row(ui, session);
        });
        changed
    }

    /// The picked faces, edges and points, each with a remove button.
    fn geometry_list(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens) -> bool {
        let mut changed = false;
        if self.geometry.is_empty() {
            ui.label(RichText::new("Nothing selected yet.").color(tokens.text_muted));
        }
        let mut remove = None;
        for (i, r) in self.geometry.iter().enumerate() {
            ui.horizontal(|ui| {
                let label = match session.job.entity(r) {
                    Some((m, Entity::Face(f))) => {
                        let kind = m.faces.faces.get(f).map_or("face", |x| x.kind.id());
                        format!("{} {kind} face {f}", m.name)
                    }
                    Some((m, Entity::Edge(e))) => {
                        let kind = m.edges.edges.get(e).map_or("edge", |x| x.kind.id());
                        format!("{} {kind} edge {e}", m.name)
                    }
                    _ => format!("{r} (missing)"),
                };
                ui.label(label);
                if ui.small_button("×").on_hover_text("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.geometry.remove(i);
            changed = true;
        }
        if !self.geometry.is_empty() && ui.small_button("Clear all").clicked() {
            self.geometry.clear();
            changed = true;
        }
        changed
    }

    /// The WCS row: the active one, or a chosen one.
    fn wcs_row(&mut self, ui: &mut Ui, session: &Session) -> bool {
        let mut wcs: Vec<(String, String)> = vec![(String::new(), "Active WCS".into())];
        wcs.extend(session.job.wcs.iter().map(|w| (w.id.clone(), w.name.clone())));
        let options: Vec<(&str, &str)> = wcs.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        Self::row(ui, "Program in", "The WCS the toolpath is programmed in", |ui| {
            let c = self.choice(ui, "wcs", &options, "");
            if self.values.get("wcs").and_then(Value::as_str) == Some("") {
                self.values.remove("wcs");
            }
            c
        })
    }

    /// The Geometry tab of the strategies other than the contour.
    fn strategy_geometry_tab(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens, u: DisplayUnits) -> bool {
        let mut changed = false;
        let hint = match self.strategy {
            Strategy::Pocket => {
                "Click the pocket's floor faces, or closed loops of edges. Islands in a floor face are left standing."
            }
            Strategy::Drill => {
                "Click the holes: their walls (cylinders) or rims (circular edges). Each hole is drilled once."
            }
            Strategy::Bore => "Click the holes to bore: their walls (cylinders) or rims (circular edges).",
            Strategy::Rough | Strategy::Waterline => "Works on the whole model; there is nothing to pick.",
            Strategy::Adaptive => {
                "Clears the stock around the model, or with Selected pockets the floors or closed edges you click."
            }
            Strategy::Thread => {
                "Click the holes or bosses to thread: their walls (cylinders) or rims (circular edges)."
            }
            Strategy::Flat => "Finishes every flat floor facing the tool; click faces to finish only their floors.",
            _ => "Choose the area below. With Selected faces, click faces or closed edges to work inside.",
        };
        ui.label(RichText::new(hint).color(tokens.accent));
        ui.add_space(4.0);
        if self.strategy == Strategy::Adaptive {
            egui::Grid::new("tp-adaptive-area").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                let areas = [("stock", "Stock around the model (3D)"), ("selection", "Selected pockets (2D)")];
                changed |= Self::row(
                    ui,
                    "Clear",
                    "3D clears the stock down around the model; 2D clears the pockets you pick",
                    |ui| self.choice(ui, "boundary", &areas, "stock"),
                );
            });
            ui.add_space(4.0);
        }
        if matches!(self.strategy, Strategy::Face | Strategy::Raster | Strategy::MultiAxis)
            || self.strategy.surface_finish()
        {
            egui::Grid::new("tp-boundary").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                let areas = [("stock", "Stock"), ("model", "Model"), ("selection", "Selected faces")];
                let default = if self.strategy == Strategy::Face { "stock" } else { "model" };
                changed |=
                    Self::row(ui, "Area", "What the rows cover", |ui| self.choice(ui, "boundary", &areas, default));
                changed |= Self::row(ui, "Area offset", "Grow (or shrink, negative) the area", |ui| {
                    let c = self.number(ui, "boundary_offset", "0", Some(u), false);
                    ui.label(u.length());
                    c
                });
            });
            ui.add_space(4.0);
        }
        if self.takes_geometry() {
            changed |= self.geometry_list(ui, session, tokens);
            ui.add_space(8.0);
        }
        egui::Grid::new("tp-geometry-wcs").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            changed |= self.wcs_row(ui, session);
        });
        changed
    }

    /// The Passes tab of the strategies other than the contour.
    fn strategy_passes_tab(&mut self, ui: &mut Ui, u: DisplayUnits) -> bool {
        let mut changed = false;
        let len = |ui: &mut Ui, me: &mut Self, key: &str, hint: &str| {
            let c = me.number(ui, key, hint, Some(u), false);
            ui.label(u.length());
            c
        };
        let strategy = self.strategy;
        egui::Grid::new("tp-strategy-passes").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            if strategy == Strategy::Bore {
                changed |= Self::row(ui, "Pitch", "Depth per turn of the helix", |ui| len(ui, self, "pitch", "15% Ø"));
                changed |= Self::row(ui, "Stepover", "Between helices in a hole wider than one clears", |ui| {
                    len(ui, self, "stepover", "40% Ø")
                });
                changed |= Self::row(ui, "Stock to leave", "On the wall", |ui| len(ui, self, "stock_radial", "0"));
                let dirs = [("climb", "Climb"), ("conventional", "Conventional")];
                changed |= Self::row(ui, "Direction", "Climb runs counter-clockwise inside a hole", |ui| {
                    self.choice(ui, "direction", &dirs, "climb")
                });
                changed |= Self::row(ui, "Tolerance", "How far the helix may stray from a true circle", |ui| {
                    len(ui, self, "tolerance", &u.plain(0.01))
                });
                return;
            }
            if strategy == Strategy::Drill {
                let cycles = [
                    ("drill", "Drill"),
                    ("spot", "Spot drill (dwell)"),
                    ("peck", "Peck (full retract)"),
                    ("chip_break", "Chip break (short retract)"),
                    ("tap", "Tap"),
                    ("bore", "Bore (feed out)"),
                ];
                changed |=
                    Self::row(ui, "Cycle", "How each hole is drilled", |ui| self.choice(ui, "cycle", &cycles, "drill"));
                changed |= Self::row(ui, "Peck depth", "Depth per peck", |ui| len(ui, self, "peck_depth", "Ø"));
                changed |= Self::row(ui, "Dwell", "Pause at the bottom", |ui| {
                    let c = self.number(ui, "dwell", "0", None, false);
                    ui.label("s");
                    c
                });
                changed |=
                    Self::row(ui, "Thread pitch", "For tapping: the thread's pitch", |ui| len(ui, self, "pitch", "—"));
                return;
            }
            if strategy == Strategy::Thread {
                changed |= Self::row(ui, "Pitch", "Thread pitch: advance per turn (blank: the thread mill's)", |ui| {
                    len(ui, self, "pitch", "tool")
                });
                let types = [("internal", "Internal (in a hole)"), ("external", "External (on a boss)")];
                changed |= Self::row(ui, "Thread", "", |ui| self.choice(ui, "thread_type", &types, "internal"));
                let hands = [("right", "Right-hand"), ("left", "Left-hand")];
                changed |= Self::row(ui, "Hand", "", |ui| self.choice(ui, "hand", &hands, "right"));
                changed |= Self::row(
                    ui,
                    "Major diameter",
                    "Blank: a hole's diameter plus 1.0825 × pitch, a boss's diameter",
                    |ui| len(ui, self, "major_diameter", "auto"),
                );
                changed |= Self::row(ui, "Radial passes", "Passes to full thread depth", |ui| {
                    self.number(ui, "radial_passes", "1", None, false)
                });
                changed |= Self::row(ui, "", "", |ui| self.check(ui, "spring_pass", "Spring pass", false));
                let dirs = [("climb", "Climb"), ("conventional", "Conventional")];
                changed |= Self::row(ui, "Direction", "", |ui| self.choice(ui, "direction", &dirs, "climb"));
                return;
            }
            if strategy.surface_finish() || strategy == Strategy::Flat {
                let (over, help) = match strategy {
                    Strategy::Flat => ("60% Ø", "Distance between passes on each floor"),
                    Strategy::Radial => ("10% Ø", "Distance between spokes where they end"),
                    Strategy::Pencil => ("—", ""),
                    _ => ("10% Ø", "Distance between passes, measured along the surface"),
                };
                if strategy != Strategy::Pencil {
                    changed |= Self::row(ui, "Stepover", help, |ui| len(ui, self, "stepover", over));
                }
                if strategy == Strategy::SteepShallow {
                    changed |= Self::row(
                        ui,
                        "Steep above",
                        "Slope from horizontal that divides steep walls (level passes) from shallow areas",
                        |ui| {
                            let c = self.number(ui, "threshold_angle", "40", None, false);
                            ui.label("°");
                            c
                        },
                    );
                }
                if strategy == Strategy::Pencil {
                    changed |= Self::row(ui, "Shortest corner", "Corners shorter than this get no pass", |ui| {
                        len(ui, self, "min_length", "Ø/2")
                    });
                }
                if matches!(strategy, Strategy::Spiral | Strategy::Radial) {
                    changed |= Self::row(ui, "Centre X", "Blank: the middle of the area", |ui| {
                        len(ui, self, "center_x", "auto")
                    });
                    changed |= Self::row(ui, "Centre Y", "Blank: the middle of the area", |ui| {
                        len(ui, self, "center_y", "auto")
                    });
                }
                if strategy == Strategy::Spiral {
                    let dirs = [("outward", "Outward"), ("inward", "Inward")];
                    changed |= Self::row(ui, "Spiral", "", |ui| self.choice(ui, "spiral_direction", &dirs, "outward"));
                }
                if strategy == Strategy::Radial {
                    changed |= Self::row(ui, "Inner radius", "Where the spokes start", |ui| {
                        len(ui, self, "inner_radius", "stepover")
                    });
                }
                if strategy == Strategy::Flat {
                    changed |= Self::row(ui, "Stock to leave (radial)", "On the walls", |ui| {
                        len(ui, self, "stock_radial", "0")
                    });
                }
                changed |= Self::row(ui, "Stock to leave", "On the surface (negative leaves it lower)", |ui| {
                    len(ui, self, "stock_axial", "0")
                });
                if strategy != Strategy::Flat {
                    changed |= Self::row(
                        ui,
                        "Point spacing",
                        "Spacing of points dropped onto the model along each pass",
                        |ui| len(ui, self, "sample_step", "Ø/8"),
                    );
                }
                changed |= Self::row(ui, "Tolerance", "How far the toolpath may stray from the surface", |ui| {
                    len(ui, self, "tolerance", &u.plain(0.01))
                });
                return;
            }
            if strategy == Strategy::Adaptive {
                changed |= Self::row(
                    ui,
                    "Optimal load",
                    "The width of cut the tool keeps to, all the way round: 8–25% of the diameter suits most cutters",
                    |ui| len(ui, self, "stepover", "15% Ø"),
                );
                changed |=
                    Self::row(ui, "Stepdown", "Depth per level: deep cuts at a light load use the whole flute", |ui| {
                        len(ui, self, "stepdown", "tool")
                    });
                let dirs = [("climb", "Climb"), ("conventional", "Conventional")];
                changed |=
                    Self::row(ui, "Direction", "Climb keeps the material on the right of the tool's travel", |ui| {
                        self.choice(ui, "direction", &dirs, "climb")
                    });
                changed |=
                    Self::row(ui, "Stock to leave (radial)", "On the walls", |ui| len(ui, self, "stock_radial", "0"));
                changed |=
                    Self::row(ui, "Stock to leave (axial)", "On the floors", |ui| len(ui, self, "stock_axial", "0"));
                let rests = [("none", "Everything"), ("previous", "What earlier toolpaths left")];
                changed |= Self::row(
                    ui,
                    "Clear",
                    "Rest machining: only the material the toolpaths before this one could not reach",
                    |ui| self.choice(ui, "rest", &rests, "none"),
                );
                changed |=
                    Self::row(ui, "", "", |ui| self.check(ui, "flat_levels", "Extra levels at flat areas", true));
                changed |=
                    Self::row(ui, "", "", |ui| self.check(ui, "wall_pass", "Pass along the walls at each level", true));
                changed |= Self::row(ui, "Tolerance", "How far the toolpath may stray from true curves", |ui| {
                    len(ui, self, "tolerance", &u.plain(0.01))
                });
                return;
            }
            let tolerance_help = "How far the toolpath may stray from true curves. Smaller gives more, closer points.";
            changed |= Self::row(ui, "Tolerance", tolerance_help, |ui| len(ui, self, "tolerance", &u.plain(0.01)));
            let dirs = [("climb", "Climb"), ("conventional", "Conventional")];
            changed |= Self::row(ui, "Direction", "Climb keeps the material on the right of the tool's travel", |ui| {
                self.choice(ui, "direction", &dirs, "climb")
            });
            let (over, down) = match strategy {
                Strategy::Face => ("70% Ø", "tool"),
                Strategy::Pocket | Strategy::Rough => ("45% Ø", "tool"),
                Strategy::Waterline => ("—", "10% Ø"),
                _ => ("10% Ø", "—"),
            };
            if strategy != Strategy::Waterline {
                changed |= Self::row(ui, "Stepover", "Distance between passes", |ui| len(ui, self, "stepover", over));
            }
            let rows_like_raster = matches!(strategy, Strategy::Raster | Strategy::MultiAxis);
            if !rows_like_raster {
                changed |= Self::row(ui, "Stepdown", "Depth per level", |ui| len(ui, self, "stepdown", down));
            }
            if matches!(strategy, Strategy::Face | Strategy::Pocket) {
                let default = strategy == Strategy::Pocket;
                changed |= Self::row(ui, "", "", |ui| self.check(ui, "multiple_depths", "Multiple depths", default));
            }
            if matches!(strategy, Strategy::Face | Strategy::Rough) || rows_like_raster {
                changed |= Self::row(ui, "Row angle", "Direction of the rows, degrees from X", |ui| {
                    let c = self.number(ui, "angle", "0", None, false);
                    ui.label("°");
                    c
                });
            }
            if strategy == Strategy::Face || rows_like_raster {
                let patterns = [("zigzag", "Back and forth"), ("one_way", "One way")];
                changed |= Self::row(ui, "Pattern", "", |ui| self.choice(ui, "pattern", &patterns, "zigzag"));
            }
            if strategy == Strategy::Face {
                changed |= Self::row(ui, "Overrun", "How far rows run past the area, beyond the tool radius", |ui| {
                    len(ui, self, "overrun", "Ø/4")
                });
            }
            if rows_like_raster {
                changed |= Self::row(ui, "Point spacing", "Spacing of points along each row", |ui| {
                    len(ui, self, "sample_step", "Ø/8")
                });
            }
            if strategy != Strategy::Face {
                changed |=
                    Self::row(ui, "Stock to leave (radial)", "On the walls", |ui| len(ui, self, "stock_radial", "0"));
            }
            changed |= Self::row(ui, "Stock to leave (axial)", "On the floors", |ui| len(ui, self, "stock_axial", "0"));
            if strategy == Strategy::Pocket {
                changed |= Self::row(ui, "", "", |ui| {
                    self.check(ui, "finishing_pass", "Finish the walls to size (with radial stock)", false)
                });
            }
        });
        changed
    }

    /// The Linking tab of the milling strategies other than the contour.
    fn strategy_linking_tab(&mut self, ui: &mut Ui, u: DisplayUnits) -> bool {
        let mut changed = false;
        let ramps = !(self.strategy.surface_finish() || self.strategy == Strategy::Flat);
        egui::Grid::new("tp-strategy-linking").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            if !ramps {
                ui.label(RichText::new("Links").strong());
                ui.end_row();
                changed |= Self::row(ui, "", "", |ui| self.check(ui, "rapid_links", "Rapid links", true));
                if self.strategy.surface_finish() {
                    changed |= Self::row(
                        ui,
                        "Stay down within",
                        "Longest link along the surface made without lifting the tool",
                        |ui| {
                            let c = self.number(ui, "stay_down_distance", "2 × Ø", Some(u), false);
                            ui.label(u.length());
                            c
                        },
                    );
                }
                return;
            }
            ui.label(RichText::new("Entry").strong());
            ui.end_row();
            let entries = [("helix", "Helical ramp"), ("plunge", "Plunge")];
            changed |= Self::row(ui, "Into material", "How the tool goes down where there is material", |ui| {
                self.choice(ui, "entry", &entries, "helix")
            });
            changed |= Self::row(ui, "Ramp angle", "Helix angle", |ui| {
                let c = self.number(ui, "ramp_angle", "2", None, false);
                ui.label("°");
                c
            });
            changed |= Self::row(ui, "Helix radius", "At most the tool radius", |ui| {
                let c = self.number(ui, "helix_radius", "0.4 × Ø", Some(u), false);
                ui.label(u.length());
                c
            });
            ui.label(RichText::new("Links").strong());
            ui.end_row();
            changed |= Self::row(ui, "", "", |ui| self.check(ui, "rapid_links", "Rapid links", true));
            if self.strategy == Strategy::Adaptive {
                changed |= Self::row(ui, "", "", |ui| {
                    self.check(ui, "keep_down", "Keep the tool down where the way is clear", true)
                });
            }
            if self.strategy == Strategy::Adaptive {
                changed |= Self::row(ui, "Stay down within", "Longest link made without lifting the tool", |ui| {
                    let c = self.number(ui, "stay_down_distance", "20 × Ø", Some(u), false);
                    ui.label(u.length());
                    c
                });
            }
        });
        changed
    }

    fn heights_tab(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens, u: DisplayUnits) -> bool {
        let mut changed = false;
        let defaults = if self.strategy == Strategy::Adaptive && self.takes_geometry() {
            CosmicCAM_engine::toolpath::strategies::POCKET_HEIGHTS.to_vec()
        } else {
            self.strategy.heights()
        };
        for (prefix, label, icon, help) in HEIGHT_ROWS {
            let Some(&(_, default_ref, default_offset)) = defaults.iter().find(|(p, _, _)| p == prefix) else {
                continue;
            };
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
                crate::ribbon::paint_named_icon(ui, rect, icon, tokens, true);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(*label).strong()).on_hover_text(*help);
                        let value = self.heights.iter().find(|(p, _)| p == prefix).and_then(|(_, v)| *v);
                        if let Some(z) = value {
                            ui.label(RichText::new(format!("= Z {}", u.fmt_len(z))).small().color(tokens.text_muted));
                        }
                    });
                    ui.horizontal(|ui| {
                        changed |= self.choice(ui, &format!("{prefix}_from"), REF_LABELS, default_ref);
                        ui.label("+");
                        let hint = u.plain(contour::default_offset(session, default_offset));
                        changed |= self.number(ui, &format!("{prefix}_offset"), &hint, Some(u), false);
                        ui.label(u.length());
                    });
                });
            });
            ui.add_space(4.0);
        }
        ui.label(
            RichText::new("From: what the height is measured from. +: the offset above it (negative goes below).")
                .small()
                .color(tokens.text_muted),
        );
        changed
    }

    fn passes_tab(&mut self, ui: &mut Ui, u: DisplayUnits) -> bool {
        let mut changed = false;
        let len = |ui: &mut Ui, me: &mut Self, key: &str, hint: &str| {
            let c = me.number(ui, key, hint, Some(u), false);
            ui.label(u.length());
            c
        };
        egui::Grid::new("tp-passes").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            if self.strategy == Strategy::Chamfer {
                changed |= Self::row(ui, "Chamfer width", "Width of the chamfer across each face", |ui| {
                    len(ui, self, "chamfer_width", &u.plain(0.5))
                });
                changed |= Self::row(
                    ui,
                    "Tip offset",
                    "How far the tip goes below the chamfer, so the tip itself does not cut",
                    |ui| len(ui, self, "tip_offset", &u.plain(0.25)),
                );
            }
            if self.strategy == Strategy::Trace {
                changed |= Self::row(ui, "Depth", "Below the picked edges (a bottom height overrides it)", |ui| {
                    len(ui, self, "depth", &u.plain(0.2))
                });
            }
            let tolerance_help = "How far the toolpath may stray from the true curve: arcs, circles and rounded \
                                  corners get more, closer points as this gets smaller. Straight lines are unaffected.";
            changed |= Self::row(ui, "Tolerance", tolerance_help, |ui| len(ui, self, "tolerance", &u.plain(0.01)));
            let dirs = [("climb", "Climb"), ("conventional", "Conventional")];
            changed |= Self::row(ui, "Direction", "Climb keeps the material on the right of the tool's travel", |ui| {
                self.choice(ui, "direction", &dirs, "climb")
            });
            changed |= Self::row(ui, "", "", |ui| self.check(ui, "both_ways", "Both ways (open contours)", false));
            changed |= Self::row(ui, "Radial passes", "Side-by-side passes, outermost first", |ui| {
                self.number(ui, "radial_passes", "1", None, false)
            });
            changed |= Self::row(ui, "Radial stepover", "Distance between radial passes", |ui| {
                len(ui, self, "radial_stepover", "tool")
            });
            changed |= Self::row(ui, "Spring passes", "Repeat the final pass to take off deflection", |ui| {
                self.number(ui, "spring_passes", "0", None, false)
            });
            changed |=
                Self::row(ui, "Stock to leave (radial)", "On the walls", |ui| len(ui, self, "stock_radial", "0"));
            changed |= Self::row(ui, "Stock to leave (axial)", "On the floor", |ui| len(ui, self, "stock_axial", "0"));
            changed |= Self::row(ui, "", "", |ui| self.check(ui, "multiple_depths", "Multiple depths", false));
            changed |=
                Self::row(ui, "Max stepdown", "Deepest step per depth", |ui| len(ui, self, "max_stepdown", "tool"));
            changed |= Self::row(ui, "", "", |ui| self.check(ui, "feed_optimization", "Feed optimization", false));
            changed |= Self::row(ui, "Direction change", "Turns sharper than this count, degrees", |ui| {
                let c = self.number(ui, "optimization_angle", "45", None, false);
                ui.label("°");
                c
            });
            changed |= Self::row(ui, "Reduced feed", "Percent of the cutting feed at direction changes", |ui| {
                let c = self.number(ui, "optimization_feed", "50", None, false);
                ui.label("%");
                c
            });
            changed |= Self::row(ui, "Slow-down distance", "Before and after the change", |ui| {
                len(ui, self, "optimization_distance", "Ø/2")
            });
        });
        changed
    }

    fn linking_tab(&mut self, ui: &mut Ui, u: DisplayUnits) -> bool {
        let mut changed = false;
        let leads: Vec<(&str, &str)> = LEADS
            .iter()
            .map(|l| {
                (
                    *l,
                    match *l {
                        "arc" => "Arc",
                        "line" => "Line",
                        _ => "None",
                    },
                )
            })
            .collect();
        let len = |ui: &mut Ui, me: &mut Self, key: &str, hint: &str| {
            let c = me.number(ui, key, hint, Some(u), false);
            ui.label(u.length());
            c
        };
        egui::Grid::new("tp-linking").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("Entry point").strong());
            ui.end_row();
            let entries = [("auto", "Automatic"), ("origin", "Origin"), ("wcs", "WCS"), ("selection", "Selection")];
            let entry_help = "Where each closed contour starts: mid-way along its longest side (Automatic), or as \
                              near as it passes to the world origin, the WCS origin or a point you pick";
            let entry_changed = Self::row(ui, "From", entry_help, |ui| self.choice(ui, "entry_from", &entries, "auto"));
            changed |= entry_changed;
            let from_selection = self.values.get("entry_from").and_then(Value::as_str) == Some("selection");
            if entry_changed {
                // Choosing Selection starts picking straight away when there is no point yet.
                self.picking_entry = from_selection && self.entry_point().is_none();
            }
            if !from_selection {
                self.picking_entry = false;
            } else {
                let point = self.entry_point();
                let picking = self.picking_entry;
                Self::row(ui, "", "", |ui| {
                    let label = if picking { "Click a point in the viewport…" } else { "Pick point" };
                    if ui.selectable_label(picking, label).clicked() {
                        self.picking_entry = !picking;
                    }
                    if let Some(p) = point.filter(|_| !picking) {
                        ui.label(format!(
                            "{}, {}, {}",
                            u.fmt_len(f64::from(p.x)),
                            u.fmt_len(f64::from(p.y)),
                            u.fmt_len(f64::from(p.z))
                        ));
                    }
                    false
                });
            }
            ui.label(RichText::new("Lead-in").strong());
            ui.end_row();
            changed |= Self::row(ui, "Style", "", |ui| self.choice(ui, "lead_in", &leads, "arc"));
            changed |= Self::row(ui, "Arc radius", "", |ui| len(ui, self, "lead_in_radius", "Ø/2"));
            changed |= Self::row(ui, "Arc sweep", "", |ui| {
                let c = self.number(ui, "lead_in_sweep", "90", None, false);
                ui.label("°");
                c
            });
            changed |= Self::row(ui, "Line length", "", |ui| len(ui, self, "lead_in_length", "Ø/2"));
            ui.label(RichText::new("Lead-out").strong());
            ui.end_row();
            changed |= Self::row(ui, "", "", |ui| self.check(ui, "lead_out_same", "Same as lead-in", true));
            if !self.values.get("lead_out_same").and_then(Value::as_bool).unwrap_or(true) {
                changed |= Self::row(ui, "Style", "", |ui| self.choice(ui, "lead_out", &leads, "arc"));
                changed |= Self::row(ui, "Arc radius", "", |ui| len(ui, self, "lead_out_radius", "Ø/2"));
                changed |= Self::row(ui, "Arc sweep", "", |ui| {
                    let c = self.number(ui, "lead_out_sweep", "90", None, false);
                    ui.label("°");
                    c
                });
                changed |= Self::row(ui, "Line length", "", |ui| len(ui, self, "lead_out_length", "Ø/2"));
            }
            ui.label(RichText::new("Entry").strong());
            ui.end_row();
            let ramps = [("ramp", "Ramp along the lead-in"), ("plunge", "Plunge")];
            changed |= Self::row(ui, "Going down", "", |ui| self.choice(ui, "ramp", &ramps, "ramp"));
            changed |= Self::row(ui, "Ramp angle", "", |ui| {
                let c = self.number(ui, "ramp_angle", "3", None, false);
                ui.label("°");
                c
            });
            ui.label(RichText::new("Links").strong());
            ui.end_row();
            let retracts: Vec<(&str, &str)> = RETRACTS
                .iter()
                .map(|r| {
                    (
                        *r,
                        match *r {
                            "minimum" => "Retract height",
                            "full" => "Clearance height",
                            _ => "Stay down",
                        },
                    )
                })
                .collect();
            changed |= Self::row(ui, "Between passes", "How high the tool goes between passes", |ui| {
                self.choice(ui, "retract", &retracts, "minimum")
            });
            changed |= Self::row(ui, "Stay-down distance", "Stay down when the next pass starts this close", |ui| {
                len(ui, self, "stay_down_distance", "2 × Ø")
            });
            changed |= Self::row(ui, "", "", |ui| self.check(ui, "rapid_links", "Rapid links", true));
            if !self.values.get("rapid_links").and_then(Value::as_bool).unwrap_or(true) {
                changed |= Self::row(ui, "Link feed", "", |ui| {
                    let c = self.number(ui, "link_feed", "machine max", Some(u), true);
                    ui.label(u.rate());
                    c
                });
            }
        });
        changed
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The OK and Cancel buttons centred at the foot of a side panel: (OK clicked, Cancel clicked).
pub(crate) fn ok_cancel(ui: &mut Ui, ready: bool) -> (bool, bool) {
    const BUTTON: egui::Vec2 = egui::vec2(120.0, 34.0);
    const GAP: f32 = 12.0;
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - 2.0 * BUTTON.x - GAP) / 2.0).max(0.0));
        let ok = ui.add_enabled(ready, egui::Button::new(RichText::new("OK").strong().size(15.0)).min_size(BUTTON));
        ui.add_space(GAP);
        let cancel = ui.add(egui::Button::new(RichText::new("Cancel").size(15.0)).min_size(BUTTON));
        (ok.clicked(), cancel.clicked())
    })
    .inner
}
