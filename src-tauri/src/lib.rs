mod dictator_client;
mod history;
mod maintenance;
mod provider_usage;
mod telemetry;

// UltraTerm Rust backend — single-window Tauri 2 workspace with multiple PTY sessions.
//
// Commands:
//   create_session({ request: { slot, cols, rows, workingDirectory?, launchOmp? } }) -> SessionInfo
//   write_to_session({ id, data }) where data is base64
//   resize_session({ id, cols, rows })
//   detach_session({ id })
//   close_session({ id })
//   scroll_session({ id, lines })
//   detach_all_sessions()
//   close_all_sessions()
//   list_sessions() -> SessionInfo[]
//   list_persistent_slots() -> u32[]
//   system_metrics() -> MemorySnapshot
//   token_telemetry() -> TokenTelemetry
//   search_history({ query, limit? }) -> HistoryEntry[]
//   maintenance_report() -> MaintenanceReport?
//
// Events:
//   terminal-output { id, data } base64
//   terminal-exit { id }

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use maintenance::{MaintenanceManager, MaintenanceReport};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessesToUpdate, Signal, System};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};
use telemetry::{TokenTelemetry, TokenTelemetryManager};
use uuid::Uuid;

const MAX_SESSIONS: usize = 8;
const MAX_COLS: u32 = 512;
const MAX_ROWS: u32 = 512;
const MAX_SESSION_SLOT: u32 = 8;
const DEFAULT_COMMAND_SEARCH_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub slot: u32,
    pub title: String,
    pub pid: u32,
    pub launched_omp: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MemorySnapshot {
    pub app_memory_mib: u64,
    pub terminal_memory_mib: u64,
    pub session_count: usize,
    pub max_sessions: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TerminalOutput {
    id: String,
    data: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TerminalExit {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionRequest {
    slot: u32,
    cols: u32,
    rows: u32,
    working_directory: Option<String>,
    launch_omp: Option<bool>,
}

struct Session {
    id: String,
    slot: u32,
    title: String,
    pid: Option<u32>,
    launched_omp: bool,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    reader_handle: Option<thread::JoinHandle<()>>,
}

struct MetricsSystem(Mutex<System>);

impl Drop for Session {
    fn drop(&mut self) {
        // Best-effort termination of the client child process. The reader thread
        // is expected to notice EOF and exit shortly after.
        let _ = self.child.kill();
    }
}
fn shutdown_session(session: &mut Session) -> Result<(), String> {
    let id = session.id.clone();
    let already_dead = session
        .child
        .try_wait()
        .map_err(|error| format!("Failed to check status for session {id}: {error}"))?
        .is_some();

    if !already_dead {
        session
            .child
            .kill()
            .map_err(|error| format!("Failed to kill session {id}: {error}"))?;
    }

    if let Some(handle) = session.reader_handle.take() {
        handle
            .join()
            .map_err(|_| format!("Reader thread panicked for session {id}"))?;
    }

    Ok(())
}

struct SessionManager {
    sessions: HashMap<String, Session>,
    max_sessions: usize,
    cleanup_tx: mpsc::Sender<String>,
}

impl SessionManager {
    fn new(cleanup_tx: mpsc::Sender<String>) -> Self {
        Self {
            sessions: HashMap::new(),
            max_sessions: MAX_SESSIONS,
            cleanup_tx,
        }
    }

    fn create_session(
        &mut self,
        request: CreateSessionRequest,
        app: AppHandle,
    ) -> Result<SessionInfo, String> {
        if !(1..=MAX_SESSION_SLOT).contains(&request.slot) {
            return Err(format!(
                "Terminal number must be between 1 and {}",
                MAX_SESSION_SLOT
            ));
        }

        if self
            .sessions
            .values()
            .any(|session| session.slot == request.slot)
        {
            return Err(format!("Terminal {} is already connected", request.slot));
        }

        let pty_size = validate_size(request.cols, request.rows)?;

        if self.sessions.len() >= self.max_sessions {
            return Err(format!(
                "Maximum session count ({}) reached; close an existing session first",
                self.max_sessions
            ));
        }

        let working_directory = resolve_working_directory(request.working_directory)?;
        let launch_omp = request.launch_omp.unwrap_or(false);
        let (cmd, title, launched_omp) =
            build_command(request.slot, launch_omp, &working_directory)?;

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(pty_size)
            .map_err(|e| format!("Failed to open PTY: {}", e))?;

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("Failed to spawn command: {}", e))?;
        let pid = child.process_id();

        let writer = match pair.master.take_writer() {
            Ok(w) => w,
            Err(e) => {
                let _ = child.kill();
                return Err(format!("Failed to acquire PTY writer: {}", e));
            }
        };

        let reader = match pair.master.try_clone_reader() {
            Ok(r) => r,
            Err(e) => {
                let _ = child.kill();
                return Err(format!("Failed to acquire PTY reader: {}", e));
            }
        };

        let id = Uuid::new_v4().to_string();
        let session_id_for_thread = id.clone();
        let cleanup_tx = self.cleanup_tx.clone();

        let session = Session {
            id: id.clone(),
            slot: request.slot,
            title,
            pid,
            launched_omp,
            master: pair.master,
            writer,
            child,
            reader_handle: None,
        };
        self.sessions.insert(id.clone(), session);

        let handle = thread::spawn(move || {
            reader_thread(session_id_for_thread, app, cleanup_tx, reader);
        });

        if let Some(session) = self.sessions.get_mut(&id) {
            session.reader_handle = Some(handle);
        }

        self.session_info(&id)
            .ok_or_else(|| format!("Session {} disappeared during creation", id))
    }

    fn session_info(&self, id: &str) -> Option<SessionInfo> {
        self.sessions.get(id).map(|s| SessionInfo {
            id: s.id.clone(),
            slot: s.slot,
            title: s.title.clone(),
            pid: s.pid.unwrap_or(0),
            launched_omp: s.launched_omp,
        })
    }

    fn write_to_session(&mut self, id: &str, data: &str) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(id)
            .ok_or_else(|| format!("Session {} not found", id))?;
        let bytes = STANDARD
            .decode(data)
            .map_err(|e| format!("Invalid base64 input: {}", e))?;
        session
            .writer
            .write_all(&bytes)
            .map_err(|e| format!("Failed to write to session {}: {}", id, e))?;
        session
            .writer
            .flush()
            .map_err(|e| format!("Failed to flush session {}: {}", id, e))?;
        Ok(())
    }

    fn resize_session(&mut self, id: &str, cols: u32, rows: u32) -> Result<(), String> {
        let pty_size = validate_size(cols, rows)?;

        let session = self
            .sessions
            .get_mut(id)
            .ok_or_else(|| format!("Session {} not found", id))?;
        session
            .master
            .resize(pty_size)
            .map_err(|e| format!("Failed to resize session {}: {}", id, e))?;
        Ok(())
    }

    fn finish_session(&mut self, id: &str, kill_persistent: bool) -> Result<(), String> {
        // Reader EOF is reported to the frontend before the cleanup thread can
        // necessarily remove this entry. Treat an already-cleaned client as a
        // successful no-op so reconnect can safely close that race.
        let Some(mut session) = self.sessions.remove(id) else {
            return Ok(());
        };
        let persistent_result = if kill_persistent && session.launched_omp {
            kill_persistent_slot(session.slot)
        } else {
            Ok(())
        };
        let client_result = shutdown_session(&mut session);
        persistent_result.and(client_result)
    }

    fn close_session(&mut self, id: &str) -> Result<(), String> {
        self.finish_session(id, true)
    }

    fn detach_session(&mut self, id: &str) -> Result<(), String> {
        self.finish_session(id, false)
    }

    fn close_all_sessions(&mut self) -> Result<(), String> {
        self.finish_all_sessions(true)
    }

    fn detach_all_sessions(&mut self) -> Result<(), String> {
        self.finish_all_sessions(false)
    }

    fn finish_all_sessions(&mut self, kill_persistent: bool) -> Result<(), String> {
        let ids = self.sessions.keys().cloned().collect::<Vec<_>>();
        let mut failures = Vec::new();
        for id in ids {
            let result = if kill_persistent {
                self.close_session(&id)
            } else {
                self.detach_session(&id)
            };
            if let Err(error) = result {
                failures.push(error);
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Failed to cleanly close {} terminal connection(s): {}",
                failures.len(),
                failures.join("; ")
            ))
        }
    }

    fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .values()
            .map(|s| SessionInfo {
                id: s.id.clone(),
                slot: s.slot,
                title: s.title.clone(),
                pid: s.pid.unwrap_or(0),
                launched_omp: s.launched_omp,
            })
            .collect()
    }
}

