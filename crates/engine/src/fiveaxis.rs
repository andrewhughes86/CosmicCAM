//! 4- and 5-axis output for the CL data: the rotary angles that point the tool along each tool
//! axis, the tilted work plane of 3+2 operations, and the machine's own linear positions for
//! posts whose control does not follow the tool tip itself.
//!
//! The program's WCS is taken to be aligned with the machine's axes when every rotary axis is
//! at zero (the part is set up square on the table), and the rotary centres are given relative
//! to the WCS origin (the machine's kinematics settings).

use CosmicCAM_geom::Vec3;
use CosmicCAM_geom::kinematics::{Kinematics, Layout};
use glam::DVec3;
use serde_json::{Value, json};

use crate::error::{EngineError, EngineResult};
use crate::toolpath::{MoveKind, Toolpath};

/// Deepest a move is halved while the machine's interpolation strays from the path.
const MAX_SPLITS: u32 = 10;

fn bad(m: impl Into<String>) -> EngineError {
    EngineError::InvalidParams(m.into())
}

fn round(v: f64) -> f64 {
    (v * 1.0e6).round() / 1.0e6
}

fn point(p: DVec3) -> Value {
    json!([round(p.x), round(p.y), round(p.z)])
}

/// A right-handed frame: origin and unit axes, all in some parent frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: DVec3,
    pub x: DVec3,
    pub y: DVec3,
    pub z: DVec3,
}

impl Frame {
    pub const IDENTITY: Self = Self { origin: DVec3::ZERO, x: DVec3::X, y: DVec3::Y, z: DVec3::Z };

    pub fn local(&self, p: Vec3) -> DVec3 {
        self.local_d(p.as_dvec3())
    }

    pub fn local_d(&self, p: DVec3) -> DVec3 {
        let d = p - self.origin;
        DVec3::new(d.dot(self.x), d.dot(self.y), d.dot(self.z))
    }

    pub fn local_dir(&self, d: Vec3) -> DVec3 {
        let d = d.as_dvec3();
        DVec3::new(d.dot(self.x), d.dot(self.y), d.dot(self.z)).normalize_or(DVec3::Z)
    }

    /// `other` (in the same parent) expressed in this frame.
    pub fn relative(&self, other: &Frame) -> Frame {
        let dir = |v: DVec3| DVec3::new(v.dot(self.x), v.dot(self.y), v.dot(self.z));
        Frame { origin: self.local_d(other.origin), x: dir(other.x), y: dir(other.y), z: dir(other.z) }
    }

    /// A frame whose Z is `z`, with X as close to the parent's X as it can be.
    pub fn along(z: DVec3) -> Frame {
        let z = z.normalize_or(DVec3::Z);
        let x0 = if z.x.abs() > 0.9 { DVec3::Y } else { DVec3::X };
        let x = (x0 - z * x0.dot(z)).normalize_or(DVec3::X);
        Frame { origin: DVec3::ZERO, x, y: z.cross(x), z }
    }
}

/// The ZXZ Euler angles (degrees) that turn the parent's axes onto `f`'s: about Z, then about
/// the new X, then about the new Z (as tilted-plane commands such as G68.2 take them).
pub fn euler_zxz(f: &Frame) -> [f64; 3] {
    // Columns of the rotation are the frame's axes: r[row][col].
    let r = [[f.x.x, f.y.x, f.z.x], [f.x.y, f.y.y, f.z.y], [f.x.z, f.y.z, f.z.z]];
    let beta = r[2][2].clamp(-1.0, 1.0).acos();
    let (alpha, gamma) = if beta.sin().abs() < 1.0e-9 {
        // Not tilted (or upside down): one turn about Z says it all.
        (r[1][0].atan2(r[0][0]) * if r[2][2] < 0.0 { -1.0 } else { 1.0 }, 0.0)
    } else {
        (r[0][2].atan2(-r[1][2]), r[2][0].atan2(r[2][1]))
    };
    [round(alpha.to_degrees()), round(beta.to_degrees()), round(gamma.to_degrees())]
}

/// The work plane of a 3+2 operation, for the CL data.
pub fn plane_json(plane: &Frame) -> Value {
    json!({
        "origin": point(plane.origin),
        "x": point(plane.x),
        "y": point(plane.y),
        "z": point(plane.z),
        "euler_zxz": euler_zxz(plane),
    })
}

