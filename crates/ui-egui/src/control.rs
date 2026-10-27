//! The control channel: JSON lines over a localhost TCP socket, used by the Python API,
//! scripts and agents. See `docs/control-protocol.md`.
//!
//! A listener thread accepts connections; each connection thread reads one request per
//! line, checks the token on the first one, hands the request to the app and writes the
//! reply. The app answers requests in [`crate::CosmicApp`] on the UI thread, where the
//! session lives.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use serde_json::{Value, json};

/// Version of the wire protocol, reported by `app.ping`.
pub const PROTOCOL_VERSION: u32 = 1;
/// Longest request line accepted.
const MAX_LINE_BYTES: u64 = 1 << 20;
const MAX_CONNECTIONS: usize = 16;
/// How long a connection waits for the app to answer (screenshots wait for a frame).
const REPLY_TIMEOUT: Duration = Duration::from_secs(60);
/// Environment variable that fixes the token instead of generating one.
pub const TOKEN_ENV: &str = "CosmicDMS_CONTROL_TOKEN";
/// The token's variable under the application's former name, still read.
const LEGACY_TOKEN_ENV: &str = "CosmicCAM_CONTROL_TOKEN";

/// One request waiting for the app.
pub struct Incoming {
    pub id: Value,
    pub method: String,
    pub params: Value,
    reply: Sender<String>,
}

impl Incoming {
    pub fn ok(self, result: Value) {
        let _ = self.reply.send(json!({ "id": self.id, "result": result }).to_string());
    }

    pub fn err(self, kind: &str, message: impl Into<String>) {
        let line = json!({ "id": self.id, "error": { "kind": kind, "message": message.into() } }).to_string();
        let _ = self.reply.send(line);
    }
}

pub struct ControlServer {
    rx: Receiver<Incoming>,
    port: u16,
    token_file: Option<PathBuf>,
}

impl ControlServer {
    /// Listens on `127.0.0.1:port` (0 picks a free port). `wake` is called whenever a
    /// request arrives so the app processes it even when idle.
    pub fn start(port: u16, wake: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let token = match std::env::var(TOKEN_ENV).or_else(|_| std::env::var(LEGACY_TOKEN_ENV)) {
            Ok(t) if t.len() >= 16 => t,
            Ok(_) => return Err(format!("{TOKEN_ENV} must be at least 16 characters")),
            Err(_) => new_token()?,
        };
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|e| format!("cannot listen on port {port}: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let token_file = write_token_file(port, &token);
        let (tx, rx) = channel();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let token: Arc<str> = token.into();
        let live = Arc::new(AtomicUsize::new(0));
        std::thread::Builder::new()
            .name("control-listener".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    if live.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                        let _ = reject(stream, "too many connections");
                        continue;
                    }
                    live.fetch_add(1, Ordering::SeqCst);
                    let (tx, wake, token, live) =
                        (tx.clone(), Arc::clone(&wake), Arc::clone(&token), Arc::clone(&live));
                    let spawned = std::thread::Builder::new().name("control-connection".into()).spawn(move || {
                        if let Err(e) = serve(stream, &token, &tx, wake.as_ref()) {
                            log::debug!("control connection closed: {e}");
                        }
                        live.fetch_sub(1, Ordering::SeqCst);
                    });
                    if spawned.is_err() {
                        log::warn!("control: could not start a connection thread");
                    }
                }
            })
            .map_err(|e| format!("cannot start the control thread: {e}"))?;
        Ok(Self { rx, port, token_file })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Requests that arrived since the last call.
    pub fn poll(&self) -> Vec<Incoming> {
        self.rx.try_iter().collect()
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        if let Some(path) = &self.token_file {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn serve(
    stream: TcpStream,
    token: &str,
    tx: &Sender<Incoming>,
    wake: &(dyn Fn() + Send + Sync),
) -> std::io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut authenticated = false;
    loop {
        let mut line = Vec::new();
        let n = reader.by_ref().take(MAX_LINE_BYTES + 1).read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(());
        }
        if n as u64 > MAX_LINE_BYTES {
            write_line(&mut writer, &error_line(&Value::Null, "invalid_request", "request line is too long"))?;
            return Ok(());
        }
        let request: Value = match serde_json::from_slice(&line) {
            Ok(v) => v,
            Err(e) => {
                write_line(&mut writer, &error_line(&Value::Null, "invalid_request", &format!("not JSON: {e}")))?;
                continue;
            }
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        if !authenticated {
            let given = request.get("token").and_then(Value::as_str).unwrap_or_default();
            if !constant_time_eq(given.as_bytes(), token.as_bytes()) {
                write_line(&mut writer, &error_line(&id, "unauthorized", "missing or wrong token"))?;
                return Ok(());
            }
            authenticated = true;
        }
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            write_line(&mut writer, &error_line(&id, "invalid_request", "`method` must be a string"))?;
            continue;
        };
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let (reply_tx, reply_rx) = channel();
        let incoming = Incoming { id: id.clone(), method: method.chars().take(128).collect(), params, reply: reply_tx };
        if tx.send(incoming).is_err() {
            write_line(&mut writer, &error_line(&id, "unavailable", "the application is shutting down"))?;
            return Ok(());
        }
        wake();
        let reply = reply_rx
            .recv_timeout(REPLY_TIMEOUT)
            .unwrap_or_else(|_| error_line(&id, "timeout", "the application did not answer in time"));
        write_line(&mut writer, &reply)?;
    }
}

