//! A dialog generated from a command's parameter schema. Interactive tools that only need
//! values typed in use it: the form collects the values, the app runs the command with
//! them, and the form stays open showing the error if the command refuses them.

use CosmicCAM_engine::units::DisplayUnits;
use CosmicCAM_engine::{CommandSpec, Param, ParamKind, Session};
use egui::{RichText, Ui};
use serde_json::{Map, Value, json};

use crate::Request;
use crate::theme::Tokens;
use crate::tool_view;
use crate::ui_kit::Window;

/// Whether a form defines a tool (and so draws it).
fn is_tool_form(id: &str) -> bool {
    id == "tool.create" || id.starts_with("tool.create.") || id == "tool.edit"
}

/// Shown in choice boxes for an optional parameter left unset.
const DEFAULT_LABEL: &str = "(default)";

enum Field {
    /// Strings and numbers, typed as text; empty means unset.
    Text(String),
    Bool(Option<bool>),
    Choice(Option<usize>),
    Vec3([String; 3]),
    /// One of the form's listed items: a WCS (`setup.wcs.*` ids) or a machine.
    Pick(Option<usize>),
}

pub struct ParamForm {
    spec: &'static CommandSpec,
    /// The type of the tool being edited (`tool.edit`).
    editing_kind: Option<String>,
    fields: Vec<(&'static Param, Field)>,
    /// (id, name) of what pickers choose from: the job's WCS or the machines.
    options: Vec<(String, String)>,
    /// Lengths and feeds are typed in these units and sent in millimetres.
    units: DisplayUnits,
    error: Option<String>,
    open: bool,
}

impl ParamForm {
    pub fn new(spec: &'static CommandSpec, session: &Session) -> Self {
        let machines = spec.id.starts_with("machine.") || spec.params.iter().any(|p| p.name == "machine");
        let (options, active): (Vec<(String, String)>, Option<&str>) = if machines {
            let list = session.library.machines.iter().map(|m| (m.id.clone(), m.name.clone())).collect();
            (list, session.library.active.as_deref())
        } else {
            let list = session.job.wcs.iter().map(|w| (w.id.clone(), w.name.clone())).collect();
            (list, session.job.active_wcs.as_deref())
        };
        let active = active.and_then(|a| options.iter().position(|(id, _)| id == a));
        let is_pick = |p: &Param| {
            (p.name == "id" && (spec.id.starts_with("setup.wcs.") || spec.id.starts_with("machine.")))
                || (p.name == "machine" && machines)
        };
        let units = session.prefs.units;
        let initial = prefill(spec, session);
        let fields = spec
            .params
            .iter()
            .map(|p| {
                let field = match p.kind {
                    _ if is_pick(p) => Field::Pick(active),
                    _ if initial.contains_key(p.name) => initial_field(p, initial.get(p.name), units),
                    ParamKind::Bool => Field::Bool(p.required.then_some(false)),
                    ParamKind::Choice(_) => Field::Choice(p.required.then_some(0)),
                    ParamKind::Vec3 if p.required => Field::Vec3(["0".into(), "0".into(), "0".into()]),
                    ParamKind::Vec3 => Field::Vec3(Default::default()),
                    ParamKind::Number { .. } | ParamKind::Integer { .. } if p.required => Field::Text("0".into()),
                    _ => Field::Text(String::new()),
                };
                (p, field)
            })
            .collect();
        let editing_kind = (spec.id == "tool.edit")
            .then(|| {
                session.view.selection.as_deref().and_then(|id| session.tool_by_ref(id)).map(|(_, t)| t.kind.clone())
            })
            .flatten();
        Self { spec, editing_kind, fields, options, units, error: None, open: true }
    }

