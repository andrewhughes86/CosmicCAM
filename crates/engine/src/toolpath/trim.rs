//! Trimming: cutting a finished toolpath back to a boundary seen along the tool axis (a box, a
//! polygon, a circle, the selected faces or closed edges, the stock or the model) and/or a band
//! of heights, keeping what lies inside or outside, then linking the pieces that are left.
//!
//! A trim is kept with its toolpath, as the settings `toolpath.trim` took, and applied after
//! the strategy every time the toolpath is worked out: regenerating keeps the trims, and
//! removing one gives back what it took away. The boundary can be offset, and can hold the
//! tool's centre, its whole cutter, or let the cutter reach over by its radius. Pieces are
//! linked by retracting to the toolpath's retract (or clearance) height and plunging back,
//! or, where the gap is short and runs inside what is kept, by feeding straight across.

use std::sync::Arc;

use CosmicCAM_geom::Vec3;
use CosmicCAM_geom::region::{self, Shape};
use glam::{DVec2, DVec3};
use serde_json::{Map, Value};

use super::common::{bad, footprint, selected_shapes};
use super::contour::Frame;
use super::{DrillCycle, MAX_MOVES, Move, MoveKind, Toolpath};
use crate::error::EngineResult;
use crate::params;
use crate::session::Session;

/// What a trim cuts to.
pub const BOUNDARIES: &[&str] = &["box", "polygon", "circle", "selection", "stock", "model", "none"];
/// Which side of the boundary stays.
pub const KEEPS: &[&str] = &["inside", "outside"];
/// What the boundary holds: the tool's centre, its whole cutter, or the cutter reaching over.
pub const CONTAINMENTS: &[&str] = &["center", "tool_within", "tool_over"];
/// How the pieces left are joined.
pub const LINKS: &[&str] = &["auto", "retract", "clearance", "feed"];
/// Most trims one toolpath holds.
pub const MAX_TRIMS: usize = 32;
/// Most points one polygon boundary has.
pub const MAX_POINTS: usize = 10_000;
/// The settings a trim keeps: what `toolpath.trim` takes, less the toolpaths it applies to.
pub const KEYS: &[&str] = &[
    "boundary",
    "points",
    "corners",
    "center",
    "radius",
    "geometry",
    "keep",
    "containment",
    "offset",
    "top",
    "bottom",
    "link",
    "min_cut",
];

/// Most points a circle boundary is drawn with.
const MAX_CIRCLE_POINTS: usize = 720;
/// How closely boundaries follow curves, mm.
const TOLERANCE: f64 = 0.01;
/// Pieces cutting less than this (mm) are dropped unless `min_cut` says otherwise.
const DEFAULT_MIN_CUT: f64 = 0.01;
/// How near the boundary (mm) a point counts as on it.
const ON_BOUNDARY: f64 = 1.0e-3;
/// An auto link feeds straight across gaps up to this many tool diameters.
const AUTO_FEED_DIAMETERS: f64 = 2.0;

/// A toolpath as its strategy made it, before trimming.
#[derive(Clone, Debug, PartialEq)]
pub struct Untrimmed {
    pub start: Vec3,
    pub moves: Arc<Vec<Move>>,
    pub axes: Option<Arc<Vec<Vec3>>>,
    pub drill: Option<DrillCycle>,
    pub warnings: Vec<String>,
}

impl Untrimmed {
    pub fn of(tp: &Toolpath) -> Self {
        Self {
            start: tp.start,
            moves: Arc::clone(&tp.moves),
            axes: tp.axes.clone(),
            drill: tp.drill.clone(),
            warnings: tp.warnings.clone(),
        }
    }
}

/// The trim settings in `p` (the keys `toolpath.trim` keeps), checked.
pub fn settings(p: &Value) -> EngineResult<Value> {
    let mut t = Map::new();
    if let Some(o) = p.as_object() {
        for key in KEYS {
            if let Some(v) = o.get(*key).filter(|v| !v.is_null()) {
                t.insert((*key).to_owned(), v.clone());
            }
        }
    }
    let t = Value::Object(t);
    let kind = boundary_kind(&t)?;
    if kind == "none" && t.get("top").is_none() && t.get("bottom").is_none() {
        return Err(bad("give a boundary to trim to, or a `top` or `bottom` height"));
    }
    Ok(t)
}