fn reader_thread(
    id: String,
    app: AppHandle,
    cleanup_tx: mpsc::Sender<String>,
    mut reader: Box<dyn Read + Send>,
) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let payload = TerminalOutput {
                    id: id.clone(),
                    data: STANDARD.encode(&buf[..n]),
                };
                if let Err(error) = app.emit("terminal-output", payload) {
                    eprintln!("[ultraterm] failed to emit output for {id}: {error}");
                    break;
                }
            }
            Err(error) => {
                eprintln!("[ultraterm] PTY read failed for {id}: {error}");
                break;
            }
        }
    }

    if let Err(error) = app.emit("terminal-exit", TerminalExit { id: id.clone() }) {
        eprintln!("[ultraterm] failed to emit exit for {id}: {error}");
    }
    if let Err(error) = cleanup_tx.send(id.clone()) {
        eprintln!("[ultraterm] failed to queue cleanup for {id}: {error}");
    }
}

fn cleanup_thread(rx: mpsc::Receiver<String>, manager: Arc<Mutex<SessionManager>>) {
    while let Ok(id) = rx.recv() {
        let mut manager = match manager.lock() {
            Ok(manager) => manager,
            Err(error) => {
                eprintln!("[ultraterm] session manager lock poisoned during cleanup: {error}");
                break;
            }
        };
        if let Some(mut session) = manager.sessions.remove(&id) {
            let _ = session.child.kill();
            if let Some(handle) = session.reader_handle.take() {
                let _ = handle.join();
            }
        }
    }
}

