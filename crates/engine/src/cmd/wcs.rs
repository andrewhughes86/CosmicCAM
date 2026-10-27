//! Work coordinate systems: creating, placing, orienting and choosing the WCS toolpaths are
//! programmed in.

use CosmicDMS_geom::{Aabb, Quat, Vec3};
use serde_json::{Value, json};

use super::wcs_define::{define_params, resolve_axes, resolve_origin};
use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::session::{MAX_WCS, Session, Wcs};

/// Longest WCS name kept.
const MAX_NAME_CHARS: usize = 64;
/// Shorter direction vectors are rejected as having no direction.
const MIN_AXIS_LENGTH: f32 = 1.0e-6;

const CORNERS: &[&str] = &["front_left", "front_right", "back_left", "back_right"];
const AXES: &[&str] = &["x", "y", "z"];
const FRAMES: &[&str] = &["wcs", "world"];

/// Quick placements offered from the Create WCS button's menu.
pub(crate) const CREATE_MENU: &[&str] = &["setup.wcs.top_center", "setup.wcs.bottom_corner"];
const DEGREES: ParamKind = ParamKind::Number { min: -36_000.0, max: 36_000.0 };
const WCS_RETURNS: &str = "the WCS {id, name, active, origin, x_axis, y_axis, z_axis}";

