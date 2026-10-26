//! Adaptive clearing: a tool path that keeps the cutter's engagement near a target load.
//!
//! The material is a grid of cells. The tool walks in small steps; at each one it tries
//! headings from a hard turn into the material to a hard turn away and takes the one whose
//! newly removed area is closest to the target (the load times the step), keeping the
//! material on the climb side. Cells under the tool are cleared as it goes, so the path
//! spirals out from its entry, follows the walls and turns away from corners instead of
//! burying itself in them. When nothing is left within reach the pass ends and the next one
//! starts beside the nearest material left: linked at depth where the way there is clear,
//! from above where it is not, with a helix only into fresh material.
//!
//! The tool centre stays in `allowed`; material it cannot reach (closer to a wall than the
//! radius allows) is left for a finishing pass along the walls.

use glam::DVec2;

use crate::region::{self, Shape};

/// Most points in all passes of one call.
const MAX_POINTS: usize = 4_000_000;
/// Most passes in one call.
const MAX_PASSES: usize = 100_000;
/// Most steps one pass takes with almost nothing to cut before it ends.
const IDLE_STEPS: usize = 6;

/// A cell grid over a rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub origin: DVec2,
    pub res: f64,
    pub nx: usize,
    pub ny: usize,
}

impl Grid {
    /// A grid over `lo`..`hi` with cells of `res`, coarsened until it has at most
    /// `max_cells` cells.
    pub fn new(lo: DVec2, hi: DVec2, res: f64, max_cells: usize) -> Option<Self> {
        let size = hi - lo;
        if !(size.x > 0.0 && size.y > 0.0 && res > 0.0) || !size.is_finite() || !res.is_finite() {
            return None;
        }
        let min_res = (size.x * size.y / max_cells.max(1) as f64).sqrt();
        let res = res.max(min_res);
        let nx = (size.x / res).ceil() as usize + 1;
        let ny = (size.y / res).ceil() as usize + 1;
        (nx.saturating_mul(ny) <= max_cells.max(1) * 2).then_some(Self { origin: lo, res, nx, ny })
    }

    pub fn centre(&self, i: usize, j: usize) -> DVec2 {
        self.origin + DVec2::new((i as f64 + 0.5) * self.res, (j as f64 + 0.5) * self.res)
    }

    fn index(&self, i: usize, j: usize) -> usize {
        j * self.nx + i
    }

    /// The cell holding `p`, if it is on the grid.
    pub fn cell(&self, p: DVec2) -> Option<(usize, usize)> {
        let q = (p - self.origin) / self.res;
        if !(q.x >= 0.0 && q.y >= 0.0) {
            return None;
        }
        let (i, j) = (q.x as usize, q.y as usize);
        (i < self.nx && j < self.ny).then_some((i, j))
    }

