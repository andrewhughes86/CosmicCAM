//! CAD mode's part: a history of features (sketches, extrusions, revolutions) rebuilt in order
//! into a solid body by the modelling kernel. The body's triangles are the job model "Part", so
//! CAM machines it like any imported model.
//!
//! A feature keeps the settings it was made with (as its command took them) and is worked out
//! again from them on every rebuild; the body after each feature is cached, so changing a late
//! feature does not redo the early ones. Settings are input like any other: every count is
//! capped and every number checked.

use std::sync::Arc;

use CosmicDMS_geom::{Mesh, Vec3};
use CosmicDMS_kernel::{Body, Operation, Plane, Segment};
use glam::{DVec2, DVec3};
use serde_json::{Value, json};

use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::session::{JobState, Model};

/// Most features one part holds.
pub const MAX_FEATURES: usize = 1_000;
/// Most entities one sketch holds.
pub const MAX_ENTITIES: usize = 10_000;
/// The sketch planes through the origin.
pub const PLANES: &[&str] = &["top", "front", "right", "custom"];
pub const DIRECTIONS: &[&str] = &["normal", "reverse", "symmetric"];
pub const OPERATIONS: &[&str] = &["new", "join", "cut"];
pub const AXES: &[&str] = &["x", "y", "line"];
/// How far a through-all cut reaches each way when there is no part to measure, mm.
const THROUGH: f64 = 10_000.0;
/// How closely the part's triangles follow its surfaces, mm.
pub const TOLERANCE: f64 = 0.01;
/// Largest coordinate accepted in a sketch, mm.
const MAX_COORD: f64 = 1.0e6;

fn bad(m: impl Into<String>) -> EngineError {
    EngineError::InvalidParams(m.into())
}

/// One step of the part's history.
#[derive(Clone, Debug)]
pub struct Feature {
    /// `feature.<n>`.
    pub id: String,
    pub name: String,
    /// `sketch`, `extrude` or `revolve`.
    pub kind: String,
    /// The settings it was made with.
    pub params: Value,
    pub suppressed: bool,
}

impl Feature {
    pub fn to_json(&self) -> Value {
        json!({ "id": self.id, "name": self.name, "kind": self.kind, "params": self.params, "suppressed": self.suppressed })
    }
}

/// The part being modelled.
#[derive(Clone, Debug, Default)]
pub struct Part {
    pub features: Vec<Feature>,
    pub next: u64,
    /// The job model holding the body.
    pub model: Option<String>,
    /// The body after each feature, with a hash of the history up to it.
    cache: Vec<(u64, Body)>,
    /// The finished body's mesh and the faces and edges found on it, for the history hash it
    /// was made from.
    meshed: Option<(u64, Meshed)>,
}

/// A body's mesh with its recognised faces and edges.
type Meshed = (Arc<Mesh>, Arc<CosmicDMS_geom::faces::FaceMap>, Arc<CosmicDMS_geom::edges::EdgeMap>);

/// A sketch worked out: its plane, its profile and its lines (for revolve axes).
#[derive(Clone)]
pub struct Sketch {
    pub plane: Plane,
    pub segments: Vec<Segment>,
    /// Each entity's segments, by entity index.
    pub entities: Vec<Vec<Segment>>,
}

fn point2(v: &Value, what: &str) -> EngineResult<DVec2> {
    let a = v.as_array().ok_or_else(|| bad(format!("`{what}` must be [x, y]")))?;
    let n = |i: usize| a.get(i).and_then(Value::as_f64).filter(|x| x.is_finite() && x.abs() <= MAX_COORD);
    match (n(0), n(1), a.len()) {
        (Some(x), Some(y), 2 | 3) => Ok(DVec2::new(x, y)),
        _ => Err(bad(format!("`{what}` must be [x, y] with finite numbers"))),
    }
}

fn vec3(p: &Value, key: &str) -> EngineResult<Option<DVec3>> {
    Ok(params::opt_vec3(p, key)?.map(|v| v.as_dvec3()))
}

