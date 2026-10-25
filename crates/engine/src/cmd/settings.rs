//! Application settings: the units lengths and feeds are shown in. Settings are kept across
//! jobs and saved by the app.

use serde_json::{Value, json};

use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::session::Session;
use crate::units::DisplayUnits;

const SETTINGS_RETURNS: &str = "{units, theme, fps_limit}";

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "settings.open",
        label: "Settings",
        tooltip: "Units and other preferences",
        placement: Some(Placement::large(RibbonTab::Home, "Settings")),
        icon: None,
        shortcut: None,
        params: &[],
        returns: SETTINGS_RETURNS,
        enabled: always,
        run: |s, _| Ok(settings_json(s)),
    },
    CommandSpec {
        id: "settings.get",
        label: "Get settings",
        tooltip: "The current preferences",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: SETTINGS_RETURNS,
        enabled: always,
        run: |s, _| Ok(settings_json(s)),
    },
    CommandSpec {
        id: "settings.units.set",
        label: "Units",
        tooltip: "Show lengths and feeds in millimetres or inches (commands always use millimetres)",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("units", ParamKind::Choice(DisplayUnits::IDS), "in or mm")],
        returns: SETTINGS_RETURNS,
        enabled: always,
        run: set_units,
    },
];

fn settings_json(s: &Session) -> Value {
    json!({ "units": s.prefs.units.id(), "theme": s.prefs.theme.id(), "fps_limit": s.prefs.fps_limit })
}

fn set_units(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let id = params::req_str(p, "units")?;
    s.prefs.units =
        DisplayUnits::from_id(id).ok_or_else(|| EngineError::InvalidParams(format!("unknown units `{id}`")))?;
    s.prefs.revision = s.prefs.revision.wrapping_add(1);
    Ok(settings_json(s))
}
