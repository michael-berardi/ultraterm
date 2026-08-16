use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, NaiveDate, Utc};
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;

const SNAPSHOT_TTL: Duration = Duration::from_secs(3);
const SECONDS_PER_DAY: i64 = 24 * 60 * 60;
const REGISTRY_DIRECTORY: &str = ".ultraterm";
const REGISTRY_FILE: &str = "telemetry-sessions.json";
const TELEMETRY_INDEX_FILE: &str = "telemetry.sqlite3";
const OVERSEER_USAGE_DIRECTORY: &str = ".overseer/usage";
const USAGE_ARTIFACT_VERSION: u64 = 1;
const UNKNOWN_MODEL: &str = "unknown";
const ULTRATERM_SESSION_PREFIX: &str = "ultraterm-matrix-";

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
}

impl TokenCounts {
    fn add(&mut self, other: &Self) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        self.total = self.total.saturating_add(other.total);
    }
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenChannelTelemetry {
    pub subscription: TokenCounts,
    pub paid_api: TokenCounts,
    pub paid_api_cost_usd: f64,
}

#[derive(Clone, Copy, Default)]
struct ModelPrice {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
}

impl ModelPrice {
    fn cost(self, counts: &TokenCounts) -> f64 {
        (counts.input as f64 * self.input
            + counts.output as f64 * self.output
            + counts.cache_read as f64 * self.cache_read
            + counts.cache_write as f64 * self.cache_write)
            / 1_000_000.0
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalTokenTelemetry {
    pub slot: u32,
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub model: Option<String>,
    pub usage: TokenCounts,
    pub active_subagents: usize,
    pub inactive_subagents: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenHistoryModel {
    pub model: String,
    pub usage: TokenCounts,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenHistoryDay {
    pub date: String,
    pub usage: TokenCounts,
    pub models: Vec<TokenHistoryModel>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenTelemetry {
    pub terminals: Vec<TerminalTokenTelemetry>,
    pub today: TokenCounts,
    pub history: Vec<TokenHistoryDay>,
    pub past_24_hours: TokenCounts,
    pub past_7_days: TokenCounts,
    pub all_time: TokenCounts,
    pub today_channels: TokenChannelTelemetry,
    pub past_24_hour_channels: TokenChannelTelemetry,
    pub active_subagents: usize,
    pub inactive_subagents: usize,
    pub parallel_agents: usize,
    pub tracked_sessions: usize,
    pub updated_at: u64,
}

#[derive(Clone)]
struct TimedUsage {
    timestamp: i64,
    model: Option<String>,
    provider: Option<String>,
    counts: TokenCounts,
}

#[derive(Clone, Default)]
struct FileUsage {
    total: TokenCounts,
    timed: Vec<TimedUsage>,
    exited: bool,
    assistant_records: usize,
    model: Option<String>,
    artifact_session_id: Option<String>,
}

#[derive(Clone)]
struct ParsedAssistantRecord {
    identity: Option<String>,
    timestamp: Option<i64>,
    model: Option<String>,
    provider: Option<String>,
    counts: Option<TokenCounts>,
}

struct AggregatedUsage {
    past_24_hours: TokenCounts,
    past_7_days: TokenCounts,
    all_time: TokenCounts,
    today: TokenCounts,
    today_channels: TokenChannelTelemetry,
    past_24_hour_channels: TokenChannelTelemetry,
    history: Vec<TokenHistoryDay>,
}

fn local_date(timestamp: i64) -> Option<NaiveDate> {
    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|timestamp| timestamp.with_timezone(&Local).date_naive())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TokenChannel {
    Subscription,
    PaidApi,
}

fn token_channel(provider: Option<&str>, model: Option<&str>) -> TokenChannel {
    let provider = canonical_provider_name(provider.unwrap_or_default()).to_ascii_lowercase();
    let model = canonical_model_name(model.unwrap_or_default()).to_ascii_lowercase();
    if model.ends_with(":free") {
        return TokenChannel::Subscription;
    }
    let model_provider = model.split_once('/').map(|(prefix, _)| prefix);
    let is_paid_provider = |candidate: &str| {
        matches!(
            candidate,
            "openrouter"
                | "openai"
                | "deepseek"
                | "moonshot"
                | "cerebras"
                | "google"
                | "groq"
                | "nvidia"
                | "together"
                | "fireworks"
                | "xai"
                | "zenmux"
        )
    };
    if is_paid_provider(&provider)
        || model_provider.is_some_and(is_paid_provider)
        || model.contains("deepseek")
    {
        TokenChannel::PaidApi
    } else {
        TokenChannel::Subscription
    }
}

fn canonical_model_name(raw: &str) -> String {
    let model = raw.trim().strip_prefix('~').unwrap_or(raw.trim());
    if let Some(model) = model.strip_prefix("openrouter.ai/") {
        return format!("openrouter/{model}");
    }
    model.to_string()
}

fn canonical_provider_name(raw: &str) -> String {
    let provider = raw.trim();
    if provider.eq_ignore_ascii_case("openrouter.ai") {
        return "openrouter".to_string();
    }
    provider.to_string()
}

fn model_price<'a>(
    pricing: &'a HashMap<String, ModelPrice>,
    model: Option<&str>,
) -> Option<&'a ModelPrice> {
    let model = canonical_model_name(model?);
    pricing
        .get(&model)
        .or_else(|| model.rsplit('/').next().and_then(|name| pricing.get(name)))
}

fn add_channel_usage(
    channels: &mut TokenChannelTelemetry,
    record: &TimedUsage,
    pricing: &HashMap<String, ModelPrice>,
) {
    match token_channel(record.provider.as_deref(), record.model.as_deref()) {
        TokenChannel::Subscription => channels.subscription.add(&record.counts),
        TokenChannel::PaidApi => {
            channels.paid_api.add(&record.counts);
            if let Some(price) = model_price(pricing, record.model.as_deref()) {
                channels.paid_api_cost_usd += price.cost(&record.counts);
            }
        }
    }
}

fn deduplicated_file_usages<'a>(
    file_cache: &'a HashMap<PathBuf, CachedFileUsage>,
) -> Vec<&'a FileUsage> {
    let mut entries: Vec<_> = file_cache.iter().collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    let mut sessions = HashSet::new();
    entries
        .into_iter()
        .filter_map(|(_, cached)| {
            cached
                .usage
                .artifact_session_id
                .as_ref()
                .map_or(Some(&cached.usage), |session_id| {
                    sessions.insert(session_id.clone()).then_some(&cached.usage)
                })
        })
        .collect()
}

fn aggregate_usage<'a>(
    usages: impl Iterator<Item = &'a FileUsage>,
    pricing: &HashMap<String, ModelPrice>,
    now: i64,
) -> AggregatedUsage {
    let past_24_hour_boundary = now.saturating_sub(SECONDS_PER_DAY);
    let past_7_day_boundary = now.saturating_sub(7 * SECONDS_PER_DAY);
    let today_date = local_date(now);
    let mut past_24_hours = TokenCounts::default();
    let mut past_7_days = TokenCounts::default();
    let mut all_time = TokenCounts::default();
    let mut today = TokenCounts::default();
    let mut today_channels = TokenChannelTelemetry::default();
    let mut past_24_hour_channels = TokenChannelTelemetry::default();
    let mut days: BTreeMap<NaiveDate, (TokenCounts, BTreeMap<String, TokenCounts>)> =
        BTreeMap::new();

    for usage in usages {
        all_time.add(&usage.total);
        for record in &usage.timed {
            if record.timestamp >= past_7_day_boundary {
                past_7_days.add(&record.counts);
            }
            if record.timestamp >= past_24_hour_boundary {
                past_24_hours.add(&record.counts);
                add_channel_usage(&mut past_24_hour_channels, record, pricing);
            }
            let Some(date) = local_date(record.timestamp) else {
                continue;
            };
            if Some(date) == today_date {
                today.add(&record.counts);
                add_channel_usage(&mut today_channels, record, pricing);
            }
            let (day_usage, models) = days.entry(date).or_default();
            day_usage.add(&record.counts);
            models
                .entry(
                    record
                        .model
                        .as_deref()
                        .map(canonical_model_name)
                        .unwrap_or_else(|| UNKNOWN_MODEL.to_string()),
                )
                .or_default()
                .add(&record.counts);
        }
    }

    let history = days
        .into_iter()
        .map(|(date, (usage, models))| TokenHistoryDay {
            date: date.format("%Y-%m-%d").to_string(),
            usage,
            models: models
                .into_iter()
                .map(|(model, usage)| TokenHistoryModel { model, usage })
                .collect(),
        })
        .collect();

    AggregatedUsage {
        past_24_hours,
        past_7_days,
        all_time,
        today,
        today_channels,
        past_24_hour_channels,
        history,
    }
}

struct CachedFileUsage {
    length: u64,
    modified: Option<SystemTime>,
    usage: FileUsage,
}

