//! Interactive starts: what a toolbar click, shortcut or `ui.start` does for commands that
//! need input first. Each one gathers that input (a file dialog, for now) and then runs the
//! command with explicit parameters; programmatic `engine.execute` calls never get here.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use serde_json::json;

use crate::Request;

/// Commands with an interactive start.
pub fn has_start(id: &str) -> bool {
    matches!(id, "model.import")
}

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
}

#[derive(Default)]
pub struct Tools {
    pending: Option<Pending>,
}

impl Tools {
    /// Starts `id`'s interactive tool. Errors are user-facing messages.
    pub fn start(&mut self, id: &str) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("finish the open dialog first".into());
        }
        match id {
            "model.import" => {
                self.pending = Some(Pending::Import(pick_model_file()?));
                Ok(())
            }
            other => Err(format!("`{other}` has no interactive tool")),
        }
    }

    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
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