    /// The command failed with these values; keep the form open and say why.
    pub fn fail(&mut self, message: String) {
        self.error = Some(message);
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Draws the form. Returns the command to run when the user confirms.
    pub fn show(&mut self, ctx: &egui::Context, tokens: &Tokens, session: &Session) -> Option<Request> {
        let mut open = self.open;
        let mut cancel = false;
        let confirmed = Window::dialog(self.spec.label)
            .show(ctx, tokens, &mut open, |ui| {
                ui.set_min_width(360.0);
                ui.label(RichText::new(self.spec.tooltip).color(tokens.text_muted));
                ui.add_space(8.0);
                // Leave room for the title, notes and buttons on short windows.
                let max_height = (ctx.content_rect().height() - 240.0).clamp(160.0, 480.0);
                let units = self.units;
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        egui::ScrollArea::vertical().max_height(max_height).show(ui, |ui| {
                            egui::Grid::new(("param-form", self.spec.id)).num_columns(3).spacing([10.0, 6.0]).show(
                                ui,
                                |ui| {
                                    for (param, field) in &mut self.fields {
                                        let label = ui.label(label_for(param));
                                        label.on_hover_text(param.doc);
                                        edit_field(ui, self.spec.id, param, field, &self.options);
                                        ui.label(RichText::new(unit_suffix(param, units)).color(tokens.text_muted));
                                        ui.end_row();
                                    }
                                },
                            );
                        });
                    });
                    if is_tool_form(self.spec.id) {
                        // Spacing, not a separator: a vertical separator would stretch the window.
                        ui.add_space(12.0);
                        let tool = self.preview_tool(session);
                        let holder = tool.as_ref().and_then(|t| session.library.holder_of(t));
                        tool_view::draw(
                            ui,
                            tool.as_ref(),
                            holder,
                            units,
                            tokens,
                            egui::vec2(280.0, max_height.min(400.0)),
                        );
                    }
                });
                if let Some(e) = &self.error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(e).color(tokens.error));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let ok = ui.button(RichText::new("  OK  ").strong()).clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Enter));
                    cancel = ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
                    ok
                })
                .inner
            })
            .unwrap_or(false);
        self.open = open && !cancel;
        if !(confirmed && self.open) {
            return None;
        }
        match self.params() {
            Ok(params) => {
                self.error = None;
                Some((self.spec.id.to_owned(), params))
            }
            Err(e) => {
                self.error = Some(e);
                None
            }
        }
    }

    /// The tool the fields describe, built by the engine as the command would build it.
    fn preview_tool(&self, session: &Session) -> Option<CosmicCAM_engine::library::Tool> {
        let mut p = self.params().ok()?;
        let kind = match self.spec.id {
            "tool.edit" => self.editing_kind.clone()?,
            "tool.create" => p.get("type").and_then(Value::as_str).unwrap_or("flat").to_owned(),
            id => id.strip_prefix("tool.create.")?.to_owned(),
        };
        p.as_object_mut()?.insert("type".into(), json!(kind));
        CosmicCAM_engine::cmd::tool_preview(session, &p).ok()
    }

    /// The command's parameters from the fields; unset optional fields are left out.
    fn params(&self) -> Result<Value, String> {
        let mut out = Map::new();
        for (p, field) in &self.fields {
            let value = match field {
                Field::Text(t) if t.trim().is_empty() => None,
                Field::Text(t) => Some(match p.kind {
                    ParamKind::Number { .. } => {
                        json!(t.trim().parse::<f64>().map_err(|_| format!("{} must be a number", label_for(p)))?)
                    }
                    // Typed in the user's units, sent in millimetres.
                    ParamKind::Length { .. } | ParamKind::Rate { .. } => {
                        let v = t.trim().parse::<f64>().map_err(|_| format!("{} must be a number", label_for(p)))?;
                        json!(self.units.to_mm(v))
                    }
                    ParamKind::Integer { .. } => {
                        json!(t.trim().parse::<i64>().map_err(|_| format!("{} must be a whole number", label_for(p)))?)
                    }
                    _ => json!(t),
                }),
                Field::Bool(b) => b.map(Value::Bool),
                Field::Choice(i) => match p.kind {
                    ParamKind::Choice(choices) => i.and_then(|i| choices.get(i)).map(|c| json!(c)),
                    _ => None,
                },
                Field::Vec3(v) if v.iter().all(|c| c.trim().is_empty()) => None,
                Field::Vec3(v) => {
                    let mut xyz = [0.0f64; 3];
                    for (slot, text) in xyz.iter_mut().zip(v) {
                        let text = text.trim();
                        *slot = if text.is_empty() {
                            0.0
                        } else {
                            let v: f64 = text.parse().map_err(|_| format!("{} needs three numbers", label_for(p)))?;
                            self.units.to_mm(v)
                        };
                    }
                    Some(json!(xyz))
                }
                Field::Pick(i) => i.and_then(|i| self.options.get(i)).map(|(id, _)| json!(id)),
            };
            match value {
                Some(v) => {
                    out.insert(p.name.to_owned(), v);
                }
                None if p.required => return Err(format!("{} is required", label_for(p))),
                None => {}
            }
        }
        Ok(Value::Object(out))
    }
}

