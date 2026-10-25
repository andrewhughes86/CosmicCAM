//! How a WCS is defined: an **orientation** group and an **origin** group, each a mode plus
//! the selections that mode uses. `setup.wcs.create` and `setup.wcs.align` share them.
//!
//! Selections name model geometry: faces (`<model id>/face.<n>`) and edges
//! (`<model id>/edge.<n>`), as `model.faces.list`, `model.edges.list` and the pick commands
//! report them. Parameters that the chosen mode does not use are refused, so a script never
//! silently gets something other than it asked for.

use CosmicCAM_geom::{EdgeKind, FaceKind, Vec3};
use serde_json::Value;

use super::wcs::axes;
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::session::{Session, Wcs};

/// Orientation modes: model axes, two selected axes (the third follows), or vectors.
pub const ORIENTATIONS: &[&str] = &["model", "z_x", "z_y", "x_y", "vectors"];
/// Origin modes.
pub const ORIGINS: &[&str] = &["model", "box", "stock", "point", "intersection"];
/// Points on the model's bounding box: a level, then a position seen from the front (−Y).
pub const BOX_POINTS: &[&str] = &[
    "top_front_left",
    "top_front",
    "top_front_right",
    "top_left",
    "top_center",
    "top_right",
    "top_back_left",
    "top_back",
    "top_back_right",
    "middle_front_left",
    "middle_front",
    "middle_front_right",
    "middle_left",
    "center",
    "middle_right",
    "middle_back_left",
    "middle_back",
    "middle_back_right",
    "bottom_front_left",
    "bottom_front",
    "bottom_front_right",
    "bottom_left",
    "bottom_center",
    "bottom_right",
    "bottom_back_left",
    "bottom_back",
    "bottom_back_right",
];

/// The orientation and origin groups, in the order the API guide lists them.
macro_rules! define_params {
    ($($first:expr),* $(,)?) => {
        &[
            $($first,)*
            $crate::cmd::Param::optional(
                "orientation",
                $crate::cmd::ParamKind::Choice($crate::cmd::wcs_define::ORIENTATIONS),
                "How the axes are set: model (world axes), z_x, z_y or x_y (two selected axes; the third follows), \
                 vectors",
            ),
            $crate::cmd::Param::optional("orientation_z", $crate::cmd::ParamKind::Str, "Z axis selection: a plane (its normal), cylinder (its axis) or edge"),
            $crate::cmd::Param::optional("orientation_x", $crate::cmd::ParamKind::Str, "X axis selection: a plane, cylinder or edge"),
            $crate::cmd::Param::optional("orientation_y", $crate::cmd::ParamKind::Str, "Y axis selection: a plane, cylinder or edge"),
            $crate::cmd::Param::optional("orientation_z_vector", $crate::cmd::ParamKind::Vec3, "Z direction, for orientation = vectors"),
            $crate::cmd::Param::optional("orientation_x_vector", $crate::cmd::ParamKind::Vec3, "X direction, for orientation = vectors (squared to Z)"),
            $crate::cmd::Param::optional("flip_z", $crate::cmd::ParamKind::Bool, "Reverse the Z axis"),
            $crate::cmd::Param::optional("flip_x", $crate::cmd::ParamKind::Bool, "Reverse the X axis"),
            $crate::cmd::Param::optional("flip_y", $crate::cmd::ParamKind::Bool, "Reverse the Y axis (orientations z_y and x_y)"),
            $crate::cmd::Param::optional(
                "origin",
                $crate::cmd::ParamKind::Choice($crate::cmd::wcs_define::ORIGINS),
                "Where the origin goes: model (world zero), box (a point on the model's box), stock (a point on the stock box), \
                 point (a selected point) or intersection (where selected geometry meets)",
            ),
            $crate::cmd::Param::optional("origin_box", $crate::cmd::ParamKind::Choice($crate::cmd::wcs_define::BOX_POINTS), "Box point, for origin = box or stock (default top_center)"),
            $crate::cmd::Param::optional("origin_point", $crate::cmd::ParamKind::Vec3, "Coordinates, for origin = point"),
            $crate::cmd::Param::optional(
                "origin_geometry",
                $crate::cmd::ParamKind::Str,
                "Selection, for origin = point: a circular edge or cylinder (centre / upper axis end), plane (centre) or \
                 straight edge (midpoint)",
            ),
            $crate::cmd::Param::optional("origin_a", $crate::cmd::ParamKind::Str, "First selection, for origin = intersection: a plane, cylinder or edge"),
            $crate::cmd::Param::optional("origin_b", $crate::cmd::ParamKind::Str, "Second selection, for origin = intersection"),
            $crate::cmd::Param::optional("origin_c", $crate::cmd::ParamKind::Str, "Third selection, for origin = intersection (three planes)"),
        ]
    };
}
pub(crate) use define_params;

