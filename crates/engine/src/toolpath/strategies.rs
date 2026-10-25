//! Milling strategies beyond the 2D contour:
//!
//! - **Face**: back-and-forth rows across the stock top (or a chosen area) down to a height.
//! - **Pocket**: offset rings clearing the inside of selected faces or closed edge chains,
//!   islands included, from the middle out to the wall.
//! - **Rough** (3D): Z-level clearing of the stock around the part, rows plus a pass around the
//!   part at each level.
//! - **Waterline** (3D): Z-level passes around the part, for steep walls.
//! - **Raster** (3D): parallel rows following the part's surface, for shallow areas.
//!
//! The 3D strategies work with flat end mills: the shadow of the part above each level gives
//! where the tool may go (Rough, Waterline); a drop cutter gives how low it can come (Raster).

use std::sync::Arc;

use CosmicCAM_geom::cutter::{DropCutter, shadow_above};
use CosmicCAM_geom::region::{self, Shape};
use glam::{DVec2, DVec3};
use serde_json::Value;

use super::Toolpath;
use super::common::{
    Emitter, MAX_PASSES, Pass, bad, common, cutting_contours, footprint, heights, levels, order_loops, part_triangles,
    selected_shapes,
};
use crate::error::EngineResult;
use crate::params;
use crate::session::Session;

/// Strategy ids as toolpaths record them.
pub const FACE: &str = "face";
pub const POCKET: &str = "pocket2d";
pub const ROUGH: &str = "rough3d";
pub const WATERLINE: &str = "waterline";
pub const RASTER: &str = "raster";

/// Where an area-based strategy works: the stock, the model, or the selected geometry.
pub const BOUNDARIES: &[&str] = &["stock", "model", "selection"];
/// Row patterns.
pub const PATTERNS: &[&str] = &["zigzag", "one_way"];

/// Default heights per strategy: (prefix, reference, offset mm).
pub const FACE_HEIGHTS: [(&str, &str, f64); 5] = [
    ("clearance", "stock_top", 15.0),
    ("retract", "stock_top", 5.0),
    ("feed_height", "stock_top", 2.0),
    ("top", "stock_top", 0.0),
    ("bottom", "model_top", 0.0),
];
pub const POCKET_HEIGHTS: [(&str, &str, f64); 5] = [
    ("clearance", "model_top", 15.0),
    ("retract", "model_top", 5.0),
    ("feed_height", "model_top", 2.0),
    ("top", "model_top", 0.0),
    ("bottom", "selection", 0.0),
];
pub const ROUGH_HEIGHTS: [(&str, &str, f64); 5] = [
    ("clearance", "stock_top", 15.0),
    ("retract", "stock_top", 5.0),
    ("feed_height", "stock_top", 2.0),
    ("top", "stock_top", 0.0),
    ("bottom", "model_bottom", 0.0),
];
pub const FINISH_HEIGHTS: [(&str, &str, f64); 5] = [
    ("clearance", "model_top", 15.0),
    ("retract", "model_top", 5.0),
    ("feed_height", "model_top", 2.0),
    ("top", "model_top", 0.0),
    ("bottom", "model_bottom", 0.0),
];

fn rect(lo: DVec2, hi: DVec2) -> Shape {
    region::rectangle(lo, hi)
}

/// The working area for Face and Raster: the stock's or model's footprint, or the selection's
/// outline, grown by `boundary_offset`.
fn boundary(
    s: &Session,
    p: &Value,
    c: &super::common::Common,
    default: &str,
    warnings: &mut Vec<String>,
) -> EngineResult<(Vec<Shape>, super::common::Selection)> {
    let f = &c.frame;
    let grow = params::opt_f64(p, "boundary_offset", -1.0e4, 1.0e4)?.unwrap_or(0.0);
    let (shapes, selection) = match params::opt_str(p, "boundary")?.unwrap_or(default) {
        "selection" => {
            let (shapes, sel) = selected_shapes(s, p, f, c.tolerance, warnings)?;
            if shapes.is_empty() {
                return Err(bad("select the faces or closed edges to work inside (`geometry`)"));
            }
            (shapes, sel)
        }
        "model" => {
            let (lo, hi) = footprint(f, s.job.part_bounds()).ok_or_else(|| bad("there is no model"))?;
            (vec![rect(lo, hi)], None)
        }
        _ => {
            let stock = s.job.stock.ok_or_else(|| bad("there is no stock; create a setup or fit the stock first"))?;
            let (lo, hi) = footprint(f, stock).ok_or_else(|| bad("the stock is empty"))?;
            (vec![rect(lo, hi)], None)
        }
    };
    let shapes = if grow.abs() > 1.0e-9 { region::offset(&shapes, grow, c.tolerance) } else { shapes };
    if shapes.is_empty() {
        return Err(bad("the boundary offset leaves nothing to cut"));
    }
    Ok((shapes, selection))
}

