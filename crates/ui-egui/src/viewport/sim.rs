//! Simulation geometry for the viewport: the cut stock as a shaded height surface with walls,
//! and the tool at its current position.

use CosmicCAM_engine::sim::{Sim, SimFrame, SimTool};
use CosmicCAM_geom::Vec3;
use CosmicCAM_render::scene::Rgba;
use CosmicCAM_render::scene::{SceneGeometry, Vertex};
use glam::{DVec2, DVec3};

/// Most grid points along one side of the drawn stock; finer stock grids are drawn sampled.
const MAX_DRAWN: usize = 240;

/// Colours for the simulated stock.
pub struct SimColours {
    pub stock: Rgba,
    pub cut: Rgba,
    pub gouge: Rgba,
    pub flutes: Rgba,
    pub shank: Rgba,
}

fn world_normal(f: &SimFrame, n: DVec3) -> Vec3 {
    (f.x * n.x + f.y * n.y + f.z * n.z).normalize_or_zero().as_vec3()
}

/// The stock as cut so far: a smooth top surface over the height field, the walls around it
/// and its bottom. Cut areas, uncut stock and gouges get their own colours.
pub fn stock(sim: &Sim, c: &SimColours) -> SceneGeometry {
    let z = &sim.zmap;
    let f = &sim.frame;
    let (nx, ny) = z.size();
    let cell = z.cell();
    let step = nx.div_ceil(MAX_DRAWN).max(ny.div_ceil(MAX_DRAWN)).max(1);
    let (gx, gy) = (nx.div_ceil(step), ny.div_ceil(step));
    // Cut means below the stock's original top.
    let top = z.top();
    let mut gouged = vec![false; nx * ny];
    for (i, j, _) in sim.gouges.iter().flatten() {
        if let Some(g) = gouged.get_mut(j * nx + i) {
            *g = true;
        }
    }
    // Each grid point takes the lowest cell around it, so narrow cuts stay visible when sampled.
    let w = gx + 1;
    let mut h = vec![0.0f64; w * (gy + 1)];
    let mut colour = vec![c.stock; w * (gy + 1)];
    for v in 0..=gy {
        for u in 0..=gx {
            let (ci, cj) = (u * step, v * step);
            let (i0, i1) =
                (ci.saturating_sub(step.max(1)), (ci + step).min(nx).max(ci.saturating_sub(step.max(1)) + 1));
            let (j0, j1) =
                (cj.saturating_sub(step.max(1)), (cj + step).min(ny).max(cj.saturating_sub(step.max(1)) + 1));
            let mut low = f64::INFINITY;
            let mut gouge = false;
            for j in j0..j1.min(ny) {
                for i in i0..i1.min(nx) {
                    if let Some(hz) = z.height(i, j) {
                        low = low.min(hz);
                    }
                    gouge |= gouged.get(j * nx + i).copied().unwrap_or(false);
                }
            }
            let low = if low.is_finite() { low } else { top };
            if let Some(slot) = h.get_mut(v * w + u) {
                *slot = low;
            }
            if let Some(slot) = colour.get_mut(v * w + u) {
                *slot = if gouge {
                    c.gouge
                } else if low < top - 1.0e-3 {
                    c.cut
                } else {
                    c.stock
                };
            }
        }
    }
    let origin = z.origin();
    let span = DVec2::new(nx as f64, ny as f64) * cell;
    let at = |u: usize, v: usize| -> DVec2 {
        (origin + DVec2::new(u as f64, v as f64) * cell * step as f64).min(origin + span)
    };
    let height = |u: usize, v: usize| h.get(v * w + u).copied().unwrap_or(top);
    let mut g = SceneGeometry::default();
    g.triangles.reserve(gx * gy * 6 + (gx + gy) * 12 + 6);
    // Top surface, shaded with normals from the neighbouring heights.
    let normal = |u: usize, v: usize| -> Vec3 {
        let (ul, ur) = (u.saturating_sub(1), (u + 1).min(gx));
        let (vd, vu) = (v.saturating_sub(1), (v + 1).min(gy));
        let dx = (at(ur, v).x - at(ul, v).x).max(1.0e-9);
        let dy = (at(u, vu).y - at(u, vd).y).max(1.0e-9);
        let n = DVec3::new(-(height(ur, v) - height(ul, v)) / dx, -(height(u, vu) - height(u, vd)) / dy, 1.0);
        world_normal(f, n)
    };
    let vertex = |u: usize, v: usize| -> Vertex {
        let p = at(u, v).extend(height(u, v));
        Vertex::lit(f.world(p), normal(u, v), colour.get(v * w + u).copied().unwrap_or(c.stock))
    };
    for v in 0..gy {
        for u in 0..gx {
            let (a, b, cc, d) = (vertex(u, v), vertex(u + 1, v), vertex(u + 1, v + 1), vertex(u, v + 1));
            g.triangles.extend_from_slice(&[a, b, cc, a, cc, d]);
        }
    }
    // Walls down to the stock's bottom, and the bottom.
    let bottom = z.bottom();
    let mut wall = |p: DVec2, q: DVec2, hp: f64, hq: f64, out: DVec3| {
        let n = world_normal(f, out);
        let (a, b) = (f.world(p.extend(bottom)), f.world(q.extend(bottom)));
        let (cq, cp) = (f.world(q.extend(hq)), f.world(p.extend(hp)));
        let v = |x: Vec3| Vertex::lit(x, n, c.stock);
        g.triangles.extend_from_slice(&[v(a), v(b), v(cq), v(a), v(cq), v(cp)]);
    };
    for u in 0..gx {
        wall(at(u, 0), at(u + 1, 0), height(u, 0), height(u + 1, 0), DVec3::NEG_Y);
        wall(at(u + 1, gy), at(u, gy), height(u + 1, gy), height(u, gy), DVec3::Y);
    }
    for v in 0..gy {
        wall(at(gx, v), at(gx, v + 1), height(gx, v), height(gx, v + 1), DVec3::X);
        wall(at(0, v + 1), at(0, v), height(0, v + 1), height(0, v), DVec3::NEG_X);
    }
    let n = world_normal(f, DVec3::NEG_Z);
    let corner = |p: DVec2| Vertex::lit(f.world(p.extend(bottom)), n, c.stock);
    let (p0, p1, p2, p3) = (corner(at(0, 0)), corner(at(gx, 0)), corner(at(gx, gy)), corner(at(0, gy)));
    g.triangles.extend_from_slice(&[p0, p2, p1, p0, p3, p2]);
    for p in [at(0, 0), at(gx, gy)] {
        g.bounds.include(f.world(p.extend(bottom)));
        g.bounds.include(f.world(p.extend(top)));
    }
    g
}

