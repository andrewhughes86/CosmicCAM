//! Material removal simulation: the job's toolpaths run in machine time against a height field
//! of the stock, as a 3-axis machine with flat end mills would cut it.
//!
//! - Playback follows each move's feed (rapids at the machine's rapid rate), scaled by a speed.
//! - The stock is a [`ZMap`] in the toolpaths' frame; seeking back replays from the start.
//! - Verification: gouges are cells cut below the part surface; collisions are rapids that
//!   remove material and cuts deeper than the tool's flutes (the shank would rub).

use CosmicCAM_geom::cutter::DropCutter;
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
}

/// One straight move, in the frame, with when it starts and how long it takes.
#[derive(Clone, Debug, PartialEq)]
pub struct SimMove {
    pub from: DVec3,
    pub to: DVec3,
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
}

impl Sim {
    /// A simulation of `ids` (all toolpaths when empty), in order, on the job's stock.
    pub fn new(s: &Session, ids: &[String], cell: Option<f64>) -> EngineResult<Self> {
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
        let frame = frame_of(&first.wcs)?;
        let rapid = s.library.active_machine().map_or(RAPID_RATE, |m| m.max_feed.max(RAPID_RATE));
        let mut tools: Vec<SimTool> = Vec::new();
        let mut moves: Vec<SimMove> = Vec::new();
        let mut time = 0.0;
        let mut at: Option<DVec3> = None;
        for (k, tp) in chosen.iter().enumerate() {
            let f = frame_of(&tp.wcs)?;
            if f.z.dot(frame.z) < 0.999 {
                return Err(EngineError::NotSupported(format!(
                    "simulating toolpaths with different tool axes ({} is not along {})",
                    tp.name, first.name
                )));
            }
            let t = s
                .library
                .machine(&tp.machine)
                .and_then(|m| m.tool(tp.tool))
                .ok_or_else(|| bad(format!("{} uses T{}, which is not in the library", tp.name, tp.tool)))?;
            let tool = match tools.iter().position(|x| x.number == t.number) {
                Some(i) => i,
                None => {
                    tools.push(SimTool {
                        number: t.number,
                        radius: t.diameter / 2.0,
                        flute_length: t.flute_length,
                        length: t.overall_length,
                        shank_radius: t.shank_diameter / 2.0,
                    });
                    tools.len() - 1
                }
            };
            let start = frame.local(tp.start);
            let mut from = at.unwrap_or(start);
            // Between toolpaths the tool travels at rapid to the next one's start.
            if from.distance(start) > 1.0e-9 {
                let duration = from.distance(start) / rapid * 60.0;
                moves.push(SimMove {
                    from,
                    to: start,
                    kind: MoveKind::Rapid,
                    toolpath: k,
                    tool,
                    start: time,
                    duration,
                });
                time += duration;
                from = start;
            }
            for m in tp.moves.iter() {
                if moves.len() >= MAX_SIM_MOVES {
                    return Err(bad(format!("the program is longer than {MAX_SIM_MOVES} moves")));
                }
                let to = frame.local(m.to);
                let rate = if m.kind == MoveKind::Rapid { rapid } else { f64::from(m.feed).max(1.0) };
                let duration = from.distance(to) / rate * 60.0;
                moves.push(SimMove { from, to, kind: m.kind, toolpath: k, tool, start: time, duration });
                time += duration;
                from = to;
            }
            at = Some(from);
        }
        let (lo, hi, bottom, top) = footprint(&frame, stock).ok_or_else(|| bad("the stock is empty"))?;
        let smallest = tools.iter().map(|t| t.radius).fold(f64::INFINITY, f64::min);
        let cell = cell.filter(|c| *c > 0.0).unwrap_or((smallest / 4.0).clamp(0.05, 2.0));
        let zmap =
            ZMap::new(lo, hi, bottom, top, cell, 2_000_000).ok_or_else(|| bad("the stock is too small to simulate"))?;
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
        })
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
            self.applied = (0, 0.0);
            self.removed = 0.0;
            self.events.clear();
        }
        while let Some(m) = self.moves.get(self.applied.0) {
            let f = if m.duration > 0.0 { ((time - m.start) / m.duration).clamp(0.0, 1.0) } else { 1.0 };
            if f > self.applied.1 || m.duration <= 0.0 {
                let (a, b) = (m.from.lerp(m.to, self.applied.1), m.from.lerp(m.to, f));
                let tool = self.tools.get(m.tool).cloned();
                if let Some(tool) = tool {
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
            "remaining_volume": self.zmap.volume(),
            "cell": self.zmap.cell(),
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