fn bad(message: impl Into<String>) -> EngineError {
    EngineError::InvalidParams(message.into())
}

/// Selected geometry, reduced to what defining a WCS needs.
#[derive(Clone, Copy, Debug)]
enum Geom {
    Plane {
        normal: Vec3,
        point: Vec3,
        centre: Vec3,
    },
    Cylinder {
        base: Vec3,
        axis: Vec3,
        length: f32,
    },
    Line {
        start: Vec3,
        end: Vec3,
    },
    Circle {
        centre: Vec3,
        normal: Vec3,
    },
    /// A vertex: a position only.
    Point(Vec3),
}

/// The geometry a selection names.
fn geom(s: &Session, key: &str, reference: &str) -> EngineResult<Geom> {
    if let Some((m, f)) = s.job.face(reference) {
        let face = m.faces.faces.get(f).ok_or_else(|| bad(format!("`{key}`: no face `{reference}`")))?;
        return match face.kind {
            FaceKind::Plane { normal, point } => Ok(Geom::Plane { normal, point, centre: face.centroid }),
            FaceKind::Cylinder { base, axis, length, .. } => Ok(Geom::Cylinder { base, axis, length }),
            FaceKind::Freeform => Err(bad(format!("`{key}`: {reference} is freeform; pick a plane, cylinder or edge"))),
        };
    }
    if let Some((m, e)) = s.job.edge(reference) {
        return match m.edges.edges.get(e).map(|e| e.kind) {
            Some(EdgeKind::Line { start, end }) => Ok(Geom::Line { start, end }),
            Some(EdgeKind::Circle { center, normal, .. }) => Ok(Geom::Circle { centre: center, normal }),
            None => Err(bad(format!("`{key}`: no edge `{reference}`"))),
        };
    }
    if let Some((m, crate::session::Entity::Vertex(v))) = s.job.entity(reference) {
        return m
            .edges
            .vertices
            .get(v)
            .map(|p| Geom::Point(*p))
            .ok_or_else(|| bad(format!("`{key}`: no vertex `{reference}`")));
    }
    Err(bad(format!(
        "`{key}`: no face, edge or vertex `{reference}` (expected model.N/face.N, model.N/edge.N or model.N/vertex.N)"
    )))
}

fn selection(s: &Session, p: &Value, key: &str) -> EngineResult<Option<Geom>> {
    params::opt_str(p, key)?.map(|r| geom(s, key, r)).transpose()
}

fn required(s: &Session, p: &Value, key: &str, why: &str) -> EngineResult<Geom> {
    selection(s, p, key)?.ok_or_else(|| bad(format!("`{key}` is required {why}")))
}

/// `v` or `-v`, pointing up: towards world +Z, else +X, else +Y.
fn upward(v: Vec3) -> Vec3 {
    let key = [v.z, v.x, v.y].into_iter().find(|c| c.abs() > 1.0e-4).unwrap_or(1.0);
    if key < 0.0 { -v } else { v }
}

/// `v` or `-v`, agreeing with the first of `prefs` it is not square to.
fn canonical(v: Vec3, prefs: [Vec3; 3]) -> Vec3 {
    let key = prefs.into_iter().map(|w| v.dot(w)).find(|d| d.abs() > 1.0e-4).unwrap_or(1.0);
    if key < 0.0 { -v } else { v }
}

/// Which axis a selection sets, for choosing the sense of directionless geometry.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Z,
    X,
    Y,
}

