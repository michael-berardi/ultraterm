use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceTaskReport {
    pub name: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceReport {
    pub schema_version: u32,
    pub status: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
    #[serde(default)]
    pub local_date: Option<String>,
    #[serde(default)]
    pub reclaimed_bytes: u64,
    #[serde(default)]
    pub tasks: Vec<MaintenanceTaskReport>,
}

pub struct MaintenanceManager {
    home: PathBuf,
    latest: Mutex<Option<MaintenanceReport>>,
}

impl MaintenanceManager {
    pub fn new(home: PathBuf) -> Self {
        let latest = load_report(&report_path(&home));
        Self {
            home,
            latest: Mutex::new(latest),
        }
    }

    pub fn snapshot(&self) -> Result<Option<MaintenanceReport>, String> {
        self.latest
            .lock()
            .map(|report| report.clone())
            .map_err(|error| format!("maintenance state lock poisoned: {error}"))
    }

    fn run_if_due(&self) {
        let runner = self.home.join("bin/ultraterm-maintain");
        if !runner.is_file() {
            eprintln!(
                "[ultraterm] daily maintenance unavailable: {} is missing",
                runner.display()
            );
            return;
        }

        let output = match Command::new(&runner).arg("--if-due").output() {
            Ok(output) => output,
            Err(error) => {
                eprintln!("[ultraterm] daily maintenance failed to start: {error}");
                return;
            }
        };
        if !output.status.success() {
            eprintln!(
                "[ultraterm] daily maintenance exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return;
        }

        let report = match parse_report(&output.stdout) {
            Ok(report) => report,
            Err(error) => {
                eprintln!("[ultraterm] daily maintenance report was invalid: {error}");
                return;
            }
        };
        match self.latest.lock() {
            Ok(mut latest) => *latest = Some(report),
            Err(error) => eprintln!("[ultraterm] maintenance state lock poisoned: {error}"),
        }
    }
}

pub fn spawn_scheduler(manager: Arc<MaintenanceManager>) {
    thread::spawn(move || loop {
        manager.run_if_due();
        thread::sleep(CHECK_INTERVAL);
    });
}

fn report_path(home: &Path) -> PathBuf {
    home.join(".ultraterm/maintenance-report.json")
}

fn load_report(path: &Path) -> Option<MaintenanceReport> {
    let bytes = fs::read(path).ok()?;
    parse_report(&bytes).ok()
}

fn parse_report(bytes: &[u8]) -> Result<MaintenanceReport, String> {
    serde_json::from_slice(bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_completed_maintenance_report() {
        let report = parse_report(
            br#"{"schemaVersion":1,"status":"completed","startedAt":"2026-07-21T22:00:00Z","completedAt":"2026-07-21T22:00:01Z","localDate":"2026-07-21","reclaimedBytes":4096,"tasks":[{"name":"staleNpx","status":"completed"}]}"#,
        )
        .unwrap();

        assert_eq!(report.status, "completed");
        assert_eq!(report.reclaimed_bytes, 4096);
        assert_eq!(report.tasks.len(), 1);
    }

    #[test]
    fn parses_not_due_report_without_optional_timestamps() {
        let report =
            parse_report(br#"{"schemaVersion":1,"status":"notDue","tasks":[],"reclaimedBytes":0}"#)
                .unwrap();

        assert_eq!(report.status, "notDue");
        assert!(report.started_at.is_none());
    }
}
