//! Arc fitting for G-code: runs of short line segments that lie on a circle in the XY plane
//! become one arc (G2/G3), with Z changing evenly along it for helices.
//!
//! An arc replaces points `i..=j` when every point lies within the tolerance of the circle
//! through the first, middle and last points, each segment is a chord bowing away from the arc
//! by no more than the tolerance (so a polygon whose corners happen to sit on a circle stays a
//! polygon), the path turns one way all along, Z changes in proportion to the angle swept, and
//! the sweep stays below `max_sweep`. Fitting is greedy: each arc grows as far as it can.

use glam::{DVec2, DVec3};

/// One output move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    /// A straight move to the point.
    Line(DVec3),
    /// An arc to `to` about `center` (Z ignored), clockwise seen from +Z when `cw`.
    Arc { to: DVec3, center: DVec2, cw: bool },
}

/// Limits for fitting.
#[derive(Clone, Copy, Debug)]
pub struct ArcLimits {
    /// Largest distance between the points and the arc.
    pub tolerance: f64,
    pub min_radius: f64,
    pub max_radius: f64,
    /// Largest angle one arc sweeps, radians (at most just under a full turn).
    pub max_sweep: f64,
    /// Fewest segments an arc must replace.
    pub min_segments: usize,
}

impl Default for ArcLimits {
    fn default() -> Self {
        Self { tolerance: 0.01, min_radius: 0.05, max_radius: 5000.0, max_sweep: std::f64::consts::PI, min_segments: 3 }
    }
}

/// How far a straight-looking start is followed to see whether it curves.
const MAX_STRAIGHT_LOOKAHEAD: usize = 64;

/// Most points one arc may replace (keeps fitting fast on long paths).
const MAX_ARC_POINTS: usize = 2048;

fn circle(a: DVec2, b: DVec2, c: DVec2) -> Option<(DVec2, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1.0e-12 {
        return None;
    }
    let (a2, b2, c2) = (a.length_squared(), b.length_squared(), c.length_squared());
    let ux = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let uy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    let centre = DVec2::new(ux, uy);
    let r = centre.distance(a);
    r.is_finite().then_some((centre, r))
}

/// What a run of points makes.
enum Run {
    /// An arc about this centre, clockwise when true.
    Arc(DVec2, bool),
    /// Straight within the tolerance so far: a longer run may still curve.
    Straight,
    /// Neither.
    No,
}

/// Whether `pts` (start, …, end) make an arc within the limits.
fn try_arc(pts: &[DVec3], lim: &ArcLimits) -> Run {
    let n = pts.len();
    let (Some(first), Some(last), Some(mid)) = (pts.first(), pts.last(), pts.get(n / 2)) else { return Run::No };
    if n < lim.min_segments + 1 {
        return Run::No;
    }
    // A run that is straight within the tolerance is better left as lines.
    let chord = last.truncate() - first.truncate();
    let len = chord.length();
    if len > 1.0e-12
        && pts.iter().all(|p| (p.truncate() - first.truncate()).perp_dot(chord).abs() / len <= lim.tolerance)
    {
        return Run::Straight;
    }
    match fits(pts, first, last, mid, lim) {
        Some((c, cw)) => Run::Arc(c, cw),
        None => Run::No,
    }
}

fn fits(pts: &[DVec3], first: &DVec3, last: &DVec3, mid: &DVec3, lim: &ArcLimits) -> Option<(DVec2, bool)> {
    let n = pts.len();
    let (centre, r) = circle(first.truncate(), mid.truncate(), last.truncate())?;
    if !(lim.min_radius..=lim.max_radius).contains(&r) {
        return None;
    }
    // One turning direction all along, and the angle swept from the start.
    let angle = |p: DVec2| (p - centre).to_angle();
    let mut swept = 0.0f64;
    let mut sign = 0.0f64;
    let mut sweeps = Vec::with_capacity(n);
    sweeps.push(0.0);
    for w in pts.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { return None };
        let mut d = angle(b.truncate()) - angle(a.truncate());
        if d > std::f64::consts::PI {
            d -= std::f64::consts::TAU;
        } else if d < -std::f64::consts::PI {
            d += std::f64::consts::TAU;
        }
        if d.abs() < 1.0e-12 {
            return None;
        }
        if sign == 0.0 {
            sign = d.signum();
        } else if d.signum() != sign {
            return None;
        }
        swept += d;
        sweeps.push(swept);
    }
    if swept.abs() > lim.max_sweep {
        return None;
    }
    // An arc that bows less than the tolerance over its length is a line in disguise.
    if r * (1.0 - (swept.abs() / 2.0).cos()) <= lim.tolerance {
        return None;
    }
    // Points and segment midpoints on the circle, and Z in step with the angle.
    let dz = last.z - first.z;
    for (k, p) in pts.iter().enumerate() {
        if (p.truncate().distance(centre) - r).abs() > lim.tolerance {
            return None;
        }
        let expected_z = first.z + dz * sweeps.get(k).copied().unwrap_or(0.0) / swept;
        if (p.z - expected_z).abs() > lim.tolerance {
            return None;
        }
    }
    for w in pts.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { return None };
        let half = a.truncate().distance(b.truncate()) / 2.0;
        if half >= r || r - (r * r - half * half).sqrt() > lim.tolerance {
            return None;
        }
    }
    Some((centre, sign < 0.0))
}

