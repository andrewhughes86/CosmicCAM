//! Planar regions for toolpaths: shapes made of an outer boundary and holes, with offsets,
//! booleans and point tests through the `i_overlay` engine.
//!
//! A [`Shape`] is its outer contour (counter-clockwise) followed by its holes (clockwise), with
//! no repeated closing point. Every third-party call runs under `catch_unwind`; a panic there
//! comes back as an empty result, never as a crash.

use std::panic::{AssertUnwindSafe, catch_unwind};

use glam::DVec2;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::style::{LineJoin, OutlineStyle};

use crate::offset::{arc_step, signed_area2};

/// One closed contour.
pub type Contour = Vec<DVec2>;
/// An outer contour and its holes.
pub type Shape = Vec<Contour>;

type Raw = Vec<Vec<Vec<[f64; 2]>>>;

fn to_raw(shapes: &[Shape]) -> Raw {
    shapes
        .iter()
        .map(|s| {
            s.iter()
                .filter(|c| c.len() >= 3 && c.iter().all(|p| p.is_finite()))
                .map(|c| c.iter().map(|p| [p.x, p.y]).collect())
                .collect()
        })
        .collect()
}

fn from_raw(raw: Raw) -> Vec<Shape> {
    raw.into_iter()
        .map(|s| {
            s.into_iter()
                .map(|c| c.into_iter().map(|[x, y]| DVec2::new(x, y)).collect::<Contour>())
                .filter(|c| c.len() >= 3)
                .collect::<Shape>()
        })
        .filter(|s: &Shape| !s.is_empty())
        .collect()
}

/// A shape from loops in any orientation: the largest is the outside, the rest holes.
pub fn shape_from_loops(loops: &[Contour]) -> Option<Shape> {
    let mut loops: Vec<Contour> = loops.iter().filter(|l| l.len() >= 3).cloned().collect();
    loops.sort_by(|a, b| signed_area2(b).abs().total_cmp(&signed_area2(a).abs()));
    let mut shape = Vec::with_capacity(loops.len());
    for (i, mut l) in loops.into_iter().enumerate() {
        let ccw = signed_area2(&l) > 0.0;
        if ccw != (i == 0) {
            l.reverse();
        }
        shape.push(l);
    }
    (!shape.is_empty()).then_some(shape)
}

/// An axis-aligned rectangle as a shape.
pub fn rectangle(min: DVec2, max: DVec2) -> Shape {
    vec![vec![min, DVec2::new(max.x, min.y), max, DVec2::new(min.x, max.y)]]
}

/// Offsets shapes by `distance`: outwards when positive, inwards when negative. Insetting can
/// split shapes or remove them.
pub fn offset(shapes: &[Shape], distance: f64, tolerance: f64) -> Vec<Shape> {
    if !distance.is_finite() {
        return Vec::new();
    }
    if distance.abs() < 1.0e-9 {
        return shapes.to_vec();
    }
    let raw = to_raw(shapes);
    let style = OutlineStyle::new(distance).line_join(LineJoin::Round(arc_step(distance, tolerance)));
    let out = catch_unwind(AssertUnwindSafe(|| raw.outline_as::<i64>(&style))).unwrap_or_default();
    from_raw(out)
}

/// How two sets of shapes combine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Union,
    Difference,
    Intersect,
}

/// `a` combined with `b`.
pub fn boolean(a: &[Shape], b: &[Shape], op: Op) -> Vec<Shape> {
    let (ra, rb) = (to_raw(a), to_raw(b));
    if ra.is_empty() {
        return if op == Op::Union { from_raw(rb) } else { Vec::new() };
    }
    let rule = match op {
        Op::Union => OverlayRule::Union,
        Op::Difference => OverlayRule::Difference,
        Op::Intersect => OverlayRule::Intersect,
    };
    let out = catch_unwind(AssertUnwindSafe(|| ra.overlay_as::<i64>(&rb, rule, FillRule::NonZero))).unwrap_or_default();
    from_raw(out)
}

/// The union of many polygons (any orientation), e.g. the shadow of a mesh's triangles.
pub fn union_polygons(polygons: &[Contour]) -> Vec<Shape> {
    let raw: Vec<Vec<[f64; 2]>> = polygons
        .iter()
        .filter(|p| p.len() >= 3 && p.iter().all(|q| q.is_finite()) && signed_area2(p).abs() > 1.0e-12)
        .map(|p| {
            let mut c: Vec<[f64; 2]> = p.iter().map(|q| [q.x, q.y]).collect();
            if signed_area2(p) < 0.0 {
                c.reverse();
            }
            c
        })
        .collect();
    if raw.is_empty() {
        return Vec::new();
    }
    let empty: Vec<Vec<[f64; 2]>> = Vec::new();
    let out = catch_unwind(AssertUnwindSafe(|| raw.overlay_as::<i64>(&empty, OverlayRule::Subject, FillRule::NonZero)))
        .unwrap_or_default();
    from_raw(out)
}

/// The area the shapes cover.
pub fn area(shapes: &[Shape]) -> f64 {
    shapes.iter().flatten().map(|c| signed_area2(c) / 2.0).sum()
}