/// The boundary a trim uses: as given, else what its settings carry.
fn boundary_kind(t: &Value) -> EngineResult<&str> {
    if let Some(b) = params::opt_str(t, "boundary")? {
        return if BOUNDARIES.contains(&b) { Ok(b) } else { Err(bad(format!("unknown trim boundary `{b}`"))) };
    }
    Ok(if t.get("points").is_some() {
        "polygon"
    } else if t.get("corners").is_some() {
        "box"
    } else if t.get("center").is_some() {
        "circle"
    } else if t.get("geometry").is_some() {
        "selection"
    } else {
        "none"
    })
}

fn choice<'a>(t: &'a Value, key: &str, of: &[&str], default: &'a str) -> EngineResult<&'a str> {
    let v = params::opt_str(t, key)?.unwrap_or(default);
    if of.contains(&v) { Ok(v) } else { Err(bad(format!("`{key}` must be one of: {}", of.join(", ")))) }
}

/// A trim as people read it, e.g. "Keep inside a box".
pub fn describe(t: &Value) -> String {
    let keep = params::opt_str(t, "keep").ok().flatten().unwrap_or("inside");
    let what = match boundary_kind(t).unwrap_or("none") {
        "box" => "a box".to_owned(),
        "polygon" => format!("a polygon ({} points)", t.get("points").and_then(Value::as_array).map_or(0, Vec::len)),
        "circle" => "a circle".to_owned(),
        "selection" => {
            let n = t.get("geometry").and_then(Value::as_array).map_or(0, Vec::len);
            format!("{n} selected {}", if n == 1 { "item" } else { "items" })
        }
        "stock" => "the stock".to_owned(),
        "model" => "the model".to_owned(),
        _ => String::new(),
    };
    let num = |k: &str| t.get(k).and_then(Value::as_f64);
    let band = match (num("top"), num("bottom")) {
        (Some(top), Some(bottom)) => format!("Z {bottom:.3} to {top:.3}"),
        (Some(top), None) => format!("below Z {top:.3}"),
        (None, Some(bottom)) => format!("above Z {bottom:.3}"),
        (None, None) => String::new(),
    };
    let area = match (what.is_empty(), band.is_empty()) {
        (false, false) => format!("{what}, {band}"),
        (false, true) => what,
        _ => band,
    };
    format!("Keep {keep} {area}")
}

/// A trim worked out for one toolpath, in its frame.
struct Resolved {
    region: Option<Vec<Shape>>,
    /// The region's bounding rectangle, to skip moves far from it.
    region_box: (DVec2, DVec2),
    band: (f64, f64),
    inside: bool,
    link: String,
    min_cut: f64,
    radius: f64,
}

impl Resolved {
    fn in_area(&self, p: DVec3) -> bool {
        self.region.as_ref().is_none_or(|r| region::contains(r, p.truncate()))
            && p.z >= self.band.0
            && p.z <= self.band.1
    }

    fn keeps(&self, p: DVec3) -> bool {
        self.in_area(p) == self.inside
    }

    /// Whether `p` lies on the boundary (within `ON_BOUNDARY`), where the trim stops moves.
    fn on_boundary(&self, p: DVec3) -> bool {
        let near = |h: f64| (p.z - h).abs() < ON_BOUNDARY;
        if near(self.band.0) || near(self.band.1) {
            return true;
        }
        let q = p.truncate();
        self.region.iter().flatten().flatten().any(|contour| {
            let n = contour.len();
            (0..n).any(|i| match (contour.get(i), contour.get((i + 1) % n)) {
                (Some(&a), Some(&b)) => {
                    let ab = b - a;
                    let t = ((q - a).dot(ab) / ab.length_squared().max(1.0e-18)).clamp(0.0, 1.0);
                    q.distance(a + ab * t) < ON_BOUNDARY
                }
                _ => false,
            })
        })
    }

