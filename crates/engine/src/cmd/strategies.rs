//! Toolpath strategy commands beyond the 2D contour: face, pocket, drill, 3D rough, waterline
//! and raster. Each adds a toolpath, or regenerates one when given its `id`.

use serde_json::Value;

use super::toolpath::add_toolpath;
use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::toolpath::contour::{COOLANTS, DIRECTIONS, HEIGHT_REFS};
use crate::toolpath::drill::{self, CYCLES};
use crate::toolpath::multiaxis::TOOL_AXES;
use crate::toolpath::strategies::{self, BOUNDARIES, PATTERNS};

const LENGTH: ParamKind = ParamKind::Length { min: 0.0, max: 10_000.0 };
const OFFSET: ParamKind = ParamKind::Length { min: -100_000.0, max: 100_000.0 };
const STOCK: ParamKind = ParamKind::Length { min: -1000.0, max: 1000.0 };
const RATE: ParamKind = ParamKind::Rate { min: 0.0, max: 1.0e6 };
const REFS: ParamKind = ParamKind::Choice(HEIGHT_REFS);
const ENTRY: &[&str] = &["helix", "plunge"];
const RETURNS: &str = "the toolpath {id, name, strategy, machine, tool, wcs, moves, cut_length, rapid_length, time, bounds, warnings, params}";

/// Parameters every strategy takes, then its own.
macro_rules! strategy_params {
    ($($extra:expr),* $(,)?) => {
        &[
            Param::optional("id", ParamKind::Str, "Regenerate this toolpath with these settings instead of adding one"),
            Param::optional("name", ParamKind::Str, "Name in the job outline (default: the strategy and tool)"),
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
            Param::optional("preset", ParamKind::Str, "One of the tool's cutting presets to take speed and feeds from (default: the tool's own)"),
            Param::optional("feed", RATE, "Cutting feed, mm/min (default: the tool's)"),
            Param::optional("plunge_feed", RATE, "Plunge and ramp feed, mm/min (default: the tool's, else half the feed)"),
            Param::optional("coolant", ParamKind::Choice(COOLANTS), "Coolant (default flood)"),
            Param::optional("wcs", ParamKind::Str, "WCS to program in (default: the active one)"),
            Param::optional("orientation", ParamKind::Str, "3+2: a WCS whose Z is the tool axis and whose XY is the work plane; the program stays in `wcs` (needs a 4- or 5-axis machine)"),
            Param::optional("post_stop", ParamKind::Bool, "Stop the machine (M0) before this operation"),
            Param::optional("post_comment", ParamKind::Str, "Note for the operator, written in the program"),
            Param::optional("post_code_before", ParamKind::Str, "G-code written as given before the operation"),
            Param::optional("post_code_after", ParamKind::Str, "G-code written as given after the operation"),
            Param::optional("geometry", ParamKind::StrList { max: 10_000 }, "Faces, edges or points the strategy works on (model.N/face.N, model.N/edge.N, model.N/vertex.N)"),
            Param::optional("clearance_from", REFS, "Clearance height reference"),
            Param::optional("clearance_offset", OFFSET, "Clearance height above its reference, mm"),
            Param::optional("retract_from", REFS, "Retract height reference"),
            Param::optional("retract_offset", OFFSET, "Retract height above its reference, mm"),
            Param::optional("feed_height_from", REFS, "Feed height reference: where rapids stop"),
            Param::optional("feed_height_offset", OFFSET, "Feed height above its reference, mm"),
            Param::optional("top_from", REFS, "Top height reference: where cutting starts"),
            Param::optional("top_offset", OFFSET, "Top height offset, mm"),
            Param::optional("bottom_from", REFS, "Bottom height reference: the final depth"),
            Param::optional("bottom_offset", OFFSET, "Bottom height offset, mm (negative goes deeper)"),
            Param::optional(
                "tolerance",
                ParamKind::Length { min: 0.0, max: 1.0 },
                "How far the path may stray from true curves, mm (default 0.01)",
            ),
            $($extra),*
        ]
    };
}

