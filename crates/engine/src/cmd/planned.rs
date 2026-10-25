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

use RibbonTab::{Export, Setup, Simulation, Toolpaths};

pub(super) static COMMANDS: &[CommandSpec] = &[
    // Setup (Phases 3 and 5)
    planned!("setup.stock.cylinder", "Cylinder stock", "Round bar stock (Phase 5)", large(Setup, "Stock")),
    planned!("setup.stock.margins", "Margins", "Extra stock around chosen faces (Phase 5)", small(Setup, "Stock")),
    // Toolpaths (Phase 6)
    planned!(
        "toolpath.cl.export",
        "CL data",
        "Write machine-neutral cutter location data (Phase 6)",
        large(Toolpaths, "Output")
    ),
    // Simulation (Phase 7)
    planned!("sim.play", "Play", "Run the material removal simulation (Phase 7)", large(Simulation, "Playback")),
    planned!("sim.pause", "Pause", "Pause the simulation (Phase 7)", small(Simulation, "Playback")),
    planned!("sim.step", "Step", "Advance one move (Phase 7)", small(Simulation, "Playback")),
    planned!("sim.speed", "Speed", "Playback speed (Phase 7)", small(Simulation, "Playback")),
    planned!(
        "sim.collision.check",
        "Collisions",
        "Check holder and shank against stock and fixtures (Phase 7)",
        large(Simulation, "Verify")
    ),
    planned!("sim.gouge.check", "Gouges", "Check for overcut beyond tolerance (Phase 7)", large(Simulation, "Verify")),
    // Export (Phase 8)
    planned!(
        "post.processor.select",
        "Post-processor",
        "Choose the machine controller dialect (Phase 8)",
        large(Export, "Post")
    ),
    planned!(
        "post.limits.check",
        "Check limits",
        "Validate travel, spindle and feed limits (Phase 8)",
        small(Export, "Post")
    ),
];