    /// Whether the straight move from `a` to `b` stays on the kept side (or on the boundary).
    fn keeps_segment(&self, a: DVec3, b: DVec3) -> bool {
        let n = (a.distance(b) / (self.radius * 0.25).max(0.05)).ceil().clamp(1.0, 2000.0) as usize;
        (0..=n).all(|i| {
            let p = a.lerp(b, i as f64 / n as f64);
            self.keeps(p) || self.on_boundary(p)
        })
    }

    /// The parts of the move from `a` to `b` that stay, as (from, to) fractions along it.
    fn spans(&self, a: DVec3, b: DVec3) -> Vec<(f64, f64)> {
        let mut ts = vec![0.0, 1.0];
        if let Some(shapes) = &self.region {
            let (a2, b2) = (a.truncate(), b.truncate());
            let (lo, hi) = self.region_box;
            let near = a2.min(b2).cmple(hi).all() && a2.max(b2).cmpge(lo).all();
            if near && a2.distance_squared(b2) > 1.0e-18 {
                for contour in shapes.iter().flatten() {
                    let n = contour.len();
                    for i in 0..n {
                        let (Some(&p), Some(&q)) = (contour.get(i), contour.get((i + 1) % n)) else { continue };
                        if let Some(t) = crossing(a2, b2, p, q) {
                            ts.push(t);
                        }
                    }
                }
            }
        }
        for h in [self.band.0, self.band.1] {
            if h.is_finite() && (a.z - h) * (b.z - h) < 0.0 {
                ts.push((h - a.z) / (b.z - a.z));
            }
        }
        ts.sort_by(f64::total_cmp);
        ts.dedup_by(|x, y| (*x - *y).abs() < 1.0e-9);
        let mut spans: Vec<(f64, f64)> = Vec::new();
        for w in ts.windows(2) {
            let [t0, t1] = [w[0], w[1]];
            if t1 - t0 < 1.0e-12 || !self.keeps(a.lerp(b, (t0 + t1) / 2.0)) {
                continue;
            }
            match spans.last_mut() {
                Some(last) if (last.1 - t0).abs() < 1.0e-12 => last.1 = t1,
                _ => spans.push((t0, t1)),
            }
        }
        spans
    }
}

/// Where segment `a`–`b` crosses segment `p`–`q`, as a fraction along `a`–`b`.
fn crossing(a: DVec2, b: DVec2, p: DVec2, q: DVec2) -> Option<f64> {
    let (r, s) = (b - a, q - p);
    let denom = r.perp_dot(s);
    if denom.abs() < 1.0e-15 {
        return None;
    }
    let t = (p - a).perp_dot(s) / denom;
    let u = (p - a).perp_dot(r) / denom;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some(t)
}

fn local(f: &Frame, p: DVec3) -> DVec3 {
    let d = p - f.origin;
    DVec3::new(d.dot(f.x), d.dot(f.y), d.dot(f.z))
}

