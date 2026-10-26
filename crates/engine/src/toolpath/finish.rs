//! 3D finishing strategies beyond waterline and raster:
//!
//! - **Scallop**: passes a constant stepover apart measured over the surface, from the area's
//!   edge inwards, so steep and shallow areas get the same finish.
//! - **Spiral** and **Radial**: passes spiralling out from, or radiating from, a centre.
//! - **Flat**: the model's flat floors cleared at their exact heights.
//! - **Pencil**: one pass along the concave corners, where a ball touches two faces at once.
//! - **Steep and shallow**: level passes on steep walls and scallop passes on the shallow
//!   areas, split at a slope angle.
//!
//! All but Flat drop the tool onto the model (as Raster does), so they never gouge it; the
//! scallop, steep-and-shallow and pencil passes are found on a grid of tool-tip heights.

use std::sync::Arc;

use CosmicCAM_geom::FaceKind;
use CosmicCAM_geom::adaptive::{self as grid_tools, Grid};
use CosmicCAM_geom::cutter::DropCutter;
use CosmicCAM_geom::faces::boundary_loops;
use CosmicCAM_geom::field::{Lattice, iso_lines, surface_distance};
use CosmicCAM_geom::region::{self, Shape};
use glam::{DVec2, DVec3};
use serde_json::Value;

use super::common::{
    Common, Emitter, MAX_PASSES, Pass, bad, common_for, cutting_contours, heights, order_loops, part_triangles,
};
use super::strategies::{FINISH_HEIGHTS, boundary, drop_line, require_model, simplify};
use super::{MoveKind, Toolpath};
use crate::error::EngineResult;
use crate::params;
use crate::session::Session;

pub const SCALLOP: &str = "scallop";
pub const SPIRAL: &str = "spiral";
pub const RADIAL: &str = "radial";
pub const FLAT: &str = "flat";
pub const PENCIL: &str = "pencil";
pub const STEEP_SHALLOW: &str = "steep_shallow";

/// Tools the surface-following strategies take.
pub const SURFACE_TOOLS: &[&str] = &["ball", "bull", "flat", "tapered_ball", "lollipop"];
/// Most samples in a height grid.
const MAX_SAMPLES: usize = 1_500_000;

/// The tool dropped onto the model.
struct Drop {
    cutter: DropCutter,
    floor: f64,
    raise: f64,
    step: f64,
    tolerance: f64,
    /// Longest link along the surface between passes; farther, the tool lifts.
    stay_down: f64,
}

impl Drop {
    fn new(c: &Common, tris: Vec<[DVec3; 3]>, floor: f64, p: &Value) -> EngineResult<Self> {
        let r = c.radius();
        let cutter = if c.tool.ball_ended() { DropCutter::ball(tris, r) } else { DropCutter::new(tris, r) };
        let step =
            params::opt_f64(p, "sample_step", 0.0, 1.0e3)?.filter(|v| *v > 0.0).unwrap_or((r * 0.25).clamp(0.05, 1.0));
        let stay_down = params::opt_f64(p, "stay_down_distance", 0.0, 1.0e5)?.unwrap_or(c.tool.diameter * 2.0);
        Ok(Self { cutter, floor, raise: c.stock_axial, step, tolerance: c.tolerance, stay_down })
    }

    fn z(&self, q: DVec2) -> f64 {
        self.cutter.height(q).map_or(self.floor, |z| (z + self.raise).max(self.floor))
    }

    /// A path through `pts` (closing it when `closed`) with every point dropped onto the model.
    fn path(&self, pts: &[DVec2], closed: bool) -> Vec<DVec3> {
        let mut out: Vec<DVec3> = Vec::new();
        let n = pts.len();
        let segs = if closed { n } else { n.saturating_sub(1) };
        for k in 0..segs {
            let (Some(&a), Some(&b)) = (pts.get(k), pts.get((k + 1) % n)) else { continue };
            let mut line = drop_line(&self.cutter, a, b, self.step, self.floor, self.raise, self.tolerance * 0.5);
            if !out.is_empty() && !line.is_empty() {
                line.remove(0);
            }
            out.extend(line);
        }
        simplify(&out, self.tolerance)
    }
}

