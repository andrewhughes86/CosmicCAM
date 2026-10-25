//! Job files (`.cosmic`): one JSON document holding a job's models, WCS, stock, toolpaths and
//! the camera it was saved with. Meshes and toolpath moves are packed little-endian and
//! base64-encoded so files stay compact; face and edge recognition is re-run on load, so the
//! references toolpaths keep (`model.1/face.3`) resolve as they did when the job was saved.
//!
//! Job files are input like any other: every count is capped, every number checked.

use std::sync::Arc;

use CosmicCAM_geom::{Aabb, Mesh, Vec3};
use CosmicCAM_io::{ModelFormat, Units};
use CosmicCAM_render::{OrbitCamera, Projection};
use serde_json::{Value, json};

use crate::error::{EngineError, EngineResult};
use crate::library::{MachineLibrary, Tool};
use crate::session::{JobState, MAX_MODELS, MAX_SETUPS, Model, Setup, Wcs};
use crate::toolpath::{MAX_MOVES, MAX_TOOLPATHS, Move, MoveKind, Toolpath};

/// The format tag every job file carries.
pub const FORMAT: &str = "CosmicCAM/1";
/// The file extension, without the dot.
pub const EXTENSION: &str = "cosmic";
/// Largest job file read.
pub const MAX_FILE_BYTES: u64 = 1 << 30;
/// Most WCS one job file may hold.
const MAX_WCS: usize = 1024;
/// Most vertices or triangles in one saved mesh.
const MAX_MESH_ITEMS: usize = 50_000_000;
/// Longest name or text field kept from a file.
const MAX_TEXT: usize = 4096;

const KINDS: [MoveKind; 5] = [MoveKind::Rapid, MoveKind::Link, MoveKind::Plunge, MoveKind::Lead, MoveKind::Cut];

/// A job file's contents: the job, the camera it was saved with, and the tools its toolpaths
/// use (machine id, tool), so it can be posted on a machine whose library lacks them.
pub struct Loaded {
    pub job: JobState,
    pub camera: Option<OrbitCamera>,
    pub tools: Vec<(String, Tool)>,
}

/// The job and camera as the text of a job file, with the tools its toolpaths use from
/// `library`.
pub fn to_string(job: &JobState, camera: &OrbitCamera, library: &MachineLibrary) -> String {
    let mut used: Vec<(String, u32)> = Vec::new();
    for t in &job.toolpaths {
        if !used.iter().any(|(m, n)| *m == t.machine && *n == t.tool) {
            used.push((t.machine.clone(), t.tool));
        }
    }
    let tools: Vec<Value> = used
        .iter()
        .filter_map(|(m, n)| library.machine(m).and_then(|mc| mc.tool(*n)).map(|tool| tool.to_json(m)))
        .collect();
    let models: Vec<Value> = job
        .models
        .iter()
        .map(|m| {
            let mut positions = Vec::with_capacity(m.mesh.positions().len() * 12);
            for p in m.mesh.positions() {
                for c in p.to_array() {
                    positions.extend_from_slice(&c.to_le_bytes());
                }
            }
            let mut triangles = Vec::with_capacity(m.mesh.triangles().len() * 12);
            for t in m.mesh.triangles() {
                for i in t {
                    triangles.extend_from_slice(&i.to_le_bytes());
                }
            }
            json!({
                "id": m.id,
                "name": m.name,
                "source": m.source,
                "format": m.format.map(ModelFormat::id),
                "units": m.units.id(),
                "positions": base64::encode(&positions),
                "triangles": base64::encode(&triangles),
            })
        })
        .collect();
    let toolpaths: Vec<Value> = job
        .toolpaths
        .iter()
        .map(|t| {
            let mut moves = Vec::with_capacity(t.moves.len() * 17);
            for m in t.moves.iter() {
                moves.push(KINDS.iter().position(|k| *k == m.kind).unwrap_or(0) as u8);
                for c in m.to.to_array() {
                    moves.extend_from_slice(&c.to_le_bytes());
                }
                moves.extend_from_slice(&m.feed.to_le_bytes());
            }
            json!({
                "id": t.id,
                "name": t.name,
                "strategy": t.strategy,
                "machine": t.machine,
                "tool": t.tool,
                "wcs": t.wcs,
                "params": t.params,
                "start": t.start.to_array(),
                "warnings": t.warnings,
                "moves": base64::encode(&moves),
                "drill": t.drill.as_ref().map(|d| json!({
                    "cycle": d.cycle,
                    "holes": d.holes,
                    "retract": d.retract,
                    "clearance": d.clearance,
                    "peck": d.peck,
                    "dwell": d.dwell,
                    "feed": d.feed,
                    "pitch": d.pitch,
                })),
            })
        })
        .collect();
    json!({
        "format": FORMAT,
        "job": {
            "name": job.name,
            "sample": job.sample_loaded,
            "models": models,
            "next_model": job.next_model,
            "wcs": job.wcs.iter().map(|w| json!({
                "id": w.id,
                "name": w.name,
                "origin": w.origin.to_array(),
                "x_axis": w.x_axis.to_array(),
                "z_axis": w.z_axis.to_array(),
            })).collect::<Vec<_>>(),
            "active_wcs": job.active_wcs,
            "next_wcs": job.next_wcs,
            "stock": job.stock.map(|b| json!({ "min": b.min.to_array(), "max": b.max.to_array() })),
            "toolpaths": toolpaths,
            "next_toolpath": job.next_toolpath,
            "tools": tools,
            "setups": job.setups.iter().map(|st| json!({
                "id": st.id,
                "name": st.name,
                "machine": st.machine,
                "wcs": st.wcs,
                "stock": st.stock.map(|b| json!({ "min": b.min.to_array(), "max": b.max.to_array() })),
            })).collect::<Vec<_>>(),
            "active_setup": job.active_setup,
            "next_setup": job.next_setup,
        },
        "view": {
            "target": camera.target.to_array(),
            "distance": camera.distance,
            "yaw": camera.yaw,
            "pitch": camera.pitch,
            "projection": camera.projection.id(),
        },
    })
    .to_string()
}

