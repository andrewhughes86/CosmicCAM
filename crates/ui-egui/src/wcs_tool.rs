//! The WCS panel: defines a new WCS (`setup.wcs.create`) or re-defines the active one
//! (`setup.wcs.align`) through the same two groups the commands take. **Orientation** picks
//! how the axes are set and the geometry for each selected axis, with a flip per axis;
//! **Origin** picks where zero goes. Selections come from clicks in the viewport; the result
//! is previewed live and the command runs on OK.

use std::collections::BTreeMap;

use CosmicCAM_engine::session::{edge_ref, face_ref};
use CosmicCAM_engine::{Session, cmd};
use CosmicCAM_geom::Vec3;
use egui::{RichText, Ui};
use serde_json::{Map, Value, json};

use crate::Request;
use crate::theme::Tokens;
use crate::ui_kit::Window;
use crate::viewport::{Accept, PickHit, PickMode, independent};

/// Orientation modes offered in the panel, with their labels (vectors are for scripts).
const ORIENTATIONS: &[(&str, &str)] = &[
    ("model", "Model axes"),
    ("z_x", "Select Z and X axes"),
    ("z_y", "Select Z and Y axes"),
    ("x_y", "Select X and Y axes"),
];
/// Origin modes, with their labels.
const ORIGINS: &[(&str, &str)] = &[
    ("model", "Model origin"),
    ("box", "Point on model box"),
    ("stock", "Point on stock box"),
    ("point", "Selected point"),
    ("intersection", "Intersection of selections"),
];

/// A selection made in the viewport.
#[derive(Clone, Debug, PartialEq)]
enum Picked {
    Face { model: String, face: usize, normal: Option<Vec3> },
    Edge { model: String, edge: usize },
    Point(Vec3),
}

impl Picked {
    fn describe(&self, units: CosmicCAM_engine::units::DisplayUnits) -> String {
        match self {
            Self::Face { model, face, .. } => format!("{model} face {face}"),
            Self::Edge { model, edge } => format!("{model} edge {edge}"),
            Self::Point(p) => units.fmt_point(p.to_array()),
        }
    }
}

pub struct WcsTool {
    /// `setup.wcs.create` or `setup.wcs.align`.
    command: &'static str,
    /// Index into ORIENTATIONS / ORIGINS; None keeps the current group (align only).
    orientation: Option<usize>,
    origin: Option<usize>,
    box_point: usize,
    flip: [bool; 3],
    /// Selections by parameter name.
    picks: BTreeMap<&'static str, Picked>,
    /// The parameter the next click fills.
    active: Option<&'static str>,
    name: String,
    activate: bool,
    preview: Option<(Vec3, [Vec3; 3])>,
    error: Option<String>,
    open: bool,
    /// The user's display units, for picked points.
    units: CosmicCAM_engine::units::DisplayUnits,
}

impl WcsTool {
    pub fn new(command: &'static str, session: &Session) -> Self {
        let align = command == "setup.wcs.align";
        let has_part = !session.job.part_bounds().is_empty();
        let mut tool = Self {
            command,
            orientation: if align { None } else { Some(1) },
            // Stock box top centre when there is stock, else the model box, else world zero.
            origin: if align {
                None
            } else if session.job.stock.is_some() {
                Some(2)
            } else if has_part {
                Some(1)
            } else {
                Some(0)
            },
            box_point: cmd::WCS_BOX_POINTS.iter().position(|p| *p == "top_center").unwrap_or(0),
            flip: [false; 3],
            picks: BTreeMap::new(),
            active: None,
            name: String::new(),
            activate: true,
            preview: None,
            error: None,
            open: true,
            units: session.prefs.units,
        };
        tool.active = tool.next_empty();
        tool.refresh_preview(session);
        tool
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn fail(&mut self, message: String) {
        self.error = Some(message);
    }

    fn orientation_id(&self) -> Option<&'static str> {
        self.orientation.and_then(|i| ORIENTATIONS.get(i)).map(|(id, _)| *id)
    }