/// Tool-tip heights on a grid over `area` (NaN outside it), worked out on every core.
fn height_grid(d: &Drop, area: &[Shape], res: f64) -> EngineResult<(Lattice, Vec<f64>, Vec<bool>)> {
    let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for q in area.iter().flatten().flatten() {
        lo = lo.min(*q);
        hi = hi.max(*q);
    }
    if !(lo.x < hi.x && lo.y < hi.y) {
        return Err(bad("there is no area to finish"));
    }
    let size = hi - lo;
    let res = res.max((size.x * size.y / MAX_SAMPLES as f64).sqrt());
    let l = Lattice { origin: lo, res, nx: (size.x / res).ceil() as usize + 1, ny: (size.y / res).ceil() as usize + 1 };
    // Sample points are the centres of a grid shifted back by half a cell.
    let g = Grid { origin: lo - DVec2::splat(res / 2.0), res, nx: l.nx, ny: l.ny };
    let inside = grid_tools::raster(&g, area);
    let rows: Vec<usize> = (0..l.ny).collect();
    let row = |j: usize| -> Vec<f64> {
        (0..l.nx)
            .map(|i| if inside.get(j * l.nx + i).copied().unwrap_or(false) { d.z(l.at(i, j)) } else { f64::NAN })
            .collect()
    };
    let z: Vec<f64> = {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 16);
            let chunk = rows.len().div_ceil(threads).max(1);
            std::thread::scope(|scope| {
                let parts: Vec<_> = rows
                    .chunks(chunk)
                    .map(|rs| scope.spawn(move || rs.iter().flat_map(|&j| row(j)).collect::<Vec<f64>>()))
                    .collect();
                parts.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            rows.iter().flat_map(|&j| row(j)).collect()
        }
    };
    if z.len() != l.len() {
        return Err(bad("the height grid could not be worked out"));
    }
    Ok((l, z, inside))
}

/// Samples inside the area next to one outside it (or on the grid's edge).
fn border(l: &Lattice, inside: &[bool]) -> Vec<bool> {
    (0..l.len())
        .map(|k| {
            let (i, j) = (k % l.nx, k / l.nx);
            inside.get(k).copied().unwrap_or(false)
                && (i == 0
                    || j == 0
                    || i + 1 == l.nx
                    || j + 1 == l.ny
                    || [k - 1, k + 1, k - l.nx, k + l.nx].iter().any(|&m| !inside.get(m).copied().unwrap_or(false)))
        })
        .collect()
}

/// Paths at every `step` of distance from the area's edge over the surface, outside in.
fn distance_passes(d: &Drop, l: &Lattice, z: &[f64], inside: &[bool], step: f64) -> Vec<Vec<DVec3>> {
    let seeds = border(l, inside);
    let dist = surface_distance(l, z, &seeds, inside);
    let far = dist.iter().copied().filter(|v| v.is_finite()).fold(0.0, f64::max);
    let mut passes = Vec::new();
    let mut k = 0usize;
    while k < MAX_PASSES {
        let level = step * k as f64 + step * 0.05;
        if level > far {
            break;
        }
        for (pts, closed) in iso_lines(l, &dist, level) {
            let path = d.path(&pts, closed);
            if path.len() >= 2 {
                passes.push(path);
            }
        }
        k += 1;
    }
    passes
}

