//! The simulation's controls over the viewport: a timeline of the program (one band per
//! toolpath, collisions marked) with the transport under it, a readout of where the tool is and
//! what it is doing, the comparison legend, and keys to step through the program.

use CosmicDMS_engine::Session;
use CosmicDMS_engine::sim::{PATH_DISPLAYS, Sim};
use CosmicDMS_engine::toolpath::MoveKind;
use CosmicDMS_engine::units::DisplayUnits;
use egui::{Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};

use crate::Request;
use crate::theme::Tokens;
use crate::ui_kit::{Container, Menu, Surface};

/// Minutes and seconds.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// A speed without needless decimals: 0.5, 2, 25.
fn trim_speed(v: f64) -> String {
    if v >= 10.0 || (v - v.round()).abs() < 1.0e-9 { format!("{v:.0}") } else { format!("{v:.1}") }
}

/// Monospaced text, for numbers that should not jitter as digits change.
fn mono(text: String, colour: Color32) -> egui::RichText {
    egui::RichText::new(text).monospace().color(colour)
}

/// A collision as people read it.
fn event_label(kind: &str) -> &'static str {
    match kind {
        "rapid_in_material" => "Rapid through material",
        "holder_contact" => "Holder touches the stock",
        "shank_contact" => "Shank rubs (deeper than the flutes)",
        _ => "Collision",
    }
}

fn icon_button(ui: &mut Ui, icon: &str, tip: &str, tokens: &Tokens) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, tokens.hover_bg);
    }
    crate::ribbon::paint_named_icon(ui, rect.shrink(3.0), icon, tokens, true);
    response.on_hover_text(tip).clicked()
}

/// The playback bar floating above the view tools: the timeline over the transport, time,
/// speed, the display menu and close. `bottom` is where the bar's foot goes.
pub fn playback(
    ui: &mut Ui,
    viewport: Rect,
    bottom: f32,
    sim: &Sim,
    units: DisplayUnits,
    tokens: &Tokens,
    requests: &mut Vec<Request>,
) {
    let size = vec2(760.0_f32.min(viewport.width() - 16.0), 76.0);
    let bar = Rect::from_min_size(pos2(viewport.center().x - size.x / 2.0, bottom - size.y), size);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::top_down(egui::Align::Min)));
    Container::new(Surface::Overlay).show(&mut child, tokens, |ui| {
        ui.set_width(size.x - 16.0);
        timeline(ui, sim, tokens, requests);
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let jump = |to: &str| ("sim.jump".to_owned(), json!({ "to": to }));
            if icon_button(ui, "sim_jump_start", "Go to the start (Home)", tokens) {
                requests.push(jump("start"));
            }
            if icon_button(ui, "sim_jump_previous", "Previous toolpath (Page Up)", tokens) {
                requests.push(jump("previous_toolpath"));
            }
            if icon_button(ui, "sim_step_back", "Back one move (←)", tokens) {
                requests.push(("sim.step".into(), json!({ "moves": -1 })));
            }
            let (icon, tip, command) = if sim.playing {
                ("sim_pause", "Pause (Space)", "sim.pause")
            } else {
                ("sim_play", "Play (Space)", "sim.play")
            };
            if icon_button(ui, icon, tip, tokens) {
                requests.push((command.into(), Value::Null));
            }
            if icon_button(ui, "sim_step", "Forward one move (→)", tokens) {
                requests.push(("sim.step".into(), json!({ "moves": 1 })));
            }
            if icon_button(ui, "sim_jump_next", "Next toolpath (Page Down)", tokens) {
                requests.push(jump("next_toolpath"));
            }
            if icon_button(ui, "sim_jump_end", "Go to the end (End)", tokens) {
                requests.push(jump("end"));
            }
            ui.add_space(6.0);
            ui.label(mono(format!("{} / {}", clock(sim.time), clock(sim.duration)), tokens.text));
            ui.add_space(6.0);
            let mut speed = sim.speed;
            let slider = egui::Slider::new(&mut speed, 0.5..=1000.0).logarithmic(true).show_value(false);
            if ui.add_sized(vec2(110.0, 20.0), slider).on_hover_text("Playback speed").changed() {
                requests.push(("sim.speed".into(), json!({ "speed": speed })));
            }
            ui.label(mono(format!("{:>4}×", trim_speed(sim.speed)), tokens.text_muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("×").on_hover_text("Close the simulation").clicked() {
                    requests.push(("sim.stop".into(), Value::Null));
                }
                let menu = ui.button("Display…").on_hover_text("What the simulation shows, and when it stops");
                Menu::below(&menu).show(tokens, |ui| display_menu(ui, sim, units, requests));
                let compare = ui
                    .selectable_label(sim.compare, "Compare")
                    .on_hover_text("Colour the stock by what is left over the part");
                if compare.clicked() {
                    requests.push(("sim.compare".into(), json!({ "on": !sim.compare })));
                }
                if !sim.events.is_empty() {
                    let n = sim.events.len();
                    let text = format!("{n} collision{}", if n == 1 { "" } else { "s" });
                    let r = ui
                        .add(egui::Button::new(egui::RichText::new(text).color(tokens.warning)).frame(false))
                        .on_hover_text("Go to the next collision");
                    if r.clicked() {
                        requests.push(jump("next_collision"));
                    }
                }
            });
        });
    });
}

