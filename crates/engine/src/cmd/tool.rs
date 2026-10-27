//! Cutting tools in a machine's tool library: every tool type (each with the geometry it needs),
//! edit, copy, delete, list, cutting data presets, tool holders, the built-in sample library, and
//! import or export as JSON.

use serde_json::{Value, json};

use super::machine::{machine_id, missing as missing_machine};
use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::error::{EngineError, EngineResult};
use crate::library::{MAX_TOOL_NUMBER, MAX_TOOLS, Machine, TOOL_MATERIALS, TOOL_TYPES, Tool, clip, tool_ref};
use crate::params;
use crate::session::{Pane, Session, UiRequest};
use crate::tooling::{
    HANDS, Holder, HolderSegment, MAX_HOLDER_SEGMENTS, MAX_HOLDERS, MAX_PRESETS, Preset, WORKPIECES, tool_type,
};

const TOOL_RETURNS: &str = "the tool {id, machine, number, name, type, type_label, diameter, corner_radius, flutes, \
                            flute_length, overall_length, shank_diameter, tip_angle, tip_diameter, tip_length, \
                            taper_angle, neck_diameter, shoulder_length, thread_pitch, hand, coolant_through, \
                            material, coating, vendor, product_id, holder, spindle_rpm, feed, plunge_feed, stepover, \
                            stepdown, presets, length_offset, diameter_offset, notes}";
const HOLDER_RETURNS: &str =
    "the holder {id, name, vendor, product_id, segments: [{lower_diameter, upper_diameter, height}], height}";
const TOOLS_FORMAT: &str = "CosmicDMS-tools/1";
/// Tool files written before the rename (CosmicCAM), still read.
const LEGACY_TOOLS_FORMAT: &str = "CosmicCAM-tools/1";

/// Every tool type, offered from the New tool button's menu.
const TOOL_MENU: &[&str] = &[
    "tool.create.flat",
    "tool.create.ball",
    "tool.create.bull",
    "tool.create.tapered_ball",
    "tool.create.lollipop",
    "tool.create.chamfer",
    "tool.create.engraving",
    "tool.create.dovetail",
    "tool.create.slot_mill",
    "tool.create.radius_mill",
    "tool.create.face_mill",
    "tool.create.thread_mill",
    "tool.create.drill",
    "tool.create.spot_drill",
    "tool.create.center_drill",
    "tool.create.reamer",
    "tool.create.tap",
    "tool.create.boring_bar",
    "tool.create.counterbore",
    "tool.create.countersink",
];

const NUMBER: ParamKind = ParamKind::Integer { min: 1, max: MAX_TOOL_NUMBER as i64 };
const OFFSET: ParamKind = ParamKind::Integer { min: 0, max: MAX_TOOL_NUMBER as i64 };
const LENGTH: ParamKind = ParamKind::Length { min: 0.001, max: 2000.0 };
const SMALL: ParamKind = ParamKind::Length { min: 0.0, max: 2000.0 };
const RATE: ParamKind = ParamKind::Rate { min: 0.0, max: 1.0e6 };
const RPM: ParamKind = ParamKind::Number { min: 0.0, max: 500_000.0 };
const ANGLE: ParamKind = ParamKind::Number { min: 0.0, max: 179.0 };
const MACHINE: Param = Param::optional("machine", ParamKind::Str, "Machine id (default: the active machine)");
const NEW_NUMBER: Param = Param::optional("number", NUMBER, "Tool number (default: the lowest free one)");

// Values only some types use.
const CORNER_RADIUS: Param = Param::optional(
    "corner_radius",
    SMALL,
    "Corner radius, mm (bull nose, face and slot mill inserts, dovetail; a corner rounding mill's form radius)",
);
const TIP_ANGLE: Param = Param::optional(
    "tip_angle",
    ANGLE,
    "Included point angle, degrees (drills, spot and centre drills, chamfer mills, countersinks, engravers) or a face mill's cutting edge angle",
);
const TIP_DIAMETER: Param = Param::optional(
    "tip_diameter",
    SMALL,
    "Flat at the tip (chamfer mill, engraver, countersink) or pilot diameter (centre drill, counterbore, corner rounding mill), mm",
);
const TIP_LENGTH: Param = Param::optional("tip_length", SMALL, "Pilot length (centre drill, counterbore), mm");
const TAPER_ANGLE: Param = Param::optional(
    "taper_angle",
    ParamKind::Number { min: 0.0, max: 80.0 },
    "Side angle of a tapered tool, or a dovetail's angle from the axis, degrees",
);
const NECK_DIAMETER: Param = Param::optional("neck_diameter", SMALL, "Reduced neck behind the flutes, mm (0: none)");
const SHOULDER_LENGTH: Param =
    Param::optional("shoulder_length", SMALL, "Reach: tip to where the shank starts, mm (0: the flute length)");
const THREAD_PITCH: Param = Param::optional("thread_pitch", SMALL, "Thread pitch, mm");
const HAND: Param = Param::optional("hand", ParamKind::Choice(HANDS), "Right or left hand (default right)");

/// The parameters every tool takes, after `extra`.
macro_rules! tool_params {
    ($($extra:expr),* $(,)?) => {
        &[
            $($extra,)*
            Param::optional("name", ParamKind::Str, "Description shown in the library (default: Ø<d> <type>)"),
            Param::optional("diameter", LENGTH, "Cutting diameter, mm (a ball's ball diameter; a boring bar's bore)"),
            Param::optional("flutes", ParamKind::Integer { min: 1, max: 64 }, "Number of flutes (inserts, for face mills)"),
            Param::optional("flute_length", LENGTH, "Length of cut, mm"),
            Param::optional("overall_length", LENGTH, "Stick-out from the holder to the tip, mm"),
            Param::optional("shank_diameter", LENGTH, "Shank diameter, mm"),
            Param::optional("material", ParamKind::Choice(TOOL_MATERIALS), "Tool material (default carbide)"),
            Param::optional("coating", ParamKind::Str, "Coating, e.g. TiAlN"),
            Param::optional("coolant_through", ParamKind::Bool, "Coolant through the tool"),
            Param::optional("vendor", ParamKind::Str, "Maker"),
            Param::optional("product_id", ParamKind::Str, "Maker's product number"),
            Param::optional("holder", ParamKind::Str, "Holder it sits in (holder.N; empty for none)"),
            Param::optional("spindle_rpm", RPM, "Spindle speed, rpm (0 = not set)"),
            Param::optional("feed", RATE, "Cutting feed, mm/min (0 = not set)"),
            Param::optional("plunge_feed", RATE, "Plunge feed, mm/min (0 = not set)"),
            Param::optional("stepover", SMALL, "Radial depth of cut, mm (0 = not set)"),
            Param::optional("stepdown", SMALL, "Axial depth of cut, mm (0 = not set)"),
            Param::optional("length_offset", OFFSET, "Length offset register (default: the tool number)"),
            Param::optional("diameter_offset", OFFSET, "Diameter offset register (default: the tool number)"),
            Param::optional("notes", ParamKind::Str, "Free notes"),
            MACHINE,
        ]
    };
}

