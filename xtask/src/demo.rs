//! `cargo xtask demo`: builds the demo project from scratch.
//!
//! 1. The demo part, as a closed triangle mesh written to `samples/demo-part.obj`: a base plate,
//!    a block whose +X end is a 45° slope with a hole drilled square to it (3+2 work), a Ø24
//!    blind bore in the block (helical boring) and a spherical dome (5-axis finishing).
//! 2. The demo job, made with the engine's commands as a user would: a 5-axis trunnion with the
//!    sample tool library, a setup and its stock, and toolpaths using all of it; checked for
//!    gouges and collisions, then saved to `samples/demo.cosmic`.
//! 3. The sample tool library as a file, `samples/tools.json`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use CosmicDMS_engine::Session;
use serde_json::{Value, json};

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 0.0 { mul(a, 1.0 / l) } else { a }
}

/// A triangle mesh with shared vertices.
#[derive(Default)]
struct MeshBuilder {
    vertices: Vec<V3>,
    index: HashMap<[i64; 3], u32>,
    triangles: Vec<[u32; 3]>,
}

impl MeshBuilder {
    fn vertex(&mut self, p: V3) -> u32 {
        let key = [(p[0] * 1.0e6).round() as i64, (p[1] * 1.0e6).round() as i64, (p[2] * 1.0e6).round() as i64];
        if let Some(i) = self.index.get(&key) {
            return *i;
        }
        let i = self.vertices.len() as u32;
        self.vertices.push(p);
        self.index.insert(key, i);
        i
    }

    fn triangle(&mut self, a: V3, b: V3, c: V3) {
        let (a, b, c) = (self.vertex(a), self.vertex(b), self.vertex(c));
        if a != b && b != c && a != c {
            self.triangles.push([a, b, c]);
        }
    }

    fn quad(&mut self, a: V3, b: V3, c: V3, d: V3) {
        self.triangle(a, b, c);
        self.triangle(a, c, d);
    }

