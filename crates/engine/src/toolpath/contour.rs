//! 2D contour: the tool follows selected edges (or a face's outline) at one or more depths,
//! offset by its radius plus stock to leave, with optional extra radial passes, spring
//! passes, lead-in/out arcs or lines, ramping, a retract policy and feed reduction at sharp
//! direction changes.
//!
//! Everything is worked out in the WCS: contours are projected onto its XY plane, heights
//! are WCS Z, and the moves are turned back into world coordinates at the end.

use std::sync::Arc;

use CosmicCAM_geom::faces::boundary_loops;
use CosmicCAM_geom::offset::{arc_step, offset_closed, offset_open, signed_area2};
use CosmicCAM_geom::{Aabb, EdgeKind, FaceKind, Vec3};
use glam::{DVec2, DVec3};
use serde_json::{Map, Value, json};

use super::{MAX_MOVES, Move, MoveKind, Toolpath};
use crate::error::{EngineError, EngineResult};
use crate::library::{Machine, Tool};
use crate::params;
use crate::session::{Entity, Session};

pub const HEIGHT_REFS: &[&str] = &["selection", "model_top", "model_bottom", "stock_top", "stock_bottom", "origin"];
pub const SIDES: &[&str] = &["outside", "inside", "on"];
pub const DIRECTIONS: &[&str] = &["climb", "conventional"];
pub const COOLANTS: &[&str] = &["off", "flood", "mist", "air", "through_tool"];
pub const LEADS: &[&str] = &["arc", "line", "none"];
pub const RAMPS: &[&str] = &["ramp", "plunge"];
pub const RETRACTS: &[&str] = &["minimum", "full", "stay_down"];
pub const TOOL_AXES: &[&str] = &["wcs_z"];
/// Where closed contours start: mid-way along the longest side, or nearest a point.
pub const ENTRIES: &[&str] = &["auto", "origin", "wcs", "selection"];
/// The heights, top to bottom: (parameter prefix, default reference, default offset in mm).
pub const HEIGHTS: &[(&str, &str, f64)] = &[
    ("clearance", "model_top", 15.0),
    ("retract", "model_top", 5.0),
    ("feed_height", "model_top", 2.0),
    ("top", "model_top", 0.0),
    ("bottom", "selection", 0.0),
];

fn bad(m: impl Into<String>) -> EngineError {
    EngineError::InvalidParams(m.into())
}

/// The WCS (or world) frame the toolpath is worked out in.
#[derive(Clone, Copy)]
pub(super) struct Frame {
    pub(super) origin: DVec3,
    pub(super) x: DVec3,
    pub(super) y: DVec3,
    pub(super) z: DVec3,
}

impl Frame {
    pub(super) fn local(&self, p: Vec3) -> DVec3 {
        let d = p.as_dvec3() - self.origin;
        DVec3::new(d.dot(self.x), d.dot(self.y), d.dot(self.z))
    }

    pub(super) fn world(&self, l: DVec3) -> Vec3 {
        (self.origin + self.x * l.x + self.y * l.y + self.z * l.z).as_vec3()
    }

    /// A direction given in the frame, in world coordinates.
    pub(super) fn world_dir(&self, d: DVec3) -> Vec3 {
        (self.x * d.x + self.y * d.y + self.z * d.z).normalize_or_zero().as_vec3()
    }

    /// Local Z range of a world box.
    pub(super) fn z_range(&self, b: Aabb) -> Option<(f64, f64)> {
        if b.is_empty() {
            return None;
        }
        let zs: Vec<f64> = (0..8)
            .map(|i| {
                let c = Vec3::new(
                    if i & 1 == 0 { b.min.x } else { b.max.x },
                    if i & 2 == 0 { b.min.y } else { b.max.y },
                    if i & 4 == 0 { b.min.z } else { b.max.z },
                );
                self.local(c).z
            })
            .collect();
        Some((zs.iter().copied().fold(f64::INFINITY, f64::min), zs.iter().copied().fold(f64::NEG_INFINITY, f64::max)))
    }
}

/// A contour in the WCS plane.
#[derive(Clone, Debug)]
pub(super) struct Chain {
    pub(super) pts: Vec<DVec2>,
    pub(super) z: f64,
    pub(super) closed: bool,
}

/// All settings, resolved: millimetres and mm/min, with the tool's data filled in.
struct Settings {
    name: String,
    tool: Tool,
    feed: f64,
    plunge_feed: f64,
    lead_feed: f64,
    side: String,
    reverse: bool,
    climb: bool,
    both_ways: bool,
    radial_passes: u32,
    radial_stepover: f64,
    spring_passes: u32,
    stock_radial: f64,
    stock_axial: f64,
    multiple_depths: bool,
    max_stepdown: f64,
    optimize: Option<(f64, f64, f64)>,
    tolerance: f64,
    lead_in: (String, f64, f64, f64),
    lead_out: (String, f64, f64, f64),
    ramp_angle: Option<f64>,
    retract: String,
    stay_down_distance: f64,
    link_feed: Option<f64>,
}