fn validate_size(cols: u32, rows: u32) -> Result<PtySize, String> {
    if cols == 0 || rows == 0 || cols > MAX_COLS || rows > MAX_ROWS {
        Err(format!(
            "Terminal size must be between 1x1 and {}x{}",
            MAX_COLS, MAX_ROWS
        ))
    } else {
        Ok(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        })
    }
}

fn nonempty_env_os(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

fn resolve_working_directory(working_directory: Option<String>) -> Result<PathBuf, String> {
    let configured = working_directory
        .map(PathBuf::from)
        .or_else(|| nonempty_env_os("ULTRATERM_WORKING_DIRECTORY").map(PathBuf::from));
    if let Some(path) = configured {
        if path.is_dir() {
            return Ok(path);
        }
        return Err(format!(
            "Working directory is not valid: {}. Set ULTRATERM_WORKING_DIRECTORY to an existing directory.",
            path.display()
        ));
    }

    home_dir()
}

fn home_dir() -> Result<PathBuf, String> {
    nonempty_env_os("HOME")
        .or_else(|| nonempty_env_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| {
            "Could not determine the home directory; set HOME, USERPROFILE, or ULTRATERM_WORKING_DIRECTORY"
                .to_string()
        })
}

fn combine_command_search_path(
    configured: Option<&OsStr>,
    inherited: &OsStr,
) -> Result<OsString, String> {
    let Some(configured) = configured else {
        return Ok(inherited.to_os_string());
    };

    std::env::join_paths(std::env::split_paths(configured).chain(std::env::split_paths(inherited)))
        .map_err(|error| format!("ULTRATERM_PATH could not be combined with PATH: {error}"))
}

fn command_search_path() -> Result<OsString, String> {
    let inherited =
        nonempty_env_os("PATH").unwrap_or_else(|| OsString::from(DEFAULT_COMMAND_SEARCH_PATH));
    let configured = nonempty_env_os("ULTRATERM_PATH");
    combine_command_search_path(configured.as_deref(), &inherited)
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn find_executable(command: &OsStr, search_path: &OsStr) -> Option<PathBuf> {
    let command_path = Path::new(command);
    if command_path.is_absolute() || command_path.components().count() > 1 {
        return is_executable(command_path).then(|| command_path.to_path_buf());
    }

    std::env::split_paths(search_path)
        .map(|directory| directory.join(command_path))
        .find(|candidate| is_executable(candidate))
}

pub(crate) fn resolve_optional_executable(
    environment_variable: &str,
    command: &str,
    fallbacks: &[&Path],
) -> Result<Option<PathBuf>, String> {
    let search_path = command_search_path()?;
    if let Some(configured) = nonempty_env_os(environment_variable) {
        return find_executable(&configured, &search_path)
            .map(Some)
            .ok_or_else(|| {
                format!(
                    "{environment_variable} is set to '{}', but no executable was found. Set it to an executable path or a command available on ULTRATERM_PATH or PATH.",
                    Path::new(&configured).display()
                )
            });
    }

    Ok(
        find_executable(OsStr::new(command), &search_path).or_else(|| {
            fallbacks
                .iter()
                .find(|candidate| is_executable(candidate))
                .map(|candidate| candidate.to_path_buf())
        }),
    )
}

fn persistent_slot_from_session_name(name: &str) -> Option<u32> {
    name.strip_prefix("ultraterm-matrix-")?
        .parse::<u32>()
        .ok()
        .filter(|slot| (1..=MAX_SESSION_SLOT).contains(slot))
}

fn parse_persistent_slots(output: &[u8]) -> Vec<u32> {
    let mut slots: Vec<u32> = String::from_utf8_lossy(output)
        .lines()
        .filter_map(persistent_slot_from_session_name)
        .collect();
    slots.sort_unstable();
    slots.dedup();
    slots
}

fn scroll_persistent_session(session_name: &str, lines: i32) -> Result<(), String> {
    const MAX_SCROLL_LINES: i32 = 24;
    let lines = lines.clamp(-MAX_SCROLL_LINES, MAX_SCROLL_LINES);
    if lines == 0 {
        return Ok(());
    }
    let Some(tmux_path) = telemetry::tmux_binary()? else {
        return Err("tmux is unavailable for terminal scrolling".to_string());
    };
    if lines < 0 {
        let status = process::Command::new(&tmux_path)
            .args(["copy-mode", "-e", "-t", session_name])
            .status()
            .map_err(|error| format!("Failed to enter terminal scrollback: {error}"))?;
        if !status.success() {
            return Err("Failed to enter terminal scrollback".to_string());
        }
    }
    let count = lines.unsigned_abs().to_string();
    let action = if lines < 0 {
        "scroll-up"
    } else {
        "scroll-down"
    };
    let status = process::Command::new(tmux_path)
        .args(["send-keys", "-t", session_name, "-X", "-N", &count, action])
        .status()
        .map_err(|error| format!("Failed to scroll terminal: {error}"))?;
    if status.success() || lines > 0 {
        Ok(())
    } else {
        Err("Failed to scroll terminal".to_string())
    }
}
fn persistent_session_name(slot: u32) -> String {
    format!("ultraterm-matrix-{slot}")
}

#[derive(Clone, Copy)]
struct ProcessIdentity {
    pid: Pid,
    start_time: u64,
}

struct PersistentSessionTarget {
    session_id: String,
    pane_pid: Pid,
}

fn descendant_processes_deepest_first(system: &System, root: Pid) -> Vec<ProcessIdentity> {
    let mut descendants = system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            let mut current = *pid;
            let mut depth = 0usize;
            for _ in 0..64 {
                let parent = system
                    .process(current)
                    .and_then(|candidate| candidate.parent())?;
                depth += 1;
                if parent == root {
                    return Some((
                        ProcessIdentity {
                            pid: *pid,
                            start_time: process.start_time(),
                        },
                        depth,
                    ));
                }
                current = parent;
            }
            None
        })
        .collect::<Vec<_>>();
    descendants.sort_unstable_by(|left, right| right.1.cmp(&left.1));
    descendants
        .into_iter()
        .map(|(identity, _)| identity)
        .collect()
}

