//! Backend-neutral scene buffers: the viewport backend uploads these as-is.
//!
//! Lines and triangles share one vertex layout. A zero normal marks an unlit vertex.

use std::f32::consts::TAU;

use CosmicDMS_geom::{Aabb, Mesh, Vec3};

pub type Rgba = [f32; 4];

/// Mesh edges between faces meeting at more than this are outlined.
pub const MESH_CREASE_DEGREES: f32 = 30.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: Rgba,
}

impl Vertex {
    pub fn line(pos: Vec3, color: Rgba) -> Self {
        Self { pos: pos.to_array(), normal: [0.0; 3], color }
    }

    pub fn lit(pos: Vec3, normal: Vec3, color: Rgba) -> Self {
        Self { pos: pos.to_array(), normal: normal.to_array(), color }
    }
}

/// Colours the scene needs from the theme, already in linear RGBA.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScenePalette {
    pub grid_minor: Rgba,
    pub grid_major: Rgba,
    pub axis_x: Rgba,
    pub axis_y: Rgba,
    pub axis_z: Rgba,
    pub part: Rgba,
    pub part_edge: Rgba,
    pub stock_edge: Rgba,
    /// Toolpath colours by move kind.
    pub path_cut: Rgba,
    pub path_rapid: Rgba,
    pub path_plunge: Rgba,
    pub path_lead: Rgba,
    pub path_link: Rgba,
}

/// What a toolpath move is for, as the viewport colours it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    Cut,
    Rapid,
    Plunge,
    Lead,
    Link,
}

/// Triangle list plus line list, both in world millimetres.
#[derive(Clone, Debug, Default)]
pub struct SceneGeometry {
    pub triangles: Vec<Vertex>,
    pub lines: Vec<Vertex>,
    pub bounds: Aabb,
}

impl SceneGeometry {
    pub fn push_line(&mut self, a: Vec3, b: Vec3, color: Rgba) {
        self.lines.push(Vertex::line(a, color));
        self.lines.push(Vertex::line(b, color));
    }

    pub fn push_triangle(&mut self, a: Vec3, b: Vec3, c: Vec3, color: Rgba) {
        let n = (b - a).cross(c - a).normalize_or_zero();
        for p in [a, b, c] {
            self.triangles.push(Vertex::lit(p, n, color));
        }
    }

    /// Axis-aligned box with shaded faces and outlined edges.
    pub fn push_box(&mut self, b: &Aabb, fill: Rgba, edge: Option<Rgba>) {
        let c = box_corners(b);
        // Faces as corner indices wound counter-clockwise seen from outside.
        const FACES: [[usize; 4]; 6] = [
            [0, 3, 2, 1], // bottom
            [4, 5, 6, 7], // top
            [0, 1, 5, 4], // front (-Y)
            [2, 3, 7, 6], // back (+Y)
            [1, 2, 6, 5], // right (+X)
            [3, 0, 4, 7], // left (-X)
        ];
        for [i0, i1, i2, i3] in FACES {
            self.push_triangle(c[i0], c[i1], c[i2], fill);
            self.push_triangle(c[i0], c[i2], c[i3], fill);
        }
        if let Some(edge) = edge {
            self.push_box_edges(b, edge);
        }
        self.bounds.include(b.min);
        self.bounds.include(b.max);
    }

