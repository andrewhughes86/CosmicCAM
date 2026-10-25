//! Job, selection, introspection and console commands.

use serde_json::{Value, json};

use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, all, always, find};
use crate::error::{EngineError, EngineResult};
use crate::jobfile;
use crate::params;
use crate::session::{OutlineNode, Session};

const SINCE: ParamKind = ParamKind::Integer { min: 0, max: i64::MAX };
const TAB: ParamKind = ParamKind::Integer { min: 0, max: crate::session::MAX_TABS as i64 - 1 };
const TABS_RETURNS: &str = "{\"active\": index, \"tabs\": [{index, name, path, modified, active}]}";
const LIMIT: ParamKind = ParamKind::Integer { min: 1, max: crate::session::MAX_LOG as i64 };

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "job.new",
        label: "New job",
        tooltip: "Start an empty job",
        placement: Some(Placement::large(RibbonTab::Home, "Job")),
        icon: None,
        shortcut: Some("Ctrl+N"),
        params: &[Param::optional("sample", ParamKind::Bool, "Load the built-in sample part")],
        returns: "{\"job\": name}",
        enabled: always,
        run: |s, p| {
            s.new_document(params::opt_bool(p, "sample")?.unwrap_or(false))?;
            Ok(json!({ "job": s.job.name }))
        },
    },
    CommandSpec {
        id: "job.open",
        label: "Open",
        tooltip: "Open a job file (.cosmic) in a new tab",
        placement: Some(Placement::small(RibbonTab::Home, "Job")),
        icon: None,
        shortcut: Some("Ctrl+O"),
        params: &[Param::required("path", ParamKind::Str, "Job file to open (.cosmic)")],
        returns: "{\"job\": name, \"tab\": index, \"tabs\": [...]}",
        enabled: always,
        run: open,
    },
    CommandSpec {
        id: "job.save",
        label: "Save",
        tooltip: "Save the job to its file (.cosmic)",
        placement: Some(Placement::small(RibbonTab::Home, "Job")),
        icon: None,
        shortcut: Some("Ctrl+S"),
        params: &[Param::optional(
            "path",
            ParamKind::Str,
            "Where to save; omit to save over the file the job came from. `.cosmic` is added when the name has no \
             extension",
        )],
        returns: "{\"path\": file, \"bytes\": size}",
        enabled: always,
        run: save,
    },
    CommandSpec {
        id: "job.save.as",
        label: "Save as",
        tooltip: "Save the job to a new file (.cosmic)",
        placement: Some(Placement::small(RibbonTab::Home, "Job")),
        icon: Some("job_save"),
        shortcut: Some("Ctrl+Shift+S"),
        params: &[Param::required(
            "path",
            ParamKind::Str,
            "Where to save. `.cosmic` is added when the name has no extension",
        )],
        returns: "{\"path\": file, \"bytes\": size}",
        enabled: always,
        run: save,
    },
    CommandSpec {
        id: "job.tab.list",
        label: "Open documents",
        tooltip: "The documents open in tabs",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: TABS_RETURNS,
        enabled: always,
        run: |s, _| Ok(tabs_json(s)),
    },
    CommandSpec {
        id: "job.tab.activate",
        label: "Switch document",
        tooltip: "Show the document in another tab",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("index", TAB, "Tab number, from 0")],
        returns: TABS_RETURNS,
        enabled: always,
        run: |s, p| {
            s.activate_tab(tab_index(p, "index")?.unwrap_or(0))?;
            Ok(tabs_json(s))
        },
    },
    CommandSpec {
        id: "job.tab.close",
        label: "Close document",
        tooltip: "Close a document's tab",
        placement: None,
        icon: None,
        shortcut: Some("Ctrl+W"),
        params: &[
            Param::optional("index", TAB, "Tab number, from 0; omit for the active tab"),
            Param::optional("discard", ParamKind::Bool, "Close even though it has unsaved changes"),
        ],
        returns: TABS_RETURNS,
        enabled: always,
        run: |s, p| {
            let index = tab_index(p, "index")?.unwrap_or(s.active_tab());
            let discard = params::opt_bool(p, "discard")?.unwrap_or(false);
            let job = s.tab_job(index).ok_or_else(|| EngineError::InvalidParams(format!("there is no tab {index}")))?;
            if job.is_modified() && !discard {
                return Err(EngineError::InvalidParams(format!(
                    "{} has unsaved changes; save it, or close it with discard",
                    job.name
                )));
            }
            s.close_tab(index)?;
            Ok(tabs_json(s))
        },
    },
    CommandSpec {
        id: "job.outline.get",
        label: "Job outline",
        tooltip: "The job tree shown in the explorer",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "list of nodes {id, label, properties: [{name, value}], children}",
        enabled: always,
        run: |s, _| Ok(json!(s.outline().iter().map(OutlineNode::to_json).collect::<Vec<_>>())),
    },
    CommandSpec {
        id: "selection.set",
        label: "Select",
        tooltip: "Select an item in the job outline",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::optional(
            "id",
            ParamKind::Str,
            "Outline node id, or a face, edge or vertex (model.1/face.3, model.1/edge.4, model.1/vertex.2); omit or \
             null to clear the selection",
        )],
        returns: "{\"selection\": id or null}",
        enabled: always,
        run: |s, p| {
            match params::opt_str(p, "id")? {
                None => s.view.selection = None,
                Some(id) => {
                    if s.selection_node(id).is_none() {
                        return Err(EngineError::InvalidParams(format!(
                            "no outline item, face, edge or vertex `{id}`"
                        )));
                    }
                    s.view.selection = Some(id.to_owned());
                }
            }
            Ok(json!({ "selection": s.view.selection }))
        },
    },
    CommandSpec {
        id: "selection.get",
        label: "Selection",
        tooltip: "The selected outline item, face, edge or vertex",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "the selected node {id, label, properties, children}, or null",
        enabled: always,
        run: |s, _| {
            Ok(s.view.selection.as_deref().and_then(|id| s.selection_node(id)).map_or(Value::Null, |n| n.to_json()))
        },
    },
    CommandSpec {
        id: "session.state.get",
        label: "Session state",
        tooltip: "Job, camera, display, theme, selection, ribbon and pane state in one object",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "{job, theme, fps_limit, display, camera, selection, scene_bounds, ribbon_tab, open_panes, outline, log_seq}",
        enabled: always,
        run: |s, _| Ok(s.state_json()),
    },
    CommandSpec {
        id: "command.list",
        label: "List commands",
        tooltip: "Every registered command with its parameters",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::optional("enabled_only", ParamKind::Bool, "Only commands that can run right now")],
        returns: "list of {id, label, tooltip, shortcut, enabled, params, returns, ribbon}",
        enabled: always,
        run: |s, p| {
            let enabled_only = params::opt_bool(p, "enabled_only")?.unwrap_or(false);
            Ok(json!(
                all()
                    .filter(|c| !c.id.starts_with("test."))
                    .filter(|c| !enabled_only || (c.enabled)(s))
                    .map(|c| c.to_json(s))
                    .collect::<Vec<_>>()
            ))
        },
    },
    CommandSpec {
        id: "command.describe",
        label: "Describe command",
        tooltip: "One command's parameters and state",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Command id, e.g. view.camera.preset")],
        returns: "{id, label, tooltip, shortcut, enabled, params, returns, ribbon}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            find(id).map(|c| c.to_json(s)).ok_or_else(|| EngineError::InvalidParams(format!("unknown command `{id}`")))
        },
    },
    CommandSpec {
        id: "console.clear",
        label: "Clear console",
        tooltip: "Clear the command log",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "null",
        enabled: always,
        run: |s, _| {
            s.log_mut().clear();
            Ok(Value::Null)
        },
    },
    CommandSpec {
        id: "console.log.get",
        label: "Command log",
        tooltip: "Commands run so far and their outcomes",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::optional("since", SINCE, "Only entries with seq greater than this"),
            Param::optional("limit", LIMIT, "Most recent entries to return (default 100)"),
        ],
        returns: "list of {seq, command, params, ok, result | error}",
        enabled: always,
        run: |s, p| log_get(s, p),
    },
];