    pub fn len(&self) -> usize {
        self.nx * self.ny
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Which cell centres lie inside `shapes` (even-odd over all their contours), row by row.
pub fn raster(grid: &Grid, shapes: &[Shape]) -> Vec<bool> {
    let mut out = vec![false; grid.len()];
    let edges: Vec<(DVec2, DVec2)> = shapes
        .iter()
        .flatten()
        .flat_map(|c| (0..c.len()).filter_map(move |k| Some((*c.get(k)?, *c.get((k + 1) % c.len())?))))
        .filter(|(a, b)| a.y != b.y)
        .collect();
    let mut xs: Vec<f64> = Vec::new();
    for j in 0..grid.ny {
        let y = grid.centre(0, j).y;
        xs.clear();
        for (a, b) in &edges {
            if (a.y > y) != (b.y > y) {
                xs.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
            }
        }
        xs.sort_by(f64::total_cmp);
        for pair in xs.as_chunks::<2>().0 {
            let i0 = ((pair[0] - grid.origin.x) / grid.res - 0.5).ceil().max(0.0) as usize;
            let i1 = ((pair[1] - grid.origin.x) / grid.res - 0.5).floor();
            if i1 < 0.0 {
                continue;
            }
            let i1 = (i1 as usize).min(grid.nx.saturating_sub(1));
            for i in i0..=i1 {
                if let Some(c) = out.get_mut(grid.index(i, j)) {
                    *c = true;
                }
            }
        }
    }
    out
}

/// Distance (cell units, chamfer 3-4 approximation ÷ 3) from each cell to the nearest cell
/// where `from` is true.
fn distance_to(grid: &Grid, from: &[bool]) -> Vec<f64> {
    const FAR: u32 = u32::MAX / 4;
    let (nx, ny) = (grid.nx, grid.ny);
    let mut d: Vec<u32> = from.iter().map(|&f| if f { 0 } else { FAR }).collect();
    let at = |i: usize, j: usize| j * nx + i;
    for j in 0..ny {
        for i in 0..nx {
            let mut v = d[at(i, j)];
            if i > 0 {
                v = v.min(d[at(i - 1, j)] + 3);
            }
            if j > 0 {
                v = v.min(d[at(i, j - 1)] + 3);
                if i > 0 {
                    v = v.min(d[at(i - 1, j - 1)] + 4);
                }
                if i + 1 < nx {
                    v = v.min(d[at(i + 1, j - 1)] + 4);
                }
            }
            d[at(i, j)] = v;
        }
    }
    for j in (0..ny).rev() {
        for i in (0..nx).rev() {
            let mut v = d[at(i, j)];
            if i + 1 < nx {
                v = v.min(d[at(i + 1, j)] + 3);
            }
            if j + 1 < ny {
                v = v.min(d[at(i, j + 1)] + 3);
                if i + 1 < nx {
                    v = v.min(d[at(i + 1, j + 1)] + 4);
                }
                if i > 0 {
                    v = v.min(d[at(i - 1, j + 1)] + 4);
                }
            }
            d[at(i, j)] = v;
        }
    }
    d.into_iter().map(|v| f64::from(v) / 3.0).collect()
}

/// One bit per cell, rows of whole words, for counting cells under the tool fast.
struct Bits {
    words: usize,
    data: Vec<u64>,
}

impl Bits {
    fn new(grid: &Grid, cells: &[bool]) -> Self {
        let words = grid.nx.div_ceil(64);
        let mut data = vec![0u64; words * grid.ny];
        for j in 0..grid.ny {
            for i in 0..grid.nx {
                if cells.get(j * grid.nx + i).copied().unwrap_or(false)
                    && let Some(w) = data.get_mut(j * words + i / 64)
                {
                    *w |= 1u64 << (i % 64);
                }
            }
        }
        Self { words, data }
    }

    fn get(&self, i: usize, j: usize) -> bool {
        self.data.get(j * self.words + i / 64).is_some_and(|w| w >> (i % 64) & 1 == 1)
    }

    fn unset(&mut self, i: usize, j: usize) {
        if let Some(w) = self.data.get_mut(j * self.words + i / 64) {
            *w &= !(1u64 << (i % 64));
        }
    }

    /// Mask of bits `lo..=hi` within one word (0 ≤ lo ≤ hi < 64).
    fn mask(lo: usize, hi: usize) -> u64 {
        let upper = if hi >= 63 { u64::MAX } else { (1u64 << (hi + 1)) - 1 };
        upper & !((1u64 << lo) - 1)
    }

    /// Set cells `i0..=i1` in row `j`; with `clear`, unsets them as well.
    fn span(&mut self, j: usize, i0: usize, i1: usize, clear: bool) -> u32 {
        let mut n = 0;
        let (w0, w1) = (i0 / 64, i1 / 64);
        for w in w0..=w1 {
            let lo = if w == w0 { i0 % 64 } else { 0 };
            let hi = if w == w1 { i1 % 64 } else { 63 };
            let m = Self::mask(lo, hi);
            if let Some(word) = self.data.get_mut(j * self.words + w) {
                n += (*word & m).count_ones();
                if clear {
                    *word &= !m;
                }
            }
        }
        n
    }

    fn count(&self, j: usize, i0: usize, i1: usize) -> u32 {
        let mut n = 0;
        let (w0, w1) = (i0 / 64, i1 / 64);
        for w in w0..=w1 {
            let lo = if w == w0 { i0 % 64 } else { 0 };
            let hi = if w == w1 { i1 % 64 } else { 63 };
            if let Some(word) = self.data.get(j * self.words + w) {
                n += (*word & Self::mask(lo, hi)).count_ones();
            }
        }
        n
    }

    #[cfg(test)]
    fn total(&self) -> usize {
        self.data.iter().map(|w| w.count_ones() as usize).sum()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Tool radius.
    pub radius: f64,
    /// Target radial engagement (the "optimal load"), as a width of cut.
    pub load: f64,
    /// Material on the right of the path (climb milling with a clockwise spindle).
    pub climb: bool,
    /// Largest helix radius for entries into fresh material.
    pub helix_radius: f64,
    /// Longest way linked at depth through cleared ground; farther, the tool lifts.
    pub stay_down: f64,
}

/// How a pass is entered from where the last one ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Entry {
    /// Feed straight across at depth: the way is clear.
    Link,
    /// Lift, move over and come down: the start is clear (or nearly) at this depth.
    Retract,
    /// Lift, move over and helix down into fresh material, around the start, with this radius
    /// (0: plunge).
    Helix(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    pub entry: Entry,
    /// For a linked pass: the way there at depth, between the last pass's end and `pts[0]`.
    pub link: Vec<DVec2>,
    pub pts: Vec<DVec2>,
}

struct State<'a> {
    grid: Grid,
    /// Material still to cut, per cell.
    material: Bits,
    /// Where the tool centre may be, per cell (inside `allowed` with room to spare).
    allow: Vec<bool>,
    /// Distance from each cell to the nearest cell the centre may not be in (cell units).
    room: Vec<f64>,
    s: &'a Settings,
}

impl State<'_> {
    fn allowed(&self, p: DVec2) -> bool {
        self.grid.cell(p).is_some_and(|(i, j)| self.allow.get(self.grid.index(i, j)).copied().unwrap_or(false))
    }

    /// For each grid row a disc of radius `r` at `p` covers: (row, first cell, last cell).
    fn rows(&self, p: DVec2, r: f64) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
        let g = self.grid;
        let j0 = ((p.y - r - g.origin.y) / g.res - 0.5).ceil().max(0.0) as usize;
        let j1 = ((p.y + r - g.origin.y) / g.res - 0.5).floor();
        let j1 = if j1 < 0.0 { 0 } else { (j1 as usize + 1).min(g.ny) };
        (j0..j1).filter_map(move |j| {
            let dy = g.origin.y + (j as f64 + 0.5) * g.res - p.y;
            let half2 = r * r - dy * dy;
            if half2 < 0.0 {
                return None;
            }
            let half = half2.sqrt();
            let i0 = ((p.x - half - g.origin.x) / g.res - 0.5).ceil().max(0.0);
            let i1 = ((p.x + half - g.origin.x) / g.res - 0.5).floor();
            if i1 < i0 || i1 < 0.0 {
                return None;
            }
            let i1 = (i1 as usize).min(g.nx.saturating_sub(1));
            let i0 = i0 as usize;
            (i0 <= i1).then_some((j, i0, i1))
        })
    }