fn persistent_session_target(
    tmux_path: &Path,
    session_name: &str,
) -> Option<PersistentSessionTarget> {
    let output = process::Command::new(tmux_path)
        .args([
            "display-message",
            "-p",
            "-t",
            session_name,
            "#{session_id}|#{pane_pid}",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let output = String::from_utf8_lossy(&output.stdout);
    let (session_id, pane_pid) = output.trim().split_once('|')?;
    Some(PersistentSessionTarget {
        session_id: session_id.to_string(),
        pane_pid: Pid::from_u32(pane_pid.parse::<u32>().ok()?),
    })
}

fn signal_if_same_process(system: &System, identity: ProcessIdentity, signal: Signal) {
    if let Some(process) = system
        .process(identity.pid)
        .filter(|process| process.start_time() == identity.start_time)
    {
        let _ = process.kill_with(signal);
    }
}

fn terminate_process_tree(root: Pid) {
    let mut system = System::new_all();
    let Some(root_process) = system.process(root) else {
        return;
    };
    let root_identity = ProcessIdentity {
        pid: root,
        start_time: root_process.start_time(),
    };
    let descendants = descendant_processes_deepest_first(&system, root);
    for identity in descendants
        .iter()
        .copied()
        .chain(std::iter::once(root_identity))
    {
        signal_if_same_process(&system, identity, Signal::Term);
    }

    thread::sleep(std::time::Duration::from_millis(300));
    system.refresh_processes(ProcessesToUpdate::All, true);
    for identity in descendants
        .iter()
        .copied()
        .chain(std::iter::once(root_identity))
    {
        signal_if_same_process(&system, identity, Signal::Kill);
    }
}

fn kill_persistent_slot(slot: u32) -> Result<(), String> {
    let Some(tmux_path) = telemetry::tmux_binary()? else {
        return Ok(());
    };
    let session_name = persistent_session_name(slot);
    let Some(target) = persistent_session_target(&tmux_path, &session_name) else {
        return Ok(());
    };

    // tmux only terminates the pane leader. OMP can own long-lived browser and
    // MCP descendants that otherwise survive the closed terminal and retain
    // gigabytes of memory, so explicitly reap the complete pane process tree.
    terminate_process_tree(target.pane_pid);

    let still_exists = process::Command::new(&tmux_path)
        .args(["has-session", "-t", &target.session_id])
        .status()
        .map_err(|error| format!("Failed to inspect persistent terminal {slot}: {error}"))?;
    if !still_exists.success() {
        return Ok(());
    }

    let status = process::Command::new(tmux_path)
        .args(["kill-session", "-t", &target.session_id])
        .status()
        .map_err(|error| format!("Failed to remove persistent terminal {slot}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Failed to remove persistent terminal {slot}"))
    }
}

#[tauri::command]
fn remove_persistent_slot(slot: u32) -> Result<(), String> {
    if !(1..=MAX_SESSION_SLOT).contains(&slot) {
        return Err(format!(
            "Terminal number must be between 1 and {MAX_SESSION_SLOT}"
        ));
    }
    kill_persistent_slot(slot)
}

#[tauri::command]
fn list_persistent_slots() -> Result<Vec<u32>, String> {
    let Some(tmux_path) = telemetry::tmux_binary()? else {
        return Ok(Vec::new());
    };
    let output = process::Command::new(tmux_path)
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
        .map_err(|error| format!("Failed to inspect persistent terminals: {error}"))?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    Ok(parse_persistent_slots(&output.stdout))
}

fn build_command(
    slot: u32,
    launch_omp: bool,
    working_directory: &Path,
) -> Result<(CommandBuilder, String, bool), String> {
    if launch_omp {
        let session_name = persistent_session_name(slot);
        let path = command_search_path()?;
        let omp_path = resolve_optional_executable(
            "ULTRATERM_OMP_LAUNCHER",
            "omp-safe",
            &[],
        )?
        .ok_or_else(|| {
            "OMP launcher not found. Install scripts/omp-safe on ULTRATERM_PATH or PATH, or set ULTRATERM_OMP_LAUNCHER to its executable path."
                .to_string()
        })?;

        let mut cmd = CommandBuilder::new(&omp_path);
        cmd.env_remove("TMUX");
        cmd.env_remove("TMUX_PANE");
        cmd.env("OMP_TMUX_SESSION", session_name);
        cmd.env("PATH", path);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("LANG", "en_US.UTF-8");
        cmd.env("LC_CTYPE", "en_US.UTF-8");
        if let Some(tmux_path) = telemetry::tmux_binary()? {
            cmd.env("TMUX_BIN", tmux_path);
        }
        cmd.cwd(working_directory);
        Ok((cmd, format!("Terminal {}", slot), true))
    } else {
        let mut cmd = CommandBuilder::new_default_prog();
        cmd.cwd(working_directory);
        let title = default_shell_title();
        Ok((cmd, title, false))
    }
}

fn default_shell_title() -> String {
    std::env::var("SHELL")
        .map(|s| shell_name(&s))
        .unwrap_or_else(|_| "Terminal".to_string())
}

fn shell_name(shell: &str) -> String {
    Path::new(shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Terminal")
        .to_string()
}

#[tauri::command]
fn create_session(
    request: CreateSessionRequest,
    state: State<Arc<Mutex<SessionManager>>>,
    app: AppHandle,
) -> Result<SessionInfo, String> {
    let mut manager = state.lock().map_err(|e| e.to_string())?;
    manager.create_session(request, app)
}

#[tauri::command]
fn write_to_session(
    id: String,
    data: String,
    state: State<Arc<Mutex<SessionManager>>>,
) -> Result<(), String> {
    let mut manager = state.lock().map_err(|e| e.to_string())?;
    manager.write_to_session(&id, &data)
}

#[tauri::command]
fn resize_session(
    id: String,
    cols: u32,
    rows: u32,
    state: State<Arc<Mutex<SessionManager>>>,
) -> Result<(), String> {
    let mut manager = state.lock().map_err(|e| e.to_string())?;
    manager.resize_session(&id, cols, rows)
}

#[tauri::command]
fn scroll_session(
    id: String,
    lines: i32,
    state: State<Arc<Mutex<SessionManager>>>,
) -> Result<(), String> {
    let session_name = {
        let manager = state.lock().map_err(|error| error.to_string())?;
        let session = manager
            .sessions
            .get(&id)
            .ok_or_else(|| format!("Session {id} not found"))?;
        if !session.launched_omp {
            return Ok(());
        }
        format!("ultraterm-matrix-{}", session.slot)
    };
    scroll_persistent_session(&session_name, lines)
}

#[tauri::command]
fn detach_session(id: String, state: State<Arc<Mutex<SessionManager>>>) -> Result<(), String> {
    let mut manager = state.lock().map_err(|e| e.to_string())?;
    manager.detach_session(&id)
}

#[tauri::command]
fn close_session(id: String, state: State<Arc<Mutex<SessionManager>>>) -> Result<(), String> {
    let mut manager = state.lock().map_err(|e| e.to_string())?;
    manager.close_session(&id)
}

#[tauri::command]
fn detach_all_sessions(state: State<Arc<Mutex<SessionManager>>>) -> Result<(), String> {
    let mut manager = state.lock().map_err(|e| e.to_string())?;
    manager.detach_all_sessions()
}

#[tauri::command]
fn close_all_sessions(state: State<Arc<Mutex<SessionManager>>>) -> Result<(), String> {
    let mut manager = state.lock().map_err(|e| e.to_string())?;
    manager.close_all_sessions()
}

#[tauri::command]
fn list_sessions(state: State<Arc<Mutex<SessionManager>>>) -> Result<Vec<SessionInfo>, String> {
    let manager = state.lock().map_err(|e| e.to_string())?;
    Ok(manager.list_sessions())
}

fn managed_tmux_pane_pids(slots: &HashSet<u32>) -> HashSet<Pid> {
    if slots.is_empty() {
        return HashSet::new();
    }

    let tmux_path = match telemetry::tmux_binary() {
        Ok(Some(path)) => path,
        Ok(None) => return HashSet::new(),
        Err(error) => {
            eprintln!("[ultraterm] tmux configuration error: {error}");
            return HashSet::new();
        }
    };
    let output = match process::Command::new(tmux_path)
        .args(["list-panes", "-a", "-F", "#{session_name}|#{pane_pid}"])
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            eprintln!(
                "[ultraterm] tmux memory query exited with status {}",
                output.status
            );
            return HashSet::new();
        }
        Err(error) => {
            eprintln!("[ultraterm] tmux memory query failed: {error}");
            return HashSet::new();
        }
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (session, pid) = line.split_once('|')?;
            let slot = session
                .strip_prefix("ultraterm-matrix-")?
                .parse::<u32>()
                .ok()?;
            if !slots.contains(&slot) {
                return None;
            }
            pid.parse::<u32>().ok().map(Pid::from_u32)
        })
        .collect()
}

