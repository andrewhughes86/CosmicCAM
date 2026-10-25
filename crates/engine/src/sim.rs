//! Material removal simulation: the job's toolpaths run in machine time against a model of the
//! stock.
//!
//! - Playback follows each move's feed (rapids at the machine's rapid rate), scaled by a speed.
//! - When every toolpath works along one tool axis the stock is a height field ([`ZMap`]) in
//!   that axis's frame: fast, and exact for flat end mills. When the tool tilts (multi-axis
//!   toolpaths, or 3+2 toolpaths along different axes) it is a voxel model ([`Voxels`]) in world
//!   coordinates, cut by the tool swept along each move with its axis.
//! - Seeking back replays from the start.
//! - Verification: gouges are cells cut below the part surface; collisions are rapids that
//!   remove material and cuts deeper than the tool's flutes (the shank would rub).

use CosmicCAM_geom::cutter::DropCutter;
use CosmicCAM_geom::voxel::{ToolShape, Voxels};
use CosmicCAM_geom::zmap::ZMap;
use CosmicCAM_geom::{Aabb, Vec3};
use glam::{DVec2, DVec3};
use serde_json::{Value, json};

use crate::error::{EngineError, EngineResult};
use crate::session::Session;
use crate::toolpath::MoveKind;

/// Most moves one simulation runs.
pub const MAX_SIM_MOVES: usize = 5_000_000;
/// Most events (collisions) kept.
const MAX_EVENTS: usize = 1_000;
/// Most gouge cells reported.
const MAX_REPORTED: usize = 50;
/// Rapid rate assumed when the machine gives none faster, mm/min.
const RAPID_RATE: f64 = 10_000.0;
/// Most cells a voxel stock gets by default (a finer `cell` may ask for up to `MAX_VOXELS`).
const DEFAULT_VOXELS: f64 = 6.0e6;
const MAX_VOXELS: usize = 24_000_000;

fn bad(m: impl Into<String>) -> EngineError {
    EngineError::InvalidParams(m.into())
}

/// The frame the simulation works in: the toolpaths' WCS (or world).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimFrame {
    pub origin: DVec3,
    pub x: DVec3,
    pub y: DVec3,
    pub z: DVec3,
}

impl SimFrame {
    pub const WORLD: Self = Self { origin: DVec3::ZERO, x: DVec3::X, y: DVec3::Y, z: DVec3::Z };

    /// A direction given in the frame, in world coordinates.
    pub fn world_dir(&self, d: DVec3) -> Vec3 {
        (self.x * d.x + self.y * d.y + self.z * d.z).normalize_or_zero().as_vec3()
    }

    fn local_dir(&self, d: Vec3) -> DVec3 {
        let d = d.as_dvec3();
        DVec3::new(d.dot(self.x), d.dot(self.y), d.dot(self.z)).normalize_or(DVec3::Z)
    }

    pub fn local(&self, p: Vec3) -> DVec3 {
        let d = p.as_dvec3() - self.origin;
        DVec3::new(d.dot(self.x), d.dot(self.y), d.dot(self.z))
    }

    pub fn world(&self, l: DVec3) -> Vec3 {
        (self.origin + self.x * l.x + self.y * l.y + self.z * l.z).as_vec3()
    }
}

/// A tool as the simulation draws and checks it, mm.
#[derive(Clone, Debug, PartialEq)]
pub struct SimTool {
    pub number: u32,
    pub radius: f64,
    pub flute_length: f64,
    pub length: f64,
    pub shank_radius: f64,
    pub ball: bool,
}

impl SimTool {
    fn shape(&self) -> ToolShape {
        ToolShape {
            radius: self.radius,
            ball: self.ball,
            flute_length: self.flute_length,
            shank_radius: self.shank_radius,
            length: self.length,
        }
    }
}

/// One straight move, in the frame, with when it starts and how long it takes.
#[derive(Clone, Debug, PartialEq)]
pub struct SimMove {
    pub from: DVec3,
    pub to: DVec3,
    /// The tool axis (unit, in the frame) at the start and end.
    pub axis_from: DVec3,
    pub axis_to: DVec3,
    pub kind: MoveKind,
    pub toolpath: usize,
    pub tool: usize,
    pub start: f64,
    pub duration: f64,
}

