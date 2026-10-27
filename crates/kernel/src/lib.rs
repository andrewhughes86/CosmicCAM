//! L1 solid modelling kernel for CAD mode: closed sketch profiles become solids by extruding or
//! revolving them, solids combine by booleans, and bodies tessellate to triangles.
//!
//! Built on the `truck` B-rep kernel (Apache-2.0). Every call into it runs under
//! `catch_unwind`: a panic there comes back as a [`KernelError`], never as a crash.

#![allow(non_snake_case)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::panic::{AssertUnwindSafe, catch_unwind};

use glam::{DVec2, DVec3};
use truck_meshalgo::prelude::*;
use truck_modeling::{Point3, Rad, Vector3, builder};

/// Most loops one profile may have.
pub const MAX_LOOPS: usize = 1_000;
/// Most segments one loop may have.
pub const MAX_SEGMENTS: usize = 10_000;
/// Distance below which two points are the same, mm.
const EPS: f64 = 1.0e-7;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelError(pub String);

impl std::fmt::Display for KernelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for KernelError {}

pub type Result<T> = std::result::Result<T, KernelError>;

fn err(m: impl Into<String>) -> KernelError {
    KernelError(m.into())
}

/// Runs `f`, turning a panic in the kernel into an error.
fn guarded<T>(what: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(err(format!("the modelling kernel failed: {what}"))))
}

/// A plane to sketch on: its origin and in-plane X and Y (unit, square to each other); its
/// normal is X × Y.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub origin: DVec3,
    pub x: DVec3,
    pub y: DVec3,
}

impl Plane {
    pub fn new(origin: DVec3, x: DVec3, y: DVec3) -> Option<Self> {
        let x = x.try_normalize()?;
        let y = (y - x * y.dot(x)).try_normalize()?;
        Some(Self { origin, x, y })
    }
    pub fn normal(&self) -> DVec3 {
        self.x.cross(self.y)
    }
    pub fn world(&self, p: DVec2) -> DVec3 {
        self.origin + self.x * p.x + self.y * p.y
    }
}

/// One piece of a sketch loop, in plane coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    Line {
        a: DVec2,
        b: DVec2,
    },
    /// From angle `start` to angle `end` (radians) about `center`, counter-clockwise when
    /// `ccw`.
    Arc {
        center: DVec2,
        radius: f64,
        start: f64,
        end: f64,
        ccw: bool,
    },
    /// A whole circle (a loop by itself).
    Circle {
        center: DVec2,
        radius: f64,
    },
}

impl Segment {
    fn ends(&self) -> (DVec2, DVec2) {
        match *self {
            Segment::Line { a, b } => (a, b),
            Segment::Arc { center, radius, start, end, .. } => (
                center + DVec2::new(start.cos(), start.sin()) * radius,
                center + DVec2::new(end.cos(), end.sin()) * radius,
            ),
            Segment::Circle { center, radius } => {
                let p = center + DVec2::new(radius, 0.0);
                (p, p)
            }
        }
    }

    /// Points along it, a few per arc, for containment tests.
    fn sample(&self, out: &mut Vec<DVec2>) {
        match *self {
            Segment::Line { a, .. } => out.push(a),
            Segment::Arc { center, radius, start, end, ccw } => {
                let sweep = arc_sweep(start, end, ccw);
                for i in 0..16 {
                    let t = start + sweep * f64::from(i) / 16.0;
                    out.push(center + DVec2::new(t.cos(), t.sin()) * radius);
                }
            }
            Segment::Circle { center, radius } => {
                for i in 0..32 {
                    let t = std::f64::consts::TAU * f64::from(i) / 32.0;
                    out.push(center + DVec2::new(t.cos(), t.sin()) * radius);
                }
            }
        }
    }

    /// The same segment walked the other way.
    fn reversed(&self) -> Segment {
        match *self {
            Segment::Line { a, b } => Segment::Line { a: b, b: a },
            Segment::Arc { center, radius, start, end, ccw } => {
                Segment::Arc { center, radius, start: end, end: start, ccw: !ccw }
            }
            other => other,
        }
    }
}

/// The signed angle an arc turns through: positive counter-clockwise.
fn arc_sweep(start: f64, end: f64, ccw: bool) -> f64 {
    if ccw { ccw_sweep(start, end) } else { -ccw_sweep(end, start) }
}