    /// Material cells under the tool at `p`.
    fn engagement(&self, p: DVec2) -> usize {
        self.rows(p, self.s.radius).map(|(j, i0, i1)| self.material.count(j, i0, i1) as usize).sum()
    }

    /// Clears the cells under a tool of radius `r` at `p`.
    fn clear(&mut self, p: DVec2, r: f64) {
        let spans: Vec<(usize, usize, usize)> = self.rows(p, r).collect();
        for (j, i0, i1) in spans {
            self.material.span(j, i0, i1, true);
        }
    }

    /// The straight move `a`→`b` crosses no material and stays where the centre may go.
    fn clear_way(&self, a: DVec2, b: DVec2) -> bool {
        let n = ((a.distance(b) / self.grid.res).ceil() as usize).clamp(1, 100_000);
        (0..=n).all(|k| {
            let q = a.lerp(b, k as f64 / n as f64);
            self.allowed(q) && self.engagement(q) <= 1
        })
    }

    /// A way at depth from `a` to `b` through cleared ground, at most `max_len` long: the
    /// points between them (none for a straight way), or None.
    fn route(&self, a: DVec2, b: DVec2, max_len: f64) -> Option<Vec<DVec2>> {
        use std::cmp::Reverse;
        use std::collections::{BinaryHeap, HashMap};
        if a.distance(b) > max_len {
            return None;
        }
        if self.clear_way(a, b) {
            return Some(Vec::new());
        }
        let g = &self.grid;
        let (sa, sb) = (g.cell(a)?, g.cell(b)?);
        let (start, goal) = (g.index(sa.0, sa.1), g.index(sb.0, sb.1));
        let mut passable: HashMap<usize, bool> = HashMap::new();
        let mut cost: HashMap<usize, f64> = HashMap::new();
        let mut came: HashMap<usize, usize> = HashMap::new();
        let mut open = BinaryHeap::new();
        let h = |k: usize| g.centre(k % g.nx, k / g.nx).distance(b);
        cost.insert(start, 0.0);
        open.push((Reverse(h(start).to_bits()), start));
        let mut expanded = 0usize;
        let mut found = false;
        while let Some((_, k)) = open.pop() {
            if k == goal {
                found = true;
                break;
            }
            expanded += 1;
            if expanded > 200_000 {
                return None;
            }
            let here = cost.get(&k).copied().unwrap_or(f64::INFINITY);
            let (i, j) = ((k % g.nx) as i64, (k / g.nx) as i64);
            for (di, dj, step) in [
                (1, 0, 1.0),
                (-1, 0, 1.0),
                (0, 1, 1.0),
                (0, -1, 1.0),
                (1, 1, std::f64::consts::SQRT_2),
                (1, -1, std::f64::consts::SQRT_2),
                (-1, 1, std::f64::consts::SQRT_2),
                (-1, -1, std::f64::consts::SQRT_2),
            ] {
                let (ni, nj) = (i + di, j + dj);
                if ni < 0 || nj < 0 || ni as usize >= g.nx || nj as usize >= g.ny {
                    continue;
                }
                let n = nj as usize * g.nx + ni as usize;
                let ok = *passable.entry(n).or_insert_with(|| {
                    n == goal
                        || (self.allow.get(n).copied().unwrap_or(false)
                            && self.engagement(g.centre(ni as usize, nj as usize)) <= 1)
                });
                if !ok {
                    continue;
                }
                let next = here + step * g.res;
                if next > max_len || next >= cost.get(&n).copied().unwrap_or(f64::INFINITY) {
                    continue;
                }
                cost.insert(n, next);
                came.insert(n, k);
                open.push((Reverse((next + h(n)).to_bits()), n));
            }
        }
        if !found {
            return None;
        }
        let mut cells = vec![goal];
        while let Some(&prev) = cells.last().and_then(|c| came.get(c)) {
            cells.push(prev);
            if prev == start || cells.len() > 1_000_000 {
                break;
            }
        }
        cells.reverse();
        let mut pts: Vec<DVec2> = cells.iter().map(|&k| g.centre(k % g.nx, k / g.nx)).collect();
        if let Some(f) = pts.first_mut() {
            *f = a;
        }
        if let Some(l) = pts.last_mut() {
            *l = b;
        }
        // Pull the string tight: skip ahead while the straight way stays clear.
        let mut out = Vec::new();
        let mut i = 0;
        while i + 1 < pts.len() {
            let mut j = pts.len() - 1;
            while j > i + 1 && !pts.get(i).zip(pts.get(j)).is_some_and(|(p, q)| self.clear_way(*p, *q)) {
                j = i + 1 + (j - i - 1) / 2;
            }
            if j + 1 < pts.len()
                && let Some(q) = pts.get(j)
            {
                out.push(*q);
            }
            i = j;
        }
        Some(out)
    }