fn angle(p: &Value) -> EngineResult<f64> {
    Ok(params::opt_f64(p, "angle", -360.0, 360.0)?.unwrap_or(0.0).to_radians())
}

fn zigzag(p: &Value) -> EngineResult<bool> {
    Ok(params::opt_str(p, "pattern")?.unwrap_or("zigzag") == "zigzag")
}

// ---- Face --------------------------------------------------------------------------------

pub fn face(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let c = common(s, p, 0.7, 1.0)?;
    let mut warnings = Vec::new();
    let (area, selection) = boundary(s, p, &c, "stock", &mut warnings)?;
    let [clearance, retract, feed_height, top, bottom] = heights(s, p, &c.frame, selection, &FACE_HEIGHTS)?;
    let r = c.radius();
    let overrun = params::opt_f64(p, "overrun", 0.0, 1.0e4)?.unwrap_or(r * 0.5);
    // Rows run past the area by the radius plus the overrun so the tool leaves it entirely.
    let bounds = cutting_bounds(&area);
    let a = angle(p)?;
    let rows = row_lines(bounds, c.stepover, a, r + overrun);
    let final_z = bottom + c.stock_axial;
    let zs = levels(top, final_z, c.stepdown, params::opt_bool(p, "multiple_depths")?.unwrap_or(false));
    let mut e = Emitter::new(c.frame, clearance, retract, feed_height - top);
    let zz = zigzag(p)?;
    let mut clear = top;
    for z in zs {
        for (k, (a0, b0)) in rows.iter().enumerate() {
            let (a1, b1) = if zz && k % 2 == 1 { (*b0, *a0) } else { (*a0, *b0) };
            // Rows start and end off the stock, so the tool links between them at depth.
            let pass = Pass {
                pts: vec![a1.extend(z), b1.extend(z)],
                closed: false,
                clear_above: clear,
                ramp: false,
                safe: zz.then(|| Arc::new(vec![link_band(bounds, r + overrun)])),
            };
            e.pass(&c, &pass)?;
        }
        clear = z;
    }
    e.finish(p, c, id, FACE, "Face", warnings)
}

/// The bounding rectangle of shapes.
fn cutting_bounds(shapes: &[Shape]) -> (DVec2, DVec2) {
    let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for q in shapes.iter().flatten().flatten() {
        lo = lo.min(*q);
        hi = hi.max(*q);
    }
    (lo, hi)
}

/// Rows across a rectangle at `angle`, `spacing` apart, reaching `reach` past its sides.
fn row_lines(bounds: (DVec2, DVec2), spacing: f64, angle: f64, reach: f64) -> Vec<(DVec2, DVec2)> {
    let (lo, hi) = bounds;
    let centre = (lo + hi) / 2.0;
    let dir = DVec2::new(angle.cos(), angle.sin());
    let across = dir.perp();
    // Half-extents of the rectangle along and across the rows.
    let corners = [lo, DVec2::new(hi.x, lo.y), hi, DVec2::new(lo.x, hi.y)];
    let along = corners.iter().map(|q| (*q - centre).dot(dir).abs()).fold(0.0, f64::max) + reach;
    let half = corners.iter().map(|q| (*q - centre).dot(across).abs()).fold(0.0, f64::max);
    let count = ((2.0 * half / spacing.max(1.0e-3)).ceil() as usize).clamp(1, MAX_PASSES);
    (0..=count)
        .map(|k| {
            let off = -half + 2.0 * half * k as f64 / count as f64;
            let m = centre + across * off;
            (m - dir * along, m + dir * along)
        })
        .collect()
}