fn resolve(s: &Session, tp: &Toolpath, f: &Frame, t: &Value) -> EngineResult<Resolved> {
    let kind = boundary_kind(t)?;
    let inside = choice(t, "keep", KEEPS, "inside")? == "inside";
    let containment = choice(t, "containment", CONTAINMENTS, "center")?;
    let link = choice(t, "link", LINKS, "auto")?.to_owned();
    let radius = s.library.machine(&tp.machine).and_then(|m| m.tool(tp.tool)).map(|tool| tool.diameter / 2.0);
    if containment != "center" && radius.is_none() {
        return Err(bad(format!("T{} is not in the machine's tool library; trim to the tool centre", tp.tool)));
    }
    let radius = radius.unwrap_or(0.0);
    let flat = |p: DVec3| local(f, p).truncate();
    let shapes: Option<Vec<Shape>> = match kind {
        "polygon" => {
            let pts = params::point_list(t, "points", MAX_POINTS)?;
            if pts.len() < 3 {
                return Err(bad("a polygon boundary needs at least three `points`"));
            }
            let contour: Vec<DVec2> = pts.into_iter().map(flat).collect();
            Some(region::union_polygons(&[contour]))
        }
        "box" => {
            let corners = params::point_list(t, "corners", 2)?;
            let [a, b] = corners.as_slice() else {
                return Err(bad("a box boundary needs two opposite `corners`"));
            };
            let (a, b) = (flat(*a), flat(*b));
            Some(vec![region::rectangle(a.min(b), a.max(b))])
        }
        "circle" => {
            let center = params::opt_vec3(t, "center")?.ok_or_else(|| bad("a circle boundary needs its `center`"))?;
            let r =
                params::opt_f64(t, "radius", 0.0, 1.0e5)?.ok_or_else(|| bad("a circle boundary needs a `radius`"))?;
            let c = flat(center.as_dvec3());
            let step = 2.0 * (1.0 - (TOLERANCE / r.max(1.0e-6)).min(1.0)).acos();
            let n = if step > 1.0e-9 { (std::f64::consts::TAU / step).ceil() as usize } else { MAX_CIRCLE_POINTS };
            let n = n.clamp(16, MAX_CIRCLE_POINTS);
            let ring = (0..n)
                .map(|i| {
                    let a = std::f64::consts::TAU * i as f64 / n as f64;
                    c + DVec2::new(a.cos(), a.sin()) * r
                })
                .collect();
            Some(vec![vec![ring]])
        }
        "selection" => {
            let mut warnings = Vec::new();
            let (shapes, _) = selected_shapes(s, t, f, TOLERANCE, &mut warnings)?;
            if shapes.is_empty() {
                return Err(bad("select the faces or closed edges to trim to (`geometry`)"));
            }
            Some(shapes)
        }
        "stock" => {
            let stock = s.job.stock.ok_or_else(|| bad("there is no stock to trim to"))?;
            let (lo, hi) = footprint(f, stock).ok_or_else(|| bad("the stock is empty"))?;
            Some(vec![region::rectangle(lo, hi)])
        }
        "model" => {
            let (lo, hi) = footprint(f, s.job.part_bounds()).ok_or_else(|| bad("there is no model to trim to"))?;
            Some(vec![region::rectangle(lo, hi)])
        }
        _ => None,
    };
    let offset = params::opt_f64(t, "offset", -1.0e4, 1.0e4)?.unwrap_or(0.0);
    let grow = offset
        + match (containment, inside) {
            ("tool_within", true) | ("tool_over", false) => -radius,
            ("tool_within", false) | ("tool_over", true) => radius,
            _ => 0.0,
        };
    let region = shapes.map(|sh| if grow.abs() > 1.0e-9 { region::offset(&sh, grow, TOLERANCE) } else { sh });
    if region.as_ref().is_some_and(Vec::is_empty) && inside {
        return Err(bad("the boundary leaves no room for the tool; make it larger or trim to the tool centre"));
    }
    let mut region_box = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for p in region.iter().flatten().flatten().flatten() {
        region_box = (region_box.0.min(*p), region_box.1.max(*p));
    }
    let top = params::opt_f64(t, "top", -1.0e5, 1.0e5)?;
    let bottom = params::opt_f64(t, "bottom", -1.0e5, 1.0e5)?;
    let band = (bottom.unwrap_or(f64::NEG_INFINITY), top.unwrap_or(f64::INFINITY));
    if band.0 > band.1 {
        return Err(bad("the trim's `bottom` is above its `top`"));
    }
    if tp.drill.is_some() && (top.is_some() || bottom.is_some()) {
        return Err(bad("a drilling toolpath is trimmed by a boundary only; leave out `top` and `bottom`"));
    }
    if region.is_none() && top.is_none() && bottom.is_none() {
        return Err(bad("give a boundary to trim to, or a `top` or `bottom` height"));
    }
    Ok(Resolved {
        region,
        region_box,
        band,
        inside,
        link,
        min_cut: params::opt_f64(t, "min_cut", 0.0, 1.0e4)?.unwrap_or(DEFAULT_MIN_CUT),
        radius,
    })
}

/// A run of moves that stays, from its first point.
struct Piece {
    start: DVec3,
    start_axis: Vec3,
    moves: Vec<(MoveKind, DVec3, f32, Vec3)>,
}