fn display_menu(ui: &mut Ui, sim: &Sim, units: DisplayUnits, requests: &mut Vec<Request>) {
    ui.label(egui::RichText::new("Toolpaths").strong());
    for (id, label) in PATH_DISPLAYS.iter().zip(["None", "The current one, up to the tool", "All simulated"]) {
        if ui.radio(sim.show_paths == *id, label).clicked() {
            requests.push(("sim.display.set".into(), json!({ "paths": id })));
        }
    }
    ui.separator();
    let mut tool = sim.show_tool;
    if ui.checkbox(&mut tool, "Tool and holder").changed() {
        requests.push(("sim.display.set".into(), json!({ "tool": tool })));
    }
    let mut stop = sim.stop_on_collision;
    if ui.checkbox(&mut stop, "Stop at collisions").changed() {
        requests.push(("sim.display.set".into(), json!({ "stop_on_collision": stop })));
    }
    ui.separator();
    ui.label(egui::RichText::new("Compare tolerance").strong());
    ui.horizontal(|ui| {
        // Round steps in the units shown: millimetres, or thousandths of an inch.
        let steps: [f64; 5] = match units {
            DisplayUnits::Inches => [0.0005, 0.001, 0.002, 0.005, 0.01].map(|t| units.to_mm(t)),
            _ => [0.01, 0.02, 0.05, 0.1, 0.25],
        };
        for t in steps {
            if ui.selectable_label((sim.compare_tolerance - t).abs() < 1.0e-6, units.plain(t)).clicked() {
                requests.push(("sim.compare".into(), json!({ "on": sim.compare, "tolerance": t })));
            }
        }
        ui.label(units.length());
    });
}

/// The program as a bar: a band per toolpath, collisions as ticks, the playhead; click or drag
/// to go there, hover to see which toolpath runs then.
fn timeline(ui: &mut Ui, sim: &Sim, tokens: &Tokens, requests: &mut Vec<Request>) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click_and_drag());
    let painter = ui.painter_at(rect.expand(2.0));
    let duration = sim.duration.max(1.0e-9);
    let x_of = |t: f64| rect.left() + (t / duration).clamp(0.0, 1.0) as f32 * rect.width();
    let t_of = |x: f32| f64::from(((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0)) * duration;
    painter.rect_filled(rect, 4.0, tokens.raised_bg);
    let segments = sim.segments();
    let current = sim.moves.get(sim.move_at(sim.time)).map(|m| m.toolpath);
    for (k, (a, b)) in segments.iter().enumerate() {
        let band = Rect::from_x_y_ranges(x_of(*a)..=x_of(*b).max(x_of(*a) + 1.0), rect.y_range());
        let base = if k % 2 == 0 { tokens.accent_strong } else { tokens.accent };
        let fill = base.gamma_multiply(if current == Some(k) { 0.55 } else { 0.25 });
        painter.rect_filled(band.shrink2(vec2(0.5, 3.0)), 2.0, fill);
        // Names where they fit.
        if band.width() > 60.0
            && let Some((_, name)) = sim.toolpaths.get(k)
        {
            painter.text(
                band.left_center() + vec2(5.0, 0.0),
                egui::Align2::LEFT_CENTER,
                name,
                egui::FontId::proportional(11.0),
                tokens.text,
            );
        }
    }
    // What has run so far, along the foot.
    let done = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 3.0), pos2(x_of(sim.time), rect.bottom()));
    painter.rect_filled(done, 1.0, tokens.accent);
    for e in &sim.events {
        let x = x_of(e.time);
        painter.line_segment([pos2(x, rect.top() + 1.0), pos2(x, rect.bottom() - 1.0)], Stroke::new(2.0, tokens.error));
    }
    let head = x_of(sim.time);
    painter
        .line_segment([pos2(head, rect.top() - 2.0), pos2(head, rect.bottom() + 2.0)], Stroke::new(2.0, tokens.text));
    painter.circle(pos2(head, rect.top() - 1.0), 4.0, tokens.text, Stroke::new(1.5, tokens.panel_bg));
    if let Some(p) = response.interact_pointer_pos().filter(|_| response.clicked() || response.dragged()) {
        requests.push(("sim.seek".into(), json!({ "time": t_of(p.x) })));
    }
    if let Some(p) = response.hover_pos() {
        let t = t_of(p.x);
        let k = segments.iter().position(|(a, b)| t >= *a && t <= *b);
        let name = k.and_then(|k| sim.toolpaths.get(k)).map_or("", |(_, n)| n.as_str());
        let near = sim.events.iter().find(|e| (x_of(e.time) - p.x).abs() < 4.0);
        let text = match near {
            Some(e) => format!("{} · {}\n{}", clock(t), name, event_label(e.kind)),
            None => format!("{} · {}", clock(t), name),
        };
        response.on_hover_text_at_pointer(text);
    }
}

