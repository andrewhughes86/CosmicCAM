//! Multi-axis finish: simultaneous 4/5-axis finishing with a ball end mill. Parallel rows run
//! over the part as in Raster; at every point the ball rests on the surface (a ball drop cutter
//! gives its centre, so the ball itself never gouges) and the tool axis is chosen by a rule:
//!
//! - `tilt`: the toolpath's Z, leaned sideways (`tilt_angle`) and along the travel (`lead_angle`)
//!   so the ball cuts off its centre, where it has no cutting speed;
//! - `normal`: the surface normal, leaned the same way;
//! - `toward_point` / `away_from_point`, `toward_line` / `away_from_line`: aimed at or away from
//!   a point or a line (`axis_point`, `axis_direction`);
//! - `fixed`: one direction (`axis_direction`).
//!
//! The axis never leans more than `max_tilt` from the toolpath's Z and turns at most
//! `max_axis_rate` degrees per mm along a row. With `avoid_collisions` the flutes above the ball
//! and the shank are checked against the part, and the axis is brought back towards Z until
//! they clear. Tool tips (centre minus one radius along the axis) are what the moves record.

use CosmicCAM_geom::collide::MeshIndex;
use CosmicCAM_geom::cutter::DropCutter;
use CosmicCAM_geom::region;
use glam::{DQuat, DVec2, DVec3};
use serde_json::Value;

use super::common::{Emitter, MAX_PASSES, common_for, heights, part_triangles};
use super::strategies::{FINISH_HEIGHTS, angle, boundary, refine, require_model, zigzag};
use super::{MoveKind, Toolpath};
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::session::Session;

/// The strategy id toolpaths record.
pub const MULTIAXIS: &str = "multiaxis";
/// Tool axis rules.
pub const TOOL_AXES: &[&str] =
    &["tilt", "normal", "toward_point", "away_from_point", "toward_line", "away_from_line", "fixed"];

/// Most points sampled along one row.
const MAX_ROW_POINTS: usize = 200_000;
/// Axis changes smaller than this (radians) between kept points may be interpolated.
const AXIS_TOLERANCE: f64 = 0.25 * std::f64::consts::PI / 180.0;

fn bad(m: impl Into<String>) -> EngineError {
    EngineError::InvalidParams(m.into())
}

/// The unit vector a fraction `t` of the way from `a` to `b` along the great circle.
pub(crate) fn slerp(a: DVec3, b: DVec3, t: f64) -> DVec3 {
    let cos = a.dot(b).clamp(-1.0, 1.0);
    let theta = cos.acos();
    if theta < 1.0e-9 {
        return a.lerp(b, t).normalize_or(a);
    }
    let s = theta.sin();
    if s < 1.0e-9 {
        // Opposite: turn through any perpendicular.
        let side = a.any_orthonormal_vector();
        return DQuat::from_axis_angle(side, theta * t) * a;
    }
    ((a * ((1.0 - t) * theta).sin() + b * (t * theta).sin()) / s).normalize_or(a)
}

fn angle_between(a: DVec3, b: DVec3) -> f64 {
    a.dot(b).clamp(-1.0, 1.0).acos()
}

/// How the tool axis is chosen, from the parameters.
struct AxisRule {
    mode: String,
    lead: f64,
    tilt: f64,
    point: DVec3,
    direction: DVec3,
    max_tilt: f64,
    /// Radians per mm.
    rate: f64,
    avoid: bool,
    /// Extra clearance around the shank, mm.
    margin: f64,
}

