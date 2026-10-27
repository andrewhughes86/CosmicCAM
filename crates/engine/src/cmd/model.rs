//! Model import and management.

use std::path::Path;
use std::sync::Arc;

use CosmicCAM_io::{DEFAULT_TOLERANCE, ModelFormat, Options, Units, import_mesh, import_mesh_bytes};
use serde_json::{Value, json};

use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::session::{MAX_MODELS, Model, SAMPLE_MODEL_ID, Session, edge_json, face_json, vertex_json};

/// Longest model name kept.
const MAX_NAME_CHARS: usize = 128;

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "model.import",
        label: "Import model",
        tooltip: "Import a CAD model (STEP, IGES, SAT) or mesh (STL, OBJ, 3MF, PLY, glTF and more)",
        placement: Some(Placement::large(RibbonTab::Setup, "Model")),
        icon: None,
        shortcut: Some("Ctrl+I"),
        params: &[
            Param::required(
                "path",
                ParamKind::Str,
                "Model file (any format `model.formats` lists, optionally gzipped), or upload://<name> for a file staged by the browser app",
            ),
            Param::optional(
                "units",
                ParamKind::Choice(Units::IDS),
                "Unit the file was authored in; files that record theirs (STEP, IGES, SAT, 3MF, AMF, glTF, COLLADA, DXF) use it, others default to mm",
            ),
            Param::optional(
                "tolerance",
                ParamKind::Length { min: 0.0001, max: 10.0 },
                "Most a tessellated CAD surface (STEP, IGES, SAT) may stray from the exact one, mm (default 0.01)",
            ),
            Param::optional("name", ParamKind::Str, "Name in the job outline (default: the file name)"),
        ],
        returns: "the model {id, name, source, format, units, triangles, vertices, faces, bounds, warnings}",
        enabled: always,
        run: import,
    },
    CommandSpec {
        id: "model.formats",
        label: "Model formats",
        tooltip: "The file formats model.import reads",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "list of {id, label, extensions, cad} (cad: exact geometry, tessellated on import)",
        enabled: always,
        run: |_, _| {
            Ok(json!(
                ModelFormat::ALL
                    .iter()
                    .map(
                        |f| json!({ "id": f.id(), "label": f.label(), "extensions": f.extensions(), "cad": f.is_cad() })
                    )
                    .collect::<Vec<_>>()
            ))
        },
    },
    CommandSpec {
        id: "model.list",
        label: "Models",
        tooltip: "The job's imported models",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "list of {id, name, source, format, units, triangles, vertices, faces, bounds}",
        enabled: always,
        run: |s, _| Ok(json!(s.job.models.iter().map(Model::to_json).collect::<Vec<_>>())),
    },
    CommandSpec {
        id: "model.visibility.set",
        label: "Show or hide model",
        tooltip: "Show or hide a model in the viewport; hidden models still count for toolpaths",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("id", ParamKind::Str, "Model id, e.g. model.1"),
            Param::optional("visible", ParamKind::Bool, "Show it (true) or hide it (false); omit to toggle"),
        ],
        returns: "{\"id\": id, \"visible\": bool}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?.to_owned();
            let want = params::opt_bool(p, "visible")?;
            let m = s
                .job
                .models
                .iter_mut()
                .find(|m| m.id == id)
                .ok_or_else(|| EngineError::InvalidParams(format!("no model `{id}`")))?;
            m.visible = want.unwrap_or(!m.visible);
            let visible = m.visible;
            s.job.geometry_changed();
            Ok(json!({ "id": id, "visible": visible }))
        },
    },
    CommandSpec {
        id: "model.rename",
        label: "Rename model",
        tooltip: "Change a model's name",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("id", ParamKind::Str, "Model id, e.g. model.1"),
            Param::required("name", ParamKind::Str, "New name"),
        ],
        returns: "{\"id\": id, \"name\": name}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?.to_owned();
            let name: String = params::req_str(p, "name")?.trim().chars().take(MAX_NAME_CHARS).collect();
            if name.is_empty() {
                return Err(EngineError::InvalidParams("give the model a name".into()));
            }
            let m = s
                .job
                .models
                .iter_mut()
                .find(|m| m.id == id)
                .ok_or_else(|| EngineError::InvalidParams(format!("no model `{id}`")))?;
            m.name = name.clone();
            Ok(json!({ "id": id, "name": name }))
        },
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
            if id == SAMPLE_MODEL_ID {
                s.job.sample_loaded = false;
                s.job.set_stock(None);
            }
            if s.view.selection.as_deref() == Some(id) {
                s.view.selection = None;
            }
            s.job.geometry_changed();
            Ok(json!({ "removed": id }))
        },
    },
    CommandSpec {
        id: "model.faces.list",
        label: "Model faces",
        tooltip: "The planes, cylinders and freeform patches recognised on a model",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("id", ParamKind::Str, "Model id, e.g. model.1"),
            Param::optional(
                "kind",
                ParamKind::Choice(&["plane", "cylinder", "sphere", "freeform"]),
                "Only faces of this kind",
            ),
        ],
        returns: "list of faces {face, model, kind, area, centroid, triangles, ...}: planes add normal and point; \
                  cylinders add base, axis, radius, diameter, length and hole; spheres add center, radius, diameter \
                  and convex (at most 5000)",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            let kind = params::opt_str(p, "kind")?;
            let m = s.job.model(id).ok_or_else(|| EngineError::InvalidParams(format!("no model `{id}`")))?;
            Ok(json!(
                m.faces
                    .faces
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| kind.is_none_or(|k| f.kind.id() == k))
                    .take(MAX_LISTED_FACES)
                    .map(|(i, _)| face_json(m, i))
                    .collect::<Vec<_>>()
            ))
        },
    },
    CommandSpec {
        id: "model.face.get",
        label: "Face",
        tooltip: "One recognised face: its kind and geometry",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("face", ParamKind::Str, "Face reference, e.g. model.1/face.3")],
        returns: "{face, model, kind, area, centroid, triangles, ...} as model.faces.list",
        enabled: always,
        run: |s, p| {
            let r = params::req_str(p, "face")?;
            let (m, f) = s.job.face(r).ok_or_else(|| EngineError::InvalidParams(format!("no face `{r}`")))?;
            Ok(face_json(m, f))
        },
    },
    CommandSpec {
        id: "model.pick",
        label: "Pick",
        tooltip: "The model face a ray hits first, as a click in the viewport picks it",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("origin", ParamKind::Vec3, "Ray start, world mm"),
            Param::required("direction", ParamKind::Vec3, "Ray direction"),
        ],
        returns: "{face, model, kind, ..., point, distance} for the face hit, or null",
        enabled: always,
        run: |s, p| {
            let origin = params::opt_vec3(p, "origin")?.unwrap_or_default();
            let direction = params::opt_vec3(p, "direction")?.unwrap_or_default();
            if direction.length() <= 1.0e-9 {
                return Err(EngineError::InvalidParams("`direction` must have a direction".into()));
            }
            Ok(s.job.pick(origin, direction).map_or(Value::Null, |hit| {
                let mut v = s.job.models.get(hit.model).map_or(Value::Null, |m| face_json(m, hit.face));
                v["point"] = json!(hit.point.to_array());
                v["distance"] = json!(hit.distance);
                v
            }))
        },
    },
    CommandSpec {
        id: "model.edges.list",
        label: "Model edges",
        tooltip: "The straight and circular edges recognised on a model",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("id", ParamKind::Str, "Model id, e.g. model.1"),
            Param::optional("kind", ParamKind::Choice(&["line", "circle"]), "Only edges of this kind"),
        ],
        returns: "list of edges {edge, model, kind, length, direction, ...}: lines add start and end; \
                  circles add center, normal and radius (at most 5000)",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            let kind = params::opt_str(p, "kind")?;
            let m = s.job.model(id).ok_or_else(|| EngineError::InvalidParams(format!("no model `{id}`")))?;
            Ok(json!(
                m.edges
                    .edges
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| kind.is_none_or(|k| e.kind.id() == k))
                    .take(MAX_LISTED_FACES)
                    .map(|(i, _)| edge_json(m, i))
                    .collect::<Vec<_>>()
            ))
        },
    },
    CommandSpec {
        id: "model.edge.get",
        label: "Edge",
        tooltip: "One recognised edge: its kind and geometry",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("edge", ParamKind::Str, "Edge reference, e.g. model.1/edge.4")],
        returns: "{edge, model, kind, length, direction, ...} as model.edges.list",
        enabled: always,
        run: |s, p| {
            let r = params::req_str(p, "edge")?;
            let (m, e) = s.job.edge(r).ok_or_else(|| EngineError::InvalidParams(format!("no edge `{r}`")))?;
            Ok(edge_json(m, e))
        },
    },
    CommandSpec {
        id: "model.edge.pick",
        label: "Pick edge",
        tooltip: "The visible edge passing closest to a ray, as a click in the viewport picks it",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("origin", ParamKind::Vec3, "Ray start, world mm"),
            Param::required("direction", ParamKind::Vec3, "Ray direction"),
            Param::optional(
                "radius",
                ParamKind::Length { min: 0.0001, max: 1.0e6 },
                "How far from the ray an edge may be, mm (default 1)",
            ),
        ],
        returns: "{edge, model, kind, length, direction, ...} for the edge found, or null",
        enabled: always,
        run: |s, p| {
            let origin = params::opt_vec3(p, "origin")?.unwrap_or_default();
            let direction = params::opt_vec3(p, "direction")?.unwrap_or_default();
            if direction.length() <= 1.0e-9 {
                return Err(EngineError::InvalidParams("`direction` must have a direction".into()));
            }
            let radius = params::opt_f64(p, "radius", 0.0001, 1.0e6)?.unwrap_or(1.0) as f32;
            Ok(s.job
                .pick_edge(origin, direction, radius)
                .map_or(Value::Null, |(mi, e)| s.job.models.get(mi).map_or(Value::Null, |m| edge_json(m, e))))
        },
    },
    CommandSpec {
        id: "model.vertices.list",
        label: "Model vertices",
        tooltip: "The corners recognised on a model, where straight edges end",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Model id, e.g. model.1")],
        returns: "list of {vertex, model, kind, position} (at most 5000)",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            let m = s.job.model(id).ok_or_else(|| EngineError::InvalidParams(format!("no model `{id}`")))?;
            Ok(json!((0..m.edges.vertices.len()).take(MAX_LISTED_FACES).map(|i| vertex_json(m, i)).collect::<Vec<_>>()))
        },
    },
    CommandSpec {
        id: "model.vertex.pick",
        label: "Pick vertex",
        tooltip: "The visible vertex closest to a ray",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("origin", ParamKind::Vec3, "Ray start, world mm"),
            Param::required("direction", ParamKind::Vec3, "Ray direction"),
            Param::optional(
                "radius",
                ParamKind::Length { min: 0.0001, max: 1.0e6 },
                "How far from the ray a vertex may be, mm (default 1)",
            ),
        ],
        returns: "{vertex, model, kind, position} for the vertex found, or null",
        enabled: always,
        run: |s, p| {
            let origin = params::opt_vec3(p, "origin")?.unwrap_or_default();
            let direction = params::opt_vec3(p, "direction")?.unwrap_or_default();
            if direction.length() <= 1.0e-9 {
                return Err(EngineError::InvalidParams("`direction` must have a direction".into()));
            }
            let radius = params::opt_f64(p, "radius", 0.0001, 1.0e6)?.unwrap_or(1.0) as f32;
            Ok(s.job
                .pick_vertex(origin, direction, radius)
                .map_or(Value::Null, |(mi, v)| s.job.models.get(mi).map_or(Value::Null, |m| vertex_json(m, v))))
        },
    },
];

