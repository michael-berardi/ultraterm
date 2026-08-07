//! Remote provider usage fetching with macOS Keychain-backed credentials.
//!
//! Commands:
//!   provider_usage() -> ProviderUsage[]            (all four providers, fetched concurrently)
//!   save_provider_credential({ input }) -> ProviderUsage
//!   remove_provider_credential({ provider }) -> ()
//!
//! Credentials live only in the macOS Keychain (service = app identifier). Secret
//! values are never returned to the frontend, never logged, and never written to disk.
//! Usage numbers always come from the remote provider endpoints below — never from
//! local transcript estimates.
//!
//! Endpoint notes (source-verified against each provider's first-party client):
//!   Kimi   GET https://api.kimi.com/coding/v1/usages        Authorization: Bearer
//!          Payload: root `usage` object, or `limits[]` items shaped
//!          `{detail: {limit, used|remaining, name|title, reset_at|reset_in…}, window: {duration, timeUnit}}`.
//!   Codex  GET https://chatgpt.com/backend-api/wham/usage   Authorization: Bearer
//!          plus ChatGPT-Account-Id when an account id is stored. ChatGPT OAuth only.
//!          Payload: `{plan_type, rate_limit: {primary_window, secondary_window}, credits}`.
//!   Claude GET https://api.anthropic.com/api/oauth/usage    Authorization: Bearer
//!          plus anthropic-beta oauth header. Undocumented; subscription OAuth only —
//!          API keys are rejected, so auth failures surface as actionable 401/403 errors.
//!          Payload: keyed windows `five_hour`, `seven_day`, `seven_day_opus`, … with
//!          `utilization` (0..1 fraction or 0..100) and `resets_at`.
//!   ZAI    GET https://api.z.ai/api/monitor/usage/quota/limit
//!          Authorization: <raw Coding Plan token> (NO Bearer prefix — the official
//!          @z_ai/coding-helper script sends the token verbatim). Payload:
//!          `data.limits[]` where TOKENS_LIMIT is the 5-hour window and TIME_LIMIT is
//!          the monthly MCP window. No reset timestamp is confirmed; none is invented.

use std::process::Stdio;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine as _;
use reqwest::header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::task::JoinSet;

const KEYCHAIN_SERVICE: &str = "com.libertydesignstudio.ultraterm";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_PARSE_DEPTH: u8 = 4;

static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("ultraterm/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("reqwest client builder must succeed")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Kimi,
    Codex,
    Claude,
    Zai,
}

impl ProviderId {
    const ALL: [ProviderId; 4] = [
        ProviderId::Kimi,
        ProviderId::Codex,
        ProviderId::Claude,
        ProviderId::Zai,
    ];

