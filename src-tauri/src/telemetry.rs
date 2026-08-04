use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use serde::Serialize;
use serde_json::Value;

const SNAPSHOT_TTL: Duration = Duration::from_secs(3);
const SECONDS_PER_DAY: i64 = 24 * 60 * 60;
const REGISTRY_DIRECTORY: &str = ".ultraterm";
const REGISTRY_FILE: &str = "telemetry-sessions.json";
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalTokenTelemetry {
    pub slot: u32,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub usage: TokenCounts,
    pub active_subagents: usize,
    pub inactive_subagents: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenTelemetry {
    pub terminals: Vec<TerminalTokenTelemetry>,
    pub past_24_hours: TokenCounts,
    pub past_7_days: TokenCounts,
    pub all_time: TokenCounts,
    pub active_subagents: usize,
    pub inactive_subagents: usize,
    pub parallel_agents: usize,
    pub tracked_sessions: usize,
    pub updated_at: u64,
}

#[derive(Clone, Default)]
struct FileUsage {
    total: TokenCounts,
    timed: Vec<(i64, TokenCounts)>,
    exited: bool,
    assistant_records: usize,
    model: Option<String>,
}

fn aggregate_windows<'a>(
    usages: impl Iterator<Item = &'a FileUsage>,
    now: i64,
) -> (TokenCounts, TokenCounts, TokenCounts) {
    let past_24_hour_boundary = now.saturating_sub(SECONDS_PER_DAY);
    let past_7_day_boundary = now.saturating_sub(7 * SECONDS_PER_DAY);
    let mut past_24_hours = TokenCounts::default();
    let mut past_7_days = TokenCounts::default();
    let mut all_time = TokenCounts::default();

    for usage in usages {
        all_time.add(&usage.total);
        for (timestamp, counts) in &usage.timed {
            if *timestamp >= past_7_day_boundary {
                past_7_days.add(counts);
            }
            if *timestamp >= past_24_hour_boundary {
                past_24_hours.add(counts);
            }
        }
    }

    (past_24_hours, past_7_days, all_time)
}

struct CachedFileUsage {
    length: u64,
    modified: Option<SystemTime>,
    usage: FileUsage,
}

pub struct TokenTelemetryManager {
    file_cache: HashMap<PathBuf, CachedFileUsage>,
    registered_sessions: HashSet<PathBuf>,
    registry_loaded: bool,
    snapshot: Option<(Instant, TokenTelemetry)>,
}

impl Default for TokenTelemetryManager {
    fn default() -> Self {
        Self {
            file_cache: HashMap::new(),
            registered_sessions: HashSet::new(),
            registry_loaded: false,
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
        self.refresh_files(&files);
        self.file_cache.retain(|path, _| files.contains(path));

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let (past_24_hours, past_7_days, all_time) =
            aggregate_windows(self.file_cache.values().map(|cached| &cached.usage), now);

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
            past_24_hours,
            past_7_days,
            all_time,
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

    fn refresh_files(&mut self, files: &HashSet<PathBuf>) {
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
            if let Ok(usage) = parse_usage_file(path) {
                self.file_cache.insert(
                    path.clone(),
                    CachedFileUsage {
                        length,
                        modified,
                        usage,
                    },
                );
            }
        }
    }
}

fn registry_path(home: &Path) -> PathBuf {
    home.join(REGISTRY_DIRECTORY).join(REGISTRY_FILE)
}

fn terminal_session_mapping_directory(home: &Path) -> PathBuf {
    crate::history::omp_agent_directory(home).join("terminal-sessions")
}

fn current_terminal_sessions(home: &Path) -> Result<HashMap<u32, PathBuf>, String> {
    let Some(tmux) = tmux_binary()? else {
        return Ok(HashMap::new());
    };
    let output = match Command::new(tmux)
        .args(["list-panes", "-a", "-F", "#{session_name}|#{pane_tty}"])
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(_) => return Ok(HashMap::new()),
        Err(error) => return Err(format!("Failed to query tmux terminal sessions: {error}")),
    };