/// The plane a sketch's settings name.
pub fn plane_of(p: &Value) -> EngineResult<Plane> {
    let offset = params::opt_f64(p, "offset", -MAX_COORD, MAX_COORD)?.unwrap_or(0.0);
    let (origin, x, y) = match params::opt_str(p, "plane")?.unwrap_or("top") {
        "top" => (DVec3::ZERO, DVec3::X, DVec3::Y),
        "front" => (DVec3::ZERO, DVec3::X, DVec3::Z),
        "right" => (DVec3::ZERO, DVec3::Y, DVec3::Z),
        "custom" => {
            let origin = vec3(p, "origin")?.unwrap_or(DVec3::ZERO);
            let n = vec3(p, "normal")?
                .and_then(|n| n.try_normalize())
                .ok_or_else(|| bad("a custom plane needs its `normal`"))?;
            let x = vec3(p, "x_axis")?.unwrap_or_else(|| n.any_orthonormal_vector());
            let x = (x - n * x.dot(n)).try_normalize().ok_or_else(|| bad("`x_axis` must not be along the normal"))?;
            (origin, x, n.cross(x))
        }
        other => return Err(bad(format!("unknown sketch plane `{other}`"))),
    };
    let plane = Plane::new(origin, x, y).ok_or_else(|| bad("the sketch plane has no direction"))?;
    Ok(Plane { origin: origin + plane.normal() * offset, ..plane })
}

/// One sketch entity's segments (plane coordinates).
pub fn entity(e: &Value) -> EngineResult<Vec<Segment>> {
    let kind = e.get("type").and_then(Value::as_str).ok_or_else(|| bad("each sketch entity needs a `type`"))?;
    let get = |k: &str| e.get(k).ok_or_else(|| bad(format!("a {kind} needs `{k}`")));
    let num = |k: &str, lo: f64, hi: f64| -> EngineResult<f64> {
        get(k)?
            .as_f64()
            .filter(|v| v.is_finite() && (lo..=hi).contains(v))
            .ok_or_else(|| bad(format!("a {kind}'s `{k}` must be between {lo} and {hi}")))
    };
    let line = |a: DVec2, b: DVec2| Segment::Line { a, b };
    Ok(match kind {
        "line" => {
            let (a, b) = (point2(get("from")?, "from")?, point2(get("to")?, "to")?);
            if a.distance(b) < 1.0e-6 {
                return Err(bad("a line needs two different ends"));
            }
            vec![line(a, b)]
        }
        "rectangle" => {
            let (a, c) = (point2(get("corner")?, "corner")?, point2(get("opposite")?, "opposite")?);
            if (a.x - c.x).abs() < 1.0e-6 || (a.y - c.y).abs() < 1.0e-6 {
                return Err(bad("a rectangle needs a width and a height"));
            }
            let (b, d) = (DVec2::new(c.x, a.y), DVec2::new(a.x, c.y));
            vec![line(a, b), line(b, c), line(c, d), line(d, a)]
        }
        "circle" => vec![Segment::Circle {
            center: point2(get("center")?, "center")?,
            radius: num("radius", 1.0e-6, MAX_COORD)?,
        }],
        "arc" => {
            let c = point2(get("center")?, "center")?;
            let (s, t) = (point2(get("start")?, "start")?, point2(get("end")?, "end")?);
            let radius = s.distance(c);
            if radius < 1.0e-6 || t.distance(c) < 1.0e-6 || s.distance(t) < 1.0e-6 {
                return Err(bad("an arc needs a centre and two different ends off it"));
            }
            let ccw = !e.get("clockwise").and_then(Value::as_bool).unwrap_or(false);
            let angle = |p: DVec2| (p.y - c.y).atan2(p.x - c.x);
            vec![Segment::Arc { center: c, radius, start: angle(s), end: angle(t), ccw }]
        }
        "polygon" => {
            let c = point2(get("center")?, "center")?;
            let r = num("radius", 1.0e-6, MAX_COORD)?;
            let sides = num("sides", 3.0, 360.0)? as usize;
            let a0 = e.get("angle").and_then(Value::as_f64).filter(|a| a.is_finite()).unwrap_or(90.0).to_radians();
            let at = |i: usize| {
                let a = a0 + std::f64::consts::TAU * i as f64 / sides as f64;
                c + DVec2::new(a.cos(), a.sin()) * r
            };
            (0..sides).map(|i| line(at(i), at(i + 1))).collect()
        }
        other => return Err(bad(format!("unknown sketch entity `{other}` (line, rectangle, circle, arc, polygon)"))),
    })
}