/// Values a form opens with: the selected tool for tool edits, the active machine for
/// machine edits, the next free number and a common size for new tools.
fn prefill(spec: &CommandSpec, session: &Session) -> Map<String, Value> {
    let selected_tool = session.view.selection.as_deref().and_then(|id| session.tool_by_ref(id));
    let source: Option<Value> = match spec.id {
        "tool.edit" | "tool.feeds" | "tool.duplicate" | "tool.remove" | "tool.preset.set" | "tool.preset.remove"
        | "tool.preset.apply" => selected_tool.map(|(m, t)| {
            let mut v = t.to_json(&m.id);
            // A preset form starts a new preset, not from the tool's name.
            if let Some(o) = v.as_object_mut().filter(|_| spec.id.starts_with("tool.preset")) {
                o.remove("name");
            }
            v
        }),
        // A common size in the user's units: 1/4 in or 6 mm.
        id if id == "tool.create" || id.starts_with("tool.create.") => session.library.active_machine().map(|m| {
            // A common size for the type, in the user's units.
            let inches = session.prefs.units == DisplayUnits::Inches;
            let diameter = match id.strip_prefix("tool.create.").unwrap_or("flat") {
                "face_mill" => {
                    if inches {
                        50.8
                    } else {
                        50.0
                    }
                }
                "slot_mill" => {
                    if inches {
                        25.4
                    } else {
                        25.0
                    }
                }
                "boring_bar" | "counterbore" | "countersink" => {
                    if inches {
                        12.7
                    } else {
                        12.0
                    }
                }
                "chamfer" | "dovetail" | "radius_mill" | "spot_drill" => {
                    if inches {
                        9.525
                    } else {
                        10.0
                    }
                }
                "tap" | "reamer" | "drill" | "thread_mill" => {
                    if inches {
                        6.35
                    } else {
                        6.0
                    }
                }
                _ => {
                    if inches {
                        6.35
                    } else {
                        6.0
                    }
                }
            };
            json!({ "number": m.next_tool_number(), "diameter": diameter, "type": "flat" })
        }),
        "machine.edit" => session.library.active_machine().map(|m| {
            let mut v = json!({
                "name": m.name, "description": m.description, "kind": m.kind,
                "travel_x": m.travel[0], "travel_y": m.travel[1], "travel_z": m.travel[2],
                "max_spindle_rpm": m.max_spindle_rpm, "max_feed": m.max_feed, "tool_capacity": m.tool_capacity,
            });
            if let Some(k) = &m.kinematics {
                v["layout"] = json!(k.layout);
                v["primary_axis"] = json!(k.primary.name);
                v["primary_min"] = json!(k.primary.min);
                v["primary_max"] = json!(k.primary.max);
                v["primary_reversed"] = json!(k.primary.reversed);
                if let Some(sec) = &k.secondary {
                    v["secondary_axis"] = json!(sec.name);
                    v["secondary_min"] = json!(sec.min);
                    v["secondary_max"] = json!(sec.max);
                    v["secondary_reversed"] = json!(sec.reversed);
                }
                v["primary_centre"] = json!(k.primary_centre);
                v["secondary_centre"] = json!(k.secondary_centre);
                v["pivot_length"] = json!(k.pivot_length);
            }
            v
        }),
        _ => None,
    };
    let mut out = Map::new();
    if let Some(Value::Object(src)) = source {
        for p in spec.params {
            match src.get(p.name) {
                // Tool edits leave cutting data that is not set blank, not "0".
                Some(Value::Number(n))
                    if n.as_f64() == Some(0.0) && spec.id.starts_with("tool.") && p.name != "number" => {}
                Some(v) if !v.is_null() => {
                    out.insert(p.name.to_owned(), v.clone());
                }
                _ => {}
            }
        }
    }
    out
}

