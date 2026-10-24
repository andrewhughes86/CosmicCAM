//! Edge recognition on triangle meshes: the mesh's feature edges (creases and open
//! boundaries) chained into the edges a user would point at, either straight lines or
//! circles and arcs (hole and boss rims).

use std::collections::HashMap;

use glam::DVec3;

use crate::{Mesh, Vec3};

/// Faces meeting at more than this many degrees make an edge (as the viewport outlines).
const CREASE_DEG: f32 = 30.0;
/// Consecutive segments within this many degrees continue a straight line.
const STRAIGHT_DEG: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EdgeKind {
    Line {
        start: Vec3,
        end: Vec3,
    },
    /// A circle or arc: centre, unit normal of its plane, radius.
    Circle {
        center: Vec3,
        normal: Vec3,
        radius: f32,
    },
}

impl EdgeKind {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Line { .. } => "line",
            Self::Circle { .. } => "circle",
        }
    }

    /// The direction the edge gives an axis: along a line, or a circle's normal.
    pub fn direction(&self) -> Vec3 {
        match *self {
            Self::Line { start, end } => (end - start).normalize_or_zero(),
            Self::Circle { normal, .. } => normal,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub kind: EdgeKind,
    /// The polyline the edge follows, in order.
    pub points: Vec<Vec3>,
    pub length: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdgeMap {
    pub edges: Vec<Edge>,
}

impl EdgeMap {
    /// The nearest edge passing within `radius` of a ray and not behind the first surface
    /// hit at `limit` along it: (edge index, distance along the ray).
    pub fn nearest_to_ray(&self, origin: Vec3, dir: Vec3, radius: f32, limit: f32) -> Option<(usize, f32)> {
        let dir = dir.normalize_or_zero();
        if dir == Vec3::ZERO {
            return None;
        }
        let mut best: Option<(usize, f32, f32)> = None;
        for (i, e) in self.edges.iter().enumerate() {
            for pair in e.points.windows(2) {
                let [a, b] = [pair.first().copied().unwrap_or_default(), pair.get(1).copied().unwrap_or_default()];
                let (gap, along) = segment_ray_distance(a, b, origin, dir);
                // Of the edges close enough to the ray, the one nearest the eye is the one seen.
                if gap <= radius && along > 0.0 && along <= limit && best.is_none_or(|(_, _, d)| along < d) {
                    best = Some((i, gap, along));
                }
            }
        }
        best.map(|(i, _, along)| (i, along))
    }
}

/// Closest approach between segment `a`–`b` and a ray: (gap, distance along the ray).
fn segment_ray_distance(a: Vec3, b: Vec3, origin: Vec3, dir: Vec3) -> (f32, f32) {
    let u = b - a;
    let w = a - origin;
    let (uu, ud, uw, dw) = (u.dot(u), u.dot(dir), u.dot(w), dir.dot(w));
    let denom = uu - ud * ud;
    // A point-like segment, or one parallel to the ray, is measured from its start.
    let s = if uu <= 1.0e-12 || denom.abs() < 1.0e-9 { 0.0 } else { ((ud * dw - uw) / denom).clamp(0.0, 1.0) };
    let p = a + u * s;
    let t = (p - origin).dot(dir);
    ((origin + dir * t - p).length(), t)
}

pub fn extract(mesh: &Mesh) -> EdgeMap {
    let pos = mesh.positions();
    let p = |i: u32| pos.get(i as usize).copied().unwrap_or(Vec3::ZERO);
    // Weld so chains continue across unwelded copies of a vertex.
    let mut canon: HashMap<[u32; 3], u32> = HashMap::new();
    let mut weld = vec![0u32; pos.len()];
    for (i, v) in pos.iter().enumerate() {
        let next = canon.len() as u32;
        let id = *canon.entry((*v + Vec3::ZERO).to_array().map(f32::to_bits)).or_insert(next);
        if let Some(slot) = weld.get_mut(i) {
            *slot = id;
        }
    }
    let w = |i: u32| weld.get(i as usize).copied().unwrap_or(i);
    let mut segments: Vec<[u32; 2]> = mesh
        .feature_edges(CREASE_DEG)
        .into_iter()
        .map(|[a, b]| [w(a), w(b)])
        .filter(|[a, b]| a != b)
        .map(|[a, b]| [a.min(b), a.max(b)])
        .collect();
    segments.sort_unstable();
    segments.dedup();
    // Any original vertex per welded id, for its position.
    let mut at = vec![Vec3::ZERO; canon.len()];
    for (i, v) in pos.iter().enumerate() {
        if let Some(slot) = weld.get(i).and_then(|&c| at.get_mut(c as usize)) {
            *slot = *v;
        }
    }
    let q = |c: u32| at.get(c as usize).copied().unwrap_or_else(|| p(c));

    let mut incident: HashMap<u32, Vec<usize>> = HashMap::new();
    for (si, [a, b]) in segments.iter().enumerate() {
        incident.entry(*a).or_default().push(si);
        incident.entry(*b).or_default().push(si);
    }
    let degree = |v: u32| incident.get(&v).map_or(0, Vec::len);
    let mut used = vec![false; segments.len()];
    let mut chains: Vec<Vec<u32>> = Vec::new();
    // Walk chains from ends and junctions first, then the closed loops left over.
    let mut starts: Vec<usize> = (0..segments.len()).collect();
    starts.sort_by_key(|&si| segments.get(si).map_or(0, |[a, b]| usize::from(degree(*a) == 2 && degree(*b) == 2)));
    for si in starts {
        if used.get(si).copied().unwrap_or(true) {
            continue;
        }
        let Some(&[a, b]) = segments.get(si) else { continue };
        if let Some(u) = used.get_mut(si) {
            *u = true;
        }
        let (first, mut current) = if degree(a) != 2 { (a, b) } else { (b, a) };
        let mut chain = vec![first, current];
        while degree(current) == 2 && current != first {
            let next = incident
                .get(&current)
                .and_then(|list| list.iter().copied().find(|&s| !used.get(s).copied().unwrap_or(true)));
            let Some(next) = next else { break };
            if let Some(u) = used.get_mut(next) {
                *u = true;
            }
            let Some(&[x, y]) = segments.get(next) else { break };
            current = if x == current { y } else { x };
            chain.push(current);
        }
        chains.push(chain);
    }

    let mut edges = Vec::new();
    for chain in chains {
        let pts: Vec<Vec3> = chain.iter().map(|&c| q(c)).collect();
        if let Some(circle) = fit_circle_edge(&pts) {
            edges.push(make(circle, pts));
            continue;
        }
        // Split at corners into straight runs.
        let cos = STRAIGHT_DEG.to_radians().cos() as f32;
        let mut run = vec![pts.first().copied().unwrap_or_default()];
        for pair in pts.windows(3) {
            if let [a, b, c] = *pair {
                run.push(b);
                if (b - a).normalize_or_zero().dot((c - b).normalize_or_zero()) < cos {
                    edges.push(line(std::mem::replace(&mut run, vec![b])));
                }
            }
        }
        if let Some(last) = pts.last() {
            run.push(*last);
        }
        if run.len() >= 2 {
            edges.push(line(run));
        }
    }
    EdgeMap { edges }
}

fn polyline_length(pts: &[Vec3]) -> f32 {
    pts.windows(2).map(|w| w.first().zip(w.get(1)).map_or(0.0, |(a, b)| a.distance(*b))).sum()
}

fn line(pts: Vec<Vec3>) -> Edge {
    let start = pts.first().copied().unwrap_or_default();
    let end = pts.last().copied().unwrap_or_default();
    Edge { kind: EdgeKind::Line { start, end }, length: polyline_length(&pts), points: pts }
}

fn make(kind: EdgeKind, pts: Vec<Vec3>) -> Edge {
    Edge { kind, length: polyline_length(&pts), points: pts }
}

/// A circle through a chain of at least four segments that turns steadily in one plane.
fn fit_circle_edge(pts: &[Vec3]) -> Option<EdgeKind> {
    if pts.len() < 5 {
        return None;
    }
    let d: Vec<DVec3> = pts.iter().map(|v| v.as_dvec3()).collect();
    let centroid = d.iter().sum::<DVec3>() / d.len() as f64;
    // Newell's normal of the polygon, robust for arcs too.
    let mut normal = DVec3::ZERO;
    for pair in d.windows(2) {
        if let [a, b] = pair {
            normal += (*a - centroid).cross(*b - centroid);
        }
    }
    let normal = normal.normalize_or_zero();
    if normal == DVec3::ZERO {
        return None;
    }
    let scale = d.iter().map(|v| v.distance(centroid)).fold(0.0, f64::max).max(1.0e-9);
    if d.iter().any(|v| (*v - centroid).dot(normal).abs() > scale * 1.0e-3) {
        return None;
    }
    let (u, v) = normal.any_orthonormal_pair();
    let flat: Vec<(f64, f64)> = d.iter().map(|q| ((*q - centroid).dot(u), (*q - centroid).dot(v))).collect();
    let (cx, cy, r) = crate::faces::fit_circle_2d(&flat)?;
    if flat.iter().any(|(x, y)| ((x - cx).hypot(y - cy) - r).abs() > r * 0.01 + scale * 1.0e-5) {
        return None;
    }
    let center = centroid + u * cx + v * cy;
    Some(EdgeKind::Circle { center: center.as_vec3(), normal: normal.as_vec3(), radius: r as f32 })
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    fn cube() -> Result<Mesh, crate::MeshError> {
        let c = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        let p = [
            c(0., 0., 0.),
            c(1., 0., 0.),
            c(1., 1., 0.),
            c(0., 1., 0.),
            c(0., 0., 1.),
            c(1., 0., 1.),
            c(1., 1., 1.),
            c(0., 1., 1.),
        ];
        let quads = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [2, 3, 7, 6], [1, 2, 6, 5], [3, 0, 4, 7]];
        Mesh::from_soup(
            quads.iter().flat_map(|q: &[usize; 4]| [[p[q[0]], p[q[1]], p[q[2]]], [p[q[0]], p[q[2]], p[q[3]]]]),
        )
    }

    #[test]
    fn a_cube_has_twelve_straight_edges() -> Result<(), crate::MeshError> {
        let map = extract(&cube()?);
        assert_eq!(map.edges.len(), 12);
        assert!(map.edges.iter().all(|e| matches!(e.kind, EdgeKind::Line { .. }) && (e.length - 1.0).abs() < 1e-5));
        Ok(())
    }

    #[test]
    fn a_disc_rim_is_a_circle() -> Result<(), crate::MeshError> {
        let n = 32;
        let ring = |i: usize| {
            let a = TAU * (i % n) as f32 / n as f32;
            Vec3::new(5.0 + 3.0 * a.cos(), 3.0 * a.sin(), 2.0)
        };
        let centre = Vec3::new(5.0, 0.0, 2.0);
        let map = extract(&Mesh::from_soup((0..n).map(|i| [centre, ring(i), ring(i + 1)]))?);
        assert_eq!(map.edges.len(), 1, "{:?}", map.edges.iter().map(|e| e.kind).collect::<Vec<_>>());
        let EdgeKind::Circle { center, normal, radius } = map.edges[0].kind else {
            return Ok(());
        };
        assert!((center - centre).length() < 1e-4 && (radius - 3.0).abs() < 1e-3 && normal.z.abs() > 0.9999);
        Ok(())
    }

    #[test]
    fn rays_pick_the_nearest_edge() -> Result<(), crate::MeshError> {
        let map = extract(&cube()?);
        // Looking down at the top front edge (y = 0, z = 1), slightly off it.
        let hit = map.nearest_to_ray(Vec3::new(0.5, 0.02, 5.0), Vec3::NEG_Z, 0.05, 10.0);
        let Some((i, _)) = hit else {
            return Err(crate::MeshError::TooManyVertices);
        };
        let d = map.edges[i].kind.direction();
        assert!(d.x.abs() > 0.999, "{d:?}");
        assert!(map.nearest_to_ray(Vec3::new(0.5, 0.5, 5.0), Vec3::NEG_Z, 0.05, 10.0).is_none());
        // Behind the first surface hit does not count.
        assert!(map.nearest_to_ray(Vec3::new(0.5, 0.02, 5.0), Vec3::NEG_Z, 0.05, 1.0).is_none());
        Ok(())
    }
}