fn bad(what: impl std::fmt::Display) -> EngineError {
    EngineError::InvalidParams(format!("not a valid job file: {what}"))
}

fn text(v: &Value, key: &str) -> EngineResult<String> {
    let s = v.get(key).and_then(Value::as_str).ok_or_else(|| bad(format!("`{key}` is missing")))?;
    Ok(s.chars().take(MAX_TEXT).collect())
}

fn opt_text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(|s| s.chars().take(MAX_TEXT).collect())
}

fn counter(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn vec3(v: &Value, key: &str) -> EngineResult<Vec3> {
    let a = v.get(key).and_then(Value::as_array).ok_or_else(|| bad(format!("`{key}` is missing")))?;
    let c: Vec<f32> = a.iter().filter_map(Value::as_f64).map(|x| x as f32).collect();
    match c.as_slice() {
        [x, y, z] if c.iter().all(|x| x.is_finite() && x.abs() < 1.0e9) => Ok(Vec3::new(*x, *y, *z)),
        _ => Err(bad(format!("`{key}` is not a point"))),
    }
}

fn list<'a>(v: &'a Value, key: &str, max: usize) -> EngineResult<&'a [Value]> {
    let a = match v.get(key) {
        None | Some(Value::Null) => return Ok(&[]),
        Some(a) => a.as_array().ok_or_else(|| bad(format!("`{key}` is not a list")))?,
    };
    if a.len() > max {
        return Err(bad(format!("more than {max} entries in `{key}`")));
    }
    Ok(a)
}

fn bytes(v: &Value, key: &str, stride: usize) -> EngineResult<Vec<u8>> {
    let b = base64::decode(v.get(key).and_then(Value::as_str).unwrap_or_default())
        .ok_or_else(|| bad(format!("`{key}` is not base64")))?;
    if b.len() % stride != 0 {
        return Err(bad(format!("`{key}` has a partial entry")));
    }
    Ok(b)
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    let mut a = [0u8; 4];
    if let Some(s) = b.get(at..at + 4) {
        a.copy_from_slice(s);
    }
    f32::from_le_bytes(a)
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    let mut a = [0u8; 4];
    if let Some(s) = b.get(at..at + 4) {
        a.copy_from_slice(s);
    }
    u32::from_le_bytes(a)
}

fn model(v: &Value) -> EngineResult<Model> {
    let name = text(v, "name")?;
    let positions = bytes(v, "positions", 12)?;
    let triangles = bytes(v, "triangles", 12)?;
    if positions.len() / 12 > MAX_MESH_ITEMS || triangles.len() / 12 > MAX_MESH_ITEMS {
        return Err(bad(format!("{name} is too large")));
    }
    let points = (0..positions.len() / 12)
        .map(|i| Vec3::new(f32_at(&positions, i * 12), f32_at(&positions, i * 12 + 4), f32_at(&positions, i * 12 + 8)));
    let tris = (0..triangles.len() / 12)
        .map(|i| [u32_at(&triangles, i * 12), u32_at(&triangles, i * 12 + 4), u32_at(&triangles, i * 12 + 8)]);
    let mesh = Mesh::new(points.collect(), tris.collect()).map_err(|e| bad(format!("{name}: {e:?}")))?;
    let (faces, edges) = Model::recognise(&mesh);
    Ok(Model {
        id: text(v, "id")?,
        name,
        source: opt_text(v, "source").unwrap_or_default(),
        format: v.get("format").and_then(Value::as_str).and_then(ModelFormat::from_id),
        units: v.get("units").and_then(Value::as_str).and_then(Units::from_id).unwrap_or(Units::Millimetres),
        mesh: Arc::new(mesh),
        faces,
        edges,
    })
}