/// Something wrong found while cutting.
#[derive(Clone, Debug, PartialEq)]
pub struct SimEvent {
    /// `rapid_in_material` or `shank_contact`.
    pub kind: &'static str,
    pub toolpath: usize,
    pub move_index: usize,
    pub time: f64,
    /// How deep into the material, mm.
    pub depth: f64,
    pub at: Vec3,
}

#[derive(Clone, Debug)]
pub struct Sim {
    pub frame: SimFrame,
    /// (id, name) of each toolpath, in running order.
    pub toolpaths: Vec<(String, String)>,
    pub tools: Vec<SimTool>,
    pub moves: Vec<SimMove>,
    pub duration: f64,
    /// Machine time shown, seconds.
    pub time: f64,
    pub playing: bool,
    /// Machine seconds per real second.
    pub speed: f64,
    stock: ZMap,
    /// The stock as cut up to `applied`.
    pub zmap: ZMap,
    /// How far the cutting has been applied: move index and the fraction of it.
    applied: (usize, f64),
    pub removed: f64,
    pub events: Vec<SimEvent>,
    /// Cells cut below the part, after a gouge check: (i, j, depth).
    pub gouges: Option<Vec<(usize, usize, f64)>>,
    /// Bumped when `gouges` changes, so the display rebuilds.
    pub marks_revision: u64,
    /// The voxel stock as cut so far, when the tool tilts (then `zmap` is unused).
    pub voxels: Option<Voxels>,
    voxel_stock: Option<Voxels>,
    /// Voxel cells cut inside the part, after a gouge check.
    pub voxel_gouges: Option<Vec<[usize; 3]>>,
}

