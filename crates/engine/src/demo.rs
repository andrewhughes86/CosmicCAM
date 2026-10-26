//! The demo project shipped with the app while it is in pre-release: a part with a pocket and a
//! boss, a WCS, a setup with its stock, two tools and a set of toolpaths, ready to look at,
//! change and post. It is `samples/demo.cosmic`, built in.

use crate::error::EngineResult;
use crate::jobfile;
use crate::session::Session;

/// The demo project's machine: opening the demo adds it to the library when it is missing.
pub const DEMO_MACHINE: &str = "Demo 5-axis trunnion";

/// The demo project's job file.
pub const DEMO: &str = include_str!("../../../samples/demo.cosmic");

impl Session {
    /// Opens the demo project in a tab (replacing an empty one). Its tools are added to the
    /// library where they are missing, so its toolpaths post.
    ///
    /// The demo runs on its own 5-axis machine, [`DEMO_MACHINE`], added to the library with the
    /// sample tools when it is missing (tools already there are left as they are) and made the
    /// active machine.
    pub fn open_demo(&mut self) -> EngineResult<usize> {
        let mut loaded = jobfile::from_str(DEMO)?;
        loaded.job.name = "Demo project".into();
        loaded.job.path = None;
        let mid = self.demo_machine()?;
        crate::cmd::tool::load_sample(self, &mid)?;
        for tp in &mut loaded.job.toolpaths {
            tp.machine = mid.clone();
        }
        for setup in &mut loaded.job.setups {
            setup.machine = mid.clone();
        }
        loaded.tools.clear();
        self.library.active = Some(mid);
        self.library.changed();
        self.open_loaded(loaded)
    }

    /// The demo's machine: found by name, else added.
    fn demo_machine(&mut self) -> EngineResult<String> {
        if let Some(m) = self.library.machines.iter().find(|m| m.name == DEMO_MACHINE && m.kinematics.is_some()) {
            return Ok(m.id.clone());
        }
        if self.library.machines.len() >= crate::library::MAX_MACHINES {
            return Err(crate::error::EngineError::InvalidParams(
                "the machine library is full; remove a machine to open the demo".into(),
            ));
        }
        let id = format!("machine.{}", self.library.next_machine);
        self.library.next_machine = self.library.next_machine.saturating_add(1);
        let mut m = crate::library::Machine::generic(id.clone());
        m.name = DEMO_MACHINE.into();
        m.description = "The demo project's machine: a trunnion with A tilting ±120° and a continuous C".into();
        m.kind = "mill_5axis".into();
        m.kinematics = crate::library::Kinematics::typical("mill_5axis");
        m.travel = [600.0, 500.0, 500.0];
        m.max_spindle_rpm = 20_000.0;
        m.max_feed = 20_000.0;
        m.tool_capacity = 40;
        m.post = "fanuc".into();
        m.validate().map_err(crate::error::EngineError::Internal)?;
        self.library.machines.push(m);
        Ok(id)
    }
}