    fn display_name(self) -> &'static str {
        match self {
            ProviderId::Kimi => "Kimi",
            ProviderId::Codex => "Codex",
            ProviderId::Claude => "Claude",
            ProviderId::Zai => "Z.ai",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            ProviderId::Kimi => "https://api.kimi.com/coding/v1/usages",
            ProviderId::Codex => "https://chatgpt.com/backend-api/wham/usage",
            ProviderId::Claude => "https://api.anthropic.com/api/oauth/usage",
            ProviderId::Zai => "https://api.z.ai/api/monitor/usage/quota/limit",
        }
    }

    fn keychain_account(self) -> &'static str {
        match self {
            ProviderId::Kimi => "provider-usage-kimi",
            ProviderId::Codex => "provider-usage-codex",
            ProviderId::Claude => "provider-usage-claude",
            ProviderId::Zai => "provider-usage-zai",
        }
    }

    fn omp_auth_source(self) -> Option<(&'static str, &'static str)> {
        match self {
            ProviderId::Kimi => Some(("--profile=kimi-k3", "kimi-code")),
            ProviderId::Codex => Some(("--profile=gpt-only", "openai-codex")),
            ProviderId::Claude | ProviderId::Zai => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageWindow {
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub provider: ProviderId,
    pub display_name: String,
    pub plan: Option<String>,
    pub status: String,
    pub windows: Vec<ProviderUsageWindow>,
    pub balance: Option<String>,
    pub updated_at: Option<i64>,
    pub error: Option<String>,
}

impl ProviderUsage {
    fn shell(provider: ProviderId, status: &str) -> Self {
        Self {
            provider,
            display_name: provider.display_name().to_string(),
            plan: None,
            status: status.to_string(),
            windows: Vec::new(),
            balance: None,
            updated_at: None,
            error: None,
        }
    }

    fn disconnected(provider: ProviderId) -> Self {
        Self::shell(provider, "disconnected")
    }

    fn error(provider: ProviderId, message: String) -> Self {
        let mut usage = Self::shell(provider, "error");
        usage.error = Some(message);
        usage
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCredentialInput {
    pub provider: ProviderId,
    pub access_token: String,
    pub account_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredCredential {
    access_token: String,
    #[serde(default)]
    account_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Keychain storage
// ---------------------------------------------------------------------------

fn keychain_entry(provider: ProviderId) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, provider.keychain_account())
        .map_err(|error| format!("macOS Keychain unavailable: {error}"))
}

fn read_credential(provider: ProviderId) -> Result<Option<StoredCredential>, String> {
    let entry = keychain_entry(provider)?;
    match entry.get_password() {
        Ok(raw) => serde_json::from_str(&raw).map(Some).map_err(|_| {
            format!(
                "Stored {} credential is corrupted; save it again from Settings.",
                provider.display_name()
            )
        }),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!(
            "Could not read the {} credential from Keychain: {error}",
            provider.display_name()
        )),
    }
}

fn write_credential(provider: ProviderId, credential: &StoredCredential) -> Result<(), String> {
    let entry = keychain_entry(provider)?;
    let payload = serde_json::to_string(credential)
        .map_err(|error| format!("Could not serialize credential: {error}"))?;
    entry.set_password(&payload).map_err(|error| {
        format!(
            "Could not save the {} credential to Keychain: {error}",
            provider.display_name()
        )
    })
}

fn delete_credential(provider: ProviderId) -> Result<(), String> {
    let entry = keychain_entry(provider)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!(
            "Could not remove the {} credential from Keychain: {error}",
            provider.display_name()
        )),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn provider_usage() -> Result<Vec<ProviderUsage>, String> {
    let mut tasks = JoinSet::new();
    for (index, provider) in ProviderId::ALL.into_iter().enumerate() {
        let credential = match read_credential(provider) {
            Ok(credential) => credential,
            Err(error) => {
                // Keychain read failures are per-provider, not list-fatal.
                let usage = ProviderUsage::error(provider, error);
                tasks.spawn(async move { (index, usage) });
                continue;
            }
        };
        tasks.spawn(async move { (index, fetch_provider(provider, credential).await) });
    }

    let mut results: Vec<(usize, ProviderUsage)> = Vec::with_capacity(ProviderId::ALL.len());
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(pair) => results.push(pair),
            Err(error) => {
                if results.is_empty() && tasks.is_empty() {
                    return Err(format!("Provider usage fetch failed: {error}"));
                }
            }
        }
    }
    results.sort_by_key(|(index, _)| *index);
    Ok(results.into_iter().map(|(_, usage)| usage).collect())
}

#[tauri::command]
pub async fn save_provider_credential(
    input: ProviderCredentialInput,
) -> Result<ProviderUsage, String> {
    let access_token = input.access_token.trim().to_string();
    if access_token.is_empty() {
        return Err("Access token cannot be empty.".to_string());
    }
    let account_id = input
        .account_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let credential = StoredCredential {
        access_token,
        account_id,
    };
    let usage = fetch_remote(input.provider, &credential).await?;
    write_credential(input.provider, &credential)?;
    Ok(usage)
}

#[tauri::command]
pub async fn remove_provider_credential(provider: ProviderId) -> Result<(), String> {
    delete_credential(provider)
}

// ---------------------------------------------------------------------------
// Remote fetching
// ---------------------------------------------------------------------------

fn codex_account_id(access_token: &str) -> Option<String> {
    let encoded_payload = access_token.split('.').nth(1)?;
    let payload = URL_SAFE_NO_PAD
        .decode(encoded_payload)
        .or_else(|_| URL_SAFE.decode(encoded_payload))
        .ok()?;
    let claims: Value = serde_json::from_slice(&payload).ok()?;
    let claims = claims.as_object()?;
    let account_id = claims
        .get("chatgpt_account_id")
        .or_else(|| {
            claims
                .get("https://api.openai.com/auth")
                .and_then(Value::as_object)
                .and_then(|auth| auth.get("chatgpt_account_id"))
        })
        .or_else(|| {
            claims
                .get("organizations")
                .and_then(Value::as_array)
                .and_then(|organizations| organizations.first())
                .and_then(Value::as_object)
                .and_then(|organization| organization.get("id"))
        })?
        .as_str()?
        .trim();
    (!account_id.is_empty()).then(|| account_id.to_string())
}

async fn read_omp_credential(provider: ProviderId) -> Option<StoredCredential> {
    let (profile_argument, token_provider) = provider.omp_auth_source()?;
    let omp = crate::resolve_optional_executable("OMP_BIN", "omp", &[])
        .ok()
        .flatten()?;
    let mut command = tokio::process::Command::new(omp);
    command
        .args([profile_argument, "token", token_provider])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(REQUEST_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let access_token = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if access_token.is_empty() {
        return None;
    }
    let account_id = match provider {
        ProviderId::Codex => codex_account_id(&access_token),
        ProviderId::Kimi | ProviderId::Claude | ProviderId::Zai => None,
    };
    Some(StoredCredential {
        access_token,
        account_id,
    })
}

async fn fetch_provider(
    provider: ProviderId,
    credential: Option<StoredCredential>,
) -> ProviderUsage {
    let credential = match credential {
        Some(credential) => credential,
        None => match read_omp_credential(provider).await {
            Some(credential) => credential,
            None => return ProviderUsage::disconnected(provider),
        },
    };
    match fetch_remote(provider, &credential).await {
        Ok(usage) => usage,
        Err(error) => ProviderUsage::error(provider, error),
    }
}

async fn fetch_remote(
    provider: ProviderId,
    credential: &StoredCredential,
) -> Result<ProviderUsage, String> {
    let display = provider.display_name();
    let mut request = HTTP_CLIENT.get(provider.endpoint());
    match provider {
        ProviderId::Zai => {
            // The Z.ai Coding Plan quota route takes the raw token verbatim —
            // no Bearer prefix (matches the official @z_ai/coding-helper client).
            let token = HeaderValue::from_str(credential.access_token.trim()).map_err(|_| {
                "Access token contains characters that are not valid in an HTTP header.".to_string()
            })?;
            request = request
                .header(AUTHORIZATION, token)
                .header(ACCEPT, "application/json");
        }
        ProviderId::Kimi | ProviderId::Codex | ProviderId::Claude => {
            let bearer =
                HeaderValue::from_str(&format!("Bearer {}", credential.access_token.trim()))
                    .map_err(|_| {
                        "Access token contains characters that are not valid in an HTTP header."
                            .to_string()
                    })?;
            request = request.header(AUTHORIZATION, bearer);
        }
    }
    match provider {
        ProviderId::Codex => {
            if let Some(account_id) = credential.account_id.as_deref() {
                let value = HeaderValue::from_str(account_id).map_err(|_| {
                    "Account ID contains characters that are not valid in an HTTP header."
                        .to_string()
                })?;
                request = request.header("ChatGPT-Account-Id", value);
            }
        }
        ProviderId::Claude => {
            request = request
                .header("anthropic-beta", "oauth-2025-04-20")
                .header(CONTENT_TYPE, "application/json");
        }
        ProviderId::Kimi | ProviderId::Zai => {}
    }

    let response = request
        .send()
        .await
        .map_err(|error| format!("Could not reach {display}: {error}"))?;

    let status = response.status();
    if !status.is_success() {
        let hint = match status.as_u16() {
            401 | 403 => match provider {
                ProviderId::Claude => {
                    " — this endpoint requires a Claude subscription OAuth token; API keys do not work"
                }
                ProviderId::Codex => {
                    " — this endpoint requires a ChatGPT OAuth access token; API keys do not work"
                }
                _ => " — reconnect with a fresh access token",
            },
            404 => " — the usage endpoint was not found; the provider API may have changed",
            429 => " — rate limited; try again shortly",
            _ => "",
        };
        return Err(format!("{display} returned HTTP {status}{hint}."));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|_| format!("{display} returned a response that was not valid JSON."))?;

    let windows = match provider {
        ProviderId::Kimi => collect_kimi_windows(&body),
        ProviderId::Codex | ProviderId::Claude | ProviderId::Zai => collect_windows(&body, 0),
    };
    let plan = pick_plan(&body);
    let balance = pick_balance(&body);

    if windows.is_empty() && plan.is_none() && balance.is_none() {
        return Err(format!(
            "Could not read usage data from the {display} response; the provider API may have changed."
        ));
    }

    let mut usage = ProviderUsage::shell(provider, "connected");
    usage.plan = plan;
    usage.windows = windows;
    usage.balance = balance;
    usage.updated_at = Some(now_epoch_ms());
    Ok(usage)
}

// ---------------------------------------------------------------------------
// Defensive payload normalization
// ---------------------------------------------------------------------------

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn value_f64(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| {
        value
            .as_str()
            .and_then(|raw| raw.trim().parse::<f64>().ok())
    })
}

fn value_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn pick_f64(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(value_f64))
}

