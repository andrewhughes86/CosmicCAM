//! CAD commands: sketches and the entities drawn on them, extrusions and revolutions that add
//! to or cut from the part, and the feature history (list, rename, suppress, remove, rebuild).

use serde_json::{Map, Value, json};

use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::cad::{self, AXES, DIRECTIONS, Feature, MAX_ENTITIES, MAX_FEATURES, OPERATIONS, PLANES};
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::session::Session;

const LENGTH: ParamKind = ParamKind::Length { min: 0.0001, max: 1.0e6 };
const COORD: ParamKind = ParamKind::Length { min: -1.0e6, max: 1.0e6 };

const SKETCH_KEYS: &[&str] = &["plane", "origin", "normal", "x_axis", "offset", "entities"];
const EXTRUDE_KEYS: &[&str] = &["sketch", "distance", "direction", "operation", "through_all"];
const REVOLVE_KEYS: &[&str] = &["sketch", "axis", "line", "angle", "operation"];

fn bad(m: impl Into<String>) -> EngineError {
    EngineError::InvalidParams(m.into())
}

const FEATURE_ID: Param = Param::optional("id", ParamKind::Str, "Change this feature instead of adding one");
const FEATURE_NAME: Param =
    Param::optional("name", ParamKind::Str, "Name in the feature tree (default: Sketch1, Extrude1…)");

const SKETCH_PARAMS: &[Param] = &[
    FEATURE_ID,
    FEATURE_NAME,
    Param::optional(
        "plane",
        ParamKind::Choice(PLANES),
        "Plane to sketch on: top (XY), front (XZ), right (YZ) or custom (origin, normal, x_axis); default top",
    ),
    Param::optional("origin", ParamKind::Vec3, "Custom plane: a point on it, world mm"),
    Param::optional("normal", ParamKind::Vec3, "Custom plane: its normal"),
    Param::optional("x_axis", ParamKind::Vec3, "Custom plane: its X direction (default: any square to the normal)"),
    Param::optional("offset", COORD, "Move the plane along its normal, mm"),
    Param::optional(
        "entities",
        ParamKind::Objects { max: MAX_ENTITIES },
        "Entities in plane coordinates: {type: line, from, to}, {type: rectangle, corner, opposite}, \
         {type: circle, center, radius}, {type: arc, center, start, end, clockwise?}, \
         {type: polygon, center, radius, sides, angle?}; points are [x, y]; construction: true leaves one out \
         of the profile",
    ),
];

const EXTRUDE_PARAMS: &[Param] = &[
    FEATURE_ID,
    FEATURE_NAME,
    Param::optional("sketch", ParamKind::Str, "The sketch to extrude (default: the last one)"),
    Param::optional("distance", LENGTH, "How far, mm (default 10)"),
    Param::optional(
        "direction",
        ParamKind::Choice(DIRECTIONS),
        "Along the sketch normal, against it, or half each way (default normal)",
    ),
    Param::optional(
        "operation",
        ParamKind::Choice(OPERATIONS),
        "Join it to the body, cut it away, or keep it as a new body (default join; cut for Cut extrude)",
    ),
    Param::optional("through_all", ParamKind::Bool, "Go right through the part both ways (cuts)"),
];

const REVOLVE_PARAMS: &[Param] = &[
    FEATURE_ID,
    FEATURE_NAME,
    Param::optional("sketch", ParamKind::Str, "The sketch to revolve (default: the last one)"),
    Param::optional(
        "axis",
        ParamKind::Choice(AXES),
        "Axis: the sketch's X or Y axis, or one of its lines (`line`); default y",
    ),
    Param::optional(
        "line",
        ParamKind::Integer { min: 0, max: MAX_ENTITIES as i64 },
        "The sketch entity (a line, numbered from 0) to turn about, for axis line",
    ),
    Param::optional("angle", ParamKind::Number { min: 0.001, max: 360.0 }, "Degrees (default 360)"),
    Param::optional(
        "operation",
        ParamKind::Choice(OPERATIONS),
        "Join it to the body, cut it away, or keep it as a new body (default join; cut for Cut revolve)",
    ),
];

