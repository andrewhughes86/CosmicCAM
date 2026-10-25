//! Drilling: holes picked as cylinder faces, circular edges or points, each drilled straight
//! down the tool axis with a cycle (drill, spot with dwell, peck, chip break, tap, bore). The
//! moves show the cycle in the viewport; the G-code writes it as a canned cycle.

use std::sync::Arc;

use CosmicCAM_geom::{EdgeKind, FaceKind};
use glam::{DVec2, DVec3};
use serde_json::{Map, Value, json};

use super::common::{bad, height};
use super::contour::{Builder, frame};
use super::{DrillCycle, MoveKind, Toolpath};
use crate::error::EngineResult;
use crate::params;
use crate::session::{Entity, Session};

pub const DRILL: &str = "drill";
/// Drilling cycles.
pub const CYCLES: &[&str] = &["drill", "spot", "peck", "chip_break", "tap", "bore"];
/// Most holes one toolpath drills.
pub const MAX_HOLES: usize = 10_000;

/// Default heights: (prefix, reference, offset mm). `selection` is each hole's own top and bottom.
pub const DRILL_HEIGHTS: [(&str, &str, f64); 4] = [
    ("clearance", "model_top", 15.0),
    ("retract", "selection", 2.0),
    ("top", "selection", 0.0),
    ("bottom", "selection", 0.0),
];

/// A hole to drill, in the frame: centre and its own top and bottom.
struct Hole {
    at: DVec2,
    top: f64,
    bottom: f64,
}

fn holes(s: &Session, p: &Value, f: &super::contour::Frame) -> EngineResult<Vec<Hole>> {
    let refs = params::str_list(p, "geometry")?;
    if refs.is_empty() {
        return Err(bad("select the holes: cylinder faces, circular edges or points (`geometry`)"));
    }
    let mut out = Vec::new();
    for r in &refs {
        let (m, entity) = s.job.entity(r).ok_or_else(|| bad(format!("`geometry`: no face, edge or point `{r}`")))?;
        let axis_ok = |axis: glam::Vec3| f.z.dot(axis.as_dvec3().normalize_or_zero()).abs() > 0.999;
        let hole = match entity {
            Entity::Face(fi) => match m.faces.faces.get(fi).map(|face| face.kind) {
                Some(FaceKind::Cylinder { base, axis, length, .. }) => {
                    if !axis_ok(axis) {
                        return Err(bad(format!("{r} is not along the WCS Z axis; it cannot be drilled from here")));
                    }
                    let (a, b) = (f.local(base), f.local(base + axis * length));
                    Hole { at: a.truncate(), top: a.z.max(b.z), bottom: a.z.min(b.z) }
                }
                _ => return Err(bad(format!("{r} is not a cylinder; pick the hole's wall"))),
            },
            Entity::Edge(ei) => match m.edges.edges.get(ei).map(|e| e.kind) {
                Some(EdgeKind::Circle { center, normal, .. }) => {
                    if !axis_ok(normal) {
                        return Err(bad(format!("{r} does not face along the WCS Z axis")));
                    }
                    let c = f.local(center);
                    Hole { at: c.truncate(), top: c.z, bottom: c.z }
                }
                _ => return Err(bad(format!("{r} is not a circular edge"))),
            },
            Entity::Vertex(vi) => {
                let q = m.mesh.positions().get(vi).ok_or_else(|| bad(format!("no point `{r}`")))?;
                let c = f.local(*q);
                Hole { at: c.truncate(), top: c.z, bottom: c.z }
            }
        };
        // The same hole picked twice (its wall and its rim) is drilled once.
        if !out.iter().any(|h: &Hole| h.at.distance(hole.at) < 1.0e-3) {
            out.push(hole);
        }
        if out.len() > MAX_HOLES {
            return Err(bad(format!("a drilling toolpath holds at most {MAX_HOLES} holes")));
        }
    }
    Ok(out)
}

