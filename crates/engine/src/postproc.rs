//! Post-processing: machine-neutral cutter location (CL) data and the Python posts that turn it
//! into each machine's G-code.
//!
//! CosmicDMS builds the CL data (moves in each operation's WCS, arcs fitted within a tolerance,
//! drilling cycles, tools, spindle and coolant, the machine's limits) and runs the chosen post
//! as its own Python process, fed the CL data as JSON. A missing interpreter, a broken post or a
//! post that hangs is reported as an error; it never takes the app down. The posts shipped with
//! CosmicDMS are copied to `<data folder>/posts/builtin/`; posts in `<data folder>/posts/` are
//! the user's own (copy a built-in one there to change it).

use std::path::{Path, PathBuf};

use CosmicDMS_geom::arcfit::{self, ArcLimits, Segment};
use glam::DVec3;
use serde_json::{Value, json};

use crate::error::{EngineError, EngineResult};
use crate::fiveaxis::{self, Frame};
use crate::session::Session;
use crate::toolpath::{MoveKind, Toolpath};

/// The CL data's format tag.
pub const CL_FORMAT: &str = "CosmicDMS-CL/1";

/// Files of the posts shipped with CosmicDMS: (file name, contents).
pub const BUILTIN_FILES: &[(&str, &str)] = &[
    ("cosmicdms_post.py", include_str!("../../../posts/cosmicdms_post.py")),
    ("run_post.py", include_str!("../../../posts/run_post.py")),
    ("fanuc.py", include_str!("../../../posts/fanuc.py")),
    ("haas.py", include_str!("../../../posts/haas.py")),
    ("linuxcnc.py", include_str!("../../../posts/linuxcnc.py")),
    ("grbl.py", include_str!("../../../posts/grbl.py")),
    ("mach.py", include_str!("../../../posts/mach.py")),
];

/// The post that writes generic G-code in Rust, without Python.
pub const GENERIC: &str = "generic";

/// Longest a post may run.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// Largest answer read from a post.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
const MAX_OUTPUT: u64 = 512 * 1024 * 1024;

fn bad(m: impl Into<String>) -> EngineError {
    EngineError::InvalidParams(m.into())
}

/// Environment variable `name`, else the same under the application's former name
/// (`CosmicCAM_…`).
fn env_var(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).or_else(|| std::env::var_os(name.replacen("CosmicDMS_", "CosmicCAM_", 1)))
}

/// The folder posts live in: `CosmicDMS_POSTS_DIR`, else `<data folder>/posts`.
pub fn posts_dir() -> Option<PathBuf> {
    if let Some(dir) = env_var("CosmicDMS_POSTS_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".local/share")))
    };
    base.map(|b| b.join("CosmicDMS").join("posts"))
}

/// Copies the shipped posts to `<dir>/builtin/` (rewriting any that differ) and returns that folder.
pub fn install_builtin(dir: &Path) -> EngineResult<PathBuf> {
    let builtin = dir.join("builtin");
    std::fs::create_dir_all(&builtin).map_err(|e| bad(format!("could not create {}: {e}", builtin.display())))?;
    for (name, text) in BUILTIN_FILES {
        let path = builtin.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(*text) {
            std::fs::write(&path, text).map_err(|e| bad(format!("could not write {}: {e}", path.display())))?;
        }
    }
    Ok(builtin)
}

/// One post that can be chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct PostEntry {
    /// What machines and commands call it: the module's name, or `generic`.
    pub id: String,
    pub description: String,
    pub builtin: bool,
    pub path: Option<PathBuf>,
}

/// The first line of a module's docstring.
fn docstring(text: &str) -> String {
    let body = text.trim_start();
    let body = body.strip_prefix("\"\"\"").or_else(|| body.strip_prefix("'''")).unwrap_or("");
    body.lines().next().unwrap_or("").trim().trim_end_matches("\"\"\"").to_owned()
}

/// The posts that can be chosen: the generic one, the shipped ones and the user's own (which
/// replace shipped ones of the same name).
pub fn available() -> Vec<PostEntry> {
    available_in(posts_dir().as_deref())
}