#[derive(Clone, PartialEq, Eq)]
struct PricingSourceFingerprint {
    path: PathBuf,
    modified: Option<SystemTime>,
    length: Option<u64>,
    wal_modified: Option<SystemTime>,
    wal_length: Option<u64>,
}

#[derive(Clone, Copy)]
struct CachedModelPrice {
    price: ModelPrice,
    updated_at: i64,
}

fn insert_freshest_model_price(
    pricing: &mut HashMap<String, CachedModelPrice>,
    model: String,
    candidate: CachedModelPrice,
) {
    let replace = pricing
        .get(&model)
        .is_none_or(|current| candidate.updated_at >= current.updated_at);
    if replace {
        pricing.insert(model, candidate);
    }
}

pub struct TokenTelemetryManager {
    file_cache: HashMap<PathBuf, CachedFileUsage>,
    registered_sessions: HashSet<PathBuf>,
    registry_loaded: bool,
    index_loaded: bool,
    pricing: HashMap<String, ModelPrice>,
    pricing_sources: Vec<PricingSourceFingerprint>,
    snapshot: Option<(Instant, TokenTelemetry)>,
}

impl Default for TokenTelemetryManager {
    fn default() -> Self {
        Self {
            file_cache: HashMap::new(),
            registered_sessions: HashSet::new(),
            registry_loaded: false,
            index_loaded: false,
            pricing: HashMap::new(),
            pricing_sources: Vec::new(),
            snapshot: None,
        }
    }
}

impl TokenTelemetryManager {
    pub fn snapshot(&mut self, home: &Path) -> Result<TokenTelemetry, String> {
        if let Some((created_at, snapshot)) = &self.snapshot {
            if created_at.elapsed() < SNAPSHOT_TTL {
                return Ok(snapshot.clone());
            }
        }

        self.load_registry(home)?;
        self.load_index(home)?;
        let terminal_sessions = current_terminal_sessions(home)?;
        let previous_sessions = self.registered_sessions.clone();
        self.registered_sessions.retain(|path| path.is_file());
        self.registered_sessions.extend(
            terminal_sessions
                .values()
                .filter(|path| path.is_file())
                .cloned(),
        );
        if self.registered_sessions != previous_sessions {
            self.save_registry(home)?;
        }

        let mut files = HashSet::new();
        for session_path in &self.registered_sessions {
            collect_session_files(session_path, &mut files);
        }
        collect_usage_artifacts(home, &mut files);
        self.refresh_files(home, &files)?;
        self.refresh_model_pricing(home);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let aggregate = aggregate_usage(
            deduplicated_file_usages(&self.file_cache).into_iter(),
            &self.pricing,
            now,
        );

        let mut terminals = Vec::with_capacity(crate::MAX_SESSION_SLOT as usize);
        let mut active_agent_paths = HashSet::new();
        let mut completed_agent_paths = HashSet::new();
        for slot in 1..=crate::MAX_SESSION_SLOT {
            let session_path = terminal_sessions.get(&slot);
            let mut usage = TokenCounts::default();
            let mut model = None;
            let mut active_subagents = 0;
            let mut inactive_subagents = 0;

            if let Some(main_path) = session_path {
                let companion_directory = main_path.with_extension("");
                for (path, cached) in &self.file_cache {
                    if path == main_path {
                        model.clone_from(&cached.usage.model);
                    }
                    if path == main_path || path.starts_with(&companion_directory) {
                        usage.add(&cached.usage.total);
                    }
                    if path.starts_with(&companion_directory) && is_counted_subagent(path) {
                        if subagent_is_active(path, &cached.usage) {
                            active_subagents += 1;
                            active_agent_paths.insert(path.clone());
                        } else if subagent_is_complete(path, &cached.usage) {
                            inactive_subagents += 1;
                            completed_agent_paths.insert(path.clone());
                        }
                    }
                }
            }

            terminals.push(TerminalTokenTelemetry {
                slot,
                session_id: session_path.and_then(|path| session_id(path)),
                title: session_path.and_then(|path| session_title(path)),
                model,
                usage,
                active_subagents,
                inactive_subagents,
            });
        }

        let active_subagents = active_agent_paths.len();
        let inactive_subagents = completed_agent_paths.len();
        let updated_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let snapshot = TokenTelemetry {
            terminals,
            today: aggregate.today,
            today_channels: aggregate.today_channels,
            history: aggregate.history,
            past_24_hours: aggregate.past_24_hours,
            past_24_hour_channels: aggregate.past_24_hour_channels,
            past_7_days: aggregate.past_7_days,
            all_time: aggregate.all_time,
            active_subagents,
            inactive_subagents,
            parallel_agents: active_subagents,
            tracked_sessions: self.registered_sessions.len(),
            updated_at,
        };
        self.snapshot = Some((Instant::now(), snapshot.clone()));
        Ok(snapshot)
    }

    fn load_registry(&mut self, home: &Path) -> Result<(), String> {
        if self.registry_loaded {
            return Ok(());
        }
        self.registry_loaded = true;
        let registry_path = registry_path(home);
        let content = match fs::read_to_string(&registry_path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(format!(
                    "Failed to read token telemetry registry {}: {error}",
                    registry_path.display()
                ))
            }
        };
        let paths: Vec<PathBuf> = serde_json::from_str(&content).map_err(|error| {
            format!(
                "Failed to parse token telemetry registry {}: {error}",
                registry_path.display()
            )
        })?;
        self.registered_sessions.extend(paths);
        Ok(())
    }

    fn load_index(&mut self, home: &Path) -> Result<(), String> {
        if self.index_loaded {
            return Ok(());
        }
        self.file_cache = load_file_usage_index(home)?;
        self.index_loaded = true;
        Ok(())
    }

    fn save_registry(&self, home: &Path) -> Result<(), String> {
        let registry_path = registry_path(home);
        if let Some(parent) = registry_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Failed to create telemetry directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        let mut paths: Vec<_> = self.registered_sessions.iter().collect();
        paths.sort();
        let content = serde_json::to_string_pretty(&paths)
            .map_err(|error| format!("Failed to serialize token telemetry registry: {error}"))?;
        fs::write(&registry_path, content).map_err(|error| {
            format!(
                "Failed to write token telemetry registry {}: {error}",
                registry_path.display()
            )
        })
    }

    fn refresh_files(&mut self, home: &Path, files: &HashSet<PathBuf>) -> Result<(), String> {
        let mut changed = Vec::new();
        for path in files {
            let metadata = match fs::metadata(path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            let length = metadata.len();
            let modified = metadata.modified().ok();
            let unchanged = self
                .file_cache
                .get(path)
                .is_some_and(|cached| cached.length == length && cached.modified == modified);
            if unchanged {
                continue;
            }
            let parsed = if is_usage_artifact_path(home, path) {
                parse_usage_artifact(path)
            } else {
                parse_usage_file(path)
            };
            if let Ok(usage) = parsed {
                changed.push((
                    path.clone(),
                    CachedFileUsage {
                        length,
                        modified,
                        usage,
                    },
                ));
            }
        }
        if changed.is_empty() {
            return Ok(());
        }

        persist_file_usage_index(home, &changed)?;
        self.file_cache.extend(changed);
        Ok(())
    }

    fn refresh_model_pricing(&mut self, home: &Path) {
        let sources = model_pricing_sources(home);
        if sources == self.pricing_sources {
            return;
        }

        let mut cached_prices: HashMap<String, CachedModelPrice> = HashMap::new();
        for source in &sources {
            let Ok(source_prices) = load_cached_model_pricing(&source.path) else {
                return;
            };
            for (model, candidate) in source_prices {
                insert_freshest_model_price(&mut cached_prices, model, candidate);
            }
        }

        let mut pricing = fallback_model_pricing();
        pricing.extend(
            cached_prices
                .into_iter()
                .map(|(model, cached)| (model, cached.price)),
        );
        self.pricing = pricing;
        self.pricing_sources = sources;
    }
}

