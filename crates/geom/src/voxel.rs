//! A voxel model of the stock, for simulating tools that are not vertical (3+2 and 5-axis): a
//! grid of solid / empty cells cut by a tool swept along a move with any (constant) axis.
//!
//! - A move removes every cell whose centre the cutting part of the tool sweeps through: a
//!   cylinder (flat end mill) or a cylinder on a ball (ball end mill), tested exactly against the
//!   straight sweep. Cells the shank sweeps through are counted as shank contact and removed.
//! - [`Voxels::part_cells`] marks the cells inside a closed mesh, for gouge checks.
//! - [`Voxels::surface`] meshes the solid with surface nets for display.

use glam::{DVec3, Vec3};

use crate::cutter::Triangle;

/// A tool as the voxel model cuts it, mm.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolShape {
    pub radius: f64,
    /// A ball end (radius `radius`) instead of a flat bottom.
    pub ball: bool,
    pub flute_length: f64,
    pub shank_radius: f64,
    /// Stick-out from the holder to the tip.
    pub length: f64,
    /// The holder above the stick-out: (bottom, top, radius) of each segment, heights above the
    /// holder face, as cylinders of the segment's wider end; empty for none.
    pub holder: Vec<(f64, f64, f64)>,
}

/// What one cut did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VoxelCut {
    /// Cells the flutes removed.
    pub removed: usize,
    /// Cells the shank went through (removed too).
    pub shank: usize,
    /// Of those, cells more than a cell inside the flutes' and the shank's sweep: material the
    /// tool really went into, not cells its surface grazed.
    pub deep: usize,
    pub deep_shank: usize,
    /// One cell the shank went deep into, if any.
    pub shank_at: Option<DVec3>,
    /// Cells the holder went through, and one of them.
    pub holder: usize,
    pub holder_at: Option<DVec3>,
    /// The lowest removed cell centre along the tool axis, measured up from the tip at the
    /// move's start (how deep into material the tool went), if anything was removed.
    pub lowest: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Voxels {
    origin: DVec3,
    cell: f64,
    dims: [usize; 3],
    bits: Vec<u64>,
    /// Bumped whenever a cut removes something.
    pub revision: u64,
}