/// Fits arcs to the path from `start` through `pts`. Returns the moves that replace `pts`.
pub fn fit(start: DVec3, pts: &[DVec3], lim: &ArcLimits) -> Vec<Segment> {
    let mut out = Vec::new();
    if !(lim.tolerance > 0.0 && lim.tolerance.is_finite()) {
        out.extend(pts.iter().map(|p| Segment::Line(*p)));
        return out;
    }
    let mut path: Vec<DVec3> = Vec::with_capacity(pts.len() + 1);
    path.push(start);
    path.extend_from_slice(pts);
    let mut i = 0;
    while i + 1 < path.len() {
        // Grow the arc while it still fits; keep the longest that did.
        let mut best: Option<(usize, DVec2, bool)> = None;
        let mut j = i + lim.min_segments.max(2);
        while j < path.len() && j - i < MAX_ARC_POINTS {
            match path.get(i..=j).map_or(Run::No, |run| try_arc(run, lim)) {
                Run::Arc(c, cw) => best = Some((j, c, cw)),
                // A straight start may turn into a gentle arc; look a little further.
                Run::Straight if best.is_none() && j - i < MAX_STRAIGHT_LOOKAHEAD => {}
                _ => break,
            }
            j += 1;
        }
        match best {
            Some((j, center, cw)) => {
                if let Some(&to) = path.get(j) {
                    out.push(Segment::Arc { to, center, cw });
                }
                i = j;
            }
            None => {
                if let Some(&to) = path.get(i + 1) {
                    out.push(Segment::Line(to));
                }
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arc_points(c: DVec2, r: f64, a0: f64, a1: f64, n: usize, z0: f64, z1: f64) -> Vec<DVec3> {
        (0..=n)
            .map(|k| {
                let t = k as f64 / n as f64;
                let a = a0 + (a1 - a0) * t;
                (c + DVec2::new(a.cos(), a.sin()) * r).extend(z0 + (z1 - z0) * t)
            })
            .collect()
    }

    #[test]
    fn a_polyline_on_a_circle_becomes_one_arc() {
        let pts = arc_points(DVec2::new(10.0, 5.0), 3.175, 0.0, std::f64::consts::FRAC_PI_2, 12, 0.0, 0.0);
        let start = pts[0];
        let out = fit(start, &pts[1..], &ArcLimits::default());
        assert_eq!(out.len(), 1, "{out:?}");
        let Some(Segment::Arc { to, center, cw }) = out.first().copied() else {
            return assert!(out.is_empty(), "expected an arc: {out:?}");
        };
        assert!(!cw, "counter-clockwise");
        assert!(center.distance(DVec2::new(10.0, 5.0)) < 1.0e-6);
        assert!(to.distance(pts[12]) < 1.0e-9);
    }

    #[test]
    fn lines_stay_lines_and_corners_split_arcs() {
        let straight: Vec<DVec3> = (1..=10).map(|k| DVec3::new(f64::from(k), 0.0, 0.0)).collect();
        let out = fit(DVec3::ZERO, &straight, &ArcLimits::default());
        assert!(out.iter().all(|s| matches!(s, Segment::Line(_))));
        assert_eq!(out.len(), 10);
        // A quarter circle, a straight run, then another quarter circle the other way.
        let mut pts = arc_points(DVec2::ZERO, 5.0, 0.0, std::f64::consts::FRAC_PI_2, 24, 0.0, 0.0);
        pts.extend((1..=5).map(|k| DVec3::new(-f64::from(k), 5.0, 0.0)));
        pts.extend(
            arc_points(DVec2::new(-5.0, 0.0), 5.0, std::f64::consts::FRAC_PI_2, std::f64::consts::PI, 24, 0.0, 0.0)
                .into_iter()
                .skip(1),
        );
        let out = fit(pts[0], &pts[1..], &ArcLimits::default());
        let arcs = out.iter().filter(|s| matches!(s, Segment::Arc { .. })).count();
        assert_eq!(arcs, 2, "{out:?}");
    }

    #[test]
    fn a_polygon_with_corners_on_a_circle_stays_a_polygon() {
        let octagon = arc_points(DVec2::ZERO, 5.0, 0.0, std::f64::consts::PI, 4, 0.0, 0.0);
        let out = fit(octagon[0], &octagon[1..], &ArcLimits::default());
        assert!(out.iter().all(|s| matches!(s, Segment::Line(_))), "{out:?}");
    }

    #[test]
    fn helices_fit_when_z_follows_the_angle() {
        let pts = arc_points(DVec2::ZERO, 2.0, 0.0, 3.0, 60, 10.0, 9.0);
        let out = fit(pts[0], &pts[1..], &ArcLimits { max_sweep: 6.0, ..ArcLimits::default() });
        assert_eq!(out.len(), 1, "{out:?}");
        // Z that does not follow the angle is not a helix.
        let mut bent = pts.clone();
        bent[30].z += 0.5;
        let out = fit(bent[0], &bent[1..], &ArcLimits { max_sweep: 6.0, ..ArcLimits::default() });
        assert!(out.len() > 1);
    }

    #[test]
    fn sweeps_are_capped_and_bad_input_is_left_alone() {
        let pts = arc_points(DVec2::ZERO, 5.0, 0.0, 6.0, 240, 0.0, 0.0);
        let out = fit(pts[0], &pts[1..], &ArcLimits::default());
        assert!(out.len() >= 2, "a near full circle splits at 180°");
        let out = fit(DVec3::ZERO, &[DVec3::X, DVec3::NAN], &ArcLimits::default());
        assert_eq!(out.len(), 2);
        let out = fit(DVec3::ZERO, &[DVec3::X], &ArcLimits { tolerance: 0.0, ..ArcLimits::default() });
        assert_eq!(out, vec![Segment::Line(DVec3::X)]);
    }
}