fn log_get(s: &Session, p: &Value) -> EngineResult<Value> {
    let since = p.get("since").and_then(Value::as_u64).unwrap_or(0);
    let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
    let entries: Vec<Value> = s
        .log()
        .iter()
        .filter(|e| e.seq > since)
        .map(|e| match &e.outcome {
            Ok(v) => json!({ "seq": e.seq, "command": e.command, "params": e.params, "ok": true, "result": v }),
            Err(err) => json!({
                "seq": e.seq,
                "command": e.command,
                "params": e.params,
                "ok": false,
                "error": crate::error::error_json(err),
            }),
        })
        .collect();
    let skip = entries.len().saturating_sub(limit);
    Ok(json!(entries.into_iter().skip(skip).collect::<Vec<_>>()))
}

fn tab_index(p: &Value, key: &str) -> EngineResult<Option<usize>> {
    Ok(params::opt_f64(p, key, 0.0, (crate::session::MAX_TABS - 1) as f64)?.map(|i| i as usize))
}

fn tabs_json(s: &Session) -> Value {
    let tabs: Vec<Value> = s
        .tabs()
        .into_iter()
        .enumerate()
        .map(|(i, t)| json!({ "index": i, "name": t.name, "path": t.path, "modified": t.modified, "active": t.active }))
        .collect();
    json!({ "active": s.active_tab(), "tabs": tabs })
}