    let mapping_directory = terminal_session_mapping_directory(home);
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (session_name, tty) = line.split_once('|')?;
            let slot = session_name
                .strip_prefix(ULTRATERM_SESSION_PREFIX)?
                .parse::<u32>()
                .ok()?;
            let tty_name = Path::new(tty).file_name()?;
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
    collect_jsonl_files(&session_path.with_extension(""), files);
}

fn collect_jsonl_files(directory: &Path, files: &mut HashSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            files.insert(path);
        }
    }
}

fn parse_usage_file(path: &Path) -> Result<FileUsage, String> {
    let file = File::open(path).map_err(|error| {
        format!(
            "Failed to open token telemetry file {}: {error}",
            path.display()
        )
    })?;
    let mut usage = FileUsage::default();

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        usage.exited |= value
            .get("customType")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "session_exit");
        if let Some(model) = value.get("model").and_then(Value::as_str) {
            usage.model = Some(model.to_string());
        }

        let Some(message) = value.get("message") else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        usage.assistant_records = usage.assistant_records.saturating_add(1);
        if let Some(model) = message.get("model").and_then(Value::as_str) {
            usage.model = Some(model.to_string());
        }
        let Some(raw_usage) = message.get("usage") else {
            continue;
        };
        let input = number(raw_usage, "input");
        let output = number(raw_usage, "output");
        let cache_read = number(raw_usage, "cacheRead");
        let cache_write = number(raw_usage, "cacheWrite");
        let fresh_total = input.saturating_add(output).saturating_add(cache_write);
        let counts = TokenCounts {
            input,
            output,
            cache_read,
            cache_write,
            total: if fresh_total == 0 {
                number(raw_usage, "totalTokens")
            } else {
                fresh_total
            },
        };
        usage.total.add(&counts);
        if let Some(timestamp) = timestamp_seconds(value.get("timestamp").and_then(Value::as_str)) {
            usage.timed.push((timestamp, counts));
        }
    }
    Ok(usage)
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn timestamp_seconds(timestamp: Option<&str>) -> Option<i64> {
    DateTime::parse_from_rfc3339(timestamp?)
        .ok()
        .map(|timestamp| timestamp.timestamp())
}

fn session_id(path: &Path) -> Option<String> {
    path.file_stem()?
        .to_str()?
        .rsplit_once('_')
        .map(|(_, id)| id.to_string())
}

fn is_counted_subagent(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name != "__advisor.jsonl")
}

fn subagent_is_complete(path: &Path, usage: &FileUsage) -> bool {
    usage.assistant_records > 0 && (usage.exited || path.with_extension("md").is_file())
}

fn subagent_is_active(path: &Path, usage: &FileUsage) -> bool {
    usage.assistant_records > 0 && !subagent_is_complete(path, usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_session_id_from_omp_path() {
        let path = Path::new("2026-07-18T07-10-31-739Z_019f740f-f93b.jsonl");
        assert_eq!(session_id(path).as_deref(), Some("019f740f-f93b"));
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
    fn discovers_nested_subagent_and_delegation_transcripts() {
        let root = std::env::temp_dir().join(format!(
            "ultraterm-telemetry-delegations-{}",
            std::process::id()
        ));
        let nested = root.join("delegated-task");
        fs::create_dir_all(&nested).unwrap();
        let transcript = nested.join("worker.jsonl");
        fs::write(&transcript, "").unwrap();

        let mut files = HashSet::new();
        collect_jsonl_files(&root, &mut files);

        assert!(files.contains(&transcript));
        assert!(is_counted_subagent(&transcript));
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
    fn rolling_windows_include_only_usage_inside_each_boundary() {
        let now = 2_000_000;
        let mut usage = FileUsage::default();
        let counts = |total| TokenCounts {
            total,
            ..TokenCounts::default()
        };
        usage.timed = vec![
            (now - 60 * 60, counts(1)),
            (now - 2 * 24 * 60 * 60, counts(2)),
            (now - 8 * 24 * 60 * 60, counts(4)),
        ];
        usage.total = counts(7);

        let (past_24_hours, past_7_days, all_time) =
            aggregate_windows(std::iter::once(&usage), now);

        assert_eq!(past_24_hours.total, 1);
        assert_eq!(past_7_days.total, 3);
        assert_eq!(all_time.total, 7);
    }
}
