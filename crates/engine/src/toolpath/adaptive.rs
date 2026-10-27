//! Adaptive clearing: roughing with a steady tool load ([`CosmicDMS_geom::adaptive`]), level
//! by level, deep and fast where an offset or zigzag path would bury the tool in corners.
//!
//! In 3D it clears the stock around the model; with `boundary: "selection"` it clears the
//! selected pockets (2D). Levels follow the stepdown, with extra ones at the model's flat
//! areas so shelves come out at their height. A pass along the walls at each level takes off
//! what the steady-load path leaves there. Rest machining (`rest: "previous"`) clears only what
//! the toolpaths before this one leave, found by cutting them through a height map of the stock.

use std::sync::Arc;

use CosmicDMS_geom::FaceKind;
use CosmicDMS_geom::adaptive::{self as core, Entry, Grid, Settings};
use CosmicDMS_geom::region::{self, Shape};
use CosmicDMS_geom::zmap::ZMap;
use glam::{DVec2, DVec3};
use serde_json::Value;

use super::common::{Common, Emitter, Pass, bad, footprint, heights, part_triangles, selected_shapes};
use super::strategies::{POCKET_HEIGHTS, ROUGH_HEIGHTS, require_model};
use super::{MoveKind, Toolpath};
use crate::error::EngineResult;
use crate::params;
use crate::session::Session;

pub const ADAPTIVE: &str = "adaptive";
/// What a rest pass clears: everything, or what the earlier toolpaths leave.
pub const REST: &[&str] = &["none", "previous"];
/// Most grid cells per level.
const MAX_CELLS: usize = 4_000_000;

/// Levels from `top` down to `bottom` by `step`, plus `extra` heights in between.
fn levels_with(top: f64, bottom: f64, step: f64, extra: &[f64]) -> Vec<f64> {
    let mut zs = super::common::levels(top, bottom, step, true);
    for &z in extra {
        if z < top - 1.0e-6 && z > bottom + 1.0e-6 && zs.iter().all(|l| (l - z).abs() > 0.05) {
            zs.push(z);
        }
    }
    zs.sort_by(|a, b| b.total_cmp(a));
    zs
}

/// Heights (in the frame) of the models' flat faces square to the tool axis.
fn flat_heights(s: &Session, c: &Common) -> Vec<f64> {
    let axis = c.frame.world_dir(DVec3::Z);
    let mut out = Vec::new();
    for m in &s.job.models {
        for f in &m.faces.faces {
            if let FaceKind::Plane { normal, point } = f.kind
                && normal.dot(axis) > 0.999
            {
                out.push(c.frame.local(point).z);
            }
        }
    }
    out
}

/// The stock left by the toolpaths before `id` (all of them for a new toolpath) that work
/// along this frame's axis, as a height map over `lo`..`hi`.
#[allow(clippy::too_many_arguments)]
fn rest_map(s: &Session, c: &Common, id: &str, lo: DVec2, hi: DVec2, bottom: f64, top: f64, cell: f64) -> Option<ZMap> {
    let mut map = ZMap::new(lo, hi, bottom, top, cell, MAX_CELLS)?;
    let axis = c.frame.world_dir(DVec3::Z);
    let before = s.job.toolpaths.iter().position(|t| t.id == id).unwrap_or(s.job.toolpaths.len());
    for tp in s.job.toolpaths.iter().take(before) {
        if tp.multi_axis() || tp.axis.dot(axis) < 0.9999 {
            continue;
        }
        let Some(tool) = s.library.machine(&tp.machine).and_then(|m| m.tool(tp.tool)) else { continue };
        let (r, ball) = (tool.diameter / 2.0, tool.ball_ended());
        let mut at = c.frame.local(tp.start);
        for mv in tp.moves.iter() {
            let to = c.frame.local(mv.to);
            map.cut_tool(at, to, r, ball);
            at = to;
        }
    }
    Some(map)
}

/// Emits one level's adaptive passes and its wall pass.
struct Level<'a> {
    c: &'a Common,
    z: f64,
    /// The material is clear down to here above this level.
    clear_above: f64,
    retract: f64,
    gap: f64,
}

