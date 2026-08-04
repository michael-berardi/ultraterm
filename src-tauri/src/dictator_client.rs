use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::{sleep, timeout, Instant};
use uuid::Uuid;

const PROTOCOL_VERSION: u8 = 1;
const MAX_FRAME_BYTES: usize = 64 * 1024;
const SOCKET_ENV: &str = "DICTATOR_VOICE_SOCKET";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const SERVICE_READY_TIMEOUT: Duration = Duration::from_secs(10);
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(10);
static ACTIVE_RECORDING_ID: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceServiceResponse {
    pub version: u8,
    pub request_id: String,
    pub ok: bool,
    pub state: String,
    pub recording_id: Option<String>,
    pub transcript: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub audio_level: Option<f32>,
    #[serde(default)]
    pub service_started: bool,
}

fn active_recording_id() -> &'static Mutex<Option<String>> {
    &ACTIVE_RECORDING_ID
}

fn remember_recording(recording_id: &str) -> Result<(), String> {
    *active_recording_id()
        .lock()
        .map_err(|error| error.to_string())? = Some(recording_id.to_string());
    Ok(())
}

fn forget_recording(recording_id: &str) -> Result<(), String> {
    let mut active = active_recording_id()
        .lock()
        .map_err(|error| error.to_string())?;
    if active.as_deref() == Some(recording_id) {
        *active = None;
    }
    Ok(())
}

pub async fn health() -> Result<VoiceServiceResponse, String> {
    send("health", None).await
}

pub async fn start_recording() -> Result<VoiceServiceResponse, String> {
    let service_started = ensure_service().await?;
    let existing_recording_id = active_recording_id()
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    if let Some(existing_recording_id) = existing_recording_id {
        if let Ok(mut response) = recording_status(&existing_recording_id).await {
            if matches!(response.state.as_str(), "recording" | "transcribing") {
                remember_recording(&existing_recording_id)?;
                response.service_started = service_started;
                return Ok(response);
            }
        }
        forget_recording(&existing_recording_id)?;
    }
    let recording_id = Uuid::new_v4().to_string();
    remember_recording(&recording_id)?;

    let result = match send("start", Some(&recording_id)).await {
        Ok(response) if response.recording_id.as_deref() == Some(&recording_id) => Ok(response),
        Ok(response) => Err(format!(
            "Dictator started an unexpected recording {}; expected {recording_id}",
            response.recording_id.as_deref().unwrap_or("without an id")
        )),
        Err(start_error) => {
            recover_status(
                &recording_id,
                &["recording", "transcribing", "completed"],
                start_error,
            )
            .await
        }
    };

    match result {
        Ok(mut response) => {
            response.service_started = service_started;
            Ok(response)
        }
        Err(error) => Err(error),
    }
}

pub async fn stop_recording(recording_id: &str) -> Result<VoiceServiceResponse, String> {
    match send("stop", Some(recording_id)).await {
        Ok(response) => Ok(response),
        Err(stop_error) => {
            recover_status(
                recording_id,
                &["transcribing", "completed", "failed", "cancelled"],
                stop_error,
            )
            .await
        }
    }
}

pub async fn recording_status(recording_id: &str) -> Result<VoiceServiceResponse, String> {
    let response = send_with_timeout("status", Some(recording_id), REQUEST_TIMEOUT, false).await?;
    if matches!(
        response.state.as_str(),
        "completed" | "failed" | "cancelled"
    ) {
        forget_recording(recording_id)?;
    }
    Ok(response)
}

pub async fn cancel_recording(recording_id: &str) -> Result<VoiceServiceResponse, String> {
    let response = send("cancel", Some(recording_id)).await?;
    forget_recording(recording_id)?;
    Ok(response)
}

pub async fn cancel_active_recording() -> Result<(), String> {
    let recording_id = active_recording_id()
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    if let Some(recording_id) = recording_id {
        cancel_recording(&recording_id).await?;
    }
    Ok(())
}

fn socket_path() -> PathBuf {
    std::env::var_os(SOCKET_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir()
                .join("com.imploselabs.dictator")
                .join("voice-v1.sock")
        })
}

