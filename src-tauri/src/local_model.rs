//! Local model lifecycle for the `local` launch profile: the OverSeer
//! Qwen3.8-27B MTPLX model served by the `mtplx` daemon on port 8000.
//!
//! Creating a local-profile terminal dispatches a background daemon start so
//! the model is serving by the first prompt; closing or detaching the *last*
//! local-profile terminal stops the daemon so ~14 GB does not sit resident on
//! a 24 GB machine.
//!
//! The daemon runs the verified `omp-qwen` serving contract (canonical:
//! `~/bin/omp-qwen`; benchmarked 2026-08-18, 5/5 strict coding at 13.6 s
//! mean): `performance-cold`, depth 2, 0.7/0.8/20 samplers for both target
//! and draft, 6,144-token context, 2,048-token responses, Q8 paged KV, SSD
//! session cache off, one 64 MiB session-bank entry, postcommit rewrites and
//! admission waits disabled. The previous `quickstart --profile sustained`
//! (depth 3, 32K context, 1 GiB bank, preserved thinking, KV off) drove this
//! 24 GB machine into memory-pressure guards and 30-second zero-token
//! streams — the TUI looked stuck on "working".
//!
//! All requests funnel through a single worker thread fed by an mpsc channel.
//! Sequential processing guarantees a rapid close→reopen produces Stop then
//! Start in order — never a stale stop killing a freshly started daemon.
//! The worker keeps the spawned child handle so the daemon is reaped on stop
//! instead of leaking a zombie, and stop only ever targets a daemon this app
//! spawned: a backend owned by `omp-qwen` or a manual `mtplx serve` on the
//! same port is left running.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::LazyLock;
use std::thread;
use std::time::{Duration, Instant};

const SERVER_PORT: u16 = 8000;
const SERVER_ADDR: &str = "127.0.0.1:8000";
const MODEL_ID: &str = "overseer-qwen3.8-27b-mtplx";
const API_KEY: &str = "mtplx-local";
const START_TIMEOUT: Duration = Duration::from_secs(180);
const STOP_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelRequest {
    Start,
    Stop,
}

static WORKER: LazyLock<Sender<ModelRequest>> = LazyLock::new(|| {
    let (tx, rx) = mpsc::channel::<ModelRequest>();
    thread::spawn(move || {
        let mut daemon: Option<Child> = None;
        while let Ok(request) = rx.recv() {
            match request {
                ModelRequest::Start => start(&mut daemon),
                ModelRequest::Stop => stop(&mut daemon),
            }
        }
    });
    tx
});

/// Queue a background daemon start. Non-blocking: model load takes tens of
/// seconds and must never stall session creation.
pub(crate) fn ensure_loaded() {
    if WORKER.send(ModelRequest::Start).is_err() {
        eprintln!("[ultraterm] local model worker is gone; start request dropped");
    }
}

/// Queue a background daemon stop. Non-blocking.
pub(crate) fn unload() {
    if WORKER.send(ModelRequest::Stop).is_err() {
        eprintln!("[ultraterm] local model worker is gone; stop request dropped");
    }
}

fn default_model_path() -> PathBuf {
    crate::home_dir()
        .unwrap_or_else(|_| PathBuf::from("/"))
        .join(".mtplx/models/OverSeer-Qwen3.8-27B-MTPLX")
}