/// [`available`] with posts in `dir`.
pub fn available_in(dir: Option<&Path>) -> Vec<PostEntry> {
    let mut out = vec![PostEntry {
        id: GENERIC.into(),
        description: "Generic G-code written by CosmicDMS itself (no Python needed)".into(),
        builtin: true,
        path: None,
    }];
    for (name, text) in BUILTIN_FILES.iter().filter(|(n, _)| is_post_file(n)) {
        out.push(PostEntry {
            id: name.trim_end_matches(".py").to_owned(),
            description: docstring(text),
            builtin: true,
            path: dir.map(|d| d.join("builtin").join(name)),
        });
    }
    if let Some(entries) = dir.and_then(|d| std::fs::read_dir(d).ok()) {
        let mut user: Vec<PostEntry> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "py"))
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(is_post_file))
            .take(500)
            .map(|p| {
                let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                PostEntry { id, description: docstring(&text), builtin: false, path: Some(p) }
            })
            .collect();
        user.sort_by(|a, b| a.id.cmp(&b.id));
        for u in user {
            out.retain(|e| e.id != u.id);
            out.push(u);
        }
    }
    out
}

fn is_post_file(name: &str) -> bool {
    name.ends_with(".py") && !name.starts_with('_') && name != "cosmicdms_post.py" && name != "run_post.py"
}

/// The post a command means: `requested`, else the machine's, else the generic one.
pub fn resolve(s: &Session, requested: Option<&str>, machine: &str) -> String {
    let from_machine = s.library.machine(machine).map(|m| m.post.trim().to_owned()).filter(|p| !p.is_empty());
    requested.map(str::to_owned).or(from_machine).unwrap_or_else(|| GENERIC.into())
}

// ---- CL data ---------------------------------------------------------------------------

/// How the CL data is built.
#[derive(Clone, Debug)]
pub struct ClOptions {
    pub units: &'static str,
    pub program_number: u32,
    pub line_numbers: bool,
    /// Fit arcs within this tolerance (mm); None writes lines only.
    pub arc_tolerance: Option<f64>,
}

fn round(v: f64) -> f64 {
    (v * 1.0e6).round() / 1.0e6
}

fn point(p: DVec3) -> Value {
    json!([round(p.x), round(p.y), round(p.z)])
}

/// The toolpaths `ids` names (all when empty), in order.
pub fn chosen<'a>(s: &'a Session, ids: &[String]) -> EngineResult<Vec<&'a Toolpath>> {
    let toolpaths: Vec<&Toolpath> = if ids.is_empty() {
        s.job.toolpaths.iter().collect()
    } else {
        ids.iter()
            .map(|id| s.job.toolpaths.iter().find(|t| &t.id == id).ok_or_else(|| bad(format!("no toolpath `{id}`"))))
            .collect::<EngineResult<_>>()?
    };
    if toolpaths.is_empty() {
        return Err(bad("there are no toolpaths to post; create one on the Toolpaths tab"));
    }
    Ok(toolpaths)
}

