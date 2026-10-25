//! Face recognition on triangle meshes. STL and OBJ files carry only triangles; picking a
//! "face" to set up a WCS needs the surfaces they came from. Triangles are grouped into:
//!
//! 1. **Planes**: connected triangles with the same normal on one plane.
//! 2. **Cylinders**: neighbouring planar strips whose normals are all square to one axis and
//!    whose vertices lie on one circle around it (tessellated holes, bosses and fillets).
//! 3. **Freeform** patches: small leftover facets joined smoothly (spheres, blends).
//!
//! Every triangle belongs to exactly one face. Work is linear in the triangle count apart
//! from cylinder fitting, which samples a bounded number of points.

use std::collections::HashMap;

use glam::DVec3;

use crate::{Mesh, Vec3};

/// Triangles that differ by less than this many degrees can share a plane.
const PLANE_ANGLE_DEG: f64 = 0.5;
/// Neighbouring strips meeting at more than this many degrees are not one smooth surface.
const SMOOTH_ANGLE_DEG: f64 = 35.0;
/// A cylinder must sweep at least this many degrees around its axis.
const MIN_CYLINDER_SPAN_DEG: f64 = 20.0;
/// Points used when fitting a circle, at most.
const MAX_FIT_POINTS: usize = 512;
/// Above this many triangles only planes are found.
const MAX_TRIANGLES_FOR_CURVES: usize = 4_000_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FaceKind {
    /// `normal` points out of the material (by the mesh winding); `point` lies on the plane.
    Plane {
        normal: Vec3,
        point: Vec3,
    },
    /// The axis runs from `base` for `length` mm along unit `axis`. `hole` is true when the
    /// surface faces the axis (a bore) rather than away from it (a boss).
    Cylinder {
        base: Vec3,
        axis: Vec3,
        radius: f32,
        length: f32,
        hole: bool,
    },
    Freeform,
}

impl FaceKind {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Plane { .. } => "plane",
            Self::Cylinder { .. } => "cylinder",
            Self::Freeform => "freeform",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub kind: FaceKind,
    /// Surface area in mm².
    pub area: f32,
    /// Area-weighted centre of the face's triangles.
    pub centroid: Vec3,
    pub triangles: u32,
}

/// The faces of one mesh and which face each triangle belongs to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceMap {
    pub faces: Vec<Face>,
    /// Face index per triangle, in mesh order.
    pub triangle_face: Vec<u32>,
}

impl FaceMap {
    pub fn face_of(&self, triangle: usize) -> Option<usize> {
        self.triangle_face.get(triangle).map(|&f| f as usize)
    }

    /// Indices of the triangles on face `face`.
    pub fn triangles_of(&self, face: usize) -> impl Iterator<Item = usize> + '_ {
        self.triangle_face.iter().enumerate().filter(move |(_, f)| **f as usize == face).map(|(t, _)| t)
    }
}

/// The closed boundary loops of a face, as world points, each without the first point
/// repeated. The first loop is the longest (the outside of a face with holes).
pub fn boundary_loops(mesh: &Mesh, map: &FaceMap, face: usize) -> Vec<Vec<Vec3>> {
    let key = |p: Vec3| (p + Vec3::ZERO).to_array().map(f32::to_bits);
    // Directed edges of the face's triangles; an edge with no reverse twin is on the boundary.
    let mut directed: HashMap<([u32; 3], [u32; 3]), (Vec3, Vec3)> = HashMap::new();
    for t in map.triangles_of(face) {
        let Some(&tri) = mesh.triangles().get(t) else { continue };
        let [a, b, c] = mesh.corners(tri);
        for (p, q) in [(a, b), (b, c), (c, a)] {
            if directed.remove(&(key(q), key(p))).is_none() {
                directed.insert((key(p), key(q)), (p, q));
            }
        }
    }
    let mut next: HashMap<[u32; 3], (Vec3, Vec3)> = directed.into_iter().map(|((k, _), v)| (k, v)).collect();
    let mut loops: Vec<Vec<Vec3>> = Vec::new();
    while let Some(&start) = next.keys().next() {
        let mut lp = Vec::new();
        let mut at = start;
        let mut guard = 0usize;
        while let Some((p, q)) = next.remove(&at) {
            lp.push(p);
            at = key(q);
            guard += 1;
            if at == start || guard > 1_000_000 {
                break;
            }
        }
        if lp.len() >= 3 {
            loops.push(lp);
        }
    }
    let length =
        |lp: &Vec<Vec3>| -> f32 { lp.iter().zip(lp.iter().cycle().skip(1)).map(|(a, b)| a.distance(*b)).sum() };
    loops.sort_by(|a, b| length(b).total_cmp(&length(a)));
    loops
}

