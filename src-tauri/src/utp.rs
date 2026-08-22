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
//!   the session's PTY (control/debug). `enter` defaults to true and sends
//!   carriage return, matching what TUIs expect for Enter.
//! - `{"cmd":"message","to":slot,"from":slot,"text":"…"}` — an explicitly
//!   addressed message shown as a banner on the target terminal only. There
//!   is no broadcast: a message exists only when one terminal asks for it.
//!
//! Agent entry point: `~/.ultraterm/bin/utp` (bundled from `scripts/utp`),
//! a stdlib-only Python CLI wrapping this socket. Precedent: WezTerm
//! socket, kitty remote control, Zellij `zellij action`.

use crate::omp_profiles::{self, CreateOmpProfileRequest};
use crate::SessionManager;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
#[cfg(test)]
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter};

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
#[serde(rename_all = "camelCase")]
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
    name: Option<String>,
    model: Option<String>,
    thinking_level: Option<String>,
    title_model: Option<String>,
    #[serde(default)]
    request: Option<CreateOmpProfileRequest>,
}

fn profile_create_request(request: &Request) -> Result<CreateOmpProfileRequest, String> {
    if let Some(request) = request.request.clone() {
        return Ok(request);
    }
    Ok(CreateOmpProfileRequest {
        name: request
            .name
            .clone()
            .ok_or_else(|| "profiles.create requires name".to_string())?,
        model: request
            .model
            .clone()
            .ok_or_else(|| "profiles.create requires model".to_string())?,
        thinking_level: request
            .thinking_level
            .clone()
            .ok_or_else(|| "profiles.create requires thinkingLevel".to_string())?,
        title_model: request.title_model.clone(),
    })
}

fn profile_response(request: &Request) -> Value {
    match request.cmd.as_str() {
        "profiles.list" => match omp_profiles::list() {
            Ok(profiles) => json!({"ok": true, "profiles": profiles}),
            Err(error) => json!({"ok": false, "error": error}),
        },
        "profiles.create" => match profile_create_request(request)
            .and_then(|request| omp_profiles::create(&request))
        {
            Ok(profile) => json!({"ok": true, "profile": profile}),
            Err(error) => json!({"ok": false, "error": error}),
        },
        "profiles.remove" => {
            let Some(name) = request.name.as_deref() else {
                return json!({"ok": false, "error": "profiles.remove requires name"});
            };
            match omp_profiles::remove(name) {
                Ok(()) => json!({"ok": true, "name": name}),
                Err(error) => json!({"ok": false, "error": error}),
            }
        }
        _ => json!({"ok": false, "error": "unknown profile command"}),
    }
}
#[cfg(test)]
fn profile_response_at(request: &Request, root: &Path, active: &HashSet<String>) -> Value {
    match request.cmd.as_str() {
        "profiles.list" => match omp_profiles::list_at(root, active) {
            Ok(profiles) => json!({"ok": true, "profiles": profiles}),
            Err(error) => json!({"ok": false, "error": error}),
        },
        "profiles.create" => match profile_create_request(request)
            .and_then(|request| omp_profiles::create_at(root, &request, active))
        {
            Ok(profile) => json!({"ok": true, "profile": profile}),
            Err(error) => json!({"ok": false, "error": error}),
        },
        "profiles.remove" => {
            let Some(name) = request.name.as_deref() else {
                return json!({"ok": false, "error": "profiles.remove requires name"});
            };
            match omp_profiles::remove_at(root, name, active) {
                Ok(()) => json!({"ok": true, "name": name}),
                Err(error) => json!({"ok": false, "error": error}),
            }
        }
        _ => json!({"ok": false, "error": "unknown profile command"}),
    }
}

fn handle_request(
    request: &Request,
    manager: &Arc<Mutex<SessionManager>>,
    app: &AppHandle,
) -> Value {
    match request.cmd.as_str() {
        "profiles.list" | "profiles.create" | "profiles.remove" => {
            let response = profile_response(request);
            if request.cmd != "profiles.list" && response["ok"] == true {
                let _ = app.emit("omp-profiles-changed", ());
            }
            return response;
        }
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
                // TUIs read Enter as carriage return in raw mode; LF only
                // lands in the input box without submitting.
                bytes.push(b'\r');
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
/// Operates on the lossy-decoded text so multibyte UTF-8 survives.
fn strip_ansi(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte != 0x1b {
            if byte == b'\n' || byte == b'\t' || byte >= 0x20 {
                // ASCII boundary: escape bytes are ASCII, so this index is a
                // char boundary of the lossy-decoded string.
                let mut end = index + 1;
                while end < bytes.len() && bytes[end] >= 0x80 && bytes[end] < 0xc0 {
                    end += 1;
                }
                out.push_str(&text[index..end]);
                index = end;
                continue;
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

    #[test]
    fn strip_ansi_preserves_multibyte_utf8() {
        let raw = "│ 任务 ✓ │\x1b[32m✓\x1b[0m — 参数".as_bytes();
        assert_eq!(strip_ansi(raw), "│ 任务 ✓ │✓ — 参数");
    }
    #[test]
    fn profile_requests_parse_and_dispatch_through_safe_seam() {
        let request: Request = serde_json::from_value(json!({
            "cmd": "profiles.create",
            "name": "team",
            "model": "vendor/model",
            "thinkingLevel": "medium",
            "titleModel": "vendor/title"
        }))
        .unwrap();
        let parsed = profile_create_request(&request).unwrap();
        assert_eq!(parsed.name, "team");
        assert_eq!(parsed.thinking_level, "medium");

        let root = tempfile::tempdir_in("/private/tmp").unwrap();
        let active = HashSet::new();
        let created = profile_response_at(&request, root.path(), &active);
        assert_eq!(created["ok"], true);
        let listed: Request = serde_json::from_value(json!({"cmd": "profiles.list"})).unwrap();
        let response = profile_response_at(&listed, root.path(), &active);
        assert_eq!(response["profiles"][0]["name"], "team");

        let removal: Request = serde_json::from_value(json!({
            "cmd": "profiles.remove",
            "name": "team"
        }))
        .unwrap();
        assert_eq!(profile_response_at(&removal, root.path(), &active)["ok"], true);
        assert!(root.path().join("team").exists() == false);
    }

    #[test]
    fn profile_dispatch_returns_error_envelope_for_invalid_request() {
        let request: Request = serde_json::from_value(json!({
            "cmd": "profiles.create",
            "name": "../escape",
            "model": "vendor/model",
            "thinkingLevel": "medium"
        }))
        .unwrap();
        let response = profile_response_at(&request, Path::new("/tmp"), &HashSet::new());
        assert_eq!(response["ok"], false);
        assert!(response["error"].is_string());
    }
}
