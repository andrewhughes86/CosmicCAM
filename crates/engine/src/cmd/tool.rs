//! Cutting tools in the active machine's tool library: create (flat and ball end mills for now),
//! edit, copy, delete, list, and import or export a library as JSON.

use serde_json::{Value, json};

use super::machine::{machine_id, missing as missing_machine};
use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always, never};
use crate::error::{EngineError, EngineResult};
use crate::library::{
    MAX_TOOL_NUMBER, MAX_TOOLS, SUPPORTED_TOOL_TYPES, TOOL_MATERIALS, TOOL_TYPES, Tool, clip, tool_ref,
};
use crate::params;
use crate::session::{Pane, Session, UiRequest};

const TOOL_RETURNS: &str = "the tool {id, machine, number, name, type, diameter, corner_radius, flutes, flute_length, \
                            overall_length, shank_diameter, material, coating, spindle_rpm, feed, plunge_feed, stepover, \
                            stepdown, length_offset, diameter_offset, notes}";
const TOOLS_FORMAT: &str = "CosmicCAM-tools/1";

/// Every tool type, offered from the New tool button's menu.
const TOOL_MENU: &[&str] = &[
    "tool.create.flat",
    "tool.create.ball",
    "tool.create.bull",
    "tool.create.chamfer",
    "tool.create.drill",
    "tool.create.spot_drill",
    "tool.create.face_mill",
    "tool.create.tap",
    "tool.create.thread_mill",
];

const NUMBER: ParamKind = ParamKind::Integer { min: 1, max: MAX_TOOL_NUMBER as i64 };
const OFFSET: ParamKind = ParamKind::Integer { min: 0, max: MAX_TOOL_NUMBER as i64 };
const LENGTH: ParamKind = ParamKind::Length { min: 0.001, max: 2000.0 };
const SMALL: ParamKind = ParamKind::Length { min: 0.0, max: 2000.0 };
const RATE: ParamKind = ParamKind::Rate { min: 0.0, max: 1.0e6 };
const RPM: ParamKind = ParamKind::Number { min: 0.0, max: 500_000.0 };
const MACHINE: Param = Param::optional("machine", ParamKind::Str, "Machine id (default: the active machine)");