impl AxisRule {
    fn from(p: &Value) -> EngineResult<Self> {
        let deg = |key: &str, lo: f64, hi: f64, default: f64| -> EngineResult<f64> {
            Ok(params::opt_f64(p, key, lo, hi)?.unwrap_or(default).to_radians())
        };
        let mode = params::opt_str(p, "tool_axis")?.unwrap_or("tilt").to_owned();
        let point = params::opt_vec3(p, "axis_point")?.map(|v| v.as_dvec3());
        let direction = params::opt_vec3(p, "axis_direction")?.map(|v| v.as_dvec3().normalize_or_zero());
        let needs_point =
            matches!(mode.as_str(), "toward_point" | "away_from_point" | "toward_line" | "away_from_line");
        if needs_point && point.is_none() {
            return Err(bad(format!("`tool_axis` {mode} needs `axis_point`")));
        }
        let needs_direction = matches!(mode.as_str(), "toward_line" | "away_from_line" | "fixed");
        if needs_direction && direction.is_none_or(|d| d == DVec3::ZERO) {
            return Err(bad(format!("`tool_axis` {mode} needs `axis_direction`")));
        }
        Ok(Self {
            tilt: deg("tilt_angle", -89.0, 89.0, if mode == "tilt" { 15.0 } else { 0.0 })?,
            lead: deg("lead_angle", -89.0, 89.0, 0.0)?,
            point: point.unwrap_or(DVec3::ZERO),
            direction: direction.unwrap_or(DVec3::Z),
            max_tilt: deg("max_tilt", 0.0, 120.0, 60.0)?,
            rate: deg("max_axis_rate", 0.01, 180.0, 5.0)?,
            avoid: params::opt_bool(p, "avoid_collisions")?.unwrap_or(true),
            margin: params::opt_f64(p, "shank_clearance", 0.0, 100.0)?.unwrap_or(0.5),
            mode,
        })
    }

    /// The axis the rule asks for at a point: ball centre, surface normal there, travel
    /// direction and row direction (unit, in the frame's XY plane).
    fn axis(&self, centre: DVec3, normal: DVec3, travel: DVec3, row: DVec3) -> DVec3 {
        let lean = |base: DVec3, along: DVec3| -> DVec3 {
            // Lead: about the horizontal across the travel; tilt: about the travel.
            let along = (along - base * along.dot(base)).normalize_or_zero();
            if along == DVec3::ZERO {
                return base;
            }
            let across = base.cross(along).normalize_or_zero();
            let a = DQuat::from_axis_angle(across, self.lead) * base;
            DQuat::from_axis_angle(along, self.tilt) * a
        };
        let a = match self.mode.as_str() {
            "normal" => lean(normal, travel),
            "toward_point" => self.point - centre,
            "away_from_point" => centre - self.point,
            "toward_line" | "away_from_line" => {
                let closest = self.point + self.direction * (centre - self.point).dot(self.direction);
                if self.mode == "toward_line" { closest - centre } else { centre - closest }
            }
            "fixed" => self.direction,
            // Tilt: sideways about the row (the same side on every row), lead along the travel.
            _ => {
                let a = DQuat::from_axis_angle(row, self.tilt) * DVec3::Z;
                let across = a.cross(travel).normalize_or_zero();
                if across == DVec3::ZERO { a } else { DQuat::from_axis_angle(across, self.lead) * a }
            }
        };
        let a = a.normalize_or_zero();
        if a == DVec3::ZERO || !a.is_finite() {
            return DVec3::Z;
        }
        self.limit(a)
    }

    /// `a`, leaned back towards Z so it is at most `max_tilt` from it.
    fn limit(&self, a: DVec3) -> DVec3 {
        let off = angle_between(a, DVec3::Z);
        if off <= self.max_tilt + 1.0e-12 {
            return a;
        }
        slerp(DVec3::Z, a, self.max_tilt / off)
    }
}

/// The tool for the collision check, mm: the ball's radius, then everything above the ball's
/// centre as segments along the axis, (bottom, top, radius, cutting), from the tool's profile
/// and its holder.
struct Body {
    radius: f64,
    segments: Vec<(f64, f64, f64, bool)>,
}

