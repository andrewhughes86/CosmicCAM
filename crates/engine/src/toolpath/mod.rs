//! Toolpaths: cutter location (CL) data made by a strategy from a tool, geometry and
//! settings. Moves are kept in world millimetres with the feed each uses; the WCS they were
//! programmed in travels with them for posting later.

pub mod adaptive;
pub mod bore;
mod common;
pub mod contour;
pub mod drill;
pub mod finish;
pub mod multiaxis;
pub mod strategies;
pub mod twod;

use std::sync::Arc;

use CosmicCAM_geom::{Aabb, Vec3};
use serde_json::{Value, json};

/// A strategy id as people read it.
pub fn strategy_label(strategy: &str) -> &'static str {
    match strategy {
        "face" => "Face",
        "pocket2d" => "Pocket",
        "drill" => "Drill",
        "rough3d" => "3D rough",
        "waterline" => "Waterline",
        "raster" => "Raster",
        "multiaxis" => "Multi-axis finish",
        "bore" => "Helical bore",
        "adaptive" => "Adaptive clearing",
        "scallop" => "Scallop",
        "spiral" => "Spiral",
        "radial" => "Radial",
        "flat" => "Flat",
        "pencil" => "Pencil",
        "steep_shallow" => "Steep and shallow",
        "chamfer" => "Chamfer",
        "slot" => "Slot",
        "trace" => "Trace",
        "thread" => "Thread milling",
        _ => "2D contour",
    }
}

/// The command that makes toolpaths of `strategy` (the id a toolpath records).
pub fn strategy_command(strategy: &str) -> &'static str {
    match strategy {
        "face" => "toolpath.face",
        "pocket2d" => "toolpath.pocket",
        "drill" => "toolpath.drill",
        "rough3d" => "toolpath.rough",
        "waterline" => "toolpath.waterline",
        "raster" => "toolpath.raster",
        "multiaxis" => "toolpath.multiaxis",
        "bore" => "toolpath.bore",
        "adaptive" => "toolpath.adaptive",
        "scallop" => "toolpath.scallop",
        "spiral" => "toolpath.spiral",
        "radial" => "toolpath.radial",
        "flat" => "toolpath.flat",
        "pencil" => "toolpath.pencil",
        "steep_shallow" => "toolpath.steep_shallow",
        "chamfer" => "toolpath.chamfer",
        "slot" => "toolpath.slot",
        "trace" => "toolpath.trace",
        "thread" => "toolpath.thread",
        _ => "toolpath.contour",
    }
}

/// Most toolpaths one job holds.
pub const MAX_TOOLPATHS: usize = 512;
/// Most moves one toolpath may have; generation stops with an error beyond it.
pub const MAX_MOVES: usize = 2_000_000;

/// What a move is for; the viewport colours them by kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveKind {
    /// Positioning at rapid traverse (G0).
    Rapid,
    /// Linking at a feed rate, off the part (high-feed links).
    Link,
    /// Going down into the material: plunges and ramps.
    Plunge,
    /// Lead-in and lead-out moves.
    Lead,
    /// Cutting along the contour.
    Cut,
}

impl MoveKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Rapid => "rapid",
            Self::Link => "link",
            Self::Plunge => "plunge",
            Self::Lead => "lead",
            Self::Cut => "cut",
        }
    }
}

/// One straight move to `to` (world mm) at `feed` mm/min (0 for rapids).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Move {
    pub kind: MoveKind,
    pub to: Vec3,
    pub feed: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toolpath {
    /// `toolpath.<n>`.
    pub id: String,
    pub name: String,
    /// The strategy that made it, e.g. `contour2d`.
    pub strategy: String,
    pub machine: String,
    pub tool: u32,
    /// The WCS it is programmed in, if any (else world coordinates).
    pub wcs: Option<String>,
    /// The tool axis (world, unit, from the tip up) the toolpath was worked out along: its
    /// frame's Z, which is the WCS's Z unless an `orientation` tilts it (3+2 machining).
    pub axis: Vec3,
    /// Each move's tool axis (world, unit) when they vary (simultaneous multi-axis
    /// machining); None when every move uses `axis`.
    pub axes: Option<Arc<Vec<Vec3>>>,
    /// The settings it was made with, so it can be edited and regenerated.
    pub params: Value,
    /// Where the tool starts (above the first move, at clearance height).
    pub start: Vec3,
    pub moves: Arc<Vec<Move>>,
    pub warnings: Vec<String>,
    /// A drilling toolpath's cycle, for the G-code's canned cycles.
    pub drill: Option<DrillCycle>,
    /// Drawn in the viewport even when it is not the selected toolpath.
    pub visible: bool,
}

