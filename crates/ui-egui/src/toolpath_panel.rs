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
}

const TABS: &[(Tab, &str)] = &[
    (Tab::Tool, "Tool"),
    (Tab::MultiAxis, "Multi-axis"),
    (Tab::Geometry, "Geometry"),
    (Tab::Heights, "Heights"),
    (Tab::Passes, "Passes"),
    (Tab::Linking, "Linking"),
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
        }
    }

    /// The engine's strategy id, for its height defaults.
    fn engine_id(self) -> &'static str {
        use CosmicCAM_engine::toolpath::strategies as st;
        match self {
            Self::Contour => "contour2d",
            Self::Face => st::FACE,
            Self::Pocket => st::POCKET,
            Self::Drill => CosmicCAM_engine::toolpath::drill::DRILL,
            Self::Rough => st::ROUGH,
            Self::Waterline => st::WATERLINE,
            Self::Raster => st::RASTER,
        }
    }

    fn tabs(self) -> Vec<(Tab, &'static str)> {
        TABS.iter()
            .copied()
            .filter(|(t, _)| match self {
                Self::Contour => true,
                Self::Drill => !matches!(t, Tab::MultiAxis | Tab::Linking),
                _ => *t != Tab::MultiAxis,
            })
            .collect()
    }

    /// Whether the strategy needs geometry picked before it can run.
    fn needs_geometry(self) -> bool {
        matches!(self, Self::Contour | Self::Pocket | Self::Drill)
    }

    /// (prefix, reference, offset mm) for each height the strategy uses.
    fn heights(self) -> Vec<(&'static str, &'static str, f64)> {
        use CosmicCAM_engine::toolpath::strategies as st;
        match self {
            Self::Contour => HEIGHTS.to_vec(),
            Self::Face => st::FACE_HEIGHTS.to_vec(),
            Self::Pocket => st::POCKET_HEIGHTS.to_vec(),
            Self::Drill => CosmicCAM_engine::toolpath::drill::DRILL_HEIGHTS.to_vec(),
            Self::Rough => st::ROUGH_HEIGHTS.to_vec(),
            Self::Waterline | Self::Raster => st::FINISH_HEIGHTS.to_vec(),
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
    preview: Result<(Vec3, Arc<Vec<Move>>, Value), String>,
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

    /// Recomputes the preview by running the command on a copy of the session.
    fn refresh(&mut self, session: &Session) {
        self.dirty = false;
        self.error = None;
        let mut p = self.params();
        if let Some(obj) = p.as_object_mut() {
            obj.remove("id");
        }
        self.heights = match self.strategy {
            Strategy::Contour => contour::preview_heights(session, &p),
            other => CosmicCAM_engine::toolpath::strategies::preview_heights(other.engine_id(), session, &p),
        };
        let mut trial = session.clone();
        self.preview = trial.execute(self.strategy.command(), &p).map_err(|e| e.to_string()).and_then(|summary| {
            let id = summary["id"].as_str().unwrap_or_default().to_owned();
            trial
                .job
                .toolpaths
                .iter()
                .find(|t| t.id == id)
                .map(|t| (t.start, Arc::clone(&t.moves), summary.clone()))
                .ok_or_else(|| "no toolpath".to_owned())
        });
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
                        Tab::MultiAxis => self.multi_axis_tab(ui, tokens),
                        Tab::Geometry if self.strategy != Strategy::Contour => {
                            self.strategy_geometry_tab(ui, session, tokens, units)
                        }
                        Tab::Geometry => self.geometry_tab(ui, session, tokens),
                        Tab::Heights => self.heights_tab(ui, session, tokens, units),
                        Tab::Passes if self.strategy != Strategy::Contour => self.strategy_passes_tab(ui, units),
                        Tab::Passes => self.passes_tab(ui, units),
                        Tab::Linking if self.strategy != Strategy::Contour => self.strategy_linking_tab(ui, units),
                        Tab::Linking => self.linking_tab(ui, units),
                    };
                    self.dirty |= changed;
                },
            );
            ui.separator();
            // The summary and messages sit centred over the buttons.
            ui.vertical_centered(|ui| match (&self.error, &self.preview) {
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
            let (ok, cancel) = ok_cancel(ui, self.preview.is_ok());
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
            Strategy::Face | Strategy::Raster => {
                self.values.get("boundary").and_then(Value::as_str) == Some("selection")
            }
            _ => true,
        }
    }

    // ---- tabs ----------------------------------------------------------------------------

    fn tool_tab(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens, u: DisplayUnits) -> bool {
        let Some(machine) = session.library.active_machine() else {
            ui.label(RichText::new("There is no machine. Add one on the Machine tab.").color(tokens.error));
            return false;
        };
        if machine.tools.is_empty() {
            ui.label(
                RichText::new(format!("{}'s tool library is empty. Add a tool on the Tooling tab.", machine.name))
                    .color(tokens.error),
            );
            return false;
        }
        let mut changed = false;
        let current = self
            .values
            .get("tool")
            .and_then(Value::as_u64)
            .map(|n| n as u32)
            .or(machine.tools.first().map(|t| t.number));
        let tool = current.and_then(|n| machine.tool(n));
        ui.label(RichText::new(format!("Machine: {}", machine.name)).small().color(tokens.text_muted));
        egui::ComboBox::from_id_salt("tp-tool")
            .selected_text(tool.map_or("Choose a tool".to_owned(), |t| format!("T{}  {}", t.number, t.name)))
            .width(300.0)
            .show_ui(ui, |ui| {
                for t in &machine.tools {
                    if ui.selectable_label(Some(t.number) == current, format!("T{}  {}", t.number, t.name)).clicked() {
                        self.values.insert("tool".into(), json!(t.number));
                        changed = true;
                    }
                }
            });
        if let Some(t) = tool {
            crate::tool_view::draw(
                ui,
                Some(crate::tool_view::ToolShape {
                    diameter: t.diameter,
                    shank_diameter: t.shank_diameter,
                    flute_length: t.flute_length,
                    overall_length: t.overall_length,
                    flutes: t.flutes,
                }),
                u,
                tokens,
                egui::vec2(300.0, 190.0),
            );
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
                    Strategy::Contour => &[
                        ("feed", "Cutting feed", t.feed),
                        ("plunge_feed", "Plunge feed", t.plunge_feed),
                        ("lead_feed", "Lead-in / out feed", t.feed),
                    ],
                    Strategy::Drill => &[("feed", "Drilling feed", t.plunge_feed)],
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

    fn multi_axis_tab(&mut self, ui: &mut Ui, tokens: &Tokens) -> bool {
        ui.label(RichText::new("Tool orientation").strong());
        ui.add_enabled_ui(false, |ui| {
            egui::ComboBox::from_id_salt("tp-axis").selected_text("WCS Z axis (3-axis)").show_ui(ui, |_| {});
            ui.checkbox(&mut false, "Tilt the tool");
        });
        ui.label(
            RichText::new("Multi-axis toolpaths are planned. This contour cuts with the tool along the WCS Z axis.")
                .color(tokens.text_muted),
        );
        false
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
            let sides = [("outside", "Outside"), ("inside", "Inside"), ("on", "On the line")];
            changed |= Self::row(ui, "Cut side", "Open contours: outside is the left of the edge's direction", |ui| {
                self.choice(ui, "side", &sides, "outside")
            });
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
            Strategy::Rough | Strategy::Waterline => "Works on the whole model; there is nothing to pick.",
            _ => "Choose the area below. With Selected faces, click faces or closed edges to work inside.",
        };
        ui.label(RichText::new(hint).color(tokens.accent));
        ui.add_space(4.0);
        if matches!(self.strategy, Strategy::Face | Strategy::Raster) {
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
            if strategy != Strategy::Raster {
                changed |= Self::row(ui, "Stepdown", "Depth per level", |ui| len(ui, self, "stepdown", down));
            }
            if matches!(strategy, Strategy::Face | Strategy::Pocket) {
                let default = strategy == Strategy::Pocket;
                changed |= Self::row(ui, "", "", |ui| self.check(ui, "multiple_depths", "Multiple depths", default));
            }
            if matches!(strategy, Strategy::Face | Strategy::Rough | Strategy::Raster) {
                changed |= Self::row(ui, "Row angle", "Direction of the rows, degrees from X", |ui| {
                    let c = self.number(ui, "angle", "0", None, false);
                    ui.label("°");
                    c
                });
            }
            if matches!(strategy, Strategy::Face | Strategy::Raster) {
                let patterns = [("zigzag", "Back and forth"), ("one_way", "One way")];
                changed |= Self::row(ui, "Pattern", "", |ui| self.choice(ui, "pattern", &patterns, "zigzag"));
            }
            if strategy == Strategy::Face {
                changed |= Self::row(ui, "Overrun", "How far rows run past the area, beyond the tool radius", |ui| {
                    len(ui, self, "overrun", "Ø/4")
                });
            }
            if strategy == Strategy::Raster {
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
        egui::Grid::new("tp-strategy-linking").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
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
        });
        changed
    }

    fn heights_tab(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens, u: DisplayUnits) -> bool {
        let mut changed = false;
        let defaults = self.strategy.heights();
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