/// Emits finishing passes: consecutive ones joined along the surface when they are close,
/// lifting between them otherwise. Closed passes start where they are nearest the tool.
fn emit(e: &mut Emitter, c: &Common, d: &Drop, passes: Vec<Vec<DVec3>>, join: f64) -> EngineResult<()> {
    // Links follow the surface like the passes, so they are safe to stay down longer.
    let join = join.max(d.stay_down);
    let mut left = passes;
    let mut at: Option<DVec3> = None;
    while !left.is_empty() {
        // The next pass is the nearest start, rotating closed passes to their nearest point.
        let here = at.map_or(DVec2::ZERO, |a| a.truncate());
        let mut best = (0usize, 0usize, false, f64::INFINITY);
        for (i, p) in left.iter().enumerate() {
            let closed = p.len() > 2
                && p.first().zip(p.last()).is_some_and(|(a, b)| a.truncate().distance(b.truncate()) < 1.0e-6);
            if closed {
                for (k, q) in p.iter().enumerate() {
                    let dd = q.truncate().distance(here);
                    if dd < best.3 {
                        best = (i, k, false, dd);
                    }
                }
            } else {
                for (rev, q) in [(false, p.first()), (true, p.last())] {
                    if let Some(q) = q {
                        let dd = q.truncate().distance(here);
                        if dd < best.3 {
                            best = (i, 0, rev, dd);
                        }
                    }
                }
            }
            // The first few passes in order is good enough to keep the search cheap.
            if i > 64 && best.3 < join {
                break;
            }
        }
        let mut pass = left.swap_remove(best.0);
        if best.2 {
            pass.reverse();
        } else if best.1 > 0 {
            pass.pop();
            pass.rotate_left(best.1);
            if let Some(&f) = pass.first() {
                pass.push(f);
            }
        }
        let (Some(&first), Some(&last)) = (pass.first(), pass.last()) else { continue };
        match at {
            Some(prev) if prev.truncate().distance(first.truncate()) <= join => {
                for q in d.path(&[prev.truncate(), first.truncate()], false).into_iter().skip(1) {
                    e.b.push(MoveKind::Link, q, c.feed)?;
                }
                for q in pass.iter().skip(1) {
                    e.b.push(MoveKind::Cut, *q, c.feed)?;
                }
            }
            _ => {
                e.pass(c, &Pass { pts: pass.clone(), closed: false, clear_above: first.z, ramp: false, safe: None })?
            }
        }
        at = Some(last);
    }
    Ok(())
}

/// Shared setup of the surface finishing strategies.
struct Setup {
    c: Common,
    d: Drop,
    area: Vec<Shape>,
    e: Emitter,
    warnings: Vec<String>,
}

fn setup(s: &Session, p: &Value, kinds: &[&str], stepover_of_d: f64) -> EngineResult<Setup> {
    let c = common_for(s, p, stepover_of_d, 0.1, kinds)?;
    let mut warnings = Vec::new();
    let tris = part_triangles(s, &c.frame);
    require_model(&tris)?;
    let (area, selection) = boundary(s, p, &c, "model", &mut warnings)?;
    let [clearance, retract, feed_height, top, bottom] = heights(s, p, &c.frame, selection, &FINISH_HEIGHTS)?;
    let d = Drop::new(&c, tris, bottom, p)?;
    let e = Emitter::new(c.frame, clearance, retract, feed_height - top);
    Ok(Setup { c, d, area, e, warnings })
}

pub fn scallop(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let Setup { c, d, area, mut e, warnings } = setup(s, p, SURFACE_TOOLS, 0.1)?;
    let (l, z, inside) = height_grid(&d, &area, c.stepover / 2.5)?;
    let passes = distance_passes(&d, &l, &z, &inside, c.stepover);
    emit(&mut e, &c, &d, passes, c.stepover * 2.5)?;
    e.finish(p, c, id, SCALLOP, "Scallop", warnings)
}

/// The spiral or radial centre: given, else the middle of the area.
fn centre(p: &Value, area: &[Shape]) -> EngineResult<DVec2> {
    if let (Some(x), Some(y)) =
        (params::opt_f64(p, "center_x", -1.0e6, 1.0e6)?, params::opt_f64(p, "center_y", -1.0e6, 1.0e6)?)
    {
        return Ok(DVec2::new(x, y));
    }
    let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for q in area.iter().flatten().flatten() {
        lo = lo.min(*q);
        hi = hi.max(*q);
    }
    Ok((lo + hi) / 2.0)
}

/// The parts of the polyline `pts` inside `area`.
fn clip(pts: &[DVec2], area: &[Shape]) -> Vec<Vec<DVec2>> {
    let mut out = Vec::new();
    let mut run: Vec<DVec2> = Vec::new();
    for &q in pts {
        if region::contains(area, q) {
            run.push(q);
        } else if run.len() >= 2 {
            out.push(std::mem::take(&mut run));
        } else {
            run.clear();
        }
    }
    if run.len() >= 2 {
        out.push(run);
    }
    out
}