    /// A planar face facing `normal`: the `outer` loop with `holes` cut out, triangulated.
    fn face(&mut self, normal: V3, outer: &[V3], holes: &[Vec<V3>]) {
        let n = norm(normal);
        let e1 = norm(if n[0].abs() < 0.9 { cross([1.0, 0.0, 0.0], n) } else { cross([0.0, 1.0, 0.0], n) });
        let e2 = cross(n, e1);
        let uv = |p: V3| [dot(p, e1), dot(p, e2)];
        let area = |l: &[V3]| {
            let q: Vec<[f64; 2]> = l.iter().map(|p| uv(*p)).collect();
            (0..q.len())
                .map(|i| {
                    let (a, b) = (q[i], q[(i + 1) % q.len()]);
                    a[0] * b[1] - b[0] * a[1]
                })
                .sum::<f64>()
        };
        // Outer counter-clockwise, holes clockwise, seen from outside.
        let mut poly: Vec<V3> = outer.to_vec();
        if area(&poly) < 0.0 {
            poly.reverse();
        }
        let mut holes: Vec<Vec<V3>> = holes
            .iter()
            .map(|h| {
                let mut h = h.clone();
                if area(&h) > 0.0 {
                    h.reverse();
                }
                h
            })
            .collect();
        // Bridge holes in, rightmost first, from their rightmost point to a visible vertex.
        holes.sort_by(|a, b| {
            let mx = |h: &Vec<V3>| h.iter().map(|p| uv(*p)[0]).fold(f64::MIN, f64::max);
            mx(b).total_cmp(&mx(a))
        });
        for h in &holes {
            let mi = (0..h.len()).max_by(|&a, &b| uv(h[a])[0].total_cmp(&uv(h[b])[0])).unwrap_or(0);
            let m = uv(h[mi]);
            let all_edges: Vec<([f64; 2], [f64; 2])> = poly
                .iter()
                .enumerate()
                .map(|(i, p)| (uv(*p), uv(poly[(i + 1) % poly.len()])))
                .chain(
                    holes
                        .iter()
                        .flat_map(|o| (0..o.len()).map(move |i| (o[i], o[(i + 1) % o.len()])))
                        .map(|(a, b)| (uv(a), uv(b))),
                )
                .collect();
            let mut candidates: Vec<usize> = (0..poly.len()).collect();
            candidates.sort_by(|&a, &b| {
                let d = |i: usize| {
                    let q = uv(poly[i]);
                    (q[0] - m[0]).hypot(q[1] - m[1])
                };
                d(a).total_cmp(&d(b))
            });
            let visible = candidates.into_iter().find(|&pi| {
                let p = uv(poly[pi]);
                p[0] >= m[0] - 1.0e-9
                    && all_edges.iter().all(|(a, b)| {
                        let touches = |q: [f64; 2]| {
                            (q[0] - p[0]).abs() < 1.0e-9 && (q[1] - p[1]).abs() < 1.0e-9
                                || (q[0] - m[0]).abs() < 1.0e-9 && (q[1] - m[1]).abs() < 1.0e-9
                        };
                        touches(*a) || touches(*b) || !segments_cross(m, p, *a, *b)
                    })
            });
            let pi = visible.unwrap_or(0);
            let mut merged: Vec<V3> = poly[..=pi].to_vec();
            merged.extend((0..=h.len()).map(|k| h[(mi + k) % h.len()]));
            merged.push(poly[pi]);
            merged.extend_from_slice(&poly[pi + 1..]);
            poly = merged;
        }
        // Ear clipping.
        let mut idx: Vec<usize> = (0..poly.len()).collect();
        let q: Vec<[f64; 2]> = poly.iter().map(|p| uv(*p)).collect();
        let mut guard = 0;
        while idx.len() > 3 && guard < 100_000 {
            guard += 1;
            let n = idx.len();
            let mut clipped = false;
            for i in 0..n {
                let (ia, ib, ic) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
                let (a, b, c) = (q[ia], q[ib], q[ic]);
                let turn = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                if turn <= 1.0e-12 {
                    continue;
                }
                let same = |p: [f64; 2], o: [f64; 2]| (p[0] - o[0]).abs() < 1.0e-9 && (p[1] - o[1]).abs() < 1.0e-9;
                let inside = idx.iter().any(|&j| {
                    let p = q[j];
                    if same(p, a) || same(p, b) || same(p, c) {
                        return false;
                    }
                    let s = |x: [f64; 2], y: [f64; 2]| (y[0] - x[0]) * (p[1] - x[1]) - (y[1] - x[1]) * (p[0] - x[0]);
                    s(a, b) >= 0.0 && s(b, c) >= 0.0 && s(c, a) >= 0.0
                });
                if inside {
                    continue;
                }
                self.triangle(poly[ia], poly[ib], poly[ic]);
                idx.remove(i);
                clipped = true;
                break;
            }
            if !clipped {
                break;
            }
        }
        if idx.len() == 3 {
            self.triangle(poly[idx[0]], poly[idx[1]], poly[idx[2]]);
        }
    }

    fn obj(&self) -> String {
        let mut out = String::from("# CosmicDMS demo part (cargo xtask demo), millimetres, Z up\n");
        for v in &self.vertices {
            let _ = writeln!(out, "v {:.6} {:.6} {:.6}", v[0], v[1], v[2]);
        }
        for t in &self.triangles {
            let _ = writeln!(out, "f {} {} {}", t[0] + 1, t[1] + 1, t[2] + 1);
        }
        out
    }

    /// Every edge shared by exactly two triangles, running opposite ways.
    fn closed(&self) -> Result<(), String> {
        let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
        for t in &self.triangles {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edges.entry((a, b)).or_insert(0) += 1;
            }
        }
        for (&(a, b), &n) in &edges {
            if n != 1 || edges.get(&(b, a)).copied() != Some(1) {
                let (pa, pb) = (self.vertices[a as usize], self.vertices[b as usize]);
                return Err(format!(
                    "edge {pa:?} - {pb:?} is not shared properly ({n} one way, {:?} the other)",
                    edges.get(&(b, a))
                ));
            }
        }
        Ok(())
    }
}

fn segments_cross(p1: [f64; 2], p2: [f64; 2], q1: [f64; 2], q2: [f64; 2]) -> bool {
    let o = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    let (d1, d2, d3, d4) = (o(q1, q2, p1), o(q1, q2, p2), o(p1, p2, q1), o(p1, p2, q2));
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

/// `n` points round a circle of `radius` about `centre` in the plane of unit `u`, `v`.
fn circle(centre: V3, u: V3, v: V3, radius: f64, n: usize) -> Vec<V3> {
    (0..n)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / n as f64;
            add(centre, add(mul(u, radius * a.cos()), mul(v, radius * a.sin())))
        })
        .collect()
}