/// The settings in `p` under `keys`.
fn settings(p: &Value, keys: &[&str]) -> Map<String, Value> {
    let mut m = Map::new();
    if let Some(o) = p.as_object() {
        for k in keys {
            if let Some(v) = o.get(*k).filter(|v| !v.is_null()) {
                m.insert((*k).to_owned(), v.clone());
            }
        }
    }
    m
}

/// The last sketch in the history.
fn last_sketch(s: &Session) -> Option<String> {
    s.job.part.features.iter().rev().find(|f| f.kind == "sketch" && !f.suppressed).map(|f| f.id.clone())
}

/// Adds feature `kind` with `settings`, or changes the one `id` names, and rebuilds the part;
/// if it cannot be built nothing changes.
fn commit(s: &mut Session, p: &Value, kind: &str, label: &str, settings: Map<String, Value>) -> EngineResult<Value> {
    let edit = params::opt_str(p, "id")?.map(str::to_owned);
    let name = params::opt_str(p, "name")?.map(crate::library::clip).filter(|n| !n.is_empty());
    let before = s.job.part.clone();
    let id = match &edit {
        Some(id) => {
            let f = s
                .job
                .part
                .features
                .iter_mut()
                .find(|f| &f.id == id)
                .ok_or_else(|| bad(format!("no feature `{id}`")))?;
            if f.kind != kind {
                return Err(bad(format!("{} is a {}, not a {kind}", f.name, f.kind)));
            }
            f.params = Value::Object(settings);
            if let Some(n) = name {
                f.name = n;
            }
            id.clone()
        }
        None => {
            if s.job.part.features.len() >= MAX_FEATURES {
                return Err(bad(format!("a part holds at most {MAX_FEATURES} features")));
            }
            let part = &mut s.job.part;
            part.next = part.next.saturating_add(1);
            let id = format!("feature.{}", part.next);
            let name = name.unwrap_or_else(|| part.new_name(label));
            part.features.push(Feature {
                id: id.clone(),
                name,
                kind: kind.into(),
                params: Value::Object(settings),
                suppressed: false,
            });
            id
        }
    };
    match cad::update_model(&mut s.job) {
        Ok(triangles) => {
            let f = s.job.part.find(&id).map(Feature::to_json).unwrap_or(Value::Null);
            Ok(json!({ "feature": f, "triangles": triangles }))
        }
        Err(e) => {
            s.job.part = before;
            Err(e)
        }
    }
}

fn sketch_feature(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let m = settings(p, SKETCH_KEYS);
    cad::sketch_of(&Value::Object(m.clone()))?;
    commit(s, p, "sketch", "Sketch", m)
}

fn extrude(s: &mut Session, p: &Value, default_op: &str) -> EngineResult<Value> {
    let mut m = settings(p, EXTRUDE_KEYS);
    fill(s, &mut m, default_op)?;
    m.entry("distance").or_insert(json!(10.0));
    commit(s, p, "extrude", if default_op == "cut" { "Cut" } else { "Extrude" }, m)
}

fn revolve(s: &mut Session, p: &Value, default_op: &str) -> EngineResult<Value> {
    let mut m = settings(p, REVOLVE_KEYS);
    fill(s, &mut m, default_op)?;
    commit(s, p, "revolve", if default_op == "cut" { "Cut-Revolve" } else { "Revolve" }, m)
}

/// The sketch and operation a feature uses when not given.
fn fill(s: &Session, m: &mut Map<String, Value>, default_op: &str) -> EngineResult<()> {
    if !m.contains_key("sketch") {
        let sketch = last_sketch(s).ok_or_else(|| bad("draw a sketch first"))?;
        m.insert("sketch".into(), json!(sketch));
    }
    m.entry("operation").or_insert(json!(default_op));
    Ok(())
}

/// Adds entity `e` to sketch `id`.
fn add_entity(s: &mut Session, p: &Value, e: Value) -> EngineResult<Value> {
    let id = params::req_str(p, "id")?.to_owned();
    let f = s.job.part.find(&id).filter(|f| f.kind == "sketch").ok_or_else(|| bad(format!("no sketch `{id}`")))?;
    cad::entity(&e)?;
    let mut params = f.params.clone();
    let list = params.as_object_mut().ok_or_else(|| EngineError::Internal("sketch settings".into()))?;
    let entities = list.entry("entities").or_insert(json!([]));
    let arr = entities.as_array_mut().ok_or_else(|| bad("the sketch's entities are not a list"))?;
    if arr.len() >= MAX_ENTITIES {
        return Err(bad(format!("a sketch holds at most {MAX_ENTITIES} entities")));
    }
    arr.push(e);
    let index = arr.len() - 1;
    let settings = list.clone();
    let mut r = commit(s, &json!({ "id": id }), "sketch", "Sketch", settings)?;
    r["entity"] = json!(index);
    Ok(r)
}