/// A sketch's settings worked out.
pub fn sketch_of(p: &Value) -> EngineResult<Sketch> {
    let plane = plane_of(p)?;
    let list = p.get("entities").and_then(Value::as_array).cloned().unwrap_or_default();
    if list.len() > MAX_ENTITIES {
        return Err(bad(format!("a sketch holds at most {MAX_ENTITIES} entities")));
    }
    let mut entities = Vec::with_capacity(list.len());
    let mut segments = Vec::new();
    for e in &list {
        let segs = entity(e)?;
        // Construction geometry is drawn but not part of the profile.
        if !e.get("construction").and_then(Value::as_bool).unwrap_or(false) {
            segments.extend(segs.iter().copied());
        }
        entities.push(segs);
    }
    Ok(Sketch { plane, segments, entities })
}

/// The operation a feature's settings ask for, given whether there is a body already.
fn operation(p: &Value, default: &str, have_body: bool) -> EngineResult<&'static str> {
    let op = params::opt_str(p, "operation")?.unwrap_or(default);
    Ok(match op {
        "cut" if !have_body => return Err(bad("there is nothing to cut yet; make a body first")),
        "cut" => "cut",
        "new" => "new",
        "join" if have_body => "join",
        "join" => "new",
        other => return Err(bad(format!("unknown operation `{other}`"))),
    })
}

fn hash_feature(h: u64, f: &Feature) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut s = std::collections::hash_map::DefaultHasher::new();
    h.hash(&mut s);
    f.kind.hash(&mut s);
    f.params.to_string().hash(&mut s);
    f.suppressed.hash(&mut s);
    s.finish()
}

impl Part {
    pub fn find(&self, id: &str) -> Option<&Feature> {
        self.features.iter().find(|f| f.id == id)
    }

    /// Takes the bodies `other` worked out (a preview of the same part), so a rebuild after the
    /// same change finds them ready. Each is checked against the history before it is used.
    pub fn share_cache(&mut self, other: &Part) {
        if other.cache.len() >= self.cache.len() {
            self.cache = other.cache.clone();
            self.meshed = other.meshed.clone();
        }
    }

    /// A name for a new feature of `kind`: its label and the next free number.
    pub fn new_name(&self, label: &str) -> String {
        (1..).map(|n| format!("{label}{n}")).find(|n| self.features.iter().all(|f| &f.name != n)).unwrap_or_default()
    }

    /// The sketches up to (not including) feature `upto`, by id.
    pub fn sketch(&self, id: &str) -> EngineResult<Sketch> {
        let f = self.find(id).filter(|f| f.kind == "sketch").ok_or_else(|| bad(format!("no sketch `{id}`")))?;
        if f.suppressed {
            return Err(bad(format!("{} is suppressed", f.name)));
        }
        sketch_of(&f.params)
    }

    /// Works the body out from the features, reusing what has not changed. A feature that
    /// cannot be made stops the rebuild with its error.
    pub fn rebuild(&mut self) -> EngineResult<Body> {
        self.rebuild_hashed().map(|(b, _)| b)
    }

    /// [`Part::rebuild`], with the hash of the whole history.
    fn rebuild_hashed(&mut self) -> EngineResult<(Body, u64)> {
        let mut body = Body::default();
        let mut hash = 0u64;
        let mut cache = std::mem::take(&mut self.cache);
        for (i, f) in self.features.clone().iter().enumerate() {
            hash = hash_feature(hash, f);
            if let Some((h, b)) = cache.get(i)
                && *h == hash
            {
                body = b.clone();
                continue;
            }
            cache.truncate(i);
            if f.kind != "sketch" && !f.suppressed {
                body = self.apply(f, &body).map_err(|e| match e {
                    EngineError::InvalidParams(m) => bad(format!("{}: {m}", f.name)),
                    other => other,
                })?;
            }
            cache.push((hash, body.clone()));
        }
        cache.truncate(self.features.len());
        self.cache = cache;
        Ok((body, hash))
    }

