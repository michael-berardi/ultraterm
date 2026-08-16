//! Anonymous, opt-out product telemetry.
//!
//! UltraTerm reports only coarse product-usage facts — an anonymous random
//! install ID, app version, OS/arch, and terminal counts — never names, file
//! paths, prompts, or any terminal content. Consent defaults to unset; the
//! app asks once on first run and honors the choice forever after. All
//! network sends are fire-and-forget with a short timeout and never block
//! startup or terminal work.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sysinfo::System;

const TELEMETRY_ENDPOINT: &str = "https://analytics.libertydesign.studio/api/ultraterm/event";
const TELEMETRY_SCHEMA: &str = "lds.ultraterm.event.v1";
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelemetryConsent {
    #[default]
    Unset,
    Enabled,
    Disabled,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TelemetryFile {
    #[serde(default)]
    install_id: Option<String>,
    #[serde(default)]
    consent: TelemetryConsent,
    #[serde(default)]
    last_heartbeat_day: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppTelemetryState {
    pub consent: TelemetryConsent,
}

fn state_path(home: &Path) -> PathBuf {
    home.join(".ultraterm").join("app-telemetry.json")
}

fn load_state(home: &Path) -> TelemetryFile {
    let Ok(content) = fs::read_to_string(state_path(home)) else {
        return TelemetryFile::default();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_state(home: &Path, state: &TelemetryFile) -> Result<(), String> {
    let path = state_path(home);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create telemetry directory: {error}"))?;
    }
    let content = serde_json::to_string_pretty(state)
        .map_err(|error| format!("Failed to serialize telemetry state: {error}"))?;
    fs::write(&path, content).map_err(|error| format!("Failed to save telemetry state: {error}"))
}

fn install_id(state: &mut TelemetryFile) -> String {
    if state.install_id.is_none() {
        state.install_id = Some(uuid::Uuid::new_v4().to_string());
    }
    state.install_id.clone().unwrap_or_default()
}

async fn send_event(
    state: &TelemetryFile,
    event: &str,
    data: serde_json::Value,
) -> Result<(), String> {
    let Some(install_id) = state.install_id.as_deref() else {
        return Err("Telemetry install ID is missing.".to_string());
    };
    let payload = json!({
        "schema": TELEMETRY_SCHEMA,
        "event": event,
        "installId": install_id,
        "version": env!("CARGO_PKG_VERSION"),
        "os": "macos",
        "arch": std::env::consts::ARCH,
        "osVersion": System::os_version().unwrap_or_default(),
        "data": data,
        "sentAt": chrono::Utc::now().timestamp(),
    });
    let response = crate::updater::http_client(HTTP_TIMEOUT)?
        .post(TELEMETRY_ENDPOINT)
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("Telemetry event failed: {error}"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("Telemetry endpoint returned HTTP {}", response.status()))
    }
}

#[tauri::command]
pub fn app_telemetry_state() -> Result<AppTelemetryState, String> {
    let home = crate::home_dir().map_err(|error| error.to_string())?;
    Ok(AppTelemetryState {
        consent: load_state(&home).consent,
    })
}

#[tauri::command(async)]
pub async fn set_app_telemetry_consent(enabled: bool) -> Result<(), String> {
    let home = crate::home_dir().map_err(|error| error.to_string())?;
    let mut state = load_state(&home);
    state.consent = if enabled {
        TelemetryConsent::Enabled
    } else {
        TelemetryConsent::Disabled
    };
    if enabled {
        install_id(&mut state);
    }
    save_state(&home, &state)?;
    if enabled {
        // Confirm the choice with an immediate launch event so the dashboard
        // reflects the install without waiting for the next app start.
        let _ = send_event(&state, "launch", json!({ "reason": "consent" })).await;
    }
    Ok(())
}

/// Records a product-usage event. `event` is `launch` on every boot and
/// `heartbeat` at most once per local day; both are no-ops unless the user
/// has explicitly left telemetry enabled.
#[tauri::command(async)]
pub async fn record_app_event(event: String, data: serde_json::Value) -> Result<(), String> {
    let home = crate::home_dir().map_err(|error| error.to_string())?;
    let mut state = load_state(&home);
    if state.consent != TelemetryConsent::Enabled {
        return Ok(());
    }
    let event = match event.as_str() {
        "launch" => "launch",
        "heartbeat" => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            if state.last_heartbeat_day.as_deref() == Some(&today) {
                return Ok(());
            }
            state.last_heartbeat_day = Some(today);
            "heartbeat"
        }
        _ => return Err(format!("Unknown telemetry event: {event}")),
    };
    install_id(&mut state);
    save_state(&home, &state)?;
    send_event(&state, event, data).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_round_trips_and_defaults_unset() {
        let root = std::env::temp_dir().join(format!(
            "ultraterm-telemetry-consent-{}",
            std::process::id()
        ));
        let home = root.as_path();
        assert_eq!(load_state(home).consent, TelemetryConsent::Unset);

        let mut state = TelemetryFile::default();
        state.consent = TelemetryConsent::Enabled;
        install_id(&mut state);
        save_state(home, &state).unwrap();

        let loaded = load_state(home);
        assert_eq!(loaded.consent, TelemetryConsent::Enabled);
        assert!(loaded.install_id.is_some());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn install_id_is_stable_once_generated() {
        let mut state = TelemetryFile::default();
        let first = install_id(&mut state);
        let second = install_id(&mut state);
        assert_eq!(first, second);
        assert!(!first.is_empty());
    }
}