    fn origin_id(&self) -> Option<&'static str> {
        self.origin.and_then(|i| ORIGINS.get(i)).map(|(id, _)| *id)
    }

    /// The selections the current modes use: (parameter, label, required).
    fn slots(&self) -> Vec<(&'static str, &'static str, bool)> {
        let mut out = Vec::new();
        match self.orientation_id() {
            Some("z_x") => out.extend([("orientation_z", "Z axis", true), ("orientation_x", "X axis", true)]),
            Some("z_y") => out.extend([("orientation_z", "Z axis", true), ("orientation_y", "Y axis", true)]),
            Some("x_y") => out.extend([("orientation_x", "X axis", true), ("orientation_y", "Y axis", true)]),
            _ => {}
        }
        match self.origin_id() {
            Some("point") => out.push(("origin_point", "Point", true)),
            Some("intersection") => {
                // Two planes meet in a line, so a third plane is needed once both are planes.
                let plane = |k: &str| matches!(self.picks.get(k), Some(Picked::Face { normal: Some(_), .. }));
                let third = plane("origin_a") && plane("origin_b");
                out.extend([("origin_a", "First", true), ("origin_b", "Second", true), ("origin_c", "Third", third)]);
            }
            _ => {}
        }
        out
    }

    fn next_empty(&self) -> Option<&'static str> {
        self.slots().into_iter().find(|(k, _, required)| *required && !self.picks.contains_key(k)).map(|(k, _, _)| k)
    }

    fn accept_for(key: &str) -> Accept {
        if key == "origin_point" { Accept::Point } else { Accept::Axis }
    }

    /// What the viewport should pick and show this frame.
    pub fn mode(&self) -> PickMode {
        let used: Vec<&str> = self.slots().into_iter().map(|(k, _, _)| k).collect();
        let mut mode = PickMode {
            accept: self.active.map(Self::accept_for),
            picked: Vec::new(),
            picked_edges: Vec::new(),
            picked_points: Vec::new(),
            preview: self.preview,
            independent_of: Vec::new(),
            path: None,
        };
        for (key, p) in self.picks.iter().filter(|(k, _)| used.contains(k)) {
            match p {
                Picked::Face { model, face, normal } => {
                    mode.picked.push((model.clone(), *face));
                    // Three planes for an intersection must point three different ways.
                    if key.starts_with("origin_") && Some(*key) != self.active {
                        mode.independent_of.extend(*normal);
                    }
                }
                Picked::Edge { model, edge } => mode.picked_edges.push((model.clone(), *edge)),
                Picked::Point(v) => mode.picked_points.push(*v),
            }
        }
        if !self.active.is_some_and(|k| k.starts_with("origin_")) {
            mode.independent_of.clear();
        }
        mode
    }

    /// A click in the viewport fills the active selection and moves to the next empty one.
    pub fn picked(&mut self, hit: PickHit, session: &Session) {
        let Some(key) = self.active else {
            return;
        };
        let pick = match (hit.edge, hit.face) {
            (Some(edge), _) => Picked::Edge { model: hit.model, edge },
            (None, _) if key == "origin_point" => Picked::Point(hit.point),
            (None, Some(face)) => Picked::Face { model: hit.model, face, normal: hit.normal },
            (None, None) => return,
        };
        if let (Picked::Face { normal: Some(n), .. }, true) = (&pick, key.starts_with("origin_")) {
            let others: Vec<Vec3> = self
                .picks
                .iter()
                .filter(|(k, _)| k.starts_with("origin_") && **k != key)
                .filter_map(|(_, p)| match p {
                    Picked::Face { normal, .. } => *normal,
                    _ => None,
                })
                .collect();
            if !independent(*n, &others) {
                self.error =
                    Some("pick a plane facing a new direction: not parallel to the planes already picked".into());
                return;
            }
        }
        self.picks.insert(key, pick);
        self.active = self.next_empty();
        self.refresh_preview(session);
    }

    fn params(&self) -> Value {
        let mut out = Map::new();
        if let Some(o) = self.orientation_id() {
            out.insert("orientation".into(), json!(o));
        }
        let flips: &[(usize, &str)] = match self.orientation_id() {
            Some("z_y") => &[(0, "flip_z"), (2, "flip_y")],
            Some("x_y") => &[(1, "flip_x"), (2, "flip_y")],
            _ => &[(0, "flip_z"), (1, "flip_x")],
        };
        for (i, key) in flips {
            if self.flip.get(*i).copied().unwrap_or(false) {
                out.insert((*key).into(), json!(true));
            }
        }
        if let Some(o) = self.origin_id() {
            out.insert("origin".into(), json!(o));
            if o == "box" || o == "stock" {
                out.insert(
                    "origin_box".into(),
                    json!(cmd::WCS_BOX_POINTS.get(self.box_point).copied().unwrap_or("top_center")),
                );
            }
        }
        for (key, _, _) in self.slots() {
            match self.picks.get(key) {
                Some(Picked::Face { model, face, .. }) => {
                    out.insert(key.into(), json!(face_ref(model, *face)));
                }
                Some(Picked::Edge { model, edge }) => {
                    // A circle picked as the origin point gives its centre.
                    let key = if key == "origin_point" { "origin_geometry" } else { key };
                    out.insert(key.into(), json!(edge_ref(model, *edge)));
                }
                Some(Picked::Point(p)) => {
                    out.insert(key.into(), json!(p.to_array()));
                }
                None => {}
            }
        }
        if self.command == "setup.wcs.create" {
            if !self.name.trim().is_empty() {
                out.insert("name".into(), json!(self.name.trim()));
            }
            if !self.activate {
                out.insert("activate".into(), json!(false));
            }
        }
        Value::Object(out)
    }

    fn complete(&self) -> bool {
        self.next_empty().is_none()
    }

    /// Runs the command on a copy of the session to show what it would make.
    fn refresh_preview(&mut self, session: &Session) {
        self.preview = None;
        self.error = None;
        if !self.complete() {
            return;
        }
        let mut trial = session.clone();
        match trial.execute(self.command, &self.params()) {
            Ok(w) => {
                let v = |k: &str| {
                    let a = w.get(k).and_then(Value::as_array);
                    let c = |i: usize| a.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(0.0) as f32;
                    Vec3::new(c(0), c(1), c(2))
                };
                self.preview = Some((v("origin"), [v("x_axis"), v("y_axis"), v("z_axis")]));
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Draws the panel. Returns the command to run when the user confirms.
    pub fn show(&mut self, ctx: &egui::Context, tokens: &Tokens, session: &Session) -> Option<Request> {
        self.units = session.prefs.units;
        let mut open = self.open;
        let mut cancel = false;
        let mut changed = false;
        let align = self.command == "setup.wcs.align";
        let title = if align { "Align WCS" } else { "Create WCS" };
        let confirmed = Window::tool(title)
            .show(ctx, tokens, &mut open, |ui| {
                ui.set_width(340.0);
                changed |= self.orientation_section(ui, tokens, align);
                ui.add_space(8.0);
                changed |= self.origin_section(ui, tokens, align);
                if !align {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut self.name).hint_text("WCS <n>").desired_width(200.0));
                    });
                    ui.checkbox(&mut self.activate, "Make active");
                }
                ui.add_space(4.0);
                if let Some(e) = &self.error {
                    ui.label(RichText::new(e).color(tokens.error));
                } else if let Some(key) = self.active {
                    let hint = match Self::accept_for(key) {
                        Accept::Axis => "Click a plane (its normal), a cylinder (its axis) or an edge in the viewport.",
                        Accept::Point | Accept::Select => {
                            "Click a point on the model (corners snap) or a circular edge (its centre)."
                        }
                    };
                    ui.label(RichText::new(hint).color(tokens.accent));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let ready = self.complete() && self.error.is_none();
                    let ok = ui.add_enabled(ready, egui::Button::new(RichText::new("  OK  ").strong())).clicked();
                    cancel = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
                    ok
                })
                .inner
            })
            .unwrap_or(false);
        self.open = open && !cancel;
        if changed {
            self.active =
                self.active.filter(|k| self.slots().iter().any(|(s, _, _)| s == k)).or_else(|| self.next_empty());
            self.refresh_preview(session);
        }
        (confirmed && self.open).then(|| (self.command.to_owned(), self.params()))
    }

    /// A mode box; `keep` adds "Keep current" (align). True when the choice changed.
    fn mode_combo(ui: &mut Ui, id: &str, choice: &mut Option<usize>, options: &[(&str, &str)], keep: bool) -> bool {
        let before = *choice;
        let text = choice.and_then(|i| options.get(i)).map_or("Keep current", |(_, l)| *l);
        egui::ComboBox::from_id_salt(id).selected_text(text).width(220.0).show_ui(ui, |ui| {
            if keep {
                ui.selectable_value(choice, None, "Keep current");
            }
            for (i, (_, label)) in options.iter().enumerate() {
                ui.selectable_value(choice, Some(i), *label);
            }
        });
        *choice != before
    }

    fn orientation_section(&mut self, ui: &mut Ui, tokens: &Tokens, align: bool) -> bool {
        ui.label(RichText::new("Orientation").strong());
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Mode");
            changed |= Self::mode_combo(ui, "wcs-orientation", &mut self.orientation, ORIENTATIONS, align);
        });
        let axes: Vec<(&'static str, &'static str, usize)> = match self.orientation_id() {
            Some("z_x") => vec![("orientation_z", "Z axis", 0), ("orientation_x", "X axis", 1)],
            Some("z_y") => vec![("orientation_z", "Z axis", 0), ("orientation_y", "Y axis", 2)],
            Some("x_y") => vec![("orientation_x", "X axis", 1), ("orientation_y", "Y axis", 2)],
            _ => vec![],
        };
        for (key, label, flip) in axes {
            changed |= self.slot_row(ui, tokens, key, label, Some(flip));
        }
        // Model axes (or the kept axes) can still be flipped.
        if matches!(self.orientation_id(), Some("model") | None) {
            ui.horizontal(|ui| {
                for (i, label) in [(0, "Flip Z"), (1, "Flip X")] {
                    if let Some(f) = self.flip.get_mut(i) {
                        changed |= ui.checkbox(f, label).changed();
                    }
                }
            });
        }
        changed
    }

    fn origin_section(&mut self, ui: &mut Ui, tokens: &Tokens, align: bool) -> bool {
        ui.label(RichText::new("Origin").strong());
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Mode");
            changed |= Self::mode_combo(ui, "wcs-origin", &mut self.origin, ORIGINS, align);
        });
        match self.origin_id() {
            Some("box" | "stock") => {
                ui.horizontal(|ui| {
                    ui.label("Box point");
                    let before = self.box_point;
                    let text = cmd::WCS_BOX_POINTS.get(self.box_point).map_or(String::new(), |p| p.replace('_', " "));
                    egui::ComboBox::from_id_salt("wcs-box-point").selected_text(text).width(200.0).show_ui(ui, |ui| {
                        for (i, p) in cmd::WCS_BOX_POINTS.iter().enumerate() {
                            ui.selectable_value(&mut self.box_point, i, p.replace('_', " "));
                        }
                    });
                    changed |= self.box_point != before;
                });
            }
            Some("point") => changed |= self.slot_row(ui, tokens, "origin_point", "Point", None),
            Some("intersection") => {
                for (key, label) in
                    [("origin_a", "First"), ("origin_b", "Second"), ("origin_c", "Third (with two planes)")]
                {
                    changed |= self.slot_row(ui, tokens, key, label, None);
                }
            }
            _ => {}
        }
        changed
    }

    /// One selection row: click it to make it the next one picked; × clears it; Flip
    /// reverses the axis it sets. True when something changed.
    fn slot_row(&mut self, ui: &mut Ui, tokens: &Tokens, key: &'static str, label: &str, flip: Option<usize>) -> bool {
        let active = self.active == Some(key);
        let units = self.units;
        let status = self.picks.get(key).map_or_else(|| "click to select".to_owned(), |p| p.describe(units));
        let text = RichText::new(format!("{label}: {status}"));
        let text = if active { text.color(tokens.accent).strong() } else { text };
        let mut changed = false;
        ui.horizontal(|ui| {
            if ui.add(egui::Button::selectable(active, text).min_size(egui::vec2(220.0, 0.0))).clicked() {
                self.active = if active { None } else { Some(key) };
            }
            if self.picks.contains_key(key) && ui.small_button("×").on_hover_text("Select again").clicked() {
                self.picks.remove(key);
                self.active = Some(key);
                changed = true;
            }
            if let Some(i) = flip
                && let Some(f) = self.flip.get_mut(i)
            {
                changed |= ui.checkbox(f, "Flip").changed();
            }
        });
        changed
    }
}
