//! Interactive starts: what a toolbar click, shortcut or `ui.start` does for commands that
//! need input first. Each one gathers that input (a file dialog, for now) and then runs the
//! command with explicit parameters; programmatic `engine.execute` calls never get here.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use serde_json::json;

use CosmicCAM_engine::{Session, cmd};

use crate::Request;
use crate::param_form::ParamForm;
use crate::theme::Tokens;
use crate::viewport::{PickHit, PickMode};
use crate::wcs_tool::WcsTool;

/// Commands with an interactive start.
pub fn has_start(id: &str) -> bool {
    id == "model.import" || FORM_COMMANDS.contains(&id) || WCS_COMMANDS.contains(&id) || FILE_COMMANDS.contains(&id)
}

/// Commands whose `path` comes from a file dialog.
const FILE_COMMANDS: &[&str] = &["tool.library.import", "tool.library.export"];

/// Commands started by the WCS panel.
const WCS_COMMANDS: &[&str] = &["setup.wcs.create", "setup.wcs.align"];

/// Commands started by a dialog generated from their parameters.
const FORM_COMMANDS: &[&str] = &[
    "setup.wcs.offset",
    "setup.wcs.rotate",
    "setup.wcs.rename",
    "tool.create",
    "tool.create.flat",
    "tool.edit",
    "tool.feeds",
    "tool.duplicate",
    "tool.remove",
    "machine.create",
    "machine.edit",
    "machine.duplicate",
    "machine.remove",
];

/// File types offered by the model import dialog: (label, extensions).
const MODEL_FILTERS: &[(&str, &[&str])] = &[
    ("Models (STL, OBJ)", &["stl", "STL", "obj", "OBJ"]),
    ("STL", &["stl", "STL"]),
    ("Wavefront OBJ", &["obj", "OBJ"]),
];

/// A model file the user chose: a path on the desktop, the bytes themselves in the browser.
pub enum ImportChoice {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Path(PathBuf),
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    Upload { name: String, bytes: Vec<u8> },
}

/// An interactive start waiting for the user.
pub enum Pending {
    /// A file dialog, answered on a channel when it closes.
    Import(Receiver<Option<ImportChoice>>),
    /// A parameter dialog.
    Form(Box<ParamForm>),
    /// The WCS panel, picking geometry in the viewport.
    Pick(Box<WcsTool>),
    /// A file dialog choosing `path` for a command.
    File(&'static str, Receiver<Option<PathBuf>>),
}

pub struct Tools {
    pending: Option<Pending>,
    /// Files dropped on the window whose bytes are still being read (browser).
    drops: (Sender<ImportChoice>, Receiver<ImportChoice>),
}

impl Default for Tools {
    fn default() -> Self {
        Self { pending: None, drops: channel() }
    }
}

impl Tools {
    /// Starts `id`'s interactive tool. Errors are user-facing messages.
    pub fn start(&mut self, id: &str, session: &Session) -> Result<(), String> {
        // A browser file picker may never report a cancel, so a new import replaces it.
        if self.pending.as_ref().is_some_and(|p| !(id == "model.import" && matches!(p, Pending::Import(_)))) {
            return Err("finish the open dialog first".into());
        }
        match id {
            "model.import" => {
                self.pending = Some(Pending::Import(pick_model_file()?));
                Ok(())
            }
            "setup.wcs.create" | "setup.wcs.align" => {
                if id == "setup.wcs.align" && session.job.active_wcs.is_none() {
                    return Err("there is no WCS to align yet; create one first".into());
                }
                let command = if id == "setup.wcs.align" { "setup.wcs.align" } else { "setup.wcs.create" };
                self.pending = Some(Pending::Pick(Box::new(WcsTool::new(command, session))));
                Ok(())
            }
            "tool.library.import" => {
                self.pending = Some(Pending::File("tool.library.import", pick_json_file(false)?));
                Ok(())
            }
            "tool.library.export" => {
                self.pending = Some(Pending::File("tool.library.export", pick_json_file(true)?));
                Ok(())
            }
            other if FORM_COMMANDS.contains(&other) => {
                let spec = cmd::find(other).ok_or_else(|| format!("unknown command `{other}`"))?;
                self.pending = Some(Pending::Form(Box::new(ParamForm::new(spec, session))));
                Ok(())
            }
            other => Err(format!("`{other}` has no interactive tool")),
        }
    }

    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Draws an open dialog. Returns the command it confirmed; report the outcome with
    /// [`Tools::finished`].
    pub fn show(&mut self, ctx: &egui::Context, tokens: &Tokens, session: &Session) -> Option<Request> {
        let (request, open) = match &mut self.pending {
            Some(Pending::Form(form)) => (form.show(ctx, tokens), form.is_open()),
            Some(Pending::Pick(tool)) => (tool.show(ctx, tokens, session), tool.is_open()),
            _ => return None,
        };
        if !open {
            self.pending = None;
        }
        request
    }

    /// What the viewport should pick while a picking tool is open.
    pub fn pick_mode(&self) -> Option<PickMode> {
        match &self.pending {
            Some(Pending::Pick(tool)) => Some(tool.mode()),
            _ => None,
        }
    }

    /// A pick from the viewport, for the open picking tool.
    pub fn picked(&mut self, hit: PickHit, session: &Session) {
        if let Some(Pending::Pick(tool)) = &mut self.pending {
            tool.picked(hit, session);
        }
    }

