//! 2D contour offsetting for toolpaths: closed loops through the `i_overlay` outline engine
//! (round joins, self-intersections resolved), open polylines with a direct parallel offset.
//!
//! The third-party call runs under `catch_unwind`; a panic there comes back as no result,
//! never as a crash.

use std::panic::{AssertUnwindSafe, catch_unwind};

use glam::DVec2;
use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::style::{LineJoin, OutlineStyle};

/// Twice the signed area: positive for counter-clockwise loops.
pub fn signed_area2(pts: &[DVec2]) -> f64 {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (a, b) = (pts.get(i).copied().unwrap_or_default(), pts.get((i + 1) % n).copied().unwrap_or_default());
            a.perp_dot(b)
        })
        .sum()
}

/// The angle step for round joins and arcs that keeps the chord within `tolerance` of a
/// circle of `radius`.
pub fn arc_step(radius: f64, tolerance: f64) -> f64 {
    let r = radius.abs().max(1.0e-9);
    let t = tolerance.clamp(1.0e-6, r);
    (2.0 * (1.0 - t / r).clamp(-1.0, 1.0).acos()).clamp(0.02, 0.5)
}

/// Offsets a closed loop (any orientation) by `distance`: outwards when positive, inwards when
/// negative. Returns the resulting outer boundaries, counter-clockwise, without repeating the
/// first point. Insetting can split a loop into several or remove it; none come back then.
pub fn offset_closed(loop_pts: &[DVec2], distance: f64, tolerance: f64) -> Vec<Vec<DVec2>> {
    if loop_pts.len() < 3 || !distance.is_finite() || loop_pts.iter().any(|p| !p.is_finite()) {
        return Vec::new();
    }
    if distance.abs() < 1.0e-9 {
        let mut pts = loop_pts.to_vec();
        if signed_area2(&pts) < 0.0 {
            pts.reverse();
        }
        return vec![pts];
    }
    // The engine reads clockwise loops as holes; offsets here are always of a solid outline.
    let mut path: Vec<[f64; 2]> = loop_pts.iter().map(|p| [p.x, p.y]).collect();
    if signed_area2(loop_pts) < 0.0 {
        path.reverse();
    }
    let style = OutlineStyle::new(distance).line_join(LineJoin::Round(arc_step(distance, tolerance)));
    let shapes = catch_unwind(AssertUnwindSafe(|| path.outline_as::<i64>(&style))).unwrap_or_default();
    shapes
        .into_iter()
        .filter_map(|shape| shape.into_iter().next())
        .map(|outer| {
            let mut pts: Vec<DVec2> = outer.into_iter().map(|[x, y]| DVec2::new(x, y)).collect();
            if signed_area2(&pts) < 0.0 {
                pts.reverse();
            }
            pts
        })
        .filter(|pts| pts.len() >= 3)
        .collect()
}

/// Offsets an open polyline sideways by `distance` (positive: to the left of travel), with
/// round joins on the outside of bends and trimmed corners on the inside.
pub fn offset_open(pts: &[DVec2], distance: f64, tolerance: f64) -> Vec<DVec2> {
    if pts.len() < 2 || distance.abs() < 1.0e-12 {
        return pts.to_vec();
    }
    let normal = |a: DVec2, b: DVec2| (b - a).normalize_or_zero().perp() * distance;
    let mut out: Vec<DVec2> = Vec::new();
    let segs: Vec<(DVec2, DVec2)> = pts
        .windows(2)
        .filter_map(|w| Some((*w.first()?, *w.get(1)?)))
        .filter(|(a, b)| a.distance(*b) > 1.0e-12)
        .collect();
    for (i, &(a, b)) in segs.iter().enumerate() {
        let n = normal(a, b);
        let (sa, sb) = (a + n, b + n);
        if i == 0 {
            out.push(sa);
        }
        match segs.get(i + 1) {
            None => out.push(sb),
            Some(&(c, d)) => {
                let n2 = normal(c, d);
                let turn = (b - a).perp_dot(d - c);
                // The offset side is the outside of the bend: round it; else meet the next line.
                if turn * distance < 0.0 {
                    out.push(sb);
                    let (a0, a1) = (n.y.atan2(n.x), n2.y.atan2(n2.x));
                    let mut sweep = a1 - a0;
                    while sweep > std::f64::consts::PI {
                        sweep -= std::f64::consts::TAU;
                    }
                    while sweep < -std::f64::consts::PI {
                        sweep += std::f64::consts::TAU;
                    }
                    let steps = (sweep.abs() / arc_step(distance, tolerance)).ceil().clamp(1.0, 256.0) as usize;
                    for k in 1..steps {
                        let t = a0 + sweep * k as f64 / steps as f64;
                        out.push(b + DVec2::new(t.cos(), t.sin()) * distance.abs());
                    }
                    out.push(c + n2);
                } else {
                    // Intersect the two offset lines.
                    let (p, r) = (sa, sb - sa);
                    let (q, s) = (c + n2, d - c);
                    let denom = r.perp_dot(s);
                    if denom.abs() > 1.0e-12 {
                        let t = (q - p).perp_dot(s) / denom;
                        out.push(p + r * t);
                    } else {
                        out.push(sb);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(s: f64) -> Vec<DVec2> {
        vec![DVec2::new(0.0, 0.0), DVec2::new(s, 0.0), DVec2::new(s, s), DVec2::new(0.0, s)]
    }

    #[test]
    fn outward_offset_rounds_corners() {
        let out = offset_closed(&square(10.0), 2.0, 0.01);
        assert_eq!(out.len(), 1);
        let loop_ = &out[0];
        assert!(signed_area2(loop_) > 0.0);
        // Every point lies 2 mm outside the square (rounded corners).
        for p in loop_ {
            let dx = (p.x - p.x.clamp(0.0, 10.0)).abs();
            let dy = (p.y - p.y.clamp(0.0, 10.0)).abs();
            assert!((dx.hypot(dy) - 2.0).abs() < 0.05, "{p:?}");
        }
    }

    #[test]
    fn inward_offset_shrinks_and_vanishes() {
        let out = offset_closed(&square(10.0), -2.0, 0.01);
        assert_eq!(out.len(), 1);
        assert!((signed_area2(&out[0]) / 2.0 - 36.0).abs() < 0.5);
        assert!(offset_closed(&square(10.0), -6.0, 0.01).is_empty());
        let mut cw = square(10.0);
        cw.reverse();
        assert_eq!(offset_closed(&cw, 1.0, 0.01).len(), 1, "orientation does not matter");
    }

    #[test]
    fn open_offsets_follow_the_side() {
        let line = [DVec2::new(0.0, 0.0), DVec2::new(10.0, 0.0), DVec2::new(10.0, 10.0)];
        let left = offset_open(&line, 1.0, 0.01);
        assert!(left.first().is_some_and(|p| (p.y - 1.0).abs() < 1e-9));
        // Left of an anticlockwise bend is its inside: a sharp corner at (9, 1).
        assert!(left.iter().any(|p| p.distance(DVec2::new(9.0, 1.0)) < 1e-9), "{left:?}");
        let right = offset_open(&line, -1.0, 0.01);
        assert!(right.len() > 3, "the outside of the bend is rounded");
        assert!(offset_closed(&[], 1.0, 0.01).is_empty());
        assert!(offset_closed(&square(f64::NAN), 1.0, 0.01).is_empty());
    }
}