/// A band around a rectangle, `reach` wide: where Face rows turn round off the stock.
fn link_band(bounds: (DVec2, DVec2), reach: f64) -> Shape {
    let grow = DVec2::splat(reach * 3.0 + 1.0);
    let mut s = rect(bounds.0 - grow, bounds.1 + grow);
    let mut hole = rect(bounds.0 - DVec2::splat(reach * 0.5), bounds.1 + DVec2::splat(reach * 0.5));
    if let Some(h) = hole.first_mut() {
        h.reverse();
        s.push(h.clone());
    }
    s
}

// ---- Pocket ------------------------------------------------------------------------------

pub fn pocket(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let c = common(s, p, 0.45, 1.0)?;
    let mut warnings = Vec::new();
    let (shapes, selection) = selected_shapes(s, p, &c.frame, c.tolerance, &mut warnings)?;
    if shapes.is_empty() {
        return Err(bad("select the pocket: its floor faces or closed edges (`geometry`)"));
    }
    let [clearance, retract, feed_height, top, bottom] = heights(s, p, &c.frame, selection, &POCKET_HEIGHTS)?;
    let r = c.radius();
    let allowed = region::offset(&shapes, -(r + c.stock_radial), c.tolerance);
    if allowed.is_empty() {
        return Err(bad(format!("T{} (Ø{}) does not fit in the pocket", c.tool.number, c.tool.diameter)));
    }
    // Rings from the wall inwards; cut from the middle out so the last ring finishes the wall.
    let mut rings: Vec<Vec<Shape>> = vec![allowed.clone()];
    while rings.len() < MAX_PASSES {
        let Some(next) = rings.last().map(|last| region::offset(last, -c.stepover, c.tolerance)) else { break };
        if next.is_empty() {
            break;
        }
        rings.push(next);
    }
    rings.reverse();
    let finish = params::opt_bool(p, "finishing_pass")?.unwrap_or(false) && c.stock_radial > 1.0e-9;
    let finish_ring = if finish { region::offset(&shapes, -r, c.tolerance) } else { Vec::new() };
    // Ring starts lie on the region's edge: links are checked against it grown by a micron.
    let safe = Arc::new(region::offset(&allowed, 1.0e-3, c.tolerance));
    let final_z = bottom + c.stock_axial;
    let zs = levels(top, final_z, c.stepdown, params::opt_bool(p, "multiple_depths")?.unwrap_or(true));
    let mut e = Emitter::new(c.frame, clearance, retract, feed_height - top);
    let mut clear = top;
    let mut from = DVec2::ZERO;
    for z in zs {
        let mut first = true;
        for ring in &rings {
            let loops = order_loops(cutting_contours(ring, c.climb), from);
            for l in loops {
                from = l.first().copied().unwrap_or(from);
                e.pass(
                    &c,
                    &Pass {
                        pts: l.iter().map(|q| q.extend(z)).collect(),
                        closed: true,
                        clear_above: clear,
                        ramp: first,
                        safe: Some(Arc::clone(&safe)),
                    },
                )?;
                first = false;
            }
        }
        if finish {
            let finish_safe = Arc::new(region::offset(&shapes, -r + 1.0e-3, c.tolerance));
            for l in order_loops(cutting_contours(&finish_ring, c.climb), from) {
                from = l.first().copied().unwrap_or(from);
                e.pass(
                    &c,
                    &Pass {
                        pts: l.iter().map(|q| q.extend(z)).collect(),
                        closed: true,
                        clear_above: z,
                        ramp: false,
                        safe: Some(Arc::clone(&finish_safe)),
                    },
                )?;
            }
        }
        clear = z;
    }
    e.finish(p, c, id, POCKET, "Pocket", warnings)
}

// ---- 3D: shared --------------------------------------------------------------------------

/// Where the tool centre must stay out of at height `z`: the part's shadow above it (raised by
/// the axial stock), grown by the radius plus the radial stock.
fn keep_out(tris: &[[DVec3; 3]], z: f64, c: &super::common::Common) -> Vec<Shape> {
    let shadow = shadow_above(tris, z - c.stock_axial);
    region::offset(&shadow, c.radius() + c.stock_radial, c.tolerance)
}

