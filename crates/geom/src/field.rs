//! Scalar fields on a grid: distance measured over a height surface (fast marching), and
//! iso-lines (marching squares) chained into polylines.
//!
//! Finishing strategies use them on the tool-centre height surface: the distance from the
//! area's boundary measured over that surface gives passes a constant 3D stepover on slopes
//! and flats alike (scallop finishing).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use glam::DVec2;

/// A field's grid: `nx` × `ny` samples `res` apart, the first at `origin`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lattice {
    pub origin: DVec2,
    pub res: f64,
    pub nx: usize,
    pub ny: usize,
}

impl Lattice {
    pub fn at(&self, i: usize, j: usize) -> DVec2 {
        self.origin + DVec2::new(i as f64, j as f64) * self.res
    }

    pub fn len(&self) -> usize {
        self.nx * self.ny
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bilinear interpolation of `f` at `p`, None off the grid or where a corner is NaN.
    pub fn sample(&self, f: &[f64], p: DVec2) -> Option<f64> {
        let q = (p - self.origin) / self.res;
        if !(q.x >= 0.0 && q.y >= 0.0) {
            return None;
        }
        let (i, j) = ((q.x as usize).min(self.nx.saturating_sub(2)), (q.y as usize).min(self.ny.saturating_sub(2)));
        let (tx, ty) = ((q.x - i as f64).clamp(0.0, 1.0), (q.y - j as f64).clamp(0.0, 1.0));
        let v = |a: usize, b: usize| f.get(b * self.nx + a).copied();
        let (a, b, c, d) = (v(i, j)?, v(i + 1, j)?, v(i, j + 1)?, v(i + 1, j + 1)?);
        let out = (a * (1.0 - tx) + b * tx) * (1.0 - ty) + (c * (1.0 - tx) + d * tx) * ty;
        out.is_finite().then_some(out)
    }
}

/// Distance from the `seeds` over the surface `z` (one height per sample), within `inside`:
/// first-order fast marching with the local metric √(1 + |∇z|²). Samples outside or not
/// reached are infinite.
pub fn surface_distance(l: &Lattice, z: &[f64], seeds: &[bool], inside: &[bool]) -> Vec<f64> {
    let n = l.len();
    let mut dist = vec![f64::INFINITY; n];
    if z.len() != n || seeds.len() != n || inside.len() != n {
        return dist;
    }
    let (nx, ny) = (l.nx, l.ny);
    // The cost of crossing a sample: the surface length per unit of plan distance, steep
    // slopes capped so walls (seen from above) cost a lot but not without bound.
    let speed: Vec<f64> = (0..n)
        .map(|k| {
            let (i, j) = (k % nx, k / nx);
            let h = |a: usize, b: usize| z.get(b * nx + a).copied().unwrap_or(f64::NAN);
            let gx = if i > 0 && i + 1 < nx { (h(i + 1, j) - h(i - 1, j)) / (2.0 * l.res) } else { 0.0 };
            let gy = if j > 0 && j + 1 < ny { (h(i, j + 1) - h(i, j - 1)) / (2.0 * l.res) } else { 0.0 };
            let g2 = if gx.is_finite() && gy.is_finite() { gx * gx + gy * gy } else { 0.0 };
            (1.0 + g2.min(400.0)).sqrt() * l.res
        })
        .collect();
    let mut done = vec![false; n];
    let mut heap = BinaryHeap::new();
    for k in 0..n {
        if seeds[k] && inside[k] {
            dist[k] = 0.0;
            heap.push((Reverse(0u64), k));
        }
    }
    while let Some((_, k)) = heap.pop() {
        if done[k] {
            continue;
        }
        done[k] = true;
        let (i, j) = (k % nx, k / nx);
        let mut visit = |a: usize, b: usize| {
            let m = b * nx + a;
            if done[m] || !inside[m] {
                return;
            }
            // Eikonal update from the smaller neighbour in each direction.
            let dx = [a.checked_sub(1).map(|x| b * nx + x), (a + 1 < nx).then(|| b * nx + a + 1)]
                .into_iter()
                .flatten()
                .map(|q| dist[q])
                .fold(f64::INFINITY, f64::min);
            let dy = [b.checked_sub(1).map(|y| y * nx + a), (b + 1 < ny).then(|| (b + 1) * nx + a)]
                .into_iter()
                .flatten()
                .map(|q| dist[q])
                .fold(f64::INFINITY, f64::min);
            let f = speed[m];
            let (lo, hi) = if dx < dy { (dx, dy) } else { (dy, dx) };
            let d = if hi.is_finite() && hi - lo < f {
                (lo + hi + (2.0 * f * f - (hi - lo) * (hi - lo)).sqrt()) / 2.0
            } else {
                lo + f
            };
            if d < dist[m] {
                dist[m] = d;
                heap.push((Reverse(d.to_bits()), m));
            }
        };
        if i > 0 {
            visit(i - 1, j);
        }
        if i + 1 < nx {
            visit(i + 1, j);
        }
        if j > 0 {
            visit(i, j - 1);
        }
        if j + 1 < ny {
            visit(i, j + 1);
        }
    }
    dist
}

/// Where `f` crosses `level`, as polylines (closed ones repeat no point; the flag says
/// closed). Cells with a non-finite corner are skipped, so lines end at the field's edge.
pub fn iso_lines(l: &Lattice, f: &[f64], level: f64) -> Vec<(Vec<DVec2>, bool)> {
    if f.len() != l.len() || l.nx < 2 || l.ny < 2 {
        return Vec::new();
    }
    // Crossing points keyed by edge: horizontal edge (i, j)→(i+1, j) is 2k, vertical 2k+1.
    let key_h = |i: usize, j: usize| 2 * (j * l.nx + i);
    let key_v = |i: usize, j: usize| 2 * (j * l.nx + i) + 1;
    let v = |i: usize, j: usize| f.get(j * l.nx + i).copied().unwrap_or(f64::NAN);
    let point = |a: DVec2, b: DVec2, fa: f64, fb: f64| {
        let t = ((level - fa) / (fb - fa)).clamp(0.0, 1.0);
        a.lerp(b, t)
    };
    let mut pos: HashMap<usize, DVec2> = HashMap::new();
    let mut links: HashMap<usize, Vec<usize>> = HashMap::new();
    for j in 0..l.ny - 1 {
        for i in 0..l.nx - 1 {
            let c = [v(i, j), v(i + 1, j), v(i + 1, j + 1), v(i, j + 1)];
            if c.iter().any(|x| !x.is_finite()) {
                continue;
            }
            let p = [l.at(i, j), l.at(i + 1, j), l.at(i + 1, j + 1), l.at(i, j + 1)];
            // Edges: bottom (0-1), right (1-2), top (3-2), left (0-3).
            let edges = [(key_h(i, j), 0, 1), (key_v(i + 1, j), 1, 2), (key_h(i, j + 1), 3, 2), (key_v(i, j), 0, 3)];
            let mut cut: Vec<usize> = Vec::with_capacity(4);
            for (key, a, b) in edges {
                if (c[a] >= level) != (c[b] >= level) {
                    pos.entry(key).or_insert_with(|| point(p[a], p[b], c[a], c[b]));
                    cut.push(key);
                }
            }
            let pairs: Vec<(usize, usize)> = match cut.as_slice() {
                [a, b] => vec![(*a, *b)],
                [a, b, c2, d] => {
                    // A saddle: decide by the centre value.
                    let centre = (c[0] + c[1] + c[2] + c[3]) / 4.0;
                    if (centre >= level) == (c[0] >= level) {
                        vec![(*a, *d), (*b, *c2)]
                    } else {
                        vec![(*a, *b), (*c2, *d)]
                    }
                }
                _ => Vec::new(),
            };
            for (a, b) in pairs {
                links.entry(a).or_default().push(b);
                links.entry(b).or_default().push(a);
            }
        }
    }
    // Chain: open lines first (from their ends), then the closed loops left.
    let mut used: HashMap<usize, bool> = HashMap::new();
    let mut out = Vec::new();
    let walk =
        |start: usize, links: &HashMap<usize, Vec<usize>>, used: &mut HashMap<usize, bool>| -> (Vec<usize>, bool) {
            let mut line = vec![start];
            used.insert(start, true);
            let mut at = start;
            let mut prev: Option<usize> = None;
            loop {
                let next = links.get(&at).and_then(|n| {
                    n.iter().copied().find(|q| Some(*q) != prev && !used.get(q).copied().unwrap_or(false))
                });
                match next {
                    Some(q) => {
                        used.insert(q, true);
                        line.push(q);
                        prev = Some(at);
                        at = q;
                        if line.len() > 10_000_000 {
                            break;
                        }
                    }
                    None => break,
                }
            }
            let closed = line.len() > 2 && links.get(&at).is_some_and(|n| n.contains(&start));
            (line, closed)
        };
    let mut keys: Vec<usize> = links.keys().copied().collect();
    keys.sort_unstable();
    for &k in &keys {
        if !used.get(&k).copied().unwrap_or(false) && links.get(&k).is_some_and(|n| n.len() == 1) {
            let (line, closed) = walk(k, &links, &mut used);
            out.push((line, closed));
        }
    }
    for &k in &keys {
        if !used.get(&k).copied().unwrap_or(false) {
            let (line, closed) = walk(k, &links, &mut used);
            out.push((line, closed));
        }
    }
    out.into_iter()
        .filter(|(line, _)| line.len() >= 2)
        .map(|(line, closed)| (line.iter().filter_map(|k| pos.get(k).copied()).collect(), closed))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_over_a_flat_square_is_plan_distance() {
        let l = Lattice { origin: DVec2::ZERO, res: 1.0, nx: 41, ny: 41 };
        let z = vec![0.0; l.len()];
        let seeds: Vec<bool> = (0..l.len()).map(|k| k % l.nx == 0).collect();
        let inside = vec![true; l.len()];
        let d = surface_distance(&l, &z, &seeds, &inside);
        assert!((d[20 * 41 + 20] - 20.0).abs() < 1e-9);
        // A 45° ramp along x: the distance is √2 times longer.
        let z: Vec<f64> = (0..l.len()).map(|k| (k % l.nx) as f64).collect();
        let d = surface_distance(&l, &z, &seeds, &inside);
        assert!((d[20 * 41 + 20] - 20.0 * 2f64.sqrt()).abs() < 0.5, "{}", d[20 * 41 + 20]);
    }

    #[test]
    fn iso_lines_of_a_cone_are_circles() {
        let l = Lattice { origin: DVec2::splat(-10.0), res: 0.5, nx: 41, ny: 41 };
        let f: Vec<f64> = (0..l.len()).map(|k| l.at(k % l.nx, k / l.nx).length()).collect();
        let lines = iso_lines(&l, &f, 6.0);
        assert_eq!(lines.len(), 1);
        let (pts, closed) = &lines[0];
        assert!(*closed && pts.len() > 30);
        assert!(pts.iter().all(|p| (p.length() - 6.0).abs() < 0.1));
        // A level crossing the edge gives open lines.
        let lines = iso_lines(&l, &f, 12.0);
        assert!(lines.iter().all(|(_, closed)| !closed) && !lines.is_empty());
    }
}