/// One surface-net mesh: positions and normals per vertex, triangles as index triples, and for
/// each vertex whether any cell around it was cut (else it is on the original stock).
#[derive(Clone, Debug, Default)]
pub struct Surface {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub cut: Vec<bool>,
    /// The cell (i, j, k) each vertex sits in, for colouring by cell.
    pub cells: Vec<[usize; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

/// An interval [lo, hi] of the sweep parameter where a point is inside, intersected with
/// `range`; None when empty.
fn intersect(range: (f64, f64), lo: f64, hi: f64) -> Option<(f64, f64)> {
    let (a, b) = (range.0.max(lo), range.1.min(hi));
    (a <= b).then_some((a, b))
}

/// Whether the cylinder from `h0` to `h1` along unit axis `a` above the tip, radius `r`, swept
/// by the tip from the origin to `d`, contains `w` (relative to the tip's start).
fn in_swept_cylinder(w: DVec3, d: DVec3, a: DVec3, h0: f64, h1: f64, r: f64) -> bool {
    let (wa, da) = (w.dot(a), d.dot(a));
    let (wp, dp) = (w - a * wa, d - a * da);
    let mut s = (0.0, 1.0);
    // Height along the axis: wa - s·da within [h0, h1].
    if da.abs() < 1.0e-12 {
        if wa < h0 || wa > h1 {
            return false;
        }
    } else {
        let (x, y) = ((wa - h1) / da, (wa - h0) / da);
        match intersect(s, x.min(y), x.max(y)) {
            Some(v) => s = v,
            None => return false,
        }
    }
    // Radial distance: |wp - s·dp| <= r.
    let qa = dp.length_squared();
    let qb = wp.dot(dp);
    let qc = wp.length_squared() - r * r;
    if qa < 1.0e-18 {
        return qc <= 0.0;
    }
    let disc = qb * qb - qa * qc;
    if disc < 0.0 {
        return false;
    }
    let root = disc.sqrt();
    intersect(s, (qb - root) / qa, (qb + root) / qa).is_some()
}

fn segment_distance_sq(p: DVec3, a: DVec3, d: DVec3) -> f64 {
    let l2 = d.length_squared();
    let t = if l2 > 1.0e-24 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (a + d * t).distance_squared(p)
}

impl Voxels {
    /// A solid box from `lo` to `hi` in cells of `cell` mm, or None when that would need more
    /// than `max_cells` cells or the box is empty.
    pub fn new(lo: DVec3, hi: DVec3, cell: f64, max_cells: usize) -> Option<Self> {
        if !(lo.is_finite() && hi.is_finite() && cell.is_finite() && cell > 0.0) {
            return None;
        }
        let size = hi - lo;
        if size.min_element() <= 0.0 {
            return None;
        }
        let d = (size / cell).ceil();
        if d.x * d.y * d.z > max_cells as f64 {
            return None;
        }
        let dims = [(d.x as usize).max(1), (d.y as usize).max(1), (d.z as usize).max(1)];
        let n = dims[0] * dims[1] * dims[2];
        let mut bits = vec![u64::MAX; n.div_ceil(64)];
        if let Some(last) = bits.last_mut()
            && !n.is_multiple_of(64)
        {
            *last = (1u64 << (n % 64)) - 1;
        }
        Some(Self { origin: lo, cell, dims, bits, revision: 0 })
    }

    pub fn cell(&self) -> f64 {
        self.cell
    }

    pub fn dims(&self) -> [usize; 3] {
        self.dims
    }

    pub fn origin(&self) -> DVec3 {
        self.origin
    }

    fn index(&self, i: usize, j: usize, k: usize) -> Option<usize> {
        let [nx, ny, nz] = self.dims;
        (i < nx && j < ny && k < nz).then_some((k * ny + j) * nx + i)
    }

    pub fn get(&self, i: usize, j: usize, k: usize) -> bool {
        self.index(i, j, k).and_then(|n| self.bits.get(n / 64).map(|w| w >> (n % 64) & 1 == 1)).unwrap_or(false)
    }

    fn clear_bit(&mut self, n: usize) {
        if let Some(w) = self.bits.get_mut(n / 64) {
            *w &= !(1u64 << (n % 64));
        }
    }

    pub fn centre(&self, i: usize, j: usize, k: usize) -> DVec3 {
        self.origin + (DVec3::new(i as f64, j as f64, k as f64) + 0.5) * self.cell
    }

    /// Solid cells.
    pub fn count(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Solid volume, mm³.
    pub fn volume(&self) -> f64 {
        self.count() as f64 * self.cell.powi(3)
    }

    /// Cell index range covering world `lo`..`hi`, clipped to the grid.
    fn range(&self, lo: DVec3, hi: DVec3) -> Option<[std::ops::Range<usize>; 3]> {
        let a = ((lo - self.origin) / self.cell).floor();
        let b = ((hi - self.origin) / self.cell).ceil();
        let mut out: [std::ops::Range<usize>; 3] = [0..0, 0..0, 0..0];
        for (k, slot) in out.iter_mut().enumerate() {
            let n = self.dims.get(k).copied().unwrap_or(0) as f64;
            let (l, h) = (a[k].max(0.0), b[k].min(n));
            if l >= h || l.is_nan() || h.is_nan() {
                return None;
            }
            *slot = (l as usize)..(h as usize);
        }
        Some(out)
    }

    /// Sweeps `tool` with unit axis `axis` (from the tip up) from tip `from` to tip `to`,
    /// removing what it cuts.
    pub fn cut(&mut self, from: DVec3, to: DVec3, axis: DVec3, tool: &ToolShape) -> VoxelCut {
        let mut out = VoxelCut::default();
        let a = axis.normalize_or_zero();
        if a == DVec3::ZERO || !(from.is_finite() && to.is_finite()) {
            return out;
        }
        let r = tool.radius.max(0.0);
        let flute = tool.flute_length.max(r);
        let length = tool.length.max(flute);
        let shank_r = tool.shank_radius.max(0.0);
        let d = to - from;
        // The box a stretch of the tool from `h0` to `h1` up its axis sweeps, `pad` wide.
        let swept = |h0: f64, h1: f64, pad: f64| {
            let ends = [from + a * h0, to + a * h0, from + a * h1, to + a * h1];
            let (mut lo, mut hi) = (ends[0], ends[0]);
            for e in ends {
                lo = lo.min(e);
                hi = hi.max(e);
            }
            (lo - DVec3::splat(pad), hi + DVec3::splat(pad))
        };
        let ball_start = from + a * r;
        // The tool one cell smaller all round, for telling deep cuts from grazes.
        let c = self.cell;
        let (rs, shank_rs) = ((r - c).max(0.0), (shank_r - c).max(0.0));
        let [nx, ny, _] = self.dims;
        // The tool itself.
        let (lo, hi) = swept(0.0, length, r.max(shank_r) + c);
        if let Some([is, js, ks]) = self.range(lo, hi) {
            for k in ks {
                for j in js.clone() {
                    for i in is.clone() {
                        let n = (k * ny + j) * nx + i;
                        let solid = self.bits.get(n / 64).is_some_and(|w| w >> (n % 64) & 1 == 1);
                        if !solid {
                            continue;
                        }
                        let p = self.centre(i, j, k);
                        let w = p - from;
                        let flutes = if tool.ball {
                            segment_distance_sq(p, ball_start, d) <= r * r || in_swept_cylinder(w, d, a, r, flute, r)
                        } else {
                            in_swept_cylinder(w, d, a, 0.0, flute, r)
                        };
                        if flutes {
                            out.removed += 1;
                            let h = w.dot(a);
                            out.lowest = Some(out.lowest.map_or(h, |l: f64| l.min(h)));
                            let deep = if tool.ball {
                                segment_distance_sq(p, ball_start, d) <= rs * rs
                                    || in_swept_cylinder(w, d, a, r, flute, rs)
                            } else {
                                in_swept_cylinder(w, d, a, c, flute, rs)
                            };
                            out.deep += usize::from(deep);
                            self.clear_bit(n);
                        } else if shank_r > 0.0 && in_swept_cylinder(w, d, a, flute, length, shank_r) {
                            out.shank += 1;
                            if in_swept_cylinder(w, d, a, flute + c, length, shank_rs) {
                                out.deep_shank += 1;
                                out.shank_at.get_or_insert(p);
                            }
                            self.clear_bit(n);
                        }
                    }
                }
            }
        }
        // The holder, in its own box: usually well above the stock, so nothing to look at.
        for &(h0, h1, hr) in &tool.holder {
            if !(hr > 0.0 && h1 > h0) {
                continue;
            }
            let (lo, hi) = swept(length + h0, length + h1, hr + c);
            let Some([is, js, ks]) = self.range(lo, hi) else { continue };
            for k in ks {
                for j in js.clone() {
                    for i in is.clone() {
                        let n = (k * ny + j) * nx + i;
                        let solid = self.bits.get(n / 64).is_some_and(|w| w >> (n % 64) & 1 == 1);
                        if !solid {
                            continue;
                        }
                        let p = self.centre(i, j, k);
                        if in_swept_cylinder(p - from, d, a, length + h0, length + h1, (hr - c).max(0.0)) {
                            out.holder += 1;
                            out.holder_at.get_or_insert(p);
                            self.clear_bit(n);
                        }
                    }
                }
            }
        }
        if out.removed + out.shank > 0 {
            self.revision = self.revision.wrapping_add(1);
        }
        out
    }

    /// For every cell, whether its centre is inside the closed mesh `tris` (world mm), by
    /// counting crossings of a vertical line through each column of cells.
    pub fn part_cells(&self, tris: &[Triangle]) -> Vec<bool> {
        let [nx, ny, nz] = self.dims;
        let mut columns: Vec<Vec<f64>> = vec![Vec::new(); nx * ny];
        for t in tris {
            let (mut lo, mut hi) = (t[0], t[0]);
            for p in t {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
            let n = (t[1] - t[0]).cross(t[2] - t[0]);
            if n.z.abs() < 1.0e-12 {
                continue;
            }
            let Some([is, js, _]) = self.range(lo.with_z(self.origin.z), hi.with_z(self.origin.z + self.cell)) else {
                continue;
            };
            for j in js {
                for i in is.clone() {
                    // A hair off the cell centre, so no sample lands exactly on an edge two
                    // triangles share (it would count twice or not at all).
                    let c = self.centre(i, j, 0) + DVec3::new(3.14159e-7, 2.71828e-7, 0.0);
                    // Inside the triangle's XY projection.
                    let s = |a: DVec3, b: DVec3| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
                    let (d0, d1, d2) = (s(t[0], t[1]), s(t[1], t[2]), s(t[2], t[0]));
                    let inside = (d0 > 0.0 && d1 > 0.0 && d2 > 0.0) || (d0 < 0.0 && d1 < 0.0 && d2 < 0.0);
                    if !inside {
                        continue;
                    }
                    let z = t[0].z - (n.x * (c.x - t[0].x) + n.y * (c.y - t[0].y)) / n.z;
                    if let Some(col) = columns.get_mut(j * nx + i) {
                        col.push(z);
                    }
                }
            }
        }
        let mut out = vec![false; nx * ny * nz];
        for j in 0..ny {
            for i in 0..nx {
                let Some(col) = columns.get_mut(j * nx + i) else { continue };
                col.sort_by(f64::total_cmp);
                for &[z0, z1] in col.as_chunks::<2>().0 {
                    for k in 0..nz {
                        let z = self.centre(i, j, k).z;
                        if z > z0
                            && z < z1
                            && let Some(slot) = out.get_mut((k * ny + j) * nx + i)
                        {
                            *slot = true;
                        }
                    }
                }
            }
        }
        out
    }

    /// Cells that were removed although they lie inside the part by more than a cell (all six
    /// neighbours are part too): gouges. `part` comes from [`Voxels::part_cells`].
    pub fn gouges(&self, part: &[bool], limit: usize) -> Vec<[usize; 3]> {
        let [nx, ny, nz] = self.dims;
        let is_part = |i: usize, j: usize, k: usize| part.get((k * ny + j) * nx + i).copied().unwrap_or(false);
        let mut out = Vec::new();
        for k in 1..nz.saturating_sub(1) {
            for j in 1..ny.saturating_sub(1) {
                for i in 1..nx.saturating_sub(1) {
                    if self.get(i, j, k) || !is_part(i, j, k) {
                        continue;
                    }
                    let deep = is_part(i - 1, j, k)
                        && is_part(i + 1, j, k)
                        && is_part(i, j - 1, k)
                        && is_part(i, j + 1, k)
                        && is_part(i, j, k - 1)
                        && is_part(i, j, k + 1);
                    if deep {
                        out.push([i, j, k]);
                        if out.len() >= limit {
                            return out;
                        }
                    }
                }
            }
        }
        out
    }

    /// The solid's surface as a smooth mesh (surface nets), sampling every `step` cells: a
    /// coarse cell is solid when every fine cell in it is, so narrow cuts stay visible.
    pub fn surface(&self, step: usize) -> Surface {
        let step = step.max(1);
        let [fx, fy, fz] = self.dims;
        let (nx, ny, nz) = (fx.div_ceil(step), fy.div_ceil(step), fz.div_ceil(step));
        let cell = self.cell * step as f64;
        let solid: Vec<bool> = {
            let mut v = vec![false; nx * ny * nz];
            for k in 0..nz {
                for j in 0..ny {
                    for i in 0..nx {
                        let mut all = true;
                        'block: for kk in k * step..((k + 1) * step).min(fz) {
                            for jj in j * step..((j + 1) * step).min(fy) {
                                for ii in i * step..((i + 1) * step).min(fx) {
                                    if !self.get(ii, jj, kk) {
                                        all = false;
                                        break 'block;
                                    }
                                }
                            }
                        }
                        if let Some(s) = v.get_mut((k * ny + j) * nx + i) {
                            *s = all;
                        }
                    }
                }
            }
            v
        };
        // Coarse cell (i, j, k) with i in -1..=n: outside the grid is empty; `inside_box` tells
        // the original stock (cut when empty) from the space around it.
        let at = |i: isize, j: isize, k: isize| -> (bool, bool) {
            if i < 0 || j < 0 || k < 0 || i >= nx as isize || j >= ny as isize || k >= nz as isize {
                return (false, false);
            }
            let n = (k as usize * ny + j as usize) * nx + i as usize;
            (solid.get(n).copied().unwrap_or(false), true)
        };
        let centre =
            |i: isize, j: isize, k: isize| self.origin + (DVec3::new(i as f64, j as f64, k as f64) + 0.5) * cell;
        let mut out = Surface::default();
        // One vertex per dual cell (the cube between eight cell centres) that the surface crosses.
        let (vx, vy, vz) = (nx + 1, ny + 1, nz + 1);
        let mut vertex = vec![u32::MAX; vx * vy * vz];
        let corners: [(isize, isize, isize); 8] =
            [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0), (0, 0, 1), (1, 0, 1), (0, 1, 1), (1, 1, 1)];
        let edges: [(usize, usize); 12] =
            [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
        for k in 0..vz {
            for j in 0..vy {
                for i in 0..vx {
                    let base = (i as isize - 1, j as isize - 1, k as isize - 1);
                    let states: Vec<(bool, bool)> =
                        corners.iter().map(|(a, b, c)| at(base.0 + a, base.1 + b, base.2 + c)).collect();
                    let filled = states.iter().filter(|s| s.0).count();
                    if filled == 0 || filled == 8 {
                        continue;
                    }
                    let mut sum = DVec3::ZERO;
                    let mut crossings = 0.0_f64;
                    for (e0, e1) in edges {
                        let (Some(s0), Some(s1)) = (states.get(e0), states.get(e1)) else { continue };
                        if s0.0 != s1.0 {
                            let (Some(c0), Some(c1)) = (corners.get(e0), corners.get(e1)) else { continue };
                            let p0 = centre(base.0 + c0.0, base.1 + c0.1, base.2 + c0.2);
                            let p1 = centre(base.0 + c1.0, base.1 + c1.1, base.2 + c1.2);
                            sum += (p0 + p1) * 0.5;
                            crossings += 1.0;
                        }
                    }
                    let cut = states.iter().any(|s| s.1 && !s.0);
                    if let Some(slot) = vertex.get_mut((k * vy + j) * vx + i) {
                        *slot = out.positions.len() as u32;
                    }
                    out.positions.push((sum / crossings.max(1.0)).as_vec3());
                    out.normals.push(Vec3::ZERO);
                    out.cut.push(cut);
                    out.cells.push([
                        (i.saturating_sub(1) * step).min(fx.saturating_sub(1)),
                        (j.saturating_sub(1) * step).min(fy.saturating_sub(1)),
                        (k.saturating_sub(1) * step).min(fz.saturating_sub(1)),
                    ]);
                }
            }
        }
        let vid = |i: isize, j: isize, k: isize| -> Option<u32> {
            if i < 0 || j < 0 || k < 0 {
                return None;
            }
            vertex.get((k as usize * vy + j as usize) * vx + i as usize).copied().filter(|v| *v != u32::MAX)
        };
        // Each grid edge between a solid and an empty cell gets a quad of the four dual vertices
        // around it, facing out of the solid.
        for k in -1..nz as isize {
            for j in -1..ny as isize {
                for i in -1..nx as isize {
                    let here = at(i, j, k).0;
                    for axis in 0..3 {
                        let (di, dj, dk) = [(1, 0, 0), (0, 1, 0), (0, 0, 1)][axis];
                        let there = at(i + di, j + dj, k + dk).0;
                        if here == there {
                            continue;
                        }
                        // Dual vertex (a, b, c) sits between cells (a-1..a, ...): the four around
                        // this edge share its far index along the axis.
                        let (a, b, c) = (i + 1, j + 1, k + 1);
                        let quad = match axis {
                            0 => [(a, b - 1, c - 1), (a, b, c - 1), (a, b, c), (a, b - 1, c)],
                            1 => [(a - 1, b, c - 1), (a - 1, b, c), (a, b, c), (a, b, c - 1)],
                            _ => [(a - 1, b - 1, c), (a, b - 1, c), (a, b, c), (a - 1, b, c)],
                        };
                        let ids: Vec<u32> = quad.iter().filter_map(|(x, y, z)| vid(*x, *y, *z)).collect();
                        let [q0, q1, q2, q3] = ids[..] else { continue };
                        // Facing from the solid towards the empty side.
                        let tris = if here { [[q0, q1, q2], [q0, q2, q3]] } else { [[q0, q2, q1], [q0, q3, q2]] };
                        out.triangles.extend(tris);
                    }
                }
            }
        }
        for t in &out.triangles {
            let p: Vec<Vec3> = t.iter().filter_map(|i| out.positions.get(*i as usize).copied()).collect();
            let [p0, p1, p2] = p[..] else { continue };
            let n = (p1 - p0).cross(p2 - p0);
            for i in t {
                if let Some(slot) = out.normals.get_mut(*i as usize) {
                    *slot += n;
                }
            }
        }
        for n in &mut out.normals {
            *n = n.normalize_or_zero();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block() -> Voxels {
        Voxels::new(DVec3::ZERO, DVec3::new(20.0, 20.0, 10.0), 0.5, 1 << 22).unwrap_or_else(|| Voxels {
            origin: DVec3::ZERO,
            cell: 1.0,
            dims: [0; 3],
            bits: Vec::new(),
            revision: 0,
        })
    }

    fn flat(d: f64) -> ToolShape {
        ToolShape {
            radius: d / 2.0,
            ball: false,
            flute_length: 15.0,
            shank_radius: d / 2.0,
            length: 40.0,
            holder: Vec::new(),
        }
    }

    #[test]
    fn a_vertical_slot_removes_its_swept_volume() {
        let mut v = block();
        let full = v.volume();
        assert!((full - 4000.0).abs() < 1.0e-6);
        // A 4 mm slot 2 mm deep across the block.
        let c = v.cut(DVec3::new(-5.0, 10.0, 8.0), DVec3::new(25.0, 10.0, 8.0), DVec3::Z, &flat(4.0));
        let slot = full - v.volume();
        assert!((slot - 20.0 * 4.0 * 2.0).abs() < 8.0, "{slot}");
        assert_eq!(c.shank, 0);
        assert!(!v.get(20, 20, 17) && v.get(20, 20, 15) && v.get(20, 30, 17));
    }

    #[test]
    fn a_tilted_tool_cuts_along_its_own_axis() {
        // A flat end mill tilted 45° in XZ plunging along its axis into the top face.
        let mut v = block();
        let a = DVec3::new(-1.0, 0.0, 1.0).normalize();
        let tip = DVec3::new(10.0, 10.0, 10.0) - a * 3.0;
        v.cut(DVec3::new(10.0, 10.0, 10.0) + a * 2.0, tip, a, &flat(2.0));
        // Along the tilted axis the material is gone; straight below the entry it is not.
        let probe = DVec3::new(10.0, 10.0, 10.0) - a * 2.0;
        let cell = |p: DVec3| ((p.x / 0.5) as usize, (p.y / 0.5) as usize, (p.z / 0.5) as usize);
        let (i, j, k) = cell(probe);
        assert!(!v.get(i, j, k));
        let (i, j, k) = cell(DVec3::new(10.0, 10.0, 7.0));
        assert!(v.get(i, j, k));
    }

    #[test]
    fn a_ball_leaves_a_round_bottom_and_the_shank_is_reported() {
        let mut v = block();
        let tool = ToolShape {
            radius: 3.0,
            ball: true,
            flute_length: 5.0,
            shank_radius: 3.0,
            length: 40.0,
            holder: Vec::new(),
        };
        // Plunging straight down, the flutes clear the way for the shank.
        let c = v.cut(DVec3::new(10.0, 10.0, 12.0), DVec3::new(10.0, 10.0, 2.0), DVec3::Z, &tool);
        assert_eq!(c.shank, 0, "{c:?}");
        // At the bottom, the ball left material 2.75 mm off centre, 0.75 mm above the tip
        // (the ball is only √(9 - 2.25²) ≈ 1.98 mm wide there).
        let (i, j, k) = (((10.0 + 2.75) / 0.5) as usize, 20, ((2.0 + 0.75) / 0.5) as usize);
        assert!(v.get(i, j, k));
        assert!(!v.get(20, 20, ((2.0 + 0.75) / 0.5) as usize));
        // Moving sideways 8 mm deep with 5 mm of flutes, the shank rubs the top 3 mm.
        let c = v.cut(DVec3::new(10.0, 10.0, 2.0), DVec3::new(16.0, 10.0, 2.0), DVec3::Z, &tool);
        assert!(c.shank > 0 && c.deep_shank > 0 && c.shank_at.is_some_and(|p| p.z > 7.0), "{c:?}");
        // Brushing the side of the cut by less than a cell is no deep cut.
        let graze = v.cut(DVec3::new(10.0, 10.3, 2.5), DVec3::new(16.0, 10.3, 2.5), DVec3::Z, &tool);
        assert_eq!(graze.deep, 0, "{graze:?}");
    }

    #[test]
    fn a_holder_that_reaches_the_stock_is_reported() {
        let mut v = block();
        // 8 mm of stick-out and a Ø20 holder above it, the tip 9 mm below the block's top: the
        // holder's bottom 1 mm is level with the stock.
        let tool = ToolShape {
            radius: 2.0,
            ball: false,
            flute_length: 6.0,
            shank_radius: 2.0,
            length: 8.0,
            holder: vec![(0.0, 20.0, 10.0)],
        };
        // Beside the block (x 0..20): the tool clears it by 7 mm, the holder reaches 1 mm in.
        let c = v.cut(DVec3::new(-13.0, 10.0, 1.0), DVec3::new(-9.0, 10.0, 1.0), DVec3::Z, &tool);
        assert!(c.removed == 0 && c.holder > 0 && c.holder_at.is_some_and(|p| p.z > 9.0), "{c:?}");
        let mut v = block();
        let c = v.cut(DVec3::new(-13.0, 10.0, 4.0), DVec3::new(-9.0, 10.0, 4.0), DVec3::Z, &tool);
        assert_eq!(c.holder, 0, "{c:?}");
    }

    #[test]
    fn gouges_are_removed_cells_deep_in_the_part() {
        let mut v = block();
        let c = |x: f64, y: f64, z: f64| DVec3::new(x, y, z);
        // A part filling the lower half of the block.
        let (x, y, z) = (20.0, 20.0, 5.0);
        let quads = [
            [c(0., 0., z), c(x, 0., z), c(x, y, z), c(0., y, z)],
            [c(0., 0., 0.), c(0., y, 0.), c(x, y, 0.), c(x, 0., 0.)],
        ];
        let tris: Vec<Triangle> = quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect();
        let part = v.part_cells(&tris);
        assert_eq!(part.iter().filter(|p| **p).count(), 40 * 40 * 10);
        // Cutting down to the part's top is no gouge; 2 mm into it is.
        v.cut(c(-5.0, 10.0, 5.0), c(25.0, 10.0, 5.0), DVec3::Z, &flat(4.0));
        assert!(v.gouges(&part, 10).is_empty());
        v.cut(c(-5.0, 10.0, 3.0), c(25.0, 10.0, 3.0), DVec3::Z, &flat(4.0));
        assert!(!v.gouges(&part, 10).is_empty());
    }

    #[test]
    fn the_surface_is_closed_and_marks_cuts() {
        let mut v = Voxels::new(DVec3::ZERO, DVec3::splat(4.0), 1.0, 1000).unwrap_or_else(block);
        let s = v.surface(1);
        // A closed mesh: every edge is shared by exactly two triangles.
        let mut edges = std::collections::HashMap::new();
        for t in &s.triangles {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        assert!(!s.triangles.is_empty() && edges.values().all(|n| *n == 2));
        assert!(s.cut.iter().all(|c| !c));
        v.cut(DVec3::new(2.0, 2.0, 5.0), DVec3::new(2.0, 2.0, 2.0), DVec3::Z, &flat(2.0));
        assert!(v.surface(1).cut.iter().any(|c| *c));
    }
}