impl Body {
    fn new(tool: &crate::library::Tool, holder: Option<&crate::library::Holder>) -> Self {
        let r = tool.diameter / 2.0;
        let profile = tool.profile();
        let mut segments: Vec<(f64, f64, f64, bool)> = Vec::new();
        let mut add = |h0: f64, h1: f64, radius: f64, cutting: bool| {
            if h1 <= h0 + 1.0e-9 || radius <= 0.0 {
                return;
            }
            match segments.last_mut() {
                // Neighbours of one width and kind merge into one check.
                Some(last) if (last.2 - radius).abs() < 1.0e-6 && last.3 == cutting && (last.1 - h0).abs() < 1.0e-9 => {
                    last.1 = h1;
                }
                _ => segments.push((h0, h1, radius, cutting)),
            }
        };
        // A hair above the ball's equator: the ball itself rests on the surface (the drop cutter
        // checks it).
        let start = r * 1.02;
        for w in profile.points.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (h0, h1) = (a[1].max(start), b[1]);
            if h1 > h0 {
                let cutting = b[1] <= profile.flute_top + 1.0e-9;
                add(h0, h1, a[0].max(b[0]), cutting);
            }
        }
        if let Some(h) = holder {
            for w in h.profile().windows(2) {
                let (a, b) = (w[0], w[1]);
                add(tool.overall_length + a[1], tool.overall_length + b[1], a[0].max(b[0]), false);
            }
        }
        Self { radius: r, segments }
    }

    /// True when the tool above the ball (flutes, neck, shank, holder) keeps clear of the part:
    /// cutting parts by their own radius, the rest by `margin` more.
    fn clear(&self, index: &MeshIndex, tip: DVec3, axis: DVec3, margin: f64) -> bool {
        self.segments.iter().all(|&(h0, h1, radius, cutting)| {
            let r = if cutting { radius * 0.995 - 1.0e-3 } else { radius + margin };
            index.clear(tip + axis * h0, tip + axis * h1, r)
        })
    }
}

/// One point of a row: the tool tip and axis, in the frame.
#[derive(Clone, Copy)]
struct Point {
    tip: DVec3,
    centre: DVec3,
    axis: DVec3,
}

struct Context<'a> {
    cutter: DropCutter,
    index: Option<MeshIndex>,
    rule: &'a AxisRule,
    body: Body,
    /// Ball radius with the stock left on (the drop cutter's radius).
    offset_radius: f64,
    floor: f64,
    step: f64,
    tolerance: f64,
    /// Points where even a vertical tool touches the part.
    stuck: usize,
}

