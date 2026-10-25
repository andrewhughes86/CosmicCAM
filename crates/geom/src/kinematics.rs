//! Rotary axes of 4- and 5-axis machines: the axis angles that point the tool along a direction
//! (inverse kinematics), the direction a pair of angles gives (forward kinematics), and where
//! the linear axes must go when the control does not compensate for the rotation itself.
//!
//! Directions and positions are in the part's frame: the work coordinate system with every
//! rotary axis at zero, where the tool points along +Z. Angles are radians.
//!
//! Sign convention (ISO 841): a positive angle turns the tool, relative to the part, the same way
//! whether the head or the table moves. A table therefore turns the part by the negative angle.
//! A machine whose axis turns the other way is described with the opposite axis direction
//! (`-Z` instead of `Z`).

use glam::{DQuat, DVec3};

/// Which units carry the rotary axes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Both axes turn the table (trunnion): the primary axis tilts, carrying the secondary.
    /// With one axis, a 4-axis rotary table.
    TableTable,
    /// The primary axis turns the table, the secondary tilts the head.
    HeadTable,
    /// Both axes turn the spindle head: the primary carries the secondary.
    /// With one axis, a 4-axis tilting head.
    HeadHead,
}

/// One rotary axis: the direction it turns about and its travel (radians).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Joint {
    pub axis: DVec3,
    pub min: f64,
    pub max: f64,
}

impl Joint {
    /// The angle nearest `near` that equals `angle` modulo a full turn and lies within the
    /// travel, if any does.
    fn fit(&self, angle: f64, near: f64) -> Option<f64> {
        let tau = std::f64::consts::TAU;
        if !(angle.is_finite() && near.is_finite()) {
            return None;
        }
        let base = angle + ((near - angle) / tau).round() * tau;
        [base, base - tau, base + tau, base - 2.0 * tau, base + 2.0 * tau]
            .into_iter()
            .filter(|a| *a >= self.min - 1.0e-9 && *a <= self.max + 1.0e-9)
            .min_by(|a, b| (a - near).abs().total_cmp(&(b - near).abs()))
            .map(|a| a.clamp(self.min, self.max))
    }
}

/// A machine's rotary kinematics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kinematics {
    pub layout: Layout,
    /// The axis nearer the machine base (for [`Layout::HeadTable`], the table axis).
    pub primary: Joint,
    /// The axis the primary carries (for [`Layout::HeadTable`], the head axis); None on a
    /// 4-axis machine.
    pub secondary: Option<Joint>,
    /// A point on the primary axis, in the part frame (tables only).
    pub primary_centre: DVec3,
    /// A point on the secondary axis, in the part frame (table-table only).
    pub secondary_centre: DVec3,
    /// Heads: from the pivot (where the head turns) to the spindle gauge line, mm.
    pub pivot_length: f64,
}

/// Solutions closer than this to a singularity (the tool along the outer axis) take the
/// previous outer angle, mm-free (a sine).
const SINGULAR: f64 = 1.0e-7;

fn turn(axis: DVec3, angle: f64) -> DQuat {
    DQuat::from_axis_angle(axis.normalize_or_zero(), angle)
}

/// The signed angle about `axis` that turns `from` to `to`, both seen in the plane normal to
/// `axis`; None when either has no component in that plane.
fn angle_about(axis: DVec3, from: DVec3, to: DVec3) -> Option<f64> {
    let a = from - axis * from.dot(axis);
    let b = to - axis * to.dot(axis);
    if a.length() < SINGULAR || b.length() < SINGULAR {
        return None;
    }
    Some(axis.dot(a.cross(b)).atan2(a.dot(b)))
}

/// A joint and its slot in `[primary, secondary]`.
type Slot = Option<(Joint, usize)>;

impl Kinematics {
    /// The joints as (outer, inner) of `t = R(outer) · R(inner) · Z`, with their slots in
    /// `[primary, secondary]`.
    fn chain(&self) -> (Slot, Slot) {
        match (self.layout, self.secondary) {
            (_, None) => (Some((self.primary, 0)), None),
            (Layout::TableTable, Some(s)) => (Some((s, 1)), Some((self.primary, 0))),
            (Layout::HeadHead | Layout::HeadTable, Some(s)) => (Some((self.primary, 0)), Some((s, 1))),
        }
    }