impl Level<'_> {
    fn passes(&self, e: &mut Emitter, passes: &[core::Pass]) -> EngineResult<()> {
        let (c, z) = (self.c, self.z);
        for p in passes {
            let Some(&first) = p.pts.first() else { continue };
            let start = first.extend(z);
            let fresh = !e.started();
            match p.entry {
                Entry::Link if !fresh => {
                    for q in &p.link {
                        e.b.push(MoveKind::Link, q.extend(z), c.feed)?;
                    }
                    e.b.push(MoveKind::Link, start, c.feed)?;
                }
                entry => {
                    if fresh {
                        e.b.at = first.extend(e.clearance);
                        e.b.moves.clear();
                        e.b.push(MoveKind::Rapid, first.extend(self.retract), 0.0)?;
                        e.mark_started();
                    } else {
                        let at = e.b.at;
                        e.b.push(MoveKind::Rapid, at.truncate().extend(self.retract.max(at.z)), 0.0)?;
                        if c.rapid_links {
                            e.b.push(MoveKind::Rapid, first.extend(self.retract.max(at.z)), 0.0)?;
                        } else {
                            e.b.push(MoveKind::Link, first.extend(self.retract.max(at.z)), c.feed)?;
                        }
                    }
                    let down_to = (self.clear_above + self.gap).min(e.b.at.z).max(z);
                    e.b.push(MoveKind::Rapid, first.extend(down_to), 0.0)?;
                    let helixed = match entry {
                        Entry::Helix(r) if r > 0.0 => e.helix_at(c, first, down_to, z, r)?,
                        _ => false,
                    };
                    if !helixed {
                        e.b.push(MoveKind::Plunge, start, c.plunge_feed)?;
                    }
                }
            }
            for q in p.pts.iter().skip(1) {
                e.b.push(MoveKind::Cut, q.extend(z), c.feed)?;
            }
        }
        Ok(())
    }
}

pub fn adaptive(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let c = super::common::common(s, p, 0.15, 1.0)?;
    let mut warnings = Vec::new();
    let r = c.radius();
    let two_d = params::opt_str(p, "boundary")?.unwrap_or("stock") == "selection";
    let rest = params::opt_str(p, "rest")?.unwrap_or("none") == "previous";
    let walls = params::opt_bool(p, "wall_pass")?.unwrap_or(true);
    let keep_down = params::opt_bool(p, "keep_down")?.unwrap_or(true);
    let flats = params::opt_bool(p, "flat_levels")?.unwrap_or(true);
    let load = c.stepover.min(r * 1.9);
    if load <= 0.0 {
        return Err(bad("`stepover` (the optimal load) must be more than zero"));
    }
    let settings = Settings {
        radius: r,
        load,
        climb: c.climb,
        helix_radius: c.helix.map_or(0.0, |(_, hr)| hr),
        stay_down: params::opt_f64(p, "stay_down_distance", 0.0, 1.0e5)?.unwrap_or(c.tool.diameter * 20.0),
    };
    // The region, the material at a level and where the tool centre may go there.
    let tris = if two_d { Vec::new() } else { part_triangles(s, &c.frame) };
    let (selection_shapes, selection) = if two_d {
        let (shapes, sel) = selected_shapes(s, p, &c.frame, c.tolerance, &mut warnings)?;
        if shapes.is_empty() {
            return Err(bad("select the pockets to clear: their floor faces or closed edges (`geometry`)"));
        }
        (shapes, sel)
    } else {
        require_model(&tris)?;
        (Vec::new(), None)
    };
    let defaults = if two_d { &POCKET_HEIGHTS } else { &ROUGH_HEIGHTS };
    let [clearance, retract, feed_height, top, bottom] = heights(s, p, &c.frame, selection, defaults)?;
    let (lo, hi) = if two_d {
        let mut lo = DVec2::splat(f64::INFINITY);
        let mut hi = DVec2::splat(f64::NEG_INFINITY);
        for q in selection_shapes.iter().flatten().flatten() {
            lo = lo.min(*q);
            hi = hi.max(*q);
        }
        (lo, hi)
    } else {
        let stock = s.job.stock.ok_or_else(|| bad("there is no stock; create a setup or fit the stock first"))?;
        footprint(&c.frame, stock).ok_or_else(|| bad("the stock is empty"))?
    };
    if !(lo.x < hi.x && lo.y < hi.y) {
        return Err(bad("there is no area to clear"));
    }
    let margin = r * 1.2;
    let grid = Grid::new(lo - DVec2::splat(margin), hi + DVec2::splat(margin), r / 10.0, MAX_CELLS)
        .ok_or_else(|| bad("the area to clear is too large or empty"))?;
    let stock_rect = region::rectangle(lo, hi);
    let rest_heights = if rest {
        let map = rest_map(
            s,
            &c,
            &id,
            grid.origin,
            grid.origin + DVec2::new(grid.nx as f64, grid.ny as f64) * grid.res,
            bottom - 1.0,
            top,
            grid.res,
        );
        if map.is_none() {
            warnings.push("the rest material could not be worked out; clearing everything".into());
        }
        map
    } else {
        None
    };
    let extra =
        if flats && !two_d { flat_heights(s, &c).into_iter().map(|z| z + c.stock_axial).collect() } else { Vec::new() };
    let final_z = bottom + c.stock_axial;
    let zs = levels_with(top, final_z, c.stepdown, &extra);
    let mut e = Emitter::new(c.frame, clearance, retract, feed_height - top);
    let mut clear = top;
    let mut nothing = true;
    for z in zs {
        // Material and keep-out at this level.
        let (material_shapes, allowed) = if two_d {
            let allowed = region::offset(&selection_shapes, -(r + c.stock_radial), c.tolerance);
            (selection_shapes.clone(), allowed)
        } else {
            let shadow = CosmicDMS_geom::cutter::shadow_above(&tris, z - c.stock_axial);
            let part = region::offset(&shadow, c.stock_radial, c.tolerance);
            let material = region::boolean(std::slice::from_ref(&stock_rect), &part, region::Op::Difference);
            let keep_out = region::offset(&shadow, r + c.stock_radial, c.tolerance);
            let outer = vec![region::rectangle(lo - DVec2::splat(r * 0.95), hi + DVec2::splat(r * 0.95))];
            (material, region::boolean(&outer, &keep_out, region::Op::Difference))
        };
        if allowed.is_empty() {
            clear = z;
            continue;
        }
        let mut cells = core::raster(&grid, &material_shapes);
        if let Some(map) = &rest_heights {
            for j in 0..grid.ny {
                for i in 0..grid.nx {
                    let k = j * grid.nx + i;
                    if cells.get(k).copied().unwrap_or(false) {
                        let left = map.height_at(grid.centre(i, j)).unwrap_or(f64::NEG_INFINITY);
                        if left <= z + 1.0e-3
                            && let Some(cell) = cells.get_mut(k)
                        {
                            *cell = false;
                        }
                    }
                }
            }
        }
        let had = cells.clone();
        let mut passes = core::clear(grid, &allowed, cells, &settings);
        if !keep_down {
            for pass in &mut passes {
                if pass.entry == Entry::Link {
                    pass.entry = Entry::Retract;
                }
            }
        }
        let level = Level { c: &c, z, clear_above: clear, retract, gap: (feed_height - top).max(0.5) };
        if !passes.is_empty() {
            nothing = false;
        }
        level.passes(&mut e, &passes)?;
        if walls {
            let inside = if two_d { None } else { Some((lo, hi)) };
            nothing &= !wall_pass(&mut e, &c, &grid, &had, &allowed, inside, z, clear)?;
        }
        clear = z;
    }
    if nothing {
        return Err(bad(if rest {
            "the earlier toolpaths leave nothing for this tool to clear"
        } else {
            "there is nothing to cut with these settings"
        }));
    }
    e.finish(p, c, id, ADAPTIVE, "Adaptive", warnings)
}

