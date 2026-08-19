//! UltraTerm Protocol (utp v1): a loopback-only control socket that lets
//! local agents inspect, control, and pass addressed messages between
//! UltraTerm terminal sessions.
//!
//! Transport: JSON Lines over a unix domain socket at
//! `~/.ultraterm/utp.sock` (directory 0700, socket 0600 — same-user only,
//! never TCP). One request object per line, one response object per line.
//!
//! Commands:
//! - `{"cmd":"list"}` — every attached session (id, slot, title, profile).
//! - `{"cmd":"inspect","id"|"slot":…,"lines":N,"raw":bool}` — tail of the
//!   session's PTY output ring buffer, ANSI-scrubbed unless `raw`.
//! - `{"cmd":"send","id"|"slot":…,"text":"…","enter":bool}` — queue input to
//!   the session's PTY (control/debug). `enter` defaults to true.
//! - `{"cmd":"message","to":slot,"from":slot,"text":"…"}` — an explicitly
//!   addressed message shown as a banner on the target terminal only. There
//!   is no broadcast: a message exists only when one terminal asks for it.
//!
//! Agent entry point: `~/bin/utp` (repo: `scripts/utp`), a stdlib-only
//! Python CLI wrapping this socket. Precedent: WezTerm `wezterm cli` mux
//! socket, kitty remote control, Zellij `zellij action`.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter, Manager};

use crate::SessionManager;

const SOCKET_DIR_PERMS: u32 = 0o700;
const SOCKET_FILE_PERMS: u32 = 0o600;
const OUTPUT_TAIL_CAPACITY: usize = 256 * 1024;
const MAX_INSPECT_BYTES: usize = 64 * 1024;
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_MESSAGE_CHARS: usize = 4000;

/// Per-session ring buffer of raw PTY output, capped so a chatty terminal
/// can never grow memory without bound. Fed by the session's reader thread.
pub(crate) struct OutputTail {
    buffer: VecDeque<u8>,
}

impl OutputTail {
    pub(crate) fn new() -> Self {
        Self {
            buffer: VecDeque::with_capacity(OUTPUT_TAIL_CAPACITY.min(8192)),
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= OUTPUT_TAIL_CAPACITY {
            self.buffer.clear();
            self.buffer
                .extend(&bytes[bytes.len() - OUTPUT_TAIL_CAPACITY..]);
            return;
        }
        let overflow = (self.buffer.len() + bytes.len()).saturating_sub(OUTPUT_TAIL_CAPACITY);
        if overflow > 0 {
            self.buffer.drain(..overflow.min(self.buffer.len()));
        }
        self.buffer.extend(bytes);
    }

    pub(crate) fn snapshot(&self, max_bytes: usize) -> Vec<u8> {
        let start = self.buffer.len().saturating_sub(max_bytes);
        self.buffer.iter().skip(start).copied().collect()
    }
}

fn socket_dir() -> Result<PathBuf, String> {
    crate::home_dir()
        .map(|home| home.join(".ultraterm"))
        .map_err(|error| format!("cannot resolve home directory: {error}"))
}

fn socket_path() -> Result<PathBuf, String> {
    socket_dir().map(|dir| dir.join("utp.sock"))
}

/// Spawn the protocol server thread. Non-fatal on failure: the terminal app
/// stays usable without the control socket.
pub(crate) fn spawn(manager: Arc<Mutex<SessionManager>>, app: AppHandle) {
    thread::spawn(move || {
        if let Err(error) = serve(manager, app) {
            eprintln!("[ultraterm] utp server stopped: {error}");
        }
    });
}

fn serve(manager: Arc<Mutex<SessionManager>>, app: AppHandle) -> Result<(), String> {
    let dir = socket_dir()?;
    std::fs::create_dir_all(&dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(SOCKET_DIR_PERMS))
        .map_err(|error| format!("chmod {}: {error}", dir.display()))?;
    let path = socket_path()?;
    // A stale socket file from a crashed app must not block rebinding.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)
        .map_err(|error| format!("bind {}: {error}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(SOCKET_FILE_PERMS))
        .map_err(|error| format!("chmod {}: {error}", path.display()))?;

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let manager = manager.clone();
                let app = app.clone();
                thread::spawn(move || handle_connection(stream, manager, app));
            }
            Err(error) => {
                eprintln!("[ultraterm] utp accept failed: {error}");
            }
        }
    }
    Ok(())
}