    /// The tool direction, in the part frame, at `angles` ([primary, secondary]).
    pub fn tool_axis(&self, angles: [f64; 2]) -> DVec3 {
        let (outer, inner) = self.chain();
        let mut v = DVec3::Z;
        if let Some((j, slot)) = inner {
            v = turn(j.axis, angles.get(slot).copied().unwrap_or(0.0)) * v;
        }
        if let Some((j, slot)) = outer {
            v = turn(j.axis, angles.get(slot).copied().unwrap_or(0.0)) * v;
        }
        v
    }

    /// Every pair of raw angles ([primary, secondary], not yet fitted to the travel) that
    /// points the tool along `t`. A singular outer angle takes `prev`'s.
    fn raw_solutions(&self, t: DVec3, prev: [f64; 2]) -> Vec<[f64; 2]> {
        let (outer, inner) = self.chain();
        let mut out = Vec::new();
        let Some((o, oslot)) = outer else { return out };
        let uo = o.axis.normalize_or_zero();
        let set = |pair: &mut [f64; 2], slot: usize, v: f64| {
            if let Some(s) = pair.get_mut(slot) {
                *s = v;
            }
        };
        match inner {
            None => {
                // One axis: the tool's component along it cannot change.
                if (t.dot(uo) - DVec3::Z.dot(uo)).abs() > 1.0e-6 {
                    return out;
                }
                let a = angle_about(uo, DVec3::Z, t).unwrap_or(prev.get(oslot).copied().unwrap_or(0.0));
                let mut pair = prev;
                set(&mut pair, oslot, a);
                out.push(pair);
            }
            Some((i, islot)) => {
                let ui = i.axis.normalize_or_zero();
                // v(β) = R(inner, β)·Z must have the same component along the outer axis as t.
                let z = DVec3::Z;
                let a = z.dot(uo) - ui.dot(z) * ui.dot(uo);
                let b = ui.cross(z).dot(uo);
                let c = ui.dot(z) * ui.dot(uo);
                let d = t.dot(uo) - c;
                let r = a.hypot(b);
                if r < 1.0e-12 || d.abs() > r + 1.0e-9 {
                    return out;
                }
                let phi = b.atan2(a);
                let spread = (d / r).clamp(-1.0, 1.0).acos();
                for beta in [phi + spread, phi - spread] {
                    let v = turn(ui, beta) * z;
                    let alpha = angle_about(uo, v, t).unwrap_or(prev.get(oslot).copied().unwrap_or(0.0));
                    let mut pair = prev;
                    set(&mut pair, islot, beta);
                    set(&mut pair, oslot, alpha);
                    out.push(pair);
                }
            }
        }
        out
    }

    /// The angles ([primary, secondary]) that point the tool along `t` within the axes' travel,
    /// nearest `prev` (the angles the machine is at). None when no solution is within travel.
    pub fn solve(&self, t: DVec3, prev: [f64; 2]) -> Option<[f64; 2]> {
        let t = t.normalize_or_zero();
        if t == DVec3::ZERO {
            return None;
        }
        let joints = [Some(self.primary), self.secondary];
        let mut best: Option<([f64; 2], f64)> = None;
        for raw in self.raw_solutions(t, prev) {
            let mut fitted = [0.0; 2];
            let mut ok = true;
            for slot in 0..2 {
                let (Some(&value), Some(&previous), Some(slot_out)) =
                    (raw.get(slot), prev.get(slot), fitted.get_mut(slot))
                else {
                    continue;
                };
                match joints.get(slot).copied().flatten() {
                    Some(j) => match j.fit(value, previous) {
                        Some(v) => *slot_out = v,
                        None => ok = false,
                    },
                    None => *slot_out = 0.0,
                }
            }
            // A fitted angle must still point the tool where it should.
            if !ok || self.tool_axis(fitted).dot(t) < 1.0 - 1.0e-9 {
                continue;
            }
            let cost = (fitted[0] - prev[0]).abs() + (fitted[1] - prev[1]).abs();
            if best.is_none_or(|(_, c)| cost < c - 1.0e-12) {
                best = Some((fitted, cost));
            }
        }
        best.map(|(a, _)| a)
    }

