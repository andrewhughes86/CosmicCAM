//! The setup editor: a panel docked at the right of the window while a setup is being created
//! or edited, or the active setup's stock defined. It sets the parameters of `setup.create`,
//! `setup.edit` or `setup.stock.box`; the stock is previewed see-through in the viewport as
//! the settings change, and OK runs the command.

use std::collections::HashMap;

use CosmicDMS_engine::Session;
use CosmicDMS_engine::units::DisplayUnits;
use CosmicDMS_geom::{Aabb, Vec3};
use egui::{RichText, Ui};
use serde_json::{Map, Value, json};

use crate::Request;
use crate::theme::Tokens;
use crate::ui_kit::{Panel, Surface};

/// What the panel makes.
#[derive(Clone, PartialEq, Eq)]
pub enum SetupTarget {
    /// A new setup.
    Create,
    /// Changes to this setup.
    Edit(String),
    /// The active setup's stock only.
    Stock,
}

/// Stock modes as the panel lists them, in `setup::STOCK_MODES` order.
const MODES: &[(&str, &str)] =
    &[("fit", "Fit to model"), ("size", "Fixed size"), ("box", "Two corners"), ("none", "No stock")];
const ALIGNS: &[(&str, &str)] =
    &[("top", "Top at the model top"), ("center", "Centred on the model"), ("bottom", "Bottom on the model bottom")];

pub struct SetupPanel {
    target: SetupTarget,
    name: String,
    machine: Option<String>,
    /// `origin` or a WCS id.
    wcs: String,
    mode: String,
    align: String,
    /// Stock numbers in millimetres, by field.
    nums: HashMap<&'static str, f64>,
    /// Text being typed into number fields, by field.
    text: HashMap<&'static str, String>,
    /// The stock the settings make, or why they cannot.
    preview: Result<Option<Aabb>, String>,
    dirty: bool,
    error: Option<String>,
}

impl SetupPanel {
    pub fn new(session: &Session, target: SetupTarget) -> Self {
        let existing = match &target {
            SetupTarget::Edit(id) => session.job.setup_by_id(id).cloned(),
            SetupTarget::Stock => {
                session.job.active_setup.as_deref().and_then(|id| session.job.setup_by_id(id)).cloned()
            }
            SetupTarget::Create => None,
        };
        let stock = match &target {
            SetupTarget::Create => None,
            _ => existing.as_ref().map_or(session.job.stock, |st| st.stock),
        };
        let mut nums = HashMap::new();
        // An existing stock opens as its two corners, so OK keeps it as it is.
        let mode = match stock {
            Some(b) => {
                for (k, v) in [
                    ("min_x", b.min.x),
                    ("min_y", b.min.y),
                    ("min_z", b.min.z),
                    ("max_x", b.max.x),
                    ("max_y", b.max.y),
                    ("max_z", b.max.z),
                ] {
                    nums.insert(k, f64::from(v));
                }
                let size = b.size();
                for (k, v) in [("size_x", size.x), ("size_y", size.y), ("size_z", size.z)] {
                    nums.insert(k, f64::from(v));
                }
                "box"
            }
            None if session.job.models_bounds().is_empty() => "none",
            None => "fit",
        };
        let n = session.job.setups.len() + 1;
        let mut panel = Self {
            name: existing.as_ref().map_or_else(|| format!("Setup {n}"), |st| st.name.clone()),
            machine: existing.as_ref().map(|st| st.machine.clone()).or_else(|| session.library.active.clone()),
            wcs: existing.as_ref().map_or_else(
                || session.job.active_wcs.clone().unwrap_or_else(|| "origin".into()),
                |st| st.wcs.clone().unwrap_or_else(|| "origin".into()),
            ),
            target,
            mode: mode.into(),
            align: "top".into(),
            nums,
            text: HashMap::new(),
            preview: Ok(None),
            dirty: true,
            error: None,
        };
        panel.refresh(session);
        panel
    }

    pub fn fail(&mut self, message: String) {
        self.error = Some(message);
    }

    /// The stock to draw see-through in the viewport.
    pub fn preview_stock(&self) -> Option<Aabb> {
        self.preview.as_ref().ok().copied().flatten()
    }

    fn num(&self, key: &str) -> f64 {
        self.nums.get(key).copied().unwrap_or(0.0)
    }

