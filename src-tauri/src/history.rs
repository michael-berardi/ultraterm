use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::time::Duration;

use rusqlite::{params, Connection, OpenFlags, Row};
use serde::Serialize;

const DEFAULT_RESULT_LIMIT: u32 = 20;
const MAX_RESULT_LIMIT: u32 = 50;
const MAX_QUERY_CHARS: usize = 256;
const PREVIEW_CHARS: i64 = 800;
static DISCOVERED_OMP_AGENT_DIRECTORY: LazyLock<Option<PathBuf>> =
    LazyLock::new(discover_omp_agent_directory);

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: i64,
    pub prompt: String,
    pub created_at: i64,
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    pub truncated: bool,
}

fn nonempty_env(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

fn omp_agent_directory_from(
    home: &Path,
    agent_directory: Option<&std::ffi::OsStr>,
    profile: Option<&std::ffi::OsStr>,
    discovered: Option<&Path>,
) -> PathBuf {
    if let Some(agent_directory) = agent_directory {
        return PathBuf::from(agent_directory);
    }
    if let Some(discovered) = discovered {
        return discovered.to_path_buf();
    }

    let omp_directory = home.join(".omp");
    match profile {
        Some(profile) => omp_directory.join("profiles").join(profile).join("agent"),
        None => omp_directory.join("agent"),
    }
}
fn omp_config_path_arguments(profile: Option<&OsStr>) -> Vec<OsString> {
    let mut arguments = Vec::with_capacity(3);
    if let Some(profile) = profile {
        let mut profile_argument = OsString::from("--profile=");
        profile_argument.push(profile);
        arguments.push(profile_argument);
    }
    arguments.push(OsString::from("config"));
    arguments.push(OsString::from("path"));
    arguments
}

fn discover_omp_agent_directory() -> Option<PathBuf> {
    let omp = crate::resolve_optional_executable("OMP_BIN", "omp", &[])
        .ok()
        .flatten()?;
    let profile = nonempty_env("OMP_PROFILE");
    let output = Command::new(omp)
        .args(omp_config_path_arguments(profile.as_deref()))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let output = String::from_utf8(output.stdout).ok()?;
    let path = PathBuf::from(output.trim());
    (!path.as_os_str().is_empty()).then_some(path)
}

pub(crate) fn omp_agent_directory(home: &Path) -> PathBuf {
    let agent_directory = nonempty_env("PI_CODING_AGENT_DIR");
    let profile = nonempty_env("OMP_PROFILE");
    let discovered = agent_directory
        .is_none()
        .then(|| DISCOVERED_OMP_AGENT_DIRECTORY.as_deref());
    omp_agent_directory_from(
        home,
        agent_directory.as_deref(),
        profile.as_deref(),
        discovered.flatten(),
    )
}

pub fn history_database_path(home: &Path) -> PathBuf {
    nonempty_env("OMP_HISTORY_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| omp_agent_directory(home).join("history.db"))
}

pub fn query_history(
    home: &Path,
    query: &str,
    limit: Option<u32>,
) -> Result<Vec<HistoryEntry>, String> {
    query_history_database(&history_database_path(home), query, limit)
}

fn query_history_database(
    database_path: &Path,
    query: &str,
    limit: Option<u32>,
) -> Result<Vec<HistoryEntry>, String> {
    let query = query.trim();
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(format!(
            "History query exceeds {MAX_QUERY_CHARS} characters"
        ));
    }
    if !database_path.is_file() {
        return Err(format!(
            "OMP history database not found at {}. Set OMP_HISTORY_DB to the database path, or configure PI_CODING_AGENT_DIR/OMP_PROFILE to match OMP.",
            database_path.display()
        ));
    }

    let connection = Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("Unable to open OMP history: {error}"))?;
    connection
        .busy_timeout(Duration::from_millis(250))
        .map_err(|error| format!("Unable to configure OMP history query: {error}"))?;

    let limit = i64::from(
        limit
            .unwrap_or(DEFAULT_RESULT_LIMIT)
            .clamp(1, MAX_RESULT_LIMIT),
    );
    if query.is_empty() {
        let mut statement = connection
            .prepare(
                "SELECT id, substr(prompt, 1, ?1), created_at, cwd, session_id, \
                 CASE WHEN length(prompt) > ?1 THEN 1 ELSE 0 END \
                 FROM history ORDER BY created_at DESC, id DESC LIMIT ?2",
            )
            .map_err(|error| format!("Unable to prepare recent history query: {error}"))?;
        let rows = statement
            .query_map(params![PREVIEW_CHARS, limit], map_history_row)
            .map_err(|error| format!("Unable to query recent OMP history: {error}"))?;
        return rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| format!("Unable to read recent OMP history: {error}"));
    }

    let fts_query = query
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ");
    let mut statement = connection
        .prepare(
            "SELECT h.id, substr(h.prompt, 1, ?1), h.created_at, h.cwd, h.session_id, \
             CASE WHEN length(h.prompt) > ?1 THEN 1 ELSE 0 END \
             FROM history_fts \
             JOIN history h ON h.id = history_fts.rowid \
             WHERE history_fts MATCH ?2 \
             ORDER BY bm25(history_fts), h.created_at DESC \
             LIMIT ?3",
        )
        .map_err(|error| format!("Unable to prepare OMP history search: {error}"))?;
    let rows = statement
        .query_map(params![PREVIEW_CHARS, fts_query, limit], map_history_row)
        .map_err(|error| format!("Unable to search OMP history: {error}"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("Unable to read OMP history results: {error}"))
}