/// `tool.create.<kind>`: a tool of that type, with the values the type uses.
macro_rules! create_type {
    ($id:literal, $kind:literal, $label:literal, $tooltip:literal, [$($extra:expr),*]) => {
        CommandSpec {
            id: $id,
            label: $label,
            tooltip: $tooltip,
            placement: None,
            icon: None,
            shortcut: None,
            params: tool_params!(NEW_NUMBER, $($extra),*),
            returns: TOOL_RETURNS,
            enabled: always,
            run: |s: &mut Session, p: &Value| create_as(s, p, $kind),
        }
    };
}

/// Parameters of a holder, after `extra`.
macro_rules! holder_params {
    ($($extra:expr),* $(,)?) => {
        &[
            $($extra,)*
            Param::optional("name", ParamKind::Str, "Name, e.g. ER32 collet chuck"),
            Param::optional("vendor", ParamKind::Str, "Maker"),
            Param::optional("product_id", ParamKind::Str, "Maker's product number"),
            Param::optional("segment_a", ParamKind::Vec3, "First segment from the holder face: [lower diameter, upper diameter, height] mm"),
            Param::optional("segment_b", ParamKind::Vec3, "Second segment up"),
            Param::optional("segment_c", ParamKind::Vec3, "Third segment up"),
            Param::optional("segment_d", ParamKind::Vec3, "Fourth segment up"),
            Param::optional("segment_e", ParamKind::Vec3, "Fifth segment up"),
            Param::optional("segment_f", ParamKind::Vec3, "Sixth segment up"),
            Param::optional("segment_g", ParamKind::Vec3, "Seventh segment up"),
            Param::optional("segment_h", ParamKind::Vec3, "Eighth segment up"),
        ]
    };
}

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "tool.create",
        label: "New tool",
        tooltip: "Add a cutting tool to the active machine's library",
        placement: Some(Placement::large(RibbonTab::Tooling, "Tools").with_menu(TOOL_MENU)),
        icon: None,
        shortcut: None,
        params: tool_params!(
            Param::optional("type", ParamKind::Choice(TOOL_TYPES), "Tool type (default flat)"),
            NEW_NUMBER,
            CORNER_RADIUS,
            TIP_ANGLE,
            TIP_DIAMETER,
            TIP_LENGTH,
            TAPER_ANGLE,
            NECK_DIAMETER,
            SHOULDER_LENGTH,
            THREAD_PITCH,
            HAND,
        ),
        returns: TOOL_RETURNS,
        enabled: always,
        run: create,
    },
    create_type!(
        "tool.create.flat",
        "flat",
        "Flat end mill",
        "Add a flat (square) end mill",
        [NECK_DIAMETER, SHOULDER_LENGTH]
    ),
    create_type!(
        "tool.create.ball",
        "ball",
        "Ball end mill",
        "Add a ball nose end mill (3D and multi-axis finishing)",
        [NECK_DIAMETER, SHOULDER_LENGTH]
    ),
    create_type!(
        "tool.create.bull",
        "bull",
        "Bull nose end mill",
        "Add an end mill with a corner radius",
        [CORNER_RADIUS, NECK_DIAMETER, SHOULDER_LENGTH]
    ),
    create_type!(
        "tool.create.tapered_ball",
        "tapered_ball",
        "Tapered ball mill",
        "Add a tapered ball mill: a small ball on a stiff, tapered body",
        [TAPER_ANGLE, NECK_DIAMETER, SHOULDER_LENGTH]
    ),
    create_type!(
        "tool.create.lollipop",
        "lollipop",
        "Lollipop cutter",
        "Add a lollipop (undercut) cutter: a ball on a narrow neck",
        [NECK_DIAMETER, SHOULDER_LENGTH]
    ),
    create_type!("tool.create.chamfer", "chamfer", "Chamfer mill", "Add a chamfer mill", [TIP_ANGLE, TIP_DIAMETER]),
    create_type!(
        "tool.create.engraving",
        "engraving",
        "Engraving tool",
        "Add a V engraving tool",
        [TIP_ANGLE, TIP_DIAMETER]
    ),
    create_type!(
        "tool.create.dovetail",
        "dovetail",
        "Dovetail cutter",
        "Add a dovetail cutter",
        [TAPER_ANGLE, CORNER_RADIUS, NECK_DIAMETER, SHOULDER_LENGTH]
    ),
    create_type!(
        "tool.create.slot_mill",
        "slot_mill",
        "Slot mill",
        "Add a slot (keyseat, T-slot) mill",
        [CORNER_RADIUS, NECK_DIAMETER, SHOULDER_LENGTH]
    ),
    create_type!(
        "tool.create.radius_mill",
        "radius_mill",
        "Corner rounding mill",
        "Add a corner rounding mill",
        [CORNER_RADIUS, TIP_DIAMETER]
    ),
    create_type!(
        "tool.create.face_mill",
        "face_mill",
        "Face mill",
        "Add a face mill (insert cutter)",
        [CORNER_RADIUS, TIP_ANGLE]
    ),
    create_type!(
        "tool.create.thread_mill",
        "thread_mill",
        "Thread mill",
        "Add a thread mill",
        [THREAD_PITCH, HAND, NECK_DIAMETER, SHOULDER_LENGTH]
    ),
    create_type!("tool.create.drill", "drill", "Drill", "Add a twist drill", [TIP_ANGLE]),
    create_type!("tool.create.spot_drill", "spot_drill", "Spot drill", "Add a spot drill", [TIP_ANGLE]),
    create_type!(
        "tool.create.center_drill",
        "center_drill",
        "Centre drill",
        "Add a centre drill",
        [TIP_ANGLE, TIP_DIAMETER, TIP_LENGTH]
    ),
    create_type!("tool.create.reamer", "reamer", "Reamer", "Add a reamer", []),
    create_type!("tool.create.tap", "tap", "Tap", "Add a tap", [THREAD_PITCH, HAND]),
    create_type!(
        "tool.create.boring_bar",
        "boring_bar",
        "Boring bar",
        "Add a boring bar",
        [CORNER_RADIUS, NECK_DIAMETER, SHOULDER_LENGTH]
    ),
    create_type!(
        "tool.create.counterbore",
        "counterbore",
        "Counterbore",
        "Add a counterbore with a pilot",
        [TIP_DIAMETER, TIP_LENGTH]
    ),
    create_type!(
        "tool.create.countersink",
        "countersink",
        "Countersink",
        "Add a countersink",
        [TIP_ANGLE, TIP_DIAMETER]
    ),
    CommandSpec {
        id: "tool.edit",
        label: "Edit tool",
        tooltip: "Change a tool's geometry, cutting data, holder or number",
        placement: None,
        icon: None,
        shortcut: None,
        params: tool_params!(
            Param::required("number", NUMBER, "The tool to change"),
            Param::optional("new_number", NUMBER, "Renumber the tool"),
            CORNER_RADIUS,
            TIP_ANGLE,
            TIP_DIAMETER,
            TIP_LENGTH,
            TAPER_ANGLE,
            NECK_DIAMETER,
            SHOULDER_LENGTH,
            THREAD_PITCH,
            HAND,
        ),
        returns: TOOL_RETURNS,
        enabled: always,
        run: edit,
    },
    CommandSpec {
        id: "tool.feeds",
        label: "Feeds & speeds",
        tooltip: "Set a tool's spindle speed, feeds and depths of cut, or work them out from surface speed and chip load",
        placement: Some(Placement::small(RibbonTab::Tooling, "Tools")),
        icon: None,
        shortcut: None,
        params: &[
            Param::required("number", NUMBER, "The tool"),
            Param::optional(
                "surface_speed",
                ParamKind::Number { min: 0.0, max: 10_000.0 },
                "Cutting speed, m/min: works out the spindle speed",
            ),
            Param::optional(
                "feed_per_tooth",
                ParamKind::Number { min: 0.0, max: 10.0 },
                "Chip load, mm per tooth: works out the feed",
            ),
            Param::optional("spindle_rpm", RPM, "Spindle speed, rpm (0 = not set)"),
            Param::optional("feed", RATE, "Cutting feed, mm/min (0 = not set)"),
            Param::optional("plunge_feed", RATE, "Plunge feed, mm/min (0 = not set)"),
            Param::optional("stepover", SMALL, "Radial depth of cut, mm (0 = not set)"),
            Param::optional("stepdown", SMALL, "Axial depth of cut, mm (0 = not set)"),
            MACHINE,
        ],
        returns: TOOL_RETURNS,
        enabled: always,
        run: feeds,
    },
    CommandSpec {
        id: "tool.preset.set",
        label: "Cutting preset",
        tooltip: "Add or change one of a tool's cutting data presets (per workpiece material)",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("number", NUMBER, "The tool"),
            Param::required("name", ParamKind::Str, "Preset name, e.g. Aluminium roughing"),
            Param::optional("workpiece", ParamKind::Choice(WORKPIECES), "Workpiece material"),
            Param::optional(
                "surface_speed",
                ParamKind::Number { min: 0.0, max: 10_000.0 },
                "Cutting speed, m/min (works out the spindle speed)",
            ),
            Param::optional(
                "feed_per_tooth",
                ParamKind::Number { min: 0.0, max: 10.0 },
                "Chip load, mm per tooth (works out the feed)",
            ),
            Param::optional("spindle_rpm", RPM, "Spindle speed, rpm"),
            Param::optional("feed", RATE, "Cutting feed, mm/min"),
            Param::optional("plunge_feed", RATE, "Plunge feed, mm/min (default a third of the feed)"),
            Param::optional("ramp_feed", RATE, "Ramp feed, mm/min (default half the feed)"),
            Param::optional("stepover", SMALL, "Radial depth of cut, mm"),
            Param::optional("stepdown", SMALL, "Axial depth of cut, mm"),
            Param::optional("coolant", ParamKind::Choice(&["flood", "mist", "air", "through_tool", "off"]), "Coolant"),
            MACHINE,
        ],
        returns: TOOL_RETURNS,
        enabled: always,
        run: preset_set,
    },
    CommandSpec {
        id: "tool.preset.remove",
        label: "Delete preset",
        tooltip: "Delete one of a tool's cutting presets",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("number", NUMBER, "The tool"),
            Param::required("name", ParamKind::Str, "Preset name"),
            MACHINE,
        ],
        returns: TOOL_RETURNS,
        enabled: always,
        run: |s, p| {
            let mid = tool_machine(s, p)?;
            let number = number(p, "number")?;
            let name = params::req_str(p, "name")?.to_owned();
            update_tool(s, &mid, number, |t, _| {
                let before = t.presets.len();
                t.presets.retain(|x| x.name != name);
                if t.presets.len() == before {
                    return Err(EngineError::InvalidParams(format!("T{number} has no preset `{name}`")));
                }
                Ok(())
            })
        },
    },
    CommandSpec {
        id: "tool.preset.apply",
        label: "Use preset",
        tooltip: "Make one of a tool's presets its own cutting data",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("number", NUMBER, "The tool"),
            Param::required("name", ParamKind::Str, "Preset name"),
            MACHINE,
        ],
        returns: TOOL_RETURNS,
        enabled: always,
        run: |s, p| {
            let mid = tool_machine(s, p)?;
            let number = number(p, "number")?;
            let name = params::req_str(p, "name")?.to_owned();
            update_tool(s, &mid, number, |t, _| {
                let c = t.cutting(Some(&name)).map_err(EngineError::InvalidParams)?;
                t.spindle_rpm = c.spindle_rpm;
                t.feed = c.feed;
                t.plunge_feed = c.plunge_feed;
                t.stepover = c.stepover;
                t.stepdown = c.stepdown;
                Ok(())
            })
        },
    },
    CommandSpec {
        id: "tool.duplicate",
        label: "Copy tool",
        tooltip: "Copy a tool under a new number",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("number", NUMBER, "The tool to copy"),
            Param::optional("new_number", NUMBER, "Number of the copy (default: the lowest free one)"),
            MACHINE,
        ],
        returns: TOOL_RETURNS,
        enabled: always,
        run: |s, p| {
            let mid = tool_machine(s, p)?;
            let number = number(p, "number")?;
            let m = s.library.machine_mut(&mid).ok_or_else(|| missing_machine(&mid))?;
            if m.tools.len() >= MAX_TOOLS {
                return Err(EngineError::InvalidParams(format!("a library holds at most {MAX_TOOLS} tools")));
            }
            let mut t = m.tool(number).cloned().ok_or_else(|| missing_tool(number))?;
            t.number = match p.get("new_number").and_then(Value::as_u64) {
                Some(n) => u32::try_from(n).unwrap_or(0),
                None => m.next_tool_number(),
            };
            if m.tool(t.number).is_some() {
                return Err(EngineError::InvalidParams(format!("T{} is already in the library", t.number)));
            }
            t.length_offset = t.number;
            t.diameter_offset = t.number;
            m.check_tool(&t).map_err(EngineError::InvalidParams)?;
            let result = t.to_json(&mid);
            m.tools.push(t);
            m.tools.sort_by_key(|t| t.number);
            s.library.changed();
            Ok(result)
        },
    },
    CommandSpec {
        id: "tool.remove",
        label: "Delete tool",
        tooltip: "Delete a tool from the library",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("number", NUMBER, "The tool to delete"), MACHINE],
        returns: "{\"removed\": tool id}",
        enabled: always,
        run: |s, p| {
            let mid = tool_machine(s, p)?;
            let number = number(p, "number")?;
            let m = s.library.machine_mut(&mid).ok_or_else(|| missing_machine(&mid))?;
            if m.tool(number).is_none() {
                return Err(missing_tool(number));
            }
            m.tools.retain(|t| t.number != number);
            let id = tool_ref(&mid, number);
            if s.view.selection.as_deref() == Some(id.as_str()) {
                s.view.selection = None;
            }
            s.library.changed();
            Ok(json!({ "removed": id }))
        },
    },
    CommandSpec {
        id: "tool.list",
        label: "Tools",
        tooltip: "The tools in a machine's library, optionally of one type or matching a search",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            MACHINE,
            Param::optional("type", ParamKind::Choice(TOOL_TYPES), "Only tools of this type"),
            Param::optional(
                "search",
                ParamKind::Str,
                "Only tools whose name, type, vendor, product number or notes contain this",
            ),
        ],
        returns: "list of tools, as tool.get returns them, by number",
        enabled: always,
        run: |s, p| {
            let mid = tool_machine(s, p)?;
            let kind = params::opt_str(p, "type")?;
            let search = params::opt_str(p, "search")?.map(str::to_lowercase).unwrap_or_default();
            let m = s.library.machine(&mid).ok_or_else(|| missing_machine(&mid))?;
            Ok(json!(
                m.tools
                    .iter()
                    .filter(|t| kind.is_none_or(|k| t.kind == k))
                    .filter(|t| search.is_empty() || matches_search(t, &search))
                    .map(|t| t.to_json(&mid))
                    .collect::<Vec<_>>()
            ))
        },
    },
    CommandSpec {
        id: "tool.get",
        label: "Tool",
        tooltip: "One tool from a machine's library",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("number", NUMBER, "Tool number"), MACHINE],
        returns: TOOL_RETURNS,
        enabled: always,
        run: |s, p| {
            let mid = tool_machine(s, p)?;
            let number = number(p, "number")?;
            let m = s.library.machine(&mid).ok_or_else(|| missing_machine(&mid))?;
            Ok(m.tool(number).ok_or_else(|| missing_tool(number))?.to_json(&mid))
        },
    },
    CommandSpec {
        id: "tool.profile.get",
        label: "Tool profile",
        tooltip: "A tool's outline as the preview and the simulation draw it, with its holder's",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("number", NUMBER, "Tool number"), MACHINE],
        returns: "{\"points\": [[radius, height], ...] from the tip, \"flute_top\": mm, \"holder\": [[radius, height above the holder face], ...] or null}",
        enabled: always,
        run: |s, p| {
            let mid = tool_machine(s, p)?;
            let number = number(p, "number")?;
            let m = s.library.machine(&mid).ok_or_else(|| missing_machine(&mid))?;
            let t = m.tool(number).ok_or_else(|| missing_tool(number))?;
            let profile = t.profile();
            Ok(json!({
                "points": profile.points,
                "flute_top": profile.flute_top,
                "holder": s.library.holder_of(t).map(Holder::profile),
            }))
        },
    },
    CommandSpec {
        id: "tool.library.open",
        label: "Tool library",
        tooltip: "Show the active machine's tool library",
        placement: Some(Placement::large(RibbonTab::Tooling, "Library")),
        icon: None,
        shortcut: None,
        params: &[],
        returns: "{\"pane\": \"library\"}",
        enabled: always,
        run: |s, _| {
            s.ui.request(UiRequest::ShowPane(Pane::Library));
            s.ui.request(UiRequest::FocusPane(Pane::Library));
            Ok(json!({ "pane": "library" }))
        },
    },
    CommandSpec {
        id: "tool.library.sample",
        label: "Sample library",
        tooltip: "Add the sample tool library to a machine: two tools of every type, with presets and holders",
        placement: Some(Placement::small(RibbonTab::Tooling, "Library")),
        icon: None,
        shortcut: None,
        params: &[
            Param::optional(
                "replace",
                ParamKind::Bool,
                "Overwrite tools with the same numbers (default false: they are skipped)",
            ),
            MACHINE,
        ],
        returns: "{\"imported\": count, \"skipped\": [numbers], \"holders\": count, \"machine\": id}",
        enabled: always,
        run: |s, p| {
            let mid = tool_machine(s, p)?;
            let replace = params::opt_bool(p, "replace")?.unwrap_or(false);
            let (tools, holders) = crate::tooling::sample_library();
            add_tools(s, &mid, tools, holders, if replace { Clash::Replace } else { Clash::Skip })
        },
    },
    CommandSpec {
        id: "tool.library.import",
        label: "Import",
        tooltip: "Add tools (and their holders) from a tool library file (.json) to the active machine",
        placement: Some(Placement::small(RibbonTab::Tooling, "Library")),
        icon: None,
        shortcut: None,
        params: &[
            Param::required("path", ParamKind::Str, "Tool library file written by tool.library.export"),
            Param::optional("replace", ParamKind::Bool, "Overwrite tools with the same numbers (default false)"),
            MACHINE,
        ],
        returns: "{\"imported\": count, \"skipped\": [], \"holders\": count, \"machine\": id}",
        enabled: always,
        run: import,
    },
    CommandSpec {
        id: "tool.library.export",
        label: "Export",
        tooltip: "Write the active machine's tool library, with the holders it uses, to a file (.json)",
        placement: Some(Placement::small(RibbonTab::Tooling, "Library")),
        icon: None,
        shortcut: None,
        params: &[Param::required("path", ParamKind::Str, "File to write"), MACHINE],
        returns: "{\"exported\": count, \"path\": path}",
        enabled: always,
        run: |s, p| {
            let mid = tool_machine(s, p)?;
            let path = params::req_str(p, "path")?;
            let m = s.library.machine(&mid).ok_or_else(|| missing_machine(&mid))?;
            let holders: Vec<&Holder> = s
                .library
                .holders
                .iter()
                .filter(|h| m.tools.iter().any(|t| t.holder.as_deref() == Some(h.id.as_str())))
                .collect();
            let doc = json!({ "format": TOOLS_FORMAT, "machine": m.name, "tools": m.tools, "holders": holders });
            let text = serde_json::to_string_pretty(&doc).map_err(|e| EngineError::Internal(e.to_string()))?;
            std::fs::write(path, text)
                .map_err(|e| EngineError::InvalidParams(format!("could not write {path}: {e}")))?;
            Ok(json!({ "exported": m.tools.len(), "path": path }))
        },
    },
    CommandSpec {
        id: "holder.create",
        label: "New holder",
        tooltip: "Add a tool holder: its outline as up to eight stacked cones or cylinders from the holder face up",
        placement: Some(Placement::small(RibbonTab::Tooling, "Tools")),
        icon: None,
        shortcut: None,
        params: holder_params!(),
        returns: HOLDER_RETURNS,
        enabled: always,
        run: |s, p| {
            if s.library.holders.len() >= MAX_HOLDERS {
                return Err(EngineError::InvalidParams(format!("a library holds at most {MAX_HOLDERS} holders")));
            }
            let mut h = Holder {
                id: format!("holder.{}", s.library.next_holder),
                name: String::new(),
                vendor: String::new(),
                product_id: String::new(),
                segments: Vec::new(),
            };
            apply_holder(&mut h, p)?;
            if h.name.is_empty() {
                h.name = format!("Holder {}", s.library.next_holder);
            }
            h.validate().map_err(EngineError::InvalidParams)?;
            s.library.next_holder = s.library.next_holder.saturating_add(1);
            let result = h.to_json();
            s.library.holders.push(h);
            s.library.changed();
            Ok(result)
        },
    },
    CommandSpec {
        id: "holder.edit",
        label: "Edit holder",
        tooltip: "Change a holder's name or outline (segments given replace them all)",
        placement: None,
        icon: None,
        shortcut: None,
        params: holder_params!(Param::required("id", ParamKind::Str, "Holder id, e.g. holder.2")),
        returns: HOLDER_RETURNS,
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            let h = s.library.holders.iter_mut().find(|h| h.id == id).ok_or_else(|| missing_holder(id))?;
            let mut changed = h.clone();
            apply_holder(&mut changed, p)?;
            changed.validate().map_err(EngineError::InvalidParams)?;
            *h = changed;
            let result = h.to_json();
            s.library.changed();
            Ok(result)
        },
    },
    CommandSpec {
        id: "holder.remove",
        label: "Delete holder",
        tooltip: "Delete a holder; tools in it are left without one",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Holder id")],
        returns: "{\"removed\": id, \"tools\": count left without a holder}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?.to_owned();
            if s.library.holder(&id).is_none() {
                return Err(missing_holder(&id));
            }
            s.library.holders.retain(|h| h.id != id);
            let mut freed = 0;
            for m in &mut s.library.machines {
                for t in &mut m.tools {
                    if t.holder.as_deref() == Some(id.as_str()) {
                        t.holder = None;
                        freed += 1;
                    }
                }
            }
            s.library.changed();
            Ok(json!({ "removed": id, "tools": freed }))
        },
    },
    CommandSpec {
        id: "holder.list",
        label: "Holders",
        tooltip: "Every tool holder in the library",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "list of holders, as holder.create returns them",
        enabled: always,
        run: |s, _| Ok(json!(s.library.holders.iter().map(Holder::to_json).collect::<Vec<_>>())),
    },
];

