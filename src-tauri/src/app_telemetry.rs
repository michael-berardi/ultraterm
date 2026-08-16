//! Anonymous, opt-in product telemetry.
//!
//! The app sends only the strict multi-app v2 envelope: a random install UUID
//! created after opt-in, coarse platform/version information, a UTC day, and
//! daily terminal/session counters. Consent defaults to unset. Declining is
//! persisted and erases all telemetry state. Network failures are deliberately
//! silent and never block terminal work.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use uuid::Uuid;

const TELEMETRY_ENDPOINT: &str = "https://analytics.libertydesign.studio/api/app-telemetry/event";
const TELEMETRY_SCHEMA: &str = "lds.app-telemetry.event.v2";
const APP_NAME: &str = "ultraterm";
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_USAGE_COUNTER: u64 = 1_000_000;
const STATE_FILE_NAME: &str = "app-telemetry.json";
const MAX_PENDING_DAYS: usize = 34;

// These are app-telemetry state names used by older builds. The token-history
// files (telemetry-sessions.json and telemetry.sqlite3) are intentionally not
// included: they are local token-history storage, not app telemetry state.
const LEGACY_STATE_FILE_NAMES: &[&str] = &[
    "telemetry.json",
    "app-telemetry-state.json",
    "telemetry-state.json",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelemetryConsent {
    #[default]
    Unset,
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
struct UsageCounters {
    terminals: u64,
    sessions: u64,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TelemetryFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    install_id: Option<String>,
    #[serde(default)]
    consent: TelemetryConsent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_heartbeat_day: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pending_usage_by_day: BTreeMap<String, UsageCounters>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppTelemetryState {
    pub consent: TelemetryConsent,
}

fn counters_are_zero(counters: &UsageCounters) -> bool {
    counters.terminals == 0 && counters.sessions == 0
}

fn utc_day() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

fn state_path(home: &Path) -> PathBuf {
    home.join(".ultraterm").join(STATE_FILE_NAME)
}

fn legacy_state_paths(home: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    LEGACY_STATE_FILE_NAMES
        .iter()
        .map(|name| home.join(".ultraterm").join(name))
}

fn parse_consent(value: &Value) -> TelemetryConsent {
    match value {
        Value::String(value) => match value.to_ascii_lowercase().as_str() {
            "enabled" | "accepted" | "opted_in" | "optedin" => TelemetryConsent::Enabled,
            "disabled" | "declined" | "opted_out" | "optedout" => TelemetryConsent::Disabled,
            _ => TelemetryConsent::Unset,
        },
        Value::Bool(enabled) => {
            if *enabled {
                TelemetryConsent::Enabled
            } else {
                TelemetryConsent::Disabled
            }
        }
        _ => TelemetryConsent::Unset,
    }
}

fn parse_state(content: &str) -> TelemetryFile {
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return TelemetryFile::default();
    };
    let mut state = serde_json::from_value::<TelemetryFile>(value.clone()).unwrap_or_default();

    // Accept common legacy spellings while migrating only app-telemetry state.
    // No identifier is generated here; that happens only once consent is
    // enabled and an event is actually prepared.
    if state.consent == TelemetryConsent::Unset {
        for key in ["consent", "telemetryConsent", "enabled", "optIn", "optedIn"] {
            if let Some(value) = value.get(key) {
                state.consent = parse_consent(value);
                if state.consent != TelemetryConsent::Unset {
                    break;
                }
            }
        }
    }
    if state.install_id.is_none() {
        state.install_id = ["installId", "install_id", "telemetryId", "id"]
            .into_iter()
            .find_map(|key| value.get(key).and_then(Value::as_str).map(str::to_string));
    }
    if state.last_heartbeat_day.is_none() {
        state.last_heartbeat_day = ["lastHeartbeatDay", "last_heartbeat_day", "lastSentDay"]
            .into_iter()
            .find_map(|key| value.get(key).and_then(Value::as_str).map(str::to_string));
    }
    if state.pending_usage_by_day.is_empty() {
        let terminals = ["pendingTerminals", "pending_terminals"]
            .into_iter()
            .find_map(|key| value.get(key).and_then(Value::as_u64))
            .unwrap_or(0)
            .min(MAX_USAGE_COUNTER);
        let sessions = ["pendingSessions", "pending_sessions"]
            .into_iter()
            .find_map(|key| value.get(key).and_then(Value::as_u64))
            .unwrap_or(0)
            .min(MAX_USAGE_COUNTER);
        if terminals > 0 || sessions > 0 {
            state.pending_usage_by_day.insert(
                utc_day(),
                UsageCounters {
                    terminals,
                    sessions,
                },
            );
        }
    }
    for counters in state.pending_usage_by_day.values_mut() {
        counters.terminals = counters.terminals.min(MAX_USAGE_COUNTER);
        counters.sessions = counters.sessions.min(MAX_USAGE_COUNTER);
    }
    state
        .pending_usage_by_day
        .retain(|_, counters| !counters_are_zero(counters));
    while state.pending_usage_by_day.len() > MAX_PENDING_DAYS {
        let Some(oldest) = state.pending_usage_by_day.keys().next().cloned() else {
            break;
        };
        state.pending_usage_by_day.remove(&oldest);
    }
    if state.consent == TelemetryConsent::Disabled {
        erase_telemetry_state(&mut state);
    }
    state
}

fn load_state(home: &Path) -> TelemetryFile {
    if let Ok(content) = fs::read_to_string(state_path(home)) {
        let state = parse_state(&content);
        if state.consent != TelemetryConsent::Unset {
            return state;
        }
    }

    for path in legacy_state_paths(home) {
        if let Ok(content) = fs::read_to_string(path) {
            let state = parse_state(&content);
            if state.consent != TelemetryConsent::Unset {
                return state;
            }
        }
    }
    TelemetryFile::default()
}

fn save_state(home: &Path, state: &TelemetryFile) -> Result<(), String> {
    let path = state_path(home);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create telemetry directory: {error}"))?;
    }
    let content = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("Failed to serialize telemetry state: {error}"))?;
    // Replace the state in one rename so a failed write cannot leave a partial
    // consent choice or partially updated retry counters.
    let temporary_path = path.with_extension("json.tmp");
    fs::write(&temporary_path, content)
        .map_err(|error| format!("Failed to write telemetry state: {error}"))?;
    fs::rename(&temporary_path, &path)
        .map_err(|error| format!("Failed to commit telemetry state: {error}"))
}