fn pick_value<'a>(object: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| object.get(*key))
}

fn clamp_percent(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    value.clamp(0.0, 100.0)
}

/// Some payloads report a 0..1 fraction instead of a 0..100 percentage.
fn normalize_fraction_or_percent(value: f64) -> f64 {
    clamp_percent(if (0.0..=1.0).contains(&value) {
        value * 100.0
    } else {
        value
    })
}

/// Normalize epoch seconds, epoch milliseconds, or ISO-8601 strings to epoch ms.
fn epoch_ms(value: &Value) -> Option<i64> {
    if let Some(number) = value_f64(value) {
        if !number.is_finite() || number <= 0.0 {
            return None;
        }
        // Anything below 1e12 is epoch seconds (covers dates before ~Mar 5138).
        return Some(if number < 1e12 {
            (number * 1_000.0) as i64
        } else {
            number as i64
        });
    }
    let text = value.as_str()?.trim();
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|parsed| parsed.timestamp_millis())
}

fn duration_label(seconds: f64) -> String {
    let seconds = seconds as i64;
    // Match the canonical window labels used elsewhere ("Weekly", not "7-day").
    if seconds == 604_800 {
        "Weekly".to_string()
    } else if seconds >= 86_400 && seconds % 86_400 == 0 {
        format!("{}-day", seconds / 86_400)
    } else if seconds >= 3_600 && seconds % 3_600 == 0 {
        format!("{}-hour", seconds / 3_600)
    } else if seconds >= 60 && seconds % 60 == 0 {
        format!("{}-minute", seconds / 60)
    } else {
        format!("{seconds}-second")
    }
}