/// A group of coplanar triangles.
struct Region {
    triangles: Vec<u32>,
    normal: DVec3,
    area: f64,
    centroid: DVec3,
    neighbours: Vec<u32>,
}

pub fn segment(mesh: &Mesh) -> FaceMap {
    let tris = mesh.triangles();
    let n = tris.len();
    if n == 0 {
        return FaceMap::default();
    }
    let p = |i: u32| mesh.positions().get(i as usize).copied().unwrap_or(Vec3::ZERO).as_dvec3();
    let scale = mesh.bounds().size().as_dvec3().length().max(1.0e-3);
    let tol = scale * 1.0e-5 + 1.0e-6;

    // Weld by coordinates so neighbours are found even in unwelded meshes.
    let mut canon: HashMap<[u32; 3], u32> = HashMap::new();
    let mut weld = vec![0u32; mesh.positions().len()];
    for (i, v) in mesh.positions().iter().enumerate() {
        let key = (*v + Vec3::ZERO).to_array().map(f32::to_bits);
        let next = canon.len() as u32;
        if let Some(slot) = weld.get_mut(i) {
            *slot = *canon.entry(key).or_insert(next);
        }
    }
    let w = |i: u32| weld.get(i as usize).copied().unwrap_or(i);
    let mut edges: HashMap<(u32, u32), [u32; 3]> = HashMap::with_capacity(n * 2);
    for (ti, t) in tris.iter().enumerate() {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let (a, b) = (w(a), w(b));
            let e = edges.entry((a.min(b), a.max(b))).or_insert([u32::MAX, u32::MAX, 0]);
            if e[0] == u32::MAX {
                e[0] = ti as u32;
            } else if e[1] == u32::MAX {
                e[1] = ti as u32;
            }
            e[2] += 1;
        }
    }
    // Up to three manifold neighbours per triangle.
    let mut adjacent = vec![[u32::MAX; 3]; n];
    for &[a, b, count] in edges.values() {
        if count != 2 {
            continue;
        }
        for (from, to) in [(a, b), (b, a)] {
            if let Some(slots) = adjacent.get_mut(from as usize)
                && let Some(slot) = slots.iter_mut().find(|s| **s == u32::MAX)
            {
                *slot = to;
            }
        }
    }
    let normal = |t: usize| tris.get(t).map_or(DVec3::ZERO, |t| mesh.face_normal(*t).as_dvec3());
    let area = |t: usize| {
        tris.get(t).map_or(0.0, |t| {
            let [a, b, c] = t.map(p);
            (b - a).cross(c - a).length() * 0.5
        })
    };
    let centre = |t: usize| tris.get(t).map_or(DVec3::ZERO, |t| t.map(p).iter().sum::<DVec3>() / 3.0);

    // 1. Planar regions.
    let cos_plane = PLANE_ANGLE_DEG.to_radians().cos();
    let mut region_of = vec![u32::MAX; n];
    let mut regions: Vec<Region> = Vec::new();
    let mut stack = Vec::new();
    for seed in 0..n {
        if region_of.get(seed).is_none_or(|r| *r != u32::MAX) {
            continue;
        }
        let rn = normal(seed);
        let rid = regions.len() as u32;
        let d = rn.dot(centre(seed));
        let mut region = Region {
            triangles: Vec::new(),
            normal: DVec3::ZERO,
            area: 0.0,
            centroid: DVec3::ZERO,
            neighbours: Vec::new(),
        };
        stack.clear();
        stack.push(seed);
        if let Some(r) = region_of.get_mut(seed) {
            *r = rid;
        }
        while let Some(t) = stack.pop() {
            let a = area(t);
            region.triangles.push(t as u32);
            region.area += a;
            region.normal += normal(t) * a;
            region.centroid += centre(t) * a;
            for &nb in adjacent.get(t).into_iter().flatten() {
                let nb = nb as usize;
                if region_of.get(nb).is_none_or(|r| *r != u32::MAX) {
                    continue;
                }
                let nn = normal(nb);
                let coplanar = rn != DVec3::ZERO
                    && nn.dot(rn) >= cos_plane
                    && tris.get(nb).is_some_and(|t| t.iter().all(|&v| (rn.dot(p(v)) - d).abs() <= tol * 10.0));
                // Degenerate slivers join whichever region reaches them first.
                if coplanar || nn == DVec3::ZERO {
                    if let Some(r) = region_of.get_mut(nb) {
                        *r = rid;
                    }
                    stack.push(nb);
                }
            }
        }
        region.normal = if region.normal.length() > 0.0 { region.normal.normalize() } else { rn };
        region.centroid = if region.area > 0.0 { region.centroid / region.area } else { centre(seed) };
        regions.push(region);
    }
    for (ri, r) in regions.iter_mut().enumerate() {
        let mut nbs: Vec<u32> = r
            .triangles
            .iter()
            .flat_map(|&t| adjacent.get(t as usize).into_iter().flatten().copied())
            .filter_map(|nb| region_of.get(nb as usize).copied())
            .filter(|&nr| nr as usize != ri && nr != u32::MAX)
            .collect();
        nbs.sort_unstable();
        nbs.dedup();
        r.neighbours = nbs;
    }

    // Region -> face. Faces are numbered as they are made.
    let mut face_of_region = vec![u32::MAX; regions.len()];
    let mut faces: Vec<Face> = Vec::new();
    let total_area: f64 = regions.iter().map(|r| r.area).sum();
    let cos_smooth = SMOOTH_ANGLE_DEG.to_radians().cos();
    let smooth = |a: &Region, b: &Region| a.normal.dot(b.normal) >= cos_smooth;
    let region_points = |r: &Region| -> Vec<DVec3> {
        let mut pts: Vec<DVec3> =
            r.triangles.iter().filter_map(|&t| tris.get(t as usize)).flat_map(|t| t.map(p)).collect();
        pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)).then(a.z.total_cmp(&b.z)));
        pts.dedup();
        pts
    };

    if n <= MAX_TRIANGLES_FOR_CURVES {
        // 2. Cylinders grown from neighbouring strips.
        for seed in 0..regions.len() {
            if face_of_region.get(seed).is_none_or(|f| *f != u32::MAX) {
                continue;
            }
            let Some(seed_region) = regions.get(seed) else { continue };
            let mut members = vec![seed as u32];
            let mut points = region_points(seed_region);
            let mut cyl: Option<Cylinder> = None;
            let mut queue: Vec<u32> = seed_region.neighbours.clone();
            let mut tried: Vec<u32> = Vec::new();
            while let Some(nb) = queue.pop() {
                if members.contains(&nb) || tried.contains(&nb) {
                    continue;
                }
                let Some(r) = regions.get(nb as usize) else { continue };
                if face_of_region.get(nb as usize).is_none_or(|f| *f != u32::MAX)
                    || !members.iter().any(|m| {
                        regions.get(*m as usize).is_some_and(|mr| mr.neighbours.contains(&nb) && smooth(mr, r))
                    })
                {
                    continue;
                }
                let new_points = region_points(r);
                let accepted = match &cyl {
                    Some(c) => c.accepts(r.normal, &new_points, tol),
                    None => {
                        let normals = [regions.get(seed).map_or(DVec3::ZERO, |s| s.normal), r.normal];
                        let all: Vec<DVec3> = points.iter().chain(&new_points).copied().collect();
                        match Cylinder::fit(&normals, &all, tol) {
                            Some(c) => {
                                cyl = Some(c);
                                true
                            }
                            None => false,
                        }
                    }
                };
                if !accepted {
                    tried.push(nb);
                    continue;
                }
                members.push(nb);
                points.extend(new_points);
                queue.extend(r.neighbours.iter().copied());
                if members.len().is_power_of_two() && members.len() >= 4 {
                    let normals: Vec<DVec3> =
                        members.iter().filter_map(|m| regions.get(*m as usize)).map(|r| r.normal).collect();
                    if let Some(c) = Cylinder::fit(&normals, &points, tol) {
                        cyl = Some(c);
                    }
                }
            }
            let Some(c) = cyl else { continue };
            let normals: Vec<DVec3> =
                members.iter().filter_map(|m| regions.get(*m as usize)).map(|r| r.normal).collect();
            if members.len() < 2 || c.span_degrees(&normals) < MIN_CYLINDER_SPAN_DEG {
                continue;
            }
            let (lo, hi) = points.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| {
                let s = (*q - c.point).dot(c.axis);
                (lo.min(s), hi.max(s))
            });
            // Normals pointing back at the axis mean a bore.
            let facing: f64 = members
                .iter()
                .filter_map(|m| regions.get(*m as usize))
                .map(|r| {
                    let rel = r.centroid - c.point;
                    let radial = rel - c.axis * rel.dot(c.axis);
                    r.normal.dot(radial) * r.area
                })
                .sum();
            let fid = faces.len() as u32;
            let (area, centroid, count) = stats(&regions, &members);
            faces.push(Face {
                kind: FaceKind::Cylinder {
                    base: (c.point + c.axis * lo).as_vec3(),
                    axis: c.axis.as_vec3(),
                    radius: c.radius as f32,
                    length: (hi - lo) as f32,
                    hole: facing < 0.0,
                },
                area,
                centroid,
                triangles: count,
            });
            for m in members {
                if let Some(f) = face_of_region.get_mut(m as usize) {
                    *f = fid;
                }
            }
        }

        // 3. Small leftover facets joined smoothly become freeform patches.
        let small = |r: &Region| r.area < total_area * 0.01;
        for seed in 0..regions.len() {
            let Some(sr) = regions.get(seed) else { continue };
            if face_of_region.get(seed).is_none_or(|f| *f != u32::MAX) || !small(sr) {
                continue;
            }
            let mut members = vec![seed as u32];
            let mut i = 0;
            if let Some(f) = face_of_region.get_mut(seed) {
                *f = u32::MAX - 1; // claimed while the patch grows
            }
            while let Some(&m) = members.get(i) {
                i += 1;
                let Some(mr) = regions.get(m as usize) else { continue };
                for &nb in &mr.neighbours {
                    let Some(r) = regions.get(nb as usize) else { continue };
                    if face_of_region.get(nb as usize).is_some_and(|f| *f == u32::MAX) && small(r) && smooth(mr, r) {
                        if let Some(f) = face_of_region.get_mut(nb as usize) {
                            *f = u32::MAX - 1;
                        }
                        members.push(nb);
                    }
                }
            }
            if members.len() < 2 {
                if let Some(f) = face_of_region.get_mut(seed) {
                    *f = u32::MAX;
                }
                continue;
            }
            let fid = faces.len() as u32;
            let (area, centroid, count) = stats(&regions, &members);
            faces.push(Face { kind: FaceKind::Freeform, area, centroid, triangles: count });
            for m in members {
                if let Some(f) = face_of_region.get_mut(m as usize) {
                    *f = fid;
                }
            }
        }
    }

    // Everything else is a plane.
    for (ri, r) in regions.iter().enumerate() {
        if face_of_region.get(ri).is_some_and(|f| *f == u32::MAX) {
            let fid = faces.len() as u32;
            let (area, centroid, count) = stats(&regions, &[ri as u32]);
            faces.push(Face {
                kind: FaceKind::Plane { normal: r.normal.as_vec3(), point: r.centroid.as_vec3() },
                area,
                centroid,
                triangles: count,
            });
            if let Some(f) = face_of_region.get_mut(ri) {
                *f = fid;
            }
        }
    }

    let triangle_face = region_of.iter().map(|&r| face_of_region.get(r as usize).copied().unwrap_or(0)).collect();
    FaceMap { faces, triangle_face }
}