fn missing_tool(number: u32) -> EngineError {
    EngineError::InvalidParams(format!("no tool T{number} in this machine's library"))
}

fn missing_holder(id: &str) -> EngineError {
    EngineError::InvalidParams(format!("no holder `{id}`"))
}

fn number(p: &Value, key: &str) -> EngineResult<u32> {
    p.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| EngineError::InvalidParams(format!("`{key}` is required")))
}

/// The machine `machine` names, or the active one.
fn tool_machine(s: &Session, p: &Value) -> EngineResult<String> {
    let id = machine_id(s, &json!({ "id": p.get("machine").cloned().unwrap_or(Value::Null) }))?;
    s.library.machine(&id).map(|_| id.clone()).ok_or_else(|| missing_machine(&id))
}

fn matches_search(t: &Tool, search: &str) -> bool {
    [t.name.as_str(), t.kind.as_str(), t.type_label(), t.vendor.as_str(), t.product_id.as_str(), t.notes.as_str()]
        .iter()
        .any(|f| f.to_lowercase().contains(search))
        || format!("t{}", t.number) == search
}

fn apply_holder(h: &mut Holder, p: &Value) -> EngineResult<()> {
    if let Some(n) = params::opt_str(p, "name")? {
        h.name = clip(n);
    }
    if let Some(v) = params::opt_str(p, "vendor")? {
        h.vendor = clip(v);
    }
    if let Some(v) = params::opt_str(p, "product_id")? {
        h.product_id = clip(v);
    }
    let mut segments = Vec::new();
    for k in 1..=MAX_HOLDER_SEGMENTS.min(8) {
        let key = format!("segment_{}", char::from(b'a' + (k - 1) as u8));
        if let Some(v) = params::opt_vec3(p, &key)? {
            let v = v.as_dvec3();
            segments.push(HolderSegment { lower_diameter: v.x, upper_diameter: v.y, height: v.z });
        }
    }
    if !segments.is_empty() {
        h.segments = segments;
    }
    Ok(())
}