/// The tool a toolpath uses: `tool` (a number in the active machine's library), else the
/// first tool there of one of `kinds` (tool types), which it must be. With `preset`, the
/// tool's cutting data is that preset's.
pub(super) fn pick_tool<'a>(s: &'a Session, p: &Value, kinds: &[&str]) -> EngineResult<(&'a Machine, Tool)> {
    let m = s.library.active_machine().ok_or_else(|| bad("there is no machine; create one on the Machine tab"))?;
    let tool = match p.get("tool").and_then(Value::as_u64) {
        Some(n) => {
            m.tool(u32::try_from(n).unwrap_or(0)).ok_or_else(|| bad(format!("no tool T{n} in {}'s library", m.name)))?
        }
        None if m.tools.is_empty() => {
            return Err(bad(format!("{}'s tool library is empty; add a tool first", m.name)));
        }
        None => m.tools.iter().find(|t| kinds.contains(&t.kind.as_str())).ok_or_else(|| {
            bad(format!("{}'s tool library has no {} tool; add one first", m.name, kinds.join(" or ")))
        })?,
    };
    if !kinds.contains(&tool.kind.as_str()) {
        return Err(EngineError::NotSupported(format!(
            "this strategy with {} tools (T{}); it takes {}",
            tool.kind,
            tool.number,
            kinds.join(" or ")
        )));
    }
    let mut tool = tool.clone();
    if let Some(name) = params::opt_str(p, "preset")?.filter(|n| !n.is_empty()) {
        let c = tool.cutting(Some(name)).map_err(bad)?;
        for (slot, v) in
            [(&mut tool.spindle_rpm, c.spindle_rpm), (&mut tool.feed, c.feed), (&mut tool.plunge_feed, c.plunge_feed)]
        {
            if v > 0.0 {
                *slot = v;
            }
        }
    }
    Ok((m, tool))
}

/// A strategy built on the contour: its id, default name and the tool types it takes.
pub struct Variant {
    pub strategy: &'static str,
    pub name: &'static str,
    pub kinds: &'static [&'static str],
}

pub const CONTOUR: Variant = Variant { strategy: "contour2d", name: "2D contour", kinds: &["flat", "bull", "ball"] };

fn settings(s: &Session, p: &Value, kinds: &[&str]) -> EngineResult<(Settings, String)> {
    let (machine, tool) = pick_tool(s, p, kinds)?;
    let num = |key: &str, lo: f64, hi: f64| params::opt_f64(p, key, lo, hi);
    let choice = |key: &str, default: &str| -> EngineResult<String> {
        Ok(params::opt_str(p, key)?.unwrap_or(default).to_owned())
    };
    let d = tool.diameter;
    let pick = |v: Option<f64>, fallback: f64| v.filter(|f| *f > 0.0).unwrap_or(fallback);
    let feed = pick(num("feed", 0.0, 1.0e6)?, tool.feed);
    if feed <= 0.0 {
        return Err(bad(format!("T{} has no cutting feed; set `feed` or the tool's feeds", tool.number)));
    }
    let plunge_feed =
        pick(num("plunge_feed", 0.0, 1.0e6)?, if tool.plunge_feed > 0.0 { tool.plunge_feed } else { feed * 0.5 });
    let lead_feed = pick(num("lead_feed", 0.0, 1.0e6)?, feed);
    for (name, v) in [("feed", feed), ("plunge_feed", plunge_feed), ("lead_feed", lead_feed)] {
        if v > machine.max_feed {
            return Err(bad(format!("`{name}` {v} is above {}'s maximum feed of {}", machine.name, machine.max_feed)));
        }
    }
    let lead = |prefix: &str| -> EngineResult<(String, f64, f64, f64)> {
        Ok((
            choice(prefix, "arc")?,
            pick(num(&format!("{prefix}_radius"), 0.0, 1.0e4)?, d / 2.0),
            pick(num(&format!("{prefix}_sweep"), 0.0, 180.0)?, 90.0).to_radians(),
            pick(num(&format!("{prefix}_length"), 0.0, 1.0e4)?, d / 2.0),
        ))
    };
    let lead_in = lead("lead_in")?;
    let lead_out = if params::opt_bool(p, "lead_out_same")?.unwrap_or(true) && p.get("lead_out").is_none() {
        lead_in.clone()
    } else {
        lead("lead_out")?
    };
    let optimize = if params::opt_bool(p, "feed_optimization")?.unwrap_or(false) {
        Some((
            pick(num("optimization_angle", 0.0, 179.0)?, 45.0).to_radians(),
            pick(num("optimization_feed", 0.0, 100.0)?, 50.0) / 100.0,
            pick(num("optimization_distance", 0.0, 1.0e4)?, d / 2.0),
        ))
    } else {
        None
    };
    let ramp_angle = match choice("ramp", "ramp")?.as_str() {
        "ramp" => Some(pick(num("ramp_angle", 0.0, 90.0)?, 3.0).to_radians()),
        _ => None,
    };
    let link_feed = if params::opt_bool(p, "rapid_links")?.unwrap_or(true) {
        None
    } else {
        Some(pick(num("link_feed", 0.0, 1.0e6)?, machine.max_feed))
    };
    let settings = Settings {
        name: params::opt_str(p, "name")?.map(crate::library::clip).filter(|n| !n.is_empty()).unwrap_or_default(),
        tool: tool.clone(),
        feed,
        plunge_feed,
        lead_feed,
        side: choice("side", "outside")?,
        reverse: params::opt_bool(p, "reverse")?.unwrap_or(false),
        climb: choice("direction", "climb")? == "climb",
        both_ways: params::opt_bool(p, "both_ways")?.unwrap_or(false),
        radial_passes: p.get("radial_passes").and_then(Value::as_u64).map_or(1, |n| n.clamp(1, 100) as u32),
        radial_stepover: pick(
            num("radial_stepover", 0.0, 1.0e4)?,
            if tool.stepover > 0.0 { tool.stepover } else { d * 0.4 },
        ),
        spring_passes: p.get("spring_passes").and_then(Value::as_u64).map_or(0, |n| n.min(10) as u32),
        stock_radial: num("stock_radial", -1.0e3, 1.0e3)?.unwrap_or(0.0),
        stock_axial: num("stock_axial", -1.0e3, 1.0e3)?.unwrap_or(0.0),
        multiple_depths: params::opt_bool(p, "multiple_depths")?.unwrap_or(false),
        max_stepdown: pick(num("max_stepdown", 0.0, 1.0e4)?, if tool.stepdown > 0.0 { tool.stepdown } else { d }),
        optimize,
        tolerance: pick(num("tolerance", 0.0, 1.0)?, 0.01),
        lead_in,
        lead_out,
        ramp_angle,
        retract: choice("retract", "minimum")?,
        stay_down_distance: pick(num("stay_down_distance", 0.0, 1.0e5)?, 2.0 * d),
        link_feed,
    };
    Ok((settings, machine.id.clone()))
}