fn map_history_row(row: &Row<'_>) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry {
        id: row.get(0)?,
        prompt: row.get(1)?,
        created_at: row.get(2)?,
        cwd: row.get(3)?,
        session_id: row.get(4)?,
        truncated: row.get::<_, i64>(5)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn seed(connection: &Connection) {
        connection.execute_batch(
            "CREATE VIRTUAL TABLE history_fts USING fts5(prompt);
             CREATE TABLE history(id INTEGER PRIMARY KEY, prompt TEXT NOT NULL, created_at INTEGER, cwd TEXT, session_id TEXT);
             INSERT INTO history(prompt, created_at, cwd, session_id) VALUES('fix login router', 1000, '/a', 's1');
             INSERT INTO history(prompt, created_at, cwd, session_id) VALUES('test auth middleware', 2000, '/b', 's2');
             INSERT INTO history(prompt, created_at, cwd, session_id) VALUES('deploy staging', 3000, '/c', 's3');
             INSERT INTO history_fts(rowid, prompt) SELECT id, prompt FROM history;"
        ).unwrap();
    }

    #[test]
    fn discovered_agent_directory_matches_omp_default_selection() {
        let home = Path::new("/home/user");
        let discovered = Path::new("/custom/omp/agent");
        assert_eq!(
            omp_agent_directory_from(home, None, None, Some(discovered)),
            discovered
        );
    }
    #[test]
    fn config_path_arguments_preserve_explicit_profile_selection() {
        assert_eq!(
            omp_config_path_arguments(Some(OsStr::new("team"))),
            vec![
                OsString::from("--profile=team"),
                OsString::from("config"),
                OsString::from("path")
            ]
        );
        assert_eq!(
            omp_config_path_arguments(None),
            vec![OsString::from("config"), OsString::from("path")]
        );
    }

    #[test]
    fn global_agent_directory_is_the_discovery_fallback() {
        let home = Path::new("/home/user");
        assert_eq!(
            omp_agent_directory_from(home, None, None, None),
            home.join(".omp").join("agent")
        );
    }

    #[test]
    fn profile_agent_directory_is_the_discovery_fallback() {
        let home = Path::new("/home/user");
        assert_eq!(
            omp_agent_directory_from(home, None, Some(std::ffi::OsStr::new("team")), None),
            home.join(".omp")
                .join("profiles")
                .join("team")
                .join("agent")
        );
    }

    #[test]
    fn explicit_agent_directory_takes_precedence() {
        let home = Path::new("/home/user");
        let configured = std::ffi::OsStr::new("/var/lib/omp-agent");
        assert_eq!(
            omp_agent_directory_from(
                home,
                Some(configured),
                Some(std::ffi::OsStr::new("team")),
                Some(Path::new("/discovered/agent"))
            ),
            PathBuf::from(configured)
        );
    }

    #[test]
    fn empty_query_returns_recent_first() {
        let mut file = NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();
        let conn = Connection::open(&path).unwrap();
        seed(&conn);
        drop(conn);
        file.flush().unwrap();

        let rows = query_history_database(&path, "", Some(2)).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].created_at, 3000);
        assert_eq!(rows[1].created_at, 2000);
    }

    #[test]
    fn fts_query_matches_term() {
        let mut file = NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();
        let conn = Connection::open(&path).unwrap();
        seed(&conn);
        drop(conn);
        file.flush().unwrap();

        let rows = query_history_database(&path, "auth", None).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].prompt.contains("auth"));
    }

    #[test]
    fn missing_database_returns_error() {
        let path = PathBuf::from("/nonexistent/history.db");
        let result = query_history_database(&path, "", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn long_query_rejected() {
        let query = "x".repeat(MAX_QUERY_CHARS + 1);
        let mut file = NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();
        let conn = Connection::open(&path).unwrap();
        seed(&conn);
        drop(conn);
        file.flush().unwrap();
        assert!(query_history_database(&path, &query, None).is_err());
    }
}