fn stats(regions: &[Region], members: &[u32]) -> (f32, Vec3, u32) {
    let (mut area, mut centroid, mut count) = (0.0, DVec3::ZERO, 0usize);
    for r in members.iter().filter_map(|m| regions.get(*m as usize)) {
        area += r.area;
        centroid += r.centroid * r.area;
        count += r.triangles.len();
    }
    let centroid = if area > 0.0 { centroid / area } else { DVec3::ZERO };
    (area as f32, centroid.as_vec3(), count.min(u32::MAX as usize) as u32)
}

/// An infinite cylinder: a point on the axis, the unit axis and the radius.
#[derive(Clone, Copy, Debug)]
struct Cylinder {
    point: DVec3,
    axis: DVec3,
    radius: f64,
}

impl Cylinder {
    /// Fits a cylinder to facet normals (all square to the axis) and vertices (all on the
    /// circle). None when they do not describe one.
    fn fit(normals: &[DVec3], points: &[DVec3], tol: f64) -> Option<Self> {
        let mut m = [[0.0f64; 3]; 3];
        for nrm in normals {
            let v = nrm.to_array();
            for (i, row) in m.iter_mut().enumerate() {
                for (j, cell) in row.iter_mut().enumerate() {
                    *cell += v.get(i).copied().unwrap_or(0.0) * v.get(j).copied().unwrap_or(0.0);
                }
            }
        }
        let (values, vectors) = symmetric_eigen(m);
        let (min_i, _) = values.iter().enumerate().min_by(|a, b| a.1.total_cmp(b.1))?;
        let max = values.iter().copied().fold(0.0, f64::max);
        if max <= 0.0 || values.get(min_i).copied().unwrap_or(1.0) > max * 1.0e-3 {
            return None;
        }
        let axis = vectors.get(min_i)?.normalize_or_zero();
        if axis == DVec3::ZERO || normals.iter().any(|n| n.dot(axis).abs() > 0.05) {
            return None;
        }
        let (u, v) = axis.any_orthonormal_pair();
        let step = points.len().div_ceil(MAX_FIT_POINTS).max(1);
        let sample: Vec<(f64, f64)> = points.iter().step_by(step).map(|q| (q.dot(u), q.dot(v))).collect();
        let (cx, cy, r) = fit_circle(&sample)?;
        let cyl = Self { point: u * cx + v * cy, axis, radius: r };
        let limit = r * 0.01 + tol * 10.0;
        (r.is_finite() && r > tol && sample.iter().all(|(x, y)| ((x - cx).hypot(y - cy) - r).abs() <= limit))
            .then_some(cyl)
    }

