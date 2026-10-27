//! Explorer, inspector and console panes. They read the session and act only through commands.

use CosmicCAM_engine::Session;
use CosmicCAM_engine::session::OutlineNode;
use egui::collapsing_header::CollapsingState;
use egui::{RichText, ScrollArea, TextEdit, Ui};
use serde_json::{Value, json};

use crate::Request;
use crate::theme::Tokens;

/// What the explorer keeps between frames: the toolpaths picked with Ctrl and Shift, the
/// anchor of a Shift range, and the toolpath being dragged to a new place.
#[derive(Clone, Default)]
struct ExplorerState {
    picked: Vec<String>,
    anchor: Option<String>,
    dragging: Option<String>,
}

/// Per-frame drawing context of the explorer.
struct Draw<'a> {
    session: &'a Session,
    tokens: &'a Tokens,
    state: ExplorerState,
    /// Toolpath rows drawn this frame, top to bottom, for dropping a dragged one.
    rows: Vec<(String, egui::Rect)>,
}

impl Draw<'_> {
    fn picked(&self, id: &str) -> bool {
        self.state.picked.iter().any(|p| p == id)
    }

    /// The toolpaths a menu or the Delete key acts on: the picked ones, else the selection.
    fn targets(&self) -> Vec<String> {
        if !self.state.picked.is_empty() {
            return self.state.picked.clone();
        }
        self.session.view.selection.iter().filter(|s| s.starts_with("toolpath.")).cloned().collect()
    }
}

pub fn explorer(ui: &mut Ui, session: &Session, tokens: &Tokens, requests: &mut Vec<Request>) {
    let state_id = egui::Id::new("explorer-state");
    let mut state: ExplorerState = ui.data(|d| d.get_temp(state_id)).unwrap_or_default();
    // Toolpaths that no longer exist drop out of the picks.
    state.picked.retain(|id| session.job.toolpaths.iter().any(|t| &t.id == id));
    let mut draw = Draw { session, tokens, state, rows: Vec::new() };
    let selection = session.view.selection.as_deref();
    let no_drag = egui::scroll_area::ScrollSource {
        drag: egui::scroll_area::DragScroll::Never,
        ..egui::scroll_area::ScrollSource::ALL
    };
    ScrollArea::both().scroll_source(no_drag).auto_shrink([false, false]).show(ui, |ui| {
        for node in session.outline() {
            outline_node(ui, &node, &mut draw, selection, requests);
        }
    });
    // Delete removes the picked toolpaths (when no text field has the keyboard).
    let typing = ui.ctx().memory(|m| m.focused().is_some());
    if !typing && ui.input(|i| i.key_pressed(egui::Key::Delete)) {
        let targets = draw.targets();
        if !targets.is_empty() {
            requests.push(delete_toolpaths(&targets, session));
        }
    }
    drop_dragged(ui, &mut draw, requests);
    ui.data_mut(|d| d.insert_temp(state_id, draw.state));
}

/// The delete prompt for `ids`: one toolpath by name, several by count.
fn delete_toolpaths(ids: &[String], session: &Session) -> Request {
    let label = match ids {
        [one] => session.job.toolpaths.iter().find(|t| &t.id == one).map_or(one.clone(), |t| t.name.clone()),
        many => format!("{} toolpaths", many.len()),
    };
    ("ui.delete".into(), json!({ "command": "toolpath.remove", "ids": ids, "label": label }))
}