pub fn drill(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let m = s.library.active_machine().ok_or_else(|| bad("there is no machine; create one on the Machine tab"))?;
    let tool = match p.get("tool").and_then(Value::as_u64) {
        Some(n) => {
            m.tool(u32::try_from(n).unwrap_or(0)).ok_or_else(|| bad(format!("no tool T{n} in {}'s library", m.name)))?
        }
        None => m.tools.first().ok_or_else(|| bad(format!("{}'s tool library is empty; add a tool first", m.name)))?,
    };
    let (f, wcs) = frame(s, p)?;
    let num = |key: &str, lo: f64, hi: f64| params::opt_f64(p, key, lo, hi);
    let pick = |v: Option<f64>, fallback: f64| v.filter(|x| *x > 0.0).unwrap_or(fallback);
    let cycle = params::opt_str(p, "cycle")?.unwrap_or("drill").to_owned();
    let mut warnings = Vec::new();
    if tool.kind == "flat" && cycle != "spot" {
        warnings.push(format!("T{} is a flat end mill; check it can drill these holes", tool.number));
    }
    let feed = pick(num("feed", 0.0, 1.0e6)?, if tool.plunge_feed > 0.0 { tool.plunge_feed } else { tool.feed * 0.5 });
    if feed <= 0.0 {
        return Err(bad(format!("T{} has no feed; set `feed` or the tool's feeds", tool.number)));
    }
    if feed > m.max_feed {
        return Err(bad(format!("`feed` {feed} is above {}'s maximum feed of {}", m.name, m.max_feed)));
    }
    let pitch = num("pitch", 0.0, 100.0)?.unwrap_or(0.0);
    if cycle == "tap" && pitch <= 0.0 {
        return Err(bad("tapping needs the thread pitch (`pitch`, mm per turn)"));
    }
    let peck = pick(num("peck_depth", 0.0, 1.0e4)?, tool.diameter);
    let dwell = num("dwell", 0.0, 60.0)?.unwrap_or(if cycle == "spot" { 0.5 } else { 0.0 });
    let holes = holes(s, p, &f)?;
    let clearance = height(s, p, &f, None, "clearance", ("model_top", 15.0)).or_else(|_| {
        // Holes alone (no model height) take the highest hole top.
        let top = holes.iter().map(|h| h.top).fold(f64::NEG_INFINITY, f64::max);
        Ok::<f64, crate::error::EngineError>(top + super::contour::default_offset(s, 15.0))
    })?;
    // Retract, top and bottom are per hole when taken from the selection.
    let per_hole = |prefix: &str, default: (&str, f64), h: &Hole| -> EngineResult<f64> {
        height(s, p, &f, Some((h.top, h.bottom)), prefix, default)
    };
    let mut resolved: Vec<[f64; 4]> = Vec::with_capacity(holes.len());
    let mut retract = f64::NEG_INFINITY;
    for h in &holes {
        let r = per_hole("retract", ("selection", 2.0), h)?;
        let top = per_hole("top", ("selection", 0.0), h)?;
        let bottom = per_hole("bottom", ("selection", 0.0), h)?;
        if bottom >= top - 1.0e-9 {
            return Err(bad("a hole's bottom is not below its top; set the bottom height (e.g. a negative offset)"));
        }
        if r < top - 1.0e-9 || clearance < r - 1.0e-9 {
            return Err(bad("heights must run clearance ≥ retract ≥ top > bottom"));
        }
        retract = retract.max(r);
        resolved.push([h.at.x, h.at.y, top, bottom]);
    }
    // Nearest hole next, starting from the first.
    let mut order: Vec<[f64; 4]> = Vec::with_capacity(resolved.len());
    let mut from = resolved.first().map_or(DVec2::ZERO, |h| DVec2::new(h[0], h[1]));
    while !resolved.is_empty() {
        let i = (0..resolved.len())
            .min_by(|a, b| {
                let d = |k: usize| resolved.get(k).map_or(f64::MAX, |h| DVec2::new(h[0], h[1]).distance(from));
                d(*a).total_cmp(&d(*b))
            })
            .unwrap_or(0);
        let h = resolved.swap_remove(i);
        from = DVec2::new(h[0], h[1]);
        order.push(h);
    }
    // Moves for the viewport: down in pecks where the cycle pecks, back up to the retract plane.
    let mut b = Builder::new(f);
    let first = order.first().map_or(DVec2::ZERO, |h| DVec2::new(h[0], h[1]));
    b.at = first.extend(clearance);
    for h in &order {
        let (at, top, bottom) = (DVec2::new(h[0], h[1]), h[2], h[3]);
        b.push(MoveKind::Rapid, at.extend(b.at.z.max(retract)), 0.0)?;
        b.push(MoveKind::Rapid, at.extend(retract), 0.0)?;
        let pecks = matches!(cycle.as_str(), "peck" | "chip_break");
        let mut z = top;
        if retract > top {
            b.push(MoveKind::Rapid, at.extend(top), 0.0)?;
        }
        loop {
            let next = if pecks { (z - peck).max(bottom) } else { bottom };
            b.push(MoveKind::Plunge, at.extend(next), feed)?;
            z = next;
            if z <= bottom + 1.0e-9 {
                break;
            }
            if cycle == "peck" {
                b.push(MoveKind::Rapid, at.extend(retract), 0.0)?;
                b.push(MoveKind::Rapid, at.extend((z + 0.5).min(top)), 0.0)?;
            } else {
                b.push(MoveKind::Rapid, at.extend((z + 0.5).min(top)), 0.0)?;
                b.push(MoveKind::Rapid, at.extend(z), 0.0)?;
            }
        }
        // Taps and boring bars feed back out; drills rapid.
        let out_kind = if matches!(cycle.as_str(), "tap" | "bore") { MoveKind::Plunge } else { MoveKind::Rapid };
        b.push(out_kind, at.extend(retract), if out_kind == MoveKind::Plunge { feed } else { 0.0 })?;
    }
    let last = b.at;
    b.push(MoveKind::Rapid, last.truncate().extend(clearance), 0.0)?;
    let start = f.world(first.extend(clearance));
    if tool.spindle_rpm <= 0.0 && p.get("spindle_rpm").and_then(Value::as_f64).unwrap_or(0.0) <= 0.0 {
        warnings.push(format!("T{} has no spindle speed set", tool.number));
    }
    let name = params::opt_str(p, "name")?
        .map(crate::library::clip)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Drill T{}", tool.number));
    let mut stored = p.as_object().cloned().unwrap_or_else(Map::new);
    stored.insert("tool".into(), json!(tool.number));
    stored.remove("id");
    Ok(Toolpath {
        id,
        name,
        strategy: DRILL.into(),
        machine: m.id.clone(),
        tool: tool.number,
        wcs,
        axis: f.world_dir(DVec3::Z),
        axes: None,
        params: Value::Object(stored),
        start,
        moves: Arc::new(b.moves),
        warnings,
        drill: Some(DrillCycle { cycle, holes: order, retract, clearance, peck, dwell, feed, pitch }),
        visible: false,
    })
}