/// Farthest the area reaches from `centre`.
fn reach(area: &[Shape], centre: DVec2) -> f64 {
    area.iter().flatten().flatten().map(|q| q.distance(centre)).fold(0.0, f64::max)
}

pub fn spiral(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let Setup { c, d, area, mut e, warnings } = setup(s, p, SURFACE_TOOLS, 0.1)?;
    let o = centre(p, &area)?;
    let far = reach(&area, o);
    // An Archimedean spiral, a stepover further out each turn, sampled finely enough.
    let mut pts = Vec::new();
    let mut a = 0.0f64;
    let k = c.stepover / std::f64::consts::TAU;
    while k * a <= far && pts.len() < 4_000_000 {
        let r = k * a;
        pts.push(o + DVec2::new(a.cos(), a.sin()) * r);
        a += (d.step / r.max(d.step)).min(0.2);
    }
    if !c.climb {
        for q in &mut pts {
            q.y = 2.0 * o.y - q.y;
        }
    }
    if params::opt_str(p, "spiral_direction")?.unwrap_or("outward") == "inward" {
        pts.reverse();
    }
    let passes: Vec<Vec<DVec3>> =
        clip(&pts, &area).iter().map(|run| d.path(run, false)).filter(|r| r.len() >= 2).collect();
    emit(&mut e, &c, &d, passes, c.stepover * 3.0)?;
    e.finish(p, c, id, SPIRAL, "Spiral", warnings)
}

pub fn radial(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let Setup { c, d, area, mut e, warnings } = setup(s, p, SURFACE_TOOLS, 0.1)?;
    let o = centre(p, &area)?;
    let far = reach(&area, o);
    let inner = params::opt_f64(p, "inner_radius", 0.0, 1.0e5)?.unwrap_or(c.stepover);
    // Spokes at the stepover apart where they end.
    let spokes = ((std::f64::consts::TAU * far / c.stepover).ceil() as usize).clamp(4, MAX_PASSES);
    let mut passes = Vec::new();
    for n in 0..spokes {
        let a = std::f64::consts::TAU * n as f64 / spokes as f64;
        let dir = DVec2::new(a.cos(), a.sin());
        let count = ((far - inner) / d.step).ceil().clamp(1.0, 1.0e6) as usize;
        let mut pts: Vec<DVec2> =
            (0..=count).map(|k| o + dir * (inner + (far - inner) * k as f64 / count as f64)).collect();
        // Back and forth: every other spoke runs inwards.
        if n % 2 == 1 {
            pts.reverse();
        }
        for run in clip(&pts, &area) {
            let path = d.path(&[*run.first().unwrap_or(&o), *run.last().unwrap_or(&o)], false);
            if path.len() >= 2 {
                passes.push(path);
            }
        }
    }
    emit(&mut e, &c, &d, passes, c.stepover * 3.0)?;
    e.finish(p, c, id, RADIAL, "Radial", warnings)
}