/// The frame the toolpath is worked out in, and the WCS it is programmed in: the WCS named by
/// `wcs`, else the active WCS, else world. An `orientation` (another WCS) tilts the frame for
/// 3+2 machining: the tool works along its Z while the program stays in the WCS.
pub(super) fn frame(s: &Session, p: &Value) -> EngineResult<(Frame, Option<String>)> {
    let w = match params::opt_str(p, "wcs")? {
        Some(id) => Some(s.job.wcs_by_id(id).ok_or_else(|| bad(format!("no WCS `{id}`")))?),
        None => s.job.active_wcs(),
    };
    let of = |w: &crate::session::Wcs| Frame {
        origin: w.origin.as_dvec3(),
        x: w.x_axis.as_dvec3(),
        y: w.y_axis().as_dvec3(),
        z: w.z_axis.as_dvec3(),
    };
    let world = Frame { origin: DVec3::ZERO, x: DVec3::X, y: DVec3::Y, z: DVec3::Z };
    let program = w.map(|w| w.id.clone());
    let frame = match params::opt_str(p, "orientation")?.filter(|o| !o.is_empty()) {
        Some(id) => of(s.job.wcs_by_id(id).ok_or_else(|| bad(format!("`orientation`: no WCS `{id}`")))?),
        None => w.map_or(world, of),
    };
    Ok((frame, program))
}

/// Most points one resampled circle gets.
const MAX_CIRCLE_POINTS: usize = 20_000;

/// The circle through `points` (centre, normal, radius), sampled so no chord strays more than
/// `tolerance` from the arc. It starts where `points` does and runs the same way round.
fn resample_circle(points: &[Vec3], center: Vec3, normal: Vec3, radius: f32, tolerance: f64) -> Vec<Vec3> {
    let (c, n, r) = (center.as_dvec3(), normal.as_dvec3().normalize_or_zero(), f64::from(radius));
    let Some(&first) = points.first() else { return Vec::new() };
    let u = {
        let d = first.as_dvec3() - c;
        (d - n * d.dot(n)).normalize_or_zero()
    };
    if r <= 1.0e-9 || u == DVec3::ZERO || n == DVec3::ZERO || !r.is_finite() {
        return points.to_vec();
    }
    let mut v = n.cross(u);
    // Keep the mesh loop's direction.
    if let Some(&second) = points.get(1)
        && (second.as_dvec3() - c).dot(v) < 0.0
    {
        v = -v;
    }
    let step = 2.0 * (1.0 - (tolerance.max(1.0e-4) / r).min(1.0)).acos();
    let count = if step > 1.0e-9 { (std::f64::consts::TAU / step).ceil() as usize } else { MAX_CIRCLE_POINTS };
    let count = count.clamp(8, MAX_CIRCLE_POINTS);
    (0..=count)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / count as f64;
            (c + (u * a.cos() + v * a.sin()) * r).as_vec3()
        })
        .collect()
}

/// A cylinder face's rim loop, resampled as the circle it is when its points lie on one.
fn circle_rim(rim: &[Vec3], axis: Vec3, radius: f32, tolerance: f64) -> Vec<Vec3> {
    if rim.len() < 3 {
        return rim.to_vec();
    }
    let n = rim.len() as f32;
    let center = rim.iter().fold(Vec3::ZERO, |a, p| a + *p) / n;
    let round = rim.iter().all(|p| (p.distance(center) - radius).abs() <= radius * 0.02 + 1.0e-3);
    if round { resample_circle(rim, center, axis, radius, tolerance) } else { rim.to_vec() }
}

