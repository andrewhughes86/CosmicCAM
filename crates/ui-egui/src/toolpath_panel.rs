//! The toolpath editor: a panel docked at the right of the window while a toolpath is being
//! created or edited. Its tabs (Tool, Multi-axis, Geometry, Heights, Passes, Linking) set
//! the parameters of `toolpath.contour`; the result is previewed in the viewport as settings
//! change, and OK runs the command.

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
}

impl ToolpathPanel {
    /// A new contour, or `editing` an existing toolpath with its stored settings.
    pub fn new(session: &Session, editing: Option<&str>) -> Self {
        let mut values = Map::new();
        let mut geometry = Vec::new();
        if let Some(tp) = editing.and_then(|id| session.job.toolpaths.iter().find(|t| t.id == id)) {
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
                && session.job.entity(sel).is_some_and(|(_, e)| !matches!(e, Entity::Vertex(_)))
            {
                geometry.push(sel.to_owned());
            }
        }
        let mut panel = Self {
            editing: editing.map(str::to_owned),
            tab: if geometry.is_empty() { Tab::Geometry } else { Tab::Tool },
            values,
            text: HashMap::new(),
            geometry,
            preview: Err(String::new()),
            heights: Vec::new(),
            dirty: true,
            error: None,
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
        self.heights = contour::preview_heights(session, &p);
        let mut trial = session.clone();
        self.preview = trial.execute("toolpath.contour", &p).map_err(|e| e.to_string()).and_then(|summary| {
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
        let mut mode = PickMode {
            accept: (self.tab == Tab::Geometry).then_some(Accept::Axis),
            picked: Vec::new(),
            picked_edges: Vec::new(),
            picked_points: Vec::new(),
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
            let title = if self.editing.is_some() { "Edit 2D contour" } else { "New 2D contour" };
            ui.label(RichText::new(title).heading());
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for (tab, label) in TABS {
                    ui.selectable_value(&mut self.tab, *tab, *label);
                }
            });
            ui.separator();
            // The settings scroll; the summary and buttons stay at the bottom.
            let footer = 110.0;
            // Fill the panel so the buttons sit at its bottom.
            let body = (ui.available_height() - footer).max(120.0);
            egui::ScrollArea::vertical().max_height(body).min_scrolled_height(body).auto_shrink([false, false]).show(
                ui,
                |ui| {
                    let changed = match self.tab {
                        Tab::Tool => self.tool_tab(ui, session, tokens, units),
                        Tab::MultiAxis => self.multi_axis_tab(ui, tokens),
                        Tab::Geometry => self.geometry_tab(ui, session, tokens),
                        Tab::Heights => self.heights_tab(ui, session, tokens, units),
                        Tab::Passes => self.passes_tab(ui, units),
                        Tab::Linking => self.linking_tab(ui, units),
                    };
                    self.dirty |= changed;
                },
            );
            ui.separator();
            match (&self.error, &self.preview) {
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
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let ready = self.preview.is_ok();
                if ui.add_enabled(ready, egui::Button::new(RichText::new("  OK  ").strong())).clicked() {
                    result.0 = Some(("toolpath.contour".to_owned(), self.params()));
                }
                if ui.button("Cancel").clicked() {
                    result.1 = false;
                }
            });
        });
        result
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
                for (key, label, fallback) in [
                    ("feed", "Cutting feed", t.feed),
                    ("plunge_feed", "Plunge feed", t.plunge_feed),
                    ("lead_feed", "Lead-in / out feed", t.feed),
                ] {
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
        let mut changed = false;
        ui.label(
            RichText::new(
                "Click edges in the viewport to follow them, or a face to follow its outline. Click again to remove.",
            )
            .color(tokens.accent),
        );
        ui.add_space(4.0);
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
        ui.add_space(8.0);
        egui::Grid::new("tp-geometry").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            let sides = [("outside", "Outside"), ("inside", "Inside"), ("on", "On the line")];
            changed |= Self::row(ui, "Cut side", "Open contours: outside is the left of the edge's direction", |ui| {
                self.choice(ui, "side", &sides, "outside")
            });
            changed |= Self::row(ui, "", "", |ui| self.check(ui, "reverse", "Reverse direction", false));
            let mut wcs: Vec<(String, String)> = vec![(String::new(), "Active WCS".into())];
            wcs.extend(session.job.wcs.iter().map(|w| (w.id.clone(), w.name.clone())));
            let options: Vec<(&str, &str)> = wcs.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
            changed |= Self::row(ui, "Program in", "The WCS the toolpath is programmed in", |ui| {
                let c = self.choice(ui, "wcs", &options, "");
                if self.values.get("wcs").and_then(Value::as_str) == Some("") {
                    self.values.remove("wcs");
                }
                c
            });
        });
        changed
    }

    fn heights_tab(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens, u: DisplayUnits) -> bool {
        let mut changed = false;
        for (prefix, label, icon, help) in HEIGHT_ROWS {
            let (_, default_ref, default_offset) =
                HEIGHTS.iter().find(|(p, _, _)| p == prefix).copied().unwrap_or((prefix, "model_top", 0.0));
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
            changed |= Self::row(ui, "Tolerance", "Chord tolerance for arcs and offsets", |ui| {
                len(ui, self, "tolerance", &u.plain(0.01))
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