fn prettify_label(raw: &str) -> String {
    let normalized = raw.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    match normalized.as_str() {
        "five_hour" | "5_hour" | "primary_window" | "5h" | "fivehour" => "5-hour".to_string(),
        // Z.ai's official client labels TOKENS_LIMIT as the 5-hour window.
        "tokens_limit" | "token_limit" => "5-hour".to_string(),
        // …and TIME_LIMIT as the monthly MCP window.
        "time_limit" => "Monthly (MCP)".to_string(),
        "seven_day" | "secondary_window" | "weekly" | "week" | "sevenday" => "Weekly".to_string(),
        "seven_day_opus" => "Weekly (Opus)".to_string(),
        "seven_day_sonnet" => "Weekly (Sonnet)".to_string(),
        "seven_day_oauth_apps" => "Weekly (OAuth apps)".to_string(),
        "seven_day_overage_included" => "Weekly (overage)".to_string(),
        "daily" | "day" => "Daily".to_string(),
        "monthly" | "month" => "Monthly".to_string(),
        other => {
            let mut label = String::with_capacity(other.len());
            for (index, part) in other.split('_').enumerate() {
                if index > 0 {
                    label.push(' ');
                }
                let mut chars = part.chars();
                if let Some(first) = chars.next() {
                    label.extend(first.to_uppercase());
                    label.push_str(chars.as_str());
                }
            }
            if label.is_empty() {
                "Usage".to_string()
            } else {
                label
            }
        }
    }
}