impl Context<'_> {
    /// The ball's centre resting on the surface at `q` (never below the floor), and the surface
    /// normal there.
    fn probe(&self, q: DVec2) -> (DVec3, DVec3) {
        let r = self.body.radius;
        match self.cutter.ball_contact(q) {
            Some((tip_z, contact)) => {
                let centre = q.extend(tip_z + self.offset_radius);
                if centre.z - r < self.floor {
                    (q.extend(self.floor + r), DVec3::Z)
                } else {
                    (centre, (centre - contact).normalize_or(DVec3::Z))
                }
            }
            None => (q.extend(self.floor + r), DVec3::Z),
        }
    }

    fn line(&mut self, a: DVec2, b: DVec2, row: DVec3) -> Vec<Point> {
        let r = self.body.radius;
        let travel = (b - a).normalize_or_zero().extend(0.0);
        let n = ((a.distance(b) / self.step.max(1.0e-3)).ceil() as usize).clamp(1, MAX_ROW_POINTS);
        let samples: Vec<DVec2> = (0..=n).map(|i| a.lerp(b, i as f64 / n as f64)).collect();
        // The ball's centre and the surface normal under it, with points added over edges.
        let probe = |q: DVec2| self.probe(q);
        let probed = refine(&samples, &probe, |(c, _)| c.z, self.tolerance);
        let mut pts: Vec<Point> = probed
            .into_iter()
            .map(|(centre, normal)| {
                let axis = self.rule.axis(centre, normal, travel, row);
                Point { tip: centre - axis * r, centre, axis }
            })
            .collect();
        // Points the collision check moves are kept where it put them; smoothing then eases
        // their neighbours towards them, which may need checking again.
        let mut locked = vec![false; pts.len()];
        self.avoid(&mut pts, &mut locked);
        self.smooth(&mut pts, &locked);
        for _ in 0..4 {
            if !self.avoid(&mut pts, &mut locked) {
                break;
            }
            self.smooth(&mut pts, &locked);
        }
        let kept = simplify(&pts, self.tolerance);
        // Points added along the moves lean towards Z sharply; easing their neighbours towards
        // them (only ever more upright, so nothing new is hit) spreads the turn out again.
        let mut out = self.check_between(kept);
        for _ in 0..2 {
            let upright_only = vec![true; out.len()];
            self.smooth(&mut out, &upright_only);
            out = self.check_between(out);
        }
        out
    }

    /// A link along the surface from the end of one row to the start of the next: the ball
    /// follows the surface as on a row, and the axis turns evenly from the one row's last axis to
    /// the next row's first (a link's own travel would lean it some other way).
    fn link(&mut self, from: Point, to: Point, row: DVec3) -> Vec<Point> {
        let r = self.body.radius;
        let mut pts = self.line(from.centre.truncate(), to.centre.truncate(), row);
        let total: f64 = pts.windows(2).map(|w| w[0].centre.distance(w[1].centre)).sum();
        let mut walked = 0.0;
        let mut last = pts.first().map_or(from.centre, |p| p.centre);
        for p in pts.iter_mut() {
            walked += p.centre.distance(last);
            last = p.centre;
            let t = if total > 1.0e-9 { walked / total } else { 1.0 };
            p.axis = slerp(from.axis, to.axis, t);
            p.tip = p.centre - p.axis * r;
        }
        if let Some(end) = pts.last_mut() {
            *end = to;
        }
        self.check_between(pts)
    }

    /// The tool is checked where the points put it; between two of them it moves with the tip
    /// in a straight line and the axis turning evenly, and may pass through the part on the way
    /// (leaning over an edge while it drops past it). Checks along every move and adds a point,
    /// leaned back towards Z, wherever the tool would touch.
    fn check_between(&mut self, pts: Vec<Point>) -> Vec<Point> {
        let Some(index) = self.index.as_ref().filter(|_| self.rule.avoid) else { return pts };
        let r = self.body.radius;
        let clear = |tip: DVec3, axis: DVec3| self.body.clear(index, tip, axis, self.rule.margin);
        let mut out: Vec<Point> = Vec::with_capacity(pts.len());
        for p in pts {
            let Some(from) = out.last().copied() else {
                out.push(p);
                continue;
            };
            let n = ((from.centre.distance(p.centre) / self.step.max(1.0e-3)).ceil() as usize).clamp(1, 1_000);
            let pose = |k: usize| {
                let t = k as f64 / n as f64;
                (from.tip.lerp(p.tip, t), slerp(from.axis, p.axis, t))
            };
            if (1..n).all(|k| {
                let (tip, axis) = pose(k);
                clear(tip, axis)
            }) {
                out.push(p);
                continue;
            }
            // Somewhere on the way the tool touches: every sample becomes a point, the ball on
            // the straight line between the two (where the moves either side keep it; probing
            // again would drop it off a wall it climbs), and the axis leaned towards Z until the
            // tool clears.
            for k in 1..n {
                let t = k as f64 / n as f64;
                let (mut centre, axis) = (from.centre.lerp(p.centre, t), slerp(from.axis, p.axis, t));
                // Over a convex edge the straight line dips into the part: lift the ball clear
                // (the least lift, found by halving, up to one radius).
                let clear_at = |z: f64| index.distance(centre.with_z(z), centre.with_z(z), r) >= r - 1.0e-4;
                if !clear_at(centre.z) {
                    let (mut lo, mut hi) = (centre.z, centre.z + r);
                    for _ in 0..14 {
                        let mid = (lo + hi) / 2.0;
                        if clear_at(mid) { hi = mid } else { lo = mid }
                    }
                    centre.z = hi;
                }
                let mut fixed = Point { tip: centre - axis * r, centre, axis };
                for j in 0..=8 {
                    let a = slerp(axis, DVec3::Z, f64::from(j) / 8.0);
                    fixed = Point { tip: centre - a * r, centre, axis: a };
                    if clear(fixed.tip, a) {
                        break;
                    }
                }
                out.push(fixed);
            }
            out.push(p);
        }
        out
    }

    /// Brings each colliding point's axis back towards Z until the tool clears, locking the
    /// points it moves. True when it moved any.
    fn avoid(&mut self, pts: &mut [Point], locked: &mut [bool]) -> bool {
        if !self.rule.avoid {
            return false;
        }
        let Some(index) = self.index.as_ref() else { return false };
        let r = self.body.radius;
        let mut moved = false;
        for (p, lock) in pts.iter_mut().zip(locked.iter_mut()) {
            // A point already brought vertical that still touches has been counted.
            let given_up = *lock && p.axis.distance(DVec3::Z) < 1.0e-12;
            if given_up || self.body.clear(index, p.tip, p.axis, self.rule.margin) {
                continue;
            }
            moved = true;
            *lock = true;
            let start = p.axis;
            let mut found = false;
            for k in 1..=8 {
                let a = slerp(start, DVec3::Z, f64::from(k) / 8.0);
                let tip = p.centre - a * r;
                if self.body.clear(index, tip, a, self.rule.margin) {
                    p.axis = a;
                    p.tip = tip;
                    found = true;
                    break;
                }
            }
            if !found {
                p.axis = DVec3::Z;
                p.tip = p.centre - DVec3::Z * r;
                // Beside a wall a vertical shank as wide as the ball only touches it, as in any
                // 3-axis finish; it is stuck when it goes into the part.
                if !self.body.clear(index, p.tip, DVec3::Z, 0.0) {
                    self.stuck += 1;
                }
            }
        }
        moved
    }

    /// Limits how fast the axis turns along the row, both ways. A locked point (one the
    /// collision check leaned back) may only become more vertical.
    fn smooth(&self, pts: &mut [Point], locked: &[bool]) {
        let rate = self.rule.rate;
        let r = self.body.radius;
        let pass = |order: &mut dyn Iterator<Item = usize>, pts: &mut [Point]| {
            let mut prev: Option<Point> = None;
            for i in order {
                let fixed = locked.get(i).copied().unwrap_or(false);
                let Some(p) = pts.get_mut(i) else { continue };
                if let Some(q) = prev {
                    let allowed = rate * p.centre.distance(q.centre).max(1.0e-6);
                    let off = angle_between(q.axis, p.axis);
                    if off > allowed {
                        let eased = slerp(q.axis, p.axis, allowed / off);
                        if !fixed || angle_between(eased, DVec3::Z) < angle_between(p.axis, DVec3::Z) {
                            p.axis = eased;
                            p.tip = p.centre - p.axis * r;
                        }
                    }
                }
                prev = Some(*p);
            }
        };
        pass(&mut (0..pts.len()), pts);
        pass(&mut (0..pts.len()).rev(), pts);
    }
}