/// The direction a selection gives an axis. Planes give their outward normal; cylinders and
/// circles their axis, pointing up. A straight edge for X or Y runs away from the origin when
/// the origin sits on its line (a corner), otherwise along the world axis of the same name;
/// for Z it points up. Flips reverse any of these.
fn direction(g: Geom, origin: Vec3, role: Role) -> Vec3 {
    match g {
        Geom::Plane { normal, .. } => normal,
        Geom::Cylinder { axis, .. } => upward(axis),
        Geom::Circle { normal, .. } => upward(normal),
        Geom::Point(_) => Vec3::ZERO,
        Geom::Line { start, end } => {
            let d = (end - start).normalize_or_zero();
            let rel = origin - start;
            let off_line = (rel - d * rel.dot(d)).length();
            let on_line = off_line <= start.distance(end) * 0.01 + 1.0e-3;
            match role {
                Role::Z => upward(d),
                _ if on_line => {
                    if origin.distance(start) > origin.distance(end) {
                        -d
                    } else {
                        d
                    }
                }
                Role::X => canonical(d, [Vec3::X, Vec3::Y, Vec3::Z]),
                Role::Y => canonical(d, [Vec3::Y, Vec3::NEG_X, Vec3::Z]),
            }
        }
    }
    .normalize_or_zero()
}

/// The direction an X or Y selection gives once Z is known. A hole, boss or circle whose axis
/// runs along Z cannot point across it, so the axis points from the origin towards it
/// instead (X towards a second dowel hole).
fn direction_across(g: Geom, origin: Vec3, z: Vec3, role: Role) -> Vec3 {
    let towards = |centre: Vec3, axis: Vec3| {
        (axis.dot(z).abs() > 0.95).then(|| {
            let v = centre - origin;
            (v - z * v.dot(z)).normalize_or_zero()
        })
    };
    let across = match g {
        Geom::Cylinder { base, axis, .. } => towards(base, axis),
        Geom::Circle { centre, normal } => towards(centre, normal),
        _ => None,
    };
    across.filter(|v| *v != Vec3::ZERO).unwrap_or_else(|| direction(g, origin, role))
}

/// Refuses parameters the chosen mode does not use.
fn unused(p: &Value, keys: &[&str], mode_param: &str, mode: &str) -> EngineResult<()> {
    for key in keys {
        if p.get(key).is_some_and(|v| !v.is_null()) {
            return Err(bad(format!("`{key}` is not used with {mode_param} = \"{mode}\"")));
        }
    }
    Ok(())
}

const ORIENTATION_KEYS: &[&str] =
    &["orientation_z", "orientation_x", "orientation_y", "orientation_z_vector", "orientation_x_vector"];
const ORIGIN_KEYS: &[&str] = &["origin_box", "origin_point", "origin_geometry", "origin_a", "origin_b", "origin_c"];

/// The origin the parameters ask for; `current` is kept when `origin` is not given (align).
pub(crate) fn resolve_origin(s: &Session, p: &Value, current: Option<Vec3>) -> EngineResult<Vec3> {
    let mode = match params::opt_str(p, "origin")? {
        Some(m) => m,
        None => match current {
            Some(o) => {
                unused(p, ORIGIN_KEYS, "origin", "(not given)")?;
                return Ok(o);
            }
            None => "model",
        },
    };
    let uses =
        |keep: &[&str]| -> Vec<&'static str> { ORIGIN_KEYS.iter().copied().filter(|k| !keep.contains(k)).collect() };
    match mode {
        "model" => {
            unused(p, &uses(&[]), "origin", mode)?;
            Ok(Vec3::ZERO)
        }
        "box" => {
            unused(p, &uses(&["origin_box"]), "origin", mode)?;
            let b = s.job.part_bounds();
            if b.is_empty() {
                return Err(bad("there is no model to take a box point from; import a model first"));
            }
            box_point(b, params::opt_str(p, "origin_box")?.unwrap_or("top_center"))
        }
        "stock" => {
            unused(p, &uses(&["origin_box"]), "origin", mode)?;
            let b = s.job.stock.ok_or_else(|| bad("there is no stock yet; fit one with setup.stock.fit"))?;
            box_point(b, params::opt_str(p, "origin_box")?.unwrap_or("top_center"))
        }
        "point" => {
            unused(p, &uses(&["origin_point", "origin_geometry"]), "origin", mode)?;
            match (params::opt_vec3(p, "origin_point")?, selection(s, p, "origin_geometry")?) {
                (Some(_), Some(_)) => Err(bad("give `origin_point` or `origin_geometry`, not both")),
                (Some(point), None) => Ok(point),
                (None, Some(g)) => Ok(match g {
                    Geom::Circle { centre, .. } | Geom::Plane { centre, .. } => centre,
                    Geom::Cylinder { base, axis, length } => {
                        if upward(axis) == axis {
                            base + axis * length
                        } else {
                            base
                        }
                    }
                    Geom::Line { start, end } => (start + end) * 0.5,
                    Geom::Point(p) => p,
                }),
                (None, None) => Err(bad("origin = \"point\" needs `origin_point` or `origin_geometry`")),
            }
        }
        "intersection" => {
            unused(p, &uses(&["origin_a", "origin_b", "origin_c"]), "origin", mode)?;
            let a = required(s, p, "origin_a", "for origin = \"intersection\"")?;
            let b = required(s, p, "origin_b", "for origin = \"intersection\"")?;
            intersection(a, b, selection(s, p, "origin_c")?)
        }
        other => Err(bad(format!("unknown origin `{other}`"))),
    }
}