fn process_tree_memory_bytes(system: &System, roots: &HashSet<Pid>) -> u64 {
    system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            let mut ancestor = Some(*pid);
            for _ in 0..64 {
                let current = ancestor?;
                if roots.contains(&current) {
                    return Some(process.memory());
                }
                ancestor = system
                    .process(current)
                    .and_then(|candidate| candidate.parent());
            }
            None
        })
        .sum()
}

#[tauri::command]
fn system_metrics(
    state: State<Arc<Mutex<SessionManager>>>,
    metrics_system: State<MetricsSystem>,
) -> Result<MemorySnapshot, String> {
    let manager = state.lock().map_err(|e| e.to_string())?;
    let direct_pids: HashSet<Pid> = manager
        .sessions
        .values()
        .filter_map(|session| session.pid.map(Pid::from_u32))
        .collect();
    let matrix_slots: HashSet<u32> = manager
        .sessions
        .values()
        .filter(|session| session.launched_omp)
        .map(|session| session.slot)
        .collect();
    let session_count = manager.sessions.len();
    let max_sessions = manager.max_sessions;
    drop(manager);

    let mut sys = metrics_system.0.lock().map_err(|e| e.to_string())?;
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let app_pid = Pid::from_u32(process::id());
    let app_memory_mib = sys
        .process(app_pid)
        .map(|p| p.memory() / 1024 / 1024)
        .unwrap_or(0);

    let mut terminal_roots = direct_pids;
    terminal_roots.extend(managed_tmux_pane_pids(&matrix_slots));
    let terminal_memory_mib = process_tree_memory_bytes(&sys, &terminal_roots) / 1024 / 1024;

    Ok(MemorySnapshot {
        app_memory_mib,
        terminal_memory_mib,
        session_count,
        max_sessions,
    })
}

