//! The demo project shipped with the app while it is in pre-release: a part with a pocket and a
//! boss, a WCS, a setup with its stock, two tools and a set of toolpaths, ready to look at,
//! change and post. It is `samples/demo.cosmic`, built in.

use crate::error::EngineResult;
use crate::jobfile;
use crate::session::Session;

/// The demo project's job file.
pub const DEMO: &str = include_str!("../../../samples/demo.cosmic");

impl Session {
    /// Opens the demo project in a tab (replacing an empty one). Its tools are added to the
    /// library where they are missing, so its toolpaths post.
    pub fn open_demo(&mut self) -> EngineResult<usize> {
        let mut loaded = jobfile::from_str(DEMO)?;
        loaded.job.name = "Demo project".into();
        loaded.job.path = None;
        self.open_loaded(loaded)
    }
}