    pub fn push_box_edges(&mut self, b: &Aabb, color: Rgba) {
        let c = box_corners(b);
        const EDGES: [(usize, usize); 12] =
            [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)];
        for (a, z) in EDGES {
            self.push_line(c[a], c[z], color);
        }
    }

    /// A triangle mesh, flat shaded, with its feature edges outlined.
    pub fn push_mesh(&mut self, mesh: &Mesh, fill: Rgba, edge: Option<Rgba>) {
        self.triangles.reserve(mesh.triangles().len() * 3);
        for &t in mesh.triangles() {
            let n = mesh.face_normal(t);
            for p in mesh.corners(t) {
                self.triangles.push(Vertex::lit(p, n, fill));
            }
        }
        if let Some(edge) = edge {
            let p = mesh.positions();
            for [a, b] in mesh.feature_edges(MESH_CREASE_DEGREES) {
                if let (Some(&a), Some(&b)) = (p.get(a as usize), p.get(b as usize)) {
                    self.push_line(a, b, edge);
                }
            }
        }
        let b = mesh.bounds();
        self.bounds.include(b.min);
        self.bounds.include(b.max);
    }

    /// A toolpath as lines from `start` through each move's end point, coloured by kind.
    pub fn push_path(
        &mut self,
        start: Vec3,
        moves: impl IntoIterator<Item = (PathKind, Vec3)>,
        palette: &ScenePalette,
    ) {
        let mut at = start;
        for (kind, to) in moves {
            let color = match kind {
                PathKind::Cut => palette.path_cut,
                PathKind::Rapid => palette.path_rapid,
                PathKind::Plunge => palette.path_plunge,
                PathKind::Lead => palette.path_lead,
                PathKind::Link => palette.path_link,
            };
            if to.is_finite() {
                self.push_line(at, to, color);
                self.bounds.include(to);
                at = to;
            }
        }
    }

    /// Vertical cylinder standing on `base`.
    pub fn push_cylinder(
        &mut self,
        base: Vec3,
        radius: f32,
        height: f32,
        segments: usize,
        fill: Rgba,
        edge: Option<Rgba>,
    ) {
        let segments = segments.clamp(3, 256);
        let top = base + Vec3::Z * height;
        let ring = |i: usize| {
            let a = TAU * i as f32 / segments as f32;
            Vec3::new(a.cos(), a.sin(), 0.0)
        };
        for i in 0..segments {
            // The last segment closes on the first exactly, so the seam welds.
            let (d0, d1) = (ring(i), ring((i + 1) % segments));
            let (b0, b1) = (base + d0 * radius, base + d1 * radius);
            let (t0, t1) = (top + d0 * radius, top + d1 * radius);
            // Smooth side normals.
            for (p, n) in [(b0, d0), (b1, d1), (t1, d1), (b0, d0), (t1, d1), (t0, d0)] {
                self.triangles.push(Vertex::lit(p, n, fill));
            }
            self.push_triangle(top, t0, t1, fill);
            self.push_triangle(base, b1, b0, fill);
            if let Some(edge) = edge {
                self.push_line(t0, t1, edge);
                self.push_line(b0, b1, edge);
            }
        }
        self.bounds.include(base - Vec3::new(radius, radius, 0.0));
        self.bounds.include(top + Vec3::new(radius, radius, 0.0));
    }

    /// Shaded arrow (shaft + cone) from `origin` along unit `dir`.
    pub fn push_arrow(&mut self, origin: Vec3, dir: Vec3, length: f32, radius: f32, color: Rgba) {
        let dir = dir.normalize_or_zero();
        if dir == Vec3::ZERO || length.is_nan() || length <= 0.0 || radius.is_nan() || radius <= 0.0 {
            return;
        }
        let (u, v) = dir.any_orthonormal_pair();
        let segments = 12;
        let head_len = (length * 0.28).min(radius * 8.0);
        let shaft_end = origin + dir * (length - head_len);
        let tip = origin + dir * length;
        let head_r = radius * 2.6;
        let ring = |i: usize| {
            let a = TAU * i as f32 / segments as f32;
            u * a.cos() + v * a.sin()
        };
        for i in 0..segments {
            let (d0, d1) = (ring(i), ring(i + 1));
            let (s0, s1) = (origin + d0 * radius, origin + d1 * radius);
            let (e0, e1) = (shaft_end + d0 * radius, shaft_end + d1 * radius);
            for (p, n) in [(s0, d0), (s1, d1), (e1, d1), (s0, d0), (e1, d1), (e0, d0)] {
                self.triangles.push(Vertex::lit(p, n, color));
            }
            let (h0, h1) = (shaft_end + d0 * head_r, shaft_end + d1 * head_r);
            self.push_triangle(h0, h1, tip, color);
            self.push_triangle(shaft_end, h1, h0, color);
        }
    }
}

