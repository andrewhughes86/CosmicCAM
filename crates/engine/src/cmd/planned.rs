//! Ribbon commands for later roadmap phases. They are registered now so the ribbon,
//! console and agents see the full surface; each one is disabled and returns
//! "not supported yet" until its phase lands.

use serde_json::Value;

use super::{CommandSpec, Placement, RibbonTab, never};
use crate::error::{EngineError, EngineResult};
use crate::session::Session;

macro_rules! planned {
    ($id:literal, $label:literal, $tooltip:literal, $placement:expr) => {
        CommandSpec {
            id: $id,
            label: $label,
            tooltip: $tooltip,
            placement: $placement,
            icon: None,
            shortcut: None,
            params: &[],
            returns: "not defined yet",
            enabled: never,
            run: |_: &mut Session, _: &Value| -> EngineResult<Value> { Err(EngineError::NotSupported($id.into())) },
        }
    };
}

const fn large(tab: RibbonTab, panel: &'static str) -> Option<Placement> {
    Some(Placement::large(tab, panel))
}

const fn small(tab: RibbonTab, panel: &'static str) -> Option<Placement> {
    Some(Placement::small(tab, panel))
}

use RibbonTab::Setup;

pub(super) static COMMANDS: &[CommandSpec] = &[
    // Setup (Phases 3 and 5)
    planned!("setup.stock.cylinder", "Cylinder stock", "Round bar stock (Phase 5)", large(Setup, "Stock")),
    planned!("setup.stock.margins", "Margins", "Extra stock around chosen faces (Phase 5)", small(Setup, "Stock")),
];