impl Piece {
    fn cut_length(&self) -> f64 {
        let mut at = self.start;
        let mut cut = 0.0;
        for (kind, to, _, _) in &self.moves {
            if matches!(kind, MoveKind::Cut | MoveKind::Lead | MoveKind::Plunge) {
                cut += at.distance(*to);
            }
            at = *to;
        }
        cut
    }
}

/// Applies one trim to the toolpath's current moves.
fn apply(s: &Session, tp: &mut Toolpath, t: &Value) -> EngineResult<()> {
    let (f, _) = super::contour::frame(s, &tp.params)?;
    let rs = resolve(s, tp, &f, t)?;
    let to_local = |p: Vec3| local(&f, p.as_dvec3());

    // The runs of moves on the kept side.
    let mut pieces: Vec<Piece> = Vec::new();
    let mut open: Option<Piece> = None;
    let mut a = to_local(tp.start);
    let mut a_axis = tp.axis_at(0);
    for (i, m) in tp.moves.iter().enumerate() {
        let b = to_local(m.to);
        let b_axis = tp.axis_at(i);
        let spans = rs.spans(a, b);
        if spans.is_empty() {
            pieces.extend(open.take());
        }
        for (t0, t1) in spans {
            let axis_at = |t: f64| a_axis.lerp(b_axis, t as f32).try_normalize().unwrap_or(b_axis);
            let (p0, p1) = (a.lerp(b, t0), a.lerp(b, t1));
            if open.is_none() || t0 > 1.0e-9 {
                pieces.extend(open.take());
                open = Some(Piece { start: p0, start_axis: axis_at(t0), moves: Vec::new() });
            }
            if let Some(piece) = &mut open
                && p0.distance(p1) > 1.0e-7
            {
                piece.moves.push((m.kind, p1, m.feed, axis_at(t1)));
            }
            if t1 < 1.0 - 1.0e-9 {
                pieces.extend(open.take());
            }
        }
        a = b;
        a_axis = b_axis;
    }
    pieces.extend(open.take());
    // Travel into and out of a piece is the links' job: drop its rapids and links at either end.
    let travel = |m: &(MoveKind, DVec3, f32, Vec3)| matches!(m.0, MoveKind::Rapid | MoveKind::Link);
    for piece in &mut pieces {
        let lead = piece.moves.iter().take_while(|m| travel(m)).count();
        if let Some(last) = lead.checked_sub(1).and_then(|i| piece.moves.get(i)) {
            piece.start = last.1;
            piece.start_axis = last.3;
        }
        piece.moves.drain(..lead);
        while piece.moves.last().is_some_and(travel) {
            piece.moves.pop();
        }
    }
    pieces.retain(|p| p.cut_length() > rs.min_cut);
    let Some(first) = pieces.first() else {
        return Err(bad(format!("the trim leaves nothing of {}", tp.name)));
    };

    // Heights to link at: the toolpath's clearance (where it starts) and retract heights.
    let clearance = to_local(tp.start).z;
    let highest_cut = tp
        .moves
        .iter()
        .filter(|m| m.kind != MoveKind::Rapid)
        .map(|m| to_local(m.to).z)
        .fold(f64::NEG_INFINITY, f64::max);
    let retract_setting = super::common::height(s, &tp.params, &f, None, "retract", ("model_top", 5.0)).ok();
    let retract = retract_setting.unwrap_or(f64::NEG_INFINITY).max(highest_cut + 1.0).min(clearance);
    let plunge_feed = params::opt_f64(&tp.params, "plunge_feed", 0.0, 1.0e6)
        .ok()
        .flatten()
        .filter(|f| *f > 0.0)
        .map(|f| f as f32)
        .or_else(|| tp.moves.iter().find(|m| m.kind == MoveKind::Plunge).map(|m| m.feed))
        .or_else(|| tp.moves.iter().map(|m| m.feed).reduce(f32::max).map(|f| f * 0.5))
        .unwrap_or(100.0)
        .max(1.0);

    // Join the pieces.
    let mut out: Vec<(MoveKind, DVec3, f32, Vec3)> = Vec::new();
    let start = first.start.truncate().extend(clearance.max(first.start.z));
    let mut at = start;
    let mut at_axis = first.start_axis;
    let mut feed = tp.moves.iter().find(|m| m.kind == MoveKind::Cut).map_or(plunge_feed, |m| m.feed);
    let descend = |out: &mut Vec<_>, from_height: f64, to: DVec3, axis: Vec3| {
        if to.z < from_height - 1.0e-6 {
            let fast = retract.max(to.z);
            if fast < from_height - 1.0e-6 {
                out.push((MoveKind::Rapid, to.truncate().extend(fast), 0.0, axis));
            }
            if to.z < fast - 1.0e-6 {
                out.push((MoveKind::Plunge, to, plunge_feed, axis));
            }
        }
    };
    for (k, piece) in pieces.iter().enumerate() {
        let to = piece.start;
        if k == 0 {
            descend(&mut out, at.z, to, piece.start_axis);
        } else if at.distance(to) > 1.0e-6 {
            let direct = match rs.link.as_str() {
                "feed" => true,
                "auto" => {
                    (at.z - to.z).abs() < 1.0e-3
                        && at.truncate().distance(to.truncate()) <= AUTO_FEED_DIAMETERS * 2.0 * rs.radius
                        && rs.keeps_segment(at, to)
                }
                _ => false,
            };
            if direct {
                out.push((MoveKind::Cut, to, feed, piece.start_axis));
            } else {
                let base = if rs.link == "clearance" { clearance } else { retract };
                let h = base.max(at.z).max(to.z);
                if at.z < h - 1.0e-6 {
                    out.push((MoveKind::Rapid, at.truncate().extend(h), 0.0, at_axis));
                }
                if at.truncate().distance(to.truncate()) > 1.0e-6 {
                    out.push((MoveKind::Rapid, to.truncate().extend(h), 0.0, piece.start_axis));
                }
                descend(&mut out, h, to, piece.start_axis);
            }
        }
        for mv in &piece.moves {
            out.push(*mv);
            if mv.0 == MoveKind::Cut {
                feed = mv.2;
            }
        }
        at = piece.moves.last().map_or(to, |m| m.1);
        at_axis = piece.moves.last().map_or(piece.start_axis, |m| m.3);
    }
    if at.z < clearance - 1.0e-6 {
        out.push((MoveKind::Rapid, at.truncate().extend(clearance), 0.0, at_axis));
    }
    if out.len() > MAX_MOVES {
        return Err(bad(format!("the trimmed toolpath has more than {MAX_MOVES} moves")));
    }

    if let Some(drill) = &mut tp.drill {
        drill.holes.retain(|h| rs.keeps(DVec3::new(h[0], h[1], h[2])));
        if drill.holes.is_empty() {
            return Err(bad(format!("the trim leaves no holes of {}", tp.name)));
        }
    }
    let world = |p: DVec3| f.world(p);
    tp.start = world(start);
    if tp.axes.is_some() {
        tp.axes = Some(Arc::new(out.iter().map(|m| m.3).collect()));
    }
    tp.moves = Arc::new(out.iter().map(|&(kind, to, feed, _)| Move { kind, to: world(to), feed }).collect());
    Ok(())
}

/// Puts `base` back and applies the toolpath's trims to it in order. A trim that fails stops
/// with its error when `strict`; otherwise it is skipped with a warning (a regenerated
/// toolpath keeps its trims even when the model changed under one).
pub fn apply_all(s: &Session, tp: &mut Toolpath, base: Arc<Untrimmed>, strict: bool) -> EngineResult<()> {
    tp.start = base.start;
    tp.moves = Arc::clone(&base.moves);
    tp.axes = base.axes.clone();
    tp.drill = base.drill.clone();
    tp.warnings = base.warnings.clone();
    let trims = tp.trims.clone();
    for (i, t) in trims.iter().enumerate() {
        if let Err(e) = apply(s, tp, t) {
            if strict {
                return Err(e);
            }
            tp.warnings.push(format!("trim {} was not applied: {e}", i + 1));
        }
    }
    tp.untrimmed = (!tp.trims.is_empty()).then_some(base);
    Ok(())
}