/// Passes shared by the milling strategies.
macro_rules! milling_params {
    ($($extra:expr),* $(,)?) => {
        strategy_params![
            Param::optional("stepover", LENGTH, "Distance between passes, mm (default: a share of the tool diameter)"),
            Param::optional("stepdown", LENGTH, "Depth per level, mm (default: the tool's, else a share of its diameter)"),
            Param::optional("direction", ParamKind::Choice(DIRECTIONS), "Climb or conventional milling (default climb)"),
            Param::optional("stock_radial", STOCK, "Stock to leave on walls, mm (default 0)"),
            Param::optional("stock_axial", STOCK, "Stock to leave on floors, mm (default 0)"),
            Param::optional("entry", ParamKind::Choice(ENTRY), "Into the material: a helical ramp or a plunge (default helix)"),
            Param::optional("ramp_angle", ParamKind::Number { min: 0.1, max: 90.0 }, "Helix ramp angle, degrees (default 2)"),
            Param::optional("helix_radius", LENGTH, "Helix radius, mm (default 0.4 × Ø, at most the tool radius)"),
            Param::optional("rapid_links", ParamKind::Bool, "Link between passes at rapid traverse (default true)"),
            $($extra),*
        ]
    };
}

fn run_face(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, strategies::face)
}
fn run_pocket(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, strategies::pocket)
}
fn run_drill(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, drill::drill)
}
fn run_rough(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, strategies::rough)
}
fn run_waterline(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, strategies::waterline)
}
fn run_raster(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, strategies::raster)
}

fn run_adaptive(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, crate::toolpath::adaptive::adaptive)
}

fn run_scallop(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, crate::toolpath::finish::scallop)
}
fn run_spiral(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, crate::toolpath::finish::spiral)
}
fn run_radial(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, crate::toolpath::finish::radial)
}
fn run_flat(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, crate::toolpath::finish::flat)
}
fn run_pencil(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, crate::toolpath::finish::pencil)
}
fn run_steep_shallow(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, crate::toolpath::finish::steep_shallow)
}

fn run_thread(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, crate::toolpath::twod::thread)
}

fn run_bore(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, crate::toolpath::bore::bore)
}

fn run_multiaxis(s: &mut crate::session::Session, p: &Value) -> crate::error::EngineResult<Value> {
    add_toolpath(s, p, crate::toolpath::multiaxis::multiaxis)
}