fn model_pricing_paths(home: &Path) -> Vec<PathBuf> {
    let omp_directory = home.join(".omp");
    let mut paths = vec![omp_directory.join("agent").join("models.db")];
    if let Ok(profiles) = fs::read_dir(omp_directory.join("profiles")) {
        for profile in profiles.flatten() {
            let path = profile.path().join("agent").join("models.db");
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths
}

fn model_pricing_sources(home: &Path) -> Vec<PricingSourceFingerprint> {
    model_pricing_paths(home)
        .into_iter()
        .map(|path| {
            let metadata = fs::metadata(&path).ok();
            let mut wal_path = path.as_os_str().to_os_string();
            wal_path.push("-wal");
            let wal_metadata = fs::metadata(PathBuf::from(wal_path)).ok();
            PricingSourceFingerprint {
                path,
                modified: metadata
                    .as_ref()
                    .and_then(|metadata| metadata.modified().ok()),
                length: metadata.map(|metadata| metadata.len()),
                wal_modified: wal_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.modified().ok()),
                wal_length: wal_metadata.map(|metadata| metadata.len()),
            }
        })
        .collect()
}

fn fallback_model_pricing() -> HashMap<String, ModelPrice> {
    [
        (
            "deepseek/deepseek-v4-flash",
            ModelPrice {
                input: 0.14,
                output: 0.28,
                cache_read: 0.028,
                cache_write: 0.0,
            },
        ),
        (
            "deepseek/deepseek-v4-flash-latest",
            ModelPrice {
                input: 0.079996,
                output: 0.252,
                cache_read: 0.0252,
                cache_write: 0.0,
            },
        ),
        (
            "deepseek/deepseek-v4-flash-0731",
            ModelPrice {
                input: 0.09,
                output: 0.18,
                cache_read: 0.018,
                cache_write: 0.0,
            },
        ),
        (
            "deepseek/deepseek-v4-pro",
            ModelPrice {
                input: 0.435,
                output: 0.87,
                cache_read: 0.003625,
                cache_write: 0.0,
            },
        ),
    ]
    .into_iter()
    .flat_map(|(model, price)| {
        let short = model.rsplit('/').next().unwrap_or(model).to_string();
        [(model.to_string(), price), (short, price)]
    })
    .collect()
}

fn load_cached_model_pricing(path: &Path) -> Result<HashMap<String, CachedModelPrice>, String> {
    let mut pricing = HashMap::new();
    if !path.is_file() {
        return Ok(pricing);
    }
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|error| {
            format!(
                "Failed to read OMP model pricing {}: {error}",
                path.display()
            )
        })?;
    let mut statement = connection
        .prepare("SELECT updated_at, models FROM model_cache")
        .map_err(|error| {
            format!(
                "Failed to query OMP model pricing {}: {error}",
                path.display()
            )
        })?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| {
            format!(
                "Failed to load OMP model pricing {}: {error}",
                path.display()
            )
        })?;
    for row in rows {
        let (updated_at, row) = row.map_err(|error| {
            format!(
                "Failed to decode OMP model pricing {}: {error}",
                path.display()
            )
        })?;
        let Ok(Value::Array(models)) = serde_json::from_str::<Value>(&row) else {
            continue;
        };
        for model in models {
            let Some(id) = model.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(cost) = model.get("cost") else {
                continue;
            };
            let cached = CachedModelPrice {
                price: ModelPrice {
                    input: cost
                        .get("input")
                        .and_then(Value::as_f64)
                        .unwrap_or_default(),
                    output: cost
                        .get("output")
                        .and_then(Value::as_f64)
                        .unwrap_or_default(),
                    cache_read: cost
                        .get("cacheRead")
                        .and_then(Value::as_f64)
                        .unwrap_or_default(),
                    cache_write: cost
                        .get("cacheWrite")
                        .and_then(Value::as_f64)
                        .unwrap_or_default(),
                },
                updated_at,
            };
            let id = canonical_model_name(id);
            let short = id.rsplit('/').next().map(str::to_string);
            insert_freshest_model_price(&mut pricing, id, cached);
            if let Some(short) = short {
                insert_freshest_model_price(&mut pricing, short, cached);
            }
        }
    }
    Ok(pricing)
}

fn telemetry_index_path(home: &Path) -> PathBuf {
    home.join(REGISTRY_DIRECTORY).join(TELEMETRY_INDEX_FILE)
}

fn open_telemetry_index(home: &Path) -> Result<Connection, String> {
    let path = telemetry_index_path(home);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create telemetry directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let connection = Connection::open(&path).map_err(|error| {
        format!(
            "Failed to open token telemetry index {}: {error}",
            path.display()
        )
    })?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS telemetry_files (
                 path TEXT PRIMARY KEY,
                 length INTEGER NOT NULL,
                 modified_ns INTEGER,
                 total_input INTEGER NOT NULL,
                 total_output INTEGER NOT NULL,
                 total_cache_read INTEGER NOT NULL,
                 total_cache_write INTEGER NOT NULL,
                 total INTEGER NOT NULL,
                 exited INTEGER NOT NULL,
                 assistant_records INTEGER NOT NULL,
                 model TEXT,
                 source_session_id TEXT
             );
             CREATE TABLE IF NOT EXISTS telemetry_records (
                 file_path TEXT NOT NULL,
                 record_index INTEGER NOT NULL,
                 timestamp INTEGER NOT NULL,
                 model TEXT,
                 provider TEXT,
                 input INTEGER NOT NULL,
                 output INTEGER NOT NULL,
                 cache_read INTEGER NOT NULL,
                 cache_write INTEGER NOT NULL,
                 total INTEGER NOT NULL,
                 PRIMARY KEY (file_path, record_index),
                 FOREIGN KEY (file_path) REFERENCES telemetry_files(path) ON DELETE CASCADE
             );
             CREATE INDEX IF NOT EXISTS telemetry_records_timestamp
                 ON telemetry_records(timestamp);",
        )
        .map_err(|error| {
            format!(
                "Failed to initialize token telemetry index {}: {error}",
                path.display()
            )
        })?;
    let has_provider = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('telemetry_records') WHERE name = 'provider'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| {
            format!(
                "Failed to inspect token telemetry index {}: {error}",
                path.display()
            )
        })?
        > 0;
    if !has_provider {
        connection
            .execute_batch(
                "ALTER TABLE telemetry_records ADD COLUMN provider TEXT;
                 UPDATE telemetry_files SET modified_ns = NULL;",
            )
            .map_err(|error| {
                format!(
                    "Failed to migrate token telemetry index {}: {error}",
                    path.display()
                )
            })?;
    }
    let has_source_session_id = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('telemetry_files') WHERE name = 'source_session_id'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| {
            format!(
                "Failed to inspect token telemetry index {}: {error}",
                path.display()
            )
        })?
        > 0;
    if !has_source_session_id {
        connection
            .execute_batch("ALTER TABLE telemetry_files ADD COLUMN source_session_id TEXT;")
            .map_err(|error| {
                format!(
                    "Failed to migrate token telemetry index {}: {error}",
                    path.display()
                )
            })?;
    }
    connection
        .pragma_update(None, "user_version", 3_i64)
        .map_err(|error| {
            format!(
                "Failed to version token telemetry index {}: {error}",
                path.display()
            )
        })?;
    Ok(connection)
}

fn stored_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn sqlite_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn modified_time_to_nanos(modified: Option<SystemTime>) -> Option<i64> {
    modified?
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_nanos()).ok())
}

fn modified_time_from_nanos(modified_ns: Option<i64>) -> Option<SystemTime> {
    let modified_ns = u64::try_from(modified_ns?).ok()?;
    Some(UNIX_EPOCH + Duration::from_nanos(modified_ns))
}

fn load_file_usage_index(home: &Path) -> Result<HashMap<PathBuf, CachedFileUsage>, String> {
    let path = telemetry_index_path(home);
    let connection = open_telemetry_index(home)?;
    let mut files = HashMap::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT path, length, modified_ns, total_input, total_output,
                        total_cache_read, total_cache_write, total, exited,
                        assistant_records, model, source_session_id
                 FROM telemetry_files",
            )
            .map_err(|error| {
                format!(
                    "Failed to read token telemetry index {}: {error}",
                    path.display()
                )
            })?;
        let rows = statement
            .query_map([], |row| {
                let file_path = PathBuf::from(row.get::<_, String>(0)?);
                let length: i64 = row.get(1)?;
                let modified_ns: Option<i64> = row.get(2)?;
                let exited: i64 = row.get(8)?;
                let assistant_records: i64 = row.get(9)?;
                Ok((
                    file_path,
                    CachedFileUsage {
                        length: stored_u64(length),
                        modified: modified_time_from_nanos(modified_ns),
                        usage: FileUsage {
                            total: TokenCounts {
                                input: stored_u64(row.get(3)?),
                                output: stored_u64(row.get(4)?),
                                cache_read: stored_u64(row.get(5)?),
                                cache_write: stored_u64(row.get(6)?),
                                total: stored_u64(row.get(7)?),
                            },
                            timed: Vec::new(),
                            exited: exited != 0,
                            assistant_records: usize::try_from(assistant_records)
                                .unwrap_or_default(),
                            model: row.get(10)?,
                            artifact_session_id: row.get(11)?,
                        },
                    },
                ))
            })
            .map_err(|error| {
                format!(
                    "Failed to query token telemetry index {}: {error}",
                    path.display()
                )
            })?;
        for row in rows {
            let (file_path, usage) = row.map_err(|error| {
                format!(
                    "Failed to decode token telemetry index {}: {error}",
                    path.display()
                )
            })?;
            files.insert(file_path, usage);
        }
    }

    {
        let mut statement = connection
            .prepare(
                "SELECT file_path, timestamp, model, provider, input, output,
                        cache_read, cache_write, total
                 FROM telemetry_records
                 ORDER BY file_path, record_index",
            )
            .map_err(|error| {
                format!(
                    "Failed to read token telemetry records {}: {error}",
                    path.display()
                )
            })?;
        let rows = statement
            .query_map([], |row| {
                let file_path: String = row.get(0)?;
                Ok((
                    PathBuf::from(file_path),
                    TimedUsage {
                        timestamp: row.get(1)?,
                        model: row.get(2)?,
                        provider: row.get(3)?,
                        counts: TokenCounts {
                            input: stored_u64(row.get(4)?),
                            output: stored_u64(row.get(5)?),
                            cache_read: stored_u64(row.get(6)?),
                            cache_write: stored_u64(row.get(7)?),
                            total: stored_u64(row.get(8)?),
                        },
                    },
                ))
            })
            .map_err(|error| {
                format!(
                    "Failed to query token telemetry records {}: {error}",
                    path.display()
                )
            })?;
        for row in rows {
            let (file_path, record) = row.map_err(|error| {
                format!(
                    "Failed to decode token telemetry records {}: {error}",
                    path.display()
                )
            })?;
            if let Some(file) = files.get_mut(&file_path) {
                file.usage.timed.push(record);
            }
        }
    }

    Ok(files)
}