#[tauri::command]
fn token_telemetry(
    state: State<Arc<Mutex<TokenTelemetryManager>>>,
) -> Result<TokenTelemetry, String> {
    let home = home_dir()?;
    let mut telemetry = state.lock().map_err(|error| error.to_string())?;
    telemetry.snapshot(&home)
}

#[tauri::command]
async fn search_history(
    query: String,
    limit: Option<u32>,
) -> Result<Vec<history::HistoryEntry>, String> {
    let home = home_dir()?;
    tauri::async_runtime::spawn_blocking(move || history::query_history(&home, &query, limit))
        .await
        .map_err(|error| format!("OMP history worker failed: {error}"))?
}

#[tauri::command]
fn maintenance_report(
    state: State<Arc<MaintenanceManager>>,
) -> Result<Option<MaintenanceReport>, String> {
    state.snapshot()
}

#[tauri::command]
async fn voice_health() -> Result<dictator_client::VoiceServiceResponse, String> {
    dictator_client::health().await
}

#[tauri::command]
async fn start_voice_input() -> Result<dictator_client::VoiceServiceResponse, String> {
    dictator_client::start_recording().await
}

#[tauri::command]
async fn finish_voice_input(
    recording_id: String,
) -> Result<dictator_client::VoiceServiceResponse, String> {
    dictator_client::stop_recording(&recording_id).await
}