/// The unit shown after a field: "in", "mm/min", or nothing.
fn unit_suffix(p: &Param, units: DisplayUnits) -> &'static str {
    match p.kind {
        ParamKind::Length { .. } | ParamKind::Vec3 => units.length(),
        ParamKind::Rate { .. } => units.rate(),
        _ if p.name == "spindle_rpm" => "rpm",
        _ => "",
    }
}

/// A field showing `value` (millimetres, shown in `units` for lengths and feeds).
fn initial_field(p: &Param, value: Option<&Value>, units: DisplayUnits) -> Field {
    if let (ParamKind::Length { .. } | ParamKind::Rate { .. }, Some(mm)) = (p.kind, value.and_then(Value::as_f64)) {
        return Field::Text(units.plain(mm));
    }
    let text = |v: &Value| match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.as_f64().map_or_else(
            || n.to_string(),
            |f| {
                let s = format!("{f:.4}");
                s.trim_end_matches('0').trim_end_matches('.').to_owned()
            },
        ),
        other => other.to_string(),
    };
    match (p.kind, value) {
        (ParamKind::Vec3, Some(Value::Array(a))) => {
            let c = |k: usize| a.get(k).and_then(Value::as_f64).map_or_else(String::new, |mm| units.plain(mm));
            Field::Vec3([c(0), c(1), c(2)])
        }
        (ParamKind::Bool, v) => Field::Bool(v.and_then(Value::as_bool)),
        (ParamKind::Choice(choices), v) => {
            Field::Choice(v.and_then(Value::as_str).and_then(|s| choices.iter().position(|c| *c == s)))
        }
        (_, Some(v)) => Field::Text(text(v)),
        (_, None) => Field::Text(String::new()),
    }
}

/// "z_axis" → "Z axis".
fn label_for(p: &Param) -> String {
    let words = p.name.replace('_', " ");
    let mut chars = words.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn edit_field(ui: &mut Ui, command: &str, p: &Param, field: &mut Field, list: &[(String, String)]) {
    let combo = |ui: &mut Ui, salt: &str, current: &mut Option<usize>, options: &[String], optional: bool| {
        let text = current.and_then(|i| options.get(i)).map_or(DEFAULT_LABEL, String::as_str).to_owned();
        egui::ComboBox::from_id_salt((command, p.name, salt)).selected_text(text).width(220.0).show_ui(ui, |ui| {
            if optional {
                ui.selectable_value(current, None, DEFAULT_LABEL);
            }
            for (i, o) in options.iter().enumerate() {
                ui.selectable_value(current, Some(i), o);
            }
        });
    };
    match field {
        Field::Text(t) => {
            ui.add(egui::TextEdit::singleline(t).desired_width(220.0).hint_text(if p.required {
                ""
            } else {
                "default"
            }));
        }
        Field::Bool(b) => {
            let options = ["Yes".to_owned(), "No".to_owned()];
            let mut i = b.map(|b| if b { 0 } else { 1 });
            combo(ui, "bool", &mut i, &options, !p.required);
            *b = i.map(|i| i == 0);
        }
        Field::Choice(i) => {
            let options: Vec<String> = match p.kind {
                ParamKind::Choice(c) => c.iter().map(|s| s.replace('_', " ")).collect(),
                _ => Vec::new(),
            };
            combo(ui, "choice", i, &options, !p.required);
        }
        Field::Vec3(v) => {
            ui.horizontal(|ui| {
                for (axis, text) in ["X", "Y", "Z"].iter().zip(v.iter_mut()) {
                    ui.add(egui::TextEdit::singleline(text).desired_width(56.0).hint_text(*axis));
                }
            });
        }
        Field::Pick(i) => {
            let options: Vec<String> = list.iter().map(|(_, name)| name.clone()).collect();
            combo(ui, "pick", i, &options, !p.required);
        }
    }
}
