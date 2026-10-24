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
pub mod error;
pub mod params;
pub mod session;

pub use cmd::{ButtonSize, CommandSpec, Param, ParamKind, Placement, RibbonTab};
pub use error::{EngineError, EngineResult, error_json};
pub use session::{Display, Pane, Session, Theme, UiRequest};

#[cfg(test)]
mod tests;
