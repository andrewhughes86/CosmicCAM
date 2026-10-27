//! Helical boring: holes picked as cylinder faces or circular edges, each opened up by an end
//! mill spiralling down inside it, a full circle at the bottom to clean the floor, then out
//! through the centre. A hole wider than the tool can clear in one helix gets more helices,
//! from the middle out, so no core is left standing.

use std::sync::Arc;

use glam::{DVec2, DVec3};
use serde_json::{Map, Value, json};

use super::common::{bad, height};
use super::contour::{Builder, frame, pick_tool};
use super::drill::holes;
use super::{MoveKind, Toolpath};
use crate::error::EngineResult;
use crate::params;
use crate::session::Session;

pub const BORE: &str = "bore";

/// Default heights: (prefix, reference, offset mm). `selection` is each hole's own top and bottom.
pub const BORE_HEIGHTS: [(&str, &str, f64); 4] = [
    ("clearance", "model_top", 15.0),
    ("retract", "selection", 2.0),
    ("top", "selection", 0.0),
    ("bottom", "selection", 0.0),
];

/// Most helices one hole gets.
const MAX_RINGS: usize = 200;

pub fn bore(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let (m, tool) = pick_tool(s, p, &["flat", "bull", "ball"])?;
    let (f, wcs) = frame(s, p)?;
    let num = |key: &str, lo: f64, hi: f64| params::opt_f64(p, key, lo, hi);
    let pick = |v: Option<f64>, fallback: f64| v.filter(|x| *x > 0.0).unwrap_or(fallback);
    let r = tool.diameter / 2.0;
    let feed = pick(num("feed", 0.0, 1.0e6)?, tool.feed);
    if feed <= 0.0 {
        return Err(bad(format!("T{} has no cutting feed; set `feed` or the tool's feeds", tool.number)));
    }
    let plunge =
        pick(num("plunge_feed", 0.0, 1.0e6)?, if tool.plunge_feed > 0.0 { tool.plunge_feed } else { feed * 0.5 });
    for (name, v) in [("feed", feed), ("plunge_feed", plunge)] {
        if v > m.max_feed {
            return Err(bad(format!("`{name}` {v} is above {}'s maximum feed of {}", m.name, m.max_feed)));
        }
    }
    let pitch = pick(num("pitch", 0.0, 1.0e3)?, (tool.diameter * 0.15).clamp(0.1, 3.0));
    let stepover =
        pick(num("stepover", 0.0, 1.0e3)?, if tool.stepover > 0.0 { tool.stepover } else { tool.diameter * 0.4 });
    let stock = num("stock_radial", -1.0e3, 1.0e3)?.unwrap_or(0.0);
    let tolerance = pick(num("tolerance", 0.0, 1.0)?, 0.01);
    let climb = params::opt_str(p, "direction")?.unwrap_or("climb") == "climb";
    let holes = holes(s, p, &f)?;
    let clearance = height(s, p, &f, None, "clearance", ("model_top", 15.0)).or_else(|_| {
        let top = holes.iter().map(|h| h.top).fold(f64::NEG_INFINITY, f64::max);
        Ok::<f64, crate::error::EngineError>(top + super::contour::default_offset(s, 15.0))
    })?;
    let mut warnings: Vec<String> = Vec::new();
    let mut b = Builder::new(f);
    let mut started = false;
    // Inside a hole, climb milling runs counter-clockwise (with the spindle turning clockwise).
    let turn = if climb { 1.0 } else { -1.0 };
    for h in &holes {
        if h.radius <= 0.0 {
            return Err(bad("pick the holes' walls or rims: a point has no diameter to bore"));
        }
        let path = h.radius - r - stock;
        if path <= 1.0e-3 {
            return Err(bad(format!(
                "T{} (Ø{:.3}) does not fit inside the Ø{:.3} hole with that stock left",
                tool.number,
                tool.diameter,
                2.0 * h.radius
            )));
        }
        let sel = Some((h.top, h.bottom));
        let retract = height(s, p, &f, sel, "retract", ("selection", 2.0))?;
        let top = height(s, p, &f, sel, "top", ("selection", 0.0))?;
        let bottom = height(s, p, &f, sel, "bottom", ("selection", 0.0))?;
        if bottom >= top - 1.0e-9 {
            return Err(bad("a hole's bottom is not below its top; set the bottom height (e.g. a negative offset)"));
        }
        if retract < top - 1.0e-9 || clearance < retract - 1.0e-9 {
            return Err(bad("heights must run clearance ≥ retract ≥ top > bottom"));
        }
        // Helix radii from the middle out: the first clears the centre (no wider than the tool's
        // radius), the rest at most a stepover apart.
        let first = path.min(r * 0.9);
        let rings = (((path - first) / stepover).ceil() as usize).min(MAX_RINGS);
        let radii: Vec<f64> = (0..=rings).map(|k| first + (path - first) * k as f64 / rings.max(1) as f64).collect();
        let radii = if rings == 0 { vec![path] } else { radii };
        let step = CosmicDMS_geom::offset::arc_step(path, tolerance).max(1.0e-3);
        let segments_per_turn = ((std::f64::consts::TAU / step).ceil() as usize).clamp(16, 4_096);
        let point = |radius: f64, a: f64| h.at + DVec2::new(a.cos(), a.sin()) * radius;
        if !started {
            b.at = h.at.extend(clearance);
            started = true;
        }
        b.push(MoveKind::Rapid, h.at.extend(b.at.z.max(retract)), 0.0)?;
        b.push(MoveKind::Rapid, h.at.extend(retract), 0.0)?;
        for &radius in &radii {
            // Out to the ring above the material, then down the helix.
            b.push(MoveKind::Plunge, h.at.extend(top), plunge)?;
            b.push(MoveKind::Lead, point(radius, 0.0).extend(top), feed)?;
            let turns = ((top - bottom) / pitch).max(0.0);
            let n = ((turns * segments_per_turn as f64).ceil() as usize).clamp(1, 2_000_000);
            for k in 1..=n {
                let t = k as f64 / n as f64;
                let a = turn * std::f64::consts::TAU * turns * t;
                b.push(MoveKind::Cut, point(radius, a).extend(top + (bottom - top) * t), feed)?;
            }
            // A full circle at the bottom flattens the floor and trues the wall.
            let end = turn * std::f64::consts::TAU * turns;
            for k in 1..=segments_per_turn {
                let a = end + turn * std::f64::consts::TAU * k as f64 / segments_per_turn as f64;
                b.push(MoveKind::Cut, point(radius, a).extend(bottom), feed)?;
            }
            // Off the wall to the middle, and back up for the next ring.
            b.push(MoveKind::Lead, h.at.extend(bottom), feed)?;
            b.push(MoveKind::Rapid, h.at.extend(top.max(retract)), 0.0)?;
        }
        b.push(MoveKind::Rapid, h.at.extend(retract), 0.0)?;
    }
    let last = b.at;
    b.push(MoveKind::Rapid, last.truncate().extend(clearance), 0.0)?;
    let first = holes.first().map_or(DVec2::ZERO, |h| h.at);
    let start = f.world(first.extend(clearance));
    if tool.spindle_rpm <= 0.0 && p.get("spindle_rpm").and_then(Value::as_f64).unwrap_or(0.0) <= 0.0 {
        warnings.push(format!("T{} has no spindle speed set", tool.number));
    }
    warnings.sort();
    warnings.dedup();
    let name = params::opt_str(p, "name")?
        .map(crate::library::clip)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Helical bore T{}", tool.number));
    let mut stored = p.as_object().cloned().unwrap_or_else(Map::new);
    stored.insert("tool".into(), json!(tool.number));
    stored.remove("id");
    Ok(Toolpath {
        id,
        name,
        strategy: BORE.into(),
        machine: m.id.clone(),
        tool: tool.number,
        wcs,
        axis: f.world_dir(DVec3::Z),
        axes: None,
        params: Value::Object(stored),
        start,
        moves: Arc::new(b.moves),
        warnings,
        drill: None,
        visible: false,
        trims: Vec::new(),
        untrimmed: None,
    })
}