/// A machine's rotary axes for the CL data.
pub fn kinematics_json(k: &crate::library::Kinematics) -> Value {
    json!({
        "layout": k.layout,
        "axes": std::iter::once(&k.primary).chain(k.secondary.as_ref()).map(|a| a.to_json()).collect::<Vec<_>>(),
        "primary_centre": k.primary_centre,
        "secondary_centre": k.secondary_centre,
        "pivot_length": k.pivot_length,
    })
}

/// A machine's rotary axes with what the output needs to know about them.
pub struct Rotary {
    pub kin: Kinematics,
    /// How many rotary axes (1 or 2).
    pub count: usize,
    pub names: Vec<String>,
    /// The tool's gauge length (heads).
    pub tool_length: f64,
}

impl Rotary {
    pub fn new(k: &crate::library::Kinematics, tool_length: f64) -> Self {
        Self { kin: k.geom(), count: if k.secondary.is_some() { 2 } else { 1 }, names: k.names(), tool_length }
    }

    /// The angles (radians) for tool axis `axis` (in the WCS) nearest `prev`.
    pub fn solve(&self, axis: DVec3, prev: [f64; 2], what: &str) -> EngineResult<[f64; 2]> {
        self.kin.solve(axis, prev).ok_or_else(|| {
            let travel = std::iter::once(&self.kin.primary)
                .chain(self.kin.secondary.as_ref())
                .zip(&self.names)
                .map(|(j, n)| format!("{n} {:.0}..{:.0}°", j.min.to_degrees(), j.max.to_degrees()))
                .collect::<Vec<_>>()
                .join(", ");
            bad(format!(
                "{what}: the machine cannot point the tool along [{:.3}, {:.3}, {:.3}] within its rotary travel ({travel})",
                axis.x, axis.y, axis.z
            ))
        })
    }

    pub fn degrees(&self, a: [f64; 2]) -> Value {
        json!(a.iter().take(self.count).map(|v| round(v.to_degrees())).collect::<Vec<_>>())
    }

    pub fn machine(&self, tip: DVec3, a: [f64; 2]) -> DVec3 {
        self.kin.machine_position(tip, a, self.tool_length)
    }

    /// True when, at `a`, the tool points along the machine's Z (canned cycles still work).
    pub fn spindle_vertical(&self, a: [f64; 2]) -> bool {
        match (self.kin.layout, self.kin.secondary) {
            (Layout::TableTable, _) => true,
            (Layout::HeadTable, Some(_)) => a[1].abs() < 1.0e-9,
            (Layout::HeadTable, None) => true,
            (Layout::HeadHead, _) => a[0].abs() < 1.0e-9 && a[1].abs() < 1.0e-9,
        }
    }
}

/// One output point of a multi-axis move: tip and axis in the WCS, angles.
#[derive(Clone, Copy)]
struct State {
    tip: DVec3,
    axis: DVec3,
    angles: [f64; 2],
}

fn slerp(a: DVec3, b: DVec3, t: f64) -> DVec3 {
    crate::toolpath::multiaxis::slerp(a, b, t)
}

/// The machine-interpolated move from `a` to `b`, halved until the machine's straight-line
/// interpolation of its axes keeps the tool within `tolerance` of the path.
fn split(
    r: &Rotary,
    a: State,
    b: State,
    tolerance: f64,
    depth: u32,
    out: &mut Vec<State>,
    what: &str,
) -> EngineResult<()> {
    if depth < MAX_SPLITS {
        let mid_angles = [(a.angles[0] + b.angles[0]) / 2.0, (a.angles[1] + b.angles[1]) / 2.0];
        let tip = a.tip.lerp(b.tip, 0.5);
        let interpolated = (r.machine(a.tip, a.angles) + r.machine(b.tip, b.angles)) / 2.0;
        let axis = slerp(a.axis, b.axis, 0.5);
        let axis_error = r.kin.tool_axis(mid_angles).dot(axis).clamp(-1.0, 1.0).acos();
        if r.machine(tip, mid_angles).distance(interpolated) > tolerance || axis_error > 0.5_f64.to_radians() {
            let angles = r.solve(axis, mid_angles, what)?;
            let mid = State { tip, axis, angles };
            split(r, a, mid, tolerance, depth + 1, out, what)?;
            return split(r, mid, b, tolerance, depth + 1, out, what);
        }
    }
    out.push(b);
    Ok(())
}