fn in_contour(c: &[DVec2], p: DVec2) -> bool {
    let n = c.len();
    let mut inside = false;
    for i in 0..n {
        let (Some(a), Some(b)) = (c.get(i), c.get((i + n - 1) % n)) else { continue };
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

/// Whether `p` is inside the shapes (inside an outer contour and outside its holes).
pub fn contains(shapes: &[Shape], p: DVec2) -> bool {
    shapes.iter().any(|s| {
        let mut it = s.iter();
        it.next().is_some_and(|outer| in_contour(outer, p)) && !it.any(|hole| in_contour(hole, p))
    })
}

/// Whether the straight move from `a` to `b` stays inside the shapes, checked every `step`.
pub fn segment_inside(shapes: &[Shape], a: DVec2, b: DVec2, step: f64) -> bool {
    let n = (a.distance(b) / step.max(1.0e-3)).ceil().clamp(1.0, 10_000.0) as usize;
    (0..=n).all(|i| contains(shapes, a.lerp(b, i as f64 / n as f64)))
}

/// Lines across the shapes: rows `spacing` apart at `angle` (radians from +X), each cut to the
/// parts that lie inside, as (start, end) pairs in row order, left to right within a row.
pub fn hatch(shapes: &[Shape], spacing: f64, angle: f64, max_rows: usize) -> Vec<Vec<(DVec2, DVec2)>> {
    let (s, c) = angle.sin_cos();
    let to_local = |p: DVec2| DVec2::new(p.x * c + p.y * s, -p.x * s + p.y * c);
    let to_world = |p: DVec2| DVec2::new(p.x * c - p.y * s, p.x * s + p.y * c);
    let local: Vec<Vec<Contour>> =
        shapes.iter().map(|sh| sh.iter().map(|ct| ct.iter().map(|p| to_local(*p)).collect()).collect()).collect();
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in local.iter().flatten().flatten() {
        lo = lo.min(p.y);
        hi = hi.max(p.y);
    }
    if !(lo.is_finite() && hi.is_finite()) || spacing <= 0.0 {
        return Vec::new();
    }
    let rows = (((hi - lo) / spacing).ceil() as usize).clamp(1, max_rows.max(1));
    let step = (hi - lo) / rows as f64;
    let mut out = Vec::new();
    for r in 0..=rows {
        // Rows sit just inside the extremes so the edges themselves are cut.
        let y = (lo + step * r as f64).clamp(lo + 1.0e-6, hi - 1.0e-6);
        let mut xs: Vec<f64> = Vec::new();
        for ct in local.iter().flatten() {
            let n = ct.len();
            for i in 0..n {
                let (Some(a), Some(b)) = (ct.get(i), ct.get((i + 1) % n)) else { continue };
                if (a.y > y) != (b.y > y) {
                    xs.push(a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y));
                }
            }
        }
        xs.sort_by(f64::total_cmp);
        let row: Vec<(DVec2, DVec2)> = xs
            .chunks(2)
            .filter_map(|w| Some((*w.first()?, *w.get(1)?)))
            .filter(|(a, b)| b - a > 1.0e-6)
            .map(|(a, b)| (to_world(DVec2::new(a, y)), to_world(DVec2::new(b, y))))
            .collect();
        if !row.is_empty() {
            out.push(row);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(c: DVec2, h: f64) -> Shape {
        rectangle(c - DVec2::splat(h), c + DVec2::splat(h))
    }

    #[test]
    fn booleans_and_offsets_keep_islands() {
        let outer = square(DVec2::ZERO, 50.0);
        let island = square(DVec2::ZERO, 10.0);
        let ring = boolean(&[outer], &[island], Op::Difference);
        assert_eq!(ring.len(), 1);
        assert_eq!(ring[0].len(), 2, "outer and hole");
        assert!((area(&ring) - (10_000.0 - 400.0)).abs() < 1.0e-6);
        assert!(contains(&ring, DVec2::new(30.0, 0.0)));
        assert!(!contains(&ring, DVec2::ZERO));
        // Insetting by 5 grows the hole and shrinks the outside.
        let inset = offset(&ring, -5.0, 0.01);
        assert!(contains(&inset, DVec2::new(40.0, 0.0)));
        assert!(!contains(&inset, DVec2::new(47.0, 0.0)));
        assert!(!contains(&inset, DVec2::new(13.0, 0.0)));
        assert!(offset(&ring, -30.0, 0.01).is_empty());
    }

    #[test]
    fn unions_merge_overlapping_polygons() {
        let a = vec![DVec2::new(0.0, 0.0), DVec2::new(10.0, 0.0), DVec2::new(10.0, 10.0), DVec2::new(0.0, 10.0)];
        let mut b: Contour = a.iter().map(|p| *p + DVec2::new(5.0, 0.0)).collect();
        b.reverse();
        let u = union_polygons(&[a, b]);
        assert_eq!(u.len(), 1);
        assert!((area(&u) - 150.0).abs() < 1.0e-6);
        assert!(union_polygons(&[vec![DVec2::ZERO, DVec2::X, DVec2::X * 2.0]]).is_empty());
    }

    #[test]
    fn hatching_skips_holes() {
        let ring = boolean(&[square(DVec2::ZERO, 50.0)], &[square(DVec2::ZERO, 10.0)], Op::Difference);
        let rows = hatch(&ring, 10.0, 0.0, 1000);
        assert_eq!(rows.len(), 11);
        let middle = &rows[5];
        assert_eq!(middle.len(), 2, "the island splits the middle row");
        assert!(hatch(&ring, 0.0, 0.0, 1000).is_empty());
        let tilted = hatch(&[square(DVec2::ZERO, 50.0)], 10.0, 0.5, 1000);
        assert!(tilted.len() > 11);
    }

    #[test]
    fn hostile_input_gives_nothing() {
        let bad: Shape = vec![vec![DVec2::NAN, DVec2::X, DVec2::Y]];
        assert!(offset(std::slice::from_ref(&bad), 1.0, 0.01).is_empty());
        assert!(boolean(std::slice::from_ref(&bad), std::slice::from_ref(&bad), Op::Union).is_empty());
        assert!(offset(&[square(DVec2::ZERO, 1.0)], f64::INFINITY, 0.01).is_empty());
    }
}