const DEGREES: ParamKind = ParamKind::Number { min: -89.0, max: 89.0 };

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "toolpath.face",
        label: "Face",
        tooltip: "Rows across the top of the stock to bring it flat and down to height",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "2D")),
        icon: None,
        shortcut: None,
        params: milling_params![
            Param::optional(
                "boundary",
                ParamKind::Choice(BOUNDARIES),
                "Area to face: the stock, the model or the selection (default stock)"
            ),
            Param::optional("boundary_offset", OFFSET, "Grow (or shrink) the area by this, mm (default 0)"),
            Param::optional(
                "overrun",
                LENGTH,
                "How far rows run past the area, beyond the tool radius, mm (default Ø/4)"
            ),
            Param::optional(
                "angle",
                ParamKind::Number { min: -360.0, max: 360.0 },
                "Row direction, degrees from +X (default 0)"
            ),
            Param::optional("pattern", ParamKind::Choice(PATTERNS), "Back and forth, or all one way (default zigzag)"),
            Param::optional("multiple_depths", ParamKind::Bool, "Step down in several levels (default false)"),
        ],
        returns: RETURNS,
        enabled: always,
        run: run_face,
    },
    CommandSpec {
        id: "toolpath.pocket",
        label: "Pocket",
        tooltip: "Clear the inside of selected floor faces or closed edges, around any islands",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "2D")),
        icon: None,
        shortcut: None,
        params: milling_params![
            Param::optional("multiple_depths", ParamKind::Bool, "Step down in several levels (default true)"),
            Param::optional(
                "finishing_pass",
                ParamKind::Bool,
                "Finish the walls to size after leaving radial stock (default false)"
            ),
        ],
        returns: RETURNS,
        enabled: always,
        run: run_pocket,
    },
    CommandSpec {
        id: "toolpath.adaptive",
        label: "Adaptive",
        tooltip: "Rough with a steady tool load: deep cuts at a light stepover that never bury the tool in corners; 3D around the model or 2D in selected pockets",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "3D")),
        icon: None,
        shortcut: None,
        params: milling_params![
            Param::optional(
                "boundary",
                ParamKind::Choice(&["stock", "selection"]),
                "What to clear: the stock around the model (3D, default) or the selected pockets (2D, `geometry`)"
            ),
            Param::optional(
                "rest",
                ParamKind::Choice(crate::toolpath::adaptive::REST),
                "Rest machining: `previous` clears only what the toolpaths before this one leave (default none)"
            ),
            Param::optional(
                "wall_pass",
                ParamKind::Bool,
                "Finish each level with a pass along the walls (default true)"
            ),
            Param::optional(
                "keep_down",
                ParamKind::Bool,
                "Link between passes at depth where the way is clear, instead of lifting (default true)"
            ),
            Param::optional(
                "stay_down_distance",
                LENGTH,
                "Longest link at depth, routed through cleared ground; farther, the tool lifts, mm (default 20 × Ø)"
            ),
            Param::optional(
                "flat_levels",
                ParamKind::Bool,
                "Add levels at the model's flat areas so shelves are cleared at their height (default true)"
            ),
        ],
        returns: RETURNS,
        enabled: always,
        run: run_adaptive,
    },
    CommandSpec {
        id: "toolpath.scallop",
        label: "Scallop",
        tooltip: "Finish with passes a constant stepover apart measured over the surface, so steep and shallow areas get the same finish",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "3D")),
        icon: None,
        shortcut: None,
        params: milling_params![
            Param::optional(
                "boundary",
                ParamKind::Choice(BOUNDARIES),
                "Area to finish: the stock, the model (default) or the selected faces (`geometry`)"
            ),
            Param::optional("boundary_offset", OFFSET, "Grow (or shrink, negative) the area, mm"),
            Param::optional(
                "sample_step",
                LENGTH,
                "Spacing of points dropped onto the model along each pass, mm (default Ø/8)"
            ),
            Param::optional(
                "stay_down_distance",
                LENGTH,
                "Longest link along the surface between passes; farther, the tool lifts, mm (default 2 × Ø)"
            ),
        ],
        returns: RETURNS,
        enabled: always,
        run: run_scallop,
    },
    CommandSpec {
        id: "toolpath.steep_shallow",
        label: "Steep and shallow",
        tooltip: "Finish steep walls with level passes and shallow areas with scallop passes, split at a slope angle",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "3D")),
        icon: None,
        shortcut: None,
        params: milling_params![
            Param::optional(
                "boundary",
                ParamKind::Choice(BOUNDARIES),
                "Area to finish: the stock, the model (default) or the selected faces (`geometry`)"
            ),
            Param::optional("boundary_offset", OFFSET, "Grow (or shrink, negative) the area, mm"),
            Param::optional(
                "sample_step",
                LENGTH,
                "Spacing of points dropped onto the model along each pass, mm (default Ø/8)"
            ),
            Param::optional(
                "stay_down_distance",
                LENGTH,
                "Longest link along the surface between passes; farther, the tool lifts, mm (default 2 × Ø)"
            ),
            Param::optional(
                "threshold_angle",
                ParamKind::Number { min: 1.0, max: 89.0 },
                "Slope from horizontal, degrees, that divides steep from shallow (default 40)"
            ),
        ],
        returns: RETURNS,
        enabled: always,
        run: run_steep_shallow,
    },
    CommandSpec {
        id: "toolpath.pencil",
        label: "Pencil",
        tooltip: "Clean out concave corners with one pass of a ball-ended tool where it touches two faces at once",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "3D")),
        icon: None,
        shortcut: None,
        params: milling_params![
            Param::optional(
                "boundary",
                ParamKind::Choice(BOUNDARIES),
                "Area to finish: the stock, the model (default) or the selected faces (`geometry`)"
            ),
            Param::optional("boundary_offset", OFFSET, "Grow (or shrink, negative) the area, mm"),
            Param::optional(
                "sample_step",
                LENGTH,
                "Spacing of points dropped onto the model along each pass, mm (default Ø/8)"
            ),
            Param::optional(
                "stay_down_distance",
                LENGTH,
                "Longest link along the surface between passes; farther, the tool lifts, mm (default 2 × Ø)"
            ),
            Param::optional("min_length", LENGTH, "Shortest corner worth a pass, mm (default the tool radius)"),
        ],
        returns: RETURNS,
        enabled: always,
        run: run_pencil,
    },
    CommandSpec {
        id: "toolpath.spiral",
        label: "Spiral",
        tooltip: "Finish round areas with one pass spiralling out from (or in to) a centre",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "3D")),
        icon: None,
        shortcut: None,
        params: milling_params![
            Param::optional(
                "boundary",
                ParamKind::Choice(BOUNDARIES),
                "Area to finish: the stock, the model (default) or the selected faces (`geometry`)"
            ),
            Param::optional("boundary_offset", OFFSET, "Grow (or shrink, negative) the area, mm"),
            Param::optional(
                "sample_step",
                LENGTH,
                "Spacing of points dropped onto the model along each pass, mm (default Ø/8)"
            ),
            Param::optional(
                "stay_down_distance",
                LENGTH,
                "Longest link along the surface between passes; farther, the tool lifts, mm (default 2 × Ø)"
            ),
            Param::optional("center_x", OFFSET, "Spiral centre X in the WCS, mm (default the area's middle)"),
            Param::optional("center_y", OFFSET, "Spiral centre Y in the WCS, mm (default the area's middle)"),
            Param::optional(
                "spiral_direction",
                ParamKind::Choice(&["outward", "inward"]),
                "From the centre out (default) or in to it"
            ),
        ],
        returns: RETURNS,
        enabled: always,
        run: run_spiral,
    },
    CommandSpec {
        id: "toolpath.radial",
        label: "Radial",
        tooltip: "Finish with passes radiating from a centre, back and forth",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "3D")),
        icon: None,
        shortcut: None,
        params: milling_params![
            Param::optional(
                "boundary",
                ParamKind::Choice(BOUNDARIES),
                "Area to finish: the stock, the model (default) or the selected faces (`geometry`)"
            ),
            Param::optional("boundary_offset", OFFSET, "Grow (or shrink, negative) the area, mm"),
            Param::optional(
                "sample_step",
                LENGTH,
                "Spacing of points dropped onto the model along each pass, mm (default Ø/8)"
            ),
            Param::optional(
                "stay_down_distance",
                LENGTH,
                "Longest link along the surface between passes; farther, the tool lifts, mm (default 2 × Ø)"
            ),
            Param::optional("center_x", OFFSET, "Centre X in the WCS, mm (default the area's middle)"),
            Param::optional("center_y", OFFSET, "Centre Y in the WCS, mm (default the area's middle)"),
            Param::optional("inner_radius", LENGTH, "Where the spokes start, mm (default the stepover)"),
        ],
        returns: RETURNS,
        enabled: always,
        run: run_radial,
    },
    CommandSpec {
        id: "toolpath.flat",
        label: "Flat",
        tooltip: "Finish the model's flat floors at their exact heights (all of them, or those of the picked faces)",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "3D")),
        icon: None,
        shortcut: None,
        params: milling_params![],
        returns: RETURNS,
        enabled: always,
        run: run_flat,
    },
    CommandSpec {
        id: "toolpath.drill",
        label: "Drill",
        tooltip: "Drill, spot, peck, tap or bore holes picked as cylinders, circular edges or points",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "Drilling")),
        icon: None,
        shortcut: None,
        params: strategy_params![
            Param::optional("cycle", ParamKind::Choice(CYCLES), "Drilling cycle (default drill)"),
            Param::optional("peck_depth", LENGTH, "Depth per peck for peck and chip break, mm (default Ø)"),
            Param::optional(
                "dwell",
                ParamKind::Number { min: 0.0, max: 60.0 },
                "Pause at the bottom, seconds (default 0.5 for spot, else 0)"
            ),
            Param::optional(
                "pitch",
                ParamKind::Length { min: 0.0, max: 100.0 },
                "Thread pitch for tapping, mm per turn"
            ),
        ],
        returns: RETURNS,
        enabled: always,
        run: run_drill,
    },
    CommandSpec {
        id: "toolpath.thread",
        label: "Thread",
        tooltip: "Mill internal or external threads in holes or on bosses picked as cylinders or circular edges",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "Drilling")),
        icon: None,
        shortcut: None,
        params: strategy_params![
            Param::optional(
                "pitch",
                ParamKind::Length { min: 0.0, max: 100.0 },
                "Thread pitch, mm per turn (default: the thread mill's)"
            ),
            Param::optional(
                "thread_type",
                ParamKind::Choice(crate::toolpath::twod::THREAD_TYPES),
                "Internal (in a hole, default) or external (on a boss)"
            ),
            Param::optional(
                "hand",
                ParamKind::Choice(crate::toolpath::twod::HANDS),
                "Right-hand (default) or left-hand thread"
            ),
            Param::optional(
                "major_diameter",
                LENGTH,
                "Thread major diameter, mm (default: a hole's diameter plus 1.0825 × pitch, a boss's diameter)"
            ),
            Param::optional(
                "radial_passes",
                ParamKind::Integer { min: 1, max: 20 },
                "Passes to full depth (default 1)"
            ),
            Param::optional("spring_pass", ParamKind::Bool, "Repeat the last pass (default false)"),
            Param::optional("direction", ParamKind::Choice(DIRECTIONS), "Climb (default) or conventional"),
        ],
        returns: RETURNS,
        enabled: always,
        run: run_thread,
    },
    CommandSpec {
        id: "toolpath.bore",
        label: "Helical bore",
        tooltip: "Open up holes with an end mill spiralling down inside them, then a full circle at the bottom",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "Drilling")),
        icon: None,
        shortcut: None,
        params: strategy_params![
            Param::optional(
                "pitch",
                ParamKind::Length { min: 0.0, max: 1000.0 },
                "Depth per turn of the helix, mm (default 15% of the tool diameter)"
            ),
            Param::optional(
                "stepover",
                LENGTH,
                "Widest step between helices in a hole wider than one clears, mm (default the tool's, else 40% Ø)"
            ),
            Param::optional("stock_radial", STOCK, "Stock to leave on the wall, mm (default 0)"),
            Param::optional(
                "direction",
                ParamKind::Choice(DIRECTIONS),
                "Climb or conventional milling (default climb)"
            ),
        ],
        returns: RETURNS,
        enabled: always,
        run: run_bore,
    },
    CommandSpec {
        id: "toolpath.rough",
        label: "3D rough",
        tooltip: "Clear the stock around the part level by level, leaving stock for finishing",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "3D")),
        icon: None,
        shortcut: None,
        params: milling_params![Param::optional(
            "angle",
            ParamKind::Number { min: -360.0, max: 360.0 },
            "Row direction, degrees from +X (default 0)"
        )],
        returns: RETURNS,
        enabled: always,
        run: run_rough,
    },
    CommandSpec {
        id: "toolpath.waterline",
        label: "Waterline",
        tooltip: "Finish steep walls with passes around the part at constant heights",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "3D")),
        icon: None,
        shortcut: None,
        params: milling_params![],
        returns: RETURNS,
        enabled: always,
        run: run_waterline,
    },
    CommandSpec {
        id: "toolpath.raster",
        label: "Raster",
        tooltip: "Finish shallow surfaces with parallel rows that follow the part",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "3D")),
        icon: None,
        shortcut: None,
        params: milling_params![
            Param::optional(
                "boundary",
                ParamKind::Choice(BOUNDARIES),
                "Area to cover: the stock, the model or the selection (default model)"
            ),
            Param::optional("boundary_offset", OFFSET, "Grow (or shrink) the area by this, mm (default 0)"),
            Param::optional(
                "angle",
                ParamKind::Number { min: -360.0, max: 360.0 },
                "Row direction, degrees from +X (default 0)"
            ),
            Param::optional("pattern", ParamKind::Choice(PATTERNS), "Back and forth, or all one way (default zigzag)"),
            Param::optional(
                "sample_step",
                ParamKind::Length { min: 0.0, max: 1000.0 },
                "Spacing of points along a row, mm (default Ø/8, 0.05–1)"
            ),
        ],
        returns: RETURNS,
        enabled: always,
        run: run_raster,
    },
    CommandSpec {
        id: "toolpath.multiaxis",
        label: "Multi-axis finish",
        tooltip: "Finish the part with a ball end mill that tilts as it follows the surface (simultaneous 4/5-axis)",
        placement: Some(Placement::gallery(RibbonTab::Toolpaths, "Multi-axis")),
        icon: None,
        shortcut: None,
        params: milling_params![
            Param::optional(
                "boundary",
                ParamKind::Choice(BOUNDARIES),
                "Area to cover: the stock, the model or the selection (default model)"
            ),
            Param::optional("boundary_offset", OFFSET, "Grow (or shrink) the area by this, mm (default 0)"),
            Param::optional(
                "angle",
                ParamKind::Number { min: -360.0, max: 360.0 },
                "Row direction, degrees from +X (default 0)"
            ),
            Param::optional("pattern", ParamKind::Choice(PATTERNS), "Back and forth, or all one way (default zigzag)"),
            Param::optional(
                "sample_step",
                ParamKind::Length { min: 0.0, max: 1000.0 },
                "Spacing of points along a row, mm (default Ø/8, 0.05–1)"
            ),
            Param::optional(
                "tool_axis",
                ParamKind::Choice(TOOL_AXES),
                "How the tool leans: tilted from Z, from the surface normal, towards or away from a point or line, or fixed (default tilt)"
            ),
            Param::optional(
                "tilt_angle",
                DEGREES,
                "Sideways lean, degrees: positive leans the tool's top to the right of the row direction (default 15 for tilt, else 0)"
            ),
            Param::optional(
                "lead_angle",
                DEGREES,
                "Lean along the travel, degrees: positive leans the top forward (default 0)"
            ),
            Param::optional("axis_point", ParamKind::Vec3, "The point (WCS mm) for toward/away from a point or line"),
            Param::optional("axis_direction", ParamKind::Vec3, "The line's direction, or the fixed tool axis (WCS)"),
            Param::optional(
                "max_tilt",
                ParamKind::Number { min: 0.0, max: 120.0 },
                "Most the tool may lean from the toolpath's Z, degrees (default 60)"
            ),
            Param::optional(
                "max_axis_rate",
                ParamKind::Number { min: 0.01, max: 180.0 },
                "Fastest the tool axis may turn, degrees per mm of travel (default 5)"
            ),
            Param::optional(
                "avoid_collisions",
                ParamKind::Bool,
                "Lean the tool back towards Z where its flutes or shank would touch the part (default true)"
            ),
            Param::optional(
                "shank_clearance",
                ParamKind::Length { min: 0.0, max: 100.0 },
                "Gap kept around the shank, mm (default 0.5)"
            ),
        ],
        returns: RETURNS,
        enabled: always,
        run: run_multiaxis,
    },
];
