//! A height field ("Z-map") of the stock for 3-axis material removal: a grid over the stock's
//! footprint holding the top of the material in each cell. A tool moving from one point to
//! another lowers every cell its end passes over to the lowest height it reaches there: a flat
//! end mill's bottom, or the underside of a ball end mill's ball. Coordinates are in the
//! toolpath frame, Z along the tool.

use glam::{DVec2, DVec3};

/// Most cells one height field holds.
pub const MAX_CELLS: usize = 4_000_000;

#[derive(Clone, Debug)]
pub struct ZMap {
    origin: DVec2,
    cell: f64,
    nx: usize,
    ny: usize,
    z: Vec<f32>,
    /// The stock's bottom: material never goes below it.
    bottom: f64,
    /// The stock's top before any cutting.
    top: f64,
    /// Bumped by every change, so a display knows to rebuild.
    pub revision: u64,
}

/// What one cut did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cut {
    /// Material removed, cubic units.
    pub volume: f64,
    /// The deepest the cut went below the material it met.
    pub depth: f64,
    /// The same, counting only cells wholly under the cutter: what collision checks use, so a
    /// cutter grazing material at its edge is not taken for one running into it.
    pub core_depth: f64,
}

impl ZMap {
    /// A block from `lo` to `hi` (XY) between `bottom` and `top`, with cells about `cell`
    /// wide (coarser when the block would need more than `max_cells`).
    pub fn new(lo: DVec2, hi: DVec2, bottom: f64, top: f64, cell: f64, max_cells: usize) -> Option<Self> {
        let size = hi - lo;
        if !(size.x > 0.0 && size.y > 0.0 && size.is_finite() && top > bottom && top.is_finite() && bottom.is_finite())
        {
            return None;
        }
        let max_cells = max_cells.clamp(1, MAX_CELLS) as f64;
        let mut cell = if cell.is_finite() && cell > 0.0 { cell } else { size.max_element() / 100.0 };
        cell = cell.max((size.x * size.y / max_cells).sqrt());
        let nx = ((size.x / cell).ceil() as usize).max(1);
        let ny = ((size.y / cell).ceil() as usize).max(1);
        Some(Self { origin: lo, cell, nx, ny, z: vec![top as f32; nx * ny], bottom, top, revision: 0 })
    }

    pub fn cell(&self) -> f64 {
        self.cell
    }

    pub fn size(&self) -> (usize, usize) {
        (self.nx, self.ny)
    }

    pub fn origin(&self) -> DVec2 {
        self.origin
    }

    pub fn bottom(&self) -> f64 {
        self.bottom
    }

    /// The stock's top before any cutting.
    pub fn top(&self) -> f64 {
        self.top
    }

    /// The centre of cell (i, j).
    pub fn centre(&self, i: usize, j: usize) -> DVec2 {
        self.origin + DVec2::new((i as f64 + 0.5) * self.cell, (j as f64 + 0.5) * self.cell)
    }

    /// The material's top in cell (i, j).
    pub fn height(&self, i: usize, j: usize) -> Option<f64> {
        (i < self.nx).then(|| self.z.get(j * self.nx + i).map(|z| f64::from(*z))).flatten()
    }

    /// The material's top under a point, if the point is over the stock.
    pub fn height_at(&self, p: DVec2) -> Option<f64> {
        let d = (p - self.origin) / self.cell;
        if d.x < 0.0 || d.y < 0.0 {
            return None;
        }
        self.height(d.x as usize, d.y as usize)
    }

    /// Moves a flat end mill of `radius` from `a` to `b` (its bottom centre), removing the
    /// material it passes through.
    pub fn cut(&mut self, a: DVec3, b: DVec3, radius: f64) -> Cut {
        self.cut_tool(a, b, radius, false)
    }