fn box_point(b: CosmicCAM_geom::Aabb, name: &str) -> EngineResult<Vec3> {
    let (lo, hi, c) = (b.min, b.max, b.center());
    let (level, pos) = match name {
        "center" => ("middle", "center"),
        _ => name.split_once('_').ok_or_else(|| bad(format!("unknown box point `{name}`")))?,
    };
    let z = match level {
        "top" => hi.z,
        "middle" => c.z,
        "bottom" => lo.z,
        _ => return Err(bad(format!("unknown box point `{name}`"))),
    };
    let (x, y) = match pos {
        "front_left" => (lo.x, lo.y),
        "front" => (c.x, lo.y),
        "front_right" => (hi.x, lo.y),
        "left" => (lo.x, c.y),
        "center" => (c.x, c.y),
        "right" => (hi.x, c.y),
        "back_left" => (lo.x, hi.y),
        "back" => (c.x, hi.y),
        "back_right" => (hi.x, hi.y),
        _ => return Err(bad(format!("unknown box point `{name}`"))),
    };
    Ok(Vec3::new(x, y, z))
}

/// A plane or an infinite line, for intersections.
enum Prim {
    Plane(Vec3, Vec3),
    Line(Vec3, Vec3),
}

fn prim(g: Geom) -> Option<Prim> {
    Some(match g {
        Geom::Plane { normal, point, .. } => Prim::Plane(normal, point),
        Geom::Cylinder { base, axis, .. } => Prim::Line(base, axis),
        Geom::Circle { centre, normal } => Prim::Line(centre, normal),
        Geom::Line { start, end } => Prim::Line(start, (end - start).normalize_or_zero()),
        Geom::Point(_) => return None,
    })
}

/// Where the selections meet: three planes, a line (edge or axis) and a plane, or two lines.
fn intersection(a: Geom, b: Geom, c: Option<Geom>) -> EngineResult<Vec3> {
    let none = || bad("these selections do not meet in one point");
    let (Some(a), Some(b)) = (prim(a), prim(b)) else {
        return Err(bad("a vertex cannot take part in an intersection; use origin = \"point\" for it"));
    };
    let c = match c.map(prim) {
        Some(None) => return Err(bad("a vertex cannot take part in an intersection; use origin = \"point\" for it")),
        Some(Some(c)) => Some(c),
        None => None,
    };
    match (a, b, c) {
        (Prim::Plane(n1, p1), Prim::Plane(n2, p2), Some(Prim::Plane(n3, p3))) => {
            let det = n1.dot(n2.cross(n3));
            if det.abs() < 0.25 {
                return Err(bad("pick three planes square to different axes: these do not meet in a clear corner"));
            }
            let (d1, d2, d3) = (n1.dot(p1), n2.dot(p2), n3.dot(p3));
            Ok((n2.cross(n3) * d1 + n3.cross(n1) * d2 + n1.cross(n2) * d3) / det)
        }
        (Prim::Plane(..), Prim::Plane(..), None) => {
            Err(bad("two planes meet in a line; add a third plane (`origin_c`) or use a line and a plane"))
        }
        (Prim::Line(q, d), Prim::Plane(n, p), None) | (Prim::Plane(n, p), Prim::Line(q, d), None) => {
            let along = d.dot(n);
            if along.abs() < 0.05 {
                return Err(bad("the line runs along the plane; pick a plane it passes through"));
            }
            Ok(q + d * ((p - q).dot(n) / along))
        }
        (Prim::Line(q1, d1), Prim::Line(q2, d2), None) => {
            let (b, w) = (d1.dot(d2), q1 - q2);
            let denom = 1.0 - b * b;
            if denom.abs() < 1.0e-4 {
                return Err(bad("the two lines are parallel"));
            }
            let s = (b * d2.dot(w) - d1.dot(w)) / denom;
            let t = (d2.dot(w) - b * d1.dot(w)) / denom;
            Ok(((q1 + d1 * s) + (q2 + d2 * t)) * 0.5)
        }
        _ => Err(none()),
    }
}