fn ccw_sweep(start: f64, end: f64) -> f64 {
    let mut s = end - start;
    while s <= 0.0 {
        s += std::f64::consts::TAU;
    }
    s.min(std::f64::consts::TAU)
}

fn signed_area(pts: &[DVec2]) -> f64 {
    let n = pts.len();
    (0..n).map(|i| pts[i].perp_dot(pts[(i + 1) % n])).sum::<f64>() / 2.0
}

fn inside(pts: &[DVec2], p: DVec2) -> bool {
    let n = pts.len();
    let mut c = false;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + n - 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            c = !c;
        }
    }
    c
}

/// Closed loops from loose segments: each loop chains segments end to end (turning them where
/// needed). Segments that close no loop are reported.
pub fn loops(segments: &[Segment]) -> Result<Vec<Vec<Segment>>> {
    let tol = 1.0e-6;
    let mut left: Vec<Segment> = Vec::new();
    let mut out: Vec<Vec<Segment>> = Vec::new();
    for s in segments {
        match s {
            Segment::Circle { radius, .. } if *radius > EPS => out.push(vec![*s]),
            Segment::Circle { .. } => return Err(err("a circle needs a radius")),
            other => left.push(*other),
        }
    }
    while let Some(first) = left.pop() {
        let mut chain = vec![(first, false)];
        let (start, mut end) = first.ends();
        let mut guard = 0;
        while start.distance(end) > tol {
            guard += 1;
            if guard > MAX_SEGMENTS {
                return Err(err("a sketch loop has too many segments"));
            }
            let found = left.iter().position(|s| {
                let (a, b) = s.ends();
                a.distance(end) <= tol || b.distance(end) <= tol
            });
            let Some(i) = found else {
                return Err(err("the sketch is not closed: a line or arc ends without meeting another"));
            };
            let s = left.swap_remove(i);
            let (a, b) = s.ends();
            let flip = a.distance(end) > tol;
            end = if flip { a } else { b };
            chain.push((s, flip));
        }
        out.push(chain.into_iter().map(|(s, flip)| if flip { s.reversed() } else { s }).collect());
    }
    if out.is_empty() {
        return Err(err("the sketch has no closed profile"));
    }
    if out.len() > MAX_LOOPS {
        return Err(err("the sketch has too many loops"));
    }
    Ok(out)
}

/// The loop's points in order, for area and containment.
fn polygon(lp: &[Segment]) -> Vec<DVec2> {
    let mut pts = Vec::new();
    for s in lp {
        s.sample(&mut pts);
    }
    pts
}

type TruckWire = truck_modeling::Wire;
type TruckFace = truck_modeling::Face;
type TruckSolid = truck_modeling::Solid;

/// A loop as a truck wire on `plane`, counter-clockwise when `ccw`.
fn wire(plane: &Plane, lp: &[Segment], ccw: bool) -> Result<TruckWire> {
    let to3 = |p: DVec2| {
        let w = plane.world(p);
        Point3::new(w.x, w.y, w.z)
    };
    if let [Segment::Circle { center, radius }] = lp {
        let start = builder::vertex(to3(*center + DVec2::new(*radius, 0.0)));
        let n = plane.normal() * if ccw { 1.0 } else { -1.0 };
        let c = plane.world(*center);
        return Ok(builder::rsweep(
            &start,
            Point3::new(c.x, c.y, c.z),
            Vector3::new(n.x, n.y, n.z),
            Rad(std::f64::consts::TAU),
        ));
    }
    let pts = polygon(lp);
    let reverse = (signed_area(&pts) > 0.0) != ccw;
    let mut segs: Vec<Segment> = lp.to_vec();
    if reverse {
        segs.reverse();
        segs = segs.iter().map(Segment::reversed).collect();
    }
    let starts: Vec<DVec2> = segs.iter().map(|s| s.ends().0).collect();
    let vertices: Vec<_> = starts.iter().map(|p| builder::vertex(to3(*p))).collect();
    let mut edges = Vec::with_capacity(segs.len());
    for (i, s) in segs.iter().enumerate() {
        let (v0, v1) = (&vertices[i], &vertices[(i + 1) % vertices.len()]);
        edges.push(match *s {
            Segment::Line { .. } => builder::line(v0, v1),
            Segment::Arc { center, radius, start, end, ccw } => {
                let mid = start + arc_sweep(start, end, ccw) / 2.0;
                builder::circle_arc(v0, v1, to3(center + DVec2::new(mid.cos(), mid.sin()) * radius))
            }
            Segment::Circle { .. } => return Err(err("a circle cannot be part of a longer loop")),
        });
    }
    Ok(edges.into())
}