    /// Where the linear axes go, relative to the work origin, to put the tool tip on `tip` (part
    /// frame) with the rotary axes at `angles`, when the control applies no rotation of its own.
    /// `tool_length` is the tool's gauge length (heads only).
    pub fn machine_position(&self, tip: DVec3, angles: [f64; 2], tool_length: f64) -> DVec3 {
        let [p, s] = angles;
        let length = self.pivot_length + tool_length;
        match (self.layout, self.secondary) {
            (Layout::TableTable, None) => {
                self.primary_centre + turn(self.primary.axis, -p) * (tip - self.primary_centre)
            }
            (Layout::TableTable, Some(sec)) => {
                let q = self.secondary_centre + turn(sec.axis, -s) * (tip - self.secondary_centre);
                self.primary_centre + turn(self.primary.axis, -p) * (q - self.primary_centre)
            }
            (Layout::HeadTable, sec) => {
                let q = self.primary_centre + turn(self.primary.axis, -p) * (tip - self.primary_centre);
                let head = sec.map_or(DVec3::Z, |j| turn(j.axis, s) * DVec3::Z);
                q + (head - DVec3::Z) * length
            }
            (Layout::HeadHead, sec) => {
                let head = turn(self.primary.axis, p) * sec.map_or(DVec3::Z, |j| turn(j.axis, s) * DVec3::Z);
                tip + (head - DVec3::Z) * length
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEG: f64 = std::f64::consts::PI / 180.0;

    fn joint(axis: DVec3, min: f64, max: f64) -> Joint {
        Joint { axis, min: min * DEG, max: max * DEG }
    }

    fn machine(layout: Layout, primary: Joint, secondary: Option<Joint>) -> Kinematics {
        Kinematics {
            layout,
            primary,
            secondary,
            primary_centre: DVec3::ZERO,
            secondary_centre: DVec3::ZERO,
            pivot_length: 0.0,
        }
    }

    fn trunnion() -> Kinematics {
        machine(Layout::TableTable, joint(DVec3::X, -120.0, 120.0), Some(joint(DVec3::Z, -3600.0, 3600.0)))
    }

    fn directions() -> Vec<DVec3> {
        let mut out = Vec::new();
        for i in 0..12 {
            for k in 0..8 {
                let (az, tilt) = (f64::from(i) * 30.0 * DEG, f64::from(k) * 12.0 * DEG);
                out.push(DVec3::new(tilt.sin() * az.cos(), tilt.sin() * az.sin(), tilt.cos()));
            }
        }
        out
    }

    #[test]
    fn every_layout_points_the_tool_where_asked() {
        let layouts = [
            trunnion(),
            machine(Layout::HeadTable, joint(DVec3::Z, -3600.0, 3600.0), Some(joint(DVec3::Y, -110.0, 110.0))),
            machine(Layout::HeadHead, joint(DVec3::Z, -360.0, 360.0), Some(joint(DVec3::X, -100.0, 100.0))),
            machine(Layout::TableTable, joint(DVec3::Y, -120.0, 120.0), Some(joint(DVec3::Z, -3600.0, 3600.0))),
        ];
        for k in layouts {
            for t in directions() {
                let a = k.solve(t, [0.0, 0.0]);
                assert!(a.is_some(), "{:?}: no solution for {t}", k.layout);
                let a = a.unwrap_or_default();
                assert!(k.tool_axis(a).distance(t) < 1.0e-9, "{:?} {t} -> {a:?}", k.layout);
            }
        }
    }

    #[test]
    fn trunnion_angles_match_hand_worked_values() {
        let k = trunnion();
        // Straight down: everything at zero.
        assert_eq!(k.solve(DVec3::Z, [0.0, 0.0]), Some([0.0, 0.0]));
        // Tilted 30° towards -Y: A tilts 30°, C stays.
        let t = DVec3::new(0.0, -(30.0 * DEG).sin(), (30.0 * DEG).cos());
        let a = k.solve(t, [0.0, 0.0]).unwrap_or_default();
        assert!((a[0] - 30.0 * DEG).abs() < 1.0e-9 && a[1].abs() < 1.0e-9, "{a:?}");
        // Tilted 30° towards +X: C turns a quarter (either way), A keeps a 30° tilt.
        let t = DVec3::new((30.0 * DEG).sin(), 0.0, (30.0 * DEG).cos());
        let a = k.solve(t, [0.0, 0.0]).unwrap_or_default();
        assert!((a[0].abs() - 30.0 * DEG).abs() < 1.0e-9 && (a[1].abs() - 90.0 * DEG).abs() < 1.0e-9, "{a:?}");
    }

    #[test]
    fn rotary_axes_stay_near_where_they_were() {
        let k = trunnion();
        // At the pole C is free: it keeps its angle.
        assert_eq!(k.solve(DVec3::Z, [0.0, 2.0]), Some([0.0, 2.0]));
        // A full turn of C later, the same direction is reached without unwinding.
        let t = DVec3::new(0.0, -0.5, 0.75_f64.sqrt());
        let a = k.solve(t, [0.5, std::f64::consts::TAU]).unwrap_or_default();
        assert!((a[1] - std::f64::consts::TAU).abs() < 1.0e-9, "{a:?}");
        // Beyond the tilt's travel there is no solution.
        let limited = machine(Layout::TableTable, joint(DVec3::X, -10.0, 10.0), Some(joint(DVec3::Z, -360.0, 360.0)));
        assert_eq!(limited.solve(DVec3::new(0.0, -0.5, 0.75_f64.sqrt()), [0.0, 0.0]), None);
    }

    #[test]
    fn four_axis_reaches_only_its_plane() {
        let k = machine(Layout::TableTable, joint(DVec3::X, -3600.0, 3600.0), None);
        let t = DVec3::new(0.0, -1.0, 0.0);
        let a = k.solve(t, [0.0, 0.0]).unwrap_or_default();
        assert!((a[0] - 90.0 * DEG).abs() < 1.0e-9, "{a:?}");
        assert_eq!(k.solve(DVec3::new(1.0, 0.0, 1.0), [0.0, 0.0]), None);
    }

    #[test]
    fn machine_positions_put_the_tip_on_the_part() {
        // A trunnion turning about its centre: the part point turns with the table, and the
        // turned part puts the tool axis back along the spindle.
        let mut k = trunnion();
        k.primary_centre = DVec3::new(0.0, 0.0, -50.0);
        k.secondary_centre = DVec3::new(0.0, 0.0, -50.0);
        let t = DVec3::new(0.3, -0.4, 0.75_f64.sqrt()).normalize();
        let a = k.solve(t, [0.0, 0.0]).unwrap_or_default();
        let tip = DVec3::new(10.0, 5.0, 2.0);
        let m = k.machine_position(tip, a, 0.0);
        let along = k.machine_position(tip + t * 7.0, a, 0.0);
        assert!((along - m).normalize().distance(DVec3::Z) < 1.0e-9, "{m} {along}");
        assert!(((m - k.primary_centre).length() - (tip - k.primary_centre).length()).abs() < 1.0e-9);
        // A tilting head moves its pivot so the tip stays put.
        let mut h = machine(Layout::HeadHead, joint(DVec3::Z, -360.0, 360.0), Some(joint(DVec3::X, -100.0, 100.0)));
        h.pivot_length = 150.0;
        let a = h.solve(t, [0.0, 0.0]).unwrap_or_default();
        let m = h.machine_position(tip, a, 50.0);
        // The commanded point is where the tip would be with the head straight: the pivot is
        // 200 mm above it, and 200 mm up the tool from the real tip.
        assert!((m + DVec3::Z * 200.0).distance(tip + t * 200.0) < 1.0e-9);
    }
}
