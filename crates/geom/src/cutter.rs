//! Cutter geometry against a triangle mesh, for 3D toolpaths with a flat end mill (a cylinder
//! with a flat bottom). Triangles are given in the toolpath's frame: Z is the tool axis.
//!
//! - [`shadow_above`]: everything at or above a height, seen from above. A flat end mill whose
//!   bottom is at that height must keep its centre at least its radius away from the shadow.
//! - [`DropCutter`]: how far the cutter can come down at a point before it touches the mesh.

use glam::{DVec2, DVec3};

use crate::region::{self, Shape};

/// A triangle in the toolpath frame.
pub type Triangle = [DVec3; 3];

/// The part of `tri` at or above `z`, projected onto the XY plane.
fn clip_above(tri: &Triangle, z: f64) -> Vec<DVec2> {
    let mut out = Vec::with_capacity(4);
    for i in 0..3 {
        let (Some(&a), Some(&b)) = (tri.get(i), tri.get((i + 1) % 3)) else { continue };
        let (ina, inb) = (a.z >= z, b.z >= z);
        if ina {
            out.push(a.truncate());
        }
        if ina != inb {
            let t = (z - a.z) / (b.z - a.z);
            out.push(a.lerp(b, t).truncate());
        }
    }
    out
}

/// The shadow, seen from above, of everything at or above `z`.
pub fn shadow_above(tris: &[Triangle], z: f64) -> Vec<Shape> {
    let polygons: Vec<Vec<DVec2>> =
        tris.iter().filter(|t| t.iter().any(|p| p.z >= z)).map(|t| clip_above(t, z)).filter(|p| p.len() >= 3).collect();
    region::union_polygons(&polygons)
}

/// Heights a flat end mill of radius `radius` can come down to over a mesh, with triangles
/// binned on an XY grid so each query only looks at nearby ones.
pub struct DropCutter {
    tris: Vec<Triangle>,
    radius: f64,
    origin: DVec2,
    cell: f64,
    cols: usize,
    rows: usize,
    bins: Vec<Vec<u32>>,
}

/// Most grid cells a drop cutter uses.
const MAX_CELLS: usize = 1 << 20;

impl DropCutter {
    pub fn new(tris: Vec<Triangle>, radius: f64) -> Self {
        let radius = if radius.is_finite() { radius.max(1.0e-6) } else { 1.0 };
        let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
        for p in tris.iter().flatten() {
            lo = lo.min(p.truncate());
            hi = hi.max(p.truncate());
        }
        if !(lo.is_finite() && hi.is_finite()) {
            return Self {
                tris: Vec::new(),
                radius,
                origin: DVec2::ZERO,
                cell: 1.0,
                cols: 0,
                rows: 0,
                bins: Vec::new(),
            };
        }
        let size = hi - lo + DVec2::splat(2.0 * radius);
        let mut cell = (2.0 * radius).max(size.max_element() / 1024.0).max(1.0e-3);
        while (size.x / cell).ceil() * (size.y / cell).ceil() > MAX_CELLS as f64 {
            cell *= 2.0;
        }
        let origin = lo - DVec2::splat(radius);
        let cols = ((size.x / cell).ceil() as usize).max(1);
        let rows = ((size.y / cell).ceil() as usize).max(1);
        let mut bins = vec![Vec::new(); cols * rows];
        for (i, t) in tris.iter().enumerate() {
            let (mut a, mut b) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
            for p in t {
                a = a.min(p.truncate());
                b = b.max(p.truncate());
            }
            let (c0, r0) = Self::cell_of(origin, cell, a - DVec2::splat(radius));
            let (c1, r1) = Self::cell_of(origin, cell, b + DVec2::splat(radius));
            for r in r0.min(rows - 1)..=r1.min(rows - 1) {
                for c in c0.min(cols - 1)..=c1.min(cols - 1) {
                    if let Some(bin) = bins.get_mut(r * cols + c) {
                        bin.push(i as u32);
                    }
                }
            }
        }
        Self { tris, radius, origin, cell, cols, rows, bins }
    }

    fn cell_of(origin: DVec2, cell: f64, p: DVec2) -> (usize, usize) {
        let d = ((p - origin) / cell).max(DVec2::ZERO);
        (d.x as usize, d.y as usize)
    }