const ID_PARAM: Param = Param::optional("id", ParamKind::Str, "WCS id, e.g. wcs.1 (default: the active WCS)");

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "setup.wcs.create",
        label: "Create WCS",
        tooltip: "Define a work coordinate system: its orientation (axes) and origin",
        placement: Some(Placement::large(RibbonTab::Setup, "Coordinates").with_menu(CREATE_MENU)),
        icon: None,
        shortcut: None,
        params: define_params!(
            Param::optional("name", ParamKind::Str, "Name in the job outline (default: WCS <n>)"),
            Param::optional("activate", ParamKind::Bool, "Make it the active WCS (default true)"),
        ),
        returns: WCS_RETURNS,
        enabled: always,
        run: create,
    },
    CommandSpec {
        id: "setup.wcs.top_center",
        label: "Origin at top centre",
        tooltip: "Move the WCS origin to the top centre of the part (creates a WCS if there is none)",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[ID_PARAM],
        returns: WCS_RETURNS,
        enabled: always,
        run: |s, p| place(s, p, |b| Vec3::new(b.center().x, b.center().y, b.max.z)),
    },
    CommandSpec {
        id: "setup.wcs.bottom_corner",
        label: "Origin at bottom corner",
        tooltip: "Move the WCS origin to a bottom corner of the part (creates a WCS if there is none)",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[ID_PARAM, Param::optional("corner", ParamKind::Choice(CORNERS), "Which corner (default front_left)")],
        returns: WCS_RETURNS,
        enabled: always,
        run: |s, p| {
            let corner = params::opt_str(p, "corner")?.unwrap_or("front_left");
            let (right, back) = match corner {
                "front_left" => (false, false),
                "front_right" => (true, false),
                "back_left" => (false, true),
                "back_right" => (true, true),
                other => return Err(EngineError::InvalidParams(format!("unknown corner `{other}`"))),
            };
            place(s, p, |b| {
                Vec3::new(if right { b.max.x } else { b.min.x }, if back { b.max.y } else { b.min.y }, b.min.z)
            })
        },
    },
    CommandSpec {
        id: "setup.wcs.align",
        label: "Align",
        tooltip: "Re-define an existing WCS: a new orientation, origin, or both (groups left out are kept)",
        placement: Some(Placement::small(RibbonTab::Setup, "Coordinates")),
        icon: None,
        shortcut: None,
        params: define_params!(ID_PARAM),
        returns: WCS_RETURNS,
        enabled: always,
        run: |s, p| {
            let id = match params::opt_str(p, "id")? {
                Some(id) => id.to_owned(),
                None => s
                    .job
                    .active_wcs
                    .clone()
                    .ok_or_else(|| EngineError::InvalidParams("there is no WCS yet; create one first".into()))?,
            };
            let current = s.job.wcs_by_id(&id).cloned().ok_or_else(|| missing(&id))?;
            let origin = finite_origin(resolve_origin(s, p, Some(current.origin))?)?;
            let (x_axis, z_axis) = resolve_axes(s, p, origin, Some(&current))?;
            edit(s, p, |w| {
                w.origin = origin;
                w.x_axis = x_axis;
                w.z_axis = z_axis;
                Ok(())
            })
        },
    },
    CommandSpec {
        id: "setup.wcs.offset",
        label: "Offset",
        tooltip: "Move the WCS origin by a distance",
        placement: Some(Placement::small(RibbonTab::Setup, "Coordinates")),
        icon: None,
        shortcut: None,
        params: &[
            ID_PARAM,
            Param::required("delta", ParamKind::Vec3, "Distance to move, in mm"),
            Param::optional(
                "frame",
                ParamKind::Choice(FRAMES),
                "Axes `delta` is measured along: the WCS's own (default) or the world's",
            ),
        ],
        returns: WCS_RETURNS,
        enabled: always,
        run: |s, p| {
            let delta = params::opt_vec3(p, "delta")?
                .ok_or_else(|| EngineError::InvalidParams("`delta` is required".into()))?;
            let world = params::opt_str(p, "frame")? == Some("world");
            edit(s, p, |w| {
                let moved = if world { w.origin + delta } else { w.to_world(delta) };
                w.origin = finite_origin(moved)?;
                Ok(())
            })
        },
    },
    CommandSpec {
        id: "setup.wcs.rotate",
        label: "Rotate WCS",
        tooltip: "Turn the WCS about one of its own axes",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            ID_PARAM,
            Param::required("axis", ParamKind::Choice(AXES), "The WCS axis to turn about"),
            Param::required("degrees", DEGREES, "Angle, counter-clockwise looking down the axis"),
        ],
        returns: WCS_RETURNS,
        enabled: always,
        run: |s, p| {
            let degrees = params::opt_f64(p, "degrees", -36_000.0, 36_000.0)?.unwrap_or(0.0) as f32;
            let axis = params::req_str(p, "axis")?.to_owned();
            edit(s, p, |w| {
                let about = match axis.as_str() {
                    "x" => w.x_axis,
                    "y" => w.y_axis(),
                    _ => w.z_axis,
                };
                let q = Quat::from_axis_angle(about, degrees.to_radians());
                let (x_axis, z_axis) = axes(q * w.z_axis, Some(q * w.x_axis))?;
                w.x_axis = x_axis;
                w.z_axis = z_axis;
                Ok(())
            })
        },
    },
    CommandSpec {
        id: "setup.wcs.rename",
        label: "Rename WCS",
        tooltip: "Give a WCS a new name",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[ID_PARAM, Param::required("name", ParamKind::Str, "New name")],
        returns: WCS_RETURNS,
        enabled: always,
        run: |s, p| {
            let name = clean_name(params::req_str(p, "name")?)?;
            edit(s, p, |w| {
                w.name = name;
                Ok(())
            })
        },
    },
    CommandSpec {
        id: "setup.wcs.activate",
        label: "Activate WCS",
        tooltip: "Program toolpaths in this WCS",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "WCS id, e.g. wcs.1")],
        returns: WCS_RETURNS,
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            let w = s.job.wcs_by_id(id).ok_or_else(|| missing(id))?.to_json(true);
            s.job.active_wcs = Some(id.to_owned());
            s.job.setup_changed();
            Ok(w)
        },
    },
    CommandSpec {
        id: "setup.wcs.remove",
        label: "Remove WCS",
        tooltip: "Delete a work coordinate system",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "WCS id, e.g. wcs.1")],
        returns: "{\"removed\": id, \"active_wcs\": id or null}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            let before = s.job.wcs.len();
            s.job.wcs.retain(|w| w.id != id);
            if s.job.wcs.len() == before {
                return Err(missing(id));
            }
            if s.job.active_wcs.as_deref() == Some(id) {
                s.job.active_wcs = s.job.wcs.first().map(|w| w.id.clone());
            }
            // Setups programmed in it fall back to the origin.
            for st in s.job.setups.iter_mut().filter(|st| st.wcs.as_deref() == Some(id)) {
                st.wcs = None;
            }
            if s.view.selection.as_deref() == Some(id) {
                s.view.selection = None;
            }
            s.job.setup_changed();
            Ok(json!({ "removed": id, "active_wcs": s.job.active_wcs }))
        },
    },
    CommandSpec {
        id: "setup.wcs.list",
        label: "Coordinate systems",
        tooltip: "The job's work coordinate systems",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "list of {id, name, active, origin, x_axis, y_axis, z_axis}",
        enabled: always,
        run: |s, _| {
            let active = s.job.active_wcs.as_deref();
            Ok(json!(s.job.wcs.iter().map(|w| w.to_json(active == Some(&w.id))).collect::<Vec<_>>()))
        },
    },
    CommandSpec {
        id: "setup.wcs.get",
        label: "Get WCS",
        tooltip: "One work coordinate system",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[ID_PARAM],
        returns: "the WCS {id, name, active, origin, x_axis, y_axis, z_axis}, or null when there is none",
        enabled: always,
        run: |s, p| {
            let w = match params::opt_str(p, "id")? {
                Some(id) => Some(s.job.wcs_by_id(id).ok_or_else(|| missing(id))?),
                None => s.job.active_wcs(),
            };
            Ok(w.map_or(Value::Null, |w| w.to_json(s.job.active_wcs.as_deref() == Some(&w.id))))
        },
    },
];

fn missing(id: &str) -> EngineError {
    EngineError::InvalidParams(format!("no WCS `{id}`"))
}