/// A solid body: one or more disjoint solids.
#[derive(Clone, Debug, Default)]
pub struct Body {
    solids: Vec<TruckSolid>,
}

/// What a feature does with the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Add the new solid to the body.
    Join,
    /// Take it away from the body.
    Cut,
}

impl Body {
    pub fn is_empty(&self) -> bool {
        self.solids.is_empty()
    }

    /// How many separate solids it has.
    pub fn solid_count(&self) -> usize {
        self.solids.len()
    }

    /// This body and `other` side by side, as separate solids (no boolean).
    pub fn with(&self, other: &Body) -> Body {
        Body { solids: self.solids.iter().chain(&other.solids).cloned().collect() }
    }

    /// The faces of a profile on `plane`: each outer loop with the loops directly inside it as
    /// holes.
    fn faces(plane: &Plane, segments: &[Segment]) -> Result<Vec<TruckFace>> {
        let lps = loops(segments)?;
        let polys: Vec<Vec<DVec2>> = lps.iter().map(|l| polygon(l)).collect();
        // How many other loops each loop lies inside: even depth is an outer boundary.
        let depth: Vec<usize> = (0..lps.len())
            .map(|i| {
                let p = polys[i].first().copied().unwrap_or_default();
                (0..lps.len()).filter(|&j| j != i && inside(&polys[j], p)).count()
            })
            .collect();
        let mut faces = Vec::new();
        for i in (0..lps.len()).filter(|&i| depth[i].is_multiple_of(2)) {
            let mut wires = vec![wire(plane, &lps[i], true)?];
            for j in 0..lps.len() {
                let p = polys[j].first().copied().unwrap_or_default();
                if depth[j] == depth[i] + 1 && inside(&polys[i], p) {
                    wires.push(wire(plane, &lps[j], false)?);
                }
            }
            faces.push(builder::try_attach_plane(&wires).map_err(|e| err(format!("the profile is not flat: {e}")))?);
        }
        Ok(faces)
    }

    /// The profile on `plane` swept along its normal from `from` to `to` (mm, either sign).
    pub fn extrude(plane: &Plane, segments: &[Segment], from: f64, to: f64) -> Result<Body> {
        if !(from.is_finite() && to.is_finite()) || (to - from).abs() < 1.0e-6 {
            return Err(err("the extrusion needs a length"));
        }
        guarded("extrude", || {
            let n = plane.normal();
            let (lo, hi) = (from.min(to), from.max(to));
            let solids = Self::faces(plane, segments)?
                .into_iter()
                .map(|f| {
                    let base = builder::translated(&f, Vector3::new(n.x * lo, n.y * lo, n.z * lo));
                    let d = n * (hi - lo);
                    builder::tsweep(&base, Vector3::new(d.x, d.y, d.z))
                })
                .collect();
            Ok(Body { solids })
        })
    }

    /// The profile on `plane` turned `angle` degrees about the axis through `origin` along `dir`
    /// (plane coordinates).
    pub fn revolve(plane: &Plane, segments: &[Segment], origin: DVec2, dir: DVec2, angle: f64) -> Result<Body> {
        let Some(d) = dir.try_normalize() else { return Err(err("the revolve axis needs a direction")) };
        if !angle.is_finite() || angle.abs() < 1.0e-6 || angle.abs() > 360.0 {
            return Err(err("the revolve angle must be between 0 and 360 degrees"));
        }
        guarded("revolve", || {
            let o = plane.world(origin);
            let axis = (plane.world(origin + d) - o).normalize_or_zero();
            // The profile's faces point along the plane normal; turning against that would
            // leave the solid inside out, so such a face is turned over first.
            let pts: Vec<DVec2> = loops(segments)?.iter().flat_map(|l| polygon(l)).collect();
            let centre = pts.iter().copied().sum::<DVec2>() / pts.len().max(1) as f64;
            let sweep = axis.cross(plane.world(centre) - o);
            let flip = sweep.dot(plane.normal()) * angle.signum() < 0.0;
            let solids = Self::faces(plane, segments)?
                .into_iter()
                .map(|f| {
                    let f = if flip { f.inverse() } else { f };
                    builder::rsweep(
                        &f,
                        Point3::new(o.x, o.y, o.z),
                        Vector3::new(axis.x, axis.y, axis.z),
                        Rad(angle.to_radians()),
                    )
                })
                .collect();
            Ok(Body { solids })
        })
    }