fn box_corners(b: &Aabb) -> [Vec3; 8] {
    let (n, x) = (b.min, b.max);
    [
        Vec3::new(n.x, n.y, n.z),
        Vec3::new(x.x, n.y, n.z),
        Vec3::new(x.x, x.y, n.z),
        Vec3::new(n.x, x.y, n.z),
        Vec3::new(n.x, n.y, x.z),
        Vec3::new(x.x, n.y, x.z),
        Vec3::new(x.x, x.y, x.z),
        Vec3::new(n.x, x.y, x.z),
    ]
}

/// Ground grid on the XY plane covering `around` with some margin: 10 mm lines with a major
/// line every 50 mm, or with `inch`, 0.5 in lines with a major line every inch.
pub fn grid(around: &Aabb, palette: &ScenePalette, inch: bool) -> SceneGeometry {
    let (minor, major_every): (f32, i32) = if inch { (12.7, 2) } else { (10.0, 5) };
    #[allow(non_snake_case)]
    let (MINOR, MAJOR_EVERY) = (minor, major_every);
    const MAX_LINES_PER_AXIS: i32 = 400;
    let mut g = SceneGeometry::default();
    let reach = if around.is_empty() {
        100.0
    } else {
        let s = around.size();
        let c = around.center();
        (s.x.max(s.y) * 0.5 + c.x.abs().max(c.y.abs())) * 1.6
    };
    let half_lines = ((reach / MINOR).ceil() as i32).clamp(MAJOR_EVERY, MAX_LINES_PER_AXIS);
    let half_lines = (half_lines + MAJOR_EVERY - 1) / MAJOR_EVERY * MAJOR_EVERY;
    let extent = half_lines as f32 * MINOR;
    for i in -half_lines..=half_lines {
        if i == 0 {
            continue; // the axes are drawn on top
        }
        let t = i as f32 * MINOR;
        let color = if i % MAJOR_EVERY == 0 { palette.grid_major } else { palette.grid_minor };
        g.push_line(Vec3::new(t, -extent, 0.0), Vec3::new(t, extent, 0.0), color);
        g.push_line(Vec3::new(-extent, t, 0.0), Vec3::new(extent, t, 0.0), color);
    }
    let fade = |c: Rgba| [c[0], c[1], c[2], c[3] * 0.55];
    g.push_line(Vec3::new(0.0, 0.0, 0.0), Vec3::new(extent, 0.0, 0.0), fade(palette.axis_x));
    g.push_line(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, extent, 0.0), fade(palette.axis_y));
    g.push_line(Vec3::new(-extent, 0.0, 0.0), Vec3::ZERO, palette.grid_major);
    g.push_line(Vec3::new(0.0, -extent, 0.0), Vec3::ZERO, palette.grid_major);
    g
}

/// The WCS orientation triad drawn in the scene at `origin`.
pub fn wcs_triad(origin: Vec3, size: f32, palette: &ScenePalette) -> SceneGeometry {
    let mut g = SceneGeometry::default();
    g.push_triad(origin, [Vec3::X, Vec3::Y, Vec3::Z], size, palette, TriadStyle::Full);
    g
}

/// How prominently a coordinate-system triad is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriadStyle {
    /// The active WCS, or the world origin while no WCS exists.
    Full,
    /// An inactive WCS: greyed and thinner.
    Dim,
    /// The world (model) origin once a WCS exists: shorter, thin and mostly grey, there
    /// for reference without competing with the WCS.
    Faint,
}