/// The CL data for `toolpaths`: what a post needs to write the program.
pub fn cl_data(s: &Session, toolpaths: &[&Toolpath], opt: &ClOptions) -> EngineResult<Value> {
    let first = toolpaths.first().ok_or_else(|| bad("there are no toolpaths to post"))?;
    let machine = s
        .library
        .machine(&first.machine)
        .ok_or_else(|| bad(format!("{} uses a machine that no longer exists", first.name)))?;
    let mut tools: Vec<Value> = Vec::new();
    let mut operations: Vec<Value> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    // The rotary axes' position, carried from operation to operation.
    let mut angles = [0.0f64; 2];
    let split_tolerance = opt.arc_tolerance.unwrap_or(0.01).max(1.0e-4);
    for tp in toolpaths {
        if tp.machine != first.machine {
            return Err(bad("all the toolpaths in one program must be for the same machine"));
        }
        let tool = machine
            .tool(tp.tool)
            .ok_or_else(|| bad(format!("{} uses T{}, which is no longer in the library", tp.name, tp.tool)))?;
        if !tools.iter().any(|t| t["number"] == json!(tool.number)) {
            tools.push(tool.to_json(&machine.id));
        }
        let (frame, wcs) = match &tp.wcs {
            Some(id) => {
                let index = s.job.wcs.iter().position(|w| &w.id == id).ok_or_else(|| {
                    bad(format!("{} is programmed in {id}, which no longer exists; regenerate it", tp.name))
                })?;
                let w = s.job.wcs.get(index).ok_or_else(|| EngineError::Internal("WCS lookup".into()))?;
                (
                    Frame {
                        origin: w.origin.as_dvec3(),
                        x: w.x_axis.as_dvec3(),
                        y: w.y_axis().as_dvec3(),
                        z: w.z_axis.as_dvec3(),
                    },
                    json!({ "id": w.id, "name": w.name, "index": index }),
                )
            }
            None => (
                Frame { origin: DVec3::ZERO, x: DVec3::X, y: DVec3::Y, z: DVec3::Z },
                json!({ "id": null, "name": "world", "index": 0 }),
            ),
        };
        let rpm = toolpath_rpm(tp, tool);
        let coolant = tp.params.get("coolant").and_then(Value::as_str).unwrap_or("flood");
        let mut start = frame.local(tp.start);
        let axis = frame.local_dir(tp.axis);
        // (Axes are kept in single precision: anything within 2 millidegrees is along Z.)
        let tilted = axis.distance(DVec3::Z) > 3.0e-5;
        let rotary = machine.kinematics.as_ref().map(|k| fiveaxis::Rotary::new(k, tool.overall_length));
        let mut extra = serde_json::Map::new();
        let moves = match &rotary {
            None if tilted || tp.multi_axis() => {
                return Err(bad(format!(
                    "{} tilts the tool, which needs a 4- or 5-axis machine; {} is a 3-axis mill",
                    tp.name, machine.name
                )));
            }
            None => cl_moves(&frame, start, tp, opt.arc_tolerance),
            Some(r) if tp.multi_axis() => {
                let (moves, first) = fiveaxis::multi_moves(tp, &frame, r, &mut angles, split_tolerance)?;
                extra.insert("rotary".into(), r.degrees(first));
                extra.insert("machine_start".into(), point(r.machine(start, first)));
                moves
            }
            Some(r) if tilted => {
                // 3+2: the moves (arcs fitted) in the tilted work plane, and as lines in the
                // WCS and in machine positions for posts without a tilted plane.
                let plane_world = tp
                    .params
                    .get("orientation")
                    .and_then(Value::as_str)
                    .and_then(|id| s.job.wcs_by_id(id))
                    .map(|w| Frame {
                        origin: w.origin.as_dvec3(),
                        x: w.x_axis.as_dvec3(),
                        y: w.y_axis().as_dvec3(),
                        z: w.z_axis.as_dvec3(),
                    })
                    .unwrap_or_else(|| Frame { origin: frame.origin, ..Frame::along(tp.axis.as_dvec3()) });
                let plane = frame.relative(&plane_world);
                angles = r.solve(axis, angles, &tp.name)?;
                extra.insert("rotary".into(), r.degrees(angles));
                extra.insert("work_plane".into(), fiveaxis::plane_json(&plane));
                extra.insert("move_frame".into(), json!("plane"));
                extra.insert("wcs_start".into(), point(start));
                extra.insert("machine_start".into(), point(r.machine(start, angles)));
                extra.insert("wcs_moves".into(), json!(fiveaxis::plain_moves(tp, &frame, None)));
                extra.insert("machine_moves".into(), json!(fiveaxis::plain_moves(tp, &frame, Some((r, angles)))));
                if let Some(d) = tp.drill.as_ref().filter(|_| r.spindle_vertical(angles)) {
                    let to_wcs = |x: f64, y: f64, z: f64| plane.origin + plane.x * x + plane.y * y + plane.z * z;
                    let m = |x: f64, y: f64, z: f64| r.machine(to_wcs(x, y, z), angles);
                    let first = d.holes.first().copied().unwrap_or_default();
                    extra.insert(
                        "machine_drill".into(),
                        json!({
                            "holes": d.holes.iter().map(|h| {
                                let (top, bottom) = (m(h[0], h[1], h[2]), m(h[0], h[1], h[3]));
                                [round(top.x), round(top.y), round(top.z), round(bottom.z)]
                            }).collect::<Vec<_>>(),
                            "retract": round(m(first[0], first[1], d.retract).z),
                            "clearance": round(m(first[0], first[1], d.clearance).z),
                        }),
                    );
                }
                let moves = cl_moves(&plane_world, plane_world.local(tp.start), tp, opt.arc_tolerance);
                start = plane_world.local(tp.start);
                moves
            }
            Some(r) => {
                // Along the WCS's Z: the rotary axes go to zero (or wherever points the tool
                // down, for an axis that does not matter then).
                angles = r.solve(DVec3::Z, [0.0, 0.0], &tp.name)?;
                extra.insert("rotary".into(), r.degrees(angles));
                if angles.iter().any(|a| a.abs() > 1.0e-9) {
                    extra.insert("machine_start".into(), point(r.machine(start, angles)));
                    extra.insert("machine_moves".into(), json!(fiveaxis::plain_moves(tp, &frame, Some((r, angles)))));
                }
                cl_moves(&frame, start, tp, opt.arc_tolerance)
            }
        };
        let wcs_start = frame.local(tp.start);
        let (mut lo, mut hi) = (wcs_start, wcs_start);
        for m in tp.moves.iter() {
            let l = frame.local(m.to);
            lo = lo.min(l);
            hi = hi.max(l);
        }
        let size = hi - lo;
        for (axis, extent, travel) in
            [("X", size.x, machine.travel[0]), ("Y", size.y, machine.travel[1]), ("Z", size.z, machine.travel[2])]
        {
            if travel > 0.0 && extent > travel + 1.0e-6 {
                warnings.push(format!(
                    "{} spans {extent:.1} mm in {axis}, beyond the machine's {travel:.0} mm travel",
                    tp.name
                ));
            }
        }
        if rpm > machine.max_spindle_rpm && machine.max_spindle_rpm > 0.0 {
            warnings.push(format!("{}: {rpm:.0} rpm is above the machine's {:.0}", tp.name, machine.max_spindle_rpm));
        }
        if tp.moves.iter().any(|m| f64::from(m.feed) > machine.max_feed && machine.max_feed > 0.0) {
            warnings.push(format!("{} feeds faster than the machine's {:.0} mm/min", tp.name, machine.max_feed));
        }
        let drill = tp.drill.as_ref().map(|d| {
            json!({
                "cycle": d.cycle,
                "holes": d.holes.iter().map(|h| h.map(round)).collect::<Vec<_>>(),
                "retract": round(d.retract),
                "clearance": round(d.clearance),
                "peck": round(d.peck),
                "dwell": d.dwell,
                "feed": d.feed,
                "pitch": d.pitch,
            })
        });
        let text =
            |key: &str| tp.params.get(key).and_then(Value::as_str).unwrap_or("").chars().take(4096).collect::<String>();
        let num = |key: &str| tp.params.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        let mut op = json!({
            "id": tp.id,
            "name": tp.name,
            "strategy": tp.strategy,
            "notes": text("post_comment"),
            "stop_before": tp.params.get("post_stop").and_then(Value::as_bool).unwrap_or(false),
            "code_before": text("post_code_before"),
            "code_after": text("post_code_after"),
            "stock_to_leave": [num("stock_radial"), num("stock_axial")],
            "tolerance": tp.params.get("tolerance").and_then(Value::as_f64).unwrap_or(0.01),
            "tool": tp.tool,
            "spindle_rpm": rpm,
            "coolant": coolant,
            "wcs": wcs,
            "work_offset": wcs["index"],
            "start": point(start),
            "moves": moves,
            "drill": drill,
            "bounds": { "min": point(lo), "max": point(hi) },
            "tool_axis": point(axis),
            "multi_axis": tp.multi_axis(),
            "move_frame": "wcs",
            "work_plane": null,
            "rotary": null,
        });
        if let Some(o) = op.as_object_mut() {
            o.extend(extra);
        }
        operations.push(op);
    }
    warnings.dedup();
    Ok(json!({
        "format": CL_FORMAT,
        "job": {
            "name": s.job.name,
            "units": opt.units,
            "program_number": opt.program_number,
            "line_numbers": opt.line_numbers,
        },
        "machine": {
            "id": machine.id,
            "name": machine.name,
            "travel": machine.travel,
            "max_spindle_rpm": machine.max_spindle_rpm,
            "max_feed": machine.max_feed,
            "tool_capacity": machine.tool_capacity,
            "kinematics": machine.kinematics.as_ref().map(fiveaxis::kinematics_json),
        },
        "tools": tools,
        "operations": operations,
        "properties": machine.post_properties,
        "warnings": warnings,
    }))
}

