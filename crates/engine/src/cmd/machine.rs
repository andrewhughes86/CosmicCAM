//! Machine configurations: create, edit, copy, remove and choose the machine whose tool
//! library the Tooling tab works on.

use serde_json::{Value, json};

use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::error::{EngineError, EngineResult};
use crate::library::{MACHINE_KINDS, MAX_MACHINES, MAX_TOOL_NUMBER, Machine, clip};
use crate::params;
use crate::session::{Pane, Session, UiRequest};

const MACHINE_RETURNS: &str =
    "the machine {id, name, description, kind, active, travel, max_spindle_rpm, max_feed, tool_capacity, post, tools}";
const MACHINE_MENU: &[&str] = &["machine.duplicate", "machine.remove"];

const TRAVEL: ParamKind = ParamKind::Length { min: 0.001, max: 100_000.0 };
const RPM: ParamKind = ParamKind::Number { min: 1.0, max: 500_000.0 };
const FEED: ParamKind = ParamKind::Rate { min: 0.001, max: 1.0e6 };
const CAPACITY: ParamKind = ParamKind::Integer { min: 0, max: MAX_TOOL_NUMBER as i64 };

/// The settings shared by create and edit, then `extra`.
macro_rules! machine_params {
    ($($extra:expr),* $(,)?) => {
        &[
            $($extra,)*
            Param::optional("name", ParamKind::Str, "Machine name"),
            Param::optional("description", ParamKind::Str, "Notes about the machine"),
            Param::optional("kind", ParamKind::Choice(MACHINE_KINDS), "Machine type (3-axis mill for now)"),
            Param::optional("travel_x", TRAVEL, "X travel, mm"),
            Param::optional("travel_y", TRAVEL, "Y travel, mm"),
            Param::optional("travel_z", TRAVEL, "Z travel, mm"),
            Param::optional("max_spindle_rpm", RPM, "Highest spindle speed, rpm"),
            Param::optional("max_feed", FEED, "Fastest cutting feed, mm/min"),
            Param::optional("tool_capacity", CAPACITY, "Tool changer pockets (0 = manual tool changes)"),
        ]
    };
}