#[tauri::command]
async fn voice_input_status(
    recording_id: String,
) -> Result<dictator_client::VoiceServiceResponse, String> {
    dictator_client::recording_status(&recording_id).await
}

#[tauri::command]
async fn cancel_voice_input(
    recording_id: String,
) -> Result<dictator_client::VoiceServiceResponse, String> {
    dictator_client::cancel_recording(&recording_id).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let (cleanup_tx, cleanup_rx) = mpsc::channel::<String>();
            let manager = Arc::new(Mutex::new(SessionManager::new(cleanup_tx)));
            let telemetry = Arc::new(Mutex::new(TokenTelemetryManager::default()));
            let manager_for_cleanup = manager.clone();
            let home = home_dir().map_err(std::io::Error::other)?;
            let maintenance = Arc::new(MaintenanceManager::new(home));

            thread::spawn(move || cleanup_thread(cleanup_rx, manager_for_cleanup));
            maintenance::spawn_scheduler(maintenance.clone());

            app.manage(manager);
            app.manage(telemetry);
            app.manage(maintenance);
            app.manage(MetricsSystem(Mutex::new(System::new())));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            create_session,
            write_to_session,
            resize_session,
            scroll_session,
            detach_session,
            close_session,
            detach_all_sessions,
            close_all_sessions,
            list_sessions,
            list_persistent_slots,
            remove_persistent_slot,
            system_metrics,
            token_telemetry,
            search_history,
            maintenance_report,
            provider_usage::provider_usage,
            provider_usage::save_provider_credential,
            provider_usage::remove_provider_credential,
            voice_health,
            start_voice_input,
            finish_voice_input,
            voice_input_status,
            cancel_voice_input,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let RunEvent::Exit = event {
                if let Err(error) =
                    tauri::async_runtime::block_on(dictator_client::cancel_active_recording())
                {
                    eprintln!("[ultraterm] voice cleanup failed during exit: {error}");
                }
                let state = app_handle.state::<Arc<Mutex<SessionManager>>>();
                match state.lock() {
                    Ok(mut manager) => {
                        if let Err(error) = manager.detach_all_sessions() {
                            eprintln!("[ultraterm] session cleanup failed during exit: {error}");
                        }
                    }
                    Err(error) => {
                        eprintln!("[ultraterm] session manager lock poisoned during exit: {error}");
                    }
                };
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_terminal_size() {
        assert!(validate_size(80, 24).is_ok());
        assert!(validate_size(1, 1).is_ok());
        assert!(validate_size(MAX_COLS, MAX_ROWS).is_ok());
    }

    #[test]
    fn rejects_invalid_terminal_size() {
        assert!(validate_size(0, 24).is_err());
        assert!(validate_size(80, 0).is_err());
        assert!(validate_size(MAX_COLS + 1, 24).is_err());
        assert!(validate_size(80, MAX_ROWS + 1).is_err());
    }

    #[test]
    fn extracts_shell_name() {
        assert_eq!(shell_name("/bin/zsh"), "zsh");
        assert_eq!(shell_name("/usr/local/bin/fish"), "fish");
        assert_eq!(shell_name("bash"), "bash");
        assert_eq!(shell_name(""), "Terminal");
    }

    #[test]
    fn parses_persistent_terminal_slots() {
        let output = b"ultraterm-matrix-5\nother\nultraterm-matrix-2\nultraterm-matrix-5\nultraterm-matrix-0\nultraterm-matrix-9\n";
        assert_eq!(parse_persistent_slots(output), vec![2, 5]);
    }

    #[test]
    fn reconnect_teardown_is_idempotent_after_async_cleanup() {
        let (cleanup_tx, _cleanup_rx) = mpsc::channel();
        let mut manager = SessionManager::new(cleanup_tx);

        assert!(manager.detach_session("already-cleaned").is_ok());
        assert!(manager.close_session("already-cleaned").is_ok());
    }

    #[test]
    fn default_shell_title_uses_shell_name() {
        // We can only observe the helper behavior; the env var may or may not be set.
        let title = default_shell_title();
        assert!(!title.is_empty());
    }

    #[test]
    fn explicit_working_directory_has_no_home_layout_dependency() {
        let directory = std::env::temp_dir().join(format!("ultraterm-cwd-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let resolved =
            resolve_working_directory(Some(directory.to_string_lossy().into_owned())).unwrap();
        assert_eq!(resolved, directory);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ultraterm_path_precedes_and_preserves_inherited_path() {
        let inherited = OsStr::new("/usr/bin:/bin");
        let combined =
            combine_command_search_path(Some(OsStr::new("/custom/bin")), inherited).unwrap();
        assert_eq!(
            std::env::split_paths(&combined).collect::<Vec<_>>(),
            vec![
                PathBuf::from("/custom/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin")
            ]
        );
        assert_eq!(
            combine_command_search_path(None, inherited).unwrap(),
            inherited
        );
    }

    #[test]
    #[cfg(unix)]
    fn process_tree_shutdown_reaps_descendants() {
        let mut root = process::Command::new("/bin/sh")
            .args(["-c", "sleep 30 & wait"])
            .spawn()
            .unwrap();
        let root_pid = Pid::from_u32(root.id());
        let mut system = System::new_all();

        let descendants = (0..20).find_map(|_| {
            system.refresh_processes(ProcessesToUpdate::All, true);
            let descendants = descendant_processes_deepest_first(&system, root_pid);
            if descendants.is_empty() {
                thread::sleep(std::time::Duration::from_millis(25));
                None
            } else {
                Some(descendants)
            }
        });

        terminate_process_tree(root_pid);
        let _ = root.wait();
        system.refresh_processes(ProcessesToUpdate::All, true);
        let descendants = descendants.expect("shell child was not observed");
        assert!(system.process(root_pid).is_none());
        assert!(descendants
            .into_iter()
            .all(|identity| system.process(identity.pid).is_none()));
    }
}
