//! Interactive starts: what a toolbar click, shortcut or `ui.start` does for commands that
//! need input first. Each one gathers that input (a file dialog, for now) and then runs the
//! command with explicit parameters; programmatic `engine.execute` calls never get here.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use serde_json::json;

use CosmicCAM_engine::{Session, cmd};

use crate::Request;
use crate::param_form::ParamForm;
use crate::theme::Tokens;
use crate::viewport::{PickHit, PickMode};
use crate::wcs_tool::WcsTool;

/// Commands with an interactive start.
pub fn has_start(id: &str) -> bool {
    id == "model.import" || FORM_COMMANDS.contains(&id) || WCS_COMMANDS.contains(&id)
}

/// Commands started by the WCS panel.
const WCS_COMMANDS: &[&str] = &["setup.wcs.create", "setup.wcs.align"];

/// Commands started by a dialog generated from their parameters.
const FORM_COMMANDS: &[&str] = &["setup.wcs.offset", "setup.wcs.rotate", "setup.wcs.rename"];

/// File types offered by the model import dialog: (label, extensions).
const MODEL_FILTERS: &[(&str, &[&str])] = &[
    ("Models (STL, OBJ)", &["stl", "STL", "obj", "OBJ"]),
    ("STL", &["stl", "STL"]),
    ("Wavefront OBJ", &["obj", "OBJ"]),
];

/// An interactive start waiting for the user.
pub enum Pending {
    /// A native file dialog, answered on a channel when it closes.
    Import(Receiver<Option<PathBuf>>),
    /// A parameter dialog.
    Form(Box<ParamForm>),
    /// The WCS panel, picking geometry in the viewport.
    Pick(Box<WcsTool>),
}

#[derive(Default)]
pub struct Tools {
    pending: Option<Pending>,
}

impl Tools {
    /// Starts `id`'s interactive tool. Errors are user-facing messages.
    pub fn start(&mut self, id: &str, session: &Session) -> Result<(), String> {
        if self.pending.is_some() {
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

    /// Commands to run now that a tool has its input.
    pub fn poll(&mut self) -> Vec<Request> {
        let Some(Pending::Import(rx)) = &self.pending else {
            return Vec::new();
        };
        match rx.try_recv() {
            Err(TryRecvError::Empty) => Vec::new(),
            Ok(choice) => {
                self.pending = None;
                choice.into_iter().map(|path| import_request(&path)).collect()
            }
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                Vec::new()
            }
        }
    }
}

pub fn import_request(path: &std::path::Path) -> Request {
    ("model.import".into(), json!({ "path": path.display().to_string() }))
}

#[cfg(not(target_arch = "wasm32"))]
fn pick_model_file() -> Result<Receiver<Option<PathBuf>>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let dialog = MODEL_FILTERS
        .iter()
        .fold(rfd::FileDialog::new().set_title("Import model"), |d, (label, exts)| d.add_filter(*label, exts));
    // macOS dialogs must run on the main thread; elsewhere the dialog gets its own so the
    // window keeps drawing and the control channel keeps answering.
    #[cfg(target_os = "macos")]
    {
        let _ = tx.send(dialog.pick_file());
    }
    #[cfg(not(target_os = "macos"))]
    std::thread::Builder::new()
        .name("import-dialog".into())
        .spawn(move || {
            let _ = tx.send(dialog.pick_file());
        })
        .map_err(|e| format!("could not open the file dialog: {e}"))?;
    Ok(rx)
}

#[cfg(target_arch = "wasm32")]
fn pick_model_file() -> Result<Receiver<Option<PathBuf>>, String> {
    let _ = MODEL_FILTERS;
    Err("file dialogs are not available in the browser yet".into())
}