impl TriadStyle {
    /// Share of the axis colour kept; the rest is grey.
    pub fn colour_share(self) -> f32 {
        match self {
            Self::Full => 1.0,
            Self::Dim => 0.45,
            Self::Faint => 0.25,
        }
    }

    fn radius_ratio(self) -> f32 {
        match self {
            Self::Full => 0.025,
            Self::Dim => 0.016,
            Self::Faint => 0.011,
        }
    }

    fn length_ratio(self) -> f32 {
        if self == Self::Faint { 0.7 } else { 1.0 }
    }
}

/// Length of a triad's arrows for a requested `size`.
pub fn triad_length(size: f32, style: TriadStyle) -> f32 {
    let size = if size.is_finite() { size.clamp(5.0, 500.0) } else { 20.0 };
    size * style.length_ratio()
}

impl SceneGeometry {
    /// An XYZ triad of arrows along `axes` at `origin`, drawn in `style`.
    pub fn push_triad(&mut self, origin: Vec3, axes: [Vec3; 3], size: f32, palette: &ScenePalette, style: TriadStyle) {
        let length = triad_length(size, style);
        let r = length / style.length_ratio() * style.radius_ratio();
        let grey = palette.part_edge;
        let k = style.colour_share();
        let shade = |c: Rgba| -> Rgba { std::array::from_fn(|i| c[i] * k + grey[i] * (1.0 - k)) };
        for (axis, color) in axes.into_iter().zip([palette.axis_x, palette.axis_y, palette.axis_z]) {
            self.push_arrow(origin, axis, length, r, shade(color));
        }
    }
}

/// Bounds of the placeholder part itself, without its stock.
pub const SAMPLE_PART_BOUNDS: Aabb = Aabb { min: Vec3::new(-50.0, -30.0, 0.0), max: Vec3::new(50.0, 30.0, 42.0) };

/// The sample part's stock: its bounds plus 3 mm on the sides and top.
pub fn sample_stock_box() -> Aabb {
    Aabb::new(SAMPLE_PART_BOUNDS.min - Vec3::new(3.0, 3.0, 0.0), SAMPLE_PART_BOUNDS.max + Vec3::new(3.0, 3.0, 3.0))
}