/// The tool with its tip at `tip`: flutes in one colour, the shank above them in another.
pub fn tool(f: &SimFrame, tip: DVec3, t: &SimTool, c: &SimColours) -> SceneGeometry {
    let mut g = SceneGeometry::default();
    let flutes = t.flute_length.max(t.radius).min(t.length.max(t.radius));
    cylinder(&mut g, f, tip, t.radius, flutes, c.flutes);
    let shank = (t.length - flutes).max(0.0);
    if shank > 1.0e-6 {
        cylinder(&mut g, f, tip + DVec3::Z * flutes, t.shank_radius.max(t.radius * 0.5), shank, c.shank);
    }
    g
}

fn cylinder(g: &mut SceneGeometry, f: &SimFrame, base: DVec3, radius: f64, height: f64, colour: Rgba) {
    const SIDES: usize = 32;
    let ring = |k: usize| {
        let a = std::f64::consts::TAU * k as f64 / SIDES as f64;
        DVec2::new(a.cos(), a.sin())
    };
    let top = base + DVec3::Z * height;
    for k in 0..SIDES {
        let (d0, d1) = (ring(k), ring(k + 1));
        let p = |c: DVec3, d: DVec2| f.world(c + (d * radius).extend(0.0));
        let n0 = world_normal(f, d0.extend(0.0));
        let n1 = world_normal(f, d1.extend(0.0));
        let (a, b, cc, d) = (
            Vertex::lit(p(base, d0), n0, colour),
            Vertex::lit(p(base, d1), n1, colour),
            Vertex::lit(p(top, d1), n1, colour),
            Vertex::lit(p(top, d0), n0, colour),
        );
        g.triangles.extend_from_slice(&[a, b, cc, a, cc, d]);
        let (up, down) = (world_normal(f, DVec3::Z), world_normal(f, DVec3::NEG_Z));
        g.triangles.extend_from_slice(&[
            Vertex::lit(f.world(top), up, colour),
            Vertex::lit(p(top, d0), up, colour),
            Vertex::lit(p(top, d1), up, colour),
            Vertex::lit(f.world(base), down, colour),
            Vertex::lit(p(base, d1), down, colour),
            Vertex::lit(p(base, d0), down, colour),
        ]);
    }
}