/// While a toolpath is dragged, a line shows where it will go; letting go moves it there.
fn drop_dragged(ui: &mut Ui, draw: &mut Draw<'_>, requests: &mut Vec<Request>) {
    let Some(dragged) = draw.state.dragging.clone() else { return };
    let pointer = ui.ctx().pointer_interact_pos();
    // The row under the pointer, and whether the drop goes after it.
    let target = pointer.and_then(|p| {
        draw.rows
            .iter()
            .min_by(|a, b| (a.1.center().y - p.y).abs().total_cmp(&(b.1.center().y - p.y).abs()))
            .map(|(id, rect)| (id.clone(), *rect, p.y > rect.center().y))
    });
    if let Some((_, rect, after)) = &target {
        let y = if *after { rect.bottom() + 1.0 } else { rect.top() - 1.0 };
        ui.painter().hline(rect.x_range(), y, egui::Stroke::new(2.0, draw.tokens.accent));
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    if ui.input(|i| i.pointer.any_released()) {
        draw.state.dragging = None;
        let Some((over, _, after)) = target else { return };
        let order: Vec<&str> = draw.session.job.toolpaths.iter().map(|t| t.id.as_str()).collect();
        let (Some(from), Some(at)) = (order.iter().position(|i| *i == dragged), order.iter().position(|i| *i == over))
        else {
            return;
        };
        let mut to = if after { at + 1 } else { at };
        if from < to {
            to -= 1;
        }
        if to != from {
            requests.push(("toolpath.move".into(), json!({ "id": dragged, "to": to })));
        }
    }
}

/// An eye button: open when the item is drawn, closed (struck through) when hidden.
fn eye(ui: &mut Ui, visible: bool, tokens: &Tokens) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 16.0), egui::Sense::click());
    let painter = ui.painter_at(rect);
    let c = rect.center();
    let colour = if response.hovered() {
        tokens.accent
    } else if visible {
        tokens.text
    } else {
        tokens.text_muted.gamma_multiply(0.7)
    };
    let stroke = egui::Stroke::new(1.3, colour);
    // The eye: two arcs meeting at the corners, and the pupil.
    let arc = |up: f32| -> Vec<egui::Pos2> {
        (0..=12)
            .map(|k| {
                let t = k as f32 / 12.0;
                let x = -6.5 + 13.0 * t;
                c + egui::vec2(x, up * 4.2 * (1.0 - (2.0 * t - 1.0).powi(2)))
            })
            .collect()
    };
    painter.add(egui::Shape::line(arc(-1.0), stroke));
    painter.add(egui::Shape::line(arc(1.0), stroke));
    if visible {
        painter.circle_filled(c, 2.2, colour);
    } else {
        painter.line_segment([c + egui::vec2(-6.0, 5.0), c + egui::vec2(6.0, -5.0)], stroke);
    }
    response.on_hover_text(if visible { "Shown: click to hide" } else { "Hidden: click to show" })
}

fn outline_node(
    ui: &mut Ui,
    node: &OutlineNode,
    draw: &mut Draw<'_>,
    selection: Option<&str>,
    requests: &mut Vec<Request>,
) {
    let is_toolpath = node.id.starts_with("toolpath.");
    let selected = selection == Some(node.id.as_str()) || (is_toolpath && draw.picked(&node.id));
    // Double-clicking a toolpath opens it for editing; a setup is made active and opened; an
    // NC program opens its post-processor.
    let open = |row: &egui::Response, requests: &mut Vec<Request>| {
        if !row.double_clicked() {
            return;
        }
        if node.id.starts_with("toolpath.") {
            requests.push(("toolpath.edit".into(), json!({ "id": node.id })));
        } else if node.id.starts_with("setup.") && !node.id.contains('/') {
            requests.push(("setup.activate".into(), json!({ "id": node.id })));
            requests.push(("setup.edit".into(), json!({ "id": node.id })));
        } else if let Some(machine) = node.id.strip_prefix(CosmicCAM_engine::session::PROGRAM_PREFIX) {
            requests.push(("machine.activate".into(), json!({ "id": machine })));
            requests.push(("post.processor.select".into(), Value::Null));
        }
    };
    // One row: the eye (or room for it), the icon, the label; toolpaths can be dragged.
    let row = |ui: &mut Ui, draw: &mut Draw<'_>, requests: &mut Vec<Request>| {
        match node.visible {
            Some(visible) => {
                if eye(ui, visible, draw.tokens).clicked() {
                    requests.push(toggle(node, visible, draw.session));
                }
            }
            None => ui.add_space(18.0),
        }
        row_icon(ui, node, draw);
        let response = ui.selectable_label(selected, &node.label);
        let response = if is_toolpath { response.interact(egui::Sense::click_and_drag()) } else { response };
        if response.clicked() {
            click(ui, node, draw, requests);
        }
        if is_toolpath {
            draw.rows.push((node.id.clone(), response.rect));
            if response.drag_started() {
                draw.state.dragging = Some(node.id.clone());
            }
        }
        open(&response, requests);
        context_menu(&response, node, draw, requests);
    };
    if node.children.is_empty() {
        ui.horizontal(|ui| row(ui, draw, requests));
        return;
    }
    // Ids stay unique where one WCS shows under several toolpaths.
    let id = ui.make_persistent_id(("outline", &node.id, ui.id()));
    CollapsingState::load_with_default_open(ui.ctx(), id, true).show_header(ui, |ui| row(ui, draw, requests)).body(
        |ui| {
            for child in &node.children {
                outline_node(ui, child, draw, selection, requests);
            }
        },
    );
}

