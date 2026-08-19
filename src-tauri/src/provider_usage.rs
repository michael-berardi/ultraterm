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
pub enum ProviderId {
    #[serde(rename = "kimi")]
    Kimi,
    #[serde(rename = "codex")]
    Codex,
    /// Secondary ChatGPT account used as the runtime fallback when the primary
    /// Codex account exhausts its quota. Shares the Codex usage endpoint but
    /// stores its own Keychain credential and renders its own sidebar dial.
    #[serde(rename = "codex-fallback")]
    CodexFallback,
    #[serde(rename = "claude")]
    Claude,
    #[serde(rename = "zai")]
    Zai,
}

impl ProviderId {
    const ALL: [ProviderId; 5] = [
        ProviderId::Kimi,
        ProviderId::Codex,
        ProviderId::CodexFallback,
        ProviderId::Claude,
        ProviderId::Zai,
    ];

    fn display_name(self) -> &'static str {
        match self {
            ProviderId::Kimi => "Kimi",
            ProviderId::Codex => "Codex",
            ProviderId::CodexFallback => "Codex Fallback",
            ProviderId::Claude => "Claude",
            ProviderId::Zai => "Z.ai",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            ProviderId::Kimi => "https://api.kimi.com/coding/v1/usages",
            ProviderId::Codex | ProviderId::CodexFallback => {
                "https://chatgpt.com/backend-api/wham/usage"
            }
            ProviderId::Claude => "https://api.anthropic.com/api/oauth/usage",
            ProviderId::Zai => "https://api.z.ai/api/monitor/usage/quota/limit",
        }
    }

    fn keychain_account(self) -> &'static str {
        match self {
            ProviderId::Kimi => "provider-usage-kimi",
            ProviderId::Codex => "provider-usage-codex",
            ProviderId::CodexFallback => "provider-usage-codex-fallback",
            ProviderId::Claude => "provider-usage-claude",
            ProviderId::Zai => "provider-usage-zai",
        }
    }

    fn omp_auth_source(self) -> Option<(&'static str, &'static str)> {
        match self {
            ProviderId::Kimi => Some(("--profile=kimi-k3", "kimi-code")),
            ProviderId::Codex => Some(("--profile=gpt-only", "openai-codex")),
            // The fallback account is never read back out of OMP: it is the
            // secondary credential, explicitly supplied by the user.
            ProviderId::CodexFallback | ProviderId::Claude | ProviderId::Zai => None,
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
    /// OAuth refresh token, expiry (epoch ms), and account email. Required for
    /// the Codex fallback account so OMP can keep using (and refreshing) the
    /// credential long after the pasted access token expires.
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredCredential {
    access_token: String,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    email: Option<String>,
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
    let clean = |value: Option<String>| {
        value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let refresh_token = clean(input.refresh_token);
    if input.provider == ProviderId::CodexFallback && refresh_token.is_none() {
        return Err(
            "The fallback account needs its OAuth refresh token — paste the full contents of its ~/.codex/auth.json."
                .to_string(),
        );
    }
    let credential = StoredCredential {
        access_token,
        account_id: clean(input.account_id),
        refresh_token,
        expires_at: input.expires_at,
        email: clean(input.email),
    };
    let usage = fetch_remote(input.provider, &credential).await?;
    write_credential(input.provider, &credential)?;
    if input.provider == ProviderId::CodexFallback {
        if let Err(error) = sync_codex_fallback_to_omp(&credential).await {
            // Roll the Keychain write back so the card never claims a fallback
            // that live terminals cannot actually rotate into.
            let _ = delete_credential(input.provider);
            return Err(error);
        }
    }
    Ok(usage)
}

#[tauri::command]
pub async fn remove_provider_credential(provider: ProviderId) -> Result<(), String> {
    // Read before deleting: OMP-side cleanup keys off the account identity.
    let credential = read_credential(provider)?;
    delete_credential(provider)?;
    if provider == ProviderId::CodexFallback {
        if let Some(credential) = credential {
            remove_codex_fallback_from_omp(&credential)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Codex fallback: OMP runtime sync
// ---------------------------------------------------------------------------
//
// The sidebar dial only tracks the fallback account; the actual failover runs
// through OMP. OMP stores one row per OAuth account in each profile's
// `agent.db` and keeps sessions on the first (primary) credential until it
// hits a usage-limit error, then rotates to a sibling (verified with
// `omp dry-balance`: 100% of sampled sessions resolve to the primary while it
// is healthy). Importing the fallback account as a second `openai-codex`
// credential therefore yields exactly "use my account until it runs out, then
// hers" semantics without touching ~/.codex/auth.json.

/// Launch profiles whose OMP config routes models through openai-codex.
/// deepseek-v4-flash and local never call Codex, so they are left alone.
const OMP_CODEX_PROFILES: &[&str] = &["lds", "gpt-only", "kimi-k3"];

/// Decoded payload of a JWT, shared by the account-id and expiry helpers.
fn jwt_payload(token: &str) -> Option<Value> {
    let encoded_payload = token.split('.').nth(1)?;
    let payload = URL_SAFE_NO_PAD
        .decode(encoded_payload)
        .or_else(|_| URL_SAFE.decode(encoded_payload))
        .ok()?;
    serde_json::from_slice(&payload).ok()
}

/// Access-token expiry in epoch ms, derived from the JWT `exp` claim. Falls
/// back to "already expired" so OMP refreshes the token before first use
/// instead of trusting a stale access token.
fn codex_fallback_expires_at(credential: &StoredCredential) -> i64 {
    if let Some(expires_at) = credential.expires_at {
        return expires_at;
    }
    jwt_payload(&credential.access_token)
        .and_then(|claims| claims.get("exp").and_then(Value::as_i64).map(|exp| exp * 1000))
        .unwrap_or_else(now_epoch_ms)
}

fn codex_fallback_import_payload(credential: &StoredCredential) -> Result<String, String> {
    let refresh_token = credential.refresh_token.as_deref().ok_or(
        "The fallback account is missing its refresh token; save it again from Settings.",
    )?;
    let expires_at = chrono::DateTime::from_timestamp_millis(codex_fallback_expires_at(credential))
        .map(|timestamp| timestamp.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
    let mut payload = serde_json::json!({
        // `type: "codex"` is how `omp auth-broker import` recognizes a ChatGPT
        // OAuth credential for its openai-codex provider.
        "type": "codex",
        "access_token": credential.access_token,
        "refresh_token": refresh_token,
        "expired": expires_at,
    });
    if let Some(account_id) = credential.account_id.as_deref() {
        payload["account_id"] = Value::String(account_id.to_string());
    }
    if let Some(email) = credential.email.as_deref() {
        payload["email"] = Value::String(email.to_string());
    }
    serde_json::to_string(&payload)
        .map_err(|error| format!("Could not serialize the fallback credential: {error}"))
}

fn omp_profile_agent_db(home: &std::path::Path, profile: &str) -> std::path::PathBuf {
    home.join(".omp")
        .join("profiles")
        .join(profile)
        .join("agent")
        .join("agent.db")
}

/// Import the fallback credential into every Codex-using OMP profile so live
/// terminals rotate onto it when the primary account hits its usage limit.
/// Import is idempotent per account identity, so re-saving updates in place.
async fn sync_codex_fallback_to_omp(credential: &StoredCredential) -> Result<(), String> {
    let omp = crate::resolve_optional_executable("OMP_BIN", "omp", &[])
        .map_err(|error| format!("Could not locate the omp binary: {error}"))?
        .ok_or(
            "The omp binary was not found, so the fallback account could not be registered with OMP."
                .to_string(),
        )?;
    let payload = codex_fallback_import_payload(credential)?;

    // Secrets cross the process boundary through a 0600 temp file, never the
    // command line (argv is visible to every process on the machine).
    let temp_path = std::env::temp_dir().join(format!(
        "ultraterm-codex-fallback-{}.json",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&temp_path, &payload)
        .map_err(|error| format!("Could not stage the fallback credential: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o600));
    }

    let mut failures = Vec::new();
    for profile in OMP_CODEX_PROFILES {
        let mut command = tokio::process::Command::new(&omp);
        command
            .args([
                &format!("--profile={profile}"),
                "auth-broker",
                "import",
            ])
            .arg(&temp_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let result = tokio::time::timeout(REQUEST_TIMEOUT, command.output()).await;
        match result {
            Ok(Ok(output)) if output.status.success() => {}
            Ok(Ok(output)) => {
                let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                failures.push(format!("{profile}: {}", if detail.is_empty() {
                    format!("omp exited with {}", output.status)
                } else {
                    detail
                }));
            }
            Ok(Err(error)) => failures.push(format!("{profile}: {error}")),
            Err(_) => failures.push(format!("{profile}: omp import timed out")),
        }
    }
    let _ = std::fs::remove_file(&temp_path);

    if failures.is_empty() {
        eprintln!(
            "[ultraterm] codex fallback account synced into OMP profiles: {}",
            OMP_CODEX_PROFILES.join(", ")
        );
        Ok(())
    } else {
        Err(format!(
            "The fallback account could not be registered with OMP ({}) — live terminals will not fail over to it.",
            failures.join("; ")
        ))
    }
}

/// Remove the fallback credential from each OMP profile's auth store. OMP
/// exposes no non-interactive per-account logout, so this deletes the row by
/// identity key directly; the store's own triggers bump the change revision so
/// running OMP instances pick the removal up.
fn remove_codex_fallback_from_omp(credential: &StoredCredential) -> Result<(), String> {
    let Some(email) = credential.email.as_deref() else {
        // Without an email there is no stable identity to delete by; the
        // credential was likely never synced into OMP either.
        eprintln!("[ultraterm] codex fallback credential has no email; skipping OMP cleanup");
        return Ok(());
    };
    let identity_key = format!("email:{email}");
    let home = crate::home_dir()?;
    let mut failures = Vec::new();
    for profile in OMP_CODEX_PROFILES {
        let db_path = omp_profile_agent_db(&home, profile);
        if !db_path.exists() {
            continue;
        }
        let outcome = rusqlite::Connection::open(&db_path).and_then(|connection| {
            connection.busy_timeout(Duration::from_secs(5))?;
            connection.execute(
                "DELETE FROM auth_credential_blocks WHERE credential_id IN (
                    SELECT id FROM auth_credentials
                    WHERE provider = 'openai-codex' AND identity_key = ?1
                )",
                [&identity_key],
            )?;
            connection.execute(
                "DELETE FROM auth_credentials WHERE provider = 'openai-codex' AND identity_key = ?1",
                [&identity_key],
            )
        });
        if let Err(error) = outcome {
            failures.push(format!("{profile}: {error}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "The fallback account was disconnected here, but OMP cleanup failed ({}) — remove it from OMP manually.",
            failures.join("; ")
        ))
    }
}

// ---------------------------------------------------------------------------
// Remote fetching
// ---------------------------------------------------------------------------

fn codex_account_id(access_token: &str) -> Option<String> {
    let claims = jwt_payload(access_token)?;
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
    // Codex only: pin the FIRST stored account. Without this, once a fallback
    // account exists omp rotates round-robin and the primary dial would
    // randomly show the fallback account's quota under the primary's name.
    // (Other providers must not get the flag — e.g. kimi-code rejects
    // `--account 1` with "OAuth access unavailable".)
    let args: Vec<&str> = match provider {
        ProviderId::Codex => vec![profile_argument, "token", token_provider, "--account", "1"],
        _ => vec![profile_argument, "token", token_provider],
    };
    command
        .args(args)
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
        ProviderId::Kimi | ProviderId::CodexFallback | ProviderId::Claude | ProviderId::Zai => None,
    };
    Some(StoredCredential {
        access_token,
        account_id,
        refresh_token: None,
        expires_at: None,
        email: None,
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
        ProviderId::Kimi | ProviderId::Codex | ProviderId::CodexFallback | ProviderId::Claude => {
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
        ProviderId::Codex | ProviderId::CodexFallback => {
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
                ProviderId::Codex | ProviderId::CodexFallback => {
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
        ProviderId::Codex | ProviderId::CodexFallback | ProviderId::Claude | ProviderId::Zai => {
            collect_windows(&body, 0)
        }
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
    let seconds_per_unit = match unit {
        "second" => 1.0,
        "minute" => 60.0,
        "hour" => 3_600.0,
        "day" => 86_400.0,
        "week" => 604_800.0,
        _ => return Some(format!("{}-{unit}", duration as i64)),
    };
    Some(duration_label(duration * seconds_per_unit))
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
        // The fallback account is user-supplied only; it is never read back
        // out of OMP, where the same identity lives as a sibling credential.
        assert_eq!(ProviderId::CodexFallback.omp_auth_source(), None);
    }

    #[test]
    fn codex_fallback_serde_and_keychain_identity() {
        assert_eq!(
            serde_json::to_string(&ProviderId::CodexFallback).unwrap(),
            "\"codex-fallback\""
        );
        assert_eq!(
            serde_json::from_str::<ProviderId>("\"codex-fallback\"").unwrap(),
            ProviderId::CodexFallback
        );
        // Distinct Keychain slots keep the primary and fallback credentials
        // from clobbering each other.
        assert_ne!(
            ProviderId::Codex.keychain_account(),
            ProviderId::CodexFallback.keychain_account()
        );
    }

    #[test]
    fn codex_fallback_expiry_prefers_stored_then_jwt_then_now() {
        let base = StoredCredential {
            access_token: "not-a-jwt".to_string(),
            account_id: None,
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(1_800_000_000_000),
            email: None,
        };
        assert_eq!(codex_fallback_expires_at(&base), 1_800_000_000_000);

        let claims = serde_json::to_vec(&json!({ "exp": 1_800_000_000_i64 })).unwrap();
        let payload = URL_SAFE_NO_PAD.encode(claims);
        let jwt = StoredCredential {
            access_token: format!("header.{payload}.signature"),
            expires_at: None,
            ..base.clone()
        };
        assert_eq!(codex_fallback_expires_at(&jwt), 1_800_000_000_000);

        let no_expiry = StoredCredential {
            expires_at: None,
            ..base
        };
        // No stored expiry and no JWT claim → "already expired" so OMP
        // refreshes via the refresh token before first use.
        assert!(codex_fallback_expires_at(&no_expiry) <= now_epoch_ms());
    }

    #[test]
    fn codex_fallback_import_payload_matches_omp_importer_contract() {
        let credential = StoredCredential {
            access_token: "access".to_string(),
            account_id: Some("account-1".to_string()),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(1_800_000_000_000),
            email: Some("wife@example.com".to_string()),
        };
        let payload: Value =
            serde_json::from_str(&codex_fallback_import_payload(&credential).unwrap()).unwrap();
        // `type: "codex"` maps to omp's openai-codex provider; `expired` must
        // be an ISO-8601 string the importer can Date.parse.
        assert_eq!(payload["type"], "codex");
        assert_eq!(payload["access_token"], "access");
        assert_eq!(payload["refresh_token"], "refresh");
        assert_eq!(
            chrono::DateTime::parse_from_rfc3339(payload["expired"].as_str().unwrap())
                .unwrap()
                .timestamp_millis(),
            1_800_000_000_000
        );
        assert_eq!(payload["account_id"], "account-1");
        assert_eq!(payload["email"], "wife@example.com");

        let no_refresh = StoredCredential {
            refresh_token: None,
            ..credential
        };
        assert!(codex_fallback_import_payload(&no_refresh).is_err());
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
    fn kimi_300_minute_window_is_normalized_to_five_hours_with_reset() {
        let body = json!({
            "limits": [{
                "detail": { "limit": 100, "remaining": 90, "reset_in": 7200 },
                "window": { "duration": 300, "timeUnit": "minute" }
            }]
        });
        let windows = collect_windows(&body, 0);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "5-hour");
        assert_eq!(windows[0].used_percent, 10.0);
        assert!(windows[0].resets_at.is_some());
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
