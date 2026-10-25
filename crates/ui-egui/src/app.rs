//! The desktop application shell.

use std::path::PathBuf;
use std::sync::Arc;

use CosmicCAM_engine::{Pane, Session, Theme, UiRequest, cmd};
use egui::{Event, Key, KeyboardShortcut, Modifiers, RichText, Ui, UserData, ViewportCommand, vec2};
use egui_dock::{DockArea, DockState, Style};
use egui_wgpu::RenderState;
use serde_json::{Value, json};

use crate::Request;
use crate::automation::{self, Automation};
use crate::control::{ControlServer, Incoming, PROTOCOL_VERSION};
use crate::dock::{self, Tab};
use crate::panes::{ConsoleState, LibraryState};
use crate::ribbon;
use crate::theme::{self, Tokens};
use crate::tools::{self, Tools};
use crate::ui_kit::{Panel, Surface};
use crate::viewport::Viewport;

/// How the app was launched.
#[derive(Default)]
pub struct LaunchOptions {
    pub automation: Automation,
    /// Open the control channel on this port (0 picks a free one).
    pub control_port: Option<u16>,
}

/// A window capture waiting for the next frame.
struct Capture {
    path: PathBuf,
    reply: Option<Incoming>,
    then_quit: bool,
}

/// A click injected over the control channel, played out over three frames
/// (move, press, release) so hover and click logic see it as a real one.
struct SyntheticClick {
    at: egui::Pos2,
    step: u8,
    reply: Option<Incoming>,
}