/// A pass along the walls at `z`: the contours of where the centre may go, where material was
/// within reach of them. Returns whether it cut anything.
#[allow(clippy::too_many_arguments)]
fn wall_pass(
    e: &mut Emitter,
    c: &Common,
    grid: &Grid,
    had: &[bool],
    allowed: &[Shape],
    stock: Option<(DVec2, DVec2)>,
    z: f64,
    clear: f64,
) -> EngineResult<bool> {
    let r = c.radius();
    // Material near a point: any cell of the level's material within the radius.
    let near = |q: DVec2| -> bool {
        let reach = (r / grid.res).ceil() as i64 + 1;
        let Some((i, j)) = grid.cell(q) else { return false };
        for dj in -reach..=reach {
            for di in -reach..=reach {
                let (a, b) = (i as i64 + di, j as i64 + dj);
                if a < 0 || b < 0 || a as usize >= grid.nx || b as usize >= grid.ny {
                    continue;
                }
                let (a, b) = (a as usize, b as usize);
                if had.get(b * grid.nx + a).copied().unwrap_or(false) && grid.centre(a, b).distance(q) <= r + grid.res {
                    return true;
                }
            }
        }
        false
    };
    let contours: Vec<Vec<DVec2>> = allowed
        .iter()
        .flatten()
        // Around the outside of the stock there is nothing to finish.
        .filter(|l| stock.is_none_or(|(lo, hi)| l.iter().any(|q| q.x > lo.x && q.y > lo.y && q.x < hi.x && q.y < hi.y)))
        .filter(|l| {
            let n = l.len().max(1);
            let hits = l.iter().step_by((n / 24).max(1)).filter(|q| near(**q)).count();
            hits * 10 >= (n / (n / 24).max(1)).max(1)
        })
        .map(|l| if c.climb { l.clone() } else { l.iter().rev().copied().collect() })
        .collect();
    if contours.is_empty() {
        return Ok(false);
    }
    let safe = Arc::new(region::offset(allowed, 1.0e-3, c.tolerance));
    let from = e.b.at.truncate();
    for l in super::common::order_loops(contours, from) {
        e.pass(
            c,
            &Pass {
                pts: l.iter().map(|q| q.extend(z)).collect(),
                closed: true,
                clear_above: clear,
                ramp: false,
                safe: Some(Arc::clone(&safe)),
            },
        )?;
    }
    Ok(true)
}