fn require_model(tris: &[[DVec3; 3]]) -> EngineResult<()> {
    if tris.is_empty() {
        return Err(bad("there is no model to machine; import one first"));
    }
    Ok(())
}

// ---- Rough -------------------------------------------------------------------------------

pub fn rough(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let c = common(s, p, 0.45, 0.5)?;
    let warnings = Vec::new();
    let tris = part_triangles(s, &c.frame);
    require_model(&tris)?;
    let stock = s.job.stock.ok_or_else(|| bad("there is no stock; create a setup or fit the stock first"))?;
    let (lo, hi) = footprint(&c.frame, stock).ok_or_else(|| bad("the stock is empty"))?;
    let [clearance, retract, feed_height, top, bottom] = heights(s, p, &c.frame, None, &ROUGH_HEIGHTS)?;
    let r = c.radius();
    // The tool centre may go a radius past the stock's sides, so it clears them.
    let outer = vec![rect(lo - DVec2::splat(r * 0.95), hi + DVec2::splat(r * 0.95))];
    let stock_inside = vec![rect(lo + DVec2::splat(1.0e-3), hi - DVec2::splat(1.0e-3))];
    let a = angle(p)?;
    let zs = levels(top, bottom, c.stepdown, true);
    let mut e = Emitter::new(c.frame, clearance, retract, feed_height - top);
    let mut clear = top;
    for z in zs {
        let blocked = keep_out(&tris, z, &c);
        let allowed = region::boolean(&outer, &blocked, region::Op::Difference);
        if allowed.is_empty() {
            clear = z;
            continue;
        }
        // Rows stop a hair inside the region so their ends, and the links between them, test
        // as inside it.
        let safe = Arc::new(region::offset(&allowed, 1.0e-3, c.tolerance));
        let rows = region::hatch(&region::offset(&allowed, -0.01, c.tolerance), c.stepover, a, MAX_PASSES);
        for zone in zones(rows, &safe, c.radius() * 0.5) {
            for (i, (a0, b0)) in zone.iter().enumerate() {
                e.pass(
                    &c,
                    &Pass {
                        pts: vec![a0.extend(z), b0.extend(z)],
                        closed: false,
                        clear_above: clear,
                        ramp: i == 0 && region::contains(&stock_inside, *a0),
                        safe: Some(Arc::clone(&safe)),
                    },
                )?;
            }
        }
        // A pass around the part at this level cleans up the ends of the rows.
        let around: Vec<Vec<DVec2>> =
            allowed.iter().flat_map(|sh| sh.iter().skip(1)).map(|h| if c.climb { h.clone() } else { rev(h) }).collect();
        let from = e.b.at.truncate();
        for l in order_loops(around, from) {
            e.pass(
                &c,
                &Pass {
                    pts: l.iter().map(|q| q.extend(z)).collect(),
                    closed: true,
                    clear_above: z,
                    ramp: false,
                    safe: Some(Arc::clone(&safe)),
                },
            )?;
        }
        clear = z;
    }
    e.finish(p, c, id, ROUGH, "3D rough", warnings)
}

/// Groups hatch rows into zones cut back and forth without lifting: each zone takes a segment,
/// then the nearest segment of the next row it can reach by a link inside `safe`, turned to
/// start at the end nearer the last one, until no such segment is left.
fn zones(rows: Vec<Vec<(DVec2, DVec2)>>, safe: &[Shape], step: f64) -> Vec<Vec<(DVec2, DVec2)>> {
    let mut rows: Vec<Vec<(DVec2, DVec2)>> = rows;
    let mut out = Vec::new();
    while let Some(first_row) = rows.iter().position(|r| !r.is_empty()) {
        let Some(first) = rows.get_mut(first_row).map(|r| r.remove(0)) else { break };
        let mut zone = vec![first];
        let mut at = first.1;
        let mut r = first_row + 1;
        while let Some(row) = rows.get_mut(r) {
            let best = row
                .iter()
                .enumerate()
                .flat_map(|(i, (a, b))| [(i, false, a.distance(at)), (i, true, b.distance(at))])
                .filter(|(i, flip, _)| {
                    row.get(*i)
                        .is_some_and(|(a, b)| region::segment_inside(safe, at, if *flip { *b } else { *a }, step))
                })
                .min_by(|x, y| x.2.total_cmp(&y.2));
            let Some((i, flip, _)) = best else { break };
            let (a, b) = row.remove(i);
            let seg = if flip { (b, a) } else { (a, b) };
            at = seg.1;
            zone.push(seg);
            r += 1;
        }
        out.push(zone);
    }
    out
}