/// Applies the geometry and cutting data given in `p` to `t`.
fn apply(t: &mut Tool, p: &Value) -> EngineResult<()> {
    let num = |key: &str| params::opt_f64(p, key, 0.0, 1.0e6);
    if let Some(n) = params::opt_str(p, "name")? {
        t.name = clip(n);
    }
    for (key, slot) in [
        ("diameter", &mut t.diameter),
        ("corner_radius", &mut t.corner_radius),
        ("flute_length", &mut t.flute_length),
        ("overall_length", &mut t.overall_length),
        ("shank_diameter", &mut t.shank_diameter),
        ("tip_angle", &mut t.tip_angle),
        ("tip_diameter", &mut t.tip_diameter),
        ("tip_length", &mut t.tip_length),
        ("taper_angle", &mut t.taper_angle),
        ("neck_diameter", &mut t.neck_diameter),
        ("shoulder_length", &mut t.shoulder_length),
        ("thread_pitch", &mut t.thread_pitch),
        ("spindle_rpm", &mut t.spindle_rpm),
        ("feed", &mut t.feed),
        ("plunge_feed", &mut t.plunge_feed),
        ("stepover", &mut t.stepover),
        ("stepdown", &mut t.stepdown),
    ] {
        if let Some(v) = num(key)? {
            *slot = v;
        }
    }
    if let Some(f) = p.get("flutes").and_then(Value::as_u64) {
        t.flutes = u32::try_from(f).unwrap_or(0);
    }
    // A ball-ended tool's corner radius is half its diameter.
    if t.ball_ended() {
        t.corner_radius = t.diameter / 2.0;
    }
    if let Some(m) = params::opt_str(p, "material")? {
        t.material = m.to_owned();
    }
    if let Some(h) = params::opt_str(p, "hand")? {
        t.hand = h.to_owned();
    }
    if let Some(v) = params::opt_bool(p, "coolant_through")? {
        t.coolant_through = v;
    }
    for (key, slot) in [
        ("coating", &mut t.coating),
        ("notes", &mut t.notes),
        ("vendor", &mut t.vendor),
        ("product_id", &mut t.product_id),
    ] {
        if let Some(v) = params::opt_str(p, key)? {
            *slot = clip(v);
        }
    }
    if let Some(h) = params::opt_str(p, "holder")? {
        t.holder = Some(h.trim().to_owned()).filter(|h| !h.is_empty());
    }
    for (key, slot) in [("length_offset", &mut t.length_offset), ("diameter_offset", &mut t.diameter_offset)] {
        if let Some(v) = p.get(key).and_then(Value::as_u64) {
            *slot = u32::try_from(v).unwrap_or(u32::MAX);
        }
    }
    Ok(())
}