    /// The lowest the cutter's bottom can be at (x, y) without entering the mesh, or None when
    /// nothing is under it.
    pub fn height(&self, at: DVec2) -> Option<f64> {
        if self.cols == 0 || !at.is_finite() {
            return None;
        }
        let (c, r) = Self::cell_of(self.origin, self.cell, at);
        if c >= self.cols || r >= self.rows {
            return None;
        }
        let r2 = self.radius * self.radius;
        let mut best: Option<f64> = None;
        let mut take = |z: f64| {
            if z.is_finite() && best.is_none_or(|b| z > b) {
                best = Some(z);
            }
        };
        for &i in self.bins.get(r * self.cols + c).into_iter().flatten() {
            let Some(t) = self.tris.get(i as usize) else { continue };
            // Corners under the cutter.
            for p in t {
                if p.truncate().distance_squared(at) <= r2 {
                    take(p.z);
                }
            }
            // Edges crossing the cutter's footprint: the highest point of the part inside it.
            for k in 0..3 {
                let (Some(&a), Some(&b)) = (t.get(k), t.get((k + 1) % 3)) else { continue };
                let d = b.truncate() - a.truncate();
                let f = a.truncate() - at;
                let qa = d.length_squared();
                if qa < 1.0e-18 {
                    continue;
                }
                let qb = 2.0 * f.dot(d);
                let qc = f.length_squared() - r2;
                let disc = qb * qb - 4.0 * qa * qc;
                if disc < 0.0 {
                    continue;
                }
                let sq = disc.sqrt();
                let (t0, t1) = (((-qb - sq) / (2.0 * qa)).max(0.0), ((-qb + sq) / (2.0 * qa)).min(1.0));
                if t0 <= t1 {
                    take(a.z + (b.z - a.z) * t0);
                    take(a.z + (b.z - a.z) * t1);
                }
            }
            // The face: where the cutter's rim touches its plane, if that lies on the triangle.
            let n = (t[1] - t[0]).cross(t[2] - t[0]);
            if n.z.abs() < 1.0e-12 {
                continue;
            }
            let n = if n.z < 0.0 { -n } else { n };
            let slope = DVec2::new(n.x, n.y);
            let contact = if slope.length() > 1.0e-12 { at - slope.normalize() * self.radius } else { at };
            if inside_xy(t, contact) {
                take(t[0].z - (n.x * (contact.x - t[0].x) + n.y * (contact.y - t[0].y)) / n.z);
            }
        }
        best
    }
}

fn inside_xy(t: &Triangle, p: DVec2) -> bool {
    let s = |a: DVec3, b: DVec3| (b.truncate() - a.truncate()).perp_dot(p - a.truncate());
    let (d0, d1, d2) = (s(t[0], t[1]), s(t[1], t[2]), s(t[2], t[0]));
    let eps = 1.0e-12;
    (d0 >= -eps && d1 >= -eps && d2 >= -eps) || (d0 <= eps && d1 <= eps && d2 <= eps)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box from (0,0,0) to (20,10,5) as triangles.
    fn block() -> Vec<Triangle> {
        let c = |x: f64, y: f64, z: f64| DVec3::new(x, y, z);
        let (x, y, z) = (20.0, 10.0, 5.0);
        let quads = [
            [c(0., 0., z), c(x, 0., z), c(x, y, z), c(0., y, z)],
            [c(0., 0., 0.), c(0., y, 0.), c(x, y, 0.), c(x, 0., 0.)],
            [c(0., 0., 0.), c(x, 0., 0.), c(x, 0., z), c(0., 0., z)],
            [c(0., y, 0.), c(0., y, z), c(x, y, z), c(x, y, 0.)],
            [c(0., 0., 0.), c(0., 0., z), c(0., y, z), c(0., y, 0.)],
            [c(x, 0., 0.), c(x, y, 0.), c(x, y, z), c(x, 0., z)],
        ];
        quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect()
    }

    #[test]
    fn the_shadow_of_a_block_is_its_footprint() {
        let s = shadow_above(&block(), 2.0);
        assert!((region::area(&s) - 200.0).abs() < 1.0e-6, "{}", region::area(&s));
        assert!(shadow_above(&block(), 6.0).is_empty());
    }

    #[test]
    fn a_flat_cutter_lands_on_top_and_touches_the_edges() {
        let d = DropCutter::new(block(), 3.0);
        assert_eq!(d.height(DVec2::new(10.0, 5.0)), Some(5.0));
        // Overhanging the edge by less than the radius still sits on the top.
        assert_eq!(d.height(DVec2::new(22.0, 5.0)), Some(5.0));
        // Beyond the radius, nothing is under it.
        assert_eq!(d.height(DVec2::new(24.0, 5.0)), None);
        assert_eq!(d.height(DVec2::new(f64::NAN, 0.0)), None);
    }

    #[test]
    fn a_flat_cutter_rides_a_slope_on_its_rim() {
        // A ramp rising 1 in X per 1: the rim touches it one radius uphill.
        let c = |x: f64, y: f64| DVec3::new(x, y, x);
        let ramp = vec![[c(0., -50.), c(50., -50.), c(50., 50.)], [c(0., -50.), c(50., 50.), c(0., 50.)]];
        let d = DropCutter::new(ramp, 2.0);
        let z = d.height(DVec2::new(20.0, 0.0)).unwrap_or_default();
        assert!((z - 22.0).abs() < 1.0e-9, "{z}");
    }
}