fn persist_file_usage_index(
    home: &Path,
    changed: &[(PathBuf, CachedFileUsage)],
) -> Result<(), String> {
    let path = telemetry_index_path(home);
    let mut connection = open_telemetry_index(home)?;
    let transaction = connection.transaction().map_err(|error| {
        format!(
            "Failed to update token telemetry index {}: {error}",
            path.display()
        )
    })?;
    {
        let mut upsert_file = transaction
            .prepare(
                "INSERT INTO telemetry_files (
                     path, length, modified_ns, total_input, total_output,
                     total_cache_read, total_cache_write, total, exited,
                     assistant_records, model, source_session_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(path) DO UPDATE SET
                     length = excluded.length,
                     modified_ns = excluded.modified_ns,
                     total_input = excluded.total_input,
                     total_output = excluded.total_output,
                     total_cache_read = excluded.total_cache_read,
                     total_cache_write = excluded.total_cache_write,
                     total = excluded.total,
                     exited = excluded.exited,
                     assistant_records = excluded.assistant_records,
                     model = excluded.model,
                     source_session_id = excluded.source_session_id",
            )
            .map_err(|error| {
                format!(
                    "Failed to prepare token telemetry update {}: {error}",
                    path.display()
                )
            })?;
        let mut delete_records = transaction
            .prepare("DELETE FROM telemetry_records WHERE file_path = ?1")
            .map_err(|error| {
                format!(
                    "Failed to prepare token telemetry record update {}: {error}",
                    path.display()
                )
            })?;
        let mut insert_record = transaction
            .prepare(
                "INSERT INTO telemetry_records (
                     file_path, record_index, timestamp, model, provider, input, output,
                     cache_read, cache_write, total
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )
            .map_err(|error| {
                format!(
                    "Failed to prepare token telemetry record insert {}: {error}",
                    path.display()
                )
            })?;

        for (file_path, cached) in changed {
            let file_path = file_path.to_string_lossy();
            upsert_file
                .execute(params![
                    file_path.as_ref(),
                    sqlite_i64(cached.length),
                    modified_time_to_nanos(cached.modified),
                    sqlite_i64(cached.usage.total.input),
                    sqlite_i64(cached.usage.total.output),
                    sqlite_i64(cached.usage.total.cache_read),
                    sqlite_i64(cached.usage.total.cache_write),
                    sqlite_i64(cached.usage.total.total),
                    if cached.usage.exited { 1_i64 } else { 0_i64 },
                    i64::try_from(cached.usage.assistant_records).unwrap_or(i64::MAX),
                    cached.usage.model.as_deref(),
                    cached.usage.artifact_session_id.as_deref(),
                ])
                .map_err(|error| {
                    format!(
                        "Failed to store token telemetry file {}: {error}",
                        file_path
                    )
                })?;
            delete_records
                .execute([file_path.as_ref()])
                .map_err(|error| {
                    format!(
                        "Failed to replace token telemetry records {}: {error}",
                        file_path
                    )
                })?;
            for (record_index, record) in cached.usage.timed.iter().enumerate() {
                insert_record
                    .execute(params![
                        file_path.as_ref(),
                        i64::try_from(record_index).unwrap_or(i64::MAX),
                        record.timestamp,
                        record.model.as_deref(),
                        record.provider.as_deref(),
                        sqlite_i64(record.counts.input),
                        sqlite_i64(record.counts.output),
                        sqlite_i64(record.counts.cache_read),
                        sqlite_i64(record.counts.cache_write),
                        sqlite_i64(record.counts.total),
                    ])
                    .map_err(|error| {
                        format!(
                            "Failed to store token telemetry record {}: {error}",
                            file_path
                        )
                    })?;
            }
        }
    }
    transaction.commit().map_err(|error| {
        format!(
            "Failed to commit token telemetry index {}: {error}",
            path.display()
        )
    })
}

fn registry_path(home: &Path) -> PathBuf {
    home.join(REGISTRY_DIRECTORY).join(REGISTRY_FILE)
}

fn terminal_session_mapping_directory(home: &Path, profile: Option<&OsStr>) -> PathBuf {
    crate::history::omp_agent_directory_for_profile(home, profile).join("terminal-sessions")
}

fn tmux_pane_fields(line: &str) -> Option<(&str, &str, Option<&str>)> {
    let mut fields = line.splitn(3, '|');
    let session_name = fields.next()?;
    let tty = fields.next()?;
    let profile = fields.next().filter(|profile| !profile.is_empty());
    Some((session_name, tty, profile))
}

fn current_terminal_sessions(home: &Path) -> Result<HashMap<u32, PathBuf>, String> {
    let Some(tmux) = tmux_binary()? else {
        return Ok(HashMap::new());
    };
    let output = match Command::new(tmux)
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{session_name}|#{pane_tty}|#{@omp-profile}",
        ])
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(_) => return Ok(HashMap::new()),
        Err(error) => return Err(format!("Failed to query tmux terminal sessions: {error}")),
    };

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (session_name, tty, profile) = tmux_pane_fields(line)?;
            let profile = profile.map(OsStr::new);
            let slot = session_name
                .strip_prefix(ULTRATERM_SESSION_PREFIX)?
                .parse::<u32>()
                .ok()?;
            let tty_name = Path::new(tty).file_name()?;
            let mapping_directory = terminal_session_mapping_directory(home, profile);
            let mapping = fs::read_to_string(mapping_directory.join(tty_name)).ok()?;
            let session_path = mapping.lines().nth(1).map(PathBuf::from)?;
            Some((slot, session_path))
        })
        .collect())
}

pub(crate) fn tmux_binary() -> Result<Option<PathBuf>, String> {
    crate::resolve_optional_executable(
        "TMUX_BIN",
        "tmux",
        &[
            Path::new("/opt/homebrew/bin/tmux"),
            Path::new("/usr/local/bin/tmux"),
            Path::new("/usr/bin/tmux"),
        ],
    )
}

fn collect_session_files(session_path: &Path, files: &mut HashSet<PathBuf>) {
    if session_path.is_file() {
        files.insert(session_path.to_path_buf());
    }
    collect_telemetry_files(&session_path.with_extension(""), files);
}

fn collect_telemetry_files(directory: &Path, files: &mut HashSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_telemetry_files(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".bash.log"))
                && is_omp_event_stream(&path)
        {
            files.insert(path);
        }
    }
}

fn overseer_usage_directory(home: &Path) -> PathBuf {
    home.join(OVERSEER_USAGE_DIRECTORY)
}

fn is_usage_artifact_path(home: &Path, path: &Path) -> bool {
    path.starts_with(overseer_usage_directory(home))
        && path
            .extension()
            .is_some_and(|extension| extension == "json")
}

fn collect_usage_artifacts(home: &Path, files: &mut HashSet<PathBuf>) {
    fn visit(directory: &Path, files: &mut HashSet<PathBuf>) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, files);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                files.insert(path);
            }
        }
    }

    visit(&overseer_usage_directory(home), files);
}

fn is_omp_event_stream(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Some(Ok(first_line)) = BufReader::new(file).lines().next() else {
        return false;
    };
    serde_json::from_str::<Value>(&first_line).is_ok_and(|value| {
        value.get("type").and_then(Value::as_str) == Some("session")
            && value.get("version").and_then(Value::as_u64) == Some(3)
    })
}

fn usage_identity(value: &Value, message: &Value) -> Option<String> {
    const ID_KEYS: [&str; 7] = [
        "messageId",
        "message_id",
        "id",
        "turnId",
        "turn_id",
        "eventId",
        "event_id",
    ];
    [message, value].into_iter().find_map(|record| {
        ID_KEYS.iter().find_map(|key| {
            let identity = record.get(key)?;
            let identity = identity
                .as_str()
                .map(str::to_string)
                .or_else(|| identity.as_u64().map(|value| value.to_string()))?;
            (!identity.is_empty()).then_some(identity)
        })
    })
}