fn reject(mut stream: TcpStream, message: &str) -> std::io::Result<()> {
    write_line(&mut stream, &error_line(&Value::Null, "unavailable", message))
}

fn error_line(id: &Value, kind: &str, message: &str) -> String {
    json!({ "id": id, "error": { "kind": kind, "message": message } }).to_string()
}

fn write_line(w: &mut impl Write, line: &str) -> std::io::Result<()> {
    w.write_all(line.as_bytes())?;
    w.write_all(b"\n")?;
    w.flush()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn new_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("no system randomness for the control token: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Per-user data folder: `~/.local/share/CosmicDMS` (Linux), `~/Library/Application Support/CosmicDMS`
/// (macOS), `%APPDATA%\CosmicDMS` (Windows).
pub fn data_dir() -> Option<PathBuf> {
    data_base().map(|b| b.join("CosmicDMS"))
}

/// Moves the data folder (settings, libraries, posts, recovery and crash reports) from the
/// application's former name, CosmicCAM, to its own, the first time it starts after the rename.
/// Returns what it did, for the log.
pub fn migrate_data_dir() -> Option<String> {
    let base = data_base()?;
    // The window framework keeps its own storage (settings, the machine library) under the
    // name in lower case on Linux; it moves too.
    let notes: Vec<String> = [("CosmicCAM", "CosmicDMS"), ("cosmiccam", "cosmicdms")]
        .into_iter()
        .filter_map(|(from, to)| {
            let (old, new) = (base.join(from), base.join(to));
            if !old.is_dir() || new.exists() {
                return None;
            }
            Some(match std::fs::rename(&old, &new) {
                Ok(()) => format!("moved the data folder {} to {}", old.display(), new.display()),
                Err(e) => format!("could not move the data folder {} to {}: {e}", old.display(), new.display()),
            })
        })
        .collect();
    (!notes.is_empty()).then(|| notes.join("; "))
}

/// The folder per-user data folders go in.
fn data_base() -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".local/share")))
    }
}

/// `control/PORT.token`, readable by the user only.
fn write_token_file(port: u16, token: &str) -> Option<PathBuf> {
    let dir = data_dir()?.join("control");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{port}.token"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).ok()?;
    file.write_all(token.as_bytes()).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connect(port: u16) -> (BufReader<TcpStream>, TcpStream) {
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap_or_else(|e| fail(&e.to_string()));
        let reader = BufReader::new(stream.try_clone().unwrap_or_else(|e| fail(&e.to_string())));
        (reader, stream)
    }

    #[allow(clippy::panic)]
    fn fail(msg: &str) -> ! {
        panic!("{msg}")
    }

    fn roundtrip(reader: &mut BufReader<TcpStream>, w: &mut TcpStream, line: &str) -> Value {
        let _ = write_line(w, line);
        let mut out = String::new();
        let _ = reader.read_line(&mut out);
        serde_json::from_str(&out).unwrap_or(Value::Null)
    }

    #[test]
    fn token_is_required_and_requests_reach_the_app() {
        let token = "0123456789abcdef0123456789abcdef";
        let server = start_with_token(token);
        let port = server.port();
        let answer = std::thread::spawn(move || {
            for _ in 0..200 {
                if let Some(incoming) = server.poll().into_iter().next() {
                    let echo = json!({ "method": incoming.method, "params": incoming.params });
                    incoming.ok(echo);
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });

        let (mut r, mut w) = connect(port);
        let denied = roundtrip(&mut r, &mut w, r#"{"id":1,"method":"app.ping","token":"nope"}"#);
        assert_eq!(denied["error"]["kind"], "unauthorized");

        let (mut r, mut w) = connect(port);
        let bad = roundtrip(&mut r, &mut w, "not json");
        assert_eq!(bad["error"]["kind"], "invalid_request");
        let line =
            format!(r#"{{"id":7,"method":"engine.execute","params":{{"command":"view.fit"}},"token":"{token}"}}"#);
        let ok = roundtrip(&mut r, &mut w, &line);
        assert_eq!(ok["id"], 7);
        assert_eq!(ok["result"]["method"], "engine.execute");
        assert_eq!(ok["result"]["params"]["command"], "view.fit");
        let _ = answer.join();
    }

    fn start_with_token(token: &str) -> ControlServer {
        // The token comes from the environment in production; tests pass it directly.
        let (tx, rx) = channel();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap_or_else(|e| fail(&e.to_string()));
        let port = listener.local_addr().map(|a| a.port()).unwrap_or_default();
        let token: Arc<str> = token.into();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (tx, token) = (tx.clone(), Arc::clone(&token));
                std::thread::spawn(move || serve(stream, &token, &tx, &|| {}));
            }
        });
        ControlServer { rx, port, token_file: None }
    }

    #[test]
    fn tokens_are_long_and_random() {
        let (a, b) = (new_token().unwrap_or_default(), new_token().unwrap_or_default());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(
            constant_time_eq(b"abc", b"abc") && !constant_time_eq(b"abc", b"abd") && !constant_time_eq(b"a", b"ab")
        );
    }
}
