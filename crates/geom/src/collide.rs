//! Clearance between a tool's body and a triangle mesh in any direction: triangles binned on a
//! 3D grid, and whether a capsule (a segment with a radius, as a tilted cylinder is checked)
//! comes closer to the mesh than its radius.

use glam::DVec3;

use crate::cutter::Triangle;

/// Most grid cells the index uses.
const MAX_CELLS: usize = 1 << 21;

/// Triangles binned on a uniform 3D grid.
pub struct MeshIndex {
    tris: Vec<Triangle>,
    origin: DVec3,
    cell: f64,
    dims: [usize; 3],
    bins: Vec<Vec<u32>>,
}

fn closest_on_segment(p: DVec3, a: DVec3, b: DVec3) -> DVec3 {
    let ab = b - a;
    let l2 = ab.length_squared();
    if l2 < 1.0e-24 {
        return a;
    }
    a + ab * ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
}

/// The point of triangle `t` closest to `p`.
fn closest_on_triangle(p: DVec3, t: &Triangle) -> DVec3 {
    let [a, b, c] = *t;
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = va + vb + vc;
    if denom.abs() < 1.0e-30 {
        return a;
    }
    a + ab * (vb / denom) + ac * (vc / denom)
}

/// The distance between segments p0-p1 and q0-q1.
fn segment_distance(p0: DVec3, p1: DVec3, q0: DVec3, q1: DVec3) -> f64 {
    let (d1, d2, r) = (p1 - p0, q1 - q0, p0 - q0);
    let (a, e, f) = (d1.length_squared(), d2.length_squared(), d2.dot(r));
    let (s, t);
    if a < 1.0e-24 && e < 1.0e-24 {
        return p0.distance(q0);
    }
    if a < 1.0e-24 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e < 1.0e-24 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let s0 = if denom > 1.0e-24 { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
            let t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            } else {
                t = t0;
                s = s0;
            }
        }
    }
    (p0 + d1 * s).distance(q0 + d2 * t)
}

/// The distance between segment a-b and triangle `t`.
pub fn segment_triangle_distance(a: DVec3, b: DVec3, t: &Triangle) -> f64 {
    let [p, q, r] = *t;
    let n = (q - p).cross(r - p);
    // Crossing the triangle's plane inside the triangle: they touch.
    let (da, db) = ((a - p).dot(n), (b - p).dot(n));
    if da * db <= 0.0 && (da - db).abs() > 1.0e-30 {
        let x = a + (b - a) * (da / (da - db));
        if closest_on_triangle(x, t).distance_squared(x) < 1.0e-18 {
            return 0.0;
        }
    }
    let mut best = closest_on_triangle(a, t).distance(a).min(closest_on_triangle(b, t).distance(b));
    for (u, v) in [(p, q), (q, r), (r, p)] {
        best = best.min(segment_distance(a, b, u, v));
    }
    best
}