impl Sim {
    /// A simulation of `ids` (all toolpaths when empty), in order, on the job's stock. `mode`
    /// is "height", "voxel" or "auto" (None): voxels when the tool tilts.
    pub fn new(s: &Session, ids: &[String], cell: Option<f64>, mode: Option<&str>) -> EngineResult<Self> {
        let job = &s.job;
        let chosen: Vec<&crate::toolpath::Toolpath> = if ids.is_empty() {
            job.toolpaths.iter().collect()
        } else {
            ids.iter()
                .map(|id| job.toolpaths.iter().find(|t| &t.id == id).ok_or_else(|| bad(format!("no toolpath `{id}`"))))
                .collect::<EngineResult<_>>()?
        };
        let first = chosen.first().ok_or_else(|| bad("there are no toolpaths to simulate; create one first"))?;
        let stock = job.stock.ok_or_else(|| bad("there is no stock to cut; create a setup or fit the stock first"))?;
        let frame_of = |wcs: &Option<String>| -> EngineResult<SimFrame> {
            Ok(match wcs {
                Some(id) => {
                    let w = job
                        .wcs_by_id(id)
                        .ok_or_else(|| bad(format!("a toolpath uses {id}, which no longer exists")))?;
                    SimFrame {
                        origin: w.origin.as_dvec3(),
                        x: w.x_axis.as_dvec3(),
                        y: w.y_axis().as_dvec3(),
                        z: w.z_axis.as_dvec3(),
                    }
                }
                None => SimFrame { origin: DVec3::ZERO, x: DVec3::X, y: DVec3::Y, z: DVec3::Z },
            })
        };
        // One tool axis for every toolpath: a height field along it will do.
        let one_axis = chosen.iter().all(|t| !t.multi_axis() && t.axis.dot(first.axis) > 1.0 - 1.0e-6);
        let voxel = match mode.unwrap_or("auto") {
            "voxel" => true,
            "height" if !one_axis => {
                return Err(EngineError::NotSupported(
                    "a height-field simulation of toolpaths whose tool tilts; use the voxel mode".into(),
                ));
            }
            "height" => false,
            _ => !one_axis,
        };
        let frame = if voxel {
            SimFrame::WORLD
        } else {
            let f = frame_of(&first.wcs)?;
            if f.z.dot(first.axis.as_dvec3()) > 1.0 - 1.0e-6 {
                f
            } else {
                // A 3+2 toolpath: the frame along its tool axis.
                let z = first.axis.as_dvec3().normalize_or(DVec3::Z);
                let x0 = if z.x.abs() > 0.9 { DVec3::Y } else { DVec3::X };
                let x = (x0 - z * x0.dot(z)).normalize_or(DVec3::X);
                SimFrame { origin: f.origin, x, y: z.cross(x), z }
            }
        };
        let rapid = s.library.active_machine().map_or(RAPID_RATE, |m| m.max_feed.max(RAPID_RATE));
        let mut tools: Vec<SimTool> = Vec::new();
        let mut tool_keys: Vec<(String, u32)> = Vec::new();
        let mut moves: Vec<SimMove> = Vec::new();
        let mut time = 0.0;
        let mut at: Option<DVec3> = None;
        let mut axis_at: Option<DVec3> = None;
        for (k, tp) in chosen.iter().enumerate() {
            frame_of(&tp.wcs)?;
            let t = s
                .library
                .machine(&tp.machine)
                .and_then(|m| m.tool(tp.tool))
                .ok_or_else(|| bad(format!("{} uses T{}, which is not in the library", tp.name, tp.tool)))?;
            // Tools are told apart by machine and number: two machines may both have a T1.
            let key = (tp.machine.clone(), t.number);
            let tool = match tool_keys.iter().position(|k| *k == key) {
                Some(i) => i,
                None => {
                    tool_keys.push(key);
                    tools.push(SimTool {
                        number: t.number,
                        radius: t.diameter / 2.0,
                        flute_length: t.flute_length,
                        length: t.overall_length,
                        shank_radius: t.shank_diameter / 2.0,
                        ball: t.kind == "ball",
                    });
                    tools.len() - 1
                }
            };
            let start = frame.local(tp.start);
            let start_axis = frame.local_dir(tp.axis_at(0));
            let mut from = at.unwrap_or(start);
            let mut from_axis = axis_at.unwrap_or(start_axis);
            // Between toolpaths the tool travels at rapid to the next one's start.
            if from.distance(start) > 1.0e-9 || from_axis.distance(start_axis) > 1.0e-9 {
                let duration = from.distance(start) / rapid * 60.0;
                moves.push(SimMove {
                    from,
                    to: start,
                    axis_from: from_axis,
                    axis_to: start_axis,
                    kind: MoveKind::Rapid,
                    toolpath: k,
                    tool,
                    start: time,
                    duration,
                });
                time += duration;
                from = start;
                from_axis = start_axis;
            }
            for (i, m) in tp.moves.iter().enumerate() {
                if moves.len() >= MAX_SIM_MOVES {
                    return Err(bad(format!("the program is longer than {MAX_SIM_MOVES} moves")));
                }
                let to = frame.local(m.to);
                let axis = frame.local_dir(tp.axis_at(i));
                let rate = if m.kind == MoveKind::Rapid { rapid } else { f64::from(m.feed).max(1.0) };
                let duration = from.distance(to) / rate * 60.0;
                moves.push(SimMove {
                    from,
                    to,
                    axis_from: from_axis,
                    axis_to: axis,
                    kind: m.kind,
                    toolpath: k,
                    tool,
                    start: time,
                    duration,
                });
                time += duration;
                from = to;
                from_axis = axis;
            }
            at = Some(from);
            axis_at = Some(from_axis);
        }
        let (lo, hi, bottom, top) = footprint(&frame, stock).ok_or_else(|| bad("the stock is empty"))?;
        let smallest = tools.iter().map(|t| t.radius).fold(f64::INFINITY, f64::min);
        let (zmap, voxels) = if voxel {
            let size = (stock.max - stock.min).as_dvec3();
            let volume = size.x * size.y * size.z;
            let cell = cell
                .filter(|c| *c > 0.0)
                .unwrap_or_else(|| (smallest / 3.0).max((volume / DEFAULT_VOXELS).cbrt()).clamp(0.05, 5.0));
            let v = Voxels::new(stock.min.as_dvec3(), stock.max.as_dvec3(), cell, MAX_VOXELS).ok_or_else(|| {
                bad(format!("the stock needs more than {MAX_VOXELS} cells at {cell:.3} mm; use a coarser `cell`"))
            })?;
            // The height field is not used; a coarse one keeps the fields filled.
            let coarse = (hi - lo).max_element().max(top - bottom).max(1.0) / 4.0;
            let z = ZMap::new(lo, hi, bottom, top, coarse, 1_000).ok_or_else(|| bad("the stock is empty"))?;
            (z, Some(v))
        } else {
            let cell = cell.filter(|c| *c > 0.0).unwrap_or((smallest / 4.0).clamp(0.05, 2.0));
            let z = ZMap::new(lo, hi, bottom, top, cell, 2_000_000)
                .ok_or_else(|| bad("the stock is too small to simulate"))?;
            (z, None)
        };
        Ok(Self {
            frame,
            toolpaths: chosen.iter().map(|t| (t.id.clone(), t.name.clone())).collect(),
            tools,
            moves,
            duration: time,
            time: 0.0,
            playing: false,
            speed: 10.0,
            stock: zmap.clone(),
            zmap,
            applied: (0, 0.0),
            removed: 0.0,
            events: Vec::new(),
            gouges: None,
            marks_revision: 0,
            voxel_stock: voxels.clone(),
            voxels,
            voxel_gouges: None,
        })
    }

