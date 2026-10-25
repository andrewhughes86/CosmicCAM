//! Toolpath commands: the 2D contour strategy, and listing, reading and removing toolpaths.

use serde_json::{Value, json};

use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::session::Session;
use crate::toolpath::contour::{
    self, COOLANTS, DIRECTIONS, ENTRIES, HEIGHT_REFS, LEADS, RAMPS, RETRACTS, SIDES, TOOL_AXES,
};
use crate::toolpath::{MAX_TOOLPATHS, Toolpath};

const LENGTH: ParamKind = ParamKind::Length { min: 0.0, max: 10_000.0 };
const OFFSET: ParamKind = ParamKind::Length { min: -100_000.0, max: 100_000.0 };
const STOCK: ParamKind = ParamKind::Length { min: -1000.0, max: 1000.0 };
const RATE: ParamKind = ParamKind::Rate { min: 0.0, max: 1.0e6 };
const ANGLE: ParamKind = ParamKind::Number { min: 0.0, max: 180.0 };
const REFS: ParamKind = ParamKind::Choice(HEIGHT_REFS);

/// Rapid traverse assumed for time estimates when the machine gives none faster.
const RAPID_RATE: f64 = 10_000.0;

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "toolpath.contour",
        label: "2D contour",
        tooltip: "Follow selected edges or a face outline at depth, offset by the tool radius",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "Strategies")),
        icon: Some("toolpath_profile"),
        shortcut: None,
        params: &[
            Param::optional("id", ParamKind::Str, "Regenerate this toolpath with these settings instead of adding one"),
            Param::optional("name", ParamKind::Str, "Name in the job outline (default: 2D contour T<n>)"),
            // Tool
            Param::optional(
                "tool",
                ParamKind::Integer { min: 1, max: 9999 },
                "Tool number in the active machine's library (default: the first)",
            ),
            Param::optional(
                "spindle_rpm",
                ParamKind::Number { min: 0.0, max: 500_000.0 },
                "Spindle speed, rpm (default: the tool's)",
            ),
            Param::optional("feed", RATE, "Cutting feed, mm/min (default: the tool's)"),
            Param::optional(
                "plunge_feed",
                RATE,
                "Plunge and ramp feed, mm/min (default: the tool's, else half the feed)",
            ),
            Param::optional("lead_feed", RATE, "Lead-in and lead-out feed, mm/min (default: the cutting feed)"),
            Param::optional("coolant", ParamKind::Choice(COOLANTS), "Coolant (default flood)"),
            // Multi-axis
            Param::optional(
                "tool_axis",
                ParamKind::Choice(TOOL_AXES),
                "Tool orientation (only the WCS Z axis for now)",
            ),
            // Geometry
            Param::optional(
                "geometry",
                ParamKind::StrList { max: 1000 },
                "Edges and faces to follow (model.N/edge.N, model.N/face.N)",
            ),
            Param::optional(
                "side",
                ParamKind::Choice(SIDES),
                "Cut outside or inside closed contours, or on the line (open contours: outside = left)",
            ),
            Param::optional("reverse", ParamKind::Bool, "Run the contour the other way"),
            Param::optional("wcs", ParamKind::Str, "WCS to program in (default: the active one)"),
            // Heights
            Param::optional("clearance_from", REFS, "Clearance height reference (default model_top)"),
            Param::optional("clearance_offset", OFFSET, "Clearance height above its reference, mm (default 15)"),
            Param::optional("retract_from", REFS, "Retract height reference (default model_top)"),
            Param::optional("retract_offset", OFFSET, "Retract height above its reference, mm (default 5)"),
            Param::optional(
                "feed_height_from",
                REFS,
                "Feed height reference: where rapid stops and feed starts (default model_top)",
            ),
            Param::optional("feed_height_offset", OFFSET, "Feed height above its reference, mm (default 2)"),
            Param::optional("top_from", REFS, "Top height reference: where cutting starts (default model_top)"),
            Param::optional("top_offset", OFFSET, "Top height offset, mm (default 0)"),
            Param::optional("bottom_from", REFS, "Bottom height reference: the final depth (default selection)"),
            Param::optional("bottom_offset", OFFSET, "Bottom height offset, mm (default 0; negative goes deeper)"),
            // Passes
            Param::optional(
                "direction",
                ParamKind::Choice(DIRECTIONS),
                "Climb or conventional milling (default climb)",
            ),
            Param::optional(
                "both_ways",
                ParamKind::Bool,
                "Open contours: cut back and forth instead of returning to the start",
            ),
            Param::optional(
                "radial_passes",
                ParamKind::Integer { min: 1, max: 100 },
                "Number of side-by-side passes (default 1)",
            ),
            Param::optional(
                "radial_stepover",
                LENGTH,
                "Distance between radial passes, mm (default: the tool's stepover, else 40% of Ø)",
            ),
            Param::optional(
                "spring_passes",
                ParamKind::Integer { min: 0, max: 10 },
                "Repeat the final pass this many times (default 0)",
            ),
            Param::optional("stock_radial", STOCK, "Stock to leave on the walls, mm (default 0)"),
            Param::optional("stock_axial", STOCK, "Stock to leave on the floor, mm (default 0)"),
            Param::optional("multiple_depths", ParamKind::Bool, "Step down in several depths (default false)"),
            Param::optional(
                "max_stepdown",
                LENGTH,
                "Deepest step per depth, mm (default: the tool's stepdown, else Ø)",
            ),
            Param::optional(
                "feed_optimization",
                ParamKind::Bool,
                "Slow down at sharp direction changes (default false)",
            ),
            Param::optional("optimization_angle", ANGLE, "Direction change that counts as sharp, degrees (default 45)"),
            Param::optional(
                "optimization_feed",
                ParamKind::Number { min: 1.0, max: 100.0 },
                "Feed at sharp changes, % of the cutting feed (default 50)",
            ),
            Param::optional(
                "optimization_distance",
                LENGTH,
                "Distance before and after the change to slow down over, mm (default Ø/2)",
            ),
            Param::optional(
                "tolerance",
                ParamKind::Length { min: 0.0001, max: 1.0 },
                "How far the path may stray from true curves (circles, arcs, rounded corners), mm; smaller gives more, \
                 closer points (default 0.01)",
            ),
            // Linking
            Param::optional(
                "entry_from",
                ParamKind::Choice(ENTRIES),
                "Where closed contours start: auto (mid-way along the longest side), nearest the world origin, \
                 nearest the WCS origin, or nearest entry_point (selection)",
            ),
            Param::optional("entry_point", ParamKind::Vec3, "Entry point for entry_from selection, world mm"),
            Param::optional("lead_in", ParamKind::Choice(LEADS), "Lead-in style (default arc)"),
            Param::optional("lead_in_radius", LENGTH, "Lead-in arc radius, mm (default Ø/2)"),
            Param::optional("lead_in_sweep", ANGLE, "Lead-in arc sweep, degrees (default 90)"),
            Param::optional("lead_in_length", LENGTH, "Lead-in line length, mm (default Ø/2)"),
            Param::optional("lead_out_same", ParamKind::Bool, "Lead out the way the tool led in (default true)"),
            Param::optional("lead_out", ParamKind::Choice(LEADS), "Lead-out style"),
            Param::optional("lead_out_radius", LENGTH, "Lead-out arc radius, mm"),
            Param::optional("lead_out_sweep", ANGLE, "Lead-out arc sweep, degrees"),
            Param::optional("lead_out_length", LENGTH, "Lead-out line length, mm"),
            Param::optional(
                "ramp",
                ParamKind::Choice(RAMPS),
                "Go down by ramping along the lead-in or by plunging (default ramp)",
            ),
            Param::optional("ramp_angle", ParamKind::Number { min: 0.1, max: 90.0 }, "Ramp angle, degrees (default 3)"),
            Param::optional(
                "retract",
                ParamKind::Choice(RETRACTS),
                "Between passes: retract height (minimum), clearance height (full) or stay down",
            ),
            Param::optional(
                "stay_down_distance",
                LENGTH,
                "Stay down when the next pass starts within this distance, mm (default 2 × Ø)",
            ),
            Param::optional(
                "rapid_links",
                ParamKind::Bool,
                "Link at rapid traverse (default true) or at the link feed",
            ),
            Param::optional(
                "link_feed",
                RATE,
                "Feed for links when not rapid, mm/min (default: the machine's maximum)",
            ),
        ],
        returns: "the toolpath {id, name, strategy, machine, tool, wcs, moves, cut_length, rapid_length, time, bounds, warnings, params}",
        enabled: always,
        run: |s, p| {
            let replace = params::opt_str(p, "id")?.map(str::to_owned);
            if let Some(id) = &replace
                && s.job.toolpaths.iter().all(|t| &t.id != id)
            {
                return Err(EngineError::InvalidParams(format!("no toolpath `{id}`")));
            }
            if replace.is_none() && s.job.toolpaths.len() >= MAX_TOOLPATHS {
                return Err(EngineError::InvalidParams(format!("a job holds at most {MAX_TOOLPATHS} toolpaths")));
            }
            let id = replace.clone().unwrap_or_else(|| format!("toolpath.{}", s.job.next_toolpath));
            let tp = contour::generate(s, p, id.clone())?;
            let result = tp.to_json(rapid_rate(s));
            match replace {
                Some(id) => {
                    if let Some(slot) = s.job.toolpaths.iter_mut().find(|t| t.id == id) {
                        *slot = tp;
                    }
                }
                None => {
                    s.job.next_toolpath = s.job.next_toolpath.saturating_add(1);
                    s.job.toolpaths.push(tp);
                }
            }
            s.view.selection = Some(id);
            s.job.setup_changed();
            Ok(result)
        },
    },
    CommandSpec {
        id: "toolpath.list",
        label: "Toolpaths",
        tooltip: "The job's toolpaths",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "list of toolpaths, as toolpath.get returns them",
        enabled: always,
        run: |s, _| {
            let rate = rapid_rate(s);
            Ok(json!(s.job.toolpaths.iter().map(|t| t.to_json(rate)).collect::<Vec<_>>()))
        },
    },
    CommandSpec {
        id: "toolpath.get",
        label: "Toolpath",
        tooltip: "One toolpath, optionally with its moves",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("id", ParamKind::Str, "Toolpath id, e.g. toolpath.1"),
            Param::optional("moves", ParamKind::Bool, "Include the moves {kind, to, feed} (default false)"),
            Param::optional(
                "limit",
                ParamKind::Integer { min: 1, max: 1_000_000 },
                "Most moves to include (default 100000)",
            ),
        ],
        returns: "the toolpath, plus `move_list` when moves = true",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            let tp = find(s, id)?;
            let mut v = tp.to_json(rapid_rate(s));
            if params::opt_bool(p, "moves")?.unwrap_or(false) {
                let limit = p.get("limit").and_then(Value::as_u64).map_or(100_000, |n| n as usize);
                v["move_list"] = tp.moves_json(limit);
            }
            Ok(v)
        },
    },
    CommandSpec {
        id: "toolpath.edit",
        label: "Edit toolpath",
        tooltip: "The settings a toolpath was made with; in the app this opens it in the toolpath panel",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Toolpath id, e.g. toolpath.1")],
        returns: "the settings, as toolpath.contour takes them (pass them back with `id` to regenerate)",
        enabled: always,
        run: |s, p| Ok(find(s, params::req_str(p, "id")?)?.params.clone()),
    },
    CommandSpec {
        id: "toolpath.remove",
        label: "Delete toolpath",
        tooltip: "Delete a toolpath from the job",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Toolpath id, e.g. toolpath.1")],
        returns: "{\"removed\": id}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            find(s, id)?;
            s.job.toolpaths.retain(|t| t.id != id);
            if s.view.selection.as_deref() == Some(id) {
                s.view.selection = None;
            }
            s.job.setup_changed();
            Ok(json!({ "removed": id }))
        },
    },
];

fn find<'a>(s: &'a Session, id: &str) -> EngineResult<&'a Toolpath> {
    s.job.toolpaths.iter().find(|t| t.id == id).ok_or_else(|| EngineError::InvalidParams(format!("no toolpath `{id}`")))
}

/// Rapid traverse for time estimates: the active machine's maximum feed, if it is faster.
fn rapid_rate(s: &Session) -> f64 {
    s.library.active_machine().map_or(RAPID_RATE, |m| m.max_feed.max(RAPID_RATE))
}