fn clean_name(name: &str) -> EngineResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(EngineError::InvalidParams("`name` must not be empty".into()));
    }
    Ok(name.chars().take(MAX_NAME_CHARS).collect())
}

fn finite_origin(p: Vec3) -> EngineResult<Vec3> {
    if p.is_finite() && p.abs().max_element() <= 1.0e9 {
        Ok(p)
    } else {
        Err(EngineError::InvalidParams("the origin would be out of range".into()))
    }
}

/// Unit, perpendicular X and Z axes from a Z direction and an optional X hint. Without a
/// usable hint, X follows world X (or world Y when Z points along world X).
pub(crate) fn axes(z: Vec3, x: Option<Vec3>) -> EngineResult<(Vec3, Vec3)> {
    let bad = |what: &str| EngineError::InvalidParams(what.into());
    if !(z.is_finite() && z.length() > MIN_AXIS_LENGTH) {
        return Err(bad("`z_axis` must have a direction"));
    }
    let z = z.normalize();
    let square = |v: Vec3| {
        let v = v - z * v.dot(z);
        (v.length() > MIN_AXIS_LENGTH * 10.0).then(|| v.normalize())
    };
    let x_axis = match x {
        Some(hint) => {
            if !(hint.is_finite() && hint.length() > MIN_AXIS_LENGTH) {
                return Err(bad("`x_axis` must have a direction"));
            }
            square(hint.normalize()).ok_or_else(|| bad("`x_axis` must not be parallel to the Z axis"))?
        }
        None => square(Vec3::X).or_else(|| square(Vec3::Y)).ok_or_else(|| bad("`z_axis` must have a direction"))?,
    };
    Ok((x_axis, z))
}

fn part_bounds(s: &Session) -> EngineResult<Aabb> {
    let b = s.job.part_bounds();
    if b.is_empty() {
        return Err(EngineError::InvalidParams("there is no part to place the origin on; import a model first".into()));
    }
    Ok(b)
}

fn create(s: &mut Session, p: &Value) -> EngineResult<Value> {
    if s.job.wcs.len() >= MAX_WCS {
        return Err(EngineError::InvalidParams(format!("a job holds at most {MAX_WCS} coordinate systems")));
    }
    let origin = resolve_origin(s, p, None)?;
    let (x_axis, z_axis) = resolve_axes(s, p, origin, None)?;
    add(s, p, origin, x_axis, z_axis)
}

/// Adds a WCS with the given frame, taking `name` and `activate` from `p`.
fn add(s: &mut Session, p: &Value, origin: Vec3, x_axis: Vec3, z_axis: Vec3) -> EngineResult<Value> {
    if s.job.wcs.len() >= MAX_WCS {
        return Err(EngineError::InvalidParams(format!("a job holds at most {MAX_WCS} coordinate systems")));
    }
    let origin = finite_origin(origin)?;
    let n = s.job.next_wcs;
    let name = match params::opt_str(p, "name")? {
        Some(name) => clean_name(name)?,
        None => format!("WCS {n}"),
    };
    let w = Wcs { id: format!("wcs.{n}"), name, origin, x_axis, z_axis };
    let activate = params::opt_bool(p, "activate")?.unwrap_or(true) || s.job.active_wcs.is_none();
    s.job.next_wcs = n.saturating_add(1);
    if activate {
        s.job.active_wcs = Some(w.id.clone());
    }
    let result = w.to_json(activate);
    s.job.wcs.push(w);
    s.job.setup_changed();
    Ok(result)
}

/// Changes the WCS named by `id` (default: the active one) and returns it.
fn edit(s: &mut Session, p: &Value, change: impl FnOnce(&mut Wcs) -> EngineResult<()>) -> EngineResult<Value> {
    let id = match params::opt_str(p, "id")? {
        Some(id) => id.to_owned(),
        None => s
            .job
            .active_wcs
            .clone()
            .ok_or_else(|| EngineError::InvalidParams("there is no WCS yet; create one first".into()))?,
    };
    let active = s.job.active_wcs.as_deref() == Some(id.as_str());
    let w = s.job.wcs.iter_mut().find(|w| w.id == id).ok_or_else(|| missing(&id))?;
    change(w)?;
    let result = w.to_json(active);
    s.job.setup_changed();
    Ok(result)
}

/// Moves a WCS origin to a point on the part, creating a WCS when the job has none.
fn place(s: &mut Session, p: &Value, at: impl FnOnce(Aabb) -> Vec3) -> EngineResult<Value> {
    let origin = at(part_bounds(s)?);
    if params::opt_str(p, "id")?.is_none() && s.job.active_wcs.is_none() {
        return create(s, &json!({ "origin": "point", "origin_point": origin.to_array() }));
    }
    edit(s, p, |w| {
        w.origin = origin;
        Ok(())
    })
}