pub fn flat(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let c = common_for(s, p, 0.6, 1.0, &["flat", "bull", "face_mill"])?;
    let warnings = Vec::new();
    let r = c.radius();
    let axis = c.frame.world_dir(DVec3::Z);
    // Flat faces looking up the tool axis, grouped by height.
    let mut floors: Vec<(f64, Vec<Shape>)> = Vec::new();
    for m in &s.job.models {
        for (fi, f) in m.faces.faces.iter().enumerate() {
            let FaceKind::Plane { normal, point } = f.kind else { continue };
            if normal.dot(axis) < 0.999 {
                continue;
            }
            let z = c.frame.local(point).z;
            let loops: Vec<Vec<DVec2>> = boundary_loops(&m.mesh, &m.faces, fi)
                .iter()
                .map(|l| l.iter().map(|q| c.frame.local(*q).truncate()).collect())
                .collect();
            let Some(shape) = region::shape_from_loops(&loops) else { continue };
            match floors.iter_mut().find(|(fz, _)| (fz - z).abs() < 0.01) {
                Some((_, shapes)) => shapes.push(shape),
                None => floors.push((z, vec![shape])),
            }
        }
    }
    if floors.is_empty() {
        return Err(bad("the model has no flat areas facing the tool"));
    }
    let selected = params::str_list(p, "geometry")?;
    if !selected.is_empty() {
        // Only the floors the picked faces lie on.
        let mut warnings_unused = Vec::new();
        let (shapes, sel) = super::common::selected_shapes(s, p, &c.frame, c.tolerance, &mut warnings_unused)?;
        if let Some((top, bottom)) = sel {
            floors.retain(|(z, _)| *z <= top + 0.01 && *z >= bottom - 0.01);
            for (_, f) in &mut floors {
                *f = region::boolean(f, &shapes, region::Op::Intersect);
            }
        }
    }
    floors.sort_by(|a, b| b.0.total_cmp(&a.0));
    let [clearance, retract, feed_height, top, _] = heights(s, p, &c.frame, None, &FINISH_HEIGHTS)?;
    let mut e = Emitter::new(c.frame, clearance, retract, feed_height - top);
    let mut from = DVec2::ZERO;
    for (z, shapes) in floors {
        let shapes = if shapes.len() > 1 { region::boolean(&shapes, &[], region::Op::Union) } else { shapes };
        let allowed = region::offset(&shapes, -(r + c.stock_radial), c.tolerance);
        if allowed.is_empty() {
            continue;
        }
        let mut rings: Vec<Vec<Shape>> = vec![allowed.clone()];
        while rings.len() < MAX_PASSES {
            let Some(next) = rings.last().map(|last| region::offset(last, -c.stepover, c.tolerance)) else { break };
            if next.is_empty() {
                break;
            }
            rings.push(next);
        }
        rings.reverse();
        let safe = Arc::new(region::offset(&allowed, 1.0e-3, c.tolerance));
        let at_z = z + c.stock_axial;
        for ring in &rings {
            for l in order_loops(cutting_contours(ring, c.climb), from) {
                from = l.first().copied().unwrap_or(from);
                e.pass(
                    &c,
                    &Pass {
                        pts: l.iter().map(|q| q.extend(at_z)).collect(),
                        closed: true,
                        clear_above: at_z,
                        ramp: false,
                        safe: Some(Arc::clone(&safe)),
                    },
                )?;
            }
        }
    }
    e.finish(p, c, id, FLAT, "Flat", warnings)
}