const PERCENT_KEYS: &[&str] = &[
    "usedPercent",
    "used_percent",
    "usagePercent",
    "usage_percent",
    "percentUsed",
    "percent",
    "percentage",
];
const FRACTION_KEYS: &[&str] = &["utilization", "usageFraction", "used_fraction", "fraction"];
const RESET_KEYS: &[&str] = &[
    "resetsAt",
    "resets_at",
    "resetAt",
    "reset_at",
    "reset_time",
    "resetTime",
    "nextResetTime",
    "next_reset_time",
    "reset",
];
/// Relative reset hints, in seconds from now (Kimi reset_in/ttl, Codex reset_after_seconds).
const RESET_IN_KEYS: &[&str] = &["reset_in", "resetIn", "reset_after_seconds", "ttl"];
const RATIO_PAIRS: &[(&str, &str)] = &[
    ("used", "limit"),
    ("currentValue", "usage"),
    ("current_value", "usage"),
    ("used", "total"),
    ("consumed", "quota"),
    ("tokensUsed", "tokensLimit"),
    ("used_tokens", "token_limit"),
];

/// Kimi wraps rows as `{detail: {...}, window: {duration, timeUnit}}`.
fn kimi_detail_label(object: &serde_json::Map<String, Value>) -> Option<String> {
    let window = object.get("window")?.as_object()?;
    let duration = pick_f64(window, &["duration"])?;
    let unit = pick_value(window, &["timeUnit", "time_unit", "unit"])
        .and_then(value_string)
        .unwrap_or_else(|| "hour".to_string());
    let unit = unit.to_ascii_lowercase();
    let unit = unit.strip_prefix("time_unit_").unwrap_or(&unit);
    let unit = unit.trim_end_matches('s');
    Some(format!("{}-{unit}", duration as i64))
}

fn parse_window_item(value: &Value, fallback_label: Option<&str>) -> Option<ProviderUsageWindow> {
    let object = value.as_object()?;

    // Unwrap Kimi-style `{detail, window}` rows.
    if let Some(detail) = object.get("detail").filter(|detail| detail.is_object()) {
        let label = kimi_detail_label(object).or_else(|| fallback_label.map(prettify_label));
        let mut parsed = parse_window_item(detail, label.as_deref())?;
        if parsed.label == "Usage" {
            if let Some(label) = label {
                parsed.label = label;
            }
        }
        return Some(parsed);
    }

    let used_percent = pick_f64(object, PERCENT_KEYS)
        .map(clamp_percent)
        .or_else(|| pick_f64(object, FRACTION_KEYS).map(normalize_fraction_or_percent))
        .or_else(|| {
            RATIO_PAIRS.iter().find_map(|(used_key, limit_key)| {
                let used = pick_f64(object, &[*used_key])?;
                let limit = pick_f64(object, &[*limit_key])?;
                (limit > 0.0).then(|| clamp_percent(used / limit * 100.0))
            })
        })
        // Kimi reports `limit` + `remaining` without `used`.
        .or_else(|| {
            let limit = pick_f64(object, &["limit"])?;
            let remaining = pick_f64(object, &["remaining"])?;
            (limit > 0.0).then(|| clamp_percent((limit - remaining) / limit * 100.0))
        })?;

    let resets_at = pick_value(object, RESET_KEYS)
        .and_then(epoch_ms)
        .or_else(|| {
            pick_f64(object, RESET_IN_KEYS)
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                .map(|seconds| now_epoch_ms() + (seconds * 1_000.0) as i64)
        });

    let label = pick_value(object, &["label", "name", "title", "default"])
        .and_then(value_string)
        .map(|text| prettify_label(&text))
        .or_else(|| {
            pick_f64(
                object,
                &[
                    "limit_window_seconds",
                    "limitWindowSeconds",
                    "windowSeconds",
                ],
            )
            .map(duration_label)
        })
        .or_else(|| {
            pick_value(object, &["type", "window", "kind"])
                .and_then(value_string)
                .map(|text| prettify_label(&text))
        })
        .or_else(|| fallback_label.map(prettify_label))
        .unwrap_or_else(|| "Usage".to_string());

    Some(ProviderUsageWindow {
        label,
        used_percent,
        resets_at,
    })
}