/// The selected geometry as contours in the WCS plane.
pub(super) fn chains(
    s: &Session,
    refs: &[String],
    f: &Frame,
    tolerance: f64,
    warnings: &mut Vec<String>,
) -> EngineResult<Vec<Chain>> {
    if refs.is_empty() {
        return Err(bad("select the contour: edges, or a face to follow its outline (`geometry`)"));
    }
    let mut loops: Vec<Vec<Vec3>> = Vec::new();
    let mut pieces: Vec<Vec<Vec3>> = Vec::new();
    for r in refs {
        let (m, entity) = s.job.entity(r).ok_or_else(|| bad(format!("`geometry`: no face or edge `{r}`")))?;
        match entity {
            Entity::Edge(e) => {
                let edge = m.edges.edges.get(e).ok_or_else(|| bad(format!("`geometry`: no edge `{r}`")))?;
                match edge.kind {
                    EdgeKind::Circle { center, normal, radius } => {
                        loops.push(resample_circle(&edge.points, center, normal, radius, tolerance));
                    }
                    EdgeKind::Line { .. } => pieces.push(edge.points.clone()),
                }
            }
            Entity::Face(fi) => {
                let mut boundary = boundary_loops(&m.mesh, &m.faces, fi);
                let kind = m.faces.faces.get(fi).map(|f| f.kind);
                // A plane's outline is its longest loop; a hole or boss's is its lowest rim.
                let pick = match kind {
                    Some(FaceKind::Cylinder { .. }) => boundary
                        .iter()
                        .enumerate()
                        .min_by(|a, b| {
                            let z =
                                |l: &Vec<Vec3>| l.iter().map(|p| f.local(*p).z).sum::<f64>() / l.len().max(1) as f64;
                            z(a.1).total_cmp(&z(b.1))
                        })
                        .map(|(i, _)| i),
                    _ => (!boundary.is_empty()).then_some(0),
                };
                match pick {
                    Some(i) if i < boundary.len() => {
                        let rim = boundary.swap_remove(i);
                        // A cylinder's rim is a true circle: follow it at the tolerance, not the mesh.
                        loops.push(match kind {
                            Some(FaceKind::Cylinder { axis, radius, .. }) => circle_rim(&rim, axis, radius, tolerance),
                            _ => rim,
                        });
                    }
                    _ => return Err(bad(format!("`geometry`: {r} has no outline to follow"))),
                }
            }
            Entity::Vertex(_) => return Err(bad(format!("`geometry`: {r} is a vertex; select edges or a face"))),
        }
    }
    let to_chain = |pts: &[Vec3], closed: bool, warnings: &mut Vec<String>| -> Option<Chain> {
        let local: Vec<DVec3> = pts.iter().map(|p| f.local(*p)).collect();
        let (zmin, zmax) =
            local.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p.z), hi.max(p.z)));
        if zmax - zmin > tolerance.max(0.01) {
            warnings.push("a contour is not flat in the WCS; it was projected onto the XY plane".into());
        }
        let mut flat: Vec<DVec2> = Vec::new();
        for p in &local {
            if flat.last().is_none_or(|q: &DVec2| q.distance(p.truncate()) > 1.0e-6) {
                flat.push(p.truncate());
            }
        }
        let mut closed = closed;
        if flat.len() > 2 && flat.first().zip(flat.last()).is_some_and(|(a, b)| a.distance(*b) <= tolerance.max(1.0e-3))
        {
            flat.pop();
            closed = true;
        }
        (flat.len() >= 2).then(|| Chain {
            pts: flat,
            z: local.iter().map(|p| p.z).sum::<f64>() / local.len().max(1) as f64,
            closed,
        })
    };
    let mut out: Vec<Chain> = loops.iter().filter_map(|l| to_chain(l, true, warnings)).collect();
    // Join edge pieces end to end into chains.
    let join_tol = tolerance.max(1.0e-3);
    while let Some(first) = pieces.pop() {
        let mut chain = first;
        while let (Some(&head), Some(&tail)) = (chain.first(), chain.last()) {
            let near = |a: Vec3, b: Vec3| f64::from(a.distance(b)) <= join_tol;
            let found = pieces.iter().position(|pc| {
                pc.first()
                    .zip(pc.last())
                    .is_some_and(|(a, b)| near(*a, tail) || near(*b, tail) || near(*a, head) || near(*b, head))
            });
            let Some(i) = found else { break };
            let mut pc = pieces.swap_remove(i);
            let (a, b) = (pc.first().copied().unwrap_or_default(), pc.last().copied().unwrap_or_default());
            if near(a, tail) {
                chain.extend(pc.into_iter().skip(1));
            } else if near(b, tail) {
                pc.reverse();
                chain.extend(pc.into_iter().skip(1));
            } else if near(b, head) {
                pc.pop();
                pc.extend(chain);
                chain = pc;
            } else {
                pc.reverse();
                pc.pop();
                pc.extend(chain);
                chain = pc;
            }
        }
        if let Some(c) = to_chain(&chain, false, warnings) {
            out.push(c);
        }
    }
    Ok(out)
}

/// A height's default offset: the millimetre default, or a round inch value when the user
/// works in inches (0.5, 0.2 and 0.1 in for clearance, retract and feed).
pub fn default_offset(s: &Session, mm: f64) -> f64 {
    if s.prefs.units == crate::units::DisplayUnits::Inches {
        match mm {
            m if m >= 15.0 => 12.7,
            m if m >= 5.0 => 5.08,
            m if m >= 2.0 => 2.54,
            m => m,
        }
    } else {
        mm
    }
}

/// A height's value in WCS Z: its reference plus its offset.
fn height(
    s: &Session,
    p: &Value,
    f: &Frame,
    chains: &[Chain],
    prefix: &str,
    default_ref: &str,
    default_offset: f64,
) -> EngineResult<f64> {
    let reference = params::opt_str(p, &format!("{prefix}_from"))?.unwrap_or(default_ref);
    let offset = params::opt_f64(p, &format!("{prefix}_offset"), -1.0e5, 1.0e5)?
        .unwrap_or(self::default_offset(s, default_offset));
    let sel = |top: bool| {
        let zs = chains.iter().map(|c| c.z);
        if top { zs.fold(f64::NEG_INFINITY, f64::max) } else { zs.fold(f64::INFINITY, f64::min) }
    };
    let base = match reference {
        "selection" => sel(prefix != "bottom"),
        "model_top" | "model_bottom" => {
            let (lo, hi) = f.z_range(s.job.part_bounds()).ok_or_else(|| bad("there is no model for a model height"))?;
            if reference == "model_top" { hi } else { lo }
        }
        "stock_top" | "stock_bottom" => {
            let (lo, hi) = s
                .job
                .stock
                .and_then(|b| f.z_range(b))
                .ok_or_else(|| bad("there is no stock; fit one with setup.stock.fit"))?;
            if reference == "stock_top" { hi } else { lo }
        }
        "origin" => 0.0,
        other => return Err(bad(format!("unknown height reference `{other}`"))),
    };
    if !base.is_finite() {
        return Err(bad(format!("`{prefix}_from`: no height to take")));
    }
    Ok(base + offset)
}