fn rev(v: &[DVec2]) -> Vec<DVec2> {
    v.iter().rev().copied().collect()
}

// ---- Waterline ---------------------------------------------------------------------------

pub fn waterline(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let mut c = common(s, p, 0.1, 0.1)?;
    if params::opt_f64(p, "stepdown", 0.0, 1.0e4)?.is_none() {
        c.stepdown = (c.tool.diameter * 0.1).clamp(0.1, 1.0);
    }
    let warnings = Vec::new();
    let tris = part_triangles(s, &c.frame);
    require_model(&tris)?;
    let [clearance, retract, feed_height, top, bottom] = heights(s, p, &c.frame, None, &FINISH_HEIGHTS)?;
    let zs = levels(top, bottom, c.stepdown, true);
    let mut e = Emitter::new(c.frame, clearance, retract, feed_height - top);
    for z in zs {
        let blocked = keep_out(&tris, z, &c);
        // Around the part the tool runs with the part on its right for climb milling: the
        // outer contours come counter-clockwise, the holes clockwise, so both reverse.
        let loops: Vec<Vec<DVec2>> =
            blocked.iter().flatten().map(|l| if c.climb { rev(l) } else { l.clone() }).collect();
        let from = e.b.at.truncate();
        for l in order_loops(loops, from) {
            e.pass(
                &c,
                &Pass {
                    pts: l.iter().map(|q| q.extend(z)).collect(),
                    closed: true,
                    clear_above: top,
                    ramp: false,
                    safe: None,
                },
            )?;
        }
    }
    e.finish(p, c, id, WATERLINE, "Waterline", warnings)
}

// ---- Raster ------------------------------------------------------------------------------

/// Most points sampled along one raster row.
const MAX_ROW_POINTS: usize = 200_000;

/// Points along `a`→`b` every `step`, each at the height the cutter can come down to, never
/// below `floor`, simplified where the path stays within `tolerance` of a straight line.
fn drop_line(cutter: &DropCutter, a: DVec2, b: DVec2, step: f64, floor: f64, raise: f64, tolerance: f64) -> Vec<DVec3> {
    let n = ((a.distance(b) / step.max(1.0e-3)).ceil() as usize).clamp(1, MAX_ROW_POINTS);
    let pts: Vec<DVec3> = (0..=n)
        .map(|i| {
            let q = a.lerp(b, i as f64 / n as f64);
            q.extend(cutter.height(q).map_or(floor, |z| (z + raise).max(floor)))
        })
        .collect();
    simplify(&pts, tolerance)
}