/// Geometry and cutting data, shared by create and edit, after `extra`.
macro_rules! tool_params {
    ($($extra:expr),* $(,)?) => {
        &[
            $($extra,)*
            Param::optional("name", ParamKind::Str, "Description shown in the library (default: Ø<d> flat or ball end mill)"),
            Param::optional("diameter", LENGTH, "Cutting diameter, mm"),
            Param::optional("flutes", ParamKind::Integer { min: 1, max: 32 }, "Number of flutes (default 3, 2 below Ø3)"),
            Param::optional("flute_length", LENGTH, "Length of cut, mm (default 3 × diameter)"),
            Param::optional("overall_length", LENGTH, "Stick-out from the holder to the tip, mm"),
            Param::optional("shank_diameter", LENGTH, "Shank diameter, mm (default: the cutting diameter)"),
            Param::optional("material", ParamKind::Choice(TOOL_MATERIALS), "Tool material (default carbide)"),
            Param::optional("coating", ParamKind::Str, "Coating, e.g. TiAlN"),
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

macro_rules! placeholder {
    ($id:literal, $label:literal) => {
        CommandSpec {
            id: $id,
            label: $label,
            tooltip: concat!($label, " tools are not supported yet"),
            placement: None,
            icon: None,
            shortcut: None,
            params: &[],
            returns: "not defined yet",
            enabled: never,
            run: |_: &mut Session, _: &Value| -> EngineResult<Value> { Err(EngineError::NotSupported($id.into())) },
        }
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
            Param::optional(
                "type",
                ParamKind::Choice(TOOL_TYPES),
                "Tool type (default flat; flat and ball end mills so far)"
            ),
            Param::optional("number", NUMBER, "Tool number (default: the lowest free one)"),
        ),
        returns: TOOL_RETURNS,
        enabled: always,
        run: create,
    },
    CommandSpec {
        id: "tool.create.flat",
        label: "Flat end mill",
        tooltip: "Add a flat (square corner) end mill to the active machine's library",
        placement: None,
        icon: None,
        shortcut: None,
        params: tool_params!(Param::optional("number", NUMBER, "Tool number (default: the lowest free one)")),
        returns: TOOL_RETURNS,
        enabled: always,
        run: create,
    },
    CommandSpec {
        id: "tool.create.ball",
        label: "Ball end mill",
        tooltip: "Add a ball nose end mill to the active machine's library (for 3D and multi-axis finishing)",
        placement: None,
        icon: None,
        shortcut: None,
        params: tool_params!(Param::optional("number", NUMBER, "Tool number (default: the lowest free one)")),
        returns: TOOL_RETURNS,
        enabled: always,
        run: |s, p| {
            let mut p = p.clone();
            p["type"] = json!("ball");
            create(s, &p)
        },
    },
    placeholder!("tool.create.bull", "Bull nose"),
    placeholder!("tool.create.chamfer", "Chamfer mill"),
    placeholder!("tool.create.drill", "Drill"),
    placeholder!("tool.create.spot_drill", "Spot drill"),
    placeholder!("tool.create.face_mill", "Face mill"),
    placeholder!("tool.create.tap", "Tap"),
    placeholder!("tool.create.thread_mill", "Thread mill"),
    CommandSpec {
        id: "tool.edit",
        label: "Edit tool",
        tooltip: "Change a tool's geometry, cutting data or number",
        placement: None,
        icon: None,
        shortcut: None,
        params: tool_params!(
            Param::required("number", NUMBER, "The tool to change"),
            Param::optional("new_number", NUMBER, "Renumber the tool"),
        ),
        returns: TOOL_RETURNS,
        enabled: always,
        run: edit,
    },
    CommandSpec {
        id: "tool.feeds",
        label: "Feeds & speeds",
        tooltip: "Set a tool's spindle speed, feeds and depths of cut",
        placement: Some(Placement::small(RibbonTab::Tooling, "Tools")),
        icon: None,
        shortcut: None,
        params: &[
            Param::required("number", NUMBER, "The tool"),
            Param::optional("spindle_rpm", RPM, "Spindle speed, rpm (0 = not set)"),
            Param::optional("feed", RATE, "Cutting feed, mm/min (0 = not set)"),
            Param::optional("plunge_feed", RATE, "Plunge feed, mm/min (0 = not set)"),
            Param::optional("stepover", SMALL, "Radial depth of cut, mm (0 = not set)"),
            Param::optional("stepdown", SMALL, "Axial depth of cut, mm (0 = not set)"),
            MACHINE,
        ],
        returns: TOOL_RETURNS,
        enabled: always,
        run: edit,
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
        tooltip: "The tools in a machine's library",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[MACHINE],
        returns: "list of tools, as tool.get returns them, by number",
        enabled: always,
        run: |s, p| {
            let mid = tool_machine(s, p)?;
            let m = s.library.machine(&mid).ok_or_else(|| missing_machine(&mid))?;
            Ok(json!(m.tools.iter().map(|t| t.to_json(&mid)).collect::<Vec<_>>()))
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
        id: "tool.library.import",
        label: "Import",
        tooltip: "Add tools from a tool library file (.json) to the active machine",
        placement: Some(Placement::small(RibbonTab::Tooling, "Library")),
        icon: None,
        shortcut: None,
        params: &[
            Param::required("path", ParamKind::Str, "Tool library file written by tool.library.export"),
            Param::optional("replace", ParamKind::Bool, "Overwrite tools with the same numbers (default false)"),
            MACHINE,
        ],
        returns: "{\"imported\": count, \"machine\": id}",
        enabled: always,
        run: import,
    },
    CommandSpec {
        id: "tool.library.export",
        label: "Export",
        tooltip: "Write the active machine's tool library to a file (.json)",
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
            let doc = json!({ "format": TOOLS_FORMAT, "machine": m.name, "tools": m.tools });
            let text = serde_json::to_string_pretty(&doc).map_err(|e| EngineError::Internal(e.to_string()))?;
            std::fs::write(path, text)
                .map_err(|e| EngineError::InvalidParams(format!("could not write {path}: {e}")))?;
            Ok(json!({ "exported": m.tools.len(), "path": path }))
        },
    },
];

fn missing_tool(number: u32) -> EngineError {
    EngineError::InvalidParams(format!("no tool T{number} in this machine's library"))
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

/// Applies the geometry and cutting data given in `p` to `t`.
fn apply(t: &mut Tool, p: &Value) -> EngineResult<()> {
    let num = |key: &str| params::opt_f64(p, key, 0.0, 1.0e6);
    if let Some(n) = params::opt_str(p, "name")? {
        t.name = clip(n);
    }
    for (key, slot) in [
        ("diameter", &mut t.diameter),
        ("flute_length", &mut t.flute_length),
        ("overall_length", &mut t.overall_length),
        ("shank_diameter", &mut t.shank_diameter),
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
    // A ball's corner radius is half its diameter.
    if t.kind == "ball" {
        t.corner_radius = t.diameter / 2.0;
    }
    if let Some(m) = params::opt_str(p, "material")? {
        t.material = m.to_owned();
    }
    if let Some(c) = params::opt_str(p, "coating")? {
        t.coating = clip(c);
    }
    if let Some(n) = params::opt_str(p, "notes")? {
        t.notes = clip(n);
    }
    for (key, slot) in [("length_offset", &mut t.length_offset), ("diameter_offset", &mut t.diameter_offset)] {
        if let Some(v) = p.get(key).and_then(Value::as_u64) {
            *slot = u32::try_from(v).unwrap_or(u32::MAX);
        }
    }
    Ok(())
}

fn create(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let kind = params::opt_str(p, "type")?.unwrap_or("flat");
    if !SUPPORTED_TOOL_TYPES.contains(&kind) {
        return Err(EngineError::NotSupported(format!("{kind} tools")));
    }
    let diameter = params::opt_f64(p, "diameter", 0.001, 2000.0)?
        .ok_or_else(|| EngineError::InvalidParams("`diameter` is required".into()))?;
    let mid = tool_machine(s, p)?;
    // The default name reads in the user's units: "Ø0.25 in flat end mill" or "Ø6 mm ...".
    let units = s.prefs.units;
    let m = s.library.machine_mut(&mid).ok_or_else(|| missing_machine(&mid))?;
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
    // Sensible starting values; anything given in `p` overrides them.
    // Rounded to a micron so 3 × 3.175 reads 9.525.
    let micron = |v: f64| (v * 1000.0).round() / 1000.0;
    let flute_length = micron(3.0 * diameter);
    let mut t = Tool {
        number,
        name: format!(
            "Ø{} {} {} end mill",
            units.plain(diameter),
            units.length(),
            if kind == "ball" { "ball" } else { "flat" }
        ),
        kind: kind.to_owned(),
        diameter,
        corner_radius: if kind == "ball" { diameter / 2.0 } else { 0.0 },
        flutes: if diameter < 3.0 { 2 } else { 3 },
        flute_length,
        overall_length: micron(flute_length + (2.0 * diameter).max(15.0)),
        shank_diameter: diameter,
        material: "carbide".into(),
        coating: String::new(),
        spindle_rpm: 0.0,
        feed: 0.0,
        plunge_feed: 0.0,
        stepover: 0.0,
        stepdown: 0.0,
        length_offset: number,
        diameter_offset: number,
        notes: String::new(),
    };
    apply(&mut t, p)?;
    m.check_tool(&t).map_err(EngineError::InvalidParams)?;
    let result = t.to_json(&mid);
    m.tools.push(t);
    m.tools.sort_by_key(|t| t.number);
    s.view.selection = Some(tool_ref(&mid, number));
    s.library.changed();
    Ok(result)
}

fn edit(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let mid = tool_machine(s, p)?;
    let number = number(p, "number")?;
    let m = s.library.machine_mut(&mid).ok_or_else(|| missing_machine(&mid))?;
    let mut t = m.tool(number).cloned().ok_or_else(|| missing_tool(number))?;
    apply(&mut t, p)?;
    if let Some(n) = p.get("new_number").and_then(Value::as_u64) {
        let n = u32::try_from(n).unwrap_or(0);
        if n != number && m.tool(n).is_some() {
            return Err(EngineError::InvalidParams(format!("T{n} is already in the library")));
        }
        t.number = n;
    }
    m.check_tool(&t).map_err(EngineError::InvalidParams)?;
    let new_number = t.number;
    m.tools.retain(|o| o.number != number);
    let result = t.to_json(&mid);
    m.tools.push(t);
    m.tools.sort_by_key(|t| t.number);
    if new_number != number && s.view.selection.as_deref() == Some(tool_ref(&mid, number).as_str()) {
        s.view.selection = Some(tool_ref(&mid, new_number));
    }
    s.library.changed();
    Ok(result)
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
    if doc.get("format").and_then(Value::as_str) != Some(TOOLS_FORMAT) {
        return Err(EngineError::InvalidParams(format!("{path} is not a CosmicCAM tool library ({TOOLS_FORMAT})")));
    }
    let m = s.library.machine_mut(&mid).ok_or_else(|| missing_machine(&mid))?;
    let mut incoming: Vec<Tool> = Vec::new();
    for (i, raw) in doc.get("tools").and_then(Value::as_array).into_iter().flatten().enumerate().take(MAX_TOOLS) {
        let mut t: Tool = serde_json::from_value(raw.clone())
            .map_err(|e| EngineError::InvalidParams(format!("tool {} in {path}: {e}", i + 1)))?;
        t.name = clip(&t.name);
        t.coating = clip(&t.coating);
        t.notes = clip(&t.notes);
        m.check_tool(&t).map_err(|e| EngineError::InvalidParams(format!("T{} in {path}: {e}", t.number)))?;
        if incoming.iter().any(|o| o.number == t.number) {
            return Err(EngineError::InvalidParams(format!("{path} lists T{} twice", t.number)));
        }
        if !replace && m.tool(t.number).is_some() {
            return Err(EngineError::InvalidParams(format!(
                "T{} is already in the library; import with replace = true to overwrite",
                t.number
            )));
        }
        incoming.push(t);
    }
    let count = incoming.len();
    m.tools.retain(|o| incoming.iter().all(|t| t.number != o.number));
    if m.tools.len() + count > MAX_TOOLS {
        return Err(EngineError::InvalidParams(format!("a library holds at most {MAX_TOOLS} tools")));
    }
    m.tools.extend(incoming);
    m.tools.sort_by_key(|t| t.number);
    s.library.changed();
    Ok(json!({ "imported": count, "machine": mid }))
}