    /// The nearest allowed centre to `from` that cuts a little (gently, from cleared ground or
    /// open air) without burying the tool.
    fn gentle_start(&self, from: DVec2, target: f64) -> Option<DVec2> {
        let g = &self.grid;
        let mut best: Option<(DVec2, f64)> = None;
        for j in 0..g.ny {
            for i in 0..g.nx {
                if !self.allow.get(g.index(i, j)).copied().unwrap_or(false) {
                    continue;
                }
                let q = g.centre(i, j);
                let d = q.distance_squared(from);
                if best.is_some_and(|(_, b)| d >= b) {
                    continue;
                }
                let e = self.engagement(q);
                if e > 0 && (e as f64) <= target * 2.0 {
                    best = Some((q, d));
                }
            }
        }
        best.map(|(q, _)| q)
    }

    /// The nearest material cell to `from` that any allowed centre can reach.
    fn nearest_material(&self, from: DVec2) -> Option<(usize, usize)> {
        let g = &self.grid;
        let mut best: Option<((usize, usize), f64)> = None;
        for j in 0..g.ny {
            for w in 0..self.material.words {
                let Some(&word) = self.material.data.get(j * self.material.words + w) else { continue };
                let mut bits = word;
                while bits != 0 {
                    let i = w * 64 + bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    let d = g.centre(i, j).distance_squared(from);
                    if best.is_none_or(|(_, b)| d < b) {
                        best = Some(((i, j), d));
                    }
                }
            }
        }
        best.map(|(c, _)| c)
    }