/// A hole's wall from ring `top` down to ring `bottom` (both in the same order), facing in, and
/// its floor facing `up`.
fn hole(m: &mut MeshBuilder, top: &[V3], bottom: &[V3], up: V3) {
    let n = top.len();
    for k in 0..n {
        let (a, b) = (top[k], top[(k + 1) % n]);
        let (c, d) = (bottom[(k + 1) % n], bottom[k]);
        // Facing the axis.
        m.quad(a, b, c, d);
    }
    m.face(up, bottom, &[]);
}

/// The demo part.
fn part() -> Result<MeshBuilder, String> {
    let mut m = MeshBuilder::default();
    let (x0, x1, y0, y1, zb) = (-70.0, 70.0, -45.0, 45.0, 15.0);
    let p = |x: f64, y: f64, z: f64| [x, y, z];
    // Base plate.
    m.face([0.0, 0.0, -1.0], &[p(x0, y0, 0.0), p(x1, y0, 0.0), p(x1, y1, 0.0), p(x0, y1, 0.0)], &[]);
    m.face([0.0, -1.0, 0.0], &[p(x0, y0, 0.0), p(x1, y0, 0.0), p(x1, y0, zb), p(x0, y0, zb)], &[]);
    m.face([0.0, 1.0, 0.0], &[p(x0, y1, 0.0), p(x1, y1, 0.0), p(x1, y1, zb), p(x0, y1, zb)], &[]);
    m.face([-1.0, 0.0, 0.0], &[p(x0, y0, 0.0), p(x0, y1, 0.0), p(x0, y1, zb), p(x0, y0, zb)], &[]);
    m.face([1.0, 0.0, 0.0], &[p(x1, y0, 0.0), p(x1, y1, 0.0), p(x1, y1, zb), p(x1, y0, zb)], &[]);
    // The block: x -10..60, y -30..30, z 15..45, its +X end a 45° slope from (40, 45) to (60, 25).
    let (bx0, bx1, by0, by1, bz) = (-10.0, 60.0, -30.0, 30.0, 45.0);
    let (sx, sz) = (40.0, 25.0);
    // The dome: a spherical cap on the base, Ø44 at its foot, 14 high, centred at (-40, 0).
    let (dc, da, dh) = ([-40.0, 0.0], 22.0, 14.0);
    let ring = 64;
    let x = [1.0, 0.0, 0.0];
    let y = [0.0, 1.0, 0.0];
    let dome_foot = circle(p(dc[0], dc[1], zb), x, y, da, ring);
    // Base top: around the block's foot and the dome's.
    m.face(
        [0.0, 0.0, 1.0],
        &[p(x0, y0, zb), p(x1, y0, zb), p(x1, y1, zb), p(x0, y1, zb)],
        &[vec![p(bx0, by0, zb), p(bx1, by0, zb), p(bx1, by1, zb), p(bx0, by1, zb)], dome_foot.clone()],
    );
    // Block sides.
    for (yy, n) in [(by0, [0.0, -1.0, 0.0]), (by1, [0.0, 1.0, 0.0])] {
        m.face(n, &[p(bx0, yy, zb), p(bx1, yy, zb), p(bx1, yy, sz), p(sx, yy, bz), p(bx0, yy, bz)], &[]);
    }
    m.face([-1.0, 0.0, 0.0], &[p(bx0, by0, zb), p(bx0, by1, zb), p(bx0, by1, bz), p(bx0, by0, bz)], &[]);
    m.face([1.0, 0.0, 0.0], &[p(bx1, by0, zb), p(bx1, by1, zb), p(bx1, by1, sz), p(bx1, by0, sz)], &[]);
    // The slope, with a Ø5 hole 10 deep square to it at its middle.
    let n_slope = norm([1.0, 0.0, 1.0]);
    let down_slope = norm([1.0, 0.0, -1.0]);
    let mid = p((sx + bx1) / 2.0, 0.0, (sz + bz) / 2.0);
    let angled_top = circle(mid, down_slope, y, 2.5, 32);
    let angled_bottom: Vec<V3> = angled_top.iter().map(|q| sub(*q, mul(n_slope, 10.0))).collect();
    m.face(
        n_slope,
        &[p(sx, by0, bz), p(bx1, by0, sz), p(bx1, by1, sz), p(sx, by1, bz)],
        std::slice::from_ref(&angled_top),
    );
    hole(&mut m, &angled_top, &angled_bottom, n_slope);
    // The block top, with the Ø24 bore 20 deep.
    let bore_at = (15.0, 0.0);
    let bore_top = circle(p(bore_at.0, bore_at.1, bz), x, y, 12.0, ring);
    let bore_bottom = circle(p(bore_at.0, bore_at.1, bz - 20.0), x, y, 12.0, ring);
    m.face(
        [0.0, 0.0, 1.0],
        &[p(bx0, by0, bz), p(sx, by0, bz), p(sx, by1, bz), p(bx0, by1, bz)],
        std::slice::from_ref(&bore_top),
    );
    hole(&mut m, &bore_top, &bore_bottom, [0.0, 0.0, 1.0]);
    // The dome: rings from its foot up to a pole.
    let rs = (da * da + dh * dh) / (2.0 * dh);
    let zc = zb + dh - rs;
    let foot_angle = ((zb - zc) / rs).acos();
    let bands = 24;
    let mut rings: Vec<Vec<V3>> = vec![dome_foot];
    for k in 1..bands {
        let a = foot_angle * (1.0 - k as f64 / bands as f64);
        let r = rs * a.sin();
        rings.push(circle(p(dc[0], dc[1], zc + rs * a.cos()), x, y, r, ring));
    }
    let pole = p(dc[0], dc[1], zc + rs);
    for w in rings.windows(2) {
        let (lo, hi) = (&w[0], &w[1]);
        for k in 0..ring {
            m.quad(lo[k], lo[(k + 1) % ring], hi[(k + 1) % ring], hi[k]);
        }
    }
    if let Some(top) = rings.last() {
        for k in 0..ring {
            m.triangle(top[k], top[(k + 1) % ring], pole);
        }
    }
    m.closed()?;
    Ok(m)
}