pub struct CosmicApp {
    session: Session,
    clicks: Vec<SyntheticClick>,
    dock: DockState<Tab>,
    layout_generation: u64,
    viewport: Viewport,
    console: ConsoleState,
    /// The tool library window: open or not, and its preview state.
    library_open: bool,
    settings_open: bool,
    /// Preferences revision last written to storage.
    saved_prefs: u64,
    library: LibraryState,
    tools: Tools,
    render_state: Option<RenderState>,
    applied_theme: Option<Theme>,
    /// Last command failure, shown in the status bar.
    status: Option<String>,
    shortcuts: Vec<(KeyboardShortcut, &'static str)>,
    automation: Automation,
    control: Option<ControlServer>,
    captures: Vec<Capture>,
    capture_requested: bool,
    /// Frames a pending capture has waited for the view to settle.
    capture_wait: u32,
    /// Library revision last written to storage.
    saved_library: u64,
}

/// Storage key of the machine and tool library (a file on the desktop, localStorage on the web).
const LIBRARY_KEY: &str = "cosmiccam.library";
/// Storage key of the preferences.
const SETTINGS_KEY: &str = "cosmiccam.settings";

impl CosmicApp {
    pub fn new(cc: &eframe::CreationContext<'_>, mut session: Session, options: LaunchOptions) -> Self {
        let LaunchOptions { automation, control_port } = options;
        let layout_generation = session.view.layout_generation;
        let mut startup_status = None;
        if let Some(text) = cc.storage.and_then(|s| s.get_string(LIBRARY_KEY)) {
            let loaded =
                serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()).and_then(|v| session.load_library(&v));
            match loaded {
                Ok(dropped) if dropped.is_empty() => {}
                Ok(dropped) => {
                    log::warn!("machine library: dropped {} invalid entries: {}", dropped.len(), dropped.join("; "));
                    startup_status =
                        Some(format!("Some saved tools could not be read and were skipped: {}", dropped.join("; ")));
                }
                Err(e) => {
                    log::warn!("machine library not loaded: {e}");
                    startup_status = Some(format!(
                        "The saved machine library could not be read ({e}); starting with a generic machine"
                    ));
                }
            }
        }
        let saved_library = session.library.revision;
        // Saved preferences: the display units.
        if let Some(units) = cc
            .storage
            .and_then(|s| s.get_string(SETTINGS_KEY))
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| {
                v.get("units").and_then(Value::as_str).and_then(CosmicCAM_engine::units::DisplayUnits::from_id)
            })
        {
            session.prefs.units = units;
        }
        let saved_prefs = session.prefs.revision;
        if let Some(tab) = &automation.ribbon_tab {
            let _ = session.execute("ui.ribbon.tab.set", &json!({ "tab": tab.to_lowercase() }));
        }
        let control = control_port.and_then(|port| {
            let ctx = cc.egui_ctx.clone();
            match ControlServer::start(port, move || ctx.request_repaint()) {
                Ok(server) => {
                    // Scripts launching the app read the port from this line.
                    println!("CosmicCAM control channel listening on 127.0.0.1:{}", server.port());
                    use std::io::Write as _;
                    let _ = std::io::stdout().flush();
                    Some(server)
                }
                Err(e) => {
                    eprintln!("CosmicCAM: control channel not started: {e}");
                    None
                }
            }
        });
        Self {
            session,
            clicks: Vec::new(),
            dock: dock::default_layout(),
            layout_generation,
            viewport: Viewport::new(),
            console: ConsoleState::default(),
            library_open: false,
            settings_open: false,
            saved_prefs,
            library: LibraryState::default(),
            tools: Tools::default(),
            render_state: cc.wgpu_render_state.clone(),
            applied_theme: None,
            status: startup_status,
            shortcuts: cmd::all().filter_map(|c| Some((parse_shortcut(c.shortcut?)?, c.id))).collect(),
            automation,
            control,
            captures: Vec::new(),
            capture_requested: false,
            capture_wait: 0,
            saved_library,
        }
    }

    /// Runs a command; failures go to the status bar (and the console log). A command
    /// with an interactive tool, run without parameters (a toolbar click or shortcut),
    /// starts that tool instead.
    pub fn run(&mut self, id: &str, params: &Value) {
        // The settings button opens the settings window; scripts get the settings back.
        if params.is_null() && id == "settings.open" {
            self.settings_open = true;
            return;
        }
        if params.is_null() && tools::has_start(id) {
            self.status = self.tools.start(id, &self.session).err();
            return;
        }
        match self.session.execute(id, params) {
            Ok(_) => self.status = None,
            Err(e) => self.status = Some(e.to_string()),
        }
    }

    /// Answers one control-channel request. See `docs/control-protocol.md`.
    fn answer(&mut self, ctx: &egui::Context, incoming: Incoming) {
        let p = incoming.params.clone();
        match incoming.method.as_str() {
            "app.ping" => incoming.ok(json!({
                "name": "CosmicCAM",
                "version": env!("CARGO_PKG_VERSION"),
                "protocol": PROTOCOL_VERSION,
            })),
            "engine.execute" => {
                let Some(command) = p.get("command").and_then(Value::as_str) else {
                    return incoming.err("invalid_request", "`params.command` must be a command id");
                };
                let params = p.get("params").cloned().unwrap_or(Value::Null);
                match self.session.execute(command, &params) {
                    Ok(result) => {
                        self.status = None;
                        incoming.ok(result);
                    }
                    Err(e) => {
                        self.status = Some(e.to_string());
                        incoming.err(e.kind(), e.to_string());
                    }
                }
                ctx.request_repaint();
            }
            "engine.commands" => incoming.ok(json!(
                cmd::all().filter(|c| !c.id.starts_with("test.")).map(|c| c.to_json(&self.session)).collect::<Vec<_>>()
            )),
            "engine.state" => incoming.ok(self.session.state_json()),
            "ui.screenshot" => match p.get("path").and_then(Value::as_str) {
                Some(path) if path.to_ascii_lowercase().ends_with(".png") => {
                    self.captures.push(Capture { path: PathBuf::from(path), reply: Some(incoming), then_quit: false });
                    ctx.request_repaint();
                }
                _ => incoming.err("invalid_params", "`path` must be a file path ending in .png"),
            },
            "ui.click" => {
                let coord = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && v.abs() < 1.0e5);
                match (coord("x"), coord("y")) {
                    (Some(x), Some(y)) if self.clicks.len() < 16 => {
                        let at = egui::pos2(x as f32, y as f32);
                        self.clicks.push(SyntheticClick { at, step: 0, reply: Some(incoming) });
                        ctx.request_repaint();
                    }
                    (Some(_), Some(_)) => incoming.err("busy", "too many clicks queued"),
                    _ => incoming.err("invalid_params", "`x` and `y` must be numbers (window points)"),
                }
            }
            "ui.start" => match p.get("command").and_then(Value::as_str) {
                Some(command) if tools::has_start(command) => match self.tools.start(command, &self.session) {
                    Ok(()) => incoming.ok(json!({ "started": command })),
                    Err(e) => incoming.err("busy", e),
                },
                Some(_) => incoming.err("not_supported", "that command has no interactive tool; use engine.execute"),
                None => incoming.err("invalid_request", "`params.command` must be a command id"),
            },
            other => {
                let other = other.to_owned();
                incoming.err("unknown_method", format!("unknown method `{other}`"));
            }
        }
    }

    /// Carries out window and pane changes queued by commands.
    fn apply_ui_requests(&mut self, ctx: &egui::Context) {
        for request in self.session.take_ui_requests() {
            match request {
                // The tool library is a window, not a dock pane.
                UiRequest::ShowPane(Pane::Library) | UiRequest::FocusPane(Pane::Library) => self.library_open = true,
                UiRequest::HidePane(Pane::Library) => self.library_open = false,
                UiRequest::ShowPane(p) => dock::show(&mut self.dock, p),
                UiRequest::HidePane(p) => dock::hide(&mut self.dock, p),
                UiRequest::FocusPane(p) => dock::focus(&mut self.dock, p),
                UiRequest::WindowSize { width, height } => {
                    ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(width, height)));
                }
                UiRequest::Fullscreen(on) => ctx.send_viewport_cmd(ViewportCommand::Fullscreen(on)),
                UiRequest::Maximized(on) => ctx.send_viewport_cmd(ViewportCommand::Maximized(on)),
                UiRequest::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
            }
        }
        let mut open = dock::open_panes(&self.dock);
        if self.library_open {
            open.push(Pane::Library);
        }
        self.session.set_open_panes(open);
    }

    /// Requests a capture while any is pending and saves it when the image arrives.
    fn service_captures(&mut self, ctx: &egui::Context) {
        if self.captures.is_empty() {
            return;
        }
        ctx.request_repaint();
        if !self.capture_requested {
            // Let camera moves and window changes finish first (at most ~2 s).
            const MAX_WAIT_FRAMES: u32 = 120;
            self.capture_wait += 1;
            if (self.viewport.is_animating() || self.capture_wait < 3) && self.capture_wait < MAX_WAIT_FRAMES {
                return;
            }
            self.capture_wait = 0;
            self.capture_requested = true;
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::default()));
            return;
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                Event::Screenshot { image, .. } => Some(Arc::clone(image)),
                _ => None,
            })
        });
        let Some(image) = image else {
            return;
        };
        self.capture_requested = false;
        let mut quit = false;
        for capture in std::mem::take(&mut self.captures) {
            let result = automation::save_png(&image, &capture.path);
            match (&result, capture.reply) {
                (Ok(()), Some(reply)) => reply.ok(json!({
                    "path": capture.path.display().to_string(),
                    "width": image.size[0],
                    "height": image.size[1],
                })),
                (Err(e), Some(reply)) => reply.err("io", format!("could not save {}: {e}", capture.path.display())),
                (Ok(()), None) => println!("screenshot saved to {}", capture.path.display()),
                (Err(e), None) => eprintln!("could not save screenshot {}: {e}", capture.path.display()),
            }
            quit |= capture.then_quit;
        }
        if quit {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) -> Vec<Request> {
        if ctx.egui_wants_keyboard_input() {
            return Vec::new();
        }
        let mut fired = Vec::new();
        ctx.input_mut(|i| {
            for (shortcut, id) in &self.shortcuts {
                if i.consume_shortcut(shortcut) {
                    fired.push(((*id).to_owned(), Value::Null));
                }
            }
        });
        fired
    }
}