    /// Where to start next to reach material cell `m`, and how to get there from `from`.
    fn start_near(&self, m: (usize, usize), from: Option<DVec2>, target: f64) -> Option<(DVec2, Entry)> {
        let g = &self.grid;
        let mp = g.centre(m.0, m.1);
        let reach = (self.s.radius / g.res).ceil() as i64 + 1;
        // The gentlest allowed centre that touches the material: the least engaged one.
        let mut best: Option<(DVec2, usize, f64)> = None;
        for dj in -reach..=reach {
            for di in -reach..=reach {
                let (i, j) = (m.0 as i64 + di, m.1 as i64 + dj);
                if i < 0 || j < 0 || i as usize >= g.nx || j as usize >= g.ny {
                    continue;
                }
                let (i, j) = (i as usize, j as usize);
                if !self.allow.get(g.index(i, j)).copied().unwrap_or(false) {
                    continue;
                }
                let q = g.centre(i, j);
                if q.distance(mp) > self.s.radius {
                    continue;
                }
                let e = self.engagement(q);
                if e == 0 {
                    continue;
                }
                let d = from.map_or(0.0, |f| f.distance(q));
                if best.is_none_or(|(_, be, bd)| e < be || (e == be && d < bd)) {
                    best = Some((q, e, d));
                }
            }
        }
        let (q, e, _) = best?;
        // A sliver along a wall is left for the finishing pass along it.
        if (e as f64) < target * 0.5 && self.room_at(q) < self.s.radius * 0.5 {
            // Marked with a negative helix radius: cleared without a pass.
            return Some((q, Entry::Helix(-1.0)));
        }
        if (e as f64) <= target * 2.0 {
            let entry = match from {
                Some(f) if self.clear_way(f, q) => Entry::Link,
                _ => Entry::Retract,
            };
            return Some((q, entry));
        }
        // Fresh material: helix down where there is the most room nearby.
        let mut deepest = (q, self.room_at(q));
        for dj in -reach * 2..=reach * 2 {
            for di in -reach * 2..=reach * 2 {
                let (i, j) = (m.0 as i64 + di, m.1 as i64 + dj);
                if i < 0 || j < 0 || i as usize >= g.nx || j as usize >= g.ny {
                    continue;
                }
                let c = g.centre(i as usize, j as usize);
                if !self.allowed(c) || self.engagement(c) == 0 {
                    continue;
                }
                let room = self.room_at(c);
                if room > deepest.1 + 1.0e-9 {
                    deepest = (c, room);
                }
            }
        }
        let helix = (deepest.1 - g.res).min(self.s.helix_radius).max(0.0);
        Some((deepest.0, Entry::Helix(if helix >= self.s.radius * 0.1 { helix } else { 0.0 })))
    }

    /// The allowed centre over material with the most room around it, and a helix there.
    fn deepest(&self) -> Option<(DVec2, Entry)> {
        let g = &self.grid;
        let mut best: Option<(DVec2, f64)> = None;
        for j in 0..g.ny {
            for i in 0..g.nx {
                let k = g.index(i, j);
                if !self.allow.get(k).copied().unwrap_or(false) || !self.material.get(i, j) {
                    continue;
                }
                let room = self.room.get(k).copied().unwrap_or(0.0);
                if best.is_none_or(|(_, b)| room > b) {
                    best = Some((g.centre(i, j), room));
                }
            }
        }
        let (p, room) = best?;
        let helix = (room * g.res - g.res).min(self.s.helix_radius).max(0.0);
        Some((p, Entry::Helix(if helix >= self.s.radius * 0.1 { helix } else { 0.0 })))
    }

    fn room_at(&self, p: DVec2) -> f64 {
        self.grid.cell(p).and_then(|(i, j)| self.room.get(self.grid.index(i, j))).copied().unwrap_or(0.0)
            * self.grid.res
    }

