//! A dialog generated from a command's parameter schema. Interactive tools that only need
//! values typed in use it: the form collects the values, the app runs the command with
//! them, and the form stays open showing the error if the command refuses them.

use CosmicCAM_engine::{CommandSpec, Param, ParamKind, Session};
use egui::{RichText, Ui};
use serde_json::{Map, Value, json};

use crate::Request;
use crate::theme::Tokens;
use crate::ui_kit::Window;

/// Shown in choice boxes for an optional parameter left unset.
const DEFAULT_LABEL: &str = "(default)";

enum Field {
    /// Strings and numbers, typed as text; empty means unset.
    Text(String),
    Bool(Option<bool>),
    Choice(Option<usize>),
    Vec3([String; 3]),
    /// A WCS picked from the job, for `setup.wcs.*` commands' `id`.
    Wcs(Option<usize>),
}

pub struct ParamForm {
    spec: &'static CommandSpec,
    fields: Vec<(&'static Param, Field)>,
    /// (id, name) of the job's WCS when the form opened.
    wcs: Vec<(String, String)>,
    error: Option<String>,
    open: bool,
}

impl ParamForm {
    pub fn new(spec: &'static CommandSpec, session: &Session) -> Self {
        let wcs: Vec<(String, String)> = session.job.wcs.iter().map(|w| (w.id.clone(), w.name.clone())).collect();
        let active = session.job.active_wcs.as_deref().and_then(|a| wcs.iter().position(|(id, _)| id == a));
        let fields = spec
            .params
            .iter()
            .map(|p| {
                let field = match p.kind {
                    _ if p.name == "id" && spec.id.starts_with("setup.wcs.") => Field::Wcs(active),
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
        Self { spec, fields, wcs, error: None, open: true }
    }

    /// The command failed with these values; keep the form open and say why.
    pub fn fail(&mut self, message: String) {
        self.error = Some(message);
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Draws the form. Returns the command to run when the user confirms.
    pub fn show(&mut self, ctx: &egui::Context, tokens: &Tokens) -> Option<Request> {
        let mut open = self.open;
        let mut cancel = false;
        let confirmed = Window::dialog(self.spec.label)
            .show(ctx, tokens, &mut open, |ui| {
                ui.set_min_width(360.0);
                ui.label(RichText::new(self.spec.tooltip).color(tokens.text_muted));
                ui.add_space(8.0);
                egui::Grid::new(("param-form", self.spec.id)).num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    for (param, field) in &mut self.fields {
                        let label = ui.label(label_for(param));
                        label.on_hover_text(param.doc);
                        edit_field(ui, self.spec.id, param, field, &self.wcs);
                        ui.end_row();
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
                            text.parse().map_err(|_| format!("{} needs three numbers", label_for(p)))?
                        };
                    }
                    Some(json!(xyz))
                }
                Field::Wcs(i) => i.and_then(|i| self.wcs.get(i)).map(|(id, _)| json!(id)),
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

/// "z_axis" → "Z axis".
fn label_for(p: &Param) -> String {
    let words = p.name.replace('_', " ");
    let mut chars = words.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn edit_field(ui: &mut Ui, command: &str, p: &Param, field: &mut Field, wcs: &[(String, String)]) {
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
        Field::Wcs(i) => {
            let options: Vec<String> = wcs.iter().map(|(_, name)| name.clone()).collect();
            combo(ui, "wcs", i, &options, !p.required);
        }
    }
}
