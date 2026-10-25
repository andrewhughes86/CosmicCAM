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
const FILE_COMMANDS: &[&str] =
    &["tool.library.import", "tool.library.export", "post.gcode.export", "job.open", "job.save", "job.save.as"];

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
                self.pending = Some(Pending::File("tool.library.import", pick_file(&LIBRARY_IN, None)?));
                Ok(())
            }
            "tool.library.export" => {
                self.pending = Some(Pending::File("tool.library.export", pick_file(&LIBRARY_OUT, None)?));
                Ok(())
            }
            "post.gcode.export" => {
                // The browser has no file paths: build the program and download it instead.
                #[cfg(target_arch = "wasm32")]
                {
                    let mut trial = session.clone();
                    let text = trial.execute("post.gcode.get", &serde_json::Value::Null).map_err(|e| e.to_string())?;
                    download("program.nc", text["text"].as_str().unwrap_or_default())
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    // Check there is something to export before asking where to put it.
                    let mut trial = session.clone();
                    trial.execute("post.gcode.get", &serde_json::Value::Null).map_err(|e| e.to_string())?;
                    self.pending = Some(Pending::File("post.gcode.export", pick_file(&GCODE_OUT, None)?));
                    Ok(())
                }
            }
            "job.open" => {
                self.pending = Some(Pending::File("job.open", pick_file(&JOB_IN, None)?));
                Ok(())
            }
            "job.save" | "job.save.as" => {
                let name = format!("{}.{}", session.job.name, CosmicCAM_engine::jobfile::EXTENSION);
                // The browser has no file paths: the job file is downloaded instead.
                #[cfg(target_arch = "wasm32")]
                {
                    download(
                        &name,
                        &CosmicCAM_engine::jobfile::to_string(&session.job, &session.view.camera, &session.library),
                    )
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.pending = Some(Pending::File("job.save.as", pick_file(&JOB_OUT, Some(name))?));
                    Ok(())
                }
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
            ImportChoice::Path(path) => open_request(&path.display().to_string()),
            ImportChoice::Upload { name, bytes } => open_request(&session.stage_upload(&name, bytes)),
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
            files.iter().map(|f| open_request(&f.path().display().to_string())).collect()
        }
    }
}

/// A file to open: job files open in a tab, anything else is imported as a model.
fn open_request(path: &str) -> Request {
    let job = std::path::Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(CosmicCAM_engine::jobfile::EXTENSION));
    (if job { "job.open" } else { "model.import" }.into(), json!({ "path": path }))
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

/// What a file dialog asks for: title, filter label, extensions, suggested name (saves).
struct FileKind {
    title: &'static str,
    filter: &'static str,
    extensions: &'static [&'static str],
    save_as: Option<&'static str>,
}

const LIBRARY_IN: FileKind = FileKind {
    title: "Import tool library",
    filter: "Tool library (JSON)",
    extensions: &["json", "JSON"],
    save_as: None,
};
const LIBRARY_OUT: FileKind = FileKind {
    title: "Export tool library",
    filter: "Tool library (JSON)",
    extensions: &["json", "JSON"],
    save_as: Some("tools.json"),
};
const JOB_IN: FileKind =
    FileKind { title: "Open job", filter: "CosmicCAM job (.cosmic)", extensions: &["cosmic", "COSMIC"], save_as: None };
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
const JOB_OUT: FileKind = FileKind {
    title: "Save job",
    filter: "CosmicCAM job (.cosmic)",
    extensions: &["cosmic", "COSMIC"],
    save_as: Some("job.cosmic"),
};
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
const GCODE_OUT: FileKind = FileKind {
    title: "Export G-code",
    filter: "G-code program",
    extensions: &["nc", "NC", "gcode", "ngc", "tap"],
    save_as: Some("program.nc"),
};

/// An open or save dialog; the chosen path arrives on the channel. `name` overrides the
/// suggested file name of a save dialog.
#[cfg(not(target_arch = "wasm32"))]
fn pick_file(kind: &FileKind, name: Option<String>) -> Result<Receiver<Option<PathBuf>>, String> {
    let (tx, rx) = channel();
    let dialog = rfd::FileDialog::new().set_title(kind.title).add_filter(kind.filter, kind.extensions);
    let save = kind.save_as.is_some();
    let dialog = match name.as_deref().or(kind.save_as).filter(|_| save) {
        Some(name) => dialog.set_file_name(name),
        None => dialog,
    };
    let pick = move || if save { dialog.save_file() } else { dialog.pick_file() };
    #[cfg(target_os = "macos")]
    {
        let _ = tx.send(pick());
    }
    #[cfg(not(target_os = "macos"))]
    std::thread::Builder::new()
        .name("file-dialog".into())
        .spawn(move || {
            let _ = tx.send(pick());
        })
        .map_err(|e| format!("could not open the file dialog: {e}"))?;
    Ok(rx)
}

#[cfg(target_arch = "wasm32")]
fn pick_file(kind: &FileKind, name: Option<String>) -> Result<Receiver<Option<PathBuf>>, String> {
    let _ = (kind.filter, kind.extensions, kind.save_as, name);
    Err(format!("{} is not available in the browser yet", kind.title))
}

/// Offers `text` to the browser user as a download named `name`.
#[cfg(target_arch = "wasm32")]
pub fn download(name: &str, text: &str) -> Result<(), String> {
    use wasm_bindgen::JsCast as _;
    let fail = |what: &str| format!("could not start the download ({what})");
    let parts = js_sys::Array::of1(&wasm_bindgen::JsValue::from_str(text));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("text/plain");
    let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &options).map_err(|_| fail("blob"))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|_| fail("url"))?;
    let document = web_sys::window().and_then(|w| w.document()).ok_or_else(|| fail("document"))?;
    let a = document
        .create_element("a")
        .map_err(|_| fail("link"))?
        .dyn_into::<web_sys::HtmlAnchorElement>()
        .map_err(|_| fail("link"))?;
    a.set_href(&url);
    a.set_download(name);
    a.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
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