/// The icon in front of a row: a toolpath's strategy, a WCS triad or the stock block.
fn row_icon(ui: &mut Ui, node: &OutlineNode, draw: &Draw<'_>) {
    let id = node.id.as_str();
    let name = if id.starts_with("toolpath.") {
        let strategy =
            draw.session.job.toolpaths.iter().find(|t| t.id == id).map(|t| t.strategy.as_str()).unwrap_or_default();
        CosmicCAM_engine::cmd::find(CosmicCAM_engine::toolpath::strategy_command(strategy))
            .map(crate::icon_svg::name_for)
    } else if id.starts_with("wcs.") {
        Some("explorer_wcs".to_owned())
    } else if id == "stock" || id.ends_with("/stock") {
        Some("setup_stock_box".to_owned())
    } else {
        None
    };
    let Some(name) = name else { return };
    let size = ui.text_style_height(&egui::TextStyle::Body).round().max(12.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    crate::ribbon::paint_named_icon(ui, rect, &name, draw.tokens, true);
}

/// A click on a row selects it; on toolpaths Ctrl adds or removes one and Shift picks the run
/// from the last one clicked.
fn click(ui: &Ui, node: &OutlineNode, draw: &mut Draw<'_>, requests: &mut Vec<Request>) {
    let modifiers = ui.input(|i| i.modifiers);
    let st = &mut draw.state;
    if node.id.starts_with("toolpath.") {
        let order: Vec<&str> = draw.session.job.toolpaths.iter().map(|t| t.id.as_str()).collect();
        if modifiers.command {
            if st.picked.is_empty()
                && let Some(sel) = draw.session.view.selection.as_ref().filter(|s| s.starts_with("toolpath."))
            {
                st.picked.push(sel.clone());
            }
            match st.picked.iter().position(|p| *p == node.id) {
                Some(i) => {
                    st.picked.remove(i);
                }
                None => st.picked.push(node.id.clone()),
            }
            st.anchor = Some(node.id.clone());
        } else if modifiers.shift
            && let Some(anchor) = st.anchor.clone().or_else(|| draw.session.view.selection.clone())
            && let (Some(a), Some(b)) =
                (order.iter().position(|i| *i == anchor), order.iter().position(|i| *i == node.id))
        {
            let (lo, hi) = (a.min(b), a.max(b));
            st.picked = order.get(lo..=hi).unwrap_or_default().iter().map(|s| (*s).to_owned()).collect();
        } else {
            st.picked.clear();
            st.anchor = Some(node.id.clone());
        }
    } else {
        st.picked.clear();
        st.anchor = None;
    }
    requests.push(("selection.set".into(), json!({ "id": node.id })));
}

/// The command an eye button runs for `node`.
fn toggle(node: &OutlineNode, visible: bool, session: &Session) -> Request {
    if node.id == CosmicCAM_engine::session::ORIGIN_ID {
        return ("view.display.set".into(), json!({ "origin": !visible }));
    }
    if node.id.starts_with("wcs.") {
        // Shown because the selection uses it: switching it on keeps it; off lets it go.
        let kept = session.view.shown_wcs.contains(&node.id);
        return ("view.wcs.visibility.set".into(), json!({ "id": node.id, "visible": !kept }));
    }
    if node.id == "stock" || node.id.ends_with("/stock") {
        // A setup's stock shows when its setup is the active one.
        let setup = node.id.trim_end_matches("/stock");
        if setup.starts_with("setup.") && !visible && session.job.active_setup.as_deref() != Some(setup) {
            return ("setup.activate".into(), json!({ "id": setup }));
        }
        return ("view.display.set".into(), json!({ "stock": !visible }));
    }
    let command = if node.id.starts_with("model.") { "model.visibility.set" } else { "toolpath.visibility.set" };
    (command.into(), json!({ "id": node.id }))
}

/// The right-click menu of an explorer row: what can be done with that item.
fn context_menu(row: &egui::Response, node: &OutlineNode, draw: &Draw<'_>, requests: &mut Vec<Request>) {
    let (session, tokens) = (draw.session, draw.tokens);
    let id = node.id.as_str();
    let name = node.label.trim_end_matches(" (active)").to_owned();
    let clip = session.clipboard.as_ref().map(CosmicCAM_engine::session::Clip::kind);
    crate::ui_kit::Menu::below(row).context(tokens, |ui| {
        let run = |command: &str, params: Value| vec![(command.to_owned(), params)];
        let rename = |command: &str| run("ui.rename", json!({ "command": command, "id": id, "name": name }));
        let delete = |command: &str| run("ui.delete", json!({ "command": command, "id": id, "label": name }));
        let paste = || run("job.clipboard.paste", json!({}));
        let show_label = |visible: Option<bool>| if visible == Some(false) { "Show" } else { "Hide" };
        match id {
            "job" => {
                item(ui, requests, "Save", run("job.save", Value::Null));
                item(ui, requests, "Save as…", run("job.save.as", Value::Null));
                if clip.is_some() {
                    item(ui, requests, "Paste", paste());
                }
            }
            "models" => {
                item(ui, requests, "Import model…", run("model.import", Value::Null));
                if clip == Some("model") {
                    item(ui, requests, "Paste model", paste());
                }
            }
            "setups" => {
                item(ui, requests, "Create setup…", run("setup.create", Value::Null));
                item(ui, requests, "Create WCS…", run("setup.wcs.create", Value::Null));
                if clip == Some("wcs") {
                    item(ui, requests, "Paste WCS", paste());
                }
            }
            "origin" => {
                let shown = session.prefs.display.origin;
                item(
                    ui,
                    requests,
                    if shown { "Hide" } else { "Show" },
                    run("view.display.set", json!({ "origin": !shown })),
                );
            }
            "fixtures" => {
                ui.label(RichText::new("Fixtures are not modelled yet").color(tokens.text_muted));
            }
            "programs" => {
                item(ui, requests, "Export G-code…", run("post.gcode.export", Value::Null));
            }
            pr if pr.starts_with(CosmicCAM_engine::session::PROGRAM_PREFIX) => {
                let machine = pr.trim_start_matches(CosmicCAM_engine::session::PROGRAM_PREFIX);
                let activate = ("machine.activate".to_owned(), json!({ "id": machine }));
                item(
                    ui,
                    requests,
                    "Post-processor…",
                    vec![activate.clone(), ("post.processor.select".into(), Value::Null)],
                );
                item(ui, requests, "Export G-code…", vec![activate.clone(), ("post.gcode.export".into(), Value::Null)]);
                item(ui, requests, "Check limits", vec![activate, ("post.limits.check".into(), Value::Null)]);
            }
            "toolpaths" => {
                let all = |visible: bool| -> Vec<Request> {
                    session
                        .job
                        .toolpaths
                        .iter()
                        .map(|t| ("toolpath.visibility.set".to_owned(), json!({ "id": t.id, "visible": visible })))
                        .collect()
                };
                item(ui, requests, "Show all", all(true));
                item(ui, requests, "Hide all", all(false));
                if clip == Some("toolpath") {
                    item(ui, requests, "Paste toolpath", paste());
                }
            }
            "stock" => {
                item(ui, requests, "Edit stock…", run("setup.stock.box", Value::Null));
                item(
                    ui,
                    requests,
                    if session.prefs.display.stock { "Hide stock" } else { "Show stock" },
                    run("view.display.set", json!({ "stock": !session.prefs.display.stock })),
                );
            }
            t if t.starts_with("toolpath.") && draw.picked(t) && draw.state.picked.len() > 1 => {
                let ids = draw.state.picked.clone();
                let show = |visible: bool| -> Vec<Request> {
                    ids.iter()
                        .map(|i| ("toolpath.visibility.set".to_owned(), json!({ "id": i, "visible": visible })))
                        .collect()
                };
                item(ui, requests, "Simulate these", run("sim.start", json!({ "toolpaths": ids })));
                item(ui, requests, "Trim…", run("toolpath.trim", json!({ "ids": ids })));
                if ids.iter().any(|i| session.job.toolpaths.iter().any(|t| &t.id == i && !t.trims.is_empty())) {
                    item(ui, requests, "Remove trims", run("toolpath.trim.remove", json!({ "ids": ids })));
                }
                item(ui, requests, "Show", show(true));
                item(ui, requests, "Hide", show(false));
                item(ui, requests, "Regenerate", run("toolpath.regenerate", json!({ "ids": ids })));
                ui.separator();
                item(ui, requests, &format!("Delete {} toolpaths", ids.len()), vec![delete_toolpaths(&ids, session)]);
            }
            t if t.starts_with("toolpath.") => {
                item(ui, requests, "Edit…", run("toolpath.edit", json!({ "id": id })));
                item(ui, requests, "Simulate", run("sim.start", json!({ "toolpaths": [id] })));
                item(ui, requests, "Trim…", run("toolpath.trim", json!({ "ids": [id] })));
                if session.job.toolpaths.iter().any(|t| t.id == id && !t.trims.is_empty()) {
                    item(ui, requests, "Remove trims", run("toolpath.trim.remove", json!({ "id": id })));
                }
                item(ui, requests, show_label(node.visible), run("toolpath.visibility.set", json!({ "id": id })));
                item(ui, requests, "Rename…", rename("toolpath.rename"));
                ui.separator();
                item(ui, requests, "Copy", run("job.clipboard.copy", json!({ "id": id })));
                if clip.is_some() {
                    item(ui, requests, "Paste", paste());
                }
                item(ui, requests, "Duplicate", run("toolpath.duplicate", json!({ "id": id })));
                ui.separator();
                item(ui, requests, "Move up", run("toolpath.move", json!({ "id": id, "by": -1 })));
                item(ui, requests, "Move down", run("toolpath.move", json!({ "id": id, "by": 1 })));
                item(ui, requests, "Regenerate", run("toolpath.regenerate", json!({ "ids": [id] })));
                item(ui, requests, "Save as template…", rename("toolpath.template.save"));
                ui.separator();
                item(ui, requests, "Delete", vec![delete_toolpaths(&[id.to_owned()], session)]);
            }
            m if m.starts_with("model.") => {
                item(ui, requests, show_label(node.visible), run("model.visibility.set", json!({ "id": id })));
                item(ui, requests, "Rename…", rename("model.rename"));
                ui.separator();
                item(ui, requests, "Copy", run("job.clipboard.copy", json!({ "id": id })));
                if clip.is_some() {
                    item(ui, requests, "Paste", paste());
                }
                ui.separator();
                item(ui, requests, "Delete", delete("model.remove"));
            }
            st if st.starts_with("setup.") && st.ends_with("/stock") => {
                let setup = st.trim_end_matches("/stock");
                let edit = vec![
                    ("setup.activate".to_owned(), json!({ "id": setup })),
                    ("setup.stock.box".to_owned(), Value::Null),
                ];
                item(ui, requests, "Edit stock…", edit);
                item(
                    ui,
                    requests,
                    if session.prefs.display.stock { "Hide stock" } else { "Show stock" },
                    run("view.display.set", json!({ "stock": !session.prefs.display.stock })),
                );
            }
            st if st.starts_with("setup.") => {
                let edit = vec![
                    ("setup.activate".to_owned(), json!({ "id": id })),
                    ("setup.edit".to_owned(), json!({ "id": id })),
                ];
                item(ui, requests, "Edit…", edit);
                item(ui, requests, "Activate", run("setup.activate", json!({ "id": id })));
                item(ui, requests, "Rename…", rename("setup.edit"));
                if clip == Some("toolpath") {
                    item(ui, requests, "Paste toolpath", paste());
                }
                ui.separator();
                item(ui, requests, "Delete", delete("setup.remove"));
            }
            w if w.starts_with("wcs.") => {
                item(ui, requests, "Activate", run("setup.wcs.activate", json!({ "id": id })));
                let then = vec![
                    ("setup.wcs.activate".to_owned(), json!({ "id": id })),
                    ("setup.wcs.align".to_owned(), Value::Null),
                ];
                item(ui, requests, "Align…", then);
                let then = vec![
                    ("setup.wcs.activate".to_owned(), json!({ "id": id })),
                    ("setup.wcs.offset".to_owned(), Value::Null),
                ];
                item(ui, requests, "Offset…", then);
                let then = vec![
                    ("setup.wcs.activate".to_owned(), json!({ "id": id })),
                    ("setup.wcs.rotate".to_owned(), Value::Null),
                ];
                item(ui, requests, "Rotate…", then);
                item(ui, requests, "Rename…", rename("setup.wcs.rename"));
                item(
                    ui,
                    requests,
                    if session.prefs.display.wcs { "Hide WCS markers" } else { "Show WCS markers" },
                    run("view.display.set", json!({ "wcs": !session.prefs.display.wcs })),
                );
                ui.separator();
                item(ui, requests, "Copy", run("job.clipboard.copy", json!({ "id": id })));
                if clip.is_some() {
                    item(ui, requests, "Paste", paste());
                }
                ui.separator();
                item(ui, requests, "Delete", delete("setup.wcs.remove"));
            }
            _ => {
                ui.label(RichText::new("Nothing to do here").color(tokens.text_muted));
            }
        }
    });
}

/// One menu item: runs `run` when clicked and closes the menu.
fn item(ui: &mut Ui, requests: &mut Vec<Request>, label: &str, run: Vec<Request>) {
    if ui.button(label).clicked() {
        requests.extend(run);
        ui.close();
    }
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

/// Per-window state of the tool library: the row under the pointer (previewed in place of the
/// selection while hovered), the type filter and the search text.
#[derive(Default)]
pub struct LibraryState {
    hovered: Option<u32>,
    kind: Option<String>,
    search: String,
}

/// Machines and the active machine's tool library: every tool type, filtered by type and search,
/// with a to-scale preview of the tool under the pointer (or the selected one) drawn from its
/// real geometry and holder, its type's values and its cutting presets, and the holder library.
/// Every button runs a command; the selected tool is the session selection, so the inspector
/// shows it and edits start from it. Up and Down step through the tools shown while the pointer
/// is over the window.
pub fn library(ui: &mut Ui, session: &Session, tokens: &Tokens, state: &mut LibraryState, requests: &mut Vec<Request>) {
    use CosmicCAM_engine::tooling::{TYPES, tool_type};
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
        let axes = machine
            .kinematics
            .as_ref()
            .map_or("3-axis".to_owned(), |k| format!("{}-axis {}", 3 + k.names().len(), k.names().join("")));
        ui.label(
            RichText::new(format!(
                "{axes} · travel {} × {} × {} {} · {} rpm · {} · {}",
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

    // The tools shown: of the chosen type, matching the search.
    let search = state.search.trim().to_lowercase();
    let shown: Vec<&CosmicCAM_engine::library::Tool> = machine
        .tools
        .iter()
        .filter(|t| state.kind.as_deref().is_none_or(|k| t.kind == k))
        .filter(|t| {
            search.is_empty()
                || [t.name.as_str(), t.type_label(), t.vendor.as_str(), t.product_id.as_str(), t.notes.as_str()]
                    .iter()
                    .any(|f| f.to_lowercase().contains(&search))
                || format!("t{}", t.number) == search
        })
        .collect();

    // Up / Down move through the list while the pointer is over the library.
    if ui.ui_contains_pointer() && !ui.ctx().egui_wants_keyboard_input() && !shown.is_empty() {
        let (up, down) = ui.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown)));
        if up || down {
            let i = selected.and_then(|n| shown.iter().position(|t| t.number == n));
            let last = shown.len() - 1;
            let next = match (i, down) {
                (None, _) => 0,
                (Some(i), true) => (i + 1).min(last),
                (Some(i), false) => i.saturating_sub(1),
            };
            if let Some(t) = shown.get(next) {
                select(requests, t.number);
                state.hovered = None;
            }
        }
    }

    let preview_width = 300.0;
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            // A cap, not a fixed width: sizing to all the remaining space would overflow by the
            // spacing each frame and grow the window without end.
            ui.set_max_width((ui.available_width() - preview_width - 40.0).max(300.0));
            ui.horizontal_wrapped(|ui| {
                ui.menu_button("New tool…", |ui| {
                    for ty in TYPES {
                        if ui.button(ty.label).clicked() {
                            requests.push((format!("tool.create.{}", ty.id), Value::Null));
                            ui.close();
                        }
                    }
                });
                ui.add_enabled_ui(selected.is_some(), |ui| {
                    if ui.button("Edit…").clicked() {
                        requests.push(("tool.edit".into(), Value::Null));
                    }
                    if ui
                        .button("Feeds…")
                        .on_hover_text("Spindle speed and feeds, or work them out from surface speed and chip load")
                        .clicked()
                    {
                        requests.push(("tool.feeds".into(), Value::Null));
                    }
                    if ui
                        .button("Preset…")
                        .on_hover_text("Add or change a cutting preset (per workpiece material)")
                        .clicked()
                    {
                        requests.push(("tool.preset.set".into(), Value::Null));
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
                ui.separator();
                if ui
                    .button("Sample library")
                    .on_hover_text("Add two tools of every type, with presets and holders")
                    .clicked()
                {
                    requests.push(("tool.library.sample".into(), json!({})));
                }
                if ui.button("Import…").clicked() {
                    requests.push(("tool.library.import".into(), Value::Null));
                }
                if ui.button("Export…").clicked() {
                    requests.push(("tool.library.export".into(), Value::Null));
                }
            });
            ui.horizontal(|ui| {
                let label = state.kind.as_deref().and_then(tool_type).map_or("All types", |t| t.label);
                egui::ComboBox::from_id_salt("library-type").selected_text(label).width(170.0).show_ui(ui, |ui| {
                    if ui.selectable_label(state.kind.is_none(), "All types").clicked() {
                        state.kind = None;
                    }
                    for ty in TYPES {
                        let count = machine.tools.iter().filter(|t| t.kind == ty.id).count();
                        let text = format!("{} ({count})", ty.label);
                        if ui.selectable_label(state.kind.as_deref() == Some(ty.id), text).clicked() {
                            state.kind = Some(ty.id.to_owned());
                        }
                    }
                });
                ui.add(
                    egui::TextEdit::singleline(&mut state.search)
                        .hint_text("Search name, vendor, part no.")
                        .desired_width(200.0),
                );
                ui.label(
                    RichText::new(format!("{} of {} tools", shown.len(), machine.tools.len()))
                        .small()
                        .color(tokens.text_muted),
                );
            });
            ui.add_space(4.0);
            if machine.tools.is_empty() {
                state.hovered = None;
                ui.label(
                    RichText::new("No tools yet. Add one with New tool, or load the sample library.")
                        .color(tokens.text_muted),
                );
                return;
            }
            let rate = |v: f64| if v > 0.0 { u.plain_rate(v) } else { "–".into() };
            let rpm = |v: f64| if v > 0.0 { format!("{v}") } else { "–".into() };
            let mut hovered = None;
            ScrollArea::vertical()
                .id_salt("library-scroll")
                .auto_shrink([true, true])
                .max_height(ui.available_height().max(200.0))
                .show(ui, |ui| {
                    egui::Grid::new("library-tools").striped(true).num_columns(10).spacing([12.0, 4.0]).show(
                        ui,
                        |ui| {
                            for h in [
                                "T".to_owned(),
                                "Name".to_owned(),
                                "Type".to_owned(),
                                format!("Ø {}", u.length()),
                                "R / angle".to_owned(),
                                "Flutes".to_owned(),
                                format!("LOC {}", u.length()),
                                "Holder".to_owned(),
                                "rpm".to_owned(),
                                format!("Feed {}", u.rate()),
                            ] {
                                ui.label(RichText::new(h).small().color(tokens.text_muted));
                            }
                            ui.end_row();
                            for t in &shown {
                                let on = selected == Some(t.number);
                                let shape = if t.tip_angle > 0.0 {
                                    format!("{}°", t.tip_angle)
                                } else if t.corner_radius > 0.0 {
                                    format!("R{}", u.plain(t.corner_radius))
                                } else if t.thread_pitch > 0.0 {
                                    format!("P{}", u.plain(t.thread_pitch))
                                } else {
                                    "–".into()
                                };
                                let holder = lib.holder_of(t).map_or("–".to_owned(), |h| h.name.clone());
                                let mut cells = vec![
                                    ui.selectable_label(on, format!("T{}", t.number)),
                                    ui.selectable_label(on, t.name.as_str()),
                                ];
                                for text in [
                                    t.type_label().to_owned(),
                                    u.plain(t.diameter),
                                    shape,
                                    t.flutes.to_string(),
                                    u.plain(t.flute_length),
                                ] {
                                    cells.push(ui.label(text));
                                }
                                cells.push(ui.label(RichText::new(holder).small()));
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
                        },
                    );
                });
            state.hovered = hovered;
        });
        ui.separator();
        ui.vertical(|ui| {
            ui.set_width(preview_width);
            let shown_tool = state.hovered.or(selected).and_then(|n| machine.tool(n));
            match shown_tool {
                Some(t) => tool_details(ui, session, t, tokens, preview_width, selected == Some(t.number), requests),
                None => {
                    ui.label(RichText::new("Point at a tool to preview it.").color(tokens.text_muted));
                }
            }
            ui.add_space(8.0);
            holders(ui, session, tokens, requests);
        });
    });
}

/// One tool: its drawing, its type's values and its cutting presets.
fn tool_details(
    ui: &mut Ui,
    session: &Session,
    t: &CosmicCAM_engine::library::Tool,
    tokens: &Tokens,
    width: f32,
    selected: bool,
    requests: &mut Vec<Request>,
) {
    let u = session.prefs.units;
    ui.label(RichText::new(format!("T{}  {}", t.number, t.name)).strong());
    ui.label(RichText::new(t.type_label()).small().color(tokens.text_muted));
    crate::tool_view::draw(ui, Some(t), session.library.holder_of(t), u, tokens, egui::vec2(width, 300.0));
    let mut facts: Vec<(&str, String)> = vec![
        ("Flutes", t.flutes.to_string()),
        ("Shank", u.fmt_len(t.shank_diameter)),
        ("Stick-out", u.fmt_len(t.overall_length)),
    ];
    let len = |v: f64| u.fmt_len(v);
    if t.corner_radius > 0.0 && !t.ball_ended() {
        facts.push(("Corner radius", len(t.corner_radius)));
    }
    if t.tip_angle > 0.0 {
        facts.push(("Point angle", format!("{}°", t.tip_angle)));
    }
    if t.tip_diameter > 0.0 {
        facts.push(("Tip / pilot Ø", len(t.tip_diameter)));
    }
    if t.tip_length > 0.0 {
        facts.push(("Pilot length", len(t.tip_length)));
    }
    if t.taper_angle > 0.0 {
        facts.push(("Taper", format!("{}°", t.taper_angle)));
    }
    if t.neck_diameter > 0.0 {
        facts.push(("Neck Ø", len(t.neck_diameter)));
    }
    if t.shoulder_length > 0.0 {
        facts.push(("Reach", len(t.shoulder_length)));
    }
    if t.thread_pitch > 0.0 {
        facts.push(("Pitch", format!("{} ({} hand)", len(t.thread_pitch), t.hand)));
    }
    facts.push((
        "Material",
        format!("{}{}", t.material, if t.coating.is_empty() { String::new() } else { format!(", {}", t.coating) }),
    ));
    if t.coolant_through {
        facts.push(("Coolant", "through the tool".into()));
    }
    if !t.vendor.is_empty() || !t.product_id.is_empty() {
        facts.push(("Vendor", format!("{} {}", t.vendor, t.product_id).trim().to_owned()));
    }
    facts.push(("Speed", if t.spindle_rpm > 0.0 { format!("{} rpm", t.spindle_rpm) } else { "not set".into() }));
    facts.push(("Feed", if t.feed > 0.0 { u.fmt_rate(t.feed) } else { "not set".into() }));
    egui::Grid::new("library-facts").num_columns(2).spacing([10.0, 2.0]).show(ui, |ui| {
        for (k, v) in facts {
            ui.label(RichText::new(k).small().color(tokens.text_muted));
            ui.label(RichText::new(v).small());
            ui.end_row();
        }
    });
    if !t.presets.is_empty() {
        ui.add_space(6.0);
        ui.label(RichText::new("Cutting presets").strong());
        egui::Grid::new("library-presets").num_columns(5).spacing([8.0, 2.0]).show(ui, |ui| {
            for p in &t.presets {
                ui.label(RichText::new(p.name.as_str()).small());
                ui.label(
                    RichText::new(if p.surface_speed > 0.0 {
                        format!("{} m/min", p.surface_speed)
                    } else {
                        "–".into()
                    })
                    .small(),
                );
                ui.label(RichText::new(format!("{} rpm", p.spindle_rpm)).small());
                ui.label(RichText::new(u.fmt_rate(p.feed)).small());
                ui.horizontal(|ui| {
                    if selected
                        && ui.small_button("Use").on_hover_text("Make this the tool's own cutting data").clicked()
                    {
                        requests.push(("tool.preset.apply".into(), json!({ "number": t.number, "name": p.name })));
                    }
                    if selected && ui.small_button("×").on_hover_text("Delete this preset").clicked() {
                        requests.push(("tool.preset.remove".into(), json!({ "number": t.number, "name": p.name })));
                    }
                });
                ui.end_row();
            }
        });
    }
}

/// The holder library: each holder's name and size, with New and Delete.
fn holders(ui: &mut Ui, session: &Session, tokens: &Tokens, requests: &mut Vec<Request>) {
    let u = session.prefs.units;
    egui::CollapsingHeader::new(format!("Holders ({})", session.library.holders.len()))
        .id_salt("library-holders")
        .show(ui, |ui| {
            if ui.small_button("New holder…").clicked() {
                requests.push(("holder.create".into(), Value::Null));
            }
            for h in &session.library.holders {
                let widest = h.segments.iter().map(|s| s.lower_diameter.max(s.upper_diameter)).fold(0.0, f64::max);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(h.name.as_str()).small());
                    ui.label(
                        RichText::new(format!("Ø{} × {}", u.plain(widest), u.fmt_len(h.height())))
                            .small()
                            .color(tokens.text_muted),
                    );
                    if ui.small_button("×").on_hover_text("Delete this holder").clicked() {
                        requests.push(("holder.remove".into(), json!({ "id": h.id })));
                    }
                });
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