/// The axes the parameters ask for: (x, z), both unit and square. `current` is kept when
/// `orientation` is not given (align).
pub(crate) fn resolve_axes(s: &Session, p: &Value, origin: Vec3, current: Option<&Wcs>) -> EngineResult<(Vec3, Vec3)> {
    let flip = |key: &str| params::opt_bool(p, key).map(|b| b.unwrap_or(false));
    let mode = match params::opt_str(p, "orientation")? {
        Some(m) => m,
        None => match current {
            Some(w) => {
                unused(p, ORIENTATION_KEYS, "orientation", "(not given)")?;
                let (mut x, mut z) = (w.x_axis, w.z_axis);
                if flip("flip_z")? {
                    z = -z;
                }
                if flip("flip_x")? {
                    x = -x;
                }
                unused(p, &["flip_y"], "orientation", "(not given)")?;
                return Ok((x, z));
            }
            None => "model",
        },
    };
    let uses = |keep: &[&str]| -> Vec<&'static str> {
        ORIENTATION_KEYS.iter().copied().filter(|k| !keep.contains(k)).collect()
    };
    let need = |key: &str| match required(s, p, key, &format!("for orientation = \"{mode}\""))? {
        Geom::Point(_) => Err(bad(format!("`{key}`: a vertex has no direction; pick a plane, cylinder or edge"))),
        g => Ok(g),
    };
    let sign = |v: Vec3, key: &str| -> EngineResult<Vec3> { Ok(if flip(key)? { -v } else { v }) };
    let (x, z) = match mode {
        "model" => {
            unused(p, &uses(&[]), "orientation", mode)?;
            unused(p, &["flip_y"], "orientation", mode)?;
            (sign(Vec3::X, "flip_x")?, sign(Vec3::Z, "flip_z")?)
        }
        "vectors" => {
            unused(p, &uses(&["orientation_z_vector", "orientation_x_vector"]), "orientation", mode)?;
            unused(p, &["flip_y"], "orientation", mode)?;
            let z = params::opt_vec3(p, "orientation_z_vector")?.unwrap_or(Vec3::Z);
            let (x, z) = axes(z, params::opt_vec3(p, "orientation_x_vector")?)?;
            (sign(x, "flip_x")?, sign(z, "flip_z")?)
        }
        "z_x" => {
            unused(p, &uses(&["orientation_z", "orientation_x"]), "orientation", mode)?;
            unused(p, &["flip_y"], "orientation", mode)?;
            let z = sign(direction(need("orientation_z")?, origin, Role::Z), "flip_z")?;
            let x = sign(direction_across(need("orientation_x")?, origin, z, Role::X), "flip_x")?;
            (x, z)
        }
        "z_y" => {
            unused(p, &uses(&["orientation_z", "orientation_y"]), "orientation", mode)?;
            unused(p, &["flip_x"], "orientation", mode)?;
            let z = sign(direction(need("orientation_z")?, origin, Role::Z), "flip_z")?;
            let y = sign(direction_across(need("orientation_y")?, origin, z, Role::Y), "flip_y")?;
            (y.cross(z), z)
        }
        "x_y" => {
            unused(p, &uses(&["orientation_x", "orientation_y"]), "orientation", mode)?;
            unused(p, &["flip_z"], "orientation", mode)?;
            let x = sign(direction(need("orientation_x")?, origin, Role::X), "flip_x")?;
            let y = sign(direction(need("orientation_y")?, origin, Role::Y), "flip_y")?;
            (x, x.cross(y))
        }
        other => return Err(bad(format!("unknown orientation `{other}`"))),
    };
    if z.length() < 0.1 || x.cross(z).length() < 0.1 {
        return Err(bad("the two selected axes are parallel; pick geometry along different directions"));
    }
    axes(z, Some(x))
}
