//! Toolpaths: cutter location (CL) data made by a strategy from a tool, geometry and
//! settings. Moves are kept in world millimetres with the feed each uses; the WCS they were
//! programmed in travels with them for posting later.

pub mod contour;

use std::sync::Arc;

use CosmicCAM_geom::{Aabb, Vec3};
use serde_json::{Value, json};

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