/// Drops points that lie within `tolerance` of the line through their neighbours.
fn simplify(pts: &[DVec3], tolerance: f64) -> Vec<DVec3> {
    let mut out: Vec<DVec3> = Vec::with_capacity(pts.len());
    for (i, &q) in pts.iter().enumerate() {
        if i + 1 == pts.len() || out.is_empty() {
            out.push(q);
            continue;
        }
        let (Some(&a), Some(&b)) = (out.last(), pts.get(i + 1)) else { continue };
        let ab = b - a;
        let t =
            if ab.length_squared() > 1.0e-18 { ((q - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
        if (a + ab * t).distance(q) > tolerance {
            out.push(q);
        }
    }
    out
}

pub fn raster(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let c = common(s, p, 0.1, 1.0)?;
    let mut warnings = Vec::new();
    let tris = part_triangles(s, &c.frame);
    require_model(&tris)?;
    let (area, selection) = boundary(s, p, &c, "model", &mut warnings)?;
    let [clearance, retract, feed_height, top, bottom] = heights(s, p, &c.frame, selection, &FINISH_HEIGHTS)?;
    let r = c.radius();
    let cutter = DropCutter::new(tris, r);
    let step =
        params::opt_f64(p, "sample_step", 0.0, 1.0e3)?.filter(|v| *v > 0.0).unwrap_or((r * 0.25).clamp(0.05, 1.0));
    let a = angle(p)?;
    let rows = region::hatch(&area, c.stepover, a, MAX_PASSES);
    let zz = zigzag(p)?;
    let mut e = Emitter::new(c.frame, clearance, retract, feed_height - top);
    let mut previous: Option<DVec3> = None;
    for (k, row) in rows.iter().enumerate() {
        let flip = zz && k % 2 == 1;
        let segments: Vec<(DVec2, DVec2)> =
            if flip { row.iter().rev().map(|(a0, b0)| (*b0, *a0)).collect() } else { row.clone() };
        for (a0, b0) in segments {
            let mut pts = drop_line(&cutter, a0, b0, step, bottom, c.stock_axial, c.tolerance);
            // Zigzag rows join along the surface: the link is dropped like a row, so it is safe.
            if zz
                && let (Some(prev), Some(first)) = (previous, pts.first().copied())
                && prev.truncate().distance(first.truncate()) <= c.stepover * 1.5 + 1.0e-6
            {
                let link =
                    drop_line(&cutter, prev.truncate(), first.truncate(), step, bottom, c.stock_axial, c.tolerance);
                for q in link.into_iter().skip(1) {
                    e.b.push(super::MoveKind::Link, q, c.feed)?;
                }
                for q in pts.iter().skip(1) {
                    e.b.push(super::MoveKind::Cut, *q, c.feed)?;
                }
                previous = pts.last().copied();
                continue;
            }
            let Some(first_z) = pts.first().map(|q| q.z) else { continue };
            e.pass(
                &c,
                &Pass { pts: std::mem::take(&mut pts), closed: false, clear_above: first_z, ramp: false, safe: None },
            )?;
            previous = e.b.moves.last().map(|m| c.frame.local(m.to));
        }
    }
    let _ = top;
    e.finish(p, c, id, RASTER, "Raster", warnings)
}

/// Each height's Z in the toolpath's WCS for the settings so far, for display: (prefix,
/// value), None where it cannot be worked out yet. `strategy` is a toolpath strategy id.
pub fn preview_heights(strategy: &str, s: &Session, p: &Value) -> Vec<(String, Option<f64>)> {
    let defaults: &[(&str, &str, f64)] = match strategy {
        FACE => &FACE_HEIGHTS,
        POCKET => &POCKET_HEIGHTS,
        ROUGH => &ROUGH_HEIGHTS,
        super::drill::DRILL => &super::drill::DRILL_HEIGHTS,
        _ => &FINISH_HEIGHTS,
    };
    let Ok((f, _)) = super::contour::frame(s, p) else {
        return defaults.iter().map(|(k, _, _)| ((*k).to_owned(), None)).collect();
    };
    let mut warnings = Vec::new();
    let selection = selected_shapes(s, p, &f, 0.01, &mut warnings).ok().and_then(|(_, sel)| sel);
    defaults
        .iter()
        .map(|(prefix, reference, offset)| {
            let z = super::common::height(s, p, &f, selection, prefix, (reference, *offset)).ok();
            ((*prefix).to_owned(), z)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplify_drops_points_on_straight_runs() {
        let pts: Vec<DVec3> = (0..=10).map(|i| DVec3::new(f64::from(i), 0.0, 0.0)).collect();
        assert_eq!(simplify(&pts, 0.01).len(), 2);
        let bent = vec![DVec3::ZERO, DVec3::new(1.0, 0.0, 1.0), DVec3::new(2.0, 0.0, 0.0)];
        assert_eq!(simplify(&bent, 0.01).len(), 3);
    }

    #[test]
    fn rows_cover_a_rectangle_at_any_angle() {
        let rows = row_lines((DVec2::ZERO, DVec2::new(100.0, 50.0)), 10.0, 0.0, 5.0);
        assert_eq!(rows.len(), 6);
        assert!(rows.iter().all(|(a, b)| a.x <= -5.0 + 1.0e-9 && b.x >= 105.0 - 1.0e-9));
        assert!(row_lines((DVec2::ZERO, DVec2::new(100.0, 50.0)), 10.0, 0.7, 5.0).len() > 6);
    }
}