    fn command(&self) -> &'static str {
        match self.target {
            SetupTarget::Create => "setup.create",
            SetupTarget::Edit(_) => "setup.edit",
            SetupTarget::Stock => "setup.stock.box",
        }
    }

    fn params(&self) -> Value {
        let mut p = Map::new();
        match &self.target {
            SetupTarget::Create | SetupTarget::Edit(_) => {
                if let SetupTarget::Edit(id) = &self.target {
                    p.insert("id".into(), json!(id));
                }
                p.insert("name".into(), json!(self.name.trim()));
                if let Some(m) = &self.machine {
                    p.insert("machine".into(), json!(m));
                }
                p.insert("wcs".into(), json!(self.wcs));
            }
            SetupTarget::Stock => {}
        }
        p.insert("stock".into(), json!(self.mode));
        let v3 = |a: &str, b: &str, c: &str| json!([self.num(a), self.num(b), self.num(c)]);
        match self.mode.as_str() {
            "fit" => {
                p.insert("stock_margin_sides".into(), json!(self.num("sides")));
                p.insert("stock_margin_top".into(), json!(self.num("top")));
                p.insert("stock_margin_bottom".into(), json!(self.num("bottom")));
                p.insert("stock_round".into(), json!(self.num("round")));
            }
            "size" => {
                p.insert("stock_size".into(), v3("size_x", "size_y", "size_z"));
                p.insert("stock_align".into(), json!(self.align));
                p.insert("stock_margin_top".into(), json!(self.num("top")));
                p.insert("stock_round".into(), json!(self.num("round")));
            }
            "box" => {
                p.insert("stock_min".into(), v3("min_x", "min_y", "min_z"));
                p.insert("stock_max".into(), v3("max_x", "max_y", "max_z"));
            }
            _ => {}
        }
        Value::Object(p)
    }

    /// Recomputes the stock preview by running the command on a copy of the session.
    fn refresh(&mut self, session: &Session) {
        self.dirty = false;
        self.error = None;
        let mut p = self.params();
        // A name is only needed on OK; the preview shows the stock whatever it is called.
        if let Some(obj) = p.as_object_mut()
            && obj.get("name").and_then(Value::as_str).is_some_and(str::is_empty)
        {
            obj.insert("name".into(), json!("Setup"));
        }
        let mut trial = session.clone();
        self.preview = trial.execute(self.command(), &p).map_err(|e| e.to_string()).map(|r| {
            let stock = if self.target == SetupTarget::Stock { &r } else { &r["stock"] };
            let v = |key: &str| {
                let a = stock.get(key)?.as_array()?;
                let c: Vec<f32> = a.iter().filter_map(Value::as_f64).map(|x| x as f32).collect();
                match c.as_slice() {
                    [x, y, z] => Some(Vec3::new(*x, *y, *z)),
                    _ => None,
                }
            };
            v("min").zip(v("max")).map(|(min, max)| Aabb { min, max })
        });
    }

    /// Draws the panel. Returns the command to run on OK, and whether the panel stays open.
    pub fn show(&mut self, ui: &mut Ui, session: &Session, tokens: &Tokens) -> (Option<Request>, bool) {
        if self.dirty {
            self.refresh(session);
        }
        let mut result = (None, true);
        let u = session.prefs.units;
        Panel::right("setup-panel", Surface::SidePanel, 340.0).show(ui, tokens, |ui| {
            let title = match self.target {
                SetupTarget::Create => "New setup",
                SetupTarget::Edit(_) => "Edit setup",
                SetupTarget::Stock => "Stock",
            };
            ui.label(RichText::new(title).heading());
            ui.separator();
            let footer = 120.0;
            let body = (ui.available_height() - footer).max(120.0);
            egui::ScrollArea::vertical().max_height(body).min_scrolled_height(body).auto_shrink([false, false]).show(
                ui,
                |ui| {
                    let mut changed = false;
                    egui::Grid::new("setup-grid").num_columns(2).spacing([10.0, 7.0]).show(ui, |ui| {
                        if self.target != SetupTarget::Stock {
                            changed |= self.setup_rows(ui, session);
                        }
                        changed |= self.stock_rows(ui, u);
                    });
                    self.dirty |= changed;
                },
            );
            ui.separator();
            let named = self.target == SetupTarget::Stock || !self.name.trim().is_empty();
            ui.vertical_centered(|ui| match (&self.error, &self.preview) {
                (Some(e), _) => {
                    ui.label(RichText::new(e).color(tokens.error));
                }
                (None, Err(e)) => {
                    ui.label(RichText::new(e).color(tokens.warning));
                }
                (None, Ok(stock)) => {
                    if !named {
                        ui.label(RichText::new("Give the setup a name").color(tokens.warning));
                    }
                    ui.label(match stock {
                        Some(b) => format!("Stock {}", u.fmt_size(b.size().to_array())),
                        None => "No stock".to_owned(),
                    });
                }
            });
            ui.add_space(8.0);
            let (ok, cancel) = crate::toolpath_panel::ok_cancel(ui, named && self.preview.is_ok());
            if ok {
                result.0 = Some((self.command().to_owned(), self.params()));
            }
            if cancel {
                result.1 = false;
            }
        });
        result
    }

    fn setup_rows(&mut self, ui: &mut Ui, session: &Session) -> bool {
        let mut changed = false;
        ui.label("Setup name");
        changed |=
            ui.add(egui::TextEdit::singleline(&mut self.name).hint_text("Required").desired_width(190.0)).changed();
        ui.end_row();

        ui.label("Machine");
        let machines: Vec<(String, String)> =
            session.library.machines.iter().map(|m| (m.id.clone(), m.name.clone())).collect();
        let current = self
            .machine
            .as_ref()
            .and_then(|id| machines.iter().find(|(m, _)| m == id))
            .map_or("Choose a machine", |(_, n)| n.as_str())
            .to_owned();
        egui::ComboBox::from_id_salt("setup-machine").selected_text(current).width(190.0).show_ui(ui, |ui| {
            for (id, name) in &machines {
                if ui.selectable_label(self.machine.as_deref() == Some(id), name).clicked() {
                    self.machine = Some(id.clone());
                    changed = true;
                }
            }
        });
        ui.end_row();

        ui.label("WCS");
        let mut options = vec![("origin".to_owned(), "Origin".to_owned())];
        options.extend(session.job.wcs.iter().map(|w| (w.id.clone(), w.name.clone())));
        let current = options.iter().find(|(id, _)| *id == self.wcs).map_or("Origin", |(_, n)| n.as_str()).to_owned();
        egui::ComboBox::from_id_salt("setup-wcs").selected_text(current).width(190.0).show_ui(ui, |ui| {
            for (id, name) in &options {
                if ui.selectable_label(self.wcs == *id, name).clicked() {
                    self.wcs = id.clone();
                    changed = true;
                }
            }
        });
        ui.end_row();
        changed
    }

    fn stock_rows(&mut self, ui: &mut Ui, u: DisplayUnits) -> bool {
        let mut changed = false;
        ui.label(RichText::new("Stock").strong());
        ui.end_row();
        ui.label("Type");
        let current = MODES.iter().find(|(id, _)| *id == self.mode).map_or("", |(_, l)| *l);
        egui::ComboBox::from_id_salt("setup-stock").selected_text(current).width(190.0).show_ui(ui, |ui| {
            for (id, label) in MODES {
                if ui.selectable_label(self.mode == *id, *label).clicked() {
                    self.mode = (*id).to_owned();
                    changed = true;
                }
            }
        });
        ui.end_row();
        match self.mode.as_str() {
            "fit" => {
                changed |= self.length_row(ui, u, "Side margin", "sides", "Extra material on the four sides");
                changed |= self.length_row(ui, u, "Top margin", "top", "Extra material above the model");
                changed |= self.length_row(ui, u, "Bottom margin", "bottom", "Extra material below the model");
                changed |= self.length_row(ui, u, "Round up to", "round", "Round each size up to a multiple of this");
            }
            "size" => {
                changed |= self.length_row(ui, u, "Size X", "size_x", "");
                changed |= self.length_row(ui, u, "Size Y", "size_y", "");
                changed |= self.length_row(ui, u, "Size Z", "size_z", "");
                ui.label("Position");
                let current = ALIGNS.iter().find(|(id, _)| *id == self.align).map_or("", |(_, l)| *l);
                egui::ComboBox::from_id_salt("setup-align").selected_text(current).width(190.0).show_ui(ui, |ui| {
                    for (id, label) in ALIGNS {
                        if ui.selectable_label(self.align == *id, *label).clicked() {
                            self.align = (*id).to_owned();
                            changed = true;
                        }
                    }
                });
                ui.end_row();
                if self.align == "top" {
                    changed |= self.length_row(ui, u, "Top margin", "top", "Stock above the model top");
                }
                changed |= self.length_row(ui, u, "Round up to", "round", "Round each size up to a multiple of this");
            }
            "box" => {
                for (label, key) in [
                    ("Min X", "min_x"),
                    ("Min Y", "min_y"),
                    ("Min Z", "min_z"),
                    ("Max X", "max_x"),
                    ("Max Y", "max_y"),
                    ("Max Z", "max_z"),
                ] {
                    changed |= self.length_row(ui, u, label, key, "World coordinates");
                }
            }
            _ => {}
        }
        changed
    }

    /// One length field in display units. True when the value changed.
    fn length_row(&mut self, ui: &mut Ui, u: DisplayUnits, label: &str, key: &'static str, help: &str) -> bool {
        ui.label(label).on_hover_text(help);
        let shown = u.plain(self.num(key));
        let changed = ui
            .horizontal(|ui| {
                let entry = self.text.entry(key).or_insert(shown);
                let response = ui.add(egui::TextEdit::singleline(entry).desired_width(90.0));
                ui.label(u.length());
                if !response.changed() {
                    return false;
                }
                let text = entry.trim().to_owned();
                match (text.is_empty(), text.parse::<f64>()) {
                    (true, _) => {
                        self.nums.insert(key, 0.0);
                        true
                    }
                    (false, Ok(v)) if v.is_finite() => {
                        self.nums.insert(key, u.to_mm(v));
                        true
                    }
                    _ => false,
                }
            })
            .inner;
        ui.end_row();
        changed
    }
}