fn parse_usage_file(path: &Path) -> Result<FileUsage, String> {
    let file = File::open(path).map_err(|error| {
        format!(
            "Failed to open token telemetry file {}: {error}",
            path.display()
        )
    })?;
    let mut usage = FileUsage::default();
    let mut records = Vec::new();

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        usage.exited |= value
            .get("customType")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "session_exit");
        if let Some(model) = value.get("model").and_then(Value::as_str) {
            usage.model = Some(canonical_model_name(model));
        }

        if !matches!(
            value.get("type").and_then(Value::as_str),
            Some("message" | "message_end")
        ) {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if let Some(model) = message.get("model").and_then(Value::as_str) {
            usage.model = Some(canonical_model_name(model));
        }
        let record_model = usage.model.clone();
        let record_provider = message
            .get("provider")
            .and_then(Value::as_str)
            .or_else(|| value.get("provider").and_then(Value::as_str))
            .map(canonical_provider_name);
        let counts = message.get("usage").map(|raw_usage| {
            let input = number(raw_usage, "input");
            let output = number(raw_usage, "output");
            let cache_read = number(raw_usage, "cacheRead");
            let cache_write = number(raw_usage, "cacheWrite");
            let fresh_total = input.saturating_add(output).saturating_add(cache_write);
            TokenCounts {
                input,
                output,
                cache_read,
                cache_write,
                total: if fresh_total == 0 {
                    number(raw_usage, "totalTokens")
                } else {
                    fresh_total
                },
            }
        });
        let timestamp = timestamp_seconds(value.get("timestamp").and_then(Value::as_str))
            .or_else(|| json_timestamp_seconds(message.get("timestamp")));
        records.push(ParsedAssistantRecord {
            identity: usage_identity(&value, message),
            timestamp,
            model: record_model,
            provider: record_provider,
            counts,
        });
    }

    let mut identities = HashMap::new();
    let mut unique_records = Vec::with_capacity(records.len());
    for record in records {
        if let Some(identity) = record.identity.clone() {
            if let Some(index) = identities.get(&identity).copied() {
                unique_records[index] = record;
                continue;
            }
            identities.insert(identity, unique_records.len());
        }
        unique_records.push(record);
    }
    usage.assistant_records = unique_records.len();
    for record in unique_records {
        let Some(counts) = record.counts else {
            continue;
        };
        usage.total.add(&counts);
        if let Some(timestamp) = record.timestamp {
            usage.timed.push(TimedUsage {
                timestamp,
                model: record.model,
                provider: record.provider,
                counts,
            });
        }
    }
    Ok(usage)
}

fn parse_usage_artifact(path: &Path) -> Result<FileUsage, String> {
    let content = fs::read_to_string(path).map_err(|error| {
        format!(
            "Failed to read Overseer token usage artifact {}: {error}",
            path.display()
        )
    })?;
    let value: Value = serde_json::from_str(&content).map_err(|error| {
        format!(
            "Failed to parse Overseer token usage artifact {}: {error}",
            path.display()
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        format!(
            "Overseer token usage artifact {} must be a JSON object",
            path.display()
        )
    })?;
    const ALLOWED_KEYS: [&str; 12] = [
        "version",
        "session_id",
        "started_at",
        "completed_at",
        "model",
        "provider",
        "run_mode",
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
        "reasoning_tokens",
    ];
    if object
        .keys()
        .any(|key| !ALLOWED_KEYS.contains(&key.as_str()))
    {
        return Err(format!(
            "Overseer token usage artifact {} contains an unknown field",
            path.display()
        ));
    }
    if object.len() != ALLOWED_KEYS.len() {
        return Err(format!(
            "Overseer token usage artifact {} is incomplete",
            path.display()
        ));
    }
    if object.get("version").and_then(Value::as_u64) != Some(USAGE_ARTIFACT_VERSION) {
        return Err(format!(
            "Unsupported Overseer token usage artifact version in {}",
            path.display()
        ));
    }
    let session_id = object
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Invalid session_id in {}", path.display()))?
        .to_string();
    let started_at = object
        .get("started_at")
        .and_then(Value::as_u64)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| format!("Invalid started_at in {}", path.display()))?;
    let completed_at = object
        .get("completed_at")
        .and_then(Value::as_u64)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| format!("Invalid completed_at in {}", path.display()))?;
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(canonical_model_name)
        .ok_or_else(|| format!("Invalid model in {}", path.display()))?;
    let provider = object
        .get("provider")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(canonical_provider_name)
        .ok_or_else(|| format!("Invalid provider in {}", path.display()))?;
    object
        .get("run_mode")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Invalid run_mode in {}", path.display()))?;
    let artifact_number = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("Invalid {key} in {}", path.display()))
    };
    let input = artifact_number("input_tokens")?;
    let output = artifact_number("output_tokens")?;
    let cache_read = artifact_number("cache_read_tokens")?;
    let cache_write = artifact_number("cache_write_tokens")?;
    let _reasoning = artifact_number("reasoning_tokens")?;
    let total = input.saturating_add(output).saturating_add(cache_write);
    let counts = TokenCounts {
        input,
        output,
        cache_read,
        cache_write,
        total,
    };
    Ok(FileUsage {
        total: counts.clone(),
        timed: vec![TimedUsage {
            timestamp: completed_at.max(started_at),
            model: Some(model.clone()),
            provider: Some(provider),
            counts,
        }],
        exited: true,
        assistant_records: 1,
        model: Some(model),
        artifact_session_id: Some(session_id),
    })
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn timestamp_seconds(timestamp: Option<&str>) -> Option<i64> {
    DateTime::parse_from_rfc3339(timestamp?)
        .ok()
        .map(|timestamp| timestamp.timestamp())
}

fn json_timestamp_seconds(timestamp: Option<&Value>) -> Option<i64> {
    let timestamp = timestamp?;
    if let Some(timestamp) = timestamp.as_str() {
        return timestamp_seconds(Some(timestamp));
    }
    let timestamp = timestamp.as_i64().or_else(|| {
        timestamp
            .as_u64()
            .and_then(|value| i64::try_from(value).ok())
    })?;
    Some(if timestamp >= 100_000_000_000 {
        timestamp / 1_000
    } else {
        timestamp
    })
}

fn session_id(path: &Path) -> Option<String> {
    path.file_stem()?
        .to_str()?
        .rsplit_once('_')
        .map(|(_, id)| id.to_string())
}

/// Reads the OMP session title from the padded `{"type":"title"}` record on
/// the first line of a session transcript. OMP rewrites that line in place
/// when it generates a topic summary, so a fresh read always reflects the
/// latest title without touching the sqlite usage index.
fn session_title(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(file).read_line(&mut line).ok()?;
    let value = serde_json::from_str::<Value>(line.trim_end()).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("title") {
        return None;
    }
    let title = value.get("title").and_then(Value::as_str)?.trim();
    (!title.is_empty()).then(|| title.to_string())
}
fn is_counted_subagent(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "jsonl")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name != "__advisor.jsonl")
}

fn subagent_is_complete(path: &Path, usage: &FileUsage) -> bool {
    usage.assistant_records > 0 && (usage.exited || path.with_extension("md").is_file())
}