/// A multi-axis toolpath's moves for the CL data: lines (and rapids) in the WCS with the tool
/// axis, the rotary angles, the machine's linear position and the inverse-time feed. `prev`
/// carries the machine's rotary position from operation to operation.
pub fn multi_moves(
    tp: &Toolpath,
    wcs: &Frame,
    r: &Rotary,
    prev: &mut [f64; 2],
    tolerance: f64,
) -> EngineResult<(Vec<Value>, [f64; 2])> {
    let first_axis = wcs.local_dir(tp.axis_at(0));
    let start = State { tip: wcs.local(tp.start), axis: first_axis, angles: r.solve(first_axis, *prev, &tp.name)? };
    let mut at = start;
    let mut out = Vec::with_capacity(tp.moves.len());
    let mut pieces = Vec::new();
    for (i, m) in tp.moves.iter().enumerate() {
        let axis = wcs.local_dir(tp.axis_at(i));
        let what = format!("{} (move {})", tp.name, i + 1);
        let next = State { tip: wcs.local(m.to), axis, angles: r.solve(axis, at.angles, &what)? };
        pieces.clear();
        if m.kind == MoveKind::Rapid {
            pieces.push(next);
        } else {
            split(r, at, next, tolerance, 0, &mut pieces, &what)?;
        }
        let feed = f64::from(m.feed);
        for p in &pieces {
            let machine = r.machine(p.tip, p.angles);
            let mut v = json!({
                "to": point(p.tip),
                "axis": point(p.axis),
                "rotary": r.degrees(p.angles),
                "machine": point(machine),
            });
            if m.kind == MoveKind::Rapid {
                v["type"] = json!("rapid");
            } else {
                // Inverse time: 1 / minutes for the move, from the tip's travel (or the
                // machine's, for a move that only turns the tool about its tip).
                let mut length = p.tip.distance(at.tip);
                if length < 1.0e-6 {
                    length = machine.distance(r.machine(at.tip, at.angles));
                }
                let minutes = (length / feed.max(1.0e-6)).max(1.0e-5);
                v["type"] = json!("linear");
                v["feed"] = json!(feed);
                v["inverse_time"] = json!(round((1.0 / minutes).min(1.0e6)));
            }
            out.push(v);
            at = *p;
        }
    }
    *prev = at.angles;
    Ok((out, start.angles))
}

/// Straight moves (as the toolpath has them) in `frame`, optionally with the machine position
/// at fixed `angles`: the 3+2 output for posts without a tilted work plane.
pub fn plain_moves(tp: &Toolpath, frame: &Frame, machine: Option<(&Rotary, [f64; 2])>) -> Vec<Value> {
    tp.moves
        .iter()
        .map(|m| {
            let p = frame.local(m.to);
            let p = machine.map_or(p, |(r, a)| r.machine(p, a));
            if m.kind == MoveKind::Rapid {
                json!({ "type": "rapid", "to": point(p) })
            } else {
                json!({ "type": "linear", "to": point(p), "feed": f64::from(m.feed) })
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn euler_angles_rebuild_the_plane() {
        // A plane tilted 30° about X, then turned 90° about the part's Z.
        let tilt =
            glam::DQuat::from_rotation_z(90_f64.to_radians()) * glam::DQuat::from_rotation_x(30_f64.to_radians());
        let f = Frame { origin: DVec3::ZERO, x: tilt * DVec3::X, y: tilt * DVec3::Y, z: tilt * DVec3::Z };
        let [a, b, c] = euler_zxz(&f);
        let back = glam::DQuat::from_rotation_z(a.to_radians())
            * glam::DQuat::from_rotation_x(b.to_radians())
            * glam::DQuat::from_rotation_z(c.to_radians());
        for (v, w) in [(DVec3::X, f.x), (DVec3::Y, f.y), (DVec3::Z, f.z)] {
            assert!((back * v).distance(w) < 1.0e-6, "{a} {b} {c}");
        }
        assert!((b - 30.0).abs() < 1.0e-6);
        assert_eq!(euler_zxz(&Frame::IDENTITY), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn frames_along_an_axis_are_right_handed() {
        for z in [DVec3::Z, DVec3::X, DVec3::new(0.3, -0.5, 0.8).normalize(), -DVec3::Z] {
            let f = Frame::along(z);
            assert!(f.x.cross(f.y).distance(f.z) < 1.0e-12 && f.x.dot(f.z).abs() < 1.0e-12);
        }
    }
}
