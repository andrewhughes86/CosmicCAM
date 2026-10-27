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

/// The 2D contour's parameters, then a variant's own.
macro_rules! contour_params {
    ($($extra:expr),* $(,)?) => {
        &[
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
                Param::optional(
                    "preset",
                    ParamKind::Str,
                    "One of the tool's cutting presets to take speed and feeds from (default: the tool's own)",
                ),
                Param::optional("feed", RATE, "Cutting feed, mm/min (default: the tool's)"),
                Param::optional(
                    "plunge_feed",
                    RATE,
                    "Plunge and ramp feed, mm/min (default: the tool's, else half the feed)",
                ),
                Param::optional("lead_feed", RATE, "Lead-in and lead-out feed, mm/min (default: the cutting feed)"),
                Param::optional("coolant", ParamKind::Choice(COOLANTS), "Coolant (default flood)"),
                Param::optional("post_stop", ParamKind::Bool, "Stop the machine (M0) before this operation"),
                Param::optional("post_comment", ParamKind::Str, "Note for the operator, written in the program"),
                Param::optional("post_code_before", ParamKind::Str, "G-code written as given before the operation"),
                Param::optional("post_code_after", ParamKind::Str, "G-code written as given after the operation"),
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
                Param::optional(
                    "orientation",
                    ParamKind::Str,
                    "3+2: a WCS whose Z is the tool axis and whose XY is the work plane; the program stays in `wcs` (needs a 4- or 5-axis machine)",
                ),
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
            $($extra),*
        ]
    };
}

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "toolpath.contour",
        label: "2D contour",
        tooltip: "Follow selected edges or a face outline at depth, offset by the tool radius",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "2D")),
        icon: Some("toolpath_profile"),
        shortcut: None,
        params: contour_params![],
        returns: "the toolpath {id, name, strategy, machine, tool, wcs, moves, cut_length, rapid_length, time, bounds, warnings, params}",
        enabled: always,
        run: |s, p| add_toolpath(s, p, contour::generate),
    },
    CommandSpec {
        id: "toolpath.chamfer",
        label: "Chamfer",
        tooltip: "Break picked edges with a chamfer mill: the chamfer width and the tip offset set the depth and offset from the tool's angle",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "2D")),
        icon: None,
        shortcut: None,
        params: contour_params![
            Param::optional("chamfer_width", LENGTH, "Width of the chamfer across each face, mm (default 0.5)"),
            Param::optional(
                "tip_offset",
                LENGTH,
                "How far the tool's tip goes below the chamfer, so the tip itself does not cut, mm (default 0.25)"
            ),
        ],
        returns: "the toolpath {id, name, strategy, machine, tool, wcs, moves, cut_length, rapid_length, time, bounds, warnings, params}",
        enabled: always,
        run: |s, p| add_toolpath(s, p, crate::toolpath::twod::chamfer),
    },
    CommandSpec {
        id: "toolpath.slot",
        label: "Slot",
        tooltip: "Cut a slot with the tool's centre along picked edges, down in steps",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "2D")),
        icon: None,
        shortcut: None,
        params: contour_params![],
        returns: "the toolpath {id, name, strategy, machine, tool, wcs, moves, cut_length, rapid_length, time, bounds, warnings, params}",
        enabled: always,
        run: |s, p| add_toolpath(s, p, crate::toolpath::twod::slot),
    },
    CommandSpec {
        id: "toolpath.trace",
        label: "Trace",
        tooltip: "Run the tool's centre along picked edges at one depth: engraving, scribing, marking out",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "2D")),
        icon: None,
        shortcut: None,
        params: contour_params![Param::optional(
            "depth",
            LENGTH,
            "Depth below the picked edges, mm (default 0.2; a bottom height overrides it)"
        ),],
        returns: "the toolpath {id, name, strategy, machine, tool, wcs, moves, cut_length, rapid_length, time, bounds, warnings, params}",
        enabled: always,
        run: |s, p| add_toolpath(s, p, crate::toolpath::twod::trace),
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
        id: "toolpath.visibility.set",
        label: "Show or hide toolpath",
        tooltip: "Draw a toolpath in the viewport even when it is not selected, or stop drawing it",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("id", ParamKind::Str, "Toolpath id, e.g. toolpath.1"),
            Param::optional("visible", ParamKind::Bool, "Show it (true) or hide it (false); omit to toggle"),
        ],
        returns: "{\"id\": id, \"visible\": bool}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?.to_owned();
            let want = params::opt_bool(p, "visible")?;
            let tp = s
                .job
                .toolpaths
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or_else(|| EngineError::InvalidParams(format!("no toolpath `{id}`")))?;
            tp.visible = want.unwrap_or(!tp.visible);
            let visible = tp.visible;
            s.job.setup_changed();
            Ok(json!({ "id": id, "visible": visible }))
        },
    },
    CommandSpec {
        id: "toolpath.rename",
        label: "Rename toolpath",
        tooltip: "Change a toolpath's name",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("id", ParamKind::Str, "Toolpath id, e.g. toolpath.1"),
            Param::required("name", ParamKind::Str, "New name"),
        ],
        returns: "{\"id\": id, \"name\": name}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?.to_owned();
            let name = crate::library::clip(params::req_str(p, "name")?);
            if name.is_empty() {
                return Err(EngineError::InvalidParams("give the toolpath a name".into()));
            }
            let tp = s
                .job
                .toolpaths
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or_else(|| EngineError::InvalidParams(format!("no toolpath `{id}`")))?;
            tp.name = name.clone();
            if let Some(obj) = tp.params.as_object_mut() {
                obj.insert("name".into(), json!(name));
            }
            s.job.setup_changed();
            Ok(json!({ "id": id, "name": name }))
        },
    },
    CommandSpec {
        id: "toolpath.move",
        label: "Move toolpath",
        tooltip: "Move a toolpath earlier or later in the program",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("id", ParamKind::Str, "Toolpath id, e.g. toolpath.1"),
            Param::optional(
                "by",
                ParamKind::Integer { min: -10_000, max: 10_000 },
                "Places to move: negative is earlier (default -1)",
            ),
            Param::optional(
                "to",
                ParamKind::Integer { min: 0, max: 10_000 },
                "Position to move to, 0 first (instead of `by`)",
            ),
        ],
        returns: "{\"id\": id, \"index\": its new position}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?.to_owned();
            let from = s
                .job
                .toolpaths
                .iter()
                .position(|t| t.id == id)
                .ok_or_else(|| EngineError::InvalidParams(format!("no toolpath `{id}`")))?;
            let last = s.job.toolpaths.len().saturating_sub(1);
            let to = match p.get("to").and_then(Value::as_u64) {
                Some(t) => (t as usize).min(last),
                None => {
                    let by = p.get("by").and_then(Value::as_i64).unwrap_or(-1);
                    (from as i64 + by).clamp(0, last as i64) as usize
                }
            };
            if to != from {
                let tp = s.job.toolpaths.remove(from);
                s.job.toolpaths.insert(to, tp);
                s.job.setup_changed();
            }
            Ok(json!({ "id": id, "index": to }))
        },
    },
    CommandSpec {
        id: "toolpath.regenerate",
        label: "Regenerate toolpaths",
        tooltip: "Work toolpaths out again with their settings, after the model, stock or tools changed",
        placement: Some(Placement::large(RibbonTab::Toolpaths, "Output")),
        icon: None,
        shortcut: None,
        params: &[Param::optional("ids", ParamKind::StrList { max: 512 }, "Toolpaths to regenerate (default all)")],
        returns: "{\"regenerated\": [ids], \"failed\": [{id, error}]}",
        enabled: |s| !s.job.toolpaths.is_empty(),
        run: |s, p| {
            let wanted = params::str_list(p, "ids")?;
            let ids: Vec<String> = s
                .job
                .toolpaths
                .iter()
                .map(|t| t.id.clone())
                .filter(|id| wanted.is_empty() || wanted.contains(id))
                .collect();
            let mut done = Vec::new();
            let mut failed = Vec::new();
            for id in ids {
                let Some(tp) = s.job.toolpaths.iter().find(|t| t.id == id) else { continue };
                let command = crate::toolpath::strategy_command(&tp.strategy);
                let mut params = tp.params.clone();
                if let Some(o) = params.as_object_mut() {
                    o.insert("id".into(), json!(id));
                    o.insert("name".into(), json!(tp.name));
                }
                match s.execute(command, &params) {
                    Ok(_) => done.push(id),
                    Err(e) => failed.push(json!({ "id": id, "error": e.to_string() })),
                }
            }
            Ok(json!({ "regenerated": done, "failed": failed }))
        },
    },
    CommandSpec {
        id: "toolpath.template.save",
        label: "Save as template",
        tooltip: "Save a toolpath's settings (not its geometry) under a name, to start new toolpaths from",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("id", ParamKind::Str, "Toolpath id, e.g. toolpath.1"),
            Param::required("name", ParamKind::Str, "Template name; one of the same name is replaced"),
        ],
        returns: "the template {name, command, params}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            let name: String = crate::library::clip(params::req_str(p, "name")?.trim());
            if name.is_empty() {
                return Err(EngineError::InvalidParams("give the template a name".into()));
            }
            let tp = s
                .job
                .toolpaths
                .iter()
                .find(|t| t.id == id)
                .ok_or_else(|| EngineError::InvalidParams(format!("no toolpath `{id}`")))?;
            let mut params = tp.params.clone();
            if let Some(o) = params.as_object_mut() {
                for k in ["id", "geometry", "name", "entry_point", "entry_from"] {
                    o.remove(k);
                }
            }
            let template = crate::library::Template {
                name: name.clone(),
                command: crate::toolpath::strategy_command(&tp.strategy).into(),
                params,
            };
            let lib = &mut s.library;
            lib.templates.retain(|t| t.name != name);
            if lib.templates.len() >= crate::library::MAX_TEMPLATES {
                return Err(EngineError::InvalidParams(format!(
                    "at most {} templates are kept",
                    crate::library::MAX_TEMPLATES
                )));
            }
            lib.templates.push(template.clone());
            lib.templates.sort_by_key(|t| t.name.to_lowercase());
            lib.changed();
            Ok(json!(template))
        },
    },
    CommandSpec {
        id: "toolpath.template.list",
        label: "Toolpath templates",
        tooltip: "The saved toolpath templates",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::optional(
            "command",
            ParamKind::Str,
            "Only templates for this strategy command, e.g. toolpath.adaptive",
        )],
        returns: "list of {name, command, label, params}",
        enabled: always,
        run: |s, p| {
            let only = params::opt_str(p, "command")?;
            Ok(json!(
                s.library
                    .templates
                    .iter()
                    .filter(|t| only.is_none_or(|c| t.command == c))
                    .map(|t| json!({
                        "name": t.name,
                        "command": t.command,
                        "label": super::find(&t.command).map_or("", |c| c.label),
                        "params": t.params,
                    }))
                    .collect::<Vec<_>>()
            ))
        },
    },
    CommandSpec {
        id: "toolpath.template.apply",
        label: "New toolpath from template",
        tooltip: "Make a toolpath from a saved template, on the geometry given",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("name", ParamKind::Str, "Template name"),
            Param::optional(
                "geometry",
                ParamKind::StrList { max: 10_000 },
                "Faces, edges or points to work on, as the strategy takes them",
            ),
            Param::optional(
                "tool",
                ParamKind::Integer { min: 1, max: 9999 },
                "Use this tool instead of the template's",
            ),
            Param::optional("wcs", ParamKind::Str, "Program in this WCS instead of the template's"),
        ],
        returns: "the toolpath {id, name, strategy, …}",
        enabled: always,
        run: |s, p| {
            let name = params::req_str(p, "name")?;
            let t = s
                .library
                .templates
                .iter()
                .find(|t| t.name == name)
                .cloned()
                .ok_or_else(|| EngineError::InvalidParams(format!("no template `{name}`")))?;
            let mut params = t.params.clone();
            if let Some(o) = params.as_object_mut() {
                for key in ["geometry", "tool", "wcs"] {
                    if let Some(v) = p.get(key) {
                        o.insert(key.into(), v.clone());
                    }
                }
            }
            s.execute(&t.command, &params)
        },
    },
    CommandSpec {
        id: "toolpath.template.remove",
        label: "Remove template",
        tooltip: "Delete a saved toolpath template",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("name", ParamKind::Str, "Template name")],
        returns: "{\"removed\": name}",
        enabled: always,
        run: |s, p| {
            let name = params::req_str(p, "name")?.to_owned();
            let before = s.library.templates.len();
            s.library.templates.retain(|t| t.name != name);
            if s.library.templates.len() == before {
                return Err(EngineError::InvalidParams(format!("no template `{name}`")));
            }
            s.library.changed();
            Ok(json!({ "removed": name }))
        },
    },
    CommandSpec {
        id: "toolpath.duplicate",
        label: "Duplicate toolpath",
        tooltip: "Copy a toolpath with its settings, to change one of them",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Toolpath id, e.g. toolpath.1")],
        returns: "the copy, as toolpath.get returns it",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            if s.job.toolpaths.len() >= MAX_TOOLPATHS {
                return Err(EngineError::InvalidParams(format!("a job holds at most {MAX_TOOLPATHS} toolpaths")));
            }
            let mut copy = find(s, id)?.clone();
            copy.id = format!("toolpath.{}", s.job.next_toolpath);
            copy.name = crate::library::clip(&format!("{} (copy)", copy.name));
            if let Some(obj) = copy.params.as_object_mut() {
                obj.insert("name".into(), json!(copy.name));
            }
            s.job.next_toolpath = s.job.next_toolpath.saturating_add(1);
            let result = copy.to_json(rapid_rate(s));
            s.view.selection = Some(copy.id.clone());
            s.job.toolpaths.push(copy);
            s.job.setup_changed();
            Ok(result)
        },
    },
    CommandSpec {
        id: "toolpath.remove",
        label: "Delete toolpath",
        tooltip: "Delete toolpaths from the job",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::optional("id", ParamKind::Str, "Toolpath id, e.g. toolpath.1"),
            Param::optional("ids", ParamKind::StrList { max: 512 }, "Several toolpath ids, deleted together"),
        ],
        returns: "{\"removed\": id}, or {\"removed\": [ids]} for `ids`",
        enabled: always,
        run: |s, p| {
            let single = params::opt_str(p, "id")?.map(str::to_owned);
            let many = params::str_list(p, "ids")?;
            let ids: Vec<String> = single.iter().cloned().chain(many.iter().cloned()).collect();
            if ids.is_empty() {
                return Err(EngineError::InvalidParams("give the toolpath `id` (or `ids`)".into()));
            }
            for id in &ids {
                find(s, id)?;
            }
            s.job.toolpaths.retain(|t| !ids.contains(&t.id));
            if s.view.selection.as_ref().is_some_and(|sel| ids.contains(sel)) {
                s.view.selection = None;
            }
            s.job.setup_changed();
            Ok(match (single, many.is_empty()) {
                (Some(id), true) => json!({ "removed": id }),
                _ => json!({ "removed": ids }),
            })
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

/// Runs a strategy and adds its toolpath, or replaces the one `id` names.
pub(super) fn add_toolpath(
    s: &mut Session,
    p: &Value,
    generate: fn(&Session, &Value, String) -> EngineResult<Toolpath>,
) -> EngineResult<Value> {
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
    let tp = generate(s, p, id.clone())?;
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
}
