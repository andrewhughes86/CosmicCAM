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
            let row = ui.selectable_label(selected, &node.label);
            if row.clicked() {
                select(requests);
            }
            // Double-clicking a toolpath opens it for editing.
            if row.double_clicked() && node.id.starts_with("toolpath.") {
                requests.push(("toolpath.edit".into(), json!({ "id": node.id })));
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
    let node = session.view.selection.as_deref().and_then(|id| session.selection_node(id));
    let (title, rows) = match &node {
        Some(n) => (n.label.clone(), n.properties.clone()),
        None => {
            let c = &session.view.camera;
            (
                "Viewport".to_owned(),
                vec![
                    ("Projection".into(), c.projection.id().into()),
                    ("Target".into(), session.prefs.units.fmt_point(c.target.to_array())),
                    ("Distance".into(), session.prefs.units.fmt_len(f64::from(c.distance))),
                    ("Yaw / pitch".into(), format!("{:.1}° / {:.1}°", c.yaw.to_degrees(), c.pitch.to_degrees())),
                ],
            )
        }
    };
    ui.label(RichText::new(title).strong());
    if node.is_none() {
        ui.label(
            RichText::new("Select an item in the explorer, or click a face, edge or corner in the viewport.")
                .color(tokens.text_muted),
        );
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

/// Per-window state of the tool library: the row under the pointer, previewed in place of the
/// selection while hovered.
#[derive(Default)]
pub struct LibraryState {
    hovered: Option<u32>,
}

/// Machines and the active machine's tool library, with a to-scale preview of the tool under
/// the pointer (or the selected one). Every button runs a command; the selected tool is the
/// session selection, so the inspector shows it and edits start from it. Up and Down step
/// through the tools while the pointer is over the window.
pub fn library(ui: &mut Ui, session: &Session, tokens: &Tokens, state: &mut LibraryState, requests: &mut Vec<Request>) {
    let lib = &session.library;
    let u = session.prefs.units;
    let Some(machine) = lib.active_machine() else {
        ui.label(RichText::new("No machine yet.").color(tokens.text_muted));
        if ui.button("New machine…").clicked() {
            requests.push(("machine.create".into(), Value::Null));
        }
        return;
    };
    let selected = session
        .view
        .selection
        .as_deref()
        .and_then(|id| session.tool_by_ref(id))
        .filter(|(m, _)| m.id == machine.id)
        .map(|(_, t)| t.number);
    let select = |requests: &mut Vec<Request>, n: u32| {
        requests.push(("selection.set".into(), json!({ "id": CosmicCAM_engine::library::tool_ref(&machine.id, n) })));
    };

    ui.horizontal(|ui| {
        ui.label(RichText::new("Machine").strong());
        egui::ComboBox::from_id_salt("library-machine").selected_text(machine.name.as_str()).width(200.0).show_ui(
            ui,
            |ui| {
                for m in &lib.machines {
                    if ui.selectable_label(m.id == machine.id, m.name.as_str()).clicked() && m.id != machine.id {
                        requests.push(("machine.activate".into(), json!({ "id": m.id })));
                    }
                }
            },
        );
        if ui.small_button("New…").on_hover_text("Add a machine").clicked() {
            requests.push(("machine.create".into(), Value::Null));
        }
        if ui.small_button("Edit…").on_hover_text("Edit this machine").clicked() {
            requests.push(("machine.edit".into(), Value::Null));
        }
        let [x, y, z] = machine.travel;
        ui.label(
            RichText::new(format!(
                "Travel {} × {} × {} {} · {} rpm · {} · {}",
                u.plain(x),
                u.plain(y),
                u.plain(z),
                u.length(),
                machine.max_spindle_rpm,
                u.fmt_rate(machine.max_feed),
                if machine.tool_capacity == 0 {
                    "manual tool change".to_owned()
                } else {
                    format!("{} pockets", machine.tool_capacity)
                }
            ))
            .small()
            .color(tokens.text_muted),
        );
    });
    ui.separator();

    // Up / Down move through the list while the pointer is over the library.
    if ui.ui_contains_pointer() && !ui.ctx().egui_wants_keyboard_input() && !machine.tools.is_empty() {
        let (up, down) = ui.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown)));
        if up || down {
            let i = selected.and_then(|n| machine.tools.iter().position(|t| t.number == n));
            let last = machine.tools.len() - 1;
            let next = match (i, down) {
                (None, _) => 0,
                (Some(i), true) => (i + 1).min(last),
                (Some(i), false) => i.saturating_sub(1),
            };
            if let Some(t) = machine.tools.get(next) {
                select(requests, t.number);
                state.hovered = None;
            }
        }
    }

    let preview_width = 290.0;
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            // A cap, not a fixed width: sizing to all the remaining space would overflow by the
            // spacing each frame and grow the window without end.
            ui.set_max_width((ui.available_width() - preview_width - 40.0).max(260.0));
            ui.horizontal_wrapped(|ui| {
                if ui.button("New end mill…").on_hover_text("Add a flat end mill").clicked() {
                    requests.push(("tool.create.flat".into(), Value::Null));
                }
                ui.add_enabled_ui(selected.is_some(), |ui| {
                    if ui.button("Edit…").clicked() {
                        requests.push(("tool.edit".into(), Value::Null));
                    }
                    if ui.button("Feeds…").on_hover_text("Spindle speed, feeds and depths of cut").clicked() {
                        requests.push(("tool.feeds".into(), Value::Null));
                    }
                    if ui.button("Copy").clicked()
                        && let Some(n) = selected
                    {
                        requests.push(("tool.duplicate".into(), json!({ "number": n })));
                    }
                    if ui.button("Delete…").clicked() {
                        requests.push(("tool.remove".into(), Value::Null));
                    }
                });
            });
            ui.add_space(4.0);
            if machine.tools.is_empty() {
                state.hovered = None;
                ui.label(RichText::new("No tools yet. Add one with New end mill.").color(tokens.text_muted));
                return;
            }
            let rate = |v: f64| if v > 0.0 { u.plain_rate(v) } else { "–".into() };
            let rpm = |v: f64| if v > 0.0 { format!("{v}") } else { "–".into() };
            let mut hovered = None;
            ScrollArea::vertical().id_salt("library-scroll").auto_shrink([true, true]).show(ui, |ui| {
                egui::Grid::new("library-tools").striped(true).num_columns(8).spacing([12.0, 4.0]).show(ui, |ui| {
                    for h in [
                        "T".to_owned(),
                        "Name".to_owned(),
                        "Type".to_owned(),
                        format!("Ø {}", u.length()),
                        "Flutes".to_owned(),
                        format!("LOC {}", u.length()),
                        "rpm".to_owned(),
                        format!("Feed {}", u.rate()),
                    ] {
                        ui.label(RichText::new(h).small().color(tokens.text_muted));
                    }
                    ui.end_row();
                    for t in &machine.tools {
                        let on = selected == Some(t.number);
                        let mut cells = vec![
                            ui.selectable_label(on, format!("T{}", t.number)),
                            ui.selectable_label(on, t.name.as_str()),
                        ];
                        for text in [t.kind.clone(), u.plain(t.diameter), t.flutes.to_string(), u.plain(t.flute_length)]
                        {
                            cells.push(ui.label(text));
                        }
                        cells.push(ui.label(rpm(t.spindle_rpm)));
                        cells.push(ui.label(rate(t.feed)));
                        ui.end_row();
                        if cells.iter().any(egui::Response::hovered) {
                            hovered = Some(t.number);
                        }
                        if cells.iter().any(egui::Response::clicked) {
                            select(requests, t.number);
                        }
                        if cells.iter().any(egui::Response::double_clicked) {
                            select(requests, t.number);
                            requests.push(("tool.edit".into(), Value::Null));
                        }
                    }
                });
            });
            state.hovered = hovered;
        });
        ui.separator();
        ui.vertical(|ui| {
            ui.set_width(preview_width);
            let shown = state.hovered.or(selected).and_then(|n| machine.tool(n));
            match shown {
                Some(t) => {
                    ui.label(RichText::new(format!("T{}  {}", t.number, t.name)).strong());
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
                        egui::vec2(preview_width, 300.0),
                    );
                    let facts = [
                        ("Flutes", t.flutes.to_string()),
                        ("Shank", u.fmt_len(t.shank_diameter)),
                        ("Material", t.material.clone()),
                        (
                            "Speed",
                            if t.spindle_rpm > 0.0 { format!("{} rpm", t.spindle_rpm) } else { "not set".into() },
                        ),
                        ("Feed", if t.feed > 0.0 { u.fmt_rate(t.feed) } else { "not set".into() }),
                    ];
                    egui::Grid::new("library-facts").num_columns(2).spacing([10.0, 2.0]).show(ui, |ui| {
                        for (k, v) in facts {
                            ui.label(RichText::new(k).small().color(tokens.text_muted));
                            ui.label(RichText::new(v).small());
                            ui.end_row();
                        }
                    });
                }
                None => {
                    ui.label(RichText::new("Point at a tool to preview it.").color(tokens.text_muted));
                }
            }
        });
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