/// Drops points the moves between their neighbours already pass: the tip runs straight and the
/// axis turns evenly, so the ball's centre (tip plus a radius up the axis) must stay within
/// `tolerance` of where the point puts it, and the axis within [`AXIS_TOLERANCE`].
fn simplify(pts: &[Point], tolerance: f64) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(pts.len());
    for (i, &q) in pts.iter().enumerate() {
        if i + 1 == pts.len() || out.is_empty() {
            out.push(q);
            continue;
        }
        let (Some(&a), Some(&b)) = (out.last(), pts.get(i + 1)) else { continue };
        let ab = b.tip - a.tip;
        let t = if ab.length_squared() > 1.0e-18 {
            ((q.tip - a.tip).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let axis = slerp(a.axis, b.axis, t);
        let radius = a.centre.distance(a.tip);
        let off_path = (a.tip + ab * t + axis * radius).distance(q.centre) > tolerance;
        let off_axis = angle_between(axis, q.axis) > AXIS_TOLERANCE;
        if off_path || off_axis {
            out.push(q);
        }
    }
    out
}

pub fn multiaxis(s: &Session, p: &Value, id: String) -> EngineResult<Toolpath> {
    let c = common_for(s, p, 0.1, 1.0, crate::tooling::BALL_ENDED)?;
    let machine = s.library.machine(&c.machine).ok_or_else(|| bad("the toolpath's machine no longer exists"))?;
    if machine.kinematics.is_none() {
        return Err(bad(format!(
            "{} is a 3-axis mill; multi-axis finishing needs a 4- or 5-axis machine (Machine tab)",
            machine.name
        )));
    }
    let rule = AxisRule::from(p)?;
    let mut warnings = Vec::new();
    let tris = part_triangles(s, &c.frame);
    require_model(&tris)?;
    let (area, selection) = boundary(s, p, &c, "model", &mut warnings)?;
    let [clearance, retract, feed_height, top, bottom] = heights(s, p, &c.frame, selection, &FINISH_HEIGHTS)?;
    let r = c.radius();
    let stock = c.stock_axial.max(-0.9 * r);
    let offset_radius = r + stock;
    let holder = s.library.holder_of(&c.tool);
    let body = Body::new(&c.tool, holder);
    let index = rule.avoid.then(|| MeshIndex::new(tris.clone(), body.radius.max(1.0)));
    let step =
        params::opt_f64(p, "sample_step", 0.0, 1.0e3)?.filter(|v| *v > 0.0).unwrap_or((r * 0.25).clamp(0.05, 1.0));
    let mut ctx = Context {
        cutter: DropCutter::ball(tris, offset_radius),
        index,
        rule: &rule,
        body,
        offset_radius,
        floor: bottom - stock.min(0.0),
        step,
        tolerance: c.tolerance,
        stuck: 0,
    };
    let a = angle(p)?;
    let row_dir = DVec3::new(a.cos(), a.sin(), 0.0);
    let rows = region::hatch(&area, c.stepover, a, MAX_PASSES);
    let zz = zigzag(p)?;
    let mut e = Emitter::new(c.frame, clearance, retract, feed_height - top);
    let mut started = false;
    let mut previous: Option<Point> = None;
    for (k, row) in rows.iter().enumerate() {
        let flip = zz && k % 2 == 1;
        let segments: Vec<(DVec2, DVec2)> =
            if flip { row.iter().rev().map(|(a0, b0)| (*b0, *a0)).collect() } else { row.clone() };
        for (a0, b0) in segments {
            let pts = ctx.line(a0, b0, row_dir);
            let Some(first) = pts.first().copied() else { continue };
            // Zigzag rows join along the surface: the link is worked out like a row.
            let near = previous
                .filter(|prev| prev.centre.truncate().distance(first.centre.truncate()) <= c.stepover * 1.5 + 1.0e-6);
            if let (true, true, Some(prev)) = (zz, started, near) {
                for q in ctx.link(prev, first, row_dir).iter().skip(1) {
                    e.b.push_axis(MoveKind::Link, q.tip, c.feed, q.axis)?;
                }
            } else {
                if started {
                    // Up to clearance along the toolpath's Z, turn there, then over and down.
                    let at = e.b.at;
                    e.b.push(MoveKind::Rapid, at.truncate().extend(clearance.max(at.z)), 0.0)?;
                    e.b.push_axis(MoveKind::Rapid, first.tip.truncate().extend(clearance.max(at.z)), 0.0, first.axis)?;
                } else {
                    e.b.at = first.tip.truncate().extend(clearance);
                    e.b.axis = first.axis;
                    e.b.push_axis(MoveKind::Rapid, first.tip.truncate().extend(retract), 0.0, first.axis)?;
                    started = true;
                }
                let down_to = (first.tip.z + e.gap).min(e.b.at.z);
                e.b.push_axis(MoveKind::Rapid, first.tip.truncate().extend(down_to), 0.0, first.axis)?;
                e.b.push_axis(MoveKind::Plunge, first.tip, c.plunge_feed, first.axis)?;
            }
            for q in pts.iter().skip(1) {
                e.b.push_axis(MoveKind::Cut, q.tip, c.feed, q.axis)?;
            }
            previous = pts.last().copied();
        }
    }
    if !started {
        return Err(bad("there is nothing to cut with these settings"));
    }
    if ctx.stuck > 0 {
        warnings.push(format!(
            "at {} points the shank goes into the part even with the tool vertical: the walls are deeper than the flutes are long; use a longer-fluted tool",
            ctx.stuck
        ));
    }
    if rule.avoid && holder.is_none() {
        warnings.push(format!(
            "T{} has no holder, so only the flutes, neck and shank are checked for collisions; give it one in the tool library",
            c.tool.number
        ));
    }
    e.mark_started();
    e.finish(p, c, id, MULTIAXIS, "Multi-axis finish", warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(mode: &str, tilt: f64, lead: f64) -> AxisRule {
        AxisRule {
            mode: mode.into(),
            lead: lead.to_radians(),
            tilt: tilt.to_radians(),
            point: DVec3::new(0.0, 0.0, 100.0),
            direction: DVec3::new(0.0, -1.0, 1.0).normalize(),
            max_tilt: 60_f64.to_radians(),
            rate: 5_f64.to_radians(),
            avoid: true,
            margin: 0.5,
        }
    }

    #[test]
    fn axis_rules_lean_the_way_they_say() {
        let (centre, n, travel, row) = (DVec3::new(10.0, 0.0, 5.0), DVec3::Z, DVec3::X, DVec3::X);
        // Tilt 15° about the row (+X): the top leans to -Y... to the right of the row.
        let a = rule("tilt", 15.0, 0.0).axis(centre, n, travel, row);
        assert!((angle_between(a, DVec3::Z).to_degrees() - 15.0).abs() < 1.0e-9);
        assert!(a.x.abs() < 1.0e-12 && a.y < 0.0, "{a}");
        // Lead 10° leans the top forward along the travel.
        let a = rule("tilt", 0.0, 10.0).axis(centre, n, travel, row);
        assert!(a.x > 0.0 && (angle_between(a, DVec3::Z).to_degrees() - 10.0).abs() < 1.0e-9, "{a}");
        // Towards a point above the origin.
        let a = rule("toward_point", 0.0, 0.0).axis(centre, n, travel, row);
        assert!(a.distance((DVec3::new(0.0, 0.0, 100.0) - centre).normalize()) < 1.0e-12);
        // Never more than max_tilt from Z.
        let a = rule("normal", 0.0, 0.0).axis(centre, DVec3::X, travel, DVec3::Y);
        assert!((angle_between(a, DVec3::Z).to_degrees() - 60.0).abs() < 1.0e-9, "{a}");
        // Fixed.
        let a = rule("fixed", 0.0, 0.0).axis(centre, n, travel, row);
        assert!(a.distance(DVec3::new(0.0, -1.0, 1.0).normalize()) < 1.0e-12);
    }

    #[test]
    fn slerp_runs_along_the_great_circle() {
        let m = slerp(DVec3::Z, DVec3::X, 0.5);
        assert!(m.distance(DVec3::new(1.0, 0.0, 1.0).normalize()) < 1.0e-12);
        assert!(slerp(DVec3::Z, DVec3::Z, 0.3).distance(DVec3::Z) < 1.0e-12);
    }
}