fn run(s: &mut Session, command: &str, params: Value) -> Result<Value, String> {
    s.execute(command, &params).map_err(|e| format!("{command}: {e}"))
}

/// The face of `model.1` whose kind and centre fit `pick`.
fn face(s: &mut Session, pick: impl Fn(&Value) -> bool) -> Result<String, String> {
    let faces = run(s, "model.faces.list", json!({ "id": "model.1" }))?;
    faces
        .as_array()
        .into_iter()
        .flatten()
        .find(|f| pick(f))
        .and_then(|f| f["face"].as_str().or(f["id"].as_str()).map(str::to_owned))
        .ok_or_else(|| {
            let seen: Vec<String> = faces
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| format!("{} {} r{} c{}", f["face"], f["kind"], f["radius"], f["centroid"]))
                .collect();
            format!("a demo face was not found among: {}", seen.join("; "))
        })
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

pub fn run_demo() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    std::env::set_current_dir(&root).map_err(|e| e.to_string())?;
    let mesh = part()?;
    std::fs::write("samples/demo-part.obj", mesh.obj()).map_err(|e| e.to_string())?;
    println!("    wrote samples/demo-part.obj ({} triangles)", mesh.triangles.len());

    let mut s = Session::new();
    run(&mut s, "model.import", json!({ "path": "samples/demo-part.obj", "units": "mm" }))?;
    run(&mut s, "model.rename", json!({ "id": "model.1", "name": "Demo part" }))?;
    let machine = run(
        &mut s,
        "machine.create",
        json!({ "name": CosmicDMS_engine::demo::DEMO_MACHINE, "kind": "mill_5axis", "max_spindle_rpm": 20000,
                "max_feed": 20000, "travel_x": 600, "travel_y": 500, "travel_z": 500, "tool_capacity": 40, "post": "fanuc" }),
    )?;
    let mid = machine["id"].as_str().unwrap_or_default().to_owned();
    run(&mut s, "tool.library.sample", json!({ "machine": mid }))?;
    run(&mut s, "setup.wcs.create", json!({ "name": "Top" }))?;
    run(
        &mut s,
        "setup.create",
        json!({ "name": "Op 1 - 5-axis", "machine": mid, "wcs": "wcs.1", "stock": "fit",
                "stock_margin_sides": 3, "stock_margin_top": 2 }),
    )?;
    // The work plane square to the slope, for the 3+2 holes.
    run(&mut s, "setup.wcs.create", json!({ "name": "Slope 45°", "activate": false }))?;
    let w = run(&mut s, "setup.wcs.rotate", json!({ "id": "wcs.2", "axis": "y", "degrees": 45 }))?;
    if num(&w["z_axis"][0]) < 0.0 {
        run(&mut s, "setup.wcs.rotate", json!({ "id": "wcs.2", "axis": "y", "degrees": -90 }))?;
    }
    run(&mut s, "setup.wcs.activate", json!({ "id": "wcs.1" }))?;

    let bore = face(&mut s, |f| f["kind"] == "cylinder" && (num(&f["radius"]) - 12.0).abs() < 0.1)?;
    let angled = face(&mut s, |f| f["kind"] == "cylinder" && (num(&f["radius"]) - 2.5).abs() < 0.1)?;
    let dome = face(&mut s, |f| f["kind"] != "plane" && f["kind"] != "cylinder")?;
    let bottom = face(&mut s, |f| f["kind"] == "plane" && num(&f["normal"][2]) < -0.99)?;

    let toolpaths = [
        ("toolpath.face", json!({ "name": "Face top", "tool": 21, "multiple_depths": true, "stepdown": 1 })),
        (
            "toolpath.rough",
            json!({ "name": "Rough", "tool": 1, "stepdown": 3, "stepover": 4.8, "stock_radial": 0.5, "stock_axial": 0.3 }),
        ),
        ("toolpath.waterline", json!({ "name": "Finish walls", "tool": 5, "stepdown": 1 })),
        ("toolpath.bore", json!({ "name": "Helical bore Ø24", "tool": 1, "geometry": [bore], "pitch": 1.5 })),
        (
            "toolpath.multiaxis",
            json!({ "name": "Dome 5-axis finish", "tool": 3, "geometry": [dome], "boundary": "selection",
                                        "boundary_offset": 2, "stepover": 0.6, "tool_axis": "normal", "lead_angle": 10,
                                        "max_tilt": 40, "angle": 90 }),
        ),
        (
            "toolpath.drill",
            json!({ "name": "Spot angled hole (3+2)", "tool": 27, "geometry": [angled], "orientation": "wcs.2",
                                     "bottom_from": "selection", "bottom_offset": 7.5, "cycle": "spot" }),
        ),
        (
            "toolpath.drill",
            json!({ "name": "Drill angled hole (3+2)", "tool": 25, "geometry": [angled], "orientation": "wcs.2",
                                     "cycle": "peck", "peck_depth": 2.5 }),
        ),
        (
            "toolpath.contour",
            json!({ "name": "Profile", "tool": 1, "geometry": [bottom], "multiple_depths": true, "max_stepdown": 5 }),
        ),
    ];
    for (command, params) in toolpaths {
        let tp = run(&mut s, command, params)?;
        println!("    {:<26} {:>6} moves  {:?}", tp["name"].as_str().unwrap_or_default(), tp["moves"], tp["warnings"]);
    }
    // The whole program, checked.
    let g = run(&mut s, "sim.gouge.check", json!({}))?;
    let c = run(&mut s, "sim.collision.check", json!({}))?;
    println!("    simulation: {} gouges, {} collisions", g["gouges"], c["collisions"]);
    if g["gouges"].as_u64() != Some(0) {
        return Err(format!("the demo gouges the part: {g}"));
    }
    run(&mut s, "sim.stop", json!({}))?;
    let program = run(&mut s, "post.gcode.get", json!({}))?;
    println!("    posts with {}: {} lines", program["post"], program["lines"]);
    run(&mut s, "view.fit", json!({}))?;
    run(&mut s, "job.save.as", json!({ "path": "samples/demo.cosmic" }))?;
    println!("    wrote samples/demo.cosmic");
    run(&mut s, "tool.library.export", json!({ "path": "samples/tools.json", "machine": mid }))?;
    println!("    wrote samples/tools.json");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_demo_part_is_closed() {
        let m = super::part();
        assert!(m.is_ok(), "{:?}", m.err());
    }
}
