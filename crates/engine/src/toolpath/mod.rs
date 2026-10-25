//! Toolpaths: cutter location (CL) data made by a strategy from a tool, geometry and
//! settings. Moves are kept in world millimetres with the feed each uses; the WCS they were
//! programmed in travels with them for posting later.

mod common;
pub mod contour;
pub mod drill;
pub mod strategies;

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
        _ => "2D contour",
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

    /// The moves as the API returns them: `{kind, to: [x, y, z], feed}`.
    pub fn moves_json(&self, limit: usize) -> Value {
        json!(
            self.moves
                .iter()
                .take(limit)
                .map(|m| json!({ "kind": m.kind.id(), "to": m.to.to_array(), "feed": m.feed }))
                .collect::<Vec<_>>()
        )
    }
}
