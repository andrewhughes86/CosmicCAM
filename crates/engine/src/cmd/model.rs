//! Model import and management.

use std::path::Path;
use std::sync::Arc;

use CosmicCAM_io::{Units, import_mesh};
use serde_json::{Value, json};

use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::session::{MAX_MODELS, Model, Session};

/// Longest model name kept.
const MAX_NAME_CHARS: usize = 128;

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "model.import",
        label: "Import model",
        tooltip: "Import STL or OBJ geometry",
        placement: Some(Placement::large(RibbonTab::Setup, "Model")),
        icon: None,
        shortcut: Some("Ctrl+I"),
        params: &[
            Param::required("path", ParamKind::Str, "Model file (.stl or .obj)"),
            Param::optional(
                "units",
                ParamKind::Choice(Units::IDS),
                "Unit the file was authored in (default mm); STL and OBJ store none",
            ),
            Param::optional("name", ParamKind::Str, "Name in the job outline (default: the file name)"),
        ],
        returns: "the model {id, name, source, format, units, triangles, vertices, bounds}",
        enabled: always,
        run: import,
    },
    CommandSpec {
        id: "model.list",
        label: "Models",
        tooltip: "The job's imported models",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "list of {id, name, source, format, units, triangles, vertices, bounds}",
        enabled: always,
        run: |s, _| Ok(json!(s.job.models.iter().map(Model::to_json).collect::<Vec<_>>())),
    },
    CommandSpec {
        id: "model.remove",
        label: "Remove model",
        tooltip: "Remove an imported model from the job",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Model id, e.g. model.1")],
        returns: "{\"removed\": id}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            let before = s.job.models.len();
            s.job.models.retain(|m| m.id != id);
            if s.job.models.len() == before {
                return Err(EngineError::InvalidParams(format!("no model `{id}`")));
            }
            if s.view.selection.as_deref() == Some(id) {
                s.view.selection = None;
            }
            s.job.geometry_changed();
            Ok(json!({ "removed": id }))
        },
    },
];

fn import(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let path = params::req_str(p, "path")?;
    let units = match params::opt_str(p, "units")? {
        Some(u) => Units::from_id(u).ok_or_else(|| EngineError::InvalidParams(format!("unknown units `{u}`")))?,
        None => Units::Millimetres,
    };
    if s.job.models.len() >= MAX_MODELS {
        return Err(EngineError::InvalidParams(format!("a job holds at most {MAX_MODELS} models")));
    }
    let path = Path::new(path);
    let (mesh, format) = import_mesh(path, units).map_err(|e| EngineError::Import(e.to_string()))?;
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = match params::opt_str(p, "name")? {
        Some(n) if !n.trim().is_empty() => n.trim().to_owned(),
        _ => path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| file_name.clone()),
    };
    let model = Model {
        id: format!("model.{}", s.job.next_model),
        name: name.chars().take(MAX_NAME_CHARS).collect(),
        source: path.display().to_string(),
        format,
        units,
        mesh: Arc::new(mesh),
    };
    s.job.next_model = s.job.next_model.saturating_add(1);
    // The placeholder part stands in for a real model; the first import replaces it.
    s.job.sample_loaded = false;
    if s.view.selection.as_deref() == Some("model.sample") {
        s.view.selection = None;
    }
    let result = model.to_json();
    s.job.models.push(model);
    s.job.geometry_changed();
    Ok(result)
}
