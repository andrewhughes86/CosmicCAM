//! Explorer, inspector and console panes. They read the session and act only through commands.

use CosmicCAM_engine::Session;
use CosmicCAM_engine::session::OutlineNode;
use egui::collapsing_header::CollapsingState;
use egui::{RichText, ScrollArea, TextEdit, Ui};
use serde_json::{Value, json};

use crate::Request;
use crate::theme::Tokens;

pub fn explorer(ui: &mut Ui, session: &Session, requests: &mut Vec<Request>) {
    let selection = session.view.selection.as_deref();
    for node in session.outline() {
        outline_node(ui, &node, selection, requests);
    }
}

fn outline_node(ui: &mut Ui, node: &OutlineNode, selection: Option<&str>, requests: &mut Vec<Request>) {
    let selected = selection == Some(node.id.as_str());
    let select = |requests: &mut Vec<Request>| {
        requests.push(("selection.set".into(), json!({ "id": node.id })));
    };
    if node.children.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            if ui.selectable_label(selected, &node.label).clicked() {
                select(requests);
            }
        });
        return;
    }
    let id = ui.make_persistent_id(("outline", &node.id));
    CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| {
            if ui.selectable_label(selected, &node.label).clicked() {
                select(requests);
            }
        })
        .body(|ui| {
            for child in &node.children {
                outline_node(ui, child, selection, requests);
            }
        });
}

pub fn inspector(ui: &mut Ui, session: &Session, tokens: &Tokens) {
    let node = session.view.selection.as_deref().and_then(|id| session.outline_node(id));
    let (title, rows) = match &node {
        Some(n) => (n.label.clone(), n.properties.clone()),
        None => {
            let c = &session.view.camera;
            (
                "Viewport".to_owned(),
                vec![
                    ("Projection".into(), c.projection.id().into()),
                    ("Target".into(), format!("{:.1}, {:.1}, {:.1} mm", c.target.x, c.target.y, c.target.z)),
                    ("Distance".into(), format!("{:.1} mm", c.distance)),
                    ("Yaw / pitch".into(), format!("{:.1}° / {:.1}°", c.yaw.to_degrees(), c.pitch.to_degrees())),
                ],
            )
        }
    };
    ui.label(RichText::new(title).strong());
    if node.is_none() {
        ui.label(RichText::new("Select an item in the explorer to inspect it.").color(tokens.text_muted));
    }
    ui.separator();
    egui::Grid::new("inspector-grid").num_columns(2).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
        for (key, value) in rows {
            ui.label(RichText::new(key).color(tokens.text_muted));
            ui.label(value);
            ui.end_row();
        }
    });
}

#[derive(Default)]
pub struct ConsoleState {
    input: String,
    parse_error: Option<String>,
}

/// Parses `command.id {json params}`.
pub fn parse_console_line(line: &str) -> Result<Option<Request>, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let (id, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let rest = rest.trim();
    let params = if rest.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(rest).map_err(|e| format!("parameters are not valid JSON: {e}"))?
    };
    Ok(Some((id.to_owned(), params)))
}

pub fn console(ui: &mut Ui, session: &Session, tokens: &Tokens, state: &mut ConsoleState, requests: &mut Vec<Request>) {
    let input_height = 30.0;
    let log_height = (ui.available_height() - input_height).max(0.0);
    ScrollArea::vertical().max_height(log_height).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        for entry in session.log() {
            ui.horizontal_wrapped(|ui| {
                let params = if entry.params.is_null() { String::new() } else { entry.params.to_string() };
                ui.label(RichText::new(format!("> {} {params}", entry.command)).monospace());
                match &entry.outcome {
                    Ok(Value::Null) => {
                        ui.label(RichText::new("ok").monospace().color(tokens.success));
                    }
                    Ok(v) => {
                        ui.label(RichText::new(v.to_string()).monospace().color(tokens.success));
                    }
                    Err(e) => {
                        ui.label(RichText::new(e.to_string()).monospace().color(tokens.error));
                    }
                }
            });
        }
        if let Some(err) = &state.parse_error {
            ui.label(RichText::new(err).monospace().color(tokens.error));
        }
    });
    ui.horizontal(|ui| {
        let edit = ui.add(
            TextEdit::singleline(&mut state.input)
                .hint_text("command.id {\"param\": value}   e.g. view.camera.preset {\"preset\": \"top\"}")
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            match parse_console_line(&state.input) {
                Ok(Some(request)) => {
                    requests.push(request);
                    state.input.clear();
                    state.parse_error = None;
                }
                Ok(None) => {}
                Err(e) => state.parse_error = Some(e),
            }
            edit.request_focus();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_lines_parse() {
        assert_eq!(parse_console_line("  "), Ok(None));
        assert_eq!(parse_console_line("view.fit"), Ok(Some(("view.fit".into(), Value::Null))));
        assert_eq!(
            parse_console_line("view.theme.set {\"theme\": \"light\"}"),
            Ok(Some(("view.theme.set".into(), json!({"theme": "light"}))))
        );
        assert!(parse_console_line("view.theme.set {oops").is_err());
    }
}