    fn distance_from_surface(&self, q: DVec3) -> f64 {
        let rel = q - self.point;
        (rel - self.axis * rel.dot(self.axis)).length() - self.radius
    }

    fn accepts(&self, normal: DVec3, points: &[DVec3], tol: f64) -> bool {
        let limit = self.radius * 0.01 + tol * 10.0;
        normal.dot(self.axis).abs() <= 0.05 && points.iter().all(|q| self.distance_from_surface(*q).abs() <= limit)
    }

    /// Degrees of arc the normals sweep around the axis.
    fn span_degrees(&self, normals: &[DVec3]) -> f64 {
        let (u, v) = self.axis.any_orthonormal_pair();
        let mut angles: Vec<f64> = normals.iter().map(|n| n.dot(v).atan2(n.dot(u)).to_degrees()).collect();
        angles.sort_by(f64::total_cmp);
        let (Some(first), Some(last)) = (angles.first().copied(), angles.last().copied()) else {
            return 0.0;
        };
        let mut gap = first + 360.0 - last;
        for pair in angles.windows(2) {
            if let [a, b] = pair {
                gap = gap.max(b - a);
            }
        }
        360.0 - gap
    }
}

/// Least-squares circle through 2D points: (centre x, centre y, radius).
pub(crate) fn fit_circle_2d(points: &[(f64, f64)]) -> Option<(f64, f64, f64)> {
    fit_circle(points)
}