/// The tool `p` describes, without adding it anywhere: what `tool.create` (with `type`) would
/// make. The tool dialog draws it as it is typed.
pub fn preview(s: &Session, p: &Value) -> EngineResult<Tool> {
    let kind = params::opt_str(p, "type")?.unwrap_or("flat");
    build(s, p, kind, 1)
}

fn build(s: &Session, p: &Value, kind: &str, number: u32) -> EngineResult<Tool> {
    if tool_type(kind).is_none() {
        return Err(EngineError::InvalidParams(format!("unknown tool type `{kind}`")));
    }
    let diameter = params::opt_f64(p, "diameter", 0.001, 2000.0)?
        .ok_or_else(|| EngineError::InvalidParams("`diameter` is required".into()))?;
    // The default name reads in the user's units: "Ø0.25 in ball end mill" or "Ø6 mm ...".
    let units = s.prefs.units;
    let mut t = Tool::typical(number, kind, diameter);
    apply(&mut t, p)?;
    if p.get("name").is_none() {
        let label = t.type_label().to_lowercase();
        t.name = match kind {
            // Threads are named the ISO way (their default pitches are ISO coarse).
            "tap" | "thread_mill" => format!("M{}×{} {label}", trim(diameter), trim(t.thread_pitch)),
            _ => format!("Ø{} {} {label}", units.plain(diameter), units.length()),
        };
    }
    Ok(t)
}

fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn create(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let kind = params::opt_str(p, "type")?.unwrap_or("flat").to_owned();
    create_as(s, p, &kind)
}

fn create_as(s: &mut Session, p: &Value, kind: &str) -> EngineResult<Value> {
    let mid = tool_machine(s, p)?;
    let m = s.library.machine(&mid).ok_or_else(|| missing_machine(&mid))?;
    if m.tools.len() >= MAX_TOOLS {
        return Err(EngineError::InvalidParams(format!("a library holds at most {MAX_TOOLS} tools")));
    }
    let number = match p.get("number").and_then(Value::as_u64) {
        Some(n) => u32::try_from(n).unwrap_or(0),
        None => m.next_tool_number(),
    };
    if m.tool(number).is_some() {
        return Err(EngineError::InvalidParams(format!(
            "T{number} is already in the library; edit it or pick another number"
        )));
    }
    let t = build(s, p, kind, number)?;
    check(s, &mid, &t)?;
    let result = t.to_json(&mid);
    let m = s.library.machine_mut(&mid).ok_or_else(|| missing_machine(&mid))?;
    m.tools.push(t);
    m.tools.sort_by_key(|t| t.number);
    s.view.selection = Some(tool_ref(&mid, number));
    s.library.changed();
    Ok(result)
}