    /// The heading whose step removes closest to `target` cells, from `p` heading `h`, turning
    /// at most `max_turn`. With nothing to cut within that turn the path keeps turning towards
    /// the material side through cleared ground (a trochoidal loop) and reports no engagement.
    fn steer(&self, p: DVec2, h: f64, step: f64, target: f64, max_turn: f64) -> Option<(f64, usize)> {
        // From the hardest turn into the material (right when climbing) to the hardest away.
        let side = if self.s.climb { -1.0 } else { 1.0 };
        let probe = |a: f64| -> Option<usize> {
            let q = p + DVec2::new(a.cos(), a.sin()) * step;
            self.allowed(q).then(|| self.engagement(q))
        };
        const N: usize = 9;
        let angles: Vec<f64> = (0..N).map(|k| h + side * max_turn * (1.0 - 2.0 * k as f64 / (N - 1) as f64)).collect();
        let found: Vec<(f64, Option<usize>)> = angles.iter().map(|&a| (a, probe(a))).collect();
        let mut best: Option<(usize, f64, usize)> = None;
        for (k, (_, e)) in found.iter().enumerate() {
            let Some(e) = e.filter(|e| *e > 0) else { continue };
            let miss = (e as f64 - target).abs();
            if best.is_none_or(|(_, bm, _)| miss < bm - 1.0e-9) {
                best = Some((k, miss, e));
            }
        }
        let Some((k, _, e)) = best else {
            // Material a little further on (a strip along a wall broken by the grid): keep on.
            let dir = DVec2::new(h.cos(), h.sin());
            let ahead = (2..=8).any(|n| {
                let q = p + dir * step * n as f64;
                self.allowed(q) && self.engagement(q) > 0
            });
            if ahead && let Some(e) = probe(h) {
                return Some((h, e));
            }
            // Nothing to cut within the turn: loop towards the material side through air,
            // straight on if that is blocked, else away; at a wall, whatever way is open.
            for a in [h + side * max_turn, h, h - side * max_turn] {
                if let Some(e) = probe(a) {
                    return Some((a, e));
                }
            }
            return (1..36)
                .map(|n| h + side * std::f64::consts::TAU * n as f64 / 36.0)
                .find_map(|a| probe(a).map(|e| (a, e)));
        };
        // Bisect towards the neighbour on the other side of the target.
        let (mut a, mut ea) = (angles[k], e as f64);
        let other = [k.wrapping_sub(1), k + 1].into_iter().filter_map(|n| found.get(n)).find_map(|(an, en)| {
            en.filter(|en| (*en as f64 - target) * (ea - target) < 0.0).map(|en| (*an, en as f64))
        });
        if let Some((mut b, mut eb)) = other {
            for _ in 0..4 {
                let mid = (a + b) / 2.0;
                let Some(em) = probe(mid) else { break };
                let em = em as f64;
                if (em - target) * (ea - target) <= 0.0 {
                    b = mid;
                    eb = em;
                } else {
                    a = mid;
                    ea = em;
                }
            }
            if (eb - target).abs() < (ea - target).abs() {
                a = b;
                ea = eb;
            }
        }
        Some((a, ea.max(0.0) as usize))
    }
}