fn erase_telemetry_state(state: &mut TelemetryFile) {
    state.install_id = None;
    state.last_heartbeat_day = None;
    state.pending_usage_by_day.clear();
}

fn remove_legacy_state(home: &Path) {
    for path in legacy_state_paths(home) {
        let _ = fs::remove_file(path);
    }
}

fn valid_install_id(value: Option<&str>) -> bool {
    value
        .and_then(|value| Uuid::parse_str(value).ok())
        .is_some_and(|id| id.get_version_num() == 4 && id.get_variant() == uuid::Variant::RFC4122)
}

fn install_id(state: &mut TelemetryFile) -> String {
    if !valid_install_id(state.install_id.as_deref()) {
        state.install_id = Some(Uuid::new_v4().to_string());
    }
    state.install_id.clone().unwrap_or_default()
}

fn send_allowed(state: &TelemetryFile) -> bool {
    state.consent == TelemetryConsent::Enabled && valid_install_id(state.install_id.as_deref())
}

fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        "linux" => "linux",
        "ios" => "ios",
        "android" => "android",
        "emscripten" => "web",
        _ => "unknown",
    }
}

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        "x86" | "i686" => "x86",
        _ => "unknown",
    }
}

#[cfg(test)]
fn semver(value: &str) -> bool {
    let mut parts = value.split('.');
    let valid = (0..3).all(|_| {
        parts.next().is_some_and(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
    });
    valid && parts.next().is_none()
}

fn usage_value(counters: UsageCounters) -> Value {
    let mut usage = Map::new();
    if counters.terminals > 0 {
        usage.insert("terminals".to_string(), json!(counters.terminals));
    }
    if counters.sessions > 0 {
        usage.insert("sessions".to_string(), json!(counters.sessions));
    }
    Value::Object(usage)
}

fn build_payload(event: &str, install_id: &str, day: &str, usage: Option<UsageCounters>) -> Value {
    let mut payload = json!({
        "schema": TELEMETRY_SCHEMA,
        "app": APP_NAME,
        "event": event,
        "installId": install_id,
        "version": env!("CARGO_PKG_VERSION"),
        "platform": platform(),
        "arch": arch(),
        "day": day,
    });
    if event == "usage" {
        payload["usage"] = usage_value(usage.unwrap_or_default());
    }
    payload
}

async fn send_event(
    state: &TelemetryFile,
    event: &str,
    day: &str,
    usage: Option<UsageCounters>,
) -> Result<(), String> {
    let Some(install_id) = state.install_id.as_deref() else {
        return Err("Telemetry install ID is missing.".to_string());
    };
    if !send_allowed(state) {
        return Err("Telemetry consent or install ID is invalid.".to_string());
    }
    let payload = build_payload(event, install_id, day, usage);
    let response = crate::updater::http_client(HTTP_TIMEOUT)?
        .post(TELEMETRY_ENDPOINT)
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("Telemetry event failed: {error}"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "Telemetry endpoint returned HTTP {}",
            response.status()
        ))
    }
}