/// A tool must fit its machine and name a holder that exists.
fn check(s: &Session, mid: &str, t: &Tool) -> EngineResult<()> {
    let m: &Machine = s.library.machine(mid).ok_or_else(|| missing_machine(mid))?;
    m.check_tool(t).map_err(EngineError::InvalidParams)?;
    if let Some(h) = &t.holder
        && s.library.holder(h).is_none()
    {
        return Err(missing_holder(h));
    }
    Ok(())
}

/// Changes tool `number` with `f`, checks it and puts it back.
fn update_tool(
    s: &mut Session,
    mid: &str,
    number: u32,
    f: impl FnOnce(&mut Tool, &Session) -> EngineResult<()>,
) -> EngineResult<Value> {
    let m = s.library.machine(mid).ok_or_else(|| missing_machine(mid))?;
    let mut t = m.tool(number).cloned().ok_or_else(|| missing_tool(number))?;
    f(&mut t, s)?;
    check(s, mid, &t)?;
    let result = t.to_json(mid);
    let m = s.library.machine_mut(mid).ok_or_else(|| missing_machine(mid))?;
    if let Some(slot) = m.tools.iter_mut().find(|o| o.number == number) {
        *slot = t;
    }
    s.library.changed();
    Ok(result)
}

fn edit(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let mid = tool_machine(s, p)?;
    let number = number(p, "number")?;
    let m = s.library.machine(&mid).ok_or_else(|| missing_machine(&mid))?;
    let mut t = m.tool(number).cloned().ok_or_else(|| missing_tool(number))?;
    apply(&mut t, p)?;
    if let Some(n) = p.get("new_number").and_then(Value::as_u64) {
        let n = u32::try_from(n).unwrap_or(0);
        if n != number && m.tool(n).is_some() {
            return Err(EngineError::InvalidParams(format!("T{n} is already in the library")));
        }
        t.number = n;
    }
    check(s, &mid, &t)?;
    let new_number = t.number;
    let result = t.to_json(&mid);
    let m = s.library.machine_mut(&mid).ok_or_else(|| missing_machine(&mid))?;
    m.tools.retain(|o| o.number != number);
    m.tools.push(t);
    m.tools.sort_by_key(|t| t.number);
    if new_number != number && s.view.selection.as_deref() == Some(tool_ref(&mid, number).as_str()) {
        s.view.selection = Some(tool_ref(&mid, new_number));
    }
    s.library.changed();
    Ok(result)
}

fn feeds(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let mid = tool_machine(s, p)?;
    let number = number(p, "number")?;
    let vc = params::opt_f64(p, "surface_speed", 0.0, 10_000.0)?;
    let fz = params::opt_f64(p, "feed_per_tooth", 0.0, 10.0)?;
    update_tool(s, &mid, number, |t, _| {
        apply(t, p)?;
        if vc.is_some() || fz.is_some() {
            let mut c = Preset {
                surface_speed: vc.unwrap_or(0.0),
                feed_per_tooth: fz.unwrap_or(0.0),
                spindle_rpm: if vc.is_some() { 0.0 } else { t.spindle_rpm },
                feed: if fz.is_some() || vc.is_some() { 0.0 } else { t.feed },
                ..Preset::default()
            };
            c.derive(t.diameter, t.flutes);
            if c.spindle_rpm > 0.0 {
                t.spindle_rpm = c.spindle_rpm;
            }
            if fz.is_some() && c.feed > 0.0 {
                t.feed = c.feed;
                if p.get("plunge_feed").is_none() {
                    t.plunge_feed = c.plunge_feed;
                }
            }
        }
        Ok(())
    })
}

fn preset_set(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let mid = tool_machine(s, p)?;
    let number = number(p, "number")?;
    let name = clip(params::req_str(p, "name")?);
    if name.is_empty() {
        return Err(EngineError::InvalidParams("a preset needs a name".into()));
    }
    let max_rpm = s.library.machine(&mid).map_or(f64::INFINITY, |m| m.max_spindle_rpm);
    let max_feed = s.library.machine(&mid).map_or(f64::INFINITY, |m| m.max_feed);
    update_tool(s, &mid, number, |t, _| {
        let mut preset = t
            .presets
            .iter()
            .find(|x| x.name == name)
            .cloned()
            .unwrap_or(Preset { name: name.clone(), ..Preset::default() });
        if let Some(w) = params::opt_str(p, "workpiece")? {
            preset.workpiece = w.to_owned();
        }
        if let Some(c) = params::opt_str(p, "coolant")? {
            preset.coolant = c.to_owned();
        }
        let given = |key: &str| params::opt_f64(p, key, 0.0, 1.0e6);
        let vc = given("surface_speed")?;
        let fz = given("feed_per_tooth")?;
        // New speeds and chip loads work out what depends on them, unless that is given too.
        if vc.is_some() && p.get("spindle_rpm").is_none() {
            preset.spindle_rpm = 0.0;
        }
        if (vc.is_some() || fz.is_some()) && p.get("feed").is_none() {
            preset.feed = 0.0;
            if p.get("plunge_feed").is_none() {
                preset.plunge_feed = 0.0;
            }
            if p.get("ramp_feed").is_none() {
                preset.ramp_feed = 0.0;
            }
        }
        for (key, slot) in [
            ("surface_speed", &mut preset.surface_speed),
            ("feed_per_tooth", &mut preset.feed_per_tooth),
            ("spindle_rpm", &mut preset.spindle_rpm),
            ("feed", &mut preset.feed),
            ("plunge_feed", &mut preset.plunge_feed),
            ("ramp_feed", &mut preset.ramp_feed),
            ("stepover", &mut preset.stepover),
            ("stepdown", &mut preset.stepdown),
        ] {
            if let Some(v) = given(key)? {
                *slot = v;
            }
        }
        preset.derive(t.diameter, t.flutes);
        if preset.spindle_rpm > max_rpm {
            return Err(EngineError::InvalidParams(format!(
                "{:.0} rpm is above the machine's {max_rpm:.0}",
                preset.spindle_rpm
            )));
        }
        if preset.feed > max_feed || preset.plunge_feed > max_feed || preset.ramp_feed > max_feed {
            return Err(EngineError::InvalidParams(format!("a feed is above the machine's {max_feed:.0} mm/min")));
        }
        match t.presets.iter().position(|x| x.name == name) {
            Some(i) => {
                if let Some(slot) = t.presets.get_mut(i) {
                    *slot = preset;
                }
            }
            None if t.presets.len() < MAX_PRESETS => t.presets.push(preset),
            None => return Err(EngineError::InvalidParams(format!("a tool keeps at most {MAX_PRESETS} presets"))),
        }
        Ok(())
    })
}