/// The readout at the top left while simulating: the toolpath and tool, the move, where the tool
/// is, the time, the material removed and the collisions found (click one to go there).
pub fn readout(
    ui: &mut Ui,
    viewport: Rect,
    sim: &Sim,
    session: &Session,
    tokens: &Tokens,
    requests: &mut Vec<Request>,
) {
    let units = session.prefs.units;
    let at = viewport.left_top() + vec2(10.0, 10.0);
    let area = Rect::from_min_size(at, vec2(270.0, viewport.height() - 140.0));
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
    Container::new(Surface::Overlay).show(&mut child, tokens, |ui| {
        ui.set_width(250.0);
        let index = sim.move_at(sim.time);
        let m = sim.moves.get(index);
        let k = m.map_or(0, |m| m.toolpath);
        let name = sim.toolpaths.get(k).map_or("", |(_, n)| n.as_str());
        ui.label(egui::RichText::new(name).strong().color(tokens.text));
        let muted = |s: String| egui::RichText::new(s).color(tokens.text_muted);
        ui.label(muted(format!("Toolpath {} of {}", k + 1, sim.toolpaths.len())));
        egui::Grid::new("sim-readout").num_columns(2).spacing([10.0, 2.0]).show(ui, |ui| {
            if let Some(tool) = m.and_then(|m| sim.tools.get(m.tool)) {
                ui.label(muted("Tool".into()));
                ui.label(format!("T{} · Ø{}", tool.number, units.fmt_len(tool.radius * 2.0)));
                ui.end_row();
            }
            if let Some(m) = m {
                ui.label(muted("Move".into()));
                let what = match m.kind {
                    MoveKind::Rapid => "rapid".to_owned(),
                    kind => {
                        let rate = if m.duration > 0.0 { m.from.distance(m.to) / m.duration * 60.0 } else { 0.0 };
                        format!("{} · {}", kind.id(), units.fmt_rate(rate))
                    }
                };
                ui.label(mono(format!("{} / {} · {what}", index + 1, sim.moves.len()), tokens.text));
                ui.end_row();
            }
            if let Some((tip, _, _)) = sim.pose_at(sim.time) {
                let frame = if sim.voxels.is_some() { "world" } else { "WCS" };
                ui.label(muted(format!("Tool tip ({frame})")));
                let p = |v: f64| units.plain(v);
                ui.label(mono(format!("X {}  Y {}  Z {}", p(tip.x), p(tip.y), p(tip.z)), tokens.text));
                ui.end_row();
            }
            ui.label(muted("Time".into()));
            ui.label(mono(format!("{} · {} left", clock(sim.time), clock(sim.duration - sim.time)), tokens.text));
            ui.end_row();
            ui.label(muted("Removed".into()));
            let per = units.mm_per_unit().powi(3);
            ui.label(mono(format!("{:.1} {}³", sim.removed / per, units.length()), tokens.text));
            ui.end_row();
        });
        if !sim.events.is_empty() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(format!("Collisions ({})", sim.events.len())).color(tokens.warning).strong());
            for e in sim.events.iter().take(6) {
                let who = sim.toolpaths.get(e.toolpath).map_or("", |(_, n)| n.as_str());
                let text = format!("{} · {} · {}", clock(e.time), event_label(e.kind), who);
                let r =
                    ui.add(egui::Button::new(egui::RichText::new(text).small()).frame(false)).on_hover_text("Go there");
                if r.clicked() {
                    requests.push(("sim.seek".into(), json!({ "time": e.time })));
                }
            }
            if sim.events.len() > 6 {
                ui.label(muted(format!("and {} more", sim.events.len() - 6)));
            }
        }
    });
}