fn xy(p: &Value, key: &str) -> EngineResult<Value> {
    let v = params::opt_vec3(p, key)?.ok_or_else(|| bad(format!("give `{key}` as [x, y, 0] in the sketch plane")))?;
    Ok(json!([v.x, v.y]))
}

fn find_mut<'a>(s: &'a mut Session, id: &str) -> EngineResult<&'a mut Feature> {
    s.job.part.features.iter_mut().find(|f| f.id == id).ok_or_else(|| bad(format!("no feature `{id}`")))
}

const SKETCH_ID: Param = Param::required("id", ParamKind::Str, "The sketch, e.g. feature.1");
const POINT: ParamKind = ParamKind::Vec3;

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "cad.sketch",
        label: "Sketch",
        tooltip: "Draw a closed profile on a plane: lines, rectangles, circles, arcs and polygons to extrude or revolve",
        placement: Some(Placement::large(RibbonTab::Sketch, "Sketch")),
        icon: None,
        shortcut: None,
        params: SKETCH_PARAMS,
        returns: "{feature: {id, name, kind, params, suppressed}, triangles}",
        enabled: always,
        run: sketch_feature,
    },
    CommandSpec {
        id: "cad.sketch.line",
        label: "Line",
        tooltip: "Add a line to a sketch",
        placement: Some(Placement::large(RibbonTab::Sketch, "Draw")),
        icon: None,
        shortcut: None,
        params: &[
            SKETCH_ID,
            Param::required("start", POINT, "Start, [x, y, 0] in the sketch plane"),
            Param::required("end", POINT, "End, [x, y, 0] in the sketch plane"),
        ],
        returns: "{feature, triangles, entity: its number}",
        enabled: always,
        run: |s, p| {
            let e = json!({ "type": "line", "from": xy(p, "start")?, "to": xy(p, "end")? });
            add_entity(s, p, e)
        },
    },
    CommandSpec {
        id: "cad.sketch.rectangle",
        label: "Rectangle",
        tooltip: "Add a rectangle to a sketch, from two opposite corners",
        placement: Some(Placement::large(RibbonTab::Sketch, "Draw")),
        icon: None,
        shortcut: None,
        params: &[
            SKETCH_ID,
            Param::required("corner", POINT, "One corner, [x, y, 0]"),
            Param::required("opposite", POINT, "The opposite corner, [x, y, 0]"),
        ],
        returns: "{feature, triangles, entity: its number}",
        enabled: always,
        run: |s, p| {
            let e = json!({ "type": "rectangle", "corner": xy(p, "corner")?, "opposite": xy(p, "opposite")? });
            add_entity(s, p, e)
        },
    },
    CommandSpec {
        id: "cad.sketch.circle",
        label: "Circle",
        tooltip: "Add a circle to a sketch, from its centre and radius",
        placement: Some(Placement::large(RibbonTab::Sketch, "Draw")),
        icon: None,
        shortcut: None,
        params: &[
            SKETCH_ID,
            Param::required("center", POINT, "Centre, [x, y, 0]"),
            Param::required("radius", LENGTH, "Radius, mm"),
        ],
        returns: "{feature, triangles, entity: its number}",
        enabled: always,
        run: |s, p| {
            let r = params::opt_f64(p, "radius", 0.0001, 1.0e6)?.ok_or_else(|| bad("give the `radius`"))?;
            let e = json!({ "type": "circle", "center": xy(p, "center")?, "radius": r });
            add_entity(s, p, e)
        },
    },
    CommandSpec {
        id: "cad.sketch.arc",
        label: "Arc",
        tooltip: "Add an arc to a sketch, from its centre, start and end",
        placement: Some(Placement::large(RibbonTab::Sketch, "Draw")),
        icon: None,
        shortcut: None,
        params: &[
            SKETCH_ID,
            Param::required("center", POINT, "Centre, [x, y, 0]"),
            Param::required("start", POINT, "Start, [x, y, 0]; sets the radius"),
            Param::required("end", POINT, "End, [x, y, 0]; on the line from the centre"),
            Param::optional("clockwise", ParamKind::Bool, "Run clockwise from the start (default counter-clockwise)"),
        ],
        returns: "{feature, triangles, entity: its number}",
        enabled: always,
        run: |s, p| {
            let cw = params::opt_bool(p, "clockwise")?.unwrap_or(false);
            let e = json!({ "type": "arc", "center": xy(p, "center")?, "start": xy(p, "start")?, "end": xy(p, "end")?, "clockwise": cw });
            add_entity(s, p, e)
        },
    },
    CommandSpec {
        id: "cad.sketch.polygon",
        label: "Polygon",
        tooltip: "Add a regular polygon to a sketch, from its centre, corner radius and number of sides",
        placement: Some(Placement::large(RibbonTab::Sketch, "Draw")),
        icon: None,
        shortcut: None,
        params: &[
            SKETCH_ID,
            Param::required("center", POINT, "Centre, [x, y, 0]"),
            Param::required("radius", LENGTH, "Distance from the centre to each corner, mm"),
            Param::optional("sides", ParamKind::Integer { min: 3, max: 360 }, "Number of sides (default 6)"),
            Param::optional(
                "angle",
                ParamKind::Number { min: -360.0, max: 360.0 },
                "Angle of the first corner, degrees (default 90)",
            ),
        ],
        returns: "{feature, triangles, entity: its number}",
        enabled: always,
        run: |s, p| {
            let r = params::opt_f64(p, "radius", 0.0001, 1.0e6)?.ok_or_else(|| bad("give the `radius`"))?;
            let sides = params::opt_f64(p, "sides", 3.0, 360.0)?.unwrap_or(6.0);
            let angle = params::opt_f64(p, "angle", -360.0, 360.0)?.unwrap_or(90.0);
            let e =
                json!({ "type": "polygon", "center": xy(p, "center")?, "radius": r, "sides": sides, "angle": angle });
            add_entity(s, p, e)
        },
    },
    CommandSpec {
        id: "cad.extrude",
        label: "Extrude",
        tooltip: "Pull a sketch's closed profile out into a solid, joined to the part",
        placement: Some(Placement::large(RibbonTab::Features, "Features")),
        icon: None,
        shortcut: None,
        params: EXTRUDE_PARAMS,
        returns: "{feature, triangles}",
        enabled: always,
        run: |s, p| extrude(s, p, "join"),
    },
    CommandSpec {
        id: "cad.revolve",
        label: "Revolve",
        tooltip: "Turn a sketch's closed profile about an axis into a solid, joined to the part",
        placement: Some(Placement::large(RibbonTab::Features, "Features")),
        icon: None,
        shortcut: None,
        params: REVOLVE_PARAMS,
        returns: "{feature, triangles}",
        enabled: always,
        run: |s, p| revolve(s, p, "join"),
    },
    CommandSpec {
        id: "cad.extrude.cut",
        label: "Cut extrude",
        tooltip: "Take away what a sketch's profile sweeps through, straight along the sketch normal",
        placement: Some(Placement::large(RibbonTab::Features, "Features")),
        icon: None,
        shortcut: None,
        params: EXTRUDE_PARAMS,
        returns: "{feature, triangles}",
        enabled: |s| s.job.part.model.is_some(),
        run: |s, p| extrude(s, p, "cut"),
    },
    CommandSpec {
        id: "cad.revolve.cut",
        label: "Cut revolve",
        tooltip: "Take away what a sketch's profile sweeps through, turned about an axis",
        placement: Some(Placement::large(RibbonTab::Features, "Features")),
        icon: None,
        shortcut: None,
        params: REVOLVE_PARAMS,
        returns: "{feature, triangles}",
        enabled: |s| s.job.part.model.is_some(),
        run: |s, p| revolve(s, p, "cut"),
    },
    CommandSpec {
        id: "cad.rebuild",
        label: "Rebuild",
        tooltip: "Work the part out again from its features",
        placement: Some(Placement::large(RibbonTab::Features, "Part")),
        icon: Some("toolpath_regenerate"),
        shortcut: Some("Ctrl+B"),
        params: &[],
        returns: "{features, triangles}",
        enabled: always,
        run: |s, _| {
            let triangles = cad::update_model(&mut s.job)?;
            Ok(json!({ "features": s.job.part.features.len(), "triangles": triangles }))
        },
    },
    CommandSpec {
        id: "cad.feature.list",
        label: "Features",
        tooltip: "The part's features in the order they are built",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "[{id, name, kind, params, suppressed}]",
        enabled: always,
        run: |s, _| Ok(json!(s.job.part.features.iter().map(Feature::to_json).collect::<Vec<_>>())),
    },
    CommandSpec {
        id: "cad.feature.get",
        label: "Feature",
        tooltip: "One feature's settings; in the app this opens it for editing",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Feature id, e.g. feature.2")],
        returns: "{id, name, kind, params, suppressed}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            s.job.part.find(id).map(Feature::to_json).ok_or_else(|| bad(format!("no feature `{id}`")))
        },
    },
    CommandSpec {
        id: "cad.feature.rename",
        label: "Rename feature",
        tooltip: "Change a feature's name",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("id", ParamKind::Str, "Feature id"),
            Param::required("name", ParamKind::Str, "New name"),
        ],
        returns: "{id, name}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?.to_owned();
            let name = crate::library::clip(params::req_str(p, "name")?);
            if name.is_empty() {
                return Err(bad("give the feature a name"));
            }
            find_mut(s, &id)?.name = name.clone();
            Ok(json!({ "id": id, "name": name }))
        },
    },
    CommandSpec {
        id: "cad.feature.suppress",
        label: "Suppress feature",
        tooltip: "Leave a feature out of the part (or put it back) without deleting it",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("id", ParamKind::Str, "Feature id"),
            Param::optional(
                "suppressed",
                ParamKind::Bool,
                "Leave it out (true) or put it back (false); omit to toggle",
            ),
        ],
        returns: "{id, suppressed, triangles}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?.to_owned();
            let want = params::opt_bool(p, "suppressed")?;
            let before = s.job.part.clone();
            let f = find_mut(s, &id)?;
            f.suppressed = want.unwrap_or(!f.suppressed);
            let suppressed = f.suppressed;
            match cad::update_model(&mut s.job) {
                Ok(triangles) => Ok(json!({ "id": id, "suppressed": suppressed, "triangles": triangles })),
                Err(e) => {
                    s.job.part = before;
                    Err(e)
                }
            }
        },
    },
    CommandSpec {
        id: "cad.feature.remove",
        label: "Delete feature",
        tooltip: "Delete a feature from the part's history",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Feature id")],
        returns: "{removed: id, triangles}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?.to_owned();
            let f = s.job.part.find(&id).ok_or_else(|| bad(format!("no feature `{id}`")))?;
            let users: Vec<String> = s
                .job
                .part
                .features
                .iter()
                .filter(|g| g.params.get("sketch").and_then(Value::as_str) == Some(id.as_str()))
                .map(|g| g.name.clone())
                .collect();
            if !users.is_empty() {
                return Err(bad(format!("{} is used by {}; delete those first", f.name, users.join(", "))));
            }
            let before = s.job.part.clone();
            s.job.part.features.retain(|g| g.id != id);
            match cad::update_model(&mut s.job) {
                Ok(triangles) => {
                    if s.view.selection.as_deref() == Some(id.as_str()) {
                        s.view.selection = None;
                    }
                    Ok(json!({ "removed": id, "triangles": triangles }))
                }
                Err(e) => {
                    s.job.part = before;
                    Err(e)
                }
            }
        },
    },
    CommandSpec {
        id: "cad.mass.get",
        label: "Mass properties",
        tooltip: "The part's volume, surface area, centre of mass and size",
        placement: Some(Placement::large(RibbonTab::Evaluate, "Measure")),
        icon: None,
        shortcut: None,
        params: &[],
        returns: "{volume, area, center_of_mass, bounds}",
        enabled: |s| s.job.part.model.is_some(),
        run: |s, _| {
            let id = s.job.part.model.as_deref().ok_or_else(|| bad("there is no part yet"))?;
            let m = s.job.models.iter().find(|m| m.id == id).ok_or_else(|| bad("there is no part yet"))?;
            Ok(cad::mass_properties(&m.mesh))
        },
    },
];