/// Model path override: `ULTRATERM_LOCAL_MODEL`.
fn model_path() -> PathBuf {
    std::env::var("ULTRATERM_LOCAL_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_model_path)
}

/// Verified serving contract; keep in sync with `~/bin/omp-qwen` — that
/// launcher's `backend_pid_contract` is the source of truth these flags and
/// the environment below reproduce.
fn start_arguments(model: &std::path::Path) -> Vec<String> {
    vec![
        "serve".to_string(),
        "--model".to_string(),
        model.to_string_lossy().into_owned(),
        "--profile".to_string(),
        "performance-cold".to_string(),
        "--depth".to_string(),
        "2".to_string(),
        "--port".to_string(),
        SERVER_PORT.to_string(),
        "--yes".to_string(),
        "--scheduler-mode".to_string(),
        "serial".to_string(),
        "--max-active-requests".to_string(),
        "1".to_string(),
        "--batching-preset".to_string(),
        "solo".to_string(),
        "--ssd-session-cache".to_string(),
        "off".to_string(),
        "--paged-kv-quantization".to_string(),
        "q8".to_string(),
        "--default-temperature".to_string(),
        "0.7".to_string(),
        "--default-top-p".to_string(),
        "0.8".to_string(),
        "--default-top-k".to_string(),
        "20".to_string(),
        "--draft-temperature".to_string(),
        "0.7".to_string(),
        "--draft-top-p".to_string(),
        "0.8".to_string(),
        "--draft-top-k".to_string(),
        "20".to_string(),
        "--reasoning".to_string(),
        "auto".to_string(),
        "--reasoning-effort".to_string(),
        "low".to_string(),
        "--preserve-thinking".to_string(),
        "off".to_string(),
        "--tool-prompt-mode".to_string(),
        "native".to_string(),
        "--chat-template-profile".to_string(),
        "local_qwen36".to_string(),
        "--context-window".to_string(),
        "6144".to_string(),
        "--max-tokens".to_string(),
        "2048".to_string(),
        "--stream-interval".to_string(),
        "1".to_string(),
        "--warmup-tokens".to_string(),
        "8".to_string(),
        "--api-key".to_string(),
        API_KEY.to_string(),
    ]
}

/// Memory and scheduler guards from the verified contract: 18 GiB budget,
/// one 64 MiB session-bank entry, no extended warmup, no idle tool-history
/// rewrites, no postcommit admission waits.
fn start_environment() -> Vec<(&'static str, &'static str)> {
    vec![
        ("MTPLX_MEMORY_BUDGET", "19327352832"),
        ("MTPLX_SESSION_BANK_MAX_ENTRIES", "1"),
        ("MTPLX_SESSION_BANK_MAX_BYTES", "64M"),
        ("MTPLX_SESSION_BANK_PER_SESSION_BYTES", "64M"),
        ("MTPLX_SESSION_BLOCK_PREFIX_RESTORE", "0"),
        ("MTPLX_WARMUP_EXTENDED", "0"),
        ("MTPLX_IDLE_POSTCOMMIT_TOOL_REWRITE", "0"),
        ("MTPLX_POSTCOMMIT_ARRIVAL_WAIT_S", "0"),
        ("MTPLX_POSTCOMMIT_WAIT_TIMEOUT_S", "0"),
    ]
}

fn stop_arguments() -> Vec<String> {
    vec![
        "stop".to_string(),
        "--port".to_string(),
        SERVER_PORT.to_string(),
    ]
}

/// `mtplx` ships as a project venv binary, not a PATH install.
fn mtplx_binary() -> Option<PathBuf> {
    let fallback = crate::home_dir()
        .map(|home| home.join("dev/qwen-mtp-test/.venv/bin/mtplx"))
        .ok();
    let fallback_refs: Vec<&std::path::Path> = fallback.iter().map(PathBuf::as_path).collect();
    crate::resolve_optional_executable("ULTRATERM_MTPLX_BIN", "mtplx", &fallback_refs)
        .ok()
        .flatten()
}

/// Ready means the verified model answers the API, not just an open port: a
/// stale or foreign server on 8000 must not suppress a correct start.
fn server_ready() -> bool {
    let addr: SocketAddr = match SERVER_ADDR.parse() {
        Ok(addr) => addr,
        Err(_) => return false,
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    let request = format!(
        "GET /v1/models HTTP/1.1\r\nHost: {SERVER_ADDR}\r\nAuthorization: Bearer {API_KEY}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                response.extend_from_slice(&chunk[..n]);
                if response.len() > 16 * 1024 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    response_lists_model(&response)
}

fn response_lists_model(response: &[u8]) -> bool {
    response
        .windows(MODEL_ID.len())
        .any(|window| window == MODEL_ID.as_bytes())
}

/// Daemon stderr goes to a log file: quickstart runs in the foreground for the
/// daemon's whole lifetime, and a captured pipe would fill and stall it.
fn daemon_log() -> Stdio {
    let log_path = crate::home_dir()
        .map(|home| home.join(".mtplx/logs/ultraterm-daemon.log"))
        .ok();
    log_path
        .and_then(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
        })
        .map(Stdio::from)
        .unwrap_or_else(Stdio::null)
}

fn start(daemon: &mut Option<Child>) {
    // Reap a previous daemon that already exited on its own.
    if let Some(child) = daemon.as_mut() {
        match child.try_wait() {
            Ok(Some(_)) => *daemon = None,
            Ok(None) => {}
            Err(_) => *daemon = None,
        }
    }
    if server_ready() {
        return;
    }
    let model = model_path();
    let Some(mtplx) = mtplx_binary() else {
        eprintln!("[ultraterm] mtplx binary not found; cannot start the local model server");
        return;
    };
    let spawned = Command::new(&mtplx)
        .args(start_arguments(&model))
        .envs(start_environment())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(daemon_log())
        .spawn();
    match spawned {
        Ok(child) => {
            *daemon = Some(child);
        }
        Err(error) => {
            eprintln!("[ultraterm] failed to spawn mtplx daemon: {error}");
            return;
        }
    }
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if server_ready() {
            eprintln!("[ultraterm] local model server is ready on port {SERVER_PORT}");
            return;
        }
        // Fail fast when the daemon dies during startup (e.g. model missing).
        if let Some(child) = daemon.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    eprintln!("[ultraterm] mtplx daemon exited during startup with {status}");
                    *daemon = None;
                    return;
                }
                Ok(None) => {}
                Err(_) => {
                    *daemon = None;
                    return;
                }
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
    eprintln!("[ultraterm] local model server did not become ready within {START_TIMEOUT:?}");
}

fn stop(daemon: &mut Option<Child>) {
    // Only stop a daemon this app spawned and that is still alive:
    // `mtplx stop --port` kills whatever serves the port, including a
    // backend owned by `omp-qwen` or a manual `mtplx serve` — closing an
    // UltraTerm terminal must not tear down someone else's model server.
    let child_running = daemon
        .as_mut()
        .map(|child| matches!(child.try_wait(), Ok(None)))
        .unwrap_or(false);
    if child_running && server_ready() {
        if let Some(mtplx) = mtplx_binary() {
            let result = Command::new(&mtplx)
                .args(stop_arguments())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if let Err(error) = result {
                eprintln!("[ultraterm] failed to run mtplx stop: {error}");
            }
        }
        let deadline = Instant::now() + STOP_TIMEOUT;
        while server_ready() && Instant::now() < deadline {
            thread::sleep(POLL_INTERVAL);
        }
        if server_ready() {
            eprintln!("[ultraterm] local model server did not stop within {STOP_TIMEOUT:?}");
        } else {
            eprintln!("[ultraterm] local model server stopped");
        }
    }
    // Reap the child whether or not the stop above ran.
    if let Some(mut child) = daemon.take() {
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_arguments_pin_the_verified_serving_contract() {
        let args = start_arguments(std::path::Path::new("/models/OverSeer-Qwen3.8-27B-MTPLX"));
        let expected: Vec<String> = [
            "serve",
            "--model",
            "/models/OverSeer-Qwen3.8-27B-MTPLX",
            "--profile",
            "performance-cold",
            "--depth",
            "2",
            "--port",
            "8000",
            "--yes",
            "--scheduler-mode",
            "serial",
            "--max-active-requests",
            "1",
            "--batching-preset",
            "solo",
            "--ssd-session-cache",
            "off",
            "--paged-kv-quantization",
            "q8",
            "--default-temperature",
            "0.7",
            "--default-top-p",
            "0.8",
            "--default-top-k",
            "20",
            "--draft-temperature",
            "0.7",
            "--draft-top-p",
            "0.8",
            "--draft-top-k",
            "20",
            "--reasoning",
            "auto",
            "--reasoning-effort",
            "low",
            "--preserve-thinking",
            "off",
            "--tool-prompt-mode",
            "native",
            "--chat-template-profile",
            "local_qwen36",
            "--context-window",
            "6144",
            "--max-tokens",
            "2048",
            "--stream-interval",
            "1",
            "--warmup-tokens",
            "8",
            "--api-key",
            "mtplx-local",
        ]
        .iter()
        .map(|value| value.to_string())
        .collect();
        assert_eq!(args, expected);
    }

    #[test]
    fn start_environment_pins_memory_and_scheduler_guards() {
        let env = start_environment();
        assert!(env.contains(&("MTPLX_MEMORY_BUDGET", "19327352832")));
        assert!(env.contains(&("MTPLX_SESSION_BANK_MAX_ENTRIES", "1")));
        assert!(env.contains(&("MTPLX_SESSION_BANK_MAX_BYTES", "64M")));
        assert!(env.contains(&("MTPLX_SESSION_BANK_PER_SESSION_BYTES", "64M")));
        assert!(env.contains(&("MTPLX_WARMUP_EXTENDED", "0")));
        assert!(env.contains(&("MTPLX_IDLE_POSTCOMMIT_TOOL_REWRITE", "0")));
        assert!(env.contains(&("MTPLX_POSTCOMMIT_ARRIVAL_WAIT_S", "0")));
        assert!(env.contains(&("MTPLX_POSTCOMMIT_WAIT_TIMEOUT_S", "0")));
    }

    #[test]
    fn readiness_requires_the_verified_model_in_the_models_response() {
        let ok = b"HTTP/1.1 200 OK\r\n\r\n{\"data\":[{\"id\":\"overseer-qwen3.8-27b-mtplx\"}]}";
        assert!(response_lists_model(ok));
        let wrong_model = b"HTTP/1.1 200 OK\r\n\r\n{\"data\":[{\"id\":\"other-model\"}]}";
        assert!(!response_lists_model(wrong_model));
        assert!(!response_lists_model(b"HTTP/1.1 401 Unauthorized\r\n\r\n"));
        assert!(!response_lists_model(b""));
    }

    #[test]
    fn stop_arguments_target_the_server_port() {
        assert_eq!(stop_arguments(), vec!["stop", "--port", "8000"]);
    }

    /// Exercises the real mtplx daemon. Ignored by default; run with
    /// `cargo test --lib local_model -- --ignored`.
    #[test]
    #[ignore]
    fn start_then_stop_roundtrip_against_mtplx() {
        if mtplx_binary().is_none() {
            eprintln!("mtplx not installed; skipping");
            return;
        }
        ensure_loaded();
        let deadline = Instant::now() + START_TIMEOUT;
        while Instant::now() < deadline && !server_ready() {
            thread::sleep(POLL_INTERVAL);
        }
        assert!(server_ready(), "server did not start within {START_TIMEOUT:?}");

        unload();
        let deadline = Instant::now() + STOP_TIMEOUT + Duration::from_secs(10);
        while Instant::now() < deadline && server_ready() {
            thread::sleep(POLL_INTERVAL);
        }
        assert!(!server_ready(), "server did not stop within {STOP_TIMEOUT:?}");
    }
}