const MACHINE_PARAM: Param =
    Param::optional("id", ParamKind::Str, "Machine id, e.g. machine.1 (default: the active machine)");

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "machine.create",
        label: "New machine",
        tooltip: "Add a machine configuration with its own, empty tool library",
        placement: Some(Placement::large(RibbonTab::Machine, "Machines").with_menu(MACHINE_MENU)),
        icon: None,
        shortcut: None,
        params: machine_params!(Param::optional(
            "activate",
            ParamKind::Bool,
            "Make it the active machine (default true)"
        )),
        returns: MACHINE_RETURNS,
        enabled: always,
        run: |s, p| {
            if s.library.machines.len() >= MAX_MACHINES {
                return Err(EngineError::InvalidParams(format!("a library holds at most {MAX_MACHINES} machines")));
            }
            let id = format!("machine.{}", s.library.next_machine);
            let mut m = Machine::generic(id.clone());
            m.name = format!("Machine {}", s.library.next_machine);
            apply(&mut m, p)?;
            s.library.next_machine = s.library.next_machine.saturating_add(1);
            let activate = params::opt_bool(p, "activate")?.unwrap_or(true);
            if activate {
                s.library.active = Some(id);
            }
            let result = m.to_json(activate);
            s.library.machines.push(m);
            s.library.changed();
            Ok(result)
        },
    },
    CommandSpec {
        id: "machine.edit",
        label: "Edit machine",
        tooltip: "Change a machine's name, travel and limits",
        placement: Some(Placement::large(RibbonTab::Machine, "Machines")),
        icon: None,
        shortcut: None,
        params: machine_params!(MACHINE_PARAM),
        returns: MACHINE_RETURNS,
        enabled: always,
        run: |s, p| {
            let id = machine_id(s, p)?;
            let active = s.library.active.as_deref() == Some(id.as_str());
            let m = s.library.machine_mut(&id).ok_or_else(|| missing(&id))?;
            apply(m, p)?;
            // Tools must still fit the machine's new limits.
            for t in &m.tools {
                m.check_tool(t)
                    .map_err(|e| EngineError::InvalidParams(format!("T{} no longer fits: {e}", t.number)))?;
            }
            let result = m.to_json(active);
            s.library.changed();
            Ok(result)
        },
    },
    CommandSpec {
        id: "machine.duplicate",
        label: "Copy machine",
        tooltip: "Copy a machine and its whole tool library under a new name",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[MACHINE_PARAM, Param::optional("name", ParamKind::Str, "Name of the copy (default: <name> copy)")],
        returns: MACHINE_RETURNS,
        enabled: always,
        run: |s, p| {
            if s.library.machines.len() >= MAX_MACHINES {
                return Err(EngineError::InvalidParams(format!("a library holds at most {MAX_MACHINES} machines")));
            }
            let source = machine_id(s, p)?;
            let mut m = s.library.machine(&source).cloned().ok_or_else(|| missing(&source))?;
            m.id = format!("machine.{}", s.library.next_machine);
            m.name = match params::opt_str(p, "name")?.map(clip) {
                Some(n) if !n.is_empty() => n,
                _ => clip(&format!("{} copy", m.name)),
            };
            s.library.next_machine = s.library.next_machine.saturating_add(1);
            s.library.active = Some(m.id.clone());
            let result = m.to_json(true);
            s.library.machines.push(m);
            s.library.changed();
            Ok(result)
        },
    },
    CommandSpec {
        id: "machine.remove",
        label: "Delete machine",
        tooltip: "Delete a machine and its tool library",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Machine id, e.g. machine.2")],
        returns: "{\"removed\": id, \"active\": id}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            if s.library.machine(id).is_none() {
                return Err(missing(id));
            }
            if s.library.machines.len() == 1 {
                return Err(EngineError::InvalidParams("the library keeps at least one machine".into()));
            }
            s.library.machines.retain(|m| m.id != id);
            if s.library.active.as_deref() == Some(id) {
                s.library.active = s.library.machines.first().map(|m| m.id.clone());
            }
            if s.view.selection.as_deref().is_some_and(|sel| sel.starts_with(&format!("{id}/"))) {
                s.view.selection = None;
            }
            s.library.changed();
            Ok(json!({ "removed": id, "active": s.library.active }))
        },
    },
    CommandSpec {
        id: "machine.activate",
        label: "Use machine",
        tooltip: "Work with this machine and its tool library",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Machine id, e.g. machine.1")],
        returns: MACHINE_RETURNS,
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            let m = s.library.machine(id).ok_or_else(|| missing(id))?.to_json(true);
            s.library.active = Some(id.to_owned());
            s.library.changed();
            Ok(m)
        },
    },
    CommandSpec {
        id: "machine.list",
        label: "Machines",
        tooltip: "Every machine configuration",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "list of machines {id, name, ..., active, tools}",
        enabled: always,
        run: |s, _| {
            let active = s.library.active.as_deref();
            Ok(json!(s.library.machines.iter().map(|m| m.to_json(active == Some(&m.id))).collect::<Vec<_>>()))
        },
    },
    CommandSpec {
        id: "machine.get",
        label: "Machine",
        tooltip: "One machine configuration",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[MACHINE_PARAM],
        returns: MACHINE_RETURNS,
        enabled: always,
        run: |s, p| {
            let id = machine_id(s, p)?;
            let active = s.library.active.as_deref() == Some(id.as_str());
            Ok(s.library.machine(&id).ok_or_else(|| missing(&id))?.to_json(active))
        },
    },
    CommandSpec {
        id: "machine.library.open",
        label: "Machines & tools",
        tooltip: "Show the machine and tool library pane",
        placement: Some(Placement::large(RibbonTab::Machine, "Library")),
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
];

pub(super) fn missing(id: &str) -> EngineError {
    EngineError::InvalidParams(format!("no machine `{id}`"))
}

/// The machine `id` names, or the active one.
pub(super) fn machine_id(s: &Session, p: &Value) -> EngineResult<String> {
    match params::opt_str(p, "id")? {
        Some(id) => Ok(id.to_owned()),
        None => s.library.active.clone().ok_or_else(|| EngineError::InvalidParams("there is no machine yet".into())),
    }
}

/// Applies the settings given in `p` and checks the result.
fn apply(m: &mut Machine, p: &Value) -> EngineResult<()> {
    if let Some(n) = params::opt_str(p, "name")? {
        m.name = clip(n);
    }
    if let Some(d) = params::opt_str(p, "description")? {
        m.description = clip(d);
    }
    if let Some(k) = params::opt_str(p, "kind")? {
        m.kind = k.to_owned();
    }
    for (i, key) in ["travel_x", "travel_y", "travel_z"].into_iter().enumerate() {
        if let (Some(v), Some(slot)) = (params::opt_f64(p, key, 0.001, 100_000.0)?, m.travel.get_mut(i)) {
            *slot = v;
        }
    }
    if let Some(v) = params::opt_f64(p, "max_spindle_rpm", 1.0, 500_000.0)? {
        m.max_spindle_rpm = v;
    }
    if let Some(v) = params::opt_f64(p, "max_feed", 0.001, 1.0e6)? {
        m.max_feed = v;
    }
    if let Some(v) = p.get("tool_capacity").and_then(Value::as_u64) {
        m.tool_capacity = u32::try_from(v).unwrap_or(MAX_TOOL_NUMBER);
    }
    m.validate().map_err(EngineError::InvalidParams)
}