    /// This body with `tool` joined to it or cut from it. Solids the tool's box misses are
    /// kept as they are.
    pub fn combine(&self, tool: &Body, op: Operation, tol: f64) -> Result<Body> {
        guarded("boolean", || {
            let tol = tol.clamp(1.0e-4, 1.0);
            let mut solids = self.solids.clone();
            for t in &tool.solids {
                let tbox = bbox(t);
                let mut next = Vec::new();
                let mut merged: Option<TruckSolid> = match op {
                    Operation::Join => Some(t.clone()),
                    Operation::Cut => None,
                };
                for s in solids.drain(..) {
                    if !overlaps(&bbox(&s), &tbox, tol) {
                        next.push(s);
                        continue;
                    }
                    match op {
                        Operation::Join => {
                            let current = merged.take().unwrap_or_else(|| t.clone());
                            match truck_shapeops::or(&s, &current, tol) {
                                Some(u) => merged = Some(u),
                                None => return Err(err("joining the solids failed")),
                            }
                        }
                        Operation::Cut => {
                            let mut inv = t.clone();
                            inv.not();
                            if let Some(c) = truck_shapeops::and(&s, &inv, tol) {
                                next.push(c);
                            }
                        }
                    }
                }
                next.extend(merged);
                solids = next;
            }
            Ok(Body { solids })
        })
    }

    /// A box around the body (it may be a little loose around curved faces).
    pub fn bounds(&self) -> Option<(DVec3, DVec3)> {
        let boxes: Vec<(DVec3, DVec3)> = self.solids.iter().map(bbox).collect();
        let lo = boxes.iter().map(|b| b.0).reduce(DVec3::min)?;
        let hi = boxes.iter().map(|b| b.1).reduce(DVec3::max)?;
        (lo.is_finite() && hi.is_finite()).then_some((lo, hi))
    }

    /// The enclosed volume, mm³, from a coarse tessellation (good to about a percent).
    pub fn volume(&self) -> Result<f64> {
        Ok(self.triangles(0.1)?.iter().map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0).sum())
    }

    /// The body as triangles within `tol` mm of the exact surfaces, facing outwards.
    pub fn triangles(&self, tol: f64) -> Result<Vec<[DVec3; 3]>> {
        guarded("tessellate", || {
            let tol = tol.clamp(1.0e-3, 10.0);
            let mut out = Vec::new();
            for s in &self.solids {
                let mesh = s.triangulation(tol).to_polygon();
                let pos = mesh.positions();
                for tri in mesh.faces().triangle_iter() {
                    let p = |k: usize| pos.get(tri[k].pos).map(|p| DVec3::new(p.x, p.y, p.z));
                    if let (Some(a), Some(b), Some(c)) = (p(0), p(1), p(2)) {
                        out.push([a, b, c]);
                    }
                }
            }
            Ok(out)
        })
    }
}

fn bbox(s: &TruckSolid) -> (DVec3, DVec3) {
    let mut lo = DVec3::splat(f64::INFINITY);
    let mut hi = DVec3::splat(f64::NEG_INFINITY);
    for shell in s.boundaries() {
        for v in shell.vertex_iter() {
            let p = v.point();
            let p = DVec3::new(p.x, p.y, p.z);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        // Curved faces bulge past their vertices: widen by their curves' control points.
        for e in shell.edge_iter() {
            if let truck_modeling::Curve::NurbsCurve(c) = e.curve() {
                for cp in c.control_points() {
                    let w = cp.to_point();
                    let p = DVec3::new(w.x, w.y, w.z);
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
        }
    }
    (lo, hi)
}

fn overlaps(a: &(DVec3, DVec3), b: &(DVec3, DVec3), tol: f64) -> bool {
    let pad = DVec3::splat(tol.max(1.0e-6) + (a.1 - a.0).max_element().max((b.1 - b.0).max_element()) * 0.01);
    (a.0 - pad).cmple(b.1).all() && (b.0 - pad).cmple(a.1).all()
}

#[cfg(test)]
mod tests;