    /// The outcome of a command a dialog confirmed: success closes it, an error keeps it
    /// open showing the message.
    pub fn finished(&mut self, outcome: Result<(), String>) {
        match (&mut self.pending, outcome) {
            (Some(Pending::Form(_) | Pending::Pick(_)), Ok(())) => self.pending = None,
            (Some(Pending::Form(form)), Err(e)) => form.fail(e),
            (Some(Pending::Pick(tool)), Err(e)) => tool.fail(e),
            _ => {}
        }
    }

    /// Commands to run now that a tool has its input. Uploaded bytes are staged in the
    /// session and imported by their `upload://` path.
    pub fn poll(&mut self, session: &mut Session) -> Vec<Request> {
        let mut choices: Vec<ImportChoice> = self.drops.1.try_iter().collect();
        if let Some(Pending::Import(rx)) = &self.pending {
            match rx.try_recv() {
                Err(TryRecvError::Empty) => {}
                Ok(choice) => {
                    self.pending = None;
                    choices.extend(choice);
                }
                Err(TryRecvError::Disconnected) => self.pending = None,
            }
        }
        let mut requests = Vec::new();
        if let Some(Pending::File(command, rx)) = &self.pending {
            let command = *command;
            match rx.try_recv() {
                Err(TryRecvError::Empty) => {}
                Ok(path) => {
                    self.pending = None;
                    requests.extend(path.map(|p| (command.to_owned(), json!({ "path": p.display().to_string() }))));
                }
                Err(TryRecvError::Disconnected) => self.pending = None,
            }
        }
        requests.extend(choices.into_iter().map(|choice| match choice {
            ImportChoice::Path(path) => import_request(&path.display().to_string()),
            ImportChoice::Upload { name, bytes } => import_request(&session.stage_upload(&name, bytes)),
        }));
        requests
    }

    /// Files dropped on the window. On the desktop they have paths; in the browser their
    /// bytes are read in the background and imported when ready.
    pub fn dropped(&mut self, files: &[std::sync::Arc<dyn egui::DroppedFile + Send + Sync>]) -> Vec<Request> {
        #[cfg(target_arch = "wasm32")]
        {
            for file in files {
                let file = std::sync::Arc::clone(file);
                let tx = self.drops.0.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let name = file.path().display().to_string();
                    if let Ok(bytes) = file.bytes_async().await {
                        let _ = tx.send(ImportChoice::Upload { name, bytes });
                    }
                });
            }
            Vec::new()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = &self.drops.0;
            files.iter().map(|f| import_request(&f.path().display().to_string())).collect()
        }
    }
}

fn import_request(path: &str) -> Request {
    ("model.import".into(), json!({ "path": path }))
}

#[cfg(not(target_arch = "wasm32"))]
fn pick_model_file() -> Result<Receiver<Option<ImportChoice>>, String> {
    let (tx, rx) = channel();
    let dialog = MODEL_FILTERS
        .iter()
        .fold(rfd::FileDialog::new().set_title("Import model"), |d, (label, exts)| d.add_filter(*label, exts));
    // macOS dialogs must run on the main thread; elsewhere the dialog gets its own so the
    // window keeps drawing and the control channel keeps answering.
    #[cfg(target_os = "macos")]
    {
        let _ = tx.send(dialog.pick_file().map(ImportChoice::Path));
    }
    #[cfg(not(target_os = "macos"))]
    std::thread::Builder::new()
        .name("import-dialog".into())
        .spawn(move || {
            let _ = tx.send(dialog.pick_file().map(ImportChoice::Path));
        })
        .map_err(|e| format!("could not open the file dialog: {e}"))?;
    Ok(rx)
}

/// An open or save dialog for a tool library file.
#[cfg(not(target_arch = "wasm32"))]
fn pick_json_file(save: bool) -> Result<Receiver<Option<PathBuf>>, String> {
    let (tx, rx) = channel();
    let dialog = rfd::FileDialog::new()
        .set_title(if save { "Export tool library" } else { "Import tool library" })
        .add_filter("Tool library (JSON)", &["json", "JSON"]);
    let dialog = if save { dialog.set_file_name("tools.json") } else { dialog };
    let pick = move || if save { dialog.save_file() } else { dialog.pick_file() };
    #[cfg(target_os = "macos")]
    {
        let _ = tx.send(pick());
    }
    #[cfg(not(target_os = "macos"))]
    std::thread::Builder::new()
        .name("library-dialog".into())
        .spawn(move || {
            let _ = tx.send(pick());
        })
        .map_err(|e| format!("could not open the file dialog: {e}"))?;
    Ok(rx)
}

#[cfg(target_arch = "wasm32")]
fn pick_json_file(_save: bool) -> Result<Receiver<Option<PathBuf>>, String> {
    Err("importing and exporting tool library files is not available in the browser yet".into())
}

/// The browser's file picker; the chosen file's bytes arrive on the channel.
#[cfg(target_arch = "wasm32")]
fn pick_model_file() -> Result<Receiver<Option<ImportChoice>>, String> {
    let (tx, rx) = channel();
    let dialog = MODEL_FILTERS
        .iter()
        .fold(rfd::AsyncFileDialog::new().set_title("Import model"), |d, (label, exts)| d.add_filter(*label, exts));
    wasm_bindgen_futures::spawn_local(async move {
        let choice = match dialog.pick_file().await {
            Some(file) => Some(ImportChoice::Upload { name: file.file_name(), bytes: file.read().await }),
            None => None,
        };
        let _ = tx.send(choice);
    });
    Ok(rx)
}