impl eframe::App for CosmicApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let Some(click) = self.clicks.first_mut() else {
            return;
        };
        let button = |pressed| Event::PointerButton {
            pos: click.at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        match click.step {
            0 => raw_input.events.push(Event::PointerMoved(click.at)),
            1 => raw_input.events.push(button(true)),
            _ => raw_input.events.push(button(false)),
        }
        click.step += 1;
        if click.step > 2 {
            if let Some(reply) = click.reply.take() {
                reply.ok(json!({ "x": click.at.x, "y": click.at.y }));
            }
            self.clicks.remove(0);
        }
        ctx.request_repaint();
    }

    /// Runs every frame and also while the window is hidden, so the API keeps answering.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let incoming = self.control.as_ref().map(ControlServer::poll).unwrap_or_default();
        for request in incoming {
            self.answer(ctx, request);
        }
        self.apply_ui_requests(ctx);
    }

    /// Also written every few seconds by eframe and on exit.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(LIBRARY_KEY, self.session.library_json().to_string());
        storage.set_string(SETTINGS_KEY, json!({ "units": self.session.prefs.units.id() }).to_string());
        self.saved_library = self.session.library.revision;
        self.saved_prefs = self.session.prefs.revision;
    }

    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let tokens: &'static Tokens = Tokens::for_theme(self.session.prefs.theme);
        if self.applied_theme != Some(self.session.prefs.theme) {
            theme::apply(&ctx, tokens);
            self.applied_theme = Some(self.session.prefs.theme);
        }
        if self.layout_generation != self.session.view.layout_generation {
            self.dock = dock::default_layout();
            self.layout_generation = self.session.view.layout_generation;
        }

        let mut requests = self.handle_shortcuts(&ctx);
        requests.extend(self.tools.poll(&mut self.session));
        if self.tools.is_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // Model files dropped on the window are imported.
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        requests.extend(self.tools.dropped(&dropped));
        let step = self.automation.tick(&ctx);
        requests.extend(step.requests);
        if let Some(path) = step.capture {
            self.captures.push(Capture { path, reply: None, then_quit: true });
        }
        self.service_captures(&ctx);
        ribbon::show(ui, &self.session, tokens, &mut requests);

        let status = self.status.clone();
        let fps = self.viewport.fps();
        Panel::bottom("status", Surface::StatusBar).show(ui, tokens, |ui| {
            ui.horizontal(|ui| {
                match &status {
                    Some(msg) => ui.label(RichText::new(msg).color(tokens.error)),
                    None => ui.label(RichText::new("Ready").color(tokens.text_muted)),
                };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} theme  ·  {}  ·  Z up  ·  {fps:.0} fps",
                            match self.session.prefs.theme {
                                Theme::Dark => "Dark",
                                Theme::Light => "Light",
                            },
                            self.session.prefs.units.length()
                        ))
                        .color(tokens.text_muted),
                    );
                });
            });
        });

        self.viewport.pick_mode = self.tools.pick_mode();
        Panel::central(Surface::Workspace).show(ui, tokens, |ui| {
            let mut style = Style::from_egui(ui.style());
            style.tab_bar.bg_fill = tokens.raised_bg;
            style.tab.active.bg_fill = tokens.panel_bg;
            style.tab.focused.bg_fill = tokens.panel_bg;
            style.tab.active.text_color = tokens.text;
            style.tab.focused.text_color = tokens.accent;
            style.tab.inactive.text_color = tokens.text_muted;
            style.tab.tab_body.bg_fill = tokens.panel_bg;
            style.tab.tab_body.stroke = egui::Stroke::new(1.0, tokens.border);
            style.separator.color_idle = tokens.border;
            style.separator.color_hovered = tokens.accent;
            style.separator.color_dragged = tokens.accent;
            style.overlay.selection_color = tokens.accent.gamma_multiply(0.35);
            let mut viewer = dock::Viewer {
                session: &mut self.session,
                tokens,
                viewport: &mut self.viewport,
                render_state: self.render_state.as_ref(),
                console: &mut self.console,
                requests: &mut requests,
            };
            DockArea::new(&mut self.dock)
                .id(egui::Id::new("workspace-dock"))
                .style(style)
                .show_leaf_collapse_buttons(false)
                .show_inside(ui, &mut viewer);
        });

        if self.settings_open {
            let mut open = true;
            let units = self.session.prefs.units;
            crate::ui_kit::Window::dialog("Settings").show(&ctx, tokens, &mut open, |ui| {
                ui.set_min_width(300.0);
                ui.label(RichText::new("Units").strong());
                ui.label(
                    RichText::new(
                        "Lengths, positions and feeds are shown and typed in these units everywhere in the app. \
                                   Scripts and the API always use millimetres.",
                    )
                    .small()
                    .color(tokens.text_muted),
                );
                for choice in
                    [CosmicCAM_engine::units::DisplayUnits::Inches, CosmicCAM_engine::units::DisplayUnits::Millimetres]
                {
                    if ui.radio(units == choice, choice.label()).clicked() && units != choice {
                        requests.push(("settings.units.set".into(), json!({ "units": choice.id() })));
                    }
                }
                ui.add_space(8.0);
                if ui.button("Close").clicked() {
                    self.settings_open = false;
                }
            });
            self.settings_open &= open;
        }
        if self.library_open {
            let mut open = true;
            crate::ui_kit::Window::browser("Tool library", vec2(860.0, 460.0)).show(&ctx, tokens, &mut open, |ui| {
                crate::panes::library(ui, &self.session, tokens, &mut self.library, &mut requests);
            });
            self.library_open = open;
        }
        for (id, params) in requests {
            self.run(&id, &params);
        }
        // A changed library is written straight away, so a crash cannot lose a tool.
        if (self.session.library.revision != self.saved_library || self.session.prefs.revision != self.saved_prefs)
            && let Some(storage) = frame.storage_mut()
        {
            self.save(storage);
            storage.flush();
        }
        if let Some(hit) = self.viewport.take_pick() {
            self.tools.picked(hit, &self.session);
        }
        if let Some((id, params)) = self.tools.show(&ctx, tokens, &self.session) {
            let outcome = self.session.execute(&id, &params).map(|_| ()).map_err(|e| e.to_string());
            self.status = outcome.as_ref().err().cloned();
            self.tools.finished(outcome);
        }
    }
}

/// Parses "Ctrl+Shift+T", "F" or "7" style shortcuts.
fn parse_shortcut(text: &str) -> Option<KeyboardShortcut> {
    let mut modifiers = Modifiers::NONE;
    let mut key = None;
    for part in text.split('+') {
        match part {
            "Ctrl" | "Cmd" => modifiers |= Modifiers::COMMAND,
            "Shift" => modifiers |= Modifiers::SHIFT,
            "Alt" => modifiers |= Modifiers::ALT,
            other => key = Key::from_name(other),
        }
    }
    Some(KeyboardShortcut::new(modifiers, key?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_shortcut_parses() {
        for spec in cmd::all() {
            if let Some(s) = spec.shortcut {
                assert!(parse_shortcut(s).is_some(), "{}: {s}", spec.id);
            }
        }
    }

    #[test]
    fn shortcuts_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for spec in cmd::all() {
            if let Some(s) = spec.shortcut {
                assert!(seen.insert(s), "{s} is bound twice");
            }
        }
    }
}