fn toolpath(v: &Value) -> EngineResult<Toolpath> {
    let name = text(v, "name")?;
    let packed = bytes(v, "moves", 17)?;
    if packed.len() / 17 > MAX_MOVES {
        return Err(bad(format!("{name} has too many moves")));
    }
    let mut moves = Vec::with_capacity(packed.len() / 17);
    for i in 0..packed.len() / 17 {
        let at = i * 17;
        let kind = packed.get(at).and_then(|k| KINDS.get(usize::from(*k))).copied();
        let to = Vec3::new(f32_at(&packed, at + 1), f32_at(&packed, at + 5), f32_at(&packed, at + 9));
        let feed = f32_at(&packed, at + 13);
        match kind {
            Some(kind) if to.is_finite() && feed.is_finite() && feed >= 0.0 => moves.push(Move { kind, to, feed }),
            _ => return Err(bad(format!("{name} has a bad move"))),
        }
    }
    Ok(Toolpath {
        id: text(v, "id")?,
        name,
        strategy: text(v, "strategy")?,
        machine: text(v, "machine")?,
        tool: v
            .get("tool")
            .and_then(Value::as_u64)
            .and_then(|t| u32::try_from(t).ok())
            .ok_or_else(|| bad("a toolpath has no tool"))?,
        wcs: opt_text(v, "wcs"),
        params: v.get("params").cloned().unwrap_or(Value::Null),
        start: vec3(v, "start")?,
        moves: Arc::new(moves),
        warnings: list(v, "warnings", 256)?.iter().filter_map(|w| w.as_str().map(str::to_owned)).collect(),
        drill: match v.get("drill") {
            None | Some(Value::Null) => None,
            Some(d) => Some(drill_cycle(d)?),
        },
    })
}

fn drill_cycle(d: &Value) -> EngineResult<crate::toolpath::DrillCycle> {
    let num = |key: &str| -> EngineResult<f64> {
        let v = d.get(key).and_then(Value::as_f64).ok_or_else(|| bad(format!("a drill cycle has no `{key}`")))?;
        if v.is_finite() && v.abs() < 1.0e9 {
            Ok(v)
        } else {
            Err(bad(format!("a drill cycle's `{key}` is out of range")))
        }
    };
    let cycle = text(d, "cycle")?;
    if !crate::toolpath::drill::CYCLES.contains(&cycle.as_str()) {
        return Err(bad(format!("unknown drill cycle `{cycle}`")));
    }
    let holes = list(d, "holes", crate::toolpath::drill::MAX_HOLES)?
        .iter()
        .map(|h| {
            let c: Vec<f64> = h.as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
            match c.as_slice() {
                [x, y, top, bottom] if c.iter().all(|v| v.is_finite() && v.abs() < 1.0e9) => {
                    Ok([*x, *y, *top, *bottom])
                }
                _ => Err(bad("a drilled hole is not [x, y, top, bottom]")),
            }
        })
        .collect::<EngineResult<_>>()?;
    Ok(crate::toolpath::DrillCycle {
        cycle,
        holes,
        retract: num("retract")?,
        clearance: num("clearance")?,
        peck: num("peck")?,
        dwell: num("dwell")?,
        feed: num("feed")?,
        pitch: num("pitch")?,
    })
}

fn stock_box(b: &Value) -> EngineResult<Aabb> {
    let (min, max) = (vec3(b, "min")?, vec3(b, "max")?);
    if !(min.cmple(max).all()) {
        return Err(bad("the stock box is inside out"));
    }
    Ok(Aabb { min, max })
}