    /// Feature `f` applied to `body`. Faces flush with the body's trip up the kernel's
    /// booleans (they fail, or give a body that cannot be right by its volume): then the
    /// feature's solid is tried again reaching a micrometre, then ten, past both its ends.
    fn apply(&self, f: &Feature, body: &Body) -> EngineResult<Body> {
        let kernel = |e: CosmicDMS_kernel::KernelError| bad(e.0);
        let op = operation(&f.params, "join", !body.is_empty())?;
        if op == "new" {
            return Ok(body.with(&self.tool(f, 0.0, body.bounds())?));
        }
        let before = body.volume().map_err(kernel)?;
        let part_box = body.bounds();
        for reach in [0.0, 1.0e-3, 1.0e-2] {
            let tool = self.tool(f, reach, part_box)?;
            let made = if op == "cut" {
                body.combine(&tool, Operation::Cut, TOLERANCE)
            } else {
                body.combine(&tool, Operation::Join, TOLERANCE)
            };
            let (Ok(made), Ok(t)) = (made, tool.volume()) else { continue };
            let Ok(after) = made.volume() else { continue };
            let slack = 0.03 * before.max(t) + 1.0e-3;
            // A cut that leaves nothing is right only when the tool covers the whole part.
            let covers = match (tool.bounds(), part_box) {
                (Some((tl, th)), Some((bl, bh))) => tl.cmple(bl).all() && th.cmpge(bh).all(),
                _ => false,
            };
            let plausible = if op == "cut" {
                after <= before + slack && after >= before - t - slack && (after > slack || covers)
            } else {
                after >= before.max(t) - slack && after <= before + t + slack
            };
            if plausible {
                return Ok(made);
            }
        }
        Err(bad(if op == "cut" {
            "the kernel could not cut this from the part; try moving the sketch or changing the depth slightly"
        } else {
            "the kernel could not join this to the part; try moving the sketch or changing the distance slightly"
        }))
    }

    /// The solid feature `f` sweeps, reaching `reach` mm past its ends; a through-all one
    /// reaches past `part` (the part's box) both ways.
    fn tool(&self, f: &Feature, reach: f64, part: Option<(DVec3, DVec3)>) -> EngineResult<Body> {
        let p = &f.params;
        let sketch = self.sketch(params::req_str(p, "sketch")?)?;
        let kernel = |e: CosmicDMS_kernel::KernelError| bad(e.0);
        let (tool, _default) = match f.kind.as_str() {
            "extrude" => {
                let through = params::opt_bool(p, "through_all")?.unwrap_or(false);
                // Far enough to clear the part from the sketch plane, both ways.
                let through_len = part.map_or(THROUGH, |(lo, hi)| {
                    let centre = (lo + hi) / 2.0;
                    (hi - lo).length() + (centre - sketch.plane.origin).dot(sketch.plane.normal()).abs() + 1.0
                });
                let d = if through {
                    through_len
                } else {
                    params::opt_f64(p, "distance", 1.0e-4, MAX_COORD)?
                        .ok_or_else(|| bad("an extrusion needs a `distance`"))?
                };
                let (from, to) = if through {
                    (-through_len, through_len)
                } else {
                    match params::opt_str(p, "direction")?.unwrap_or("normal") {
                        "reverse" => (-d, 0.0),
                        "symmetric" => (-d / 2.0, d / 2.0),
                        _ => (0.0, d),
                    }
                };
                (Body::extrude(&sketch.plane, &sketch.segments, from - reach, to + reach).map_err(kernel)?, "join")
            }
            "revolve" => {
                let angle = params::opt_f64(p, "angle", 0.001, 360.0)?.unwrap_or(360.0);
                let (origin, dir, skip) = match params::opt_str(p, "axis")?.unwrap_or("y") {
                    "x" => (DVec2::ZERO, DVec2::X, None),
                    "line" => {
                        let k = params::opt_f64(p, "line", 0.0, MAX_ENTITIES as f64)?
                            .ok_or_else(|| bad("give the axis `line` (its entity number)"))?
                            as usize;
                        match sketch.entities.get(k).map(Vec::as_slice) {
                            Some([Segment::Line { a, b }]) => (*a, *b - *a, Some(k)),
                            _ => return Err(bad(format!("sketch entity {k} is not a line"))),
                        }
                    }
                    _ => (DVec2::ZERO, DVec2::Y, None),
                };
                // The axis line is not part of the profile.
                let segments: Vec<Segment> = match skip {
                    Some(k) => sketch
                        .entities
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != k)
                        .flat_map(|(_, s)| s.iter().copied())
                        .collect(),
                    None => sketch.segments.clone(),
                };
                (Body::revolve(&sketch.plane, &segments, origin, dir, angle).map_err(kernel)?, "join")
            }
            other => return Err(bad(format!("unknown feature `{other}`"))),
        };
        Ok(tool)
    }
}