/// Placeholder workpiece for trying the app without a model, with optional outline
/// edges and stock box. The bounds always include the stock.
pub fn sample_part(palette: &ScenePalette, edges: bool, stock: bool) -> SceneGeometry {
    let mut g = SceneGeometry::default();
    let edge = edges.then_some(palette.part_edge);
    g.push_box(&Aabb::new(Vec3::new(-50.0, -30.0, 0.0), Vec3::new(50.0, 30.0, 20.0)), palette.part, edge);
    g.push_box(&Aabb::new(Vec3::new(-40.0, -20.0, 20.0), Vec3::new(5.0, 20.0, 35.0)), palette.part, edge);
    g.push_cylinder(Vec3::new(28.0, 0.0, 20.0), 12.0, 22.0, 48, palette.part, edge);
    let stock_box = Aabb::new(g.bounds.min - Vec3::new(3.0, 3.0, 0.0), g.bounds.max + Vec3::new(3.0, 3.0, 3.0));
    if stock {
        g.push_box_edges(&stock_box, palette.stock_edge);
    }
    g.bounds.include(stock_box.min);
    g.bounds.include(stock_box.max);
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> ScenePalette {
        let c = [0.5, 0.5, 0.5, 1.0];
        ScenePalette {
            grid_minor: c,
            grid_major: c,
            axis_x: c,
            axis_y: c,
            axis_z: c,
            part: c,
            part_edge: c,
            stock_edge: c,
            path_cut: c,
            path_rapid: c,
            path_plunge: c,
            path_lead: c,
            path_link: c,
        }
    }

    #[test]
    fn buffers_are_well_formed() {
        let p = palette();
        for g in [sample_part(&p, true, true), grid(&Aabb::EMPTY, &p, false), wcs_triad(Vec3::ZERO, 20.0, &p)] {
            assert_eq!(g.triangles.len() % 3, 0);
            assert_eq!(g.lines.len() % 2, 0);
            assert!(g.triangles.iter().chain(&g.lines).all(|v| v.pos.iter().all(|c| c.is_finite())));
        }
    }

    #[test]
    fn box_faces_point_outwards() {
        let mut g = SceneGeometry::default();
        let b = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        g.push_box(&b, [1.0; 4], None);
        for tri in g.triangles.chunks(3) {
            let centroid = tri.iter().map(|v| Vec3::from(v.pos)).sum::<Vec3>() / 3.0;
            let n = Vec3::from(tri[0].normal);
            assert!(n.dot(centroid) > 0.0, "inward face at {centroid:?}");
        }
    }

    #[test]
    fn grid_is_capped_for_huge_scenes() {
        let huge = Aabb::new(Vec3::splat(-1.0e9), Vec3::splat(1.0e9));
        let g = grid(&huge, &palette(), false);
        assert!(g.lines.len() <= 4 * 801 + 8);
    }

    #[test]
    fn meshes_are_drawn_with_their_outline() -> Result<(), CosmicDMS_geom::MeshError> {
        let (a, b, c, d) = (Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z);
        let tetra = Mesh::from_soup([[a, c, b], [a, b, d], [b, c, d], [c, a, d]])?;
        let mut g = SceneGeometry::default();
        g.push_mesh(&tetra, [1.0; 4], Some([0.0; 4]));
        assert_eq!(g.triangles.len(), 12);
        assert_eq!(g.lines.len(), 12);
        assert_eq!(g.bounds, Aabb::new(Vec3::ZERO, Vec3::ONE));
        for tri in g.triangles.chunks(3) {
            let centroid = tri.iter().map(|v| Vec3::from(v.pos)).sum::<Vec3>() / 3.0;
            assert!(Vec3::from(tri[0].normal).dot(centroid - Vec3::splat(0.25)) > 0.0);
        }
        Ok(())
    }

    #[test]
    fn faint_triads_are_shorter_and_greyer() {
        let mut p = palette();
        p.axis_x = [1.0, 0.0, 0.0, 1.0];
        p.part_edge = [0.5, 0.5, 0.5, 1.0];
        let reach = |style| {
            let mut g = SceneGeometry::default();
            g.push_triad(Vec3::ZERO, [Vec3::X, Vec3::Y, Vec3::Z], 40.0, &p, style);
            let x_max = g.triangles.iter().map(|v| v.pos[0]).fold(0.0, f32::max);
            let red = g.triangles.iter().map(|v| v.color[0] - v.color[1]).fold(0.0, f32::max);
            (x_max, red)
        };
        let (full_len, full_red) = reach(TriadStyle::Full);
        let (faint_len, faint_red) = reach(TriadStyle::Faint);
        assert!((full_len - triad_length(40.0, TriadStyle::Full)).abs() < 1e-3);
        assert!(faint_len < full_len * 0.8 && faint_red < full_red * 0.5, "{faint_len} {faint_red}");
    }

    #[test]
    fn sample_part_bounds_constant_matches_the_part() {
        let g = sample_part(&palette(), false, false);
        let b = Aabb::from_points(g.triangles.iter().map(|v| Vec3::from(v.pos)));
        assert!((b.min - SAMPLE_PART_BOUNDS.min).abs().max_element() < 1e-3, "{b:?}");
        assert!((b.max - SAMPLE_PART_BOUNDS.max).abs().max_element() < 1e-3, "{b:?}");
    }

    #[test]
    fn sample_part_bounds_include_stock() {
        let g = sample_part(&palette(), true, true);
        assert!(sample_part(&palette(), false, false).lines.is_empty());
        assert!(!g.bounds.is_empty());
        assert!(g.bounds.min.z <= 0.0 && g.bounds.max.z >= 42.0);
    }
}
