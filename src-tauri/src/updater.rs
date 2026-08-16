//! GitHub Releases self-updater.
//!
//! `check_app_update` compares the running bundle version against the latest
//! published release. `install_app_update` downloads the signed archive,
//! verifies its SHA-256 checksum and code signature, then hands the swap to a
//! detached helper script that waits for this process to exit before replacing
//! the bundle and relaunching it — the same exit-and-reattach contract as
//! `restart_app`, so tmux-backed terminal sessions survive the update.

use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;

use serde::Serialize;
use tauri::AppHandle;

const RELEASE_API_URL: &str =
    "https://api.github.com/repos/michael-berardi/ultraterm/releases/latest";
const DOWNLOAD_BASE_URL: &str =
    "https://github.com/michael-berardi/ultraterm/releases/latest/download";
const ARCHIVE_NAME: &str = "UltraTerm-macos-arm64.zip";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateStatus {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_url: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Version(u64, u64, u64);

impl Version {
    fn parse(text: &str) -> Option<Self> {
        let trimmed = text.trim().trim_start_matches('v');
        let mut parts = trimmed.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self(major, minor, patch))
    }
}

pub(crate) fn http_client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("ultraterm-updater/", env!("CARGO_PKG_VERSION")))
        .timeout(timeout)
        .build()
        .map_err(|error| format!("Failed to create update client: {error}"))
}

fn current_bundle() -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("Failed to resolve UltraTerm executable: {error}"))?;
    exe.ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_path_buf)
        .ok_or_else(|| "Updates require an installed UltraTerm.app bundle.".to_string())
}

#[tauri::command(async)]
pub async fn check_app_update() -> Result<AppUpdateStatus, String> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let response = http_client(HTTP_TIMEOUT)?
        .get(RELEASE_API_URL)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| format!("Failed to reach GitHub releases: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "GitHub releases returned HTTP {}",
            response.status()
        ));
    }
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|error| format!("Failed to read release metadata: {error}"))?;
    let tag = payload
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Release metadata is missing tag_name.".to_string())?;
    let release_url = payload
        .get("html_url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(DOWNLOAD_BASE_URL)
        .to_string();
    let latest_version = tag.trim().trim_start_matches('v').to_string();
    let update_available = match (Version::parse(&latest_version), Version::parse(&current_version))
    {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    };
    Ok(AppUpdateStatus {
        current_version,
        latest_version,
        update_available,
        release_url,
    })
}

async fn download(url: &str, target: &Path) -> Result<(), String> {
    let bytes = http_client(DOWNLOAD_TIMEOUT)?
        .get(url)
        .send()
        .await
        .map_err(|error| format!("Download failed for {url}: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Download failed for {url}: {error}"))?
        .bytes()
        .await
        .map_err(|error| format!("Download failed for {url}: {error}"))?;
    std::fs::write(target, &bytes)
        .map_err(|error| format!("Failed to write {}: {error}", target.display()))
}

fn run_checked(program: &str, args: &[&str], what: &str) -> Result<(), String> {
    let status = process::Command::new(program)
        .args(args)
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::piped())
        .status()
        .map_err(|error| format!("Failed to {what}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Failed to {what} (exit {status})"))
    }
}

#[tauri::command(async)]
pub async fn install_app_update(app: AppHandle) -> Result<(), String> {
    let bundle = current_bundle()?;

    let staging = std::env::temp_dir().join(format!("ultraterm-update-{}", process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|error| format!("Failed to stage update: {error}"))?;

    let result = stage_update(&staging).await;
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    let unpacked_app = result?;

    // The swap happens only after this process exits: the helper waits on the
    // PID, moves the current bundle aside, installs the verified copy, and
    // relaunches. If the copy fails, the previous bundle is restored so the
    // app is never left missing. Terminal sessions stay alive in tmux
    // throughout.
    let pid = process::id();
    let backup = staging.join("UltraTerm.previous.app");
    let script = format!(
        "while kill -0 {pid} 2>/dev/null; do sleep 0.2; done; \
         mv {target} {backup} \
         && {{ ditto {source} {target} || {{ rm -rf {target}; mv {backup} {target}; }}; }} \
         || mv {backup} {target}; \
         xattr -dr com.apple.quarantine {target} 2>/dev/null; \
         open -n {target}; \
         rm -rf {staging}",
        target = crate::shell_escape(&bundle),
        backup = crate::shell_escape(&backup),
        source = crate::shell_escape(&unpacked_app),
        staging = crate::shell_escape(&staging),
    );
    process::Command::new("/bin/sh")
        .args(["-c", &script])
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null())
        .spawn()
        .map_err(|error| format!("Failed to schedule update install: {error}"))?;
    app.exit(0);
    Ok(())
}

/// Downloads the archive and its checksum into `staging`, verifies both the
/// SHA-256 checksum and the code signature, and returns the unpacked app path.
async fn stage_update(staging: &Path) -> Result<PathBuf, String> {
    let archive = staging.join(ARCHIVE_NAME);
    let checksum = staging.join(format!("{ARCHIVE_NAME}.sha256"));
    download(
        &format!("{DOWNLOAD_BASE_URL}/{ARCHIVE_NAME}"),
        &archive,
    )
    .await?;
    download(
        &format!("{DOWNLOAD_BASE_URL}/{ARCHIVE_NAME}.sha256"),
        &checksum,
    )
    .await?;

    // The published checksum file is in `shasum` format; verify from inside
    // the staging directory so the embedded relative filename resolves.
    let checksum_arg = checksum.to_string_lossy().into_owned();
    let status = process::Command::new("shasum")
        .args(["-a", "256", "--check", checksum_arg.as_str()])
        .current_dir(staging)
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null())
        .status()
        .map_err(|error| format!("Failed to verify update checksum: {error}"))?;
    if !status.success() {
        return Err("Update archive failed its SHA-256 checksum.".to_string());
    }

    let archive_arg = archive.to_string_lossy().into_owned();
    let unpacked_dir = staging.join("unpacked");
    let unpacked_arg = unpacked_dir.to_string_lossy().into_owned();
    run_checked(
        "ditto",
        &["-x", "-k", archive_arg.as_str(), unpacked_arg.as_str()],
        "unpack the update archive",
    )?;
    let unpacked_app = unpacked_dir
        .join("UltraTerm-macos-arm64")
        .join("UltraTerm.app");
    if !unpacked_app.is_dir() {
        return Err("Update archive did not contain UltraTerm.app.".to_string());
    }
    let app_arg = unpacked_app.to_string_lossy().into_owned();
    run_checked(
        "codesign",
        &["--verify", "--deep", "--strict", app_arg.as_str()],
        "verify the update signature",
    )?;
    Ok(unpacked_app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_semver_tags() {
        assert_eq!(Version::parse("v0.3.2"), Some(Version(0, 3, 2)));
        assert_eq!(Version::parse("1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse(" 0.4.0 "), Some(Version(0, 4, 0)));
        assert_eq!(Version::parse("0.3"), None);
        assert_eq!(Version::parse("0.3.2.1"), None);
        assert_eq!(Version::parse("beta"), None);
    }

    #[test]
    fn orders_versions_numerically() {
        assert!(Version(0, 4, 0) > Version(0, 3, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        assert!(Version(0, 3, 2) == Version(0, 3, 2));
    }
}