/// Rebuilds the part and puts its body into the job as the model it owns (made, replaced or
/// removed). Returns the number of triangles.
pub fn update_model(job: &mut JobState) -> EngineResult<usize> {
    let (body, hash) = job.part.rebuild_hashed()?;
    let id = job.part.model.clone();
    let meshed = match job.part.meshed.as_ref().filter(|(h, _)| *h == hash) {
        Some((_, m)) => Some(m.clone()),
        None => {
            let tris = body.triangles(TOLERANCE).map_err(|e| bad(e.0))?;
            if tris.is_empty() {
                None
            } else {
                let mesh =
                    Mesh::from_soup(tris.iter().map(|t| t.map(|p| Vec3::new(p.x as f32, p.y as f32, p.z as f32))))
                        .map_err(|e| EngineError::Internal(format!("the part's mesh: {e}")))?;
                let (faces, edges) = Model::recognise(&mesh);
                let m = (Arc::new(mesh), faces, edges);
                job.part.meshed = Some((hash, m.clone()));
                Some(m)
            }
        }
    };
    let Some((mesh, faces, edges)) = meshed else {
        if let Some(id) = id {
            job.models.retain(|m| m.id != id);
        }
        job.part.model = None;
        job.geometry_changed();
        return Ok(0);
    };
    let count = mesh.triangles().len();
    match id.as_ref().and_then(|id| job.models.iter_mut().find(|m| &m.id == id)) {
        Some(m) => {
            m.mesh = mesh;
            m.faces = faces;
            m.edges = edges;
        }
        None => {
            let id = format!("model.{}", job.next_model);
            job.next_model = job.next_model.saturating_add(1);
            job.models.push(Model {
                id: id.clone(),
                name: "Part".into(),
                source: "CAD".into(),
                format: None,
                units: CosmicDMS_io::Units::Millimetres,
                mesh,
                faces,
                edges,
                visible: true,
            });
            job.part.model = Some(id);
        }
    }
    job.geometry_changed();
    Ok(count)
}

/// Volume, surface area, centre of mass and bounds of a closed mesh, mm.
pub fn mass_properties(mesh: &Mesh) -> Value {
    let (mut volume, mut area, mut moment) = (0.0f64, 0.0f64, DVec3::ZERO);
    for t in mesh.triangles() {
        let [a, b, c] = mesh.corners(*t).map(|p| p.as_dvec3());
        let v = a.dot(b.cross(c)) / 6.0;
        volume += v;
        moment += (a + b + c) / 4.0 * v;
        area += (b - a).cross(c - a).length() / 2.0;
    }
    let b = mesh.bounds();
    json!({
        "volume": volume,
        "area": area,
        "center_of_mass": if volume.abs() > 1.0e-12 { (moment / volume).to_array() } else { [0.0; 3] },
        "bounds": crate::session::bounds_json(&b),
    })
}

/// A segment as points in the plane, a few per arc.
pub fn segment_points(s: &Segment) -> Vec<DVec2> {
    let arc = |c: DVec2, r: f64, a0: f64, sweep: f64| -> Vec<DVec2> {
        let n = ((sweep.abs() / std::f64::consts::TAU) * 64.0).ceil().clamp(4.0, 64.0) as usize;
        (0..=n)
            .map(|i| {
                let a = a0 + sweep * i as f64 / n as f64;
                c + DVec2::new(a.cos(), a.sin()) * r
            })
            .collect()
    };
    match *s {
        Segment::Line { a, b } => vec![a, b],
        Segment::Arc { center, radius, start, end, ccw } => {
            let mut sweep = end - start;
            if ccw {
                while sweep <= 0.0 {
                    sweep += std::f64::consts::TAU;
                }
            } else {
                while sweep >= 0.0 {
                    sweep -= std::f64::consts::TAU;
                }
            }
            arc(center, radius, start, sweep)
        }
        Segment::Circle { center, radius } => arc(center, radius, 0.0, std::f64::consts::TAU),
    }
}

/// Every sketch's entities as world polylines, for drawing (suppressed sketches left out).
pub fn sketch_polylines(part: &Part) -> Vec<Vec<DVec3>> {
    let mut out = Vec::new();
    for f in part.features.iter().filter(|f| f.kind == "sketch" && !f.suppressed) {
        let Ok(sk) = sketch_of(&f.params) else { continue };
        for seg in sk.entities.iter().flatten() {
            out.push(segment_points(seg).into_iter().map(|p| sk.plane.world(p)).collect());
        }
    }
    out
}