/// The spindle speed a toolpath runs at: its own, else its preset's, else the tool's.
pub fn toolpath_rpm(tp: &Toolpath, tool: &crate::library::Tool) -> f64 {
    let own = tp.params.get("spindle_rpm").and_then(Value::as_f64).filter(|v| *v > 0.0);
    let preset = tp
        .params
        .get("preset")
        .and_then(Value::as_str)
        .and_then(|name| tool.cutting(Some(name)).ok())
        .map(|c| c.spindle_rpm)
        .filter(|v| *v > 0.0);
    own.or(preset).unwrap_or(tool.spindle_rpm)
}

/// A toolpath's moves as CL moves: rapids, lines and (within `tolerance`) arcs.
fn cl_moves(frame: &Frame, start: DVec3, tp: &Toolpath, tolerance: Option<f64>) -> Vec<Value> {
    let mut out = Vec::with_capacity(tp.moves.len());
    let mut at = start;
    let mut run: Vec<DVec3> = Vec::new();
    let mut run_start = start;
    let mut run_feed = 0.0f64;
    let limits = tolerance.map(|t| ArcLimits { tolerance: t.max(1.0e-4), ..ArcLimits::default() });
    let flush = |out: &mut Vec<Value>, run: &mut Vec<DVec3>, from: DVec3, feed: f64| {
        if run.is_empty() {
            return;
        }
        let segments = match &limits {
            Some(l) => arcfit::fit(from, run, l),
            None => run.iter().map(|p| Segment::Line(*p)).collect(),
        };
        for seg in segments {
            out.push(match seg {
                Segment::Line(p) => json!({ "type": "linear", "to": point(p), "feed": feed }),
                Segment::Arc { to, center, cw } => json!({
                    "type": "arc",
                    "to": point(to),
                    "center": [round(center.x), round(center.y)],
                    "cw": cw,
                    "plane": "xy",
                    "feed": feed,
                }),
            });
        }
        run.clear();
    };
    for m in tp.moves.iter() {
        let to = frame.local(m.to);
        let feed = f64::from(m.feed);
        if m.kind == MoveKind::Rapid {
            flush(&mut out, &mut run, run_start, run_feed);
            out.push(json!({ "type": "rapid", "to": point(to) }));
        } else {
            // Feed moves at one feed are fitted together; a new feed starts a new run.
            if !run.is_empty() && (feed - run_feed).abs() > 1.0e-6 {
                flush(&mut out, &mut run, run_start, run_feed);
            }
            if run.is_empty() {
                run_start = at;
                run_feed = feed;
            }
            run.push(to);
        }
        at = to;
    }
    flush(&mut out, &mut run, run_start, run_feed);
    out
}

