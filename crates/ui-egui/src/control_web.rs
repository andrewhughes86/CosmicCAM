//! The control channel in the browser: there is no localhost socket to listen on, so the
//! server never starts. Same shape as the desktop module so the app compiles unchanged.

use serde_json::Value;

/// Version of the wire protocol, reported by `app.ping`.
pub const PROTOCOL_VERSION: u32 = 1;

/// One request waiting for the app. None ever arrive in the browser.
pub struct Incoming {
    pub id: Value,
    pub method: String,
    pub params: Value,
}

impl Incoming {
    pub fn ok(self, _result: Value) {}

    pub fn err(self, _kind: &str, _message: impl Into<String>) {}
}

pub struct ControlServer;

impl ControlServer {
    pub fn start(_port: u16, _wake: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        Err("the control channel is not available in the browser".into())
    }

    pub fn port(&self) -> u16 {
        0
    }

    pub fn poll(&self) -> Vec<Incoming> {
        Vec::new()
    }
}
