//! 2D strategies built on the contour, and thread milling:
//!
//! - **Chamfer**: breaks picked edges with a chamfer (or engraving) tool to a chamfer width,
//!   the tip a little below it, worked out from the tool's angle and tip.
//! - **Slot**: the tool's centre along the picked edges, down in steps.
//! - **Trace**: the tool's centre along the picked edges at one depth (engraving, scribing).
//! - **Thread**: thread milling of holes or bosses picked as cylinders or circular edges.

use std::sync::Arc;

use glam::{DVec2, DVec3};
use serde_json::{Map, Value, json};

use super::common::{bad, height};
use super::contour::{Builder, Variant, frame, generate_as, pick_tool};
use super::drill::holes;
use super::{MoveKind, Toolpath};
use crate::error::EngineResult;
use crate::params;
use crate::session::Session;

pub const CHAMFER: Variant =
    Variant { strategy: "chamfer", name: "Chamfer", kinds: &["chamfer", "engraving", "spot_drill", "countersink"] };
pub const SLOT: Variant = Variant { strategy: "slot", name: "Slot", kinds: &["flat", "bull", "ball", "slot"] };
pub const TRACE: Variant = Variant {
    strategy: "trace",
    name: "Trace",
    kinds: &["engraving", "chamfer", "ball", "tapered_ball", "flat", "bull"],
};
pub const THREAD: &str = "thread";
/// Thread kinds and hands.
pub const THREAD_TYPES: &[&str] = &["internal", "external"];
pub const HANDS: &[&str] = &["right", "left"];

/// `p` with the keys in `set` replaced.
fn with(p: &Value, set: &[(&str, Value)]) -> Value {
    let mut m = p.as_object().cloned().unwrap_or_else(Map::new);
    for (k, v) in set {
        m.insert((*k).to_owned(), v.clone());
    }
    Value::Object(m)
}

pub fn chamfer(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let (_, tool) = pick_tool(s, p, CHAMFER.kinds)?;
    if tool.tip_angle <= 0.0 || tool.tip_angle >= 180.0 {
        return Err(bad(format!("T{} has no point angle for a chamfer", tool.number)));
    }
    let half = (tool.tip_angle / 2.0).to_radians();
    let tan = half.tan();
    let width = params::opt_f64(p, "chamfer_width", 0.0, 1.0e3)?.unwrap_or(0.5);
    let tip_offset = params::opt_f64(p, "tip_offset", 0.0, 1.0e3)?.unwrap_or(0.25);
    let rt = tool.tip_diameter.max(0.0) / 2.0;
    let r = tool.diameter / 2.0;
    // The cone meets the edge `width` across; the tip sits `tip_offset` below the chamfer.
    let depth = width / tan + tip_offset;
    if rt + width + tip_offset * tan > r + 1.0e-9 {
        return Err(bad(format!(
            "T{} (Ø{:.3}) is too small for a {width:.3} chamfer with that tip offset",
            tool.number, tool.diameter
        )));
    }
    let stock = params::opt_f64(p, "stock_radial", -1.0e3, 1.0e3)?.unwrap_or(0.0);
    // The contour offsets the centre by its radius plus radial stock: make that the distance
    // from the edge to the centre of the cone touching the chamfer.
    let work = with(
        p,
        &[
            ("stock_radial", json!(rt + tip_offset * tan - r + stock)),
            ("bottom_from", json!(params::opt_str(p, "bottom_from")?.unwrap_or("selection"))),
            ("bottom_offset", json!(-depth)),
            ("multiple_depths", json!(false)),
        ],
    );
    generate_as(s, &work, id, &CHAMFER, p)
}

pub fn slot(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let work = with(
        p,
        &[("side", json!("on")), ("multiple_depths", json!(params::opt_bool(p, "multiple_depths")?.unwrap_or(true)))],
    );
    generate_as(s, &work, id, &SLOT, p)
}

pub fn trace(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let depth = params::opt_f64(p, "depth", 0.0, 1.0e3)?.unwrap_or(0.2);
    let mut set = vec![("side", json!("on")), ("multiple_depths", json!(false))];
    if p.get("bottom_offset").is_none() {
        set.push(("bottom_from", json!(params::opt_str(p, "bottom_from")?.unwrap_or("selection"))));
        set.push(("bottom_offset", json!(-depth)));
    }
    generate_as(s, &with(p, &set), id, &TRACE, p)
}