/// Turns `v` by `a` radians.
fn rot(v: DVec2, a: f64) -> DVec2 {
    let (s, c) = a.sin_cos();
    DVec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

pub(super) struct Builder {
    pub(super) frame: Frame,
    pub(super) moves: Vec<Move>,
    pub(super) at: DVec3,
    /// The tool axis (in the frame) the next moves end with.
    pub(super) axis: DVec3,
    /// Each move's tool axis in world coordinates, once any differs from the frame's Z.
    pub(super) axes: Option<Vec<Vec3>>,
}

impl Builder {
    pub(super) fn new(frame: Frame) -> Self {
        Self { frame, moves: Vec::new(), at: DVec3::ZERO, axis: DVec3::Z, axes: None }
    }

    pub(super) fn push(&mut self, kind: MoveKind, to: DVec3, feed: f64) -> EngineResult<()> {
        self.push_axis(kind, to, feed, self.axis)
    }

    /// A move ending with the tool along `axis` (a unit vector in the frame, from the tip up).
    pub(super) fn push_axis(&mut self, kind: MoveKind, to: DVec3, feed: f64, axis: DVec3) -> EngineResult<()> {
        let turned = axis.distance(self.axis) > 1.0e-9;
        if to.distance(self.at) < 1.0e-9 && !turned {
            return Ok(());
        }
        if self.moves.len() >= MAX_MOVES {
            return Err(bad(format!("the toolpath would have more than {MAX_MOVES} moves; loosen the tolerance")));
        }
        let world_axis = self.frame.world_dir(axis);
        if self.axes.is_none() && axis.distance(DVec3::Z) > 1.0e-9 {
            self.axes = Some(vec![self.frame.z.as_vec3(); self.moves.len()]);
        }
        if let Some(axes) = self.axes.as_mut() {
            axes.push(world_axis);
        }
        self.moves.push(Move { kind, to: self.frame.world(to), feed: feed as f32 });
        self.at = to;
        self.axis = axis;
        Ok(())
    }
}

/// One closed or open path to cut at depth `z`, in travel order.
struct Cut {
    pts: Vec<DVec2>,
    closed: bool,
    z: f64,
    chain: usize,
}

/// Points of a lead: from its free end to the contour (lead-in) or from the contour out
/// (lead-out). `away` is the unit normal away from the material.
fn lead(
    kind: &(String, f64, f64, f64),
    at: DVec2,
    tangent: DVec2,
    away: DVec2,
    entering: bool,
    tol: f64,
) -> Vec<DVec2> {
    let (style, radius, sweep, length) = (kind.0.as_str(), kind.1, kind.2, kind.3);
    match style {
        "arc" if radius > 1.0e-6 && sweep > 1.0e-6 => {
            let c = at + away * radius;
            let u = -away;
            let s = u.perp_dot(tangent).signum();
            let steps = (sweep / arc_step(radius, tol)).ceil().clamp(2.0, 512.0) as usize;
            (0..=steps)
                .map(|k| {
                    let t = k as f64 / steps as f64;
                    let phi = if entering { sweep * (1.0 - t) } else { sweep * t };
                    c + rot(u, if entering { -s * phi } else { s * phi }) * radius
                })
                .collect()
        }
        "line" if length > 1.0e-6 => {
            if entering {
                vec![at + away * length, at]
            } else {
                vec![at, at + away * length]
            }
        }
        _ => vec![at],
    }
}

/// Cutting moves with the feed reduced where the path changes direction: at a sharp corner,
/// or along a tight arc, wherever the direction turns more than `angle` within `distance`.
/// Moves are split so only `distance` either side of a lone corner runs slow.
fn optimized_cut(pts: &[DVec2], closed: bool, feed: f64, opt: Option<(f64, f64, f64)>) -> Vec<(DVec2, f64)> {
    let mut path: Vec<DVec2> = pts.to_vec();
    if closed && let Some(&p) = pts.first() {
        path.push(p);
    }
    let Some((angle, factor, distance)) = opt else {
        return path.into_iter().skip(1).map(|p| (p, feed)).collect();
    };
    let n = path.len();
    // Turn at each vertex and its distance along the path.
    let turn = |i: usize| -> f64 {
        let (prev, next) = if i == 0 || i == n - 1 {
            if !closed {
                return 0.0;
            }
            (path.get(n - 2).copied(), path.get(1).copied())
        } else {
            (path.get(i - 1).copied(), path.get(i + 1).copied())
        };
        let (Some(a), Some(b), Some(c)) = (prev, path.get(i).copied(), next) else { return 0.0 };
        let (d1, d2) = ((b - a).normalize_or_zero(), (c - b).normalize_or_zero());
        d1.dot(d2).clamp(-1.0, 1.0).acos()
    };
    let turns: Vec<f64> = (0..n).map(turn).collect();
    let mut along = vec![0.0f64; n];
    for i in 1..n {
        let step = path.get(i - 1).zip(path.get(i)).map_or(0.0, |(a, b)| a.distance(*b));
        if let (Some(prev), Some(slot)) = (along.get(i - 1).copied(), along.get_mut(i)) {
            *slot = prev + step;
        }
    }
    // Turning within ±distance of each vertex, from prefix sums (distance along only grows).
    let mut prefix = vec![0.0f64; n + 1];
    for (i, t) in turns.iter().enumerate() {
        if let (Some(prev), Some(slot)) = (prefix.get(i).copied(), prefix.get_mut(i + 1)) {
            *slot = prev + t;
        }
    }
    let flagged: Vec<bool> = (0..n)
        .map(|i| {
            let here = along.get(i).copied().unwrap_or(0.0);
            let lo = along.partition_point(|s| *s < here - distance);
            let hi = along.partition_point(|s| *s <= here + distance);
            let total = prefix.get(hi).copied().unwrap_or(0.0) - prefix.get(lo).copied().unwrap_or(0.0);
            total > angle && turns.get(i).is_some_and(|t| *t > 1.0e-6)
        })
        .collect();
    let slow = feed * factor;
    let mut out = Vec::new();
    for i in 1..n {
        let (Some(a), Some(b)) = (path.get(i - 1).copied(), path.get(i).copied()) else { continue };
        let len = a.distance(b);
        let (fa, fb) = (flagged.get(i - 1).copied().unwrap_or(false), flagged.get(i).copied().unwrap_or(false));
        let reach = distance.min(len / 2.0);
        match (fa, fb) {
            (true, true) => out.push((b, slow)),
            (false, false) => out.push((b, feed)),
            (true, false) => {
                if reach < len - 1.0e-9 {
                    out.push((a.lerp(b, reach / len), slow));
                    out.push((b, feed));
                } else {
                    out.push((b, slow));
                }
            }
            (false, true) => {
                if reach < len - 1.0e-9 {
                    out.push((a.lerp(b, (len - reach) / len), feed));
                }
                out.push((b, slow));
            }
        }
    }
    out
}

/// Builds the contour toolpath for `p`; `id` is the toolpath id it will carry.
/// The point closed contours should start nearest, in the WCS plane, if one is set.
fn entry_point(p: &Value, f: &Frame) -> EngineResult<Option<DVec2>> {
    let at = match params::opt_str(p, "entry_from")?.unwrap_or("auto") {
        "origin" => Some(Vec3::ZERO),
        "wcs" => Some(f.world(DVec3::ZERO)),
        "selection" => Some(
            params::opt_vec3(p, "entry_point")?
                .ok_or_else(|| bad("pick the entry point (entry_point) or choose another entry"))?,
        ),
        _ => None,
    };
    Ok(at.map(|w| f.local(w).truncate()))
}

/// Puts a vertex on the closed loop `pts` where it passes nearest `target`, and returns it.
fn nearest_on_loop(pts: &mut Vec<DVec2>, target: DVec2) -> DVec2 {
    let n = pts.len();
    let mut best: Option<(f64, usize, DVec2)> = None;
    for i in 0..n {
        let (Some(&a), Some(&b)) = (pts.get(i), pts.get((i + 1) % n)) else { continue };
        let ab = b - a;
        let t = if ab.length_squared() > 1.0e-18 {
            ((target - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = a + ab * t;
        let d = q.distance(target);
        if best.is_none_or(|(bd, ..)| d < bd) {
            best = Some((d, i, q));
        }
    }
    match best {
        Some((_, i, q)) => {
            let duplicate = |p: Option<&DVec2>| p.is_some_and(|p| p.distance(q) < 1.0e-6);
            if !duplicate(pts.get(i)) && !duplicate(pts.get((i + 1) % n)) {
                pts.insert(i + 1, q);
            }
            q
        }
        None => target,
    }
}

pub fn generate(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    generate_as(s, p, id, &CONTOUR, p)
}

/// The contour of variant `v` from the working parameters `p`, storing `stored` (what the
/// user set) with the toolpath so it can be edited.
pub(super) fn generate_as(s: &Session, p: &Value, id: String, v: &Variant, stored: &Value) -> EngineResult<Toolpath> {
    let (set, machine_id) = settings(s, p, v.kinds)?;
    let (f, wcs) = frame(s, p)?;
    let mut warnings = Vec::new();
    let refs = params::str_list(p, "geometry")?;
    let chains = chains(s, &refs, &f, set.tolerance, &mut warnings)?;
    if chains.is_empty() {
        return Err(bad("the selected geometry has no contour to follow"));
    }
    let mut h = [0.0f64; 5];
    for (slot, (prefix, default_ref, default_offset)) in h.iter_mut().zip(HEIGHTS) {
        *slot = height(s, p, &f, &chains, prefix, default_ref, *default_offset)?;
    }
    let [clearance, retract, feed_height, top, bottom] = h;
    let names = ["Clearance", "Retract", "Feed", "Top", "Bottom"];
    for i in 0..4 {
        if h[i] + 1.0e-9 < h[i + 1] {
            return Err(bad(format!(
                "{} height ({:.3}) must not be below the {} height ({:.3})",
                names[i],
                h[i],
                names[i + 1].to_lowercase(),
                h[i + 1]
            )));
        }
    }
    let final_z = bottom + set.stock_axial;
    if final_z > top + 1.0e-9 {
        return Err(bad("the bottom height (plus axial stock to leave) is above the top height; nothing to cut"));
    }
    let levels: Vec<f64> = if set.multiple_depths && top - final_z > set.max_stepdown + 1.0e-9 {
        let count = ((top - final_z) / set.max_stepdown).ceil().clamp(1.0, 10_000.0) as usize;
        (1..=count).map(|k| top - (top - final_z) * k as f64 / count as f64).collect()
    } else {
        vec![final_z]
    };

    // Offset distance per radial pass, outermost first; spring passes repeat the last one.
    let r = set.tool.diameter / 2.0;
    let sign = match set.side.as_str() {
        "outside" => 1.0,
        "inside" => -1.0,
        _ => 0.0,
    };
    let base = if sign == 0.0 { 0.0 } else { r + set.stock_radial };
    let mut offsets: Vec<f64> = (0..set.radial_passes)
        .map(|i| sign * (base + f64::from(set.radial_passes - 1 - i) * set.radial_stepover))
        .collect();
    if let Some(&last) = offsets.last() {
        offsets.extend(std::iter::repeat_n(last, set.spring_passes as usize));
    }
    // Climb milling keeps the material on the right of the travel direction.
    let material_right = set.climb;
    let entry = entry_point(p, &f)?;

    let mut cuts: Vec<Cut> = Vec::new();
    for (ci, chain) in chains.iter().enumerate() {
        let mut previous_start: Option<DVec2> = None;
        for (li, &z) in levels.iter().enumerate() {
            for (oi, &d) in offsets.iter().enumerate() {
                let paths: Vec<(Vec<DVec2>, bool)> = if chain.closed {
                    offset_closed(&chain.pts, d, set.tolerance).into_iter().map(|l| (l, true)).collect()
                } else {
                    let mut pts = chain.pts.clone();
                    if set.reverse {
                        pts.reverse();
                    }
                    let mut path = offset_open(&pts, d, set.tolerance);
                    // Outside is the left of the chain, inside the right; travel so the
                    // material is on the right for climb, the left for conventional.
                    let tool_left = set.side != "inside";
                    if tool_left != material_right {
                        path.reverse();
                    }
                    vec![(path, false)]
                };
                if paths.is_empty() {
                    return Err(bad(format!(
                        "T{} (Ø{}) does not fit inside the selected contour",
                        set.tool.number, set.tool.diameter
                    )));
                }
                for (mut pts, closed) in paths {
                    if closed {
                        // Offsets come back anticlockwise; climb outside or conventional inside runs clockwise.
                        let ccw = signed_area2(&pts) > 0.0;
                        let want_ccw = match set.side.as_str() {
                            "inside" => material_right,
                            _ => !material_right,
                        };
                        if ccw != want_ccw ^ set.reverse {
                            pts.reverse();
                        }
                        // Start mid-way along the longest side, or nearest the last pass's start.
                        let n = pts.len();
                        let start = match previous_start.or_else(|| entry.map(|e| nearest_on_loop(&mut pts, e))) {
                            Some(prev) => (0..n).min_by(|a, b| {
                                pts.get(*a)
                                    .map_or(f64::MAX, |p| p.distance(prev))
                                    .total_cmp(&pts.get(*b).map_or(f64::MAX, |p| p.distance(prev)))
                            }),
                            None => {
                                let longest = (0..n).max_by(|a, b| {
                                    let len = |i: usize| {
                                        pts.get(i).zip(pts.get((i + 1) % n)).map_or(0.0, |(p, q)| p.distance(*q))
                                    };
                                    len(*a).total_cmp(&len(*b))
                                });
                                if let Some(i) = longest {
                                    let mid = pts.get(i).zip(pts.get((i + 1) % n)).map(|(p, q)| p.lerp(*q, 0.5));
                                    if let Some(mid) = mid {
                                        pts.insert(i + 1, mid);
                                    }
                                    Some(i + 1)
                                } else {
                                    None
                                }
                            }
                        };
                        if let Some(k) = start {
                            let k = k.min(pts.len().saturating_sub(1));
                            pts.rotate_left(k);
                        }
                        previous_start = pts.first().copied();
                    } else if set.both_ways && (li * offsets.len() + oi) % 2 == 1 {
                        pts.reverse();
                    }
                    cuts.push(Cut { pts, closed, z, chain: ci });
                }
            }
        }
    }

    // Moves.
    let first = cuts.first().ok_or_else(|| bad("nothing to cut"))?;
    let mut b = Builder::new(f);
    let start_xy = first.pts.first().copied().unwrap_or_default();
    b.at = start_xy.extend(clearance);
    let start = f.world(b.at);
    let mut exit: Option<(DVec2, f64, usize)> = None;
    for cut in &cuts {
        let (Some(&p0), Some(&p1)) = (cut.pts.first(), cut.pts.get(1)) else { continue };
        let t_in = (p1 - p0).normalize_or_zero();
        let away = |t: DVec2| if material_right { t.perp() } else { -t.perp() };
        let entry = lead(&set.lead_in, p0, t_in, away(t_in), true, set.tolerance);
        let end = if cut.closed { p0 } else { cut.pts.last().copied().unwrap_or(p0) };
        let t_out = if cut.closed {
            (p0 - cut.pts.last().copied().unwrap_or(p0)).normalize_or_zero()
        } else {
            let n = cut.pts.len();
            (end - cut.pts.get(n.saturating_sub(2)).copied().unwrap_or(end)).normalize_or_zero()
        };
        let t_out = if cut.closed { t_in } else { t_out };
        let leave = lead(&set.lead_out, end, t_out, away(t_out), false, set.tolerance);
        let s_pt = entry.first().copied().unwrap_or(p0);

        // Link to the entry point.
        let mut descend_from = feed_height.max(cut.z);
        match exit {
            None => {
                b.push(MoveKind::Rapid, s_pt.extend(clearance), 0.0)?;
                b.push(MoveKind::Rapid, s_pt.extend(retract), 0.0)?;
                b.push(MoveKind::Rapid, s_pt.extend(descend_from), 0.0)?;
            }
            Some((from, z_prev, chain_prev)) => {
                let close = from.distance(s_pt) <= set.stay_down_distance;
                if set.retract == "stay_down" && chain_prev == cut.chain && close {
                    let z_link = z_prev.max(cut.z);
                    let kind = MoveKind::Link;
                    b.push(kind, from.extend(z_link), set.link_feed.unwrap_or(set.lead_feed))?;
                    b.push(kind, s_pt.extend(z_link), set.link_feed.unwrap_or(set.lead_feed))?;
                    descend_from = z_link;
                } else {
                    let up = if set.retract == "full" || chain_prev != cut.chain { clearance } else { retract };
                    b.push(MoveKind::Rapid, from.extend(up), 0.0)?;
                    match set.link_feed {
                        Some(lf) => b.push(MoveKind::Link, s_pt.extend(up), lf)?,
                        None => b.push(MoveKind::Rapid, s_pt.extend(up), 0.0)?,
                    }
                    b.push(MoveKind::Rapid, s_pt.extend(descend_from), 0.0)?;
                }
            }
        }

        // Down to depth: plunge, or ramp along the lead-in where there is room.
        let lead_len: f64 = entry.windows(2).filter_map(|w| Some(w.first()?.distance(*w.get(1)?))).sum();
        let drop = descend_from - cut.z;
        match set.ramp_angle {
            Some(angle) if lead_len > 1.0e-6 && drop > 1.0e-9 => {
                let reach = lead_len * angle.tan();
                let ramp_top = cut.z + drop.min(reach);
                if ramp_top < descend_from {
                    b.push(MoveKind::Plunge, s_pt.extend(ramp_top), set.plunge_feed)?;
                }
                let mut walked = 0.0;
                for w in entry.windows(2) {
                    let (Some(&a), Some(&c)) = (w.first(), w.get(1)) else { continue };
                    walked += a.distance(c);
                    let z = ramp_top - (ramp_top - cut.z) * (walked / lead_len);
                    b.push(MoveKind::Plunge, c.extend(z), set.plunge_feed)?;
                }
            }
            _ => {
                if drop > 1.0e-9 {
                    b.push(MoveKind::Plunge, s_pt.extend(cut.z), set.plunge_feed)?;
                }
                for &q in entry.iter().skip(1) {
                    b.push(MoveKind::Lead, q.extend(cut.z), set.lead_feed)?;
                }
            }
        }
        for (q, feed) in optimized_cut(&cut.pts, cut.closed, set.feed, set.optimize) {
            b.push(MoveKind::Cut, q.extend(cut.z), feed)?;
        }
        for &q in leave.iter().skip(1) {
            b.push(MoveKind::Lead, q.extend(cut.z), set.lead_feed)?;
        }
        exit = Some((leave.last().copied().unwrap_or(end), cut.z, cut.chain));
    }
    if let Some((at, _, _)) = exit {
        b.push(MoveKind::Rapid, at.extend(clearance), 0.0)?;
    }
    // The tool starts over the first entry point, at clearance height.
    let start = b.moves.first().map_or(start, |first| {
        let l = f.local(first.to);
        f.world(DVec3::new(l.x, l.y, clearance))
    });
    if set.tool.spindle_rpm <= 0.0 && p.get("spindle_rpm").and_then(Value::as_f64).unwrap_or(0.0) <= 0.0 {
        warnings.push(format!("T{} has no spindle speed set", set.tool.number));
    }
    warnings.sort();
    warnings.dedup();
    let name = if set.name.is_empty() { format!("{} T{}", v.name, set.tool.number) } else { set.name.clone() };
    let mut stored = stored.as_object().cloned().unwrap_or_else(Map::new);
    stored.insert("tool".into(), json!(set.tool.number));
    stored.remove("id");
    Ok(Toolpath {
        id,
        name,
        strategy: v.strategy.into(),
        machine: machine_id,
        tool: set.tool.number,
        wcs,
        axis: f.world_dir(DVec3::Z),
        axes: None,
        params: Value::Object(stored),
        start,
        moves: Arc::new(b.moves),
        warnings,
        drill: None,
        visible: false,
    })
}

/// Height values for the panel's preview: (prefix, WCS Z) for each height, where resolvable.
pub fn preview_heights(s: &Session, p: &Value) -> Vec<(String, Option<f64>)> {
    let tol = 0.01;
    let mut sink = Vec::new();
    let resolved = frame(s, p).ok().map(|(f, _)| {
        let refs = params::str_list(p, "geometry").unwrap_or_default();
        let chains = chains(s, &refs, &f, tol, &mut sink).unwrap_or_default();
        (f, chains)
    });
    HEIGHTS
        .iter()
        .map(|(prefix, r, o)| {
            let v = resolved.as_ref().and_then(|(f, chains)| height(s, p, f, chains, prefix, r, *o).ok());
            ((*prefix).to_owned(), v)
        })
        .collect()
}