/// Thins a mask to lines one sample wide (Zhang-Suen).
fn thin(l: &Lattice, mask: &mut [bool]) {
    let (nx, ny) = (l.nx, l.ny);
    let at = |m: &[bool], i: i64, j: i64| {
        i >= 0 && j >= 0 && (i as usize) < nx && (j as usize) < ny && m[j as usize * nx + i as usize]
    };
    for _ in 0..200 {
        let mut changed = false;
        for pass in 0..2 {
            let snapshot = mask.to_vec();
            for j in 0..ny as i64 {
                for i in 0..nx as i64 {
                    if !at(&snapshot, i, j) {
                        continue;
                    }
                    let n = [
                        at(&snapshot, i, j + 1),
                        at(&snapshot, i + 1, j + 1),
                        at(&snapshot, i + 1, j),
                        at(&snapshot, i + 1, j - 1),
                        at(&snapshot, i, j - 1),
                        at(&snapshot, i - 1, j - 1),
                        at(&snapshot, i - 1, j),
                        at(&snapshot, i - 1, j + 1),
                    ];
                    let b = n.iter().filter(|v| **v).count();
                    let a = (0..8).filter(|&k| !n[k] && n[(k + 1) % 8]).count();
                    let (p2, p4, p6, p8) = (n[0], n[2], n[4], n[6]);
                    let ok = if pass == 0 {
                        !(p2 && p4 && p6) && !(p4 && p6 && p8)
                    } else {
                        !(p2 && p4 && p8) && !(p2 && p6 && p8)
                    };
                    if (2..=6).contains(&b) && a == 1 && ok {
                        mask[j as usize * nx + i as usize] = false;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Chains of set samples (8-connected), from line ends first.
fn trace(l: &Lattice, mask: &[bool]) -> Vec<Vec<DVec2>> {
    let (nx, ny) = (l.nx as i64, l.ny as i64);
    let idx = |i: i64, j: i64| (i >= 0 && j >= 0 && i < nx && j < ny).then(|| (j * nx + i) as usize);
    let set = |i: i64, j: i64| idx(i, j).is_some_and(|k| mask.get(k).copied().unwrap_or(false));
    let neighbours = |i: i64, j: i64| -> Vec<(i64, i64)> {
        let mut v = Vec::new();
        for (di, dj) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            if set(i + di, j + dj) {
                v.push((i + di, j + dj));
            }
        }
        v
    };
    let mut used = vec![false; mask.len()];
    let mut out = Vec::new();
    for want_end in [true, false] {
        for j in 0..ny {
            for i in 0..nx {
                let Some(k) = idx(i, j) else { continue };
                if !set(i, j) || used[k] || (want_end && neighbours(i, j).len() != 1) {
                    continue;
                }
                let mut line = vec![l.at(i as usize, j as usize)];
                used[k] = true;
                let (mut ci, mut cj) = (i, j);
                loop {
                    let next = neighbours(ci, cj).into_iter().find(|&(a, b)| idx(a, b).is_some_and(|m| !used[m]));
                    let Some((a, b)) = next else { break };
                    if let Some(m) = idx(a, b) {
                        used[m] = true;
                    }
                    line.push(l.at(a as usize, b as usize));
                    (ci, cj) = (a, b);
                    if line.len() > 1_000_000 {
                        break;
                    }
                }
                out.push(line);
            }
        }
    }
    out
}

/// `q` moved onto the corner: towards the neighbour where the ball's contact jumps, as far as
/// the contact stays on `q`'s side (bisected).
fn corner(d: &Drop, q: DVec2, res: f64, jump: f64) -> DVec2 {
    let contact = |x: DVec2| d.cutter.ball_contact(x).map(|(_, c)| c);
    let Some(here) = contact(q) else { return q };
    let mut best: Option<(DVec2, f64)> = None;
    for k in 0..8 {
        let a = std::f64::consts::TAU * k as f64 / 8.0;
        let n = q + DVec2::new(a.cos(), a.sin()) * res * 1.5;
        if let Some(c) = contact(n) {
            let dd = c.distance(here);
            if dd > jump && best.is_none_or(|(_, b)| dd > b) {
                best = Some((n, dd));
            }
        }
    }
    let Some((mut far, _)) = best else { return q };
    let mut near = q;
    for _ in 0..10 {
        let mid = (near + far) / 2.0;
        match contact(mid) {
            Some(c) if c.distance(here) <= jump * 0.5 => near = mid,
            _ => far = mid,
        }
    }
    near
}

pub fn pencil(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let Setup { c, d, area, mut e, warnings } = setup(s, p, crate::tooling::BALL_ENDED, 0.1)?;
    if !c.tool.ball_ended() {
        return Err(bad("pencil finishing needs a ball-ended tool"));
    }
    let r = c.radius();
    let res = (r / 5.0).clamp(0.02, 0.5);
    let (l, z, inside) = height_grid(&d, &area, res)?;
    // Where the ball's contact point jumps between neighbouring samples, it touches two faces.
    let contact: Vec<Option<DVec3>> = (0..l.len())
        .map(|k| {
            if inside.get(k).copied().unwrap_or(false) {
                d.cutter.ball_contact(l.at(k % l.nx, k / l.nx)).map(|(_, q)| q)
            } else {
                None
            }
        })
        .collect();
    let jump = (l.res * 3.0).max(r * 0.3);
    let mut mask = vec![false; l.len()];
    for j in 0..l.ny {
        for i in 0..l.nx {
            let k = j * l.nx + i;
            let Some(a) = contact.get(k).copied().flatten() else { continue };
            for m in [(i + 1 < l.nx).then_some(k + 1), (j + 1 < l.ny).then_some(k + l.nx)].into_iter().flatten() {
                if !contact.get(m).copied().flatten().is_some_and(|b| b.distance(a) > jump) {
                    continue;
                }
                // On a slope both sides of the jump are the corner; at a wall's foot the tool
                // height jumps too, and the corner is on the lower side.
                let (zk, zm) = (z.get(k).copied().unwrap_or(f64::NAN), z.get(m).copied().unwrap_or(f64::NAN));
                let step = l.res * 3.0;
                // (NaN heights count as level.)
                let m_higher = zm > zk + step;
                let k_higher = zk > zm + step;
                if !m_higher && let Some(v) = mask.get_mut(m) {
                    *v = true;
                }
                if !k_higher {
                    mask[k] = true;
                }
            }
        }
    }
    thin(&l, &mut mask);
    let min_len = params::opt_f64(p, "min_length", 0.0, 1.0e4)?.unwrap_or(r);
    let passes: Vec<Vec<DVec3>> = trace(&l, &mask)
        .into_iter()
        .filter(|line| line.windows(2).map(|w| w[0].distance(w[1])).sum::<f64>() >= min_len)
        .map(|line| {
            let exact: Vec<DVec2> = line.iter().map(|q| corner(&d, *q, l.res, jump)).collect();
            // Refined points are on the corner already; smoothing would pull curves inwards.
            d.path(&exact, false)
        })
        .filter(|p| p.len() >= 2)
        .collect();
    if passes.is_empty() {
        return Err(bad("there are no concave corners for this tool to follow"));
    }
    emit(&mut e, &c, &d, passes, r * 2.0)?;
    e.finish(p, c, id, PENCIL, "Pencil", warnings)
}

pub fn steep_shallow(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let Setup { c, d, area, mut e, warnings } = setup(s, p, SURFACE_TOOLS, 0.1)?;
    let threshold = params::opt_f64(p, "threshold_angle", 1.0, 89.0)?.unwrap_or(40.0).to_radians();
    let (l, z, inside) = height_grid(&d, &area, c.stepover / 2.5)?;
    // Slope of the tip surface (the model's slope under the tool), radians.
    let slope: Vec<f64> = (0..l.len())
        .map(|k| {
            let (i, j) = (k % l.nx, k / l.nx);
            let h = |a: usize, b: usize| z.get(b * l.nx + a).copied().unwrap_or(f64::NAN);
            let gx = if i > 0 && i + 1 < l.nx { (h(i + 1, j) - h(i - 1, j)) / (2.0 * l.res) } else { 0.0 };
            let gy = if j > 0 && j + 1 < l.ny { (h(i, j + 1) - h(i, j - 1)) / (2.0 * l.res) } else { 0.0 };
            let g = (gx * gx + gy * gy).sqrt();
            if g.is_finite() { g.atan() } else { 0.0 }
        })
        .collect();
    // A little overlap between the two, so no band is left between them.
    let overlap = 3f64.to_radians();
    let steep_z: Vec<f64> =
        z.iter().zip(&slope).map(|(h, a)| if *a >= threshold - overlap { *h } else { f64::NAN }).collect();
    let shallow: Vec<bool> = inside.iter().zip(&slope).map(|(i, a)| *i && *a < threshold + overlap).collect();
    // Steep: level passes, the stepover apart along the wall.
    let (lo, hi) = steep_z
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(v), b.max(v)));
    let mut passes = Vec::new();
    if lo < hi {
        let dz =
            params::opt_f64(p, "stepdown", 0.0, 1.0e3)?.filter(|v| *v > 0.0).unwrap_or(c.stepover * threshold.sin());
        let mut level = hi - dz * 0.5;
        while level > lo && passes.len() < MAX_PASSES {
            for (pts, closed) in iso_lines(&l, &steep_z, level) {
                let path = d.path(&pts, closed);
                if path.len() >= 2 {
                    passes.push(path);
                }
            }
            level -= dz;
        }
    }
    let steep_count = passes.len();
    passes.extend(distance_passes(&d, &l, &z, &shallow, c.stepover));
    if passes.is_empty() {
        return Err(bad("there is nothing to finish in this area"));
    }
    // Steep passes top down first, then the shallow ones.
    let shallow_passes = passes.split_off(steep_count);
    for list in [passes, shallow_passes] {
        emit(&mut e, &c, &d, list, c.stepover * 2.5)?;
    }
    e.finish(p, c, id, STEEP_SHALLOW, "Steep and shallow", warnings)
}