/// Reads a job file.
pub fn from_str(source: &str) -> EngineResult<Loaded> {
    let root: Value = serde_json::from_str(source).map_err(bad)?;
    let format = root.get("format").and_then(Value::as_str).unwrap_or_default();
    if format != FORMAT {
        return Err(if format.starts_with("CosmicCAM/") {
            EngineError::NotSupported(format!("job files of format {format}; this version reads {FORMAT}"))
        } else {
            bad("it has no CosmicCAM format tag")
        });
    }
    let j = root.get("job").ok_or_else(|| bad("`job` is missing"))?;
    let mut job = JobState {
        name: text(j, "name")?,
        sample_loaded: j.get("sample").and_then(Value::as_bool).unwrap_or(false),
        ..JobState::default()
    };
    job.models = list(j, "models", MAX_MODELS)?.iter().map(model).collect::<EngineResult<_>>()?;
    job.next_model = counter(j, "next_model");
    job.wcs = list(j, "wcs", MAX_WCS)?
        .iter()
        .map(|w| {
            let (x, z) = (vec3(w, "x_axis")?.normalize_or_zero(), vec3(w, "z_axis")?.normalize_or_zero());
            if x == Vec3::ZERO || z == Vec3::ZERO || x.dot(z).abs() > 1.0e-3 {
                return Err(bad("a WCS has bad axes"));
            }
            Ok(Wcs { id: text(w, "id")?, name: text(w, "name")?, origin: vec3(w, "origin")?, x_axis: x, z_axis: z })
        })
        .collect::<EngineResult<_>>()?;
    job.active_wcs = opt_text(j, "active_wcs").filter(|id| job.wcs.iter().any(|w| &w.id == id));
    job.next_wcs = counter(j, "next_wcs");
    job.stock = match j.get("stock") {
        None | Some(Value::Null) => None,
        Some(b) => Some(stock_box(b)?),
    };
    job.toolpaths = list(j, "toolpaths", MAX_TOOLPATHS)?.iter().map(toolpath).collect::<EngineResult<_>>()?;
    job.next_toolpath = counter(j, "next_toolpath");
    job.setups = list(j, "setups", MAX_SETUPS)?
        .iter()
        .map(|st| {
            let wcs = opt_text(st, "wcs").filter(|id| job.wcs.iter().any(|w| &w.id == id));
            Ok(Setup {
                id: text(st, "id")?,
                name: text(st, "name")?,
                machine: text(st, "machine")?,
                wcs,
                stock: match st.get("stock") {
                    None | Some(Value::Null) => None,
                    Some(b) => Some(stock_box(b)?),
                },
            })
        })
        .collect::<EngineResult<_>>()?;
    job.active_setup = opt_text(j, "active_setup").filter(|id| job.setups.iter().any(|st| &st.id == id));
    job.next_setup = counter(j, "next_setup");
    let camera = root.get("view").and_then(|v| {
        let d = OrbitCamera::default();
        Some(
            OrbitCamera {
                target: vec3(v, "target").ok()?,
                distance: v.get("distance")?.as_f64()? as f32,
                yaw: v.get("yaw")?.as_f64()? as f32,
                pitch: v.get("pitch")?.as_f64()? as f32,
                projection: v
                    .get("projection")
                    .and_then(Value::as_str)
                    .and_then(Projection::from_id)
                    .unwrap_or(d.projection),
                ..d
            }
            .sanitized(),
        )
    });
    let tools = list(j, "tools", crate::library::MAX_TOOLS)?
        .iter()
        .filter_map(|t| {
            let machine = opt_text(t, "machine")?;
            let mut tool = serde_json::from_value::<Tool>(t.clone()).ok()?;
            tool.name = crate::library::clip(&tool.name);
            tool.coating = crate::library::clip(&tool.coating);
            tool.notes = crate::library::clip(&tool.notes);
            tool.validate().ok().map(|()| (machine, tool))
        })
        .collect();
    Ok(Loaded { job, camera, tools })
}

/// Standard base64 (RFC 4648, with padding).
mod base64 {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [
                chunk.first().copied().unwrap_or(0),
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
                if i <= chunk.len() {
                    out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn value(c: u8) -> Option<u32> {
        ALPHABET.iter().position(|a| *a == c).map(|p| p as u32)
    }

    pub fn decode(text: &str) -> Option<Vec<u8>> {
        let t = text.trim_end_matches('=').as_bytes();
        let mut out = Vec::with_capacity(t.len() * 3 / 4);
        for chunk in t.chunks(4) {
            if chunk.len() == 1 {
                return None;
            }
            let mut n = 0u32;
            for (i, c) in chunk.iter().enumerate() {
                n |= value(*c)? << (18 - 6 * i as u32);
            }
            let b = n.to_be_bytes();
            out.extend_from_slice(b.get(1..chunk.len()).unwrap_or_default());
        }
        Some(out)
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn round_trips() {
            for n in 0..20u8 {
                let data: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
                assert_eq!(super::decode(&super::encode(&data)).as_deref(), Some(data.as_slice()));
            }
            assert_eq!(super::encode(b"Man"), "TWFu");
            assert_eq!(super::encode(b"Ma"), "TWE=");
            assert!(super::decode("T").is_none());
            assert!(super::decode("T*==").is_none());
        }
    }
}