const ARRAY_KEYS: &[&str] = &[
    "usages",
    "windows",
    "limits",
    "quotas",
    "entries",
    "rateLimits",
    "rate_limits",
    "additional_rate_limits",
];
const CONTAINER_KEYS: &[&str] = &[
    "data",
    "rate_limit",
    "rateLimit",
    "result",
    "quota",
    "usage",
    "limits",
];

fn collect_kimi_windows(value: &Value) -> Vec<ProviderUsageWindow> {
    let Some(root) = value.as_object() else {
        return Vec::new();
    };
    let mut windows: Vec<_> = root
        .get("limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| parse_window_item(item, None))
        .collect();

    if !windows.iter().any(|window| window.label == "Weekly") {
        if let Some(mut weekly) = root
            .get("usage")
            .and_then(|usage| parse_window_item(usage, Some("Weekly")))
        {
            weekly.label = "Weekly".to_string();
            windows.push(weekly);
        }
    }

    if windows.is_empty() {
        collect_windows(value, 0)
    } else {
        windows
    }
}

/// Depth-first search for usage windows. Handles both arrays of window items
/// (Kimi, Z.ai) and keyed window objects (Codex primary/secondary, Claude
/// five_hour/seven_day) across nesting variants.
fn collect_windows(value: &Value, depth: u8) -> Vec<ProviderUsageWindow> {
    if depth > MAX_PARSE_DEPTH {
        return Vec::new();
    }
    let Some(object) = value.as_object() else {
        return Vec::new();
    };

    for key in ARRAY_KEYS {
        if let Some(items) = object.get(*key).and_then(Value::as_array) {
            let windows: Vec<_> = items
                .iter()
                .filter_map(|item| parse_window_item(item, None))
                .collect();
            if !windows.is_empty() {
                return windows;
            }
        }
    }

    let keyed: Vec<_> = object
        .iter()
        .filter(|(_, child)| child.is_object())
        .filter_map(|(key, child)| parse_window_item(child, Some(key)))
        .collect();
    if !keyed.is_empty() {
        return keyed;
    }

    for key in CONTAINER_KEYS {
        if let Some(child) = object.get(*key).filter(|child| child.is_object()) {
            let windows = collect_windows(child, depth + 1);
            if !windows.is_empty() {
                return windows;
            }
        }
    }

    Vec::new()
}

fn pick_plan(root: &Value) -> Option<String> {
    const PLAN_KEYS: &[&str] = &[
        "plan",
        "planName",
        "plan_name",
        "planType",
        "plan_type",
        "tier",
        "subscriptionType",
        "subscription_type",
    ];
    let search = |object: &serde_json::Map<String, Value>| {
        PLAN_KEYS
            .iter()
            .find_map(|key| object.get(*key).and_then(value_string))
    };
    root.as_object()
        .and_then(search)
        .or_else(|| root.get("data")?.as_object().and_then(search))
        .or_else(|| root.get("subscription")?.as_object().and_then(search))
}

fn pick_balance(root: &Value) -> Option<String> {
    let search = |object: &serde_json::Map<String, Value>| -> Option<String> {
        if let Some(balance) = object.get("balance").and_then(value_string) {
            return Some(balance);
        }
        if let Some(credits) = object.get("credits") {
            if let Some(credits_obj) = credits.as_object() {
                if credits_obj.get("unlimited").and_then(Value::as_bool) == Some(true) {
                    return Some("Unlimited".to_string());
                }
                if let Some(balance) = credits_obj.get("balance").and_then(value_string) {
                    return Some(balance);
                }
            } else if let Some(text) = value_string(credits) {
                return Some(text);
            }
        }
        object
            .get("remaining")
            .and_then(value_string)
            .or_else(|| object.get("remainingCredits").and_then(value_string))
    };
    root.as_object()
        .and_then(search)
        .or_else(|| root.get("data")?.as_object().and_then(search))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn window_percent(windows: &[ProviderUsageWindow], label: &str) -> Option<f64> {
        windows
            .iter()
            .find(|window| window.label == label)
            .map(|window| window.used_percent)
    }

    #[test]
    fn omp_auth_fallback_uses_only_matching_named_profiles() {
        assert_eq!(
            ProviderId::Kimi.omp_auth_source(),
            Some(("--profile=kimi-k3", "kimi-code"))
        );
        assert_eq!(
            ProviderId::Codex.omp_auth_source(),
            Some(("--profile=gpt-only", "openai-codex"))
        );
        assert_eq!(ProviderId::Claude.omp_auth_source(), None);
        assert_eq!(ProviderId::Zai.omp_auth_source(), None);
    }

    #[test]
    fn codex_account_id_is_read_from_oauth_access_token_in_memory() {
        let claims = serde_json::to_vec(&json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "account-test"
            }
        }))
        .unwrap();
        let payload = URL_SAFE_NO_PAD.encode(claims);
        let access_token = format!("header.{payload}.signature");
        assert_eq!(
            codex_account_id(&access_token).as_deref(),
            Some("account-test")
        );
        assert_eq!(codex_account_id("not-a-jwt"), None);
    }

    #[test]
    fn codex_primary_secondary_windows() {
        let body = json!({
            "plan_type": "plus",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {
                    "used_percent": 21,
                    "limit_window_seconds": 18000,
                    "reset_after_seconds": 3600,
                    "reset_at": 1_761_193_452
                },
                "secondary_window": {
                    "used_percent": 5,
                    "limit_window_seconds": 604800,
                    "reset_after_seconds": 499045,
                    "reset_at": 1_761_683_912
                }
            },
            "credits": { "has_credits": true, "unlimited": false, "balance": 12.5 }
        });
        let windows = collect_windows(&body, 0);
        assert_eq!(windows.len(), 2);
        assert_eq!(window_percent(&windows, "5-hour"), Some(21.0));
        assert_eq!(window_percent(&windows, "Weekly"), Some(5.0));
        // reset_at in epoch seconds is converted to epoch milliseconds.
        assert_eq!(windows[0].resets_at, Some(1_761_193_452_000));
        assert_eq!(pick_plan(&body).as_deref(), Some("plus"));
        assert_eq!(pick_balance(&body).as_deref(), Some("12.5"));
    }

    #[test]
    fn codex_primary_window_uses_reported_weekly_duration() {
        let body = json!({
            "rate_limit": {
                "primary_window": {
                    "used_percent": 97,
                    "limit_window_seconds": 604800
                },
                "secondary_window": null
            }
        });
        let windows = collect_windows(&body, 0);
        assert_eq!(windows.len(), 1);
        assert_eq!(window_percent(&windows, "Weekly"), Some(97.0));
    }

    #[test]
    fn claude_utilization_and_iso_resets() {
        let body = json!({
            "five_hour": { "utilization": 6.0, "resets_at": "2025-11-27T18:00:00+00:00" },
            "seven_day": { "utilization": 0.5, "resets_at": 1_764_500_000 },
            "seven_day_opus": { "utilization": null, "resets_at": null },
            "extra_usage": { "is_enabled": false, "utilization": null }
        });
        let windows = collect_windows(&body, 0);
        assert_eq!(windows.len(), 2);
        assert_eq!(window_percent(&windows, "5-hour"), Some(6.0));
        // 0..1 fractions are scaled to 0..100.
        assert_eq!(window_percent(&windows, "Weekly"), Some(50.0));
        // Numeric resets are epoch seconds -> ms.
        assert_eq!(windows[1].resets_at, Some(1_764_500_000_000));
    }

    #[test]
    fn zai_limits_array() {
        let body = json!({
            "code": 200,
            "success": true,
            "data": {
                "limits": [
                    {
                        "type": "TIME_LIMIT",
                        "percentage": 12,
                        "currentValue": 6,
                        "usage": 50
                    },
                    {
                        "type": "TOKENS_LIMIT",
                        "percentage": 6
                    }
                ]
            }
        });
        let windows = collect_windows(&body, 0);
        assert_eq!(windows.len(), 2);
        // Z.ai official client: TIME_LIMIT = monthly MCP, TOKENS_LIMIT = 5-hour.
        assert_eq!(window_percent(&windows, "Monthly (MCP)"), Some(12.0));
        assert_eq!(window_percent(&windows, "5-hour"), Some(6.0));
        // No reset timestamp is confirmed for this route; none is invented.
        assert!(windows.iter().all(|window| window.resets_at.is_none()));
    }

    #[test]
    fn kimi_detail_window_rows() {
        let body = json!({
            "plan": "kimi-for-coding",
            "limits": [
                {
                    "detail": { "limit": 100, "remaining": 25, "name": "5-hour", "reset_in": 1800 },
                    "window": { "duration": 5, "timeUnit": "hour" }
                },
                {
                    "detail": { "limit": 500, "used": 50, "title": "weekly", "reset_at": "2025-12-01T00:00:00Z" },
                    "window": { "duration": 7, "timeUnit": "day" }
                }
            ]
        });
        let windows = collect_windows(&body, 0);
        assert_eq!(windows.len(), 2);
        // used = limit - remaining -> 75%.
        assert_eq!(window_percent(&windows, "5-hour"), Some(75.0));
        assert_eq!(window_percent(&windows, "Weekly"), Some(10.0));
        assert!(windows[0].resets_at.is_some());
        assert_eq!(windows[1].resets_at, Some(1_764_547_200_000));
        assert_eq!(pick_plan(&body).as_deref(), Some("kimi-for-coding"));
    }

    #[test]
    fn kimi_root_usage_is_weekly_alongside_five_hour_limit() {
        let body = json!({
            "usage": {
                "limit": 500,
                "remaining": 400,
                "reset_at": "2025-12-08T00:00:00Z"
            },
            "limits": [
                {
                    "detail": {
                        "limit": 100,
                        "remaining": 25,
                        "reset_in": 1800
                    },
                    "window": {
                        "duration": 5,
                        "timeUnit": "TIME_UNIT_HOUR"
                    }
                }
            ]
        });
        let windows = collect_kimi_windows(&body);
        assert_eq!(windows.len(), 2);
        assert_eq!(window_percent(&windows, "5-hour"), Some(75.0));
        assert_eq!(window_percent(&windows, "Weekly"), Some(20.0));
        assert_eq!(
            windows
                .iter()
                .find(|window| window.label == "Weekly")
                .and_then(|window| window.resets_at),
            Some(1_765_152_000_000)
        );
    }

    #[test]
    fn kimi_generic_usages_array() {
        let body = json!({
            "data": {
                "usages": [
                    { "label": "5-hour", "usedPercent": 120.4, "resetsAt": "2025-12-01T00:00:00Z" },
                    { "label": "weekly", "usedPercent": "12", "resetAt": 1_764_000_000 }
                ]
            }
        });
        let windows = collect_windows(&body, 0);
        assert_eq!(windows.len(), 2);
        // Out-of-range percentages are clamped to 0..100.
        assert_eq!(window_percent(&windows, "5-hour"), Some(100.0));
        assert_eq!(window_percent(&windows, "Weekly"), Some(12.0));
        assert_eq!(windows[1].resets_at, Some(1_764_000_000_000));
    }

    #[test]
    fn epoch_ms_handles_seconds_millis_and_iso() {
        assert_eq!(epoch_ms(&json!(1_700_000_000)), Some(1_700_000_000_000));
        assert_eq!(
            epoch_ms(&json!(1_700_000_000_000i64)),
            Some(1_700_000_000_000)
        );
        assert_eq!(
            epoch_ms(&json!("2023-11-14T22:13:20Z")),
            Some(1_700_000_000_000)
        );
        assert_eq!(epoch_ms(&json!("1700000000")), Some(1_700_000_000_000));
        assert_eq!(epoch_ms(&json!(null)), None);
    }
}
