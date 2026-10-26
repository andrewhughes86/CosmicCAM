//! L4 engine: the session, its commands and their errors.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
#![allow(non_snake_case)]

pub mod cmd;
pub mod demo;
pub mod error;
pub mod fiveaxis;
pub mod jobfile;
pub mod library;
pub mod params;
pub mod post;
pub mod postproc;
pub mod session;
pub mod sim;
pub mod tooling;
pub mod toolpath;
pub mod units;

pub use cmd::{ButtonSize, CommandSpec, Param, ParamKind, Placement, RibbonTab};
pub use error::{EngineError, EngineResult, error_json};
pub use session::{Display, Model, Pane, Session, Theme, UiRequest};

#[cfg(test)]
mod tests;