/// The file name without folder or extension: a document's name.
fn stem(path: &str) -> String {
    std::path::Path::new(path.strip_prefix(crate::session::UPLOAD_PREFIX).unwrap_or(path))
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".into())
}

fn open(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let path = params::req_str(p, "path")?.to_owned();
    // A file that is already open is shown, not opened twice.
    if let Some(index) = s.tabs().iter().position(|t| t.path.as_deref() == Some(path.as_str())) {
        s.activate_tab(index)?;
        return Ok(json!({ "job": s.job.name, "tab": index, "tabs": tabs_json(s)["tabs"] }));
    }
    let text = match s.take_upload(&path) {
        Some((_, bytes)) => String::from_utf8(bytes.to_vec())
            .map_err(|_| EngineError::InvalidParams(format!("{path} is not a job file")))?,
        None => {
            let size = std::fs::metadata(&path)
                .map_err(|e| EngineError::InvalidParams(format!("could not open {path}: {e}")))?
                .len();
            if size > jobfile::MAX_FILE_BYTES {
                return Err(EngineError::InvalidParams(format!("{path} is larger than a job file can be")));
            }
            std::fs::read_to_string(&path)
                .map_err(|e| EngineError::InvalidParams(format!("could not read {path}: {e}")))?
        }
    };
    let (mut job, camera) = jobfile::from_str(&text)?;
    job.name = stem(&path);
    job.path = (!path.starts_with(crate::session::UPLOAD_PREFIX)).then(|| path.clone());
    let tab = s.open_tab(job, camera)?;
    Ok(json!({ "job": s.job.name, "tab": tab, "tabs": tabs_json(s)["tabs"] }))
}

fn save(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let path = match params::opt_str(p, "path")? {
        Some(path) if !path.trim().is_empty() => {
            let path = path.trim();
            if std::path::Path::new(path).extension().is_some() {
                path.to_owned()
            } else {
                format!("{path}.{}", jobfile::EXTENSION)
            }
        }
        _ => s.job.path.clone().ok_or_else(|| {
            EngineError::InvalidParams("this job has not been saved yet; give a path to save it to".into())
        })?,
    };
    let text = jobfile::to_string(&s.job, &s.view.camera);
    std::fs::write(&path, &text).map_err(|e| EngineError::InvalidParams(format!("could not write {path}: {e}")))?;
    s.job.name = stem(&path);
    s.job.path = Some(path.clone());
    s.job.saved_edits = s.job.edits;
    Ok(json!({ "path": path, "bytes": text.len() }))
}