// ---- running posts ---------------------------------------------------------------------

/// What a post wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct PostOutput {
    pub gcode: String,
    pub warnings: Vec<String>,
    pub extension: String,
    pub name: String,
}

/// The Python interpreter: `CosmicDMS_PYTHON`, else `python3`, else `python`.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn python() -> EngineResult<String> {
    if let Some(p) = env_var("CosmicDMS_PYTHON") {
        return Ok(p.to_string_lossy().into_owned());
    }
    for candidate in ["python3", "python"] {
        let ok = std::process::Command::new(candidate)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return Ok(candidate.into());
        }
    }
    Err(EngineError::NotSupported(
        "posting with a Python post: Python 3 was not found (install it, or set CosmicDMS_PYTHON); the generic post needs no Python".into(),
    ))
}

/// Runs `post` (an id from [`available`]) on `request` and returns its JSON answer.
#[cfg(target_arch = "wasm32")]
fn run_python(_post: &Path, _builtin: &Path, _request: &Value) -> EngineResult<Value> {
    Err(EngineError::NotSupported("Python posts in the browser; use the generic post or the desktop app".into()))
}

#[cfg(not(target_arch = "wasm32"))]
fn run_python(post: &Path, builtin: &Path, request: &Value) -> EngineResult<Value> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let interpreter = python()?;
    let runner = builtin.join("run_post.py");
    let mut child = Command::new(&interpreter)
        .args(["-I", "-B"])
        .arg(&runner)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| bad(format!("could not start {interpreter}: {e}")))?;
    let mut request = request.clone();
    if let Some(obj) = request.as_object_mut() {
        obj.insert("post".into(), json!(post.display().to_string()));
    }
    let payload = request.to_string();
    let stdin = child.stdin.take();
    let writer = std::thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(payload.as_bytes());
        }
    });
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(out) = stdout {
            let _ = out.take(MAX_OUTPUT).read_to_string(&mut text);
        }
        text
    });
    let stderr = child.stderr.take();
    let err_reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(err) = stderr {
            let _ = err.take(64 * 1024).read_to_string(&mut text);
        }
        text
    });
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(bad(format!("the post ran for more than {} s and was stopped", TIMEOUT.as_secs())));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(e) => return Err(bad(format!("the post could not be waited for: {e}"))),
        }
    }
    let _ = writer.join();
    let text = reader.join().unwrap_or_default();
    let errors = err_reader.join().unwrap_or_default();
    let answer: Value = serde_json::from_str(&text).map_err(|_| {
        let detail = errors.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
        bad(format!("the post gave no answer{}", if detail.is_empty() { String::new() } else { format!(": {detail}") }))
    })?;
    if let Some(e) = answer.get("error").and_then(Value::as_str) {
        return Err(bad(format!("post: {e}")));
    }
    Ok(answer)
}