/// Default heights: each thread's own top and bottom.
pub const THREAD_HEIGHTS: [(&str, &str, f64); 4] = [
    ("clearance", "model_top", 15.0),
    ("retract", "selection", 2.0),
    ("top", "selection", 0.0),
    ("bottom", "selection", 0.0),
];

pub fn thread(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let (m, tool) = pick_tool(s, p, &["thread_mill"])?;
    let (f, wcs) = frame(s, p)?;
    let num = |key: &str, lo: f64, hi: f64| params::opt_f64(p, key, lo, hi);
    let pick = |v: Option<f64>, fallback: f64| v.filter(|x| *x > 0.0).unwrap_or(fallback);
    let r = tool.diameter / 2.0;
    let feed = pick(num("feed", 0.0, 1.0e6)?, tool.feed);
    if feed <= 0.0 {
        return Err(bad(format!("T{} has no cutting feed; set `feed` or the tool's feeds", tool.number)));
    }
    if feed > m.max_feed {
        return Err(bad(format!("`feed` {feed} is above {}'s maximum feed of {}", m.name, m.max_feed)));
    }
    let pitch = pick(num("pitch", 0.0, 100.0)?, tool.thread_pitch);
    if pitch <= 0.0 {
        return Err(bad("set the thread `pitch` (or the thread mill's pitch)"));
    }
    let external = params::opt_str(p, "thread_type")?.unwrap_or("internal") == "external";
    let right = params::opt_str(p, "hand")?.unwrap_or("right") == "right";
    let climb = params::opt_str(p, "direction")?.unwrap_or("climb") == "climb";
    let passes = p.get("radial_passes").and_then(Value::as_u64).map_or(1, |n| n.clamp(1, 20) as usize);
    let spring = params::opt_bool(p, "spring_pass")?.unwrap_or(false);
    let tolerance = pick(num("tolerance", 0.0, 1.0)?, 0.01);
    // A 60° thread's depth: external threads are cut deeper than internal ones.
    let thread_depth = pitch * if external { 0.6134 } else { 0.5413 };
    let holes = holes(s, p, &f)?;
    let clearance = height(s, p, &f, None, "clearance", ("model_top", 15.0)).or_else(|_| {
        let top = holes.iter().map(|h| h.top).fold(f64::NEG_INFINITY, f64::max);
        Ok::<f64, crate::error::EngineError>(top + super::contour::default_offset(s, 15.0))
    })?;
    // Inside, climb runs counter-clockwise; outside, clockwise. A right-hand thread rises
    // counter-clockwise, so the direction of travel sets whether the tool climbs or descends.
    let ccw = climb != external;
    let rising = ccw == right;
    let mut b = Builder::new(f);
    let mut started = false;
    let mut warnings = Vec::new();
    for h in &holes {
        if h.radius <= 0.0 {
            return Err(bad("pick the threads' cylinders or rims: a point has no diameter"));
        }
        // The hole is drawn at the tap drill (minor) size; a boss at the major diameter.
        let major = pick(
            num("major_diameter", 0.0, 1.0e4)?,
            if external { 2.0 * h.radius } else { 2.0 * h.radius + 1.0825 * pitch },
        );
        let final_radius = if external { major / 2.0 - thread_depth + r } else { major / 2.0 - r };
        if !external && final_radius <= r * 0.1 {
            return Err(bad(format!("T{} (Ø{:.3}) is too big for a Ø{major:.3} thread", tool.number, tool.diameter)));
        }
        let sel = Some((h.top, h.bottom));
        let retract = height(s, p, &f, sel, "retract", ("selection", 2.0))?;
        let top = height(s, p, &f, sel, "top", ("selection", 0.0))?;
        let bottom = height(s, p, &f, sel, "bottom", ("selection", 0.0))?;
        if bottom >= top - 1.0e-9 {
            return Err(bad("a thread's bottom is not below its top; set the bottom height"));
        }
        // Radial passes ease into the full depth; a spring pass repeats the last.
        let mut radii: Vec<f64> = (1..=passes)
            .map(|k| {
                let frac = k as f64 / passes as f64;
                if external {
                    final_radius + thread_depth * (1.0 - frac)
                } else {
                    final_radius - thread_depth * (1.0 - frac)
                }
            })
            .collect();
        if spring && let Some(&last) = radii.last() {
            radii.push(last);
        }
        let step = CosmicCAM_geom::offset::arc_step(final_radius.max(0.1), tolerance).max(1.0e-3);
        let per_turn = ((std::f64::consts::TAU / step).ceil() as usize).clamp(16, 4096);
        // Whole turns, ending (or starting) exactly at the bottom so a blind hole is not cut deeper.
        let turns = ((top - bottom) / pitch).ceil().max(1.0);
        let (z0, z1) = if rising { (bottom, bottom + turns * pitch) } else { (bottom + turns * pitch, bottom) };
        let sense = if ccw { 1.0 } else { -1.0 };
        let point = |radius: f64, a: f64| h.at + DVec2::new(a.cos(), a.sin()) * radius;
        if !started {
            b.at = h.at.extend(clearance);
            started = true;
        }
        // Inside the hole the tool waits at its centre; outside a boss, beyond it.
        let outer_lead = r * 0.75;
        let wait = |radius: f64| if external { point(radius + 2.0 * outer_lead, 0.0) } else { h.at };
        b.push(MoveKind::Rapid, wait(final_radius).extend(b.at.z.max(retract)), 0.0)?;
        b.push(MoveKind::Rapid, wait(final_radius).extend(retract), 0.0)?;
        for &radius in &radii {
            let start = wait(radius);
            b.push(MoveKind::Plunge, start.extend(z0), feed)?;
            // A half-circle lead onto the helix, tangent where it joins.
            let lr = if external { outer_lead } else { radius / 2.0 };
            let lead_c = point(radius + if external { lr } else { -lr }, 0.0);
            let lead_from_a: f64 = if external { 0.0 } else { std::f64::consts::PI };
            let leads = (per_turn / 4).max(4);
            for k in 1..=leads {
                let t = k as f64 / leads as f64;
                let a = lead_from_a + if external { -sense } else { sense } * std::f64::consts::PI * t;
                let q = lead_c + DVec2::new(a.cos(), a.sin()) * lr;
                b.push(MoveKind::Lead, q.extend(z0), feed)?;
            }
            let n = ((turns * per_turn as f64).ceil() as usize).clamp(1, 2_000_000);
            for k in 1..=n {
                let t = k as f64 / n as f64;
                let a = sense * std::f64::consts::TAU * turns * t;
                b.push(MoveKind::Cut, point(radius, a).extend(z0 + (z1 - z0) * t), feed)?;
            }
            // Off the wall the way it came on, and back to the start.
            for k in 1..=leads {
                let t = k as f64 / leads as f64;
                let a = if external { std::f64::consts::PI } else { 0.0 }
                    + if external { -sense } else { sense } * std::f64::consts::PI * t;
                let q = lead_c + DVec2::new(a.cos(), a.sin()) * lr;
                b.push(MoveKind::Lead, q.extend(z1), feed)?;
            }
            b.push(MoveKind::Rapid, start.extend(retract), 0.0)?;
        }
    }
    let last = b.at;
    b.push(MoveKind::Rapid, last.truncate().extend(clearance), 0.0)?;
    let first = holes.first().map_or(DVec2::ZERO, |h| h.at);
    let start = f.world(first.extend(clearance));
    if tool.spindle_rpm <= 0.0 && p.get("spindle_rpm").and_then(Value::as_f64).unwrap_or(0.0) <= 0.0 {
        warnings.push(format!("T{} has no spindle speed set", tool.number));
    }
    let name = params::opt_str(p, "name")?
        .map(crate::library::clip)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Thread T{}", tool.number));
    let mut stored = p.as_object().cloned().unwrap_or_else(Map::new);
    stored.insert("tool".into(), json!(tool.number));
    stored.remove("id");
    Ok(Toolpath {
        id,
        name,
        strategy: THREAD.into(),
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