async fn ensure_service() -> Result<bool, String> {
    if send_with_timeout("health", None, PROBE_TIMEOUT, true)
        .await
        .is_ok()
    {
        return Ok(false);
    }

    let status = tokio::process::Command::new("/usr/bin/open")
        .args(["-a", "Dictator"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|e| format!("could not launch Dictator: {e}"))?;
    if !status.success() {
        return Err(format!(
            "could not launch Dictator: /usr/bin/open exited with {status}"
        ));
    }

    let deadline = Instant::now() + SERVICE_READY_TIMEOUT;
    let mut last_error = "Dictator voice service did not become ready".to_string();
    while Instant::now() < deadline {
        sleep(Duration::from_millis(200)).await;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match send_with_timeout("health", None, PROBE_TIMEOUT.min(remaining), true).await {
            Ok(_) => return Ok(true),
            Err(error) => last_error = error,
        }
    }
    Err(format!("Dictator voice service unavailable: {last_error}"))
}

async fn recover_status(
    recording_id: &str,
    accepted_states: &[&str],
    original_error: String,
) -> Result<VoiceServiceResponse, String> {
    let deadline = Instant::now() + RECOVERY_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(response) =
            send_with_timeout("status", Some(recording_id), PROBE_TIMEOUT, false).await
        {
            if accepted_states.contains(&response.state.as_str()) {
                return Ok(response);
            }
        }
        sleep(Duration::from_millis(200)).await;
    }
    Err(format!(
        "{original_error}; Dictator did not confirm recording {recording_id}"
    ))
}

async fn send(command: &str, recording_id: Option<&str>) -> Result<VoiceServiceResponse, String> {
    send_with_timeout(command, recording_id, REQUEST_TIMEOUT, true).await
}

fn build_voice_request(
    command: &str,
    recording_id: Option<&str>,
    request_id: &str,
) -> serde_json::Value {
    let mut request = serde_json::json!({
        "version": PROTOCOL_VERSION,
        "requestId": request_id,
        "command": command,
    });
    if let Some(recording_id) = recording_id {
        request["recording_id"] = serde_json::Value::String(recording_id.to_string());
    }
    request
}

async fn send_with_timeout(
    command: &str,
    recording_id: Option<&str>,
    request_timeout: Duration,
    reject_failure: bool,
) -> Result<VoiceServiceResponse, String> {
    let request_id = Uuid::new_v4().to_string();
    let request = build_voice_request(command, recording_id, &request_id);
    let response: VoiceServiceResponse = timeout(request_timeout, async {
        let mut stream = UnixStream::connect(socket_path()).await?;
        write_frame(&mut stream, &request).await?;
        read_frame(&mut stream).await
    })
    .await
    .map_err(|_| format!("timed out waiting for Dictator voice service during {command}"))?
    .map_err(|e: io::Error| e.to_string())?;

    if response.request_id != request_id {
        return Err("Dictator voice service returned a mismatched request id".to_string());
    }
    if response.version != PROTOCOL_VERSION {
        return Err(format!(
            "Dictator voice protocol version mismatch: expected {PROTOCOL_VERSION}, received {}",
            response.version
        ));
    }
    if response.ok || !reject_failure {
        Ok(response)
    } else {
        Err(response
            .error
            .unwrap_or_else(|| "Dictator voice request failed".to_string()))
    }
}

async fn write_frame(stream: &mut UnixStream, value: &serde_json::Value) -> io::Result<()> {
    let payload =
        serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Dictator voice request exceeded frame limit",
        ));
    }
    stream.write_u32(payload.len() as u32).await?;
    stream.write_all(&payload).await?;
    stream.flush().await
}

async fn read_frame<T: for<'de> Deserialize<'de>>(stream: &mut UnixStream) -> io::Result<T> {
    let length = stream.read_u32().await? as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid Dictator voice response length: {length}"),
        ));
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).await?;
    serde_json::from_slice(&payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_defaults_service_started_to_false() {
        let response: VoiceServiceResponse = serde_json::from_value(serde_json::json!({
            "version": 1,
            "requestId": "request-1",
            "ok": true,
            "state": "ready",
            "recordingId": null,
            "transcript": null,
            "error": null
        }))
        .unwrap();
        assert!(!response.service_started);
    }

    #[test]
    fn request_serializes_recording_id_with_protocol_field_name() {
        let request = build_voice_request("start", Some("recording-1"), "request-1");
        let json = serde_json::to_string(&request).unwrap();
        assert!(
            json.contains("\"recording_id\":\"recording-1\""),
            "expected recording_id in request frame, got {json}"
        );
        assert!(
            !json.contains("\"recordingId\""),
            "camelCase recordingId does not match the flattened command payload in {json}"
        );
    }
}