/// Where post `id` is in `dir`, with the shipped posts installed.
fn locate(dir: &Path, id: &str) -> EngineResult<(PathBuf, PathBuf)> {
    let builtin = install_builtin(dir)?;
    let entry = available_in(Some(dir))
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| bad(format!("there is no post `{id}`; see post.processor.list")))?;
    let path = entry.path.ok_or_else(|| bad(format!("`{id}` is not a Python post")))?;
    Ok((path, builtin))
}

/// The settings post `id` offers: its `describe_properties()` list.
pub fn properties(id: &str) -> EngineResult<Vec<Value>> {
    if id == GENERIC {
        return Ok(Vec::new());
    }
    let dir = posts_dir().ok_or_else(|| bad("there is no folder for posts (no home folder)"))?;
    properties_in(&dir, id)
}

/// [`properties`] with posts in `dir`.
pub fn properties_in(dir: &Path, id: &str) -> EngineResult<Vec<Value>> {
    let (path, builtin) = locate(dir, id)?;
    let answer = run_python(&path, &builtin, &json!({ "action": "describe" }))?;
    Ok(answer.get("properties").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().take(500).collect())
}

/// Runs Python post `id` on CL data.
pub fn run(id: &str, cl: &Value) -> EngineResult<PostOutput> {
    let dir = posts_dir().ok_or_else(|| bad("there is no folder for posts (no home folder)"))?;
    run_in(&dir, id, cl)
}

/// [`run`] with posts in `dir`.
pub fn run_in(dir: &Path, id: &str, cl: &Value) -> EngineResult<PostOutput> {
    let (path, builtin) = locate(dir, id)?;
    let answer = run_python(&path, &builtin, &json!({ "action": "post", "cl": cl }))?;
    let gcode = answer.get("gcode").and_then(Value::as_str).ok_or_else(|| bad("the post wrote no program"))?.to_owned();
    Ok(PostOutput {
        gcode,
        warnings: answer
            .get("warnings")
            .and_then(Value::as_array)
            .map(|w| w.iter().filter_map(|x| x.as_str().map(str::to_owned)).take(200).collect())
            .unwrap_or_default(),
        extension: answer.get("extension").and_then(Value::as_str).unwrap_or("nc").chars().take(16).collect(),
        name: answer.get("name").and_then(Value::as_str).unwrap_or(id).chars().take(128).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docstrings_give_descriptions() {
        assert_eq!(docstring("\"\"\"Fanuc-style mill.\n\nMore.\n\"\"\""), "Fanuc-style mill.");
        assert_eq!(docstring("import os"), "");
        assert!(BUILTIN_FILES.iter().filter(|(n, _)| is_post_file(n)).count() >= 5);
    }
}