impl MeshIndex {
    /// An index of `tris` with cells of about `cell` mm (larger when the mesh is big).
    pub fn new(tris: Vec<Triangle>, cell: f64) -> Self {
        let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
        for p in tris.iter().flatten() {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        if !(lo.is_finite() && hi.is_finite()) {
            return Self { tris: Vec::new(), origin: DVec3::ZERO, cell: 1.0, dims: [0; 3], bins: Vec::new() };
        }
        let size = (hi - lo).max(DVec3::splat(1.0e-3));
        let mut cell = if cell.is_finite() { cell.max(size.max_element() / 256.0).max(1.0e-3) } else { 1.0 };
        let count = |c: f64| {
            let d = (size / c).ceil();
            d.x.max(1.0) * d.y.max(1.0) * d.z.max(1.0)
        };
        while count(cell) > MAX_CELLS as f64 {
            cell *= 1.5;
        }
        let dims = [
            ((size.x / cell).ceil() as usize).max(1),
            ((size.y / cell).ceil() as usize).max(1),
            ((size.z / cell).ceil() as usize).max(1),
        ];
        let mut index =
            Self { tris: Vec::new(), origin: lo, cell, dims, bins: vec![Vec::new(); dims[0] * dims[1] * dims[2]] };
        for (i, t) in tris.iter().enumerate() {
            let (mut a, mut b) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
            for p in t {
                a = a.min(*p);
                b = b.max(*p);
            }
            for slot in index.cells(a, b) {
                if let Some(bin) = index.bins.get_mut(slot) {
                    bin.push(i as u32);
                }
            }
        }
        index.tris = tris;
        index
    }

    fn cell_range(&self, a: DVec3, b: DVec3) -> Option<[std::ops::RangeInclusive<usize>; 3]> {
        if self.bins.is_empty() || !(a.is_finite() && b.is_finite()) {
            return None;
        }
        let lo = ((a - self.origin) / self.cell).floor();
        let hi = ((b - self.origin) / self.cell).floor();
        let mut out: [std::ops::RangeInclusive<usize>; 3] = [0..=0, 0..=0, 0..=0];
        for (k, slot) in out.iter_mut().enumerate() {
            let n = self.dims.get(k).copied().unwrap_or(1) as f64;
            let (l, h) = (lo[k], hi[k]);
            if h < 0.0 || l >= n {
                return None;
            }
            *slot = (l.max(0.0) as usize)..=(h.min(n - 1.0) as usize);
        }
        Some(out)
    }

    fn cells(&self, a: DVec3, b: DVec3) -> Vec<usize> {
        let Some([xs, ys, zs]) = self.cell_range(a, b) else { return Vec::new() };
        let [nx, ny, _] = self.dims;
        let mut out = Vec::new();
        for z in zs {
            for y in ys.clone() {
                for x in xs.clone() {
                    out.push((z * ny + y) * nx + x);
                }
            }
        }
        out
    }

    /// The least distance from segment a-b to the mesh, up to `limit` (anything farther reports
    /// `limit`).
    pub fn distance(&self, a: DVec3, b: DVec3, limit: f64) -> f64 {
        let pad = DVec3::splat(limit.max(0.0));
        let mut best = limit;
        let near_axis = |p: DVec3| closest_on_segment(p, a, b).distance(p);
        let mut candidates: Vec<u32> = self
            .cells(a.min(b) - pad, a.max(b) + pad)
            .into_iter()
            .filter_map(|slot| self.bins.get(slot))
            .flatten()
            .copied()
            .collect();
        candidates.sort_unstable();
        candidates.dedup();
        for i in candidates {
            let Some(t) = self.tris.get(i as usize) else { continue };
            // Quick reject: a triangle whose corners are all far beyond its own size.
            let span = t[0].distance(t[1]).max(t[1].distance(t[2])).max(t[2].distance(t[0]));
            if t.iter().map(|p| near_axis(*p)).fold(f64::INFINITY, f64::min) > best + span {
                continue;
            }
            best = best.min(segment_triangle_distance(a, b, t));
            if best <= 0.0 {
                return 0.0;
            }
        }
        best
    }

    /// True when a capsule of `radius` around segment a-b keeps clear of the mesh.
    pub fn clear(&self, a: DVec3, b: DVec3, radius: f64) -> bool {
        self.distance(a, b, radius) >= radius
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<Triangle> {
        let c = |x: f64, y: f64| DVec3::new(x, y, 0.0);
        vec![[c(0., 0.), c(10., 0.), c(10., 10.)], [c(0., 0.), c(10., 10.), c(0., 10.)]]
    }

    #[test]
    fn segment_distances_to_a_triangle() {
        let t = [DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0), DVec3::new(0.0, 10.0, 0.0)];
        // Through the face.
        assert_eq!(segment_triangle_distance(DVec3::new(2.0, 2.0, -1.0), DVec3::new(2.0, 2.0, 1.0), &t), 0.0);
        // Above the face.
        let d = segment_triangle_distance(DVec3::new(2.0, 2.0, 3.0), DVec3::new(2.0, 2.0, 5.0), &t);
        assert!((d - 3.0).abs() < 1.0e-12);
        // Beside an edge, parallel to it.
        let d = segment_triangle_distance(DVec3::new(-2.0, 1.0, 0.0), DVec3::new(-2.0, 5.0, 0.0), &t);
        assert!((d - 2.0).abs() < 1.0e-12);
        // Crossing over the hypotenuse corner region.
        let d = segment_triangle_distance(DVec3::new(10.0, 10.0, -1.0), DVec3::new(10.0, 10.0, 1.0), &t);
        assert!((d - 50.0_f64.sqrt()).abs() < 1.0e-9, "{d}");
    }

    #[test]
    fn capsules_clear_or_touch_the_mesh() {
        let m = MeshIndex::new(square(), 2.0);
        // A vertical tool 1 mm above the square, radius 0.5: clear; radius 1.5: touching.
        let (a, b) = (DVec3::new(5.0, 5.0, 1.0), DVec3::new(5.0, 5.0, 30.0));
        assert!(m.clear(a, b, 0.5));
        assert!(!m.clear(a, b, 1.5));
        // A tool leaning over the edge, beside the square.
        let (a, b) = (DVec3::new(12.0, 5.0, 0.0), DVec3::new(30.0, 5.0, 10.0));
        assert!(m.clear(a, b, 1.9));
        assert!(!m.clear(a, b, 2.1));
        assert!((m.distance(a, b, 100.0) - 2.0).abs() < 1.0e-9);
        // Far away: reports the limit.
        assert_eq!(m.distance(DVec3::splat(500.0), DVec3::splat(600.0), 3.0), 3.0);
    }
}