/// The comparison legend at the top right, its right edge at `right` (left of the view cube):
/// what the colours mean, or `note` while the comparison is not drawn.
pub fn compare_panel(
    ui: &mut Ui,
    viewport: Rect,
    right: f32,
    sim: &Sim,
    session: &Session,
    tokens: &Tokens,
    note: Option<&str>,
) {
    let width = 230.0;
    let area = Rect::from_min_size(pos2(right - width, viewport.top() + 10.0), vec2(width, 200.0));
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
    Container::new(Surface::Overlay).show(&mut child, tokens, |ui| {
        ui.set_width(width - 16.0);
        ui.label(egui::RichText::new("Compared with the part").strong());
        match note {
            Some(note) => {
                ui.label(egui::RichText::new(note).color(tokens.text_muted));
            }
            None => legend(ui, sim.compare_tolerance, session, tokens),
        }
    });
}

/// The comparison colours and what they mean.
fn legend(ui: &mut Ui, tolerance: f64, session: &Session, tokens: &Tokens) {
    let units = session.prefs.units;
    let thin = crate::theme::blend(tokens.warning, tokens.stock, 0.0);
    for (colour, text) in [
        (tokens.error, format!("Cut into the part (over {})", units.fmt_len(tolerance))),
        (tokens.success, format!("On size (within ±{})", units.fmt_len(tolerance))),
        (thin, format!("Thin stock left (up to {})", units.fmt_len(1.0))),
        (crate::theme::blend(tokens.warning, tokens.stock, 0.5), format!("About {} left", units.fmt_len(1.0))),
        (tokens.stock, format!("{} or more left", units.fmt_len(2.0))),
    ] {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            ui.painter().rect_filled(r, 3.0, colour);
            ui.label(egui::RichText::new(text).small());
        });
    }
}

/// Keys while simulating: arrows step a move, Page Up and Down go by toolpath, Home and End
/// to the ends (Space plays and pauses through its command shortcut).
pub fn keys(ui: &Ui, requests: &mut Vec<Request>) {
    if ui.ctx().egui_wants_keyboard_input() {
        return;
    }
    use egui::Key;
    let pressed: Vec<Key> = ui.input(|i| {
        [Key::ArrowLeft, Key::ArrowRight, Key::PageUp, Key::PageDown, Key::Home, Key::End]
            .into_iter()
            .filter(|k| i.key_pressed(*k))
            .collect()
    });
    for key in pressed {
        requests.push(match key {
            Key::ArrowLeft => ("sim.step".into(), json!({ "moves": -1 })),
            Key::ArrowRight => ("sim.step".into(), json!({ "moves": 1 })),
            Key::PageUp => ("sim.jump".into(), json!({ "to": "previous_toolpath" })),
            Key::PageDown => ("sim.jump".into(), json!({ "to": "next_toolpath" })),
            Key::Home => ("sim.jump".into(), json!({ "to": "start" })),
            _ => ("sim.jump".into(), json!({ "to": "end" })),
        });
    }
}

/// Where a point lies on the timeline, for tests: the time at `x` across `rect`.
#[cfg(test)]
fn time_at(rect: Rect, duration: f64, x: f32) -> f64 {
    f64::from(((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0)) * duration
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks_and_the_timeline_read_right() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(83.4), "1:23");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(trim_speed(2.5), "2.5");
        assert_eq!(trim_speed(250.0), "250");
        let r = Rect::from_min_size(egui::Pos2::new(100.0, 0.0), vec2(200.0, 20.0));
        assert!((time_at(r, 60.0, 200.0) - 30.0).abs() < 1e-9);
        assert_eq!(time_at(r, 60.0, 50.0), 0.0);
    }
}
