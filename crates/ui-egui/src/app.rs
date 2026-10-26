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
    /// Start with the demo project open.
    pub demo: bool,
}

/// A window capture waiting for the next frame.
struct Capture {
    path: PathBuf,
    reply: Option<Incoming>,
    then_quit: bool,
    /// Wait for camera moves to finish first; recordings capture straight away.
    settle: bool,
}

/// A click injected over the control channel, played out over three frames
/// (move, press, release) so hover and click logic see it as a real one.
struct SyntheticClick {
    at: egui::Pos2,
    /// Primary, or secondary for a right-click.
    button: egui::PointerButton,
    step: u8,
    reply: Option<Incoming>,
}

/// Text typed into the focused field through the control channel, after any queued clicks.
struct SyntheticText {
    text: String,
    /// Select what the field holds first, so the text replaces it.
    replace: bool,
    reply: Option<Incoming>,
}

/// Longest text one `ui.type` request may type.
const MAX_TYPED_CHARS: usize = 4096;

pub struct CosmicApp {
    session: Session,
    clicks: Vec<SyntheticClick>,
    typing: Vec<SyntheticText>,
    dock: DockState<Tab>,
    layout_generation: u64,
    viewport: Viewport,
    console: ConsoleState,
    /// The toolpath editor panel, while one is open.
    toolpath_panel: Option<crate::toolpath_panel::ToolpathPanel>,
    /// The setup editor panel, while one is open.
    setup_panel: Option<crate::setup_panel::SetupPanel>,
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
    /// A success worth telling the user about (e.g. a file written), shown when there is no failure.
    notice: Option<String>,
    /// A document tab waiting for the user to confirm closing it without saving.
    confirm_close: Option<usize>,
    /// An item being renamed from the explorer: (command, id, name typed so far).
    rename: Option<(String, String, String)>,
    /// An item waiting for the user to confirm deleting it: (command, id, label).
    confirm_delete: Option<(String, String, String)>,
    /// The post-processor chooser, while it is open.
    post_chooser: bool,
    /// The machine's post settings as last read: (post id, settings), or why they could not be.
    post_settings: Option<(String, Result<Vec<Value>, String>)>,
    /// Text being typed into post settings, by name.
    post_setting_text: std::collections::HashMap<String, String>,
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
        let LaunchOptions { automation, control_port, demo } = options;
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
        // The demo project opens once the library is loaded, so its tools join the right one.
        if demo && let Err(e) = session.open_demo() {
            startup_status = Some(format!("The demo project could not be opened: {e}"));
        }
        // Saved preferences: the display units and the theme.
        if let Some(saved) =
            cc.storage.and_then(|s| s.get_string(SETTINGS_KEY)).and_then(|t| serde_json::from_str::<Value>(&t).ok())
        {
            if let Some(units) =
                saved.get("units").and_then(Value::as_str).and_then(CosmicCAM_engine::units::DisplayUnits::from_id)
            {
                session.prefs.units = units;
            }
            if let Some(theme) = saved.get("theme").and_then(Value::as_str).and_then(Theme::from_id) {
                session.prefs.theme = theme;
            }
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
            typing: Vec::new(),
            dock: dock::default_layout(),
            layout_generation,
            viewport: Viewport::new(),
            console: ConsoleState::default(),
            library_open: false,
            toolpath_panel: None,
            setup_panel: None,
            settings_open: false,
            saved_prefs,
            library: LibraryState::default(),
            tools: Tools::default(),
            render_state: cc.wgpu_render_state.clone(),
            applied_theme: None,
            status: startup_status,
            notice: None,
            confirm_close: None,
            rename: None,
            confirm_delete: None,
            post_chooser: false,
            post_settings: None,
            post_setting_text: std::collections::HashMap::new(),
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
        // A strategy tile opens the toolpath panel; double-clicking a toolpath edits it.
        let strategy = crate::toolpath_panel::Strategy::from_command(id).filter(|_| params.is_null());
        if strategy.is_some() || id == "toolpath.edit" {
            let editing = params.get("id").and_then(Value::as_str);
            if editing.is_some_and(|e| self.session.job.toolpaths.iter().all(|t| t.id != e)) {
                self.status = Some("that toolpath no longer exists".into());
                return;
            }
            let strategy = strategy.unwrap_or(crate::toolpath_panel::Strategy::Contour);
            self.toolpath_panel = Some(crate::toolpath_panel::ToolpathPanel::new(&self.session, editing, strategy));
            self.setup_panel = None;
            self.status = None;
            return;
        }
        // Create setup and Box stock open the setup panel; so does editing a setup by id alone.
        let edit_setup = id == "setup.edit" && params.as_object().is_some_and(|p| p.len() == 1);
        if (params.is_null() && matches!(id, "setup.create" | "setup.stock.box")) || edit_setup {
            use crate::setup_panel::{SetupPanel, SetupTarget};
            let target = match id {
                "setup.create" => SetupTarget::Create,
                "setup.stock.box" => SetupTarget::Stock,
                _ => match params.get("id").and_then(Value::as_str) {
                    Some(sid) if self.session.job.setup_by_id(sid).is_some() => SetupTarget::Edit(sid.to_owned()),
                    _ => {
                        self.status = Some("that setup no longer exists".into());
                        return;
                    }
                },
            };
            self.setup_panel = Some(SetupPanel::new(&self.session, target));
            self.toolpath_panel = None;
            self.status = None;
            return;
        }
        // The settings button opens the settings window; scripts get the settings back.
        if params.is_null() && id == "settings.open" {
            self.settings_open = true;
            return;
        }
        // Space (or Play) while the simulation runs pauses it.
        if id == "sim.play" && params.is_null() && self.session.sim.as_ref().is_some_and(|s| s.playing) {
            self.run("sim.pause", &Value::Null);
            return;
        }
        // The Post-processor button opens a chooser; scripts pass `post`.
        if id == "post.processor.select" && params.is_null() {
            self.post_chooser = true;
            return;
        }
        // The explorer's Rename… and Delete open a small dialog first.
        if id == "ui.rename" || id == "ui.delete" {
            let field = |k: &str| params.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
            let entry = (field("command"), field("id"), field(if id == "ui.rename" { "name" } else { "label" }));
            if id == "ui.rename" {
                self.rename = Some(entry);
            } else {
                self.confirm_delete = Some(entry);
            }
            return;
        }
        // Closing a document with unsaved changes asks first.
        if id == "job.tab.close" && params.get("discard").is_none() {
            let index = params.get("index").and_then(Value::as_u64).map_or(self.session.active_tab(), |i| i as usize);
            if self.session.tab_job(index).is_some_and(|j| j.is_modified()) {
                self.confirm_close = Some(index);
                return;
            }
        }
        // Save asks where only the first time; after that it saves over the job's file.
        let saves_in_place = id == "job.save" && self.session.job.path.is_some();
        if params.is_null() && tools::has_start(id) && !saves_in_place {
            self.status = self.tools.start(id, &self.session).err();
            return;
        }
        match self.session.execute(id, params) {
            Ok(result) => {
                self.status = None;
                self.notice = match id {
                    "post.gcode.export" => {
                        let warnings = result["warnings"].as_array().map_or(0, Vec::len);
                        Some(format!(
                            "Wrote {} lines of G-code to {} with the {} post{}",
                            result["lines"].as_u64().unwrap_or(0),
                            result["path"].as_str().unwrap_or_default(),
                            result["post"].as_str().unwrap_or_default(),
                            if warnings > 0 {
                                format!(" ({warnings} warnings: see the console)")
                            } else {
                                String::new()
                            }
                        ))
                    }
                    "post.limits.check" => Some(match result["problems"].as_array() {
                        Some(p) if !p.is_empty() => p.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; "),
                        _ => "The program is within the machine's travel, speed and feed limits".into(),
                    }),
                    "toolpath.cl.export" => {
                        Some(format!("Wrote CL data to {}", result["path"].as_str().unwrap_or_default()))
                    }
                    "job.save" | "job.save.as" => {
                        Some(format!("Saved {}", result["path"].as_str().unwrap_or_default()))
                    }
                    "tool.library.export" => {
                        Some(format!("Wrote the tool library to {}", result["path"].as_str().unwrap_or_default()))
                    }
                    "model.import" => import_notice(&result),
                    _ => self.notice.take().filter(|_| id.ends_with(".get") || id == "selection.set"),
                };
            }
            Err(e) => {
                self.status = Some(e.to_string());
                self.notice = None;
            }
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
                    let settle = p.get("settle").and_then(Value::as_bool).unwrap_or(true);
                    self.captures.push(Capture {
                        path: PathBuf::from(path),
                        reply: Some(incoming),
                        then_quit: false,
                        settle,
                    });
                    ctx.request_repaint();
                }
                _ => incoming.err("invalid_params", "`path` must be a file path ending in .png"),
            },
            "ui.click" => {
                let coord = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && v.abs() < 1.0e5);
                match (coord("x"), coord("y")) {
                    (Some(x), Some(y)) if self.clicks.len() < 16 => {
                        let at = egui::pos2(x as f32, y as f32);
                        let button = match p.get("button").and_then(Value::as_str) {
                            Some("right") => egui::PointerButton::Secondary,
                            Some("middle") => egui::PointerButton::Middle,
                            _ => egui::PointerButton::Primary,
                        };
                        self.clicks.push(SyntheticClick { at, button, step: 0, reply: Some(incoming) });
                        ctx.request_repaint();
                    }
                    (Some(_), Some(_)) => incoming.err("busy", "too many clicks queued"),
                    _ => incoming.err("invalid_params", "`x` and `y` must be numbers (window points)"),
                }
            }
            "ui.type" => match p.get("text").and_then(Value::as_str) {
                Some(text) if text.chars().count() <= MAX_TYPED_CHARS && self.typing.len() < 16 => {
                    let replace = p.get("replace").and_then(Value::as_bool).unwrap_or(false);
                    self.typing.push(SyntheticText { text: text.to_owned(), replace, reply: Some(incoming) });
                    ctx.request_repaint();
                }
                Some(_) => incoming.err("invalid_params", "`text` is too long, or too much typing is queued"),
                None => incoming.err("invalid_params", "`text` must be a string"),
            },
            "ui.start"
                if matches!(p.get("command").and_then(Value::as_str), Some("setup.create" | "setup.stock.box")) =>
            {
                let command = p.get("command").and_then(Value::as_str).unwrap_or_default().to_owned();
                self.run(&command, &Value::Null);
                incoming.ok(json!({ "started": command }));
            }
            "ui.start"
                if p.get("command")
                    .and_then(Value::as_str)
                    .and_then(crate::toolpath_panel::Strategy::from_command)
                    .is_some() =>
            {
                let command = p.get("command").and_then(Value::as_str).unwrap_or_default().to_owned();
                self.run(&command, &Value::Null);
                incoming.ok(json!({ "started": command }));
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
            let settle = self.captures.iter().any(|c| c.settle);
            if settle && (self.viewport.is_animating() || self.capture_wait < 3) && self.capture_wait < MAX_WAIT_FRAMES
            {
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
            quit |= capture.then_quit;
            // On the desktop, control-channel captures are written on their own thread so a
            // recording does not hold up the frames it records.
            #[cfg(not(target_arch = "wasm32"))]
            if capture.reply.is_some() && !capture.then_quit {
                let shared = Arc::clone(&image);
                let (tx, rx) = std::sync::mpsc::channel::<Capture>();
                let spawned = std::thread::Builder::new().name("screenshot".into()).spawn(move || {
                    if let Ok(capture) = rx.recv() {
                        finish_capture(&shared, capture);
                    }
                });
                match spawned {
                    Ok(_) => {
                        let _ = tx.send(capture);
                        continue;
                    }
                    Err(_) => {
                        finish_capture(&image, capture);
                        continue;
                    }
                }
            }
            finish_capture(&image, capture);
        }
        if quit {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    /// The post-processor chooser: every post with its description, the machine's current one
    /// ticked, and where to put your own.
    fn show_post_chooser(&mut self, ctx: &egui::Context, tokens: &Tokens, requests: &mut Vec<Request>) {
        let posts = CosmicCAM_engine::postproc::available();
        let machine = self.session.library.active_machine();
        let current = machine.map(|m| m.post.clone()).filter(|p| !p.is_empty()).unwrap_or_else(|| "generic".into());
        let machine_name = machine.map_or(String::new(), |m| m.name.clone());
        let mut open = true;
        crate::ui_kit::Window::dialog("Post-processor").show(ctx, tokens, &mut open, |ui| {
            ui.set_min_width(460.0);
            ui.label(RichText::new(format!("Programs for {machine_name} are written with:")).color(tokens.text_muted));
            ui.add_space(6.0);
            for p in &posts {
                ui.horizontal(|ui| {
                    if ui.radio(p.id == current, RichText::new(&p.id).strong()).clicked() && p.id != current {
                        requests.push(("post.processor.select".into(), json!({ "post": p.id })));
                    }
                    let origin = if p.builtin { "" } else { "  (yours)" };
                    ui.label(RichText::new(format!("{}{origin}", p.description)).color(tokens.text_muted));
                });
            }
            ui.add_space(8.0);
            ui.separator();
            self.show_post_settings(ui, tokens, &current, requests);
            ui.separator();
            ui.add_space(4.0);
            if let Some(dir) = CosmicCAM_engine::postproc::posts_dir() {
                ui.label(
                    RichText::new(format!(
                        "Your own posts go in {}. Copy one from builtin/ there to change it.",
                        dir.display()
                    ))
                    .small()
                    .color(tokens.text_muted),
                );
                #[cfg(not(target_arch = "wasm32"))]
                if ui.button("Open the posts folder").clicked() {
                    requests.push(("post.processor.folder".into(), Value::Null));
                    let _ = CosmicCAM_engine::postproc::install_builtin(&dir);
                    open_folder(&dir);
                }
            }
            ui.add_space(6.0);
            if ui.button("Close").clicked() {
                self.post_chooser = false;
            }
        });
        if !open {
            self.post_chooser = false;
        }
    }

    /// The settings the machine's post offers, grouped, each editable in place.
    fn show_post_settings(&mut self, ui: &mut Ui, tokens: &Tokens, post: &str, requests: &mut Vec<Request>) {
        if post == CosmicCAM_engine::postproc::GENERIC {
            ui.label(RichText::new("The generic post has no settings.").color(tokens.text_muted));
            return;
        }
        // Reading the settings starts the post: done once per post, not every frame.
        if self.post_settings.as_ref().is_none_or(|(id, _)| id != post) {
            let mut trial = self.session.clone();
            let read = trial
                .execute("post.properties.get", &json!({ "post": post }))
                .map(|v| v["properties"].as_array().cloned().unwrap_or_default())
                .map_err(|e| e.to_string());
            self.post_settings = Some((post.to_owned(), read));
            self.post_setting_text.clear();
        }
        let Some((_, read)) = &mut self.post_settings else { return };
        let props = match read {
            Ok(p) => p,
            Err(e) => {
                ui.label(RichText::new(format!("The post's settings could not be read: {e}")).color(tokens.error));
                return;
            }
        };
        ui.label(RichText::new(format!("{post} settings for this machine")).strong());
        let mut groups: Vec<String> = Vec::new();
        for p in props.iter() {
            let g = p["group"].as_str().unwrap_or("General").to_owned();
            if !groups.contains(&g) {
                groups.push(g);
            }
        }
        let texts = &mut self.post_setting_text;
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for group in &groups {
                egui::CollapsingHeader::new(group.as_str()).default_open(group == "Motion" || group == "Program").show(
                    ui,
                    |ui| {
                        egui::Grid::new(("post-settings", group.as_str())).num_columns(2).spacing([12.0, 6.0]).show(
                            ui,
                            |ui| {
                                for p in props.iter_mut().filter(|p| p["group"].as_str() == Some(group.as_str())) {
                                    let name = p["name"].as_str().unwrap_or_default().to_owned();
                                    let help = p["description"].as_str().unwrap_or_default().to_owned();
                                    ui.label(p["title"].as_str().unwrap_or(&name)).on_hover_text(&help);
                                    let value_text = match &p["value"] {
                                        Value::String(t) => t.clone(),
                                        Value::Bool(b) => b.to_string(),
                                        Value::Null => String::new(),
                                        other => other.to_string(),
                                    };
                                    let mut set = |v: String| {
                                        requests
                                            .push(("post.properties.set".into(), json!({ "name": name, "value": v })));
                                    };
                                    match p["kind"].as_str().unwrap_or("str") {
                                        "bool" => {
                                            let mut on = value_text == "true";
                                            if ui.checkbox(&mut on, "").changed() {
                                                set(on.to_string());
                                                p["value"] = json!(on);
                                            }
                                        }
                                        "choice" => {
                                            let choices: Vec<String> = p["choices"]
                                                .as_array()
                                                .map(|c| {
                                                    c.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()
                                                })
                                                .unwrap_or_default();
                                            let mut picked = None;
                                            egui::ComboBox::from_id_salt(("post-setting", name.as_str()))
                                                .selected_text(value_text.as_str())
                                                .width(160.0)
                                                .show_ui(ui, |ui| {
                                                    for c in &choices {
                                                        if ui.selectable_label(*c == value_text, c.as_str()).clicked() {
                                                            picked = Some(c.clone());
                                                        }
                                                    }
                                                });
                                            if let Some(c) = picked.filter(|c| *c != value_text) {
                                                set(c.clone());
                                                p["value"] = json!(c);
                                            }
                                        }
                                        _ => {
                                            let buffer =
                                                texts.entry(name.clone()).or_insert_with(|| value_text.clone());
                                            let response =
                                                ui.add(egui::TextEdit::singleline(buffer).desired_width(160.0));
                                            if response.lost_focus() && *buffer != value_text {
                                                set(buffer.clone());
                                                p["value"] = json!(buffer.clone());
                                            }
                                        }
                                    }
                                    ui.end_row();
                                }
                            },
                        );
                    },
                );
            }
        });
        if ui.button("Restore the post's defaults").clicked() {
            for p in props.iter() {
                let name = p["name"].as_str().unwrap_or_default();
                requests.push(("post.properties.set".into(), json!({ "name": name, "value": "" })));
            }
            self.post_settings = None;
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
            // Typing waits for the clicks before it, which usually focus the field.
            if !self.typing.is_empty() {
                let typed = self.typing.remove(0);
                if typed.replace {
                    raw_input.events.push(Event::Key {
                        key: egui::Key::A,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Modifiers::COMMAND,
                    });
                }
                raw_input.events.push(Event::Text(typed.text.clone()));
                if let Some(reply) = typed.reply {
                    reply.ok(json!({ "typed": typed.text }));
                }
                ctx.request_repaint();
            }
            return;
        };
        let button =
            |pressed| Event::PointerButton { pos: click.at, button: click.button, pressed, modifiers: Modifiers::NONE };
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
        storage.set_string(
            SETTINGS_KEY,
            json!({ "units": self.session.prefs.units.id(), "theme": self.session.prefs.theme.id() }).to_string(),
        );
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

        // The simulation's clock runs with the frames while it plays.
        if let Some(sim) = self.session.sim.as_deref_mut()
            && sim.playing
        {
            sim.tick(f64::from(ctx.input(|i| i.stable_dt)));
            ctx.request_repaint();
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
            self.captures.push(Capture { path, reply: None, then_quit: true, settle: true });
        }
        self.service_captures(&ctx);
        #[cfg(not(target_arch = "wasm32"))]
        ribbon::resize_edges(&ctx);
        ribbon::show(ui, &self.session, tokens, &mut requests);

        let status = self.status.clone();
        let fps = self.viewport.fps();
        Panel::bottom("status", Surface::StatusBar).show(ui, tokens, |ui| {
            ui.horizontal(|ui| {
                match (&status, &self.notice) {
                    (Some(msg), _) => ui.label(RichText::new(msg).color(tokens.error)),
                    (None, Some(note)) => ui.label(RichText::new(note).color(tokens.success)),
                    (None, None) => ui.label(RichText::new("Ready").color(tokens.text_muted)),
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

        // The toolpath panel docks at the right, before the workspace takes the rest.
        if let Some(panel) = &mut self.toolpath_panel {
            let (request, keep) = panel.show(ui, &self.session, tokens);
            if let Some((id, params)) = request {
                match self.session.execute(&id, &params) {
                    Ok(_) => {
                        self.status = None;
                        self.toolpath_panel = None;
                    }
                    Err(e) => panel.fail(e.to_string()),
                }
            } else if !keep {
                self.toolpath_panel = None;
            }
        }
        if let Some(panel) = &mut self.setup_panel {
            let (request, keep) = panel.show(ui, &self.session, tokens);
            if let Some((id, params)) = request {
                match self.session.execute(&id, &params) {
                    Ok(_) => {
                        self.status = None;
                        self.setup_panel = None;
                    }
                    Err(e) => panel.fail(e.to_string()),
                }
            } else if !keep {
                self.setup_panel = None;
            }
        }
        self.viewport.stock_preview = self.setup_panel.as_ref().and_then(|p| p.preview_stock());
        self.viewport.pick_mode = match &self.toolpath_panel {
            Some(panel) => Some(panel.mode(&self.session)),
            None => self.tools.pick_mode(),
        };
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
            crate::ui_kit::Window::dialog("Preferences...").show(&ctx, tokens, &mut open, |ui| {
                ui.set_min_size(vec2(560.0, 380.0));
                egui::Grid::new("preferences").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                    ui.label("Units:");
                    egui::ComboBox::from_id_salt("pref-units").selected_text(units.label()).width(160.0).show_ui(
                        ui,
                        |ui| {
                            for choice in [
                                CosmicCAM_engine::units::DisplayUnits::Inches,
                                CosmicCAM_engine::units::DisplayUnits::Millimetres,
                            ] {
                                if ui.selectable_label(units == choice, choice.label()).clicked() && units != choice {
                                    requests.push(("settings.units.set".into(), json!({ "units": choice.id() })));
                                }
                            }
                        },
                    );
                    ui.end_row();
                    ui.label("Theme:");
                    let theme = self.session.prefs.theme;
                    let name = |t: Theme| match t {
                        Theme::Dark => "Dark",
                        Theme::Light => "Light",
                    };
                    egui::ComboBox::from_id_salt("pref-theme").selected_text(name(theme)).width(160.0).show_ui(
                        ui,
                        |ui| {
                            for choice in [Theme::Dark, Theme::Light] {
                                if ui.selectable_label(theme == choice, name(choice)).clicked() && theme != choice {
                                    requests.push(("view.theme.set".into(), json!({ "theme": choice.id() })));
                                }
                            }
                        },
                    );
                    ui.end_row();
                    ui.label("Layout:");
                    if ui.button("Restore").on_hover_text("Put the panels back where they started").clicked() {
                        requests.push(("view.layout.reset".into(), Value::Null));
                    }
                    ui.end_row();
                });
            });
            self.settings_open &= open;
        }
        if self.post_chooser {
            self.show_post_chooser(&ctx, tokens, &mut requests);
        }
        if let Some((command, target, mut text)) = self.rename.take() {
            let mut open = true;
            let mut done: Option<bool> = None;
            // The same name prompt saves toolpath templates.
            let (title, button) =
                if command == "toolpath.template.save" { ("Save as template", "Save") } else { ("Rename", "Rename") };
            crate::ui_kit::Window::dialog(title).show(&ctx, tokens, &mut open, |ui| {
                ui.set_min_width(280.0);
                let response = ui.add(egui::TextEdit::singleline(&mut text).desired_width(260.0));
                if !response.has_focus() && !response.lost_focus() {
                    response.request_focus();
                }
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(button).clicked() || enter {
                        done = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        done = Some(false);
                    }
                });
            });
            match done {
                Some(true) if !text.trim().is_empty() => {
                    requests.push((command, json!({ "id": target, "name": text.trim() })));
                }
                Some(_) => {}
                None if open => self.rename = Some((command, target, text)),
                None => {}
            }
        }
        if let Some((command, target, label)) = self.confirm_delete.clone() {
            let mut open = true;
            crate::ui_kit::Window::dialog("Delete").show(&ctx, tokens, &mut open, |ui| {
                ui.label(format!("Delete {label}?"));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        requests.push((command.clone(), json!({ "id": target })));
                        self.confirm_delete = None;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_delete = None;
                    }
                });
            });
            if !open {
                self.confirm_delete = None;
            }
        }
        if let Some(index) = self.confirm_close {
            let mut open = true;
            let job = self.session.tab_job(index);
            let name = job.map(|j| j.name.clone()).unwrap_or_default();
            let has_file = job.is_some_and(|j| j.path.is_some());
            crate::ui_kit::Window::dialog("Unsaved changes").show(&ctx, tokens, &mut open, |ui| {
                ui.label(format!("{name} has changes that are not saved."));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if has_file && ui.button("Save and close").clicked() {
                        requests.push(("job.tab.activate".into(), json!({ "index": index })));
                        requests.push(("job.save".into(), json!({})));
                        requests.push(("job.tab.close".into(), json!({ "index": index })));
                        self.confirm_close = None;
                    }
                    if ui.button("Close without saving").clicked() {
                        requests.push(("job.tab.close".into(), json!({ "index": index, "discard": true })));
                        self.confirm_close = None;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_close = None;
                    }
                });
            });
            if !open || job.is_none() {
                self.confirm_close = None;
            }
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
            match &mut self.toolpath_panel {
                Some(panel) => panel.picked(hit, &self.session),
                None => self.tools.picked(hit, &self.session),
            }
        }
        if let Some((id, params)) = self.tools.show(&ctx, tokens, &self.session) {
            let outcome = self.session.execute(&id, &params);
            if id == "model.import"
                && let Ok(result) = &outcome
            {
                self.notice = import_notice(result);
            }
            let outcome = outcome.map(|_| ()).map_err(|e| e.to_string());
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

/// Writes a capture and answers whoever asked for it.
fn finish_capture(image: &egui::ColorImage, capture: Capture) {
    let result = automation::save_png(image, &capture.path);
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
}

/// Shows a folder in the system's file manager.
#[cfg(not(target_arch = "wasm32"))]
fn open_folder(dir: &std::path::Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(dir).spawn();
}

/// What an import had to skip or approximate, for the notice bar; None when it went cleanly.
fn import_notice(result: &Value) -> Option<String> {
    let warnings: Vec<&str> = result["warnings"].as_array()?.iter().filter_map(Value::as_str).collect();
    let (first, rest) = warnings.split_first()?;
    Some(format!(
        "Imported {} with {}: {first}",
        result["name"].as_str().unwrap_or("the model"),
        if rest.is_empty() { "a warning".to_owned() } else { format!("{} warnings", warnings.len()) }
    ))
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
