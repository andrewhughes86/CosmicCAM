//! Workspace and window commands. The engine records the request; the UI carries it out
//! on its next frame.

use serde_json::{Value, json};

use super::{CommandSpec, Mode, Param, ParamKind, RibbonTab, always};
use crate::error::EngineError;
use crate::params;
use crate::session::{Pane, Session, UiRequest};

const PANE: Param = Param::required("pane", ParamKind::Choice(Pane::IDS), "Workspace pane");
const ENABLED: Param = Param::required("enabled", ParamKind::Bool, "On or off");
const WINDOW_SIDE: ParamKind = ParamKind::Number { min: 320.0, max: 16384.0 };

fn pane(p: &serde_json::Value) -> Result<Pane, EngineError> {
    let id = params::req_str(p, "pane")?;
    Pane::from_id(id).ok_or_else(|| EngineError::InvalidParams(format!("unknown pane `{id}`")))
}

fn queue(s: &mut Session, r: UiRequest) {
    s.ui.request(r);
}

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "ui.ribbon.tab.set",
        label: "Ribbon tab",
        tooltip: "Show a ribbon tab",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("tab", ParamKind::Choice(RibbonTab::IDS), "Ribbon tab")],
        returns: "{\"tab\": id}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "tab")?;
            s.ui.ribbon_tab =
                RibbonTab::from_id(id).ok_or_else(|| EngineError::InvalidParams(format!("unknown tab `{id}`")))?;
            // A tab of the other mode's ribbon switches to that mode.
            s.ui.mode = s.ui.ribbon_tab.mode();
            Ok(json!({ "tab": s.ui.ribbon_tab.id(), "mode": s.ui.mode.id() }))
        },
    },
    CommandSpec {
        id: "ui.mode.set",
        label: "Mode",
        tooltip: "Switch between CAD (modelling the part) and CAM (machining it); each has its own ribbon",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("mode", ParamKind::Choice(Mode::IDS), "cad or cam (drawings and slicer are coming)")],
        returns: "{\"mode\": id, \"tab\": the ribbon tab shown}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "mode")?;
            let mode = Mode::from_id(id).ok_or_else(|| EngineError::InvalidParams(format!("unknown mode `{id}`")))?;
            if !mode.available() {
                return Err(EngineError::NotSupported(format!("{} mode is coming soon", mode.label())));
            }
            if s.ui.mode != mode {
                s.ui.mode = mode;
                s.ui.ribbon_tab = mode.tabs().first().copied().unwrap_or(RibbonTab::Home);
            }
            Ok(json!({ "mode": s.ui.mode.id(), "tab": s.ui.ribbon_tab.id() }))
        },
    },
    CommandSpec {
        id: "ui.mode.get",
        label: "Current mode",
        tooltip: "Which mode the application is in, and the modes there are",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "{\"mode\": id, \"modes\": [{id, label, available}]}",
        enabled: always,
        run: |s, _| {
            let modes: Vec<Value> = Mode::ALL
                .iter()
                .map(|m| json!({ "id": m.id(), "label": m.label(), "available": m.available() }))
                .collect();
            Ok(json!({ "mode": s.ui.mode.id(), "modes": modes }))
        },
    },
    CommandSpec {
        id: "ui.pane.show",
        label: "Show pane",
        tooltip: "Open a workspace pane (it is docked back if it was closed)",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[PANE],
        returns: "{\"pane\": id}",
        enabled: always,
        run: |s, p| {
            let pane = pane(p)?;
            queue(s, UiRequest::ShowPane(pane));
            Ok(json!({ "pane": pane.id() }))
        },
    },
    CommandSpec {
        id: "ui.pane.hide",
        label: "Hide pane",
        tooltip: "Close a workspace pane (the viewport always stays)",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[PANE],
        returns: "{\"pane\": id}",
        enabled: always,
        run: |s, p| {
            let pane = pane(p)?;
            if pane == Pane::Viewport {
                return Err(EngineError::InvalidParams("the viewport pane cannot be hidden".into()));
            }
            queue(s, UiRequest::HidePane(pane));
            Ok(json!({ "pane": pane.id() }))
        },
    },
    CommandSpec {
        id: "ui.pane.focus",
        label: "Focus pane",
        tooltip: "Bring a pane to the front of its tab stack, opening it if needed",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[PANE],
        returns: "{\"pane\": id}",
        enabled: always,
        run: |s, p| {
            let pane = pane(p)?;
            queue(s, UiRequest::FocusPane(pane));
            Ok(json!({ "pane": pane.id() }))
        },
    },
    CommandSpec {
        id: "ui.window.size.set",
        label: "Window size",
        tooltip: "Resize the main window",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("width", WINDOW_SIDE, "Width in points"),
            Param::required("height", WINDOW_SIDE, "Height in points"),
        ],
        returns: "{\"width\": w, \"height\": h}",
        enabled: always,
        run: |s, p| {
            let w = params::opt_f64(p, "width", 320.0, 16384.0)?.unwrap_or(1600.0) as f32;
            let h = params::opt_f64(p, "height", 320.0, 16384.0)?.unwrap_or(1000.0) as f32;
            queue(s, UiRequest::WindowSize { width: w, height: h });
            Ok(json!({ "width": w, "height": h }))
        },
    },
    CommandSpec {
        id: "ui.window.fullscreen.set",
        label: "Full screen",
        tooltip: "Enter or leave full screen",
        placement: None,
        icon: None,
        shortcut: Some("F11"),
        params: &[ENABLED],
        returns: "{\"enabled\": bool}",
        enabled: always,
        run: |s, p| {
            let on = params::opt_bool(p, "enabled")?.unwrap_or(true);
            queue(s, UiRequest::Fullscreen(on));
            Ok(json!({ "enabled": on }))
        },
    },
    CommandSpec {
        id: "ui.window.maximize.set",
        label: "Maximise",
        tooltip: "Maximise or restore the main window",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[ENABLED],
        returns: "{\"enabled\": bool}",
        enabled: always,
        run: |s, p| {
            let on = params::opt_bool(p, "enabled")?.unwrap_or(true);
            queue(s, UiRequest::Maximized(on));
            Ok(json!({ "enabled": on }))
        },
    },
    CommandSpec {
        id: "app.quit",
        label: "Quit",
        tooltip: "Close CosmicDMS",
        placement: None,
        icon: None,
        shortcut: Some("Ctrl+Q"),
        params: &[],
        returns: "null",
        enabled: always,
        run: |s, _| {
            queue(s, UiRequest::Quit);
            Ok(serde_json::Value::Null)
        },
    },
];
