//! Simulation commands: start, play, pause, step, seek and speed the material removal
//! simulation, and check the cut for gouges and collisions.

use serde_json::{Value, json};

use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::session::Session;
use crate::sim::{self, Sim};

/// Playback speeds the Speed button steps through, machine seconds per second.
const SPEEDS: [f64; 8] = [1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0];
const STATE: &str = "the simulation {time, duration, playing, speed, move, moves, toolpath, toolpaths, removed_volume, remaining_volume, cell, mode, events}";
const MODES: &[&str] = &["auto", "height", "voxel"];

/// The simulation, started on all toolpaths if none is open.
fn ensure(s: &mut Session) -> EngineResult<&mut Sim> {
    if s.sim.is_none() {
        s.sim = Some(Box::new(Sim::new(s, &[], None, None)?));
    }
    s.sim.as_deref_mut().ok_or_else(|| EngineError::Internal("simulation".into()))
}

fn state(s: &Session) -> Value {
    s.sim.as_ref().map_or(Value::Null, |sim| sim.to_json())
}

fn run_to_end(s: &mut Session) -> EngineResult<Sim> {
    let mut whole = ensure(s)?.clone();
    whole.playing = false;
    whole.seek(whole.duration);
    Ok(whole)
}

const TOLERANCE: ParamKind = ParamKind::Length { min: 0.0, max: 100.0 };

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "sim.start",
        label: "Simulate",
        tooltip: "Start the material removal simulation from the beginning",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::optional("toolpaths", ParamKind::StrList { max: 512 }, "Toolpaths to run, in order (default: all)"),
            Param::optional(
                "cell",
                ParamKind::Length { min: 0.0, max: 100.0 },
                "Stock grid spacing, mm (default: a quarter of the smallest tool radius; voxels: a quarter, coarser for big stock)",
            ),
            Param::optional(
                "mode",
                ParamKind::Choice(MODES),
                "Stock model: a height field (one tool axis), voxels (any axis), or auto: voxels when the tool tilts",
            ),
        ],
        returns: STATE,
        enabled: always,
        run: |s, p| {
            let ids = params::str_list(p, "toolpaths")?;
            let cell = params::opt_f64(p, "cell", 0.0, 100.0)?;
            let mode = params::opt_str(p, "mode")?;
            s.sim = Some(Box::new(Sim::new(s, &ids, cell, mode)?));
            Ok(state(s))
        },
    },
    CommandSpec {
        id: "sim.play",
        label: "Play",
        tooltip: "Run the material removal simulation",
        placement: Some(Placement::large(RibbonTab::Simulation, "Playback")),
        icon: None,
        shortcut: Some("Space"),
        params: &[],
        returns: STATE,
        enabled: always,
        run: |s, _| {
            let sim = ensure(s)?;
            if sim.time >= sim.duration {
                sim.seek(0.0);
            }
            sim.playing = true;
            Ok(state(s))
        },
    },
    CommandSpec {
        id: "sim.pause",
        label: "Pause",
        tooltip: "Pause the simulation",
        placement: Some(Placement::small(RibbonTab::Simulation, "Playback")),
        icon: None,
        shortcut: None,
        params: &[],
        returns: STATE,
        enabled: always,
        run: |s, _| {
            if let Some(sim) = s.sim.as_deref_mut() {
                sim.playing = false;
            }
            Ok(state(s))
        },
    },
    CommandSpec {
        id: "sim.step",
        label: "Step",
        tooltip: "Advance to the next move (or several)",
        placement: Some(Placement::small(RibbonTab::Simulation, "Playback")),
        icon: None,
        shortcut: None,
        params: &[Param::optional(
            "moves",
            ParamKind::Integer { min: -1_000_000, max: 1_000_000 },
            "How many moves to step; negative goes back (default 1)",
        )],
        returns: STATE,
        enabled: always,
        run: |s, p| {
            let n = params::opt_f64(p, "moves", -1.0e6, 1.0e6)?.unwrap_or(1.0) as i64;
            let sim = ensure(s)?;
            sim.playing = false;
            let current = sim.move_at(sim.time) as i64;
            let target = (current + n).clamp(0, sim.moves.len() as i64);
            let time = sim.moves.get(target as usize).map_or(sim.duration, |m| m.start);
            sim.seek(time);
            Ok(state(s))
        },
    },
    CommandSpec {
        id: "sim.seek",
        label: "Go to",
        tooltip: "Jump to a time in the simulation",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::optional("time", ParamKind::Number { min: 0.0, max: 1.0e9 }, "Machine time, seconds"),
            Param::optional("fraction", ParamKind::Number { min: 0.0, max: 1.0 }, "Share of the whole program, 0 to 1"),
        ],
        returns: STATE,
        enabled: always,
        run: |s, p| {
            let time = params::opt_f64(p, "time", 0.0, 1.0e9)?;
            let fraction = params::opt_f64(p, "fraction", 0.0, 1.0)?;
            let sim = ensure(s)?;
            let target = match (time, fraction) {
                (Some(t), _) => t,
                (None, Some(f)) => f * sim.duration,
                (None, None) => return Err(EngineError::InvalidParams("give `time` or `fraction`".into())),
            };
            sim.seek(target);
            Ok(state(s))
        },
    },
    CommandSpec {
        id: "sim.speed",
        label: "Speed",
        tooltip: "Playback speed: machine seconds per second (the button steps through 1× to 250×)",
        placement: Some(Placement::small(RibbonTab::Simulation, "Playback")),
        icon: None,
        shortcut: None,
        params: &[Param::optional("speed", ParamKind::Number { min: 0.1, max: 10_000.0 }, "Speed multiplier")],
        returns: STATE,
        enabled: always,
        run: |s, p| {
            let speed = params::opt_f64(p, "speed", 0.1, 10_000.0)?;
            let sim = ensure(s)?;
            sim.speed =
                speed.unwrap_or_else(|| SPEEDS.iter().copied().find(|v| *v > sim.speed + 1.0e-9).unwrap_or(SPEEDS[0]));
            Ok(state(s))
        },
    },
    CommandSpec {
        id: "sim.stop",
        label: "Close simulation",
        tooltip: "Leave the simulation and show the part again",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "null",
        enabled: always,
        run: |s, _| {
            s.sim = None;
            Ok(Value::Null)
        },
    },
    CommandSpec {
        id: "sim.get",
        label: "Simulation state",
        tooltip: "Where the simulation is, what it has removed and what it found",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "the simulation, or null when none is open",
        enabled: always,
        run: |s, _| Ok(state(s)),
    },
    CommandSpec {
        id: "sim.collision.check",
        label: "Collisions",
        tooltip: "Run the whole program and list rapids into material and cuts deeper than the flutes",
        placement: Some(Placement::large(RibbonTab::Simulation, "Verify")),
        icon: None,
        shortcut: None,
        params: &[],
        returns: "{collisions, events: [{kind, toolpath, move, time, depth, at}]}",
        enabled: always,
        run: |s, _| {
            let whole = run_to_end(s)?;
            let v = whole.to_json();
            Ok(json!({ "collisions": whole.events.len(), "events": v["events"], "toolpaths": v["toolpaths"] }))
        },
    },
    CommandSpec {
        id: "sim.gouge.check",
        label: "Gouges",
        tooltip: "Run the whole program and find where it cuts below the part's surface",
        placement: Some(Placement::large(RibbonTab::Simulation, "Verify")),
        icon: None,
        shortcut: None,
        params: &[Param::optional(
            "tolerance",
            TOLERANCE,
            "How far below the surface counts as a gouge, mm (default 0.05)",
        )],
        returns: "{tolerance, gouges, area, worst_depth, deepest: [{at, depth}]}",
        enabled: always,
        run: |s, p| {
            let tolerance = params::opt_f64(p, "tolerance", 0.0, 100.0)?.unwrap_or(0.05);
            let whole = run_to_end(s)?;
            let mut shown = whole;
            let report = if shown.voxels.is_some() {
                let found = sim::voxel_gouges(s, &shown);
                let report = sim::voxel_gouge_report(&shown, &found);
                shown.voxel_gouges = Some(found);
                report
            } else {
                let found = sim::gouges(s, &shown, tolerance);
                let report = sim::gouge_report(&shown, &found, tolerance);
                shown.gouges = Some(found);
                report
            };
            // Show the result: the finished cut with the gouges marked.
            shown.marks_revision = s.sim.as_ref().map_or(0, |x| x.marks_revision).wrapping_add(1);
            shown.zmap.revision = s.sim.as_ref().map_or(0, |x| x.zmap.revision).wrapping_add(1);
            if let Some(v) = shown.voxels.as_mut() {
                v.revision = v.revision.wrapping_add(1);
            }
            s.sim = Some(Box::new(shown));
            Ok(report)
        },
    },
];