    /// Bumped whenever the stock changes, so the display rebuilds.
    pub fn stock_revision(&self) -> u64 {
        self.voxels.as_ref().map_or(self.zmap.revision, |v| v.revision)
    }

    /// The stock's cell size, mm.
    pub fn cell(&self) -> f64 {
        self.voxels.as_ref().map_or(self.zmap.cell(), Voxels::cell)
    }

    /// The tool's tip and axis (both in the frame) at `time`.
    pub fn pose_at(&self, time: f64) -> Option<(DVec3, DVec3, &SimTool)> {
        let m = self.moves.get(self.move_at(time))?;
        let f = if m.duration > 0.0 { ((time - m.start) / m.duration).clamp(0.0, 1.0) } else { 1.0 };
        let axis = crate::toolpath::multiaxis::slerp(m.axis_from, m.axis_to, f);
        Some((m.from.lerp(m.to, f), axis, self.tools.get(m.tool)?))
    }

    /// The move under way at `time`.
    pub fn move_at(&self, time: f64) -> usize {
        match self.moves.binary_search_by(|m| m.start.total_cmp(&time)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
        .min(self.moves.len().saturating_sub(1))
    }

    /// Where the tool's tip is at `time`, in the frame.
    pub fn tool_at(&self, time: f64) -> Option<(DVec3, &SimTool)> {
        let m = self.moves.get(self.move_at(time))?;
        let f = if m.duration > 0.0 { ((time - m.start) / m.duration).clamp(0.0, 1.0) } else { 1.0 };
        Some((m.from.lerp(m.to, f), self.tools.get(m.tool)?))
    }

    /// Cuts the stock up to `time` (replaying from the start when going back).
    pub fn seek(&mut self, time: f64) {
        let time = if time.is_finite() { time.clamp(0.0, self.duration) } else { 0.0 };
        let applied_time =
            self.moves.get(self.applied.0).map_or(self.duration, |m| m.start + m.duration * self.applied.1);
        if time + 1.0e-9 < applied_time {
            let revision = self.zmap.revision;
            self.zmap = self.stock.clone();
            self.zmap.revision = revision.wrapping_add(1);
            if let (Some(v), Some(fresh)) = (self.voxels.as_mut(), self.voxel_stock.as_ref()) {
                let revision = v.revision;
                *v = fresh.clone();
                v.revision = revision.wrapping_add(1);
            }
            self.applied = (0, 0.0);
            self.removed = 0.0;
            self.events.clear();
        }
        while let Some(m) = self.moves.get(self.applied.0) {
            let f = if m.duration > 0.0 { ((time - m.start) / m.duration).clamp(0.0, 1.0) } else { 1.0 };
            if f > self.applied.1 || m.duration <= 0.0 {
                let (a, b) = (m.from.lerp(m.to, self.applied.1), m.from.lerp(m.to, f));
                let tool = self.tools.get(m.tool).cloned();
                if let (Some(tool), true) = (tool.as_ref(), self.voxels.is_some()) {
                    let index = self.applied.0;
                    let (fa, fb) = (self.applied.1, f);
                    let m = m.clone();
                    self.cut_voxels(&m, index, fa, fb, tool);
                } else if let Some(tool) = tool {
                    let cut = self.zmap.cut(a, b, tool.radius);
                    self.removed += cut.volume;
                    let tol = self.zmap.cell() * 0.5;
                    let kind = if m.kind == MoveKind::Rapid && cut.core_depth > tol {
                        Some("rapid_in_material")
                    } else if cut.core_depth > tool.flute_length + tol {
                        Some("shank_contact")
                    } else {
                        None
                    };
                    let index = self.applied.0;
                    if let Some(kind) = kind
                        && self.events.len() < MAX_EVENTS
                        && !self.events.iter().any(|e| e.kind == kind && e.move_index == index)
                    {
                        self.events.push(SimEvent {
                            kind,
                            toolpath: m.toolpath,
                            move_index: index,
                            time: m.start + m.duration * f,
                            depth: cut.core_depth,
                            at: self.frame.world(b),
                        });
                    }
                }
                self.applied.1 = f;
            }
            if f < 1.0 {
                break;
            }
            self.applied = (self.applied.0 + 1, 0.0);
        }
        self.time = time;
    }

    /// Cuts the voxel stock along move `m` (number `index`) from fraction `fa` to `fb`, the
    /// tool axis turning with it: pieces turn little enough that the tool's top moves about a
    /// cell each.
    fn cut_voxels(&mut self, m: &SimMove, index: usize, fa: f64, fb: f64, tool: &SimTool) {
        let Some(v) = self.voxels.as_mut() else { return };
        let turn = m.axis_from.dot(m.axis_to).clamp(-1.0, 1.0).acos() * (fb - fa);
        let pieces = ((turn * tool.length.max(1.0) / v.cell()).ceil() as usize).clamp(1, 256);
        let shape = tool.shape();
        let mut removed = 0usize;
        let mut deep = 0usize;
        let mut shank: Option<(usize, DVec3)> = None;
        for k in 0..pieces {
            let (t0, t1) = (fa + (fb - fa) * k as f64 / pieces as f64, fa + (fb - fa) * (k + 1) as f64 / pieces as f64);
            let axis = crate::toolpath::multiaxis::slerp(m.axis_from, m.axis_to, (t0 + t1) / 2.0);
            let cut = v.cut(m.from.lerp(m.to, t0), m.from.lerp(m.to, t1), axis, &shape);
            removed += cut.removed + cut.shank;
            deep += cut.deep;
            if let (Some(at), None) = (cut.shank_at, shank) {
                shank = Some((cut.deep_shank, at));
            }
        }
        self.removed += removed as f64 * v.cell().powi(3);
        // Cells the tool's surface only grazed (within a cell) are not collisions.
        let kind = if m.kind == MoveKind::Rapid && deep > 0 {
            Some(("rapid_in_material", m.from.lerp(m.to, fb)))
        } else {
            shank.map(|(_, at)| ("shank_contact", at))
        };
        if let Some((kind, at)) = kind
            && self.events.len() < MAX_EVENTS
            && !self.events.iter().any(|e| e.kind == kind && e.move_index == index)
        {
            self.events.push(SimEvent {
                kind,
                toolpath: m.toolpath,
                move_index: index,
                time: m.start + m.duration * fb,
                depth: v.cell(),
                at: self.frame.world(at),
            });
        }
    }

    /// Advances by `dt` real seconds while playing; stops at the end.
    pub fn tick(&mut self, dt: f64) {
        if !self.playing || !dt.is_finite() {
            return;
        }
        let next = self.time + dt.clamp(0.0, 0.25) * self.speed;
        self.seek(next);
        if self.time >= self.duration {
            self.playing = false;
        }
    }

    /// The toolpath under way at the current time.
    pub fn current_toolpath(&self) -> Option<&(String, String)> {
        self.moves.get(self.move_at(self.time)).and_then(|m| self.toolpaths.get(m.toolpath))
    }

    pub fn to_json(&self) -> Value {
        let current = self.move_at(self.time);
        json!({
            "time": self.time,
            "duration": self.duration,
            "playing": self.playing,
            "speed": self.speed,
            "move": current,
            "moves": self.moves.len(),
            "toolpath": self.current_toolpath().map(|(id, _)| id.clone()),
            "toolpaths": self.toolpaths.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            "removed_volume": self.removed,
            "remaining_volume": self.voxels.as_ref().map_or_else(|| self.zmap.volume(), Voxels::volume),
            "cell": self.cell(),
            "mode": if self.voxels.is_some() { "voxel" } else { "height" },
            "events": self.events.iter().map(event_json).collect::<Vec<_>>(),
        })
    }
}

fn event_json(e: &SimEvent) -> Value {
    json!({
        "kind": e.kind,
        "toolpath": e.toolpath,
        "move": e.move_index,
        "time": e.time,
        "depth": e.depth,
        "at": e.at.to_array(),
    })
}

/// A world box in the frame: XY footprint and Z range.
fn footprint(f: &SimFrame, b: Aabb) -> Option<(DVec2, DVec2, f64, f64)> {
    if b.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
    for i in 0..8 {
        let c = Vec3::new(
            if i & 1 == 0 { b.min.x } else { b.max.x },
            if i & 2 == 0 { b.min.y } else { b.max.y },
            if i & 4 == 0 { b.min.z } else { b.max.z },
        );
        let l = f.local(c);
        lo = lo.min(l);
        hi = hi.max(l);
    }
    Some((lo.truncate(), hi.truncate(), lo.z, hi.z))
}

/// Cells cut below the part's surface by more than `tolerance`, after running `sim` to the end:
/// (i, j, depth), deepest first.
pub fn gouges(s: &Session, sim: &Sim, tolerance: f64) -> Vec<(usize, usize, f64)> {
    let f = sim.frame;
    let tris: Vec<[DVec3; 3]> = s
        .job
        .models
        .iter()
        .flat_map(|m| m.mesh.triangles().iter().map(|t| m.mesh.corners(*t).map(|p| f.local(p))).collect::<Vec<_>>())
        .collect();
    // The part's surface at a point: the top of the part straight below it.
    let part = DropCutter::new(tris, 1.0e-4);
    // Each cell is checked against the lowest surface around it, so a cell beside a vertical
    // wall is measured against the floor, not the top of the wall; overcuts into a wall deeper
    // than this are still found.
    let reach = sim.zmap.cell().max(tolerance);
    let around = [
        DVec2::ZERO,
        DVec2::new(reach, 0.0),
        DVec2::new(-reach, 0.0),
        DVec2::new(0.0, reach),
        DVec2::new(0.0, -reach),
        DVec2::new(reach, reach) * std::f64::consts::FRAC_1_SQRT_2,
        DVec2::new(-reach, reach) * std::f64::consts::FRAC_1_SQRT_2,
        DVec2::new(reach, -reach) * std::f64::consts::FRAC_1_SQRT_2,
        DVec2::new(-reach, -reach) * std::f64::consts::FRAC_1_SQRT_2,
    ];
    let (nx, ny) = sim.zmap.size();
    let mut out = Vec::new();
    for j in 0..ny {
        for i in 0..nx {
            let Some(z) = sim.zmap.height(i, j) else { continue };
            let c = sim.zmap.centre(i, j);
            // Near the part's outline some samples miss it: nothing there to gouge.
            let surface =
                around.iter().map(|d| part.height(c + *d)).try_fold(f64::INFINITY, |low, h| h.map(|h| low.min(h)));
            let Some(surface) = surface else { continue };
            let depth = surface - z;
            if depth > tolerance {
                out.push((i, j, depth));
            }
        }
    }
    out.sort_by(|a, b| b.2.total_cmp(&a.2));
    out
}

/// Voxel cells cut inside the part (deeper than a cell), after running `sim` to the end.
pub fn voxel_gouges(s: &Session, sim: &Sim) -> Vec<[usize; 3]> {
    let Some(v) = sim.voxels.as_ref() else { return Vec::new() };
    let tris: Vec<[DVec3; 3]> = s
        .job
        .models
        .iter()
        .flat_map(|m| m.mesh.triangles().iter().map(|t| m.mesh.corners(*t).map(|p| p.as_dvec3())).collect::<Vec<_>>())
        .collect();
    let part = v.part_cells(&tris);
    v.gouges(&part, 100_000)
}

/// A voxel gouge report for the API.
pub fn voxel_gouge_report(sim: &Sim, found: &[[usize; 3]]) -> Value {
    let cell = sim.cell();
    json!({
        "tolerance": cell,
        "gouges": found.len(),
        "area": found.len() as f64 * cell * cell,
        "worst_depth": if found.is_empty() { 0.0 } else { cell },
        "deepest": found.iter().take(MAX_REPORTED).filter_map(|[i, j, k]| {
            sim.voxels.as_ref().map(|v| json!({ "at": v.centre(*i, *j, *k).to_array(), "depth": cell }))
        }).collect::<Vec<_>>(),
    })
}

/// A gouge report for the API.
pub fn gouge_report(sim: &Sim, found: &[(usize, usize, f64)], tolerance: f64) -> Value {
    let cell = sim.zmap.cell();
    json!({
        "tolerance": tolerance,
        "gouges": found.len(),
        "area": found.len() as f64 * cell * cell,
        "worst_depth": found.first().map_or(0.0, |g| g.2),
        "deepest": found.iter().take(MAX_REPORTED).map(|(i, j, d)| {
            let c = sim.zmap.centre(*i, *j);
            let z = sim.zmap.height(*i, *j).unwrap_or(0.0);
            json!({ "at": sim.frame.world(c.extend(z)).to_array(), "depth": d })
        }).collect::<Vec<_>>(),
    })
}