fn handle_connection(
    stream: std::os::unix::net::UnixStream,
    manager: Arc<Mutex<SessionManager>>,
    app: AppHandle,
) {
    let mut writer = match stream.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    };
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {
                if line.len() > MAX_REQUEST_BYTES {
                    let _ = writeln!(writer, "{}", json!({"ok": false, "error": "request too large"}));
                    continue;
                }
                let response = match serde_json::from_str::<Request>(line.trim()) {
                    Ok(request) => handle_request(&request, &manager, &app),
                    Err(error) => json!({"ok": false, "error": format!("invalid request: {error}")}),
                };
                if writeln!(writer, "{response}").is_err() {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Request {
    cmd: String,
    id: Option<String>,
    slot: Option<u32>,
    to: Option<u32>,
    from: Option<u32>,
    text: Option<String>,
    lines: Option<usize>,
    raw: Option<bool>,
    enter: Option<bool>,
}

fn handle_request(
    request: &Request,
    manager: &Arc<Mutex<SessionManager>>,
    app: &AppHandle,
) -> Value {
    match request.cmd.as_str() {
        "list" => {
            let sessions = match manager.lock() {
                Ok(manager) => manager.list_sessions(),
                Err(_) => return json!({"ok": false, "error": "session manager lock poisoned"}),
            };
            json!({"ok": true, "sessions": sessions})
        }
        "inspect" => match resolve_and_tail(request, manager) {
            Ok((id, text)) => json!({"ok": true, "id": id, "text": text}),
            Err(error) => json!({"ok": false, "error": error}),
        },
        "send" => {
            let Some(text) = request.text.as_deref() else {
                return json!({"ok": false, "error": "send requires text"});
            };
            let mut bytes = text.as_bytes().to_vec();
            if request.enter.unwrap_or(true) {
                bytes.push(b'\n');
            }
            let mut manager = match manager.lock() {
                Ok(manager) => manager,
                Err(_) => return json!({"ok": false, "error": "session manager lock poisoned"}),
            };
            let Some(id) = manager.resolve_session_id(request.id.as_deref(), request.slot) else {
                return json!({"ok": false, "error": "no such session"});
            };
            match manager.queue_input_bytes(&id, &bytes) {
                Ok(()) => json!({"ok": true, "id": id}),
                Err(error) => json!({"ok": false, "error": error}),
            }
        }
        "message" => {
            let (Some(to_slot), Some(from_slot), Some(text)) =
                (request.to, request.from, request.text.as_deref())
            else {
                return json!({"ok": false, "error": "message requires to, from, and text"});
            };
            if text.chars().count() > MAX_MESSAGE_CHARS {
                return json!({"ok": false, "error": "message too long"});
            }
            let manager = match manager.lock() {
                Ok(manager) => manager,
                Err(_) => return json!({"ok": false, "error": "session manager lock poisoned"}),
            };
            let Some(to_id) = manager.resolve_session_id(None, Some(to_slot)) else {
                return json!({"ok": false, "error": format!("no session on slot {to_slot}")});
            };
            let payload = json!({
                "to": to_id,
                "toSlot": to_slot,
                "fromSlot": from_slot,
                "text": text,
            });
            drop(manager);
            match app.emit("terminal-message", payload) {
                Ok(()) => json!({"ok": true}),
                Err(error) => json!({"ok": false, "error": format!("deliver failed: {error}")}),
            }
        }
        other => json!({"ok": false, "error": format!("unknown cmd {other:?}; expected list, inspect, send, or message")}),
    }
}

fn resolve_and_tail(
    request: &Request,
    manager: &Arc<Mutex<SessionManager>>,
) -> Result<(String, String), String> {
    let manager = manager
        .lock()
        .map_err(|_| "session manager lock poisoned".to_string())?;
    let id = manager
        .resolve_session_id(request.id.as_deref(), request.slot)
        .ok_or_else(|| "no such session".to_string())?;
    let lines = request.lines.unwrap_or(80).min(1000);
    let bytes = manager
        .output_tail(&id, MAX_INSPECT_BYTES)
        .ok_or_else(|| "session output unavailable".to_string())?;
    let text = if request.raw.unwrap_or(false) {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        strip_ansi(&bytes)
    };
    let tail_lines: Vec<&str> = text.lines().collect();
    let start = tail_lines.len().saturating_sub(lines);
    Ok((id, tail_lines[start..].join("\n")))
}

/// Minimal VT scrubber for agent-facing inspection: drops CSI, OSC, and
/// remaining escape sequences, keeps printable text and newlines. Not a
/// screen parser — cursor-movement redraws appear as successive lines.
fn strip_ansi(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte != 0x1b {
            if byte == b'\n' || byte == b'\t' || byte >= 0x20 {
                out.push_str(&String::from_utf8_lossy(&bytes[index..index + 1]));
            }
            index += 1;
            continue;
        }
        match bytes.get(index + 1) {
            Some(b'[') => {
                // CSI: ESC [ params final-byte
                index += 2;
                while index < bytes.len() {
                    let current = bytes[index];
                    index += 1;
                    if (0x40..=0x7e).contains(&current) {
                        break;
                    }
                }
            }
            Some(b']') => {
                // OSC: ESC ] ... terminated by BEL or ST
                index += 2;
                while index < bytes.len() {
                    if bytes[index] == 0x07 {
                        index += 1;
                        break;
                    }
                    if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'\\') {
                        index += 2;
                        break;
                    }
                    index += 1;
                }
            }
            Some(_) => index += 2,
            None => index += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_tail_caps_at_capacity() {
        let mut tail = OutputTail::new();
        tail.push(&vec![b'a'; OUTPUT_TAIL_CAPACITY]);
        tail.push(b"bc");
        let bytes = tail.snapshot(usize::MAX);
        assert_eq!(bytes.len(), OUTPUT_TAIL_CAPACITY);
        assert!(bytes.ends_with(b"bc"));
    }

    #[test]
    fn output_tail_returns_most_recent_bytes() {
        let mut tail = OutputTail::new();
        tail.push(b"hello world");
        assert_eq!(tail.snapshot(5), b"world");
    }

    #[test]
    fn strip_ansi_removes_csi_osc_and_keeps_text() {
        let raw = b"\x1b[1;31mred\x1b[0m plain\x1b]8;;https://x\x07link\x1b]8;;\x07\nnext\x1b[2Kline";
        assert_eq!(strip_ansi(raw), "red plainlink\nnextline");
    }

    #[test]
    fn strip_ansi_tolerates_truncated_sequences() {
        assert_eq!(strip_ansi(b"abc\x1b["), "abc");
        assert_eq!(strip_ansi(b"abc\x1b"), "abc");
    }
}