/// Adaptive passes clearing `material` (one flag per cell of `grid`) with the tool centre in
/// `allowed`.
pub fn clear(grid: Grid, allowed: &[Shape], material: Vec<bool>, s: &Settings) -> Vec<Pass> {
    if material.len() != grid.len() || s.radius.is_nan() || s.radius <= 0.0 || s.load.is_nan() || s.load <= 0.0 {
        return Vec::new();
    }
    // Centres a little inside `allowed`, so a centre anywhere in an allowed cell is inside it.
    let allow = raster(&grid, &region::offset(allowed, -grid.res * 0.75, grid.res * 0.1));
    let blocked: Vec<bool> = allow.iter().map(|a| !a).collect();
    let room = distance_to(&grid, &blocked);
    // Material out of the tool's reach is left alone.
    let from_allowed = distance_to(&grid, &allow);
    let reach = s.radius / grid.res - 0.5;
    let cells: Vec<bool> = material.iter().zip(&from_allowed).map(|(&m, &d)| m && d <= reach).collect();
    let material = Bits::new(&grid, &cells);
    let mut st = State { grid, material, allow, room, s };
    let step = (s.radius * 0.2).max(s.load * 0.75).clamp(grid.res * 1.5, s.radius * 0.5);
    let cell_area = grid.res * grid.res;
    let target = (s.load * step / cell_area).max(1.0);
    // The tightest turn: a quarter of the radius, so loops back through cleared ground are
    // smooth; a loop gets a little more than one full turn to find material again.
    let min_turn_radius = s.radius * 0.25;
    let max_turn = (step / min_turn_radius).min(std::f64::consts::FRAC_PI_2);
    let loop_budget = ((std::f64::consts::TAU / max_turn) * 1.2).ceil() as usize + IDLE_STEPS;
    let mut passes: Vec<Pass> = Vec::new();
    let mut total = 0usize;
    let mut at: Option<DVec2> = None;
    while passes.len() < MAX_PASSES && total < MAX_POINTS {
        let Some(m) = st.nearest_material(at.unwrap_or(grid.origin)) else { break };
        let Some((start, entry)) = st.start_near(m, at, target) else {
            // Unreachable after all: drop it so the search moves on.
            st.material.unset(m.0, m.1);
            continue;
        };
        if entry == Entry::Helix(-1.0) {
            // A wall sliver: cleared without a pass (the wall pass takes it).
            st.clear(start, s.radius);
            continue;
        }
        let (start, entry, m) = match (entry, at) {
            // The first entry into a closed region: helix where there is the most room.
            (Entry::Helix(_), None) => {
                let (q, e) = st.deepest().unwrap_or((start, entry));
                (q, e, m)
            }
            // Before helixing into fresh material, start gently anywhere else if there is a way.
            (Entry::Helix(_), Some(from)) => match st.gentle_start(from, target) {
                Some(q) => (q, Entry::Retract, st.nearest_material(q).unwrap_or(m)),
                None => (start, entry, m),
            },
            other => (start, other.0, m),
        };
        // Stay down: a way at depth through cleared ground to the start, if not too long.
        let mut link = Vec::new();
        let entry = match (entry, at) {
            (Entry::Link | Entry::Retract, Some(from)) => match st.route(from, start, s.stay_down) {
                Some(way) => {
                    link = way;
                    Entry::Link
                }
                None => Entry::Retract,
            },
            (e, _) => e,
        };
        let mp = grid.centre(m.0, m.1);
        match entry {
            Entry::Helix(hr) => st.clear(start, hr + s.radius),
            _ => st.clear(start, s.radius),
        }
        // Head so the material is on the cutting side.
        let to_m = (mp - start).normalize_or(DVec2::X);
        let turn = if s.climb { std::f64::consts::FRAC_PI_2 } else { -std::f64::consts::FRAC_PI_2 };
        let mut h = to_m.y.atan2(to_m.x) + turn;
        let mut p = start;
        let mut pts = vec![start];
        let mut idle = 0usize;
        while total + pts.len() < MAX_POINTS {
            let Some((a, e)) = st.steer(p, h, step, target, max_turn) else { break };
            idle = if (e as f64) < target * 0.05 { idle + 1 } else { 0 };
            if idle > loop_budget {
                // The loop found nothing: end where the cutting did.
                pts.truncate(pts.len().saturating_sub(idle - 1).max(1));
                break;
            }
            p += DVec2::new(a.cos(), a.sin()) * step;
            h = a;
            st.clear(p, s.radius);
            pts.push(p);
        }
        total += pts.len();
        at = pts.last().copied();
        passes.push(Pass { entry, link, pts });
    }
    passes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(a: f64, b: f64) -> Shape {
        region::rectangle(DVec2::splat(a), DVec2::splat(b))
    }

    /// Replays the passes on a fine grid: cells cut, and the largest step engagement seen.
    fn replay(passes: &[Pass], r: f64, material: &[Shape], res: f64) -> (f64, f64) {
        let g = Grid::new(DVec2::splat(-60.0), DVec2::splat(60.0), res, 10_000_000).unwrap_or(Grid {
            origin: DVec2::ZERO,
            res,
            nx: 1,
            ny: 1,
        });
        let cells = raster(&g, material);
        let total = cells.iter().filter(|c| **c).count() as f64;
        let mut worst: f64 = 0.0;
        let s = Settings { radius: r, load: 1.0, climb: true, helix_radius: 0.0, stay_down: 0.0 };
        let mut st = State {
            grid: g,
            material: Bits::new(&g, &cells),
            allow: vec![true; g.len()],
            room: vec![0.0; g.len()],
            s: &s,
        };
        for p in passes {
            if let Entry::Helix(hr) = p.entry
                && let Some(first) = p.pts.first()
            {
                st.clear(*first, hr + r);
            }
            for w in p.pts.windows(2) {
                let removed = st.engagement(w[1]) as f64 * res * res;
                worst = worst.max(removed / w[0].distance(w[1]).max(1e-9));
                st.clear(w[1], r);
            }
        }
        let left = st.material.total() as f64;
        (1.0 - left / total, worst)
    }

    #[test]
    fn a_closed_pocket_is_cleared_with_a_steady_load() {
        let pocket = vec![square(-40.0, 40.0)];
        let r = 5.0;
        let allowed = region::offset(&pocket, -r, 0.01);
        let grid = Grid::new(DVec2::splat(-41.0), DVec2::splat(41.0), r / 10.0, 4_000_000).unwrap_or(Grid {
            origin: DVec2::ZERO,
            res: 1.0,
            nx: 1,
            ny: 1,
        });
        let material = raster(&grid, &pocket);
        let s = Settings { radius: r, load: 1.5, climb: true, helix_radius: 4.0, stay_down: 200.0 };
        let passes = clear(grid, &allowed, material, &s);
        assert!(!passes.is_empty());
        assert!(matches!(passes[0].entry, Entry::Helix(h) if h > 0.0), "{:?}", passes[0].entry);
        for p in &passes {
            assert!(p.pts.iter().all(|q| region::contains(&allowed, *q)), "the centre left the allowed region");
        }
        let (cut, worst) = replay(&passes, r, &[square(-34.0, 34.0)], 0.25);
        assert!(cut > 0.97, "cleared {cut}");
        // Steps stay near the load (corners and entries may briefly exceed it).
        eprintln!("worst width of cut {worst}, passes {}", passes.len());
        assert!(worst < 1.5 * 4.0, "worst width of cut {worst}");
    }

    #[test]
    fn open_material_is_entered_from_outside() {
        // Stock with room around it: the tool starts outside, with no helix.
        let stock = vec![square(-20.0, 20.0)];
        let r = 4.0;
        let allowed = vec![square(-30.0, 30.0)];
        let grid = Grid::new(DVec2::splat(-31.0), DVec2::splat(31.0), r / 10.0, 4_000_000).unwrap_or(Grid {
            origin: DVec2::ZERO,
            res: 1.0,
            nx: 1,
            ny: 1,
        });
        let material = raster(&grid, &stock);
        let passes = clear(
            grid,
            &allowed,
            material,
            &Settings { radius: r, load: 1.2, climb: true, helix_radius: 3.0, stay_down: 200.0 },
        );
        assert!(
            !passes.is_empty() && !matches!(passes[0].entry, Entry::Helix(_)),
            "{:?}",
            passes.first().map(|p| p.entry)
        );
        let (cut, _) = replay(&passes, r, &stock, 0.25);
        assert!(cut > 0.995, "cleared {cut}");
    }

    #[test]
    fn rasters_follow_holes() {
        let g = Grid::new(DVec2::ZERO, DVec2::splat(10.0), 1.0, 1000).unwrap_or(Grid {
            origin: DVec2::ZERO,
            res: 1.0,
            nx: 1,
            ny: 1,
        });
        let mut ring = square(0.0, 10.0);
        ring.extend(square(3.0, 7.0).into_iter().map(|c| c.into_iter().rev().collect::<Vec<_>>()));
        let cells = raster(&g, &[ring]);
        assert!(cells[g.index(1, 1)] && !cells[g.index(5, 5)]);
    }
}