/// A drilling cycle and its holes, in the toolpath's WCS (millimetres).
#[derive(Clone, Debug, PartialEq)]
pub struct DrillCycle {
    /// One of [`drill::CYCLES`].
    pub cycle: String,
    /// Each hole: X, Y, top Z, bottom Z, in drilling order.
    pub holes: Vec<[f64; 4]>,
    /// The plane the tool returns to between pecks and holes (R).
    pub retract: f64,
    pub clearance: f64,
    /// Depth per peck.
    pub peck: f64,
    /// Seconds at the bottom.
    pub dwell: f64,
    /// Feed, mm/min.
    pub feed: f64,
    /// Thread pitch for tapping, mm per turn.
    pub pitch: f64,
}

impl Toolpath {
    /// The tool axis at the end of move `i`.
    pub fn axis_at(&self, i: usize) -> Vec3 {
        self.axes.as_ref().and_then(|a| a.get(i).copied()).unwrap_or(self.axis)
    }

    /// True when the tool tilts during the toolpath (simultaneous 4/5-axis).
    pub fn multi_axis(&self) -> bool {
        self.axes.is_some()
    }

    /// Lengths (mm) of cutting and of rapid moves, and an estimated time (s) with rapids at
    /// `rapid_rate` mm/min.
    pub fn stats(&self, rapid_rate: f64) -> (f64, f64, f64) {
        let (mut cut, mut rapid, mut time) = (0.0, 0.0, 0.0);
        let mut at = self.start;
        for m in self.moves.iter() {
            let d = f64::from(at.distance(m.to));
            if m.kind == MoveKind::Rapid {
                rapid += d;
                time += d / rapid_rate.max(1.0) * 60.0;
            } else {
                cut += d;
                time += d / f64::from(m.feed).max(1.0) * 60.0;
            }
            at = m.to;
        }
        (cut, rapid, time)
    }

    pub fn bounds(&self) -> Aabb {
        Aabb::from_points(std::iter::once(self.start).chain(self.moves.iter().map(|m| m.to)))
    }

    pub fn to_json(&self, rapid_rate: f64) -> Value {
        let (cut, rapid, time) = self.stats(rapid_rate);
        json!({
            "id": self.id,
            "name": self.name,
            "strategy": self.strategy,
            "machine": self.machine,
            "tool": self.tool,
            "wcs": self.wcs,
            "tool_axis": self.axis.to_array(),
            "multi_axis": self.multi_axis(),
            "moves": self.moves.len(),
            "cut_length": cut,
            "rapid_length": rapid,
            "time": time,
            "bounds": crate::session::bounds_json(&self.bounds()),
            "warnings": self.warnings,
            "visible": self.visible,
            "params": self.params,
        })
    }

    /// The moves as the API returns them: `{kind, to: [x, y, z], feed}`, with `axis` (the tool
    /// axis, world) on multi-axis toolpaths.
    pub fn moves_json(&self, limit: usize) -> Value {
        json!(
            self.moves
                .iter()
                .take(limit)
                .enumerate()
                .map(|(i, m)| {
                    let mut v = json!({ "kind": m.kind.id(), "to": m.to.to_array(), "feed": m.feed });
                    if self.multi_axis() {
                        v["axis"] = json!(self.axis_at(i).to_array());
                    }
                    v
                })
                .collect::<Vec<_>>()
        )
    }
}