fn parse_usage(data: Value) -> Result<UsageCounters, String> {
    let Value::Object(data) = data else {
        return Err("Usage telemetry must be an object.".to_string());
    };
    if data
        .keys()
        .any(|key| !matches!(key.as_str(), "terminals" | "sessions"))
    {
        return Err("Usage telemetry contains an unsupported field.".to_string());
    }

    fn counter(data: &Map<String, Value>, key: &str) -> Result<u64, String> {
        let Some(value) = data.get(key) else {
            return Ok(0);
        };
        let Some(value) = value.as_u64() else {
            return Err(format!("Usage counter {key} must be an integer."));
        };
        if value > MAX_USAGE_COUNTER {
            return Err(format!("Usage counter {key} exceeds the limit."));
        }
        Ok(value)
    }

    Ok(UsageCounters {
        terminals: counter(&data, "terminals")?,
        sessions: counter(&data, "sessions")?,
    })
}

fn add_pending_usage(state: &mut TelemetryFile, day: &str, delta: UsageCounters) {
    let pending = state
        .pending_usage_by_day
        .entry(day.to_string())
        .or_default();
    pending.terminals = pending
        .terminals
        .saturating_add(delta.terminals)
        .min(MAX_USAGE_COUNTER);
    pending.sessions = pending
        .sessions
        .saturating_add(delta.sessions)
        .min(MAX_USAGE_COUNTER);
    while state.pending_usage_by_day.len() > MAX_PENDING_DAYS {
        let Some(oldest) = state.pending_usage_by_day.keys().next().cloned() else {
            break;
        };
        state.pending_usage_by_day.remove(&oldest);
    }
}

fn daily_due(last_day: Option<&str>, today: &str) -> bool {
    last_day != Some(today)
}

async fn send_pending_usage(
    home: &Path,
    state: &mut TelemetryFile,
    today: &str,
) -> Result<(), String> {
    let completed_days = state
        .pending_usage_by_day
        .iter()
        .filter(|(day, counters)| day.as_str() < today && !counters_are_zero(counters))
        .map(|(day, counters)| (day.clone(), *counters))
        .collect::<Vec<_>>();
    let mut changed = false;
    for (day, usage) in completed_days {
        if !TELEMETRY_RUNTIME_ENABLED.load(Ordering::Acquire) {
            break;
        }
        if send_event(state, "usage", &day, Some(usage)).await.is_ok() {
            state.pending_usage_by_day.remove(&day);
            changed = true;
        }
    }
    if changed {
        save_state(home, state)?;
    }
    Ok(())
}

fn set_consent(home: &Path, enabled: bool) -> Result<(), String> {
    let mut state = load_state(home);
    if enabled {
        state.consent = TelemetryConsent::Enabled;
        // Existing opted-in users retain a valid UUID; disabled users receive a
        // fresh one only when they opt back in.
        install_id(&mut state);
        save_state(home, &state)
    } else {
        state.consent = TelemetryConsent::Disabled;
        erase_telemetry_state(&mut state);
        remove_legacy_state(home);
        save_state(home, &state)
    }
}
static TELEMETRY_OPERATION_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
static TELEMETRY_RUNTIME_ENABLED: AtomicBool = AtomicBool::new(false);

#[tauri::command(async)]
pub async fn app_telemetry_state() -> Result<AppTelemetryState, String> {
    let _guard = TELEMETRY_OPERATION_LOCK.lock().await;
    let home = crate::home_dir().map_err(|error| error.to_string())?;
    let consent = load_state(&home).consent;
    TELEMETRY_RUNTIME_ENABLED.store(consent == TelemetryConsent::Enabled, Ordering::Release);
    Ok(AppTelemetryState { consent })
}