fn fit_circle(points: &[(f64, f64)]) -> Option<(f64, f64, f64)> {
    if points.len() < 3 {
        return None;
    }
    // Centre the data for conditioning, then solve x² + y² = 2ax + 2by + c.
    let (mx, my) = points.iter().fold((0.0, 0.0), |(sx, sy), (x, y)| (sx + x, sy + y));
    let k = points.len() as f64;
    let (mx, my) = (mx / k, my / k);
    let mut ata = [[0.0f64; 3]; 3];
    let mut atb = [0.0f64; 3];
    for (x, y) in points {
        let (x, y) = (x - mx, y - my);
        let row = [2.0 * x, 2.0 * y, 1.0];
        let rhs = x * x + y * y;
        for i in 0..3 {
            for j in 0..3 {
                ata[i][j] += row[i] * row[j];
            }
            atb[i] += row[i] * rhs;
        }
    }
    let [a, b, c] = solve3(ata, atb)?;
    let r2 = c + a * a + b * b;
    (r2 > 0.0).then(|| (a + mx, b + my, r2.sqrt()))
}

/// Solves a 3×3 linear system by Cramer's rule; None when it is singular.
fn solve3(m: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: &[[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(&m);
    let scale = m.iter().flatten().fold(0.0f64, |a, v| a.max(v.abs())).max(1.0e-300);
    if d.abs() <= scale.powi(3) * 1.0e-12 {
        return None;
    }
    let mut out = [0.0; 3];
    for (col, slot) in out.iter_mut().enumerate() {
        let mut mc = m;
        for (row, value) in b.iter().enumerate() {
            mc[row][col] = *value;
        }
        *slot = det(&mc) / d;
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// Eigenvalues and unit eigenvectors of a symmetric 3×3 matrix (cyclic Jacobi).
fn symmetric_eigen(mut a: [[f64; 3]; 3]) -> ([f64; 3], [DVec3; 3]) {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..32 {
        let off = a[0][1].abs() + a[0][2].abs() + a[1][2].abs();
        if off < 1.0e-15 {
            break;
        }
        for (p, q) in [(0usize, 1usize), (0, 2), (1, 2)] {
            if a[p][q].abs() < 1.0e-300 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let t = if theta == 0.0 { 1.0 } else { t };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            for row in &mut a {
                let (akp, akq) = (row[p], row[q]);
                row[p] = c * akp - s * akq;
                row[q] = s * akp + c * akq;
            }
            let (row_p, row_q) = (a[p], a[q]);
            for k in 0..3 {
                a[p][k] = c * row_p[k] - s * row_q[k];
                a[q][k] = s * row_p[k] + c * row_q[k];
            }
            for row in &mut v {
                let (vp, vq) = (row[p], row[q]);
                row[p] = c * vp - s * vq;
                row[q] = s * vp + c * vq;
            }
        }
    }
    let col = |j: usize| DVec3::new(v[0][j], v[1][j], v[2][j]);
    ([a[0][0], a[1][1], a[2][2]], [col(0), col(1), col(2)])
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    fn quad(soup: &mut Vec<[Vec3; 3]>, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
        soup.push([a, b, c]);
        soup.push([a, c, d]);
    }

    /// A 40 × 30 × 10 block with a Ø12 boss of height 8 on top, centred at (10, 15).
    fn block_with_boss(segments: usize) -> Result<Mesh, crate::MeshError> {
        let v = Vec3::new;
        let mut s = Vec::new();
        let (x1, y1, z1) = (40.0, 30.0, 10.0);
        quad(&mut s, v(0., 0., 0.), v(0., y1, 0.), v(x1, y1, 0.), v(x1, 0., 0.));
        quad(&mut s, v(0., 0., 0.), v(x1, 0., 0.), v(x1, 0., z1), v(0., 0., z1));
        quad(&mut s, v(x1, 0., 0.), v(x1, y1, 0.), v(x1, y1, z1), v(x1, 0., z1));
        quad(&mut s, v(x1, y1, 0.), v(0., y1, 0.), v(0., y1, z1), v(x1, y1, z1));
        quad(&mut s, v(0., y1, 0.), v(0., 0., 0.), v(0., 0., z1), v(0., y1, z1));
        // Top face as a fan around the boss footprint is fiddly; leave it as two triangles
        // (the boss sits on it, which is fine for recognition).
        quad(&mut s, v(0., 0., z1), v(x1, 0., z1), v(x1, y1, z1), v(0., y1, z1));
        let (c, r, h) = (v(10.0, 15.0, z1), 6.0, 8.0);
        let ring = |i: usize| {
            let a = TAU * i as f32 / segments as f32;
            c + v(a.cos() * r, a.sin() * r, 0.0)
        };
        for i in 0..segments {
            let (b0, b1) = (ring(i), ring(i + 1));
            let (t0, t1) = (b0 + Vec3::Z * h, b1 + Vec3::Z * h);
            quad(&mut s, b0, b1, t1, t0);
            s.push([c + Vec3::Z * h, t0, t1]);
        }
        Mesh::from_soup(s)
    }

    #[test]
    fn finds_planes_and_a_boss() -> Result<(), crate::MeshError> {
        let m = block_with_boss(32)?;
        let map = segment(&m);
        assert_eq!(map.triangle_face.len(), m.triangles().len());
        let planes = map.faces.iter().filter(|f| matches!(f.kind, FaceKind::Plane { .. })).count();
        let cylinders: Vec<&Face> = map.faces.iter().filter(|f| matches!(f.kind, FaceKind::Cylinder { .. })).collect();
        assert_eq!(planes, 7, "{:#?}", map.faces);
        assert_eq!(cylinders.len(), 1, "{:#?}", map.faces);
        let FaceKind::Cylinder { base, axis, radius, length, hole } = cylinders[0].kind else {
            return Ok(());
        };
        assert!((radius - 6.0).abs() < 1e-3, "{radius}");
        assert!((length - 8.0).abs() < 1e-3);
        assert!(axis.z.abs() > 0.9999);
        assert!(!hole);
        let off = base - Vec3::new(10.0, 15.0, if axis.z > 0.0 { 10.0 } else { 18.0 });
        assert!(off.truncate().length() < 1e-3, "{base:?}");
        Ok(())
    }

    #[test]
    fn inward_facing_cylinders_are_holes() -> Result<(), crate::MeshError> {
        let m = block_with_boss(24)?;
        let flipped: Vec<[Vec3; 3]> = m.triangles().iter().map(|t| m.corners([t[0], t[2], t[1]])).collect();
        let map = segment(&Mesh::from_soup(flipped)?);
        assert!(map.faces.iter().any(|f| matches!(f.kind, FaceKind::Cylinder { hole: true, .. })), "{:#?}", map.faces);
        Ok(())
    }

    #[test]
    fn a_plane_face_has_its_outline_as_a_loop() -> Result<(), crate::MeshError> {
        let m = block_with_boss(16)?;
        let map = segment(&m);
        let bottom =
            map.faces.iter().position(|f| matches!(f.kind, FaceKind::Plane { normal, .. } if normal.z < -0.99));
        let loops = bottom.map(|f| boundary_loops(&m, &map, f)).unwrap_or_default();
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].len(), 4, "{:?}", loops[0]);
        Ok(())
    }

    #[test]
    fn a_coarse_prism_is_still_planes() -> Result<(), crate::MeshError> {
        // A hexagonal boss's sides meet at 60°: six planes, not a cylinder.
        let map = segment(&block_with_boss(6)?);
        assert!(map.faces.iter().all(|f| !matches!(f.kind, FaceKind::Cylinder { .. })));
        Ok(())
    }

    #[test]
    fn eigen_and_circle_helpers() {
        let (values, vectors) = symmetric_eigen([[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]]);
        let i = values.iter().enumerate().min_by(|a, b| a.1.total_cmp(b.1)).map_or(9, |p| p.0);
        assert!(vectors[i].z.abs() > 0.999);
        let pts: Vec<(f64, f64)> = (0..7)
            .map(|k| (k as f64).to_radians() * 40.0)
            .map(|a| (3.0 + 2.0 * a.cos(), -1.0 + 2.0 * a.sin()))
            .collect();
        let (cx, cy, r) = fit_circle(&pts).unwrap_or_default();
        assert!((cx - 3.0).abs() < 1e-9 && (cy + 1.0).abs() < 1e-9 && (r - 2.0).abs() < 1e-9);
        assert!(fit_circle(&[(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]).is_none());
    }

    #[test]
    fn empty_and_degenerate_meshes_do_not_panic() -> Result<(), crate::MeshError> {
        assert!(segment(&Mesh::default()).faces.is_empty());
        let m = Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::X * 2.0], vec![[0, 1, 2]])?;
        assert_eq!(segment(&m).faces.len(), 1);
        Ok(())
    }
}