/// Most faces `model.faces.list` returns.
const MAX_LISTED_FACES: usize = 5000;

fn import(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let path = params::req_str(p, "path")?;
    let units = match params::opt_str(p, "units")? {
        Some(u) => Some(Units::from_id(u).ok_or_else(|| EngineError::InvalidParams(format!("unknown units `{u}`")))?),
        None => None,
    };
    let options = Options { tolerance: params::opt_f64(p, "tolerance", 0.0001, 10.0)?.unwrap_or(DEFAULT_TOLERANCE) };
    if s.job.models.len() >= MAX_MODELS {
        return Err(EngineError::InvalidParams(format!("a job holds at most {MAX_MODELS} models")));
    }
    let staged = s.take_upload(path);
    let path = Path::new(path);
    let (imported, format) = match &staged {
        Some((name, bytes)) => import_mesh_bytes(name, bytes, units, options),
        None => import_mesh(path, units, options),
    }
    .map_err(|e| EngineError::Import(e.to_string()))?;
    let units = imported.units.unwrap_or(Units::Millimetres);
    let (mesh, warnings) = (imported.mesh, imported.warnings);
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = match params::opt_str(p, "name")? {
        Some(n) if !n.trim().is_empty() => n.trim().to_owned(),
        _ => path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| file_name.clone()),
    };
    let (faces, edges) = Model::recognise(&mesh);
    let model = Model {
        id: format!("model.{}", s.job.next_model),
        name: name.chars().take(MAX_NAME_CHARS).collect(),
        source: path.display().to_string(),
        format: Some(format),
        units,
        mesh: Arc::new(mesh),
        faces,
        edges,
        visible: true,
    };
    s.job.next_model = s.job.next_model.saturating_add(1);
    // The placeholder part stands in for a real model; the first import replaces it.
    // The sample's stock goes with it; a real model gets its own.
    if s.job.sample_loaded {
        s.job.set_stock(None);
    }
    s.job.sample_loaded = false;
    s.job.models.retain(|m| m.id != SAMPLE_MODEL_ID);
    if s.view.selection.as_deref() == Some(SAMPLE_MODEL_ID) {
        s.view.selection = None;
    }
    let mut result = model.to_json();
    if let Some(o) = result.as_object_mut() {
        o.insert("warnings".into(), json!(warnings));
    }
    s.job.models.push(model);
    s.job.geometry_changed();
    Ok(result)
}