#[tauri::command(async)]
pub async fn set_app_telemetry_consent(enabled: bool) -> Result<(), String> {
    if !enabled {
        // Stop queued sends before waiting for any in-flight request to release
        // the operation lock. At most that one request can finish after opt-out.
        TELEMETRY_RUNTIME_ENABLED.store(false, Ordering::Release);
    }
    let _guard = TELEMETRY_OPERATION_LOCK.lock().await;
    let home = crate::home_dir().map_err(|error| error.to_string())?;
    let result = set_consent(&home, enabled);
    if result.is_ok() && enabled {
        TELEMETRY_RUNTIME_ENABLED.store(true, Ordering::Release);
    }
    result
}

/// Records a product event. Launch and heartbeat never carry usage data.
/// Usage calls add successful terminal/session starts to a persisted daily
/// pending counter and send at most one usage report per UTC day. All network
/// failures are swallowed so telemetry cannot affect the app.
#[tauri::command(async)]
pub async fn record_app_event(event: String, data: Value) -> Result<(), String> {
    if !TELEMETRY_RUNTIME_ENABLED.load(Ordering::Acquire) {
        return Ok(());
    }
    let _guard = TELEMETRY_OPERATION_LOCK.lock().await;
    if !TELEMETRY_RUNTIME_ENABLED.load(Ordering::Acquire) {
        return Ok(());
    }
    let home = crate::home_dir().map_err(|error| error.to_string())?;
    let mut state = load_state(&home);
    if state.consent != TelemetryConsent::Enabled {
        return Ok(());
    }

    let today = utc_day();
    match event.as_str() {
        "launch" | "heartbeat" => {
            if !data.as_object().is_some_and(|object| object.is_empty()) {
                return Err(format!("{event} telemetry cannot contain usage data."));
            }
            install_id(&mut state);
            save_state(&home, &state)?;
            if event == "launch" {
                // Launch is emitted once by the app after startup settles. No
                // timestamp is persisted because enabling during a running app
                // must not manufacture a second startup marker.
                let _ = send_event(&state, "launch", &today, None).await;
                return Ok(());
            }

            if daily_due(state.last_heartbeat_day.as_deref(), &today) {
                if send_event(&state, "heartbeat", &today, None).await.is_ok() {
                    state.last_heartbeat_day = Some(today.clone());
                    save_state(&home, &state)?;
                }
            }
            send_pending_usage(&home, &mut state, &today).await
        }
        "usage" => {
            let delta = parse_usage(data)?;
            install_id(&mut state);
            add_pending_usage(&mut state, &today, delta);
            save_state(&home, &state)
        }
        _ => Err(format!("Unknown telemetry event: {event}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use tempfile::tempdir;

    #[test]
    fn payload_has_only_the_v2_allowlist() {
        let payload = build_payload(
            "usage",
            "00000000-0000-4000-8000-000000000000",
            "2026-08-16",
            Some(UsageCounters {
                terminals: 3,
                sessions: 2,
            }),
        );
        let keys: BTreeSet<&str> = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from([
                "app",
                "arch",
                "day",
                "event",
                "installId",
                "platform",
                "schema",
                "usage",
                "version",
            ])
        );
        assert_eq!(payload["schema"], TELEMETRY_SCHEMA);
        assert_eq!(payload["app"], APP_NAME);
        assert_eq!(payload["usage"]["terminals"], 3);
        assert_eq!(payload["usage"]["sessions"], 2);
    }

    #[test]
    fn package_version_is_strict_semver() {
        assert!(semver(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn non_usage_payload_forbids_usage_key_and_sensitive_fields() {
        let payload = build_payload(
            "heartbeat",
            "00000000-0000-4000-8000-000000000000",
            "2026-08-16",
            None,
        );
        let object = payload.as_object().unwrap();
        assert!(!object.contains_key("usage"));
        for forbidden in [
            "sentAt",
            "osVersion",
            "locale",
            "networkId",
            "url",
            "title",
            "model",
            "tokens",
            "commands",
            "paths",
        ] {
            assert!(
                !object.contains_key(forbidden),
                "unexpected field {forbidden}"
            );
        }
    }

    #[test]
    fn consent_defaults_unset_and_does_not_create_an_identifier() {
        let directory = tempdir().unwrap();
        let state = load_state(directory.path());
        assert_eq!(state.consent, TelemetryConsent::Unset);
        assert!(state.install_id.is_none());
        assert!(!valid_install_id(state.install_id.as_deref()));
        assert!(!send_allowed(&state));
    }

    #[test]
    fn decline_persists_and_erases_current_and_legacy_state() {
        let directory = tempdir().unwrap();
        let legacy = directory.path().join(".ultraterm").join("telemetry.json");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(
            &legacy,
            json!({
                "consent": "enabled",
                "installId": "00000000-0000-4000-8000-000000000000",
                "lastHeartbeatDay": "2026-08-15",
                "pendingTerminals": 4,
                "pendingSessions": 2,
            })
            .to_string(),
        )
        .unwrap();
        set_consent(directory.path(), true).unwrap();
        let mut state = load_state(directory.path());
        assert_eq!(state.consent, TelemetryConsent::Enabled);
        assert!(valid_install_id(state.install_id.as_deref()));
        state.last_heartbeat_day = Some("2026-08-15".to_string());
        add_pending_usage(
            &mut state,
            "2026-08-16",
            UsageCounters {
                terminals: 4,
                sessions: 2,
            },
        );
        save_state(directory.path(), &state).unwrap();

        set_consent(directory.path(), false).unwrap();
        let disabled = load_state(directory.path());
        assert_eq!(disabled.consent, TelemetryConsent::Disabled);
        assert!(disabled.install_id.is_none());
        assert!(disabled.last_heartbeat_day.is_none());
        assert!(disabled.pending_usage_by_day.is_empty());
        assert!(!legacy.exists());
    }

    #[test]
    fn persisted_decline_discards_legacy_identifier_and_pending_usage() {
        let state = parse_state(
            &json!({
                "consent": "declined",
                "installId": "00000000-0000-4000-8000-000000000000",
                "lastHeartbeatDay": "2026-08-15",
                "pendingTerminals": MAX_USAGE_COUNTER + 1,
                "pendingSessions": 2,
            })
            .to_string(),
        );
        assert_eq!(state.consent, TelemetryConsent::Disabled);
        assert!(state.install_id.is_none());
        assert!(state.last_heartbeat_day.is_none());
        assert!(state.pending_usage_by_day.is_empty());
    }

    #[test]
    fn opt_in_creates_a_v4_uuid_only_after_opt_in() {
        let directory = tempdir().unwrap();
        assert!(load_state(directory.path()).install_id.is_none());
        set_consent(directory.path(), true).unwrap();
        let state = load_state(directory.path());
        assert_eq!(state.consent, TelemetryConsent::Enabled);
        assert!(valid_install_id(state.install_id.as_deref()));
        assert!(!valid_install_id(Some(
            "00000000-0000-4000-0000-000000000000"
        )));
    }

    #[test]
    fn daily_cadence_is_utc_day_based() {
        assert!(daily_due(None, "2026-08-16"));
        assert!(!daily_due(Some("2026-08-16"), "2026-08-16"));
        assert!(daily_due(Some("2026-08-15"), "2026-08-16"));
    }

    #[test]
    fn usage_counters_remain_attributed_to_their_utc_day() {
        let mut state = TelemetryFile {
            consent: TelemetryConsent::Enabled,
            install_id: Some("00000000-0000-4000-8000-000000000000".to_string()),
            ..TelemetryFile::default()
        };
        add_pending_usage(
            &mut state,
            "2026-08-16",
            UsageCounters {
                terminals: 2,
                sessions: 1,
            },
        );
        add_pending_usage(
            &mut state,
            "2026-08-17",
            UsageCounters {
                terminals: 1,
                sessions: 1,
            },
        );
        assert_eq!(state.pending_usage_by_day.len(), 2);
        assert_eq!(
            state.pending_usage_by_day["2026-08-16"],
            UsageCounters {
                terminals: 2,
                sessions: 1,
            }
        );
        assert_eq!(
            state.pending_usage_by_day["2026-08-17"],
            UsageCounters {
                terminals: 1,
                sessions: 1,
            }
        );
    }

    #[test]
    fn usage_allowlist_and_bounds_are_strict() {
        assert_eq!(
            parse_usage(json!({"terminals": 2, "sessions": 1})).unwrap(),
            UsageCounters {
                terminals: 2,
                sessions: 1,
            }
        );
        assert!(parse_usage(json!({"unknown": 1})).is_err());
        assert!(parse_usage(json!({"terminals": -1})).is_err());
        assert!(parse_usage(json!({"sessions": 1.5})).is_err());
        assert!(parse_usage(json!({"terminals": MAX_USAGE_COUNTER + 1})).is_err());
    }
}