/// What to do with an incoming tool whose number is taken.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Clash {
    Fail,
    Skip,
    Replace,
}

/// Adds `tools` (with the `holders` they refer to) to machine `mid`. Holders already in the
/// library under the same name are reused; others are added under new ids.
fn add_tools(s: &mut Session, mid: &str, tools: Vec<Tool>, holders: Vec<Holder>, clash: Clash) -> EngineResult<Value> {
    let m = s.library.machine(mid).ok_or_else(|| missing_machine(mid))?;
    let mut incoming: Vec<Tool> = Vec::new();
    let mut skipped: Vec<u32> = Vec::new();
    for mut t in tools.into_iter().take(MAX_TOOLS) {
        t.clip_text();
        let holder = t.holder.take();
        m.check_tool(&t).map_err(|e| EngineError::InvalidParams(format!("T{}: {e}", t.number)))?;
        t.holder = holder;
        if incoming.iter().any(|o| o.number == t.number) {
            return Err(EngineError::InvalidParams(format!("T{} is listed twice", t.number)));
        }
        if m.tool(t.number).is_some() {
            match clash {
                Clash::Fail => {
                    return Err(EngineError::InvalidParams(format!(
                        "T{} is already in the library; import with replace = true to overwrite",
                        t.number
                    )));
                }
                Clash::Skip => {
                    skipped.push(t.number);
                    continue;
                }
                Clash::Replace => {}
            }
        }
        incoming.push(t);
    }
    // Holders the tools sit in: reuse one of the same name, else add it under a new id.
    let mut renamed: Vec<(String, String)> = Vec::new();
    let mut added = 0;
    for mut h in holders.into_iter().take(MAX_HOLDERS) {
        h.name = clip(&h.name);
        h.vendor = clip(&h.vendor);
        h.product_id = clip(&h.product_id);
        h.validate().map_err(|e| EngineError::InvalidParams(format!("holder {}: {e}", h.name)))?;
        if !incoming.iter().any(|t| t.holder.as_deref() == Some(h.id.as_str())) {
            continue;
        }
        let old = h.id.clone();
        let id = match s.library.holders.iter().find(|o| o.name == h.name) {
            Some(o) => o.id.clone(),
            None => {
                if s.library.holders.len() >= MAX_HOLDERS {
                    return Err(EngineError::InvalidParams(format!("a library holds at most {MAX_HOLDERS} holders")));
                }
                h.id = format!("holder.{}", s.library.next_holder);
                s.library.next_holder = s.library.next_holder.saturating_add(1);
                added += 1;
                let id = h.id.clone();
                s.library.holders.push(h);
                id
            }
        };
        renamed.push((old, id));
    }
    for t in &mut incoming {
        t.holder = t
            .holder
            .as_ref()
            .and_then(|old| renamed.iter().find(|(o, _)| o == old).map(|(_, n)| n.clone()))
            .filter(|id| s.library.holder(id).is_some());
    }
    let count = incoming.len();
    let m = s.library.machine_mut(mid).ok_or_else(|| missing_machine(mid))?;
    m.tools.retain(|o| incoming.iter().all(|t| t.number != o.number));
    if m.tools.len() + count > MAX_TOOLS {
        return Err(EngineError::InvalidParams(format!("a library holds at most {MAX_TOOLS} tools")));
    }
    m.tools.extend(incoming);
    m.tools.sort_by_key(|t| t.number);
    s.library.changed();
    Ok(json!({ "imported": count, "skipped": skipped, "holders": added, "machine": mid }))
}

/// Adds the sample library's tools (and holders) to machine `mid`, skipping numbers it has.
pub(crate) fn load_sample(s: &mut Session, mid: &str) -> EngineResult<Value> {
    let (tools, holders) = crate::tooling::sample_library();
    add_tools(s, mid, tools, holders, Clash::Skip)
}

fn import(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let mid = tool_machine(s, p)?;
    let path = params::req_str(p, "path")?;
    let replace = params::opt_bool(p, "replace")?.unwrap_or(false);
    let meta =
        std::fs::metadata(path).map_err(|e| EngineError::InvalidParams(format!("could not read {path}: {e}")))?;
    if !meta.is_file() {
        return Err(EngineError::InvalidParams(format!("{path} is not a regular file")));
    }
    if meta.len() > 64 * 1024 * 1024 {
        return Err(EngineError::InvalidParams("the tool library file is larger than 64 MiB".into()));
    }
    let text =
        std::fs::read_to_string(path).map_err(|e| EngineError::InvalidParams(format!("could not read {path}: {e}")))?;
    let doc: Value =
        serde_json::from_str(&text).map_err(|e| EngineError::InvalidParams(format!("{path} is not JSON: {e}")))?;
    if !matches!(doc.get("format").and_then(Value::as_str), Some(TOOLS_FORMAT | LEGACY_TOOLS_FORMAT)) {
        return Err(EngineError::InvalidParams(format!("{path} is not a CosmicDMS tool library ({TOOLS_FORMAT})")));
    }
    let mut tools = Vec::new();
    for (i, raw) in doc.get("tools").and_then(Value::as_array).into_iter().flatten().enumerate().take(MAX_TOOLS) {
        tools.push(
            serde_json::from_value::<Tool>(raw.clone())
                .map_err(|e| EngineError::InvalidParams(format!("tool {} in {path}: {e}", i + 1)))?,
        );
    }
    let mut holders = Vec::new();
    for (i, raw) in doc.get("holders").and_then(Value::as_array).into_iter().flatten().enumerate().take(MAX_HOLDERS) {
        holders.push(
            serde_json::from_value::<Holder>(raw.clone())
                .map_err(|e| EngineError::InvalidParams(format!("holder {} in {path}: {e}", i + 1)))?,
        );
    }
    add_tools(s, &mid, tools, holders, if replace { Clash::Replace } else { Clash::Fail })
}