    /// Moves a flat (or, with `ball`, ball) end mill of `radius` from tip `a` to tip `b`.
    pub fn cut_tool(&mut self, a: DVec3, b: DVec3, radius: f64, ball: bool) -> Cut {
        let mut out = Cut::default();
        if !(a.is_finite() && b.is_finite() && radius.is_finite() && radius > 0.0) {
            return out;
        }
        let lo = a.truncate().min(b.truncate()) - DVec2::splat(radius);
        let hi = a.truncate().max(b.truncate()) + DVec2::splat(radius);
        let to_index = |v: f64, n: usize| ((v / self.cell).floor().max(0.0) as usize).min(n);
        let (i0, i1) = (to_index(lo.x - self.origin.x, self.nx), to_index(hi.x - self.origin.x, self.nx - 1) + 1);
        let (j0, j1) = (to_index(lo.y - self.origin.y, self.ny), to_index(hi.y - self.origin.y, self.ny - 1) + 1);
        if i0 >= i1 || j0 >= j1 {
            return out;
        }
        let d = b.truncate() - a.truncate();
        let qa = d.length_squared();
        let r2 = radius * radius;
        // Cells whose centres are this close to the path lie wholly under the cutter.
        let core = (radius - self.cell * std::f64::consts::FRAC_1_SQRT_2).max(0.0);
        let area = self.cell * self.cell;
        let mut changed = false;
        for j in j0..j1.min(self.ny) {
            for i in i0..i1.min(self.nx) {
                let p = self.centre(i, j);
                // Where along the move the cutter's bottom covers this cell: solve
                // |a + t·d − p|² ≤ r² for t in [0, 1]; the height is lowest at an end of that.
                let f = a.truncate() - p;
                let low = if qa < 1.0e-18 {
                    (f.length_squared() <= r2).then_some(a.z.min(b.z))
                } else {
                    let qb = 2.0 * f.dot(d);
                    let qc = f.length_squared() - r2;
                    let disc = qb * qb - 4.0 * qa * qc;
                    if disc < 0.0 {
                        None
                    } else {
                        let sq = disc.sqrt();
                        let (t0, t1) = (((-qb - sq) / (2.0 * qa)).max(0.0), ((-qb + sq) / (2.0 * qa)).min(1.0));
                        (t0 <= t1).then(|| (a.z + (b.z - a.z) * t0).min(a.z + (b.z - a.z) * t1))
                    }
                };
                let Some(low) = low else { continue };
                // A ball's underside rises away from its centre: the lowest it reaches over this
                // cell, over the stretch of the move it covers the cell (convex there).
                let low = if ball {
                    let (t0, t1) = if qa < 1.0e-18 {
                        (0.0, 1.0)
                    } else {
                        let qb = 2.0 * f.dot(d);
                        let qc = f.length_squared() - r2;
                        let sq = (qb * qb - 4.0 * qa * qc).max(0.0).sqrt();
                        (((-qb - sq) / (2.0 * qa)).max(0.0), ((-qb + sq) / (2.0 * qa)).min(1.0))
                    };
                    let under = |t: f64| {
                        let c = a.truncate() + d * t;
                        let h2 = (r2 - c.distance_squared(p)).max(0.0);
                        a.z + (b.z - a.z) * t + radius - h2.sqrt()
                    };
                    let (mut lo, mut hi) = (t0, t1);
                    for _ in 0..40 {
                        if hi - lo < 1.0e-6 {
                            break;
                        }
                        let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
                        if under(m1) < under(m2) {
                            hi = m2;
                        } else {
                            lo = m1;
                        }
                    }
                    under((lo + hi) / 2.0).min(under(t0)).min(under(t1))
                } else {
                    low
                };
                let low = low.max(self.bottom);
                if let Some(z) = self.z.get_mut(j * self.nx + i) {
                    let old = f64::from(*z);
                    if low < old - 1.0e-6 {
                        out.volume += (old - low) * area;
                        out.depth = out.depth.max(old - low);
                        let t = if qa < 1.0e-18 { 0.0 } else { (-f.dot(d) / qa).clamp(0.0, 1.0) };
                        if (a.truncate() + d * t).distance(p) <= core {
                            out.core_depth = out.core_depth.max(old - low);
                        }
                        *z = low as f32;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        out
    }

    /// The material left: the sum of heights above the bottom times the cell area.
    pub fn volume(&self) -> f64 {
        self.z.iter().map(|z| (f64::from(*z) - self.bottom).max(0.0)).sum::<f64>() * self.cell * self.cell
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ball_leaves_a_round_groove() {
        let mut z = block();
        // A Ø6 ball along X with its tip at z 15: across the groove the floor follows the ball.
        z.cut_tool(DVec3::new(-10.0, 25.0, 15.0), DVec3::new(110.0, 25.0, 15.0), 3.0, true);
        for dy in [0.0, 1.0, 2.0, 2.5] {
            let h = z.height_at(DVec2::new(50.25, 25.25 + dy)).unwrap_or(f64::NAN);
            let expect = 18.0 - (9.0 - (dy + 0.25_f64).powi(2)).sqrt();
            assert!((h - expect).abs() < 1.0e-3, "{dy}: {h} vs {expect}");
        }
        // A flat end mill leaves a flat floor.
        let mut f = block();
        f.cut(DVec3::new(-10.0, 25.0, 15.0), DVec3::new(110.0, 25.0, 15.0), 3.0);
        assert_eq!(f.height_at(DVec2::new(50.25, 27.25)), Some(15.0));
    }

    /// A 100 × 50 × 20 block with 0.5 mm cells (an empty 1-cell map if that ever failed).
    fn block() -> ZMap {
        ZMap::new(DVec2::ZERO, DVec2::new(100.0, 50.0), 0.0, 20.0, 0.5, 1_000_000).unwrap_or(ZMap {
            origin: DVec2::ZERO,
            cell: 1.0,
            nx: 1,
            ny: 1,
            z: vec![0.0],
            bottom: 0.0,
            top: 0.0,
            revision: 0,
        })
    }

    #[test]
    fn a_slot_removes_its_volume() {
        let mut z = block();
        assert_eq!(z.size(), (200, 100));
        let before = z.volume();
        // A 10 mm slot, 5 mm deep, 60 mm long: 60·10·5 + the two half-circle ends (π·25·5).
        let cut = z.cut(DVec3::new(20.0, 25.0, 15.0), DVec3::new(80.0, 25.0, 15.0), 5.0);
        let expected = 60.0 * 10.0 * 5.0 + std::f64::consts::PI * 25.0 * 5.0;
        assert!((cut.volume - expected).abs() / expected < 0.02, "{} vs {expected}", cut.volume);
        assert!((before - z.volume() - cut.volume).abs() < 1.0e-6);
        assert!((cut.depth - 5.0).abs() < 1.0e-6);
        assert_eq!(z.height_at(DVec2::new(50.0, 25.0)), Some(15.0));
        assert_eq!(z.height_at(DVec2::new(50.0, 35.0)), Some(20.0));
        // Cutting the same slot again removes nothing.
        assert_eq!(z.cut(DVec3::new(20.0, 25.0, 15.0), DVec3::new(80.0, 25.0, 15.0), 5.0).volume, 0.0);
    }

    #[test]
    fn a_ramp_cuts_deepest_at_its_low_end_and_never_through_the_bottom() {
        let mut z = block();
        z.cut(DVec3::new(10.0, 25.0, 20.0), DVec3::new(90.0, 25.0, -10.0), 2.0);
        let h = |x: f64| z.height_at(DVec2::new(x, 25.0)).unwrap_or(f64::NAN);
        assert!(h(20.0) > h(50.0) && h(50.0) > h(70.0));
        assert_eq!(h(90.0), 0.0, "the bottom stops it");
    }

    #[test]
    fn bad_input_does_nothing() {
        let mut z = block();
        assert_eq!(z.cut(DVec3::NAN, DVec3::ZERO, 1.0), Cut::default());
        assert_eq!(z.cut(DVec3::ZERO, DVec3::ONE, -1.0), Cut::default());
        assert_eq!(z.cut(DVec3::new(500.0, 500.0, 0.0), DVec3::new(600.0, 600.0, 0.0), 1.0), Cut::default());
        assert!(ZMap::new(DVec2::ZERO, DVec2::ZERO, 0.0, 1.0, 1.0, 10).is_none());
        // The cell count is capped.
        let big = ZMap::new(DVec2::ZERO, DVec2::splat(1000.0), 0.0, 1.0, 0.001, 10_000);
        assert!(big.is_some_and(|b| b.size().0 * b.size().1 <= 10_100));
    }
}