#[cfg(test)]
mod picture {
    use super::*;

    /// Writes an SVG of a pocket's passes to `ADAPTIVE_SVG` (a look at the path, not a check).
    #[test]
    fn draw() {
        let Ok(path) = std::env::var("ADAPTIVE_SVG") else { return };
        let mut pocket = region::rectangle(DVec2::splat(-40.0), DVec2::splat(40.0));
        pocket.extend(
            region::rectangle(DVec2::new(-10.0, -25.0), DVec2::new(15.0, 0.0))
                .into_iter()
                .map(|c| c.into_iter().rev().collect::<Vec<_>>()),
        );
        let pocket = vec![pocket];
        let r = 5.0;
        let allowed = region::offset(&pocket, -r, 0.01);
        let Some(grid) = Grid::new(DVec2::splat(-41.0), DVec2::splat(41.0), r / 10.0, 4_000_000) else { return };
        let material = raster(&grid, &pocket);
        let passes = clear(
            grid,
            &allowed,
            material,
            &Settings { radius: r, load: 1.5, climb: true, helix_radius: 4.0, stay_down: 200.0 },
        );
        let mut svg = String::from(
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='-45 -45 90 90' width='900' height='900'><rect x='-45' y='-45' width='90' height='90' fill='white'/>",
        );
        for (k, p) in passes.iter().enumerate() {
            let colour = match p.entry {
                Entry::Link => "blue",
                Entry::Retract => "orange",
                Entry::Helix(_) => "red",
            };
            let d: Vec<String> = p.pts.iter().map(|q| format!("{:.2},{:.2}", q.x, -q.y)).collect();
            svg += &format!(
                "<polyline points='{}' fill='none' stroke='{}' stroke-width='0.15'/>",
                d.join(" "),
                if k % 2 == 0 { "black" } else { "green" }
            );
            if let Some(f) = p.pts.first() {
                svg += &format!("<circle cx='{:.2}' cy='{:.2}' r='0.4' fill='{colour}'/>", f.x, -f.y);
            }
        }
        svg += "</svg>";
        let _ = std::fs::write(path, svg);
        eprintln!("{} passes, {} points", passes.len(), passes.iter().map(|p| p.pts.len()).sum::<usize>());
    }
}