fn subagent_is_active(path: &Path, usage: &FileUsage) -> bool {
    path.is_file() && usage.assistant_records > 0 && !subagent_is_complete(path, usage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::tempdir;

    fn pricing_models_json(input: f64) -> String {
        serde_json::json!([{
            "id": "~deepseek/deepseek-v4-flash-latest",
            "cost": {
                "input": input,
                "output": 0.252,
                "cacheRead": 0.0252,
                "cacheWrite": 0.0
            }
        }])
        .to_string()
    }

    fn create_pricing_db(path: &Path, updated_at: i64, input: f64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let connection = Connection::open(path).unwrap();
        connection
            .execute(
                "CREATE TABLE model_cache (updated_at INTEGER NOT NULL, models TEXT NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO model_cache (updated_at, models) VALUES (?1, ?2)",
                params![updated_at, pricing_models_json(input)],
            )
            .unwrap();
    }

    #[test]
    fn extracts_session_id_from_omp_path() {
        let path = Path::new("2026-07-18T07-10-31-739Z_019f740f-f93b.jsonl");
        assert_eq!(session_id(path).as_deref(), Some("019f740f-f93b"));
    }

    #[test]
    fn reads_title_record_from_session_transcript() {
        let root =
            std::env::temp_dir().join(format!("ultraterm-session-title-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session.jsonl");
        fs::write(
            &path,
            "{\"type\":\"title\",\"v\":1,\"title\":\"Fixing auth flow\",\"updatedAt\":\"2026-08-15T00:00:00.000Z\",\"pad\":\"   \"}\n{\"type\":\"session\",\"version\":3}\n",
        )
        .unwrap();
        assert_eq!(session_title(&path).as_deref(), Some("Fixing auth flow"));

        fs::write(
            &path,
            "{\"type\":\"title\",\"v\":1,\"title\":\"\",\"pad\":\"   \"}\n",
        )
        .unwrap();
        assert_eq!(session_title(&path), None);

        fs::write(&path, "{\"type\":\"session\",\"version\":3}\n").unwrap();
        assert_eq!(session_title(&path), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tmux_pane_profile_is_read_with_legacy_fallback() {
        assert_eq!(
            tmux_pane_fields("ultraterm-matrix-2|/dev/ttys002|gpt-only"),
            Some(("ultraterm-matrix-2", "/dev/ttys002", Some("gpt-only")))
        );
        assert_eq!(
            tmux_pane_fields("ultraterm-matrix-3|/dev/ttys003|"),
            Some(("ultraterm-matrix-3", "/dev/ttys003", None))
        );
        assert_eq!(
            tmux_pane_fields("ultraterm-matrix-4|/dev/ttys004"),
            Some(("ultraterm-matrix-4", "/dev/ttys004", None))
        );
    }

    #[test]
    fn token_counts_add_without_overflowing() {
        let mut counts = TokenCounts {
            input: u64::MAX,
            ..TokenCounts::default()
        };
        counts.add(&TokenCounts {
            input: 1,
            ..TokenCounts::default()
        });
        assert_eq!(counts.input, u64::MAX);
    }

    #[test]
    fn discovers_nested_subagent_and_omp_stream_telemetry() {
        let root = std::env::temp_dir().join(format!(
            "ultraterm-telemetry-delegations-{}",
            std::process::id()
        ));
        let nested = root.join("delegated-task");
        fs::create_dir_all(&nested).unwrap();
        let transcript = nested.join("worker.jsonl");
        let paid_stream = nested.join("paid-api.bash.log");
        let unrelated_log = nested.join("other.bash.log");
        fs::write(&transcript, "").unwrap();
        fs::write(
            &paid_stream,
            "{\"type\":\"session\",\"version\":3,\"id\":\"paid\"}\n",
        )
        .unwrap();
        fs::write(&unrelated_log, "ordinary command output\n").unwrap();

        let mut files = HashSet::new();
        collect_telemetry_files(&root, &mut files);

        assert!(files.contains(&transcript));
        assert!(files.contains(&paid_stream));
        assert!(!files.contains(&unrelated_log));
        assert!(is_counted_subagent(&transcript));
        assert!(!is_counted_subagent(&paid_stream));
        assert!(!is_counted_subagent(Path::new("__advisor.jsonl")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_exit_is_sticky_and_latest_model_is_reported() {
        let transcript = std::env::temp_dir().join(format!(
            "ultraterm-telemetry-lifecycle-{}.jsonl",
            std::process::id()
        ));
        fs::write(
            &transcript,
            concat!(
                "{\"type\":\"model_change\",\"model\":\"openai-codex/gpt-5.5\"}\n",
                "{\"type\":\"custom\",\"customType\":\"session_exit\"}\n",
                "{\"type\":\"message\",\"timestamp\":\"2026-07-18T12:00:00Z\",\"message\":{\"role\":\"assistant\",\"model\":\"openai-codex/gpt-5.6-sol\",\"usage\":{\"input\":1,\"output\":2,\"cacheRead\":3,\"cacheWrite\":4,\"totalTokens\":10}}}\n"
            ),
        )
        .unwrap();

        let usage = parse_usage_file(&transcript).unwrap();

        assert!(usage.exited);
        assert_eq!(usage.assistant_records, 1);
        assert_eq!(usage.model.as_deref(), Some("openai-codex/gpt-5.6-sol"));
        assert_eq!(
            usage.timed[0].model.as_deref(),
            Some("openai-codex/gpt-5.6-sol")
        );
        fs::remove_file(transcript).unwrap();
    }

    #[test]
    fn total_excludes_reused_cache_tokens() {
        let transcript = std::env::temp_dir().join(format!(
            "ultraterm-telemetry-cache-{}.jsonl",
            std::process::id()
        ));
        fs::write(
            &transcript,
            "{\"type\":\"message\",\"timestamp\":\"2026-07-18T12:00:00Z\",\"message\":{\"role\":\"assistant\",\"usage\":{\"input\":10,\"output\":5,\"cacheRead\":1000,\"cacheWrite\":2,\"totalTokens\":1017}}}\n",
        )
        .unwrap();

        let usage = parse_usage_file(&transcript).unwrap();

        assert_eq!(usage.total.input, 10);
        assert_eq!(usage.total.output, 5);
        assert_eq!(usage.total.cache_read, 1000);
        assert_eq!(usage.total.cache_write, 2);
        assert_eq!(usage.total.total, 17);
        fs::remove_file(transcript).unwrap();
    }

    #[test]
    fn total_falls_back_when_provider_only_reports_total_tokens() {
        let transcript = std::env::temp_dir().join(format!(
            "ultraterm-telemetry-total-fallback-{}.jsonl",
            std::process::id()
        ));
        fs::write(
            &transcript,
            "{\"type\":\"message\",\"timestamp\":\"2026-07-18T12:00:00Z\",\"message\":{\"role\":\"assistant\",\"usage\":{\"totalTokens\":42}}}\n",
        )
        .unwrap();

        let usage = parse_usage_file(&transcript).unwrap();

        assert_eq!(usage.total.total, 42);
        fs::remove_file(transcript).unwrap();
    }

    #[test]
    fn completion_artifact_prevents_false_active_agent() {
        let root = std::env::temp_dir().join(format!(
            "ultraterm-telemetry-completion-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let transcript = root.join("worker.jsonl");
        fs::write(
            &transcript,
            "{\"type\":\"message\",\"timestamp\":\"2026-07-18T12:00:00Z\",\"message\":{\"role\":\"assistant\",\"usage\":{\"totalTokens\":1}}}\n",
        )
        .unwrap();
        let usage = parse_usage_file(&transcript).unwrap();

        assert!(subagent_is_active(&transcript, &usage));
        fs::write(transcript.with_extension("md"), "Result submitted.").unwrap();
        assert!(!subagent_is_active(&transcript, &usage));
        assert!(subagent_is_complete(&transcript, &usage));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_transcript_is_not_an_active_or_complete_agent() {
        let root =
            std::env::temp_dir().join(format!("ultraterm-telemetry-empty-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let transcript = root.join("worker.jsonl");
        fs::write(&transcript, "").unwrap();
        fs::write(transcript.with_extension("md"), "").unwrap();
        let usage = parse_usage_file(&transcript).unwrap();

        assert!(!subagent_is_active(&transcript, &usage));
        assert!(!subagent_is_complete(&transcript, &usage));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rolling_windows_and_calendar_today_use_different_boundaries() {
        let now = Local
            .with_ymd_and_hms(2026, 7, 18, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        let previous_evening = Local
            .with_ymd_and_hms(2026, 7, 17, 18, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        let today_morning = Local
            .with_ymd_and_hms(2026, 7, 18, 8, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        let old = now - 8 * SECONDS_PER_DAY;
        let counts = |total| TokenCounts {
            total,
            ..TokenCounts::default()
        };
        let mut usage = FileUsage::default();
        usage.timed = vec![
            TimedUsage {
                timestamp: previous_evening,
                model: Some("gpt".to_string()),
                provider: None,
                counts: counts(2),
            },
            TimedUsage {
                timestamp: today_morning,
                model: Some("kimi".to_string()),
                provider: None,
                counts: counts(1),
            },
            TimedUsage {
                timestamp: old,
                model: Some("gpt".to_string()),
                provider: None,
                counts: counts(4),
            },
        ];
        usage.total = counts(7);

        let aggregate = aggregate_usage(std::iter::once(&usage), &HashMap::new(), now);

        assert_eq!(aggregate.today.total, 1);
        assert_eq!(aggregate.past_24_hours.total, 3);
        assert_eq!(aggregate.past_7_days.total, 3);
        assert_eq!(aggregate.all_time.total, 7);
    }

    #[test]
    fn history_is_chronological_and_model_sums_match_each_day() {
        let first_day = Local
            .with_ymd_and_hms(2026, 7, 17, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        let second_day = Local
            .with_ymd_and_hms(2026, 7, 18, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        let counts = |total| TokenCounts {
            total,
            ..TokenCounts::default()
        };
        let mut usage = FileUsage::default();
        usage.timed = vec![
            TimedUsage {
                timestamp: second_day,
                model: Some("gpt".to_string()),
                provider: None,
                counts: counts(3),
            },
            TimedUsage {
                timestamp: first_day,
                model: Some("kimi".to_string()),
                provider: None,
                counts: counts(2),
            },
            TimedUsage {
                timestamp: second_day + 60,
                model: Some("kimi".to_string()),
                provider: None,
                counts: counts(5),
            },
        ];
        usage.total = counts(10);

        let history = aggregate_usage(std::iter::once(&usage), &HashMap::new(), second_day).history;

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].date, "2026-07-17");
        assert_eq!(history[1].date, "2026-07-18");
        for day in history {
            assert_eq!(
                day.models
                    .iter()
                    .map(|model| model.usage.total)
                    .sum::<u64>(),
                day.usage.total
            );
        }
    }

    #[test]
    fn timed_records_keep_the_model_active_for_each_record() {
        let root = tempdir().unwrap();
        let transcript = root.path().join("models.jsonl");
        fs::write(
            &transcript,
            concat!(
                "{\"type\":\"model_change\",\"model\":\"openai-codex/gpt-5.6-sol\"}\n",
                "{\"type\":\"message\",\"timestamp\":\"2026-07-18T12:00:00Z\",\"message\":{\"role\":\"assistant\",\"usage\":{\"totalTokens\":2}}}\n",
                "{\"type\":\"model_change\",\"model\":\"kimi-coding/k2p5\"}\n",
                "{\"type\":\"message\",\"timestamp\":\"2026-07-18T12:01:00Z\",\"message\":{\"role\":\"assistant\",\"usage\":{\"totalTokens\":3}}}\n"
            ),
        )
        .unwrap();

        let usage = parse_usage_file(&transcript).unwrap();

        assert_eq!(usage.timed.len(), 2);
        assert_eq!(
            usage.timed[0].model.as_deref(),
            Some("openai-codex/gpt-5.6-sol")
        );
        assert_eq!(usage.timed[1].model.as_deref(), Some("kimi-coding/k2p5"));
    }

    #[test]
    fn parses_nested_omp_stream_once_with_message_timestamp() {
        let root = tempdir().unwrap();
        let stream = root.path().join("paid-api.bash.log");
        let assistant = "{\"role\":\"assistant\",\"provider\":\"openrouter\",\"model\":\"~deepseek/deepseek-v4-flash-latest\",\"timestamp\":1786329775974,\"usage\":{\"input\":8423,\"output\":13,\"cacheRead\":64,\"cacheWrite\":0,\"totalTokens\":8500}}";
        fs::write(
            &stream,
            format!(
                "{{\"type\":\"session\",\"version\":3,\"id\":\"paid\"}}\n\
                 {{\"type\":\"message_start\",\"message\":{assistant}}}\n\
                 {{\"type\":\"message_end\",\"message\":{assistant}}}\n\
                 {{\"type\":\"turn_end\",\"message\":{assistant}}}\n"
            ),
        )
        .unwrap();

        let usage = parse_usage_file(&stream).unwrap();

        assert_eq!(usage.assistant_records, 1);
        assert_eq!(usage.total.input, 8423);
        assert_eq!(usage.total.output, 13);
        assert_eq!(usage.total.cache_read, 64);
        assert_eq!(usage.timed.len(), 1);
        assert_eq!(usage.timed[0].timestamp, 1_786_329_775);
        assert_eq!(usage.timed[0].provider.as_deref(), Some("openrouter"));
        assert_eq!(
            usage.timed[0].model.as_deref(),
            Some("deepseek/deepseek-v4-flash-latest")
        );
        assert!(matches!(
            token_channel(
                usage.timed[0].provider.as_deref(),
                usage.timed[0].model.as_deref()
            ),
            TokenChannel::PaidApi
        ));
    }

    #[test]
    fn message_and_message_end_with_same_identity_count_once() {
        let root = tempdir().unwrap();
        let transcript = root.path().join("identity.jsonl");
        fs::write(
            &transcript,
            concat!(
                "{\"type\":\"message\",\"timestamp\":\"2026-07-18T12:00:00Z\",\"message\":{\"id\":\"turn-1\",\"role\":\"assistant\",\"usage\":{\"input\":2,\"output\":1,\"cacheRead\":5,\"cacheWrite\":1}}}\n",
                "{\"type\":\"message_end\",\"timestamp\":\"2026-07-18T12:00:01Z\",\"message\":{\"id\":\"turn-1\",\"role\":\"assistant\",\"usage\":{\"input\":10,\"output\":4,\"cacheRead\":50,\"cacheWrite\":2}}}\n"
            ),
        )
        .unwrap();

        let usage = parse_usage_file(&transcript).unwrap();

        assert_eq!(usage.assistant_records, 1);
        assert_eq!(usage.total.input, 10);
        assert_eq!(usage.total.output, 4);
        assert_eq!(usage.total.cache_read, 50);
        assert_eq!(usage.total.cache_write, 2);
        assert_eq!(usage.total.total, 16);
        assert_eq!(usage.timed.len(), 1);
    }

    #[test]
    fn repeated_usage_identity_aliases_keep_only_the_final_record() {
        let root = tempdir().unwrap();
        let transcript = root.path().join("identity-aliases.jsonl");
        fs::write(
            &transcript,
            concat!(
                "{\"type\":\"message\",\"message\":{\"messageId\":\"turn-1\",\"role\":\"assistant\",\"usage\":{\"input\":2,\"output\":1}}}\n",
                "{\"type\":\"message_end\",\"message\":{\"id\":\"turn-1\",\"role\":\"assistant\",\"usage\":{\"input\":10,\"output\":4}}}\n",
                "{\"type\":\"message_end\",\"message\":{\"turnId\":\"turn-1\",\"role\":\"assistant\",\"usage\":{\"input\":20,\"output\":5}}}\n"
            ),
        )
        .unwrap();

        let usage = parse_usage_file(&transcript).unwrap();

        assert_eq!(usage.assistant_records, 1);
        assert_eq!(usage.total.input, 20);
        assert_eq!(usage.total.output, 5);
        assert_eq!(usage.total.total, 25);
    }

    #[test]
    fn canonicalizes_only_known_model_provider_syntax_aliases() {
        assert_eq!(
            canonical_model_name("~openrouter.ai/deepseek/deepseek-v4"),
            "openrouter/deepseek/deepseek-v4"
        );
        assert_eq!(canonical_provider_name("OpenRouter.AI"), "openrouter");
        assert_ne!(
            canonical_model_name("kimi-code/k3"),
            canonical_model_name("kimi-coding/k3")
        );
    }

    #[test]
    fn rejects_malformed_partial_and_unknown_usage_artifacts() {
        let root = tempdir().unwrap();
        let partial = root.path().join("partial.json");
        fs::write(&partial, "{\"version\":1,\"session_id\":\"partial\"}").unwrap();
        assert!(parse_usage_artifact(&partial).is_err());

        let unknown = root.path().join("unknown.json");
        let mut value = serde_json::json!({
            "version": 1,
            "session_id": "unknown",
            "started_at": 1,
            "completed_at": 2,
            "model": "kimi-code/k3",
            "provider": "kimi-code",
            "run_mode": "quiet",
            "input_tokens": 1,
            "output_tokens": 2,
            "cache_read_tokens": 3,
            "cache_write_tokens": 4,
            "reasoning_tokens": 5
        });
        value["prompt"] = Value::String("must not be captured".to_string());
        fs::write(&unknown, serde_json::to_string(&value).unwrap()).unwrap();
        assert!(parse_usage_artifact(&unknown).is_err());
    }

    #[test]
    fn parent_native_child_and_external_artifacts_count_once() {
        let root = tempdir().unwrap();
        let parent = root.path().join("parent.jsonl");
        let child_directory = parent.with_extension("");
        fs::create_dir_all(&child_directory).unwrap();
        fs::write(
            &parent,
            "{\"type\":\"message\",\"timestamp\":\"2026-07-18T12:00:00Z\",\"message\":{\"role\":\"assistant\",\"usage\":{\"input\":2,\"output\":2,\"cacheWrite\":1}}}\n",
        )
        .unwrap();
        let child = child_directory.join("worker.jsonl");
        fs::write(
            &child,
            "{\"type\":\"message\",\"timestamp\":\"2026-07-18T12:01:00Z\",\"message\":{\"role\":\"assistant\",\"usage\":{\"input\":1,\"output\":1,\"cacheWrite\":1}}}\n",
        )
        .unwrap();

        let usage_directory = root.path().join(".overseer/usage");
        fs::create_dir_all(&usage_directory).unwrap();
        let artifact = serde_json::json!({
            "version": 1,
            "session_id": "external-session",
            "started_at": 1784376000_u64,
            "completed_at": 1784376001_u64,
            "model": "~openrouter.ai/deepseek/deepseek-v4",
            "provider": "openrouter.ai",
            "run_mode": "quiet",
            "input_tokens": 4,
            "output_tokens": 3,
            "cache_read_tokens": 100,
            "cache_write_tokens": 1,
            "reasoning_tokens": 2
        });
        let artifact_path = usage_directory.join("external.json");
        fs::write(&artifact_path, serde_json::to_string(&artifact).unwrap()).unwrap();
        let duplicate_path = usage_directory.join("duplicate.json");
        fs::write(&duplicate_path, serde_json::to_string(&artifact).unwrap()).unwrap();

        let mut files = HashSet::new();
        collect_session_files(&parent, &mut files);
        collect_usage_artifacts(root.path(), &mut files);
        let mut cache = HashMap::new();
        for path in &files {
            let usage = if is_usage_artifact_path(root.path(), path) {
                parse_usage_artifact(path).unwrap()
            } else {
                parse_usage_file(path).unwrap()
            };
            cache.insert(
                path.clone(),
                CachedFileUsage {
                    length: fs::metadata(path).unwrap().len(),
                    modified: fs::metadata(path).unwrap().modified().ok(),
                    usage,
                },
            );
        }

        let aggregate = aggregate_usage(
            deduplicated_file_usages(&cache).into_iter(),
            &HashMap::new(),
            1_784_376_100,
        );
        assert_eq!(files.len(), 4);
        assert_eq!(aggregate.all_time.input, 7);
        assert_eq!(aggregate.all_time.output, 6);
        assert_eq!(aggregate.all_time.cache_read, 100);
        assert_eq!(aggregate.all_time.cache_write, 3);
        assert_eq!(aggregate.all_time.total, 16);
    }

    #[test]
    fn persisted_index_survives_source_deletion_and_manager_reload() {
        let root = tempdir().unwrap();
        let home = root.path();
        let transcript = home.join("session.jsonl");
        fs::write(
            &transcript,
            "{\"type\":\"message\",\"timestamp\":\"2026-07-18T12:00:00Z\",\"message\":{\"role\":\"assistant\",\"model\":\"gpt\",\"usage\":{\"input\":2,\"output\":3}}}\n",
        )
        .unwrap();
        let mut files = HashSet::new();
        files.insert(transcript.clone());
        let mut manager = TokenTelemetryManager::default();
        manager.load_index(home).unwrap();
        manager.refresh_files(home, &files).unwrap();
        let now = timestamp_seconds(Some("2026-07-18T13:00:00Z")).unwrap();
        let before = aggregate_usage(
            manager.file_cache.values().map(|cached| &cached.usage),
            &HashMap::new(),
            now,
        );
        drop(manager);
        fs::remove_file(&transcript).unwrap();

        let mut reloaded = TokenTelemetryManager::default();
        reloaded.load_index(home).unwrap();
        let after = aggregate_usage(
            reloaded.file_cache.values().map(|cached| &cached.usage),
            &HashMap::new(),
            now,
        );

        assert_eq!(before.all_time.total, 5);
        assert_eq!(after.all_time.total, before.all_time.total);
        assert_eq!(after.history.len(), before.history.len());
        assert_eq!(after.history[0].usage.total, before.history[0].usage.total);
        assert_eq!(after.today.total, before.today.total);
        assert_eq!(
            after.history[0].models[0].usage.total,
            before.history[0].models[0].usage.total
        );
        assert_eq!(after.history[0].models[0].model, "gpt");
    }

    #[test]
    fn legacy_records_infer_paid_api_from_model_namespace() {
        assert!(matches!(
            token_channel(None, Some("openai/gpt-5.1")),
            TokenChannel::PaidApi
        ));
        assert!(matches!(
            token_channel(None, Some("kimi-code/k3-256k")),
            TokenChannel::Subscription
        ));
        assert!(matches!(
            token_channel(Some("openai-codex"), Some("openai-codex/gpt-5.6-sol")),
            TokenChannel::Subscription
        ));
    }

    #[test]
    fn loads_profile_specific_model_pricing() {
        let root = tempdir().unwrap();
        let models_path = root
            .path()
            .join(".omp/profiles/deepseek-v4-flash/agent/models.db");
        create_pricing_db(&models_path, 1, 0.079996);
        let mut manager = TokenTelemetryManager::default();

        manager.refresh_model_pricing(root.path());

        let price = model_price(
            &manager.pricing,
            Some("openrouter/~deepseek/deepseek-v4-flash-latest"),
        )
        .copied()
        .unwrap();
        assert!((price.input - 0.079996).abs() < f64::EPSILON);
        assert!((price.output - 0.252).abs() < f64::EPSILON);
        assert!((price.cache_read - 0.0252).abs() < f64::EPSILON);
    }

    #[test]
    fn freshest_profile_model_price_wins() {
        let root = tempdir().unwrap();
        create_pricing_db(
            &root.path().join(".omp/profiles/z-old/agent/models.db"),
            1,
            0.1,
        );
        create_pricing_db(
            &root.path().join(".omp/profiles/a-new/agent/models.db"),
            2,
            0.2,
        );
        let mut manager = TokenTelemetryManager::default();

        manager.refresh_model_pricing(root.path());

        let price = model_price(
            &manager.pricing,
            Some("openrouter/~deepseek/deepseek-v4-flash-latest"),
        )
        .unwrap();
        assert!((price.input - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn freshest_provider_row_wins_within_profile() {
        let root = tempdir().unwrap();
        let models_path = root.path().join(".omp/profiles/shared/agent/models.db");
        create_pricing_db(&models_path, 2, 0.2);
        let connection = Connection::open(&models_path).unwrap();
        connection
            .execute(
                "INSERT INTO model_cache (updated_at, models) VALUES (?1, ?2)",
                params![1_i64, pricing_models_json(0.1)],
            )
            .unwrap();
        let mut manager = TokenTelemetryManager::default();

        manager.refresh_model_pricing(root.path());

        let price = model_price(
            &manager.pricing,
            Some("openrouter/~deepseek/deepseek-v4-flash-latest"),
        )
        .unwrap();
        assert!((price.input - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn pricing_refresh_observes_wal_updates() {
        let root = tempdir().unwrap();
        let models_path = root.path().join(".omp/profiles/wal/agent/models.db");
        fs::create_dir_all(models_path.parent().unwrap()).unwrap();
        let connection = Connection::open(&models_path).unwrap();
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE model_cache (updated_at INTEGER NOT NULL, models TEXT NOT NULL);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO model_cache (updated_at, models) VALUES (?1, ?2)",
                params![1_i64, pricing_models_json(0.1)],
            )
            .unwrap();
        let mut manager = TokenTelemetryManager::default();
        manager.refresh_model_pricing(root.path());

        connection
            .execute(
                "UPDATE model_cache SET updated_at = ?1, models = ?2",
                params![2_i64, pricing_models_json(0.2)],
            )
            .unwrap();
        manager.refresh_model_pricing(root.path());

        let price = model_price(
            &manager.pricing,
            Some("openrouter/~deepseek/deepseek-v4-flash-latest"),
        )
        .unwrap();
        assert!((price.input - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn failed_pricing_sources_are_retried() {
        let root = tempdir().unwrap();
        let models_path = root.path().join(".omp/profiles/broken/agent/models.db");
        fs::create_dir_all(models_path.parent().unwrap()).unwrap();
        fs::write(&models_path, "not a sqlite database").unwrap();
        let mut manager = TokenTelemetryManager::default();

        manager.refresh_model_pricing(root.path());

        assert!(manager.pricing_sources.is_empty());
    }

    #[test]
    fn today_channels_split_subscription_and_paid_api_with_cost() {
        let now = Local
            .with_ymd_and_hms(2026, 8, 9, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp();
        let mut usage = FileUsage::default();
        usage.timed = vec![
            TimedUsage {
                timestamp: now,
                model: Some("gpt-5.6-sol".to_string()),
                provider: Some("openai-codex".to_string()),
                counts: TokenCounts {
                    input: 10,
                    output: 5,
                    total: 15,
                    ..TokenCounts::default()
                },
            },
            TimedUsage {
                timestamp: now,
                model: Some("deepseek/deepseek-v4-flash".to_string()),
                provider: Some("openrouter".to_string()),
                counts: TokenCounts {
                    input: 1_000_000,
                    output: 500_000,
                    cache_read: 250_000,
                    total: 1_500_000,
                    ..TokenCounts::default()
                },
            },
        ];
        usage.total = TokenCounts {
            input: 1_000_010,
            output: 500_005,
            cache_read: 250_000,
            total: 1_500_015,
            ..TokenCounts::default()
        };
        let mut pricing = HashMap::new();
        pricing.insert(
            "deepseek/deepseek-v4-flash".to_string(),
            ModelPrice {
                input: 0.14,
                output: 0.28,
                cache_read: 0.028,
                cache_write: 0.0,
            },
        );

        let aggregate = aggregate_usage(std::iter::once(&usage), &pricing, now);

        assert_eq!(aggregate.today.total, 1_500_015);
        assert_eq!(aggregate.today_channels.subscription.total, 15);
        assert_eq!(aggregate.today_channels.paid_api.total, 1_500_000);
        assert!((aggregate.today_channels.paid_api_cost_usd - 0.287).abs() < 0.000_001);
        assert_eq!(aggregate.past_24_hour_channels.subscription.total, 15);
        assert_eq!(aggregate.past_24_hour_channels.paid_api.total, 1_500_000);
    }

    #[test]
    fn legacy_json_registry_paths_still_load() {
        let root = tempdir().unwrap();
        let legacy_path = root.path().join("legacy-session.jsonl");
        let registry = registry_path(root.path());
        fs::create_dir_all(registry.parent().unwrap()).unwrap();
        fs::write(
            registry,
            serde_json::to_vec(&vec![legacy_path.clone()]).unwrap(),
        )
        .unwrap();
        let mut manager = TokenTelemetryManager::default();

        manager.load_registry(root.path()).unwrap();

        assert!(manager.registered_sessions.contains(&legacy_path));
    }
}
