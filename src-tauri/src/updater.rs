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
use std::sync::OnceLock;
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
const EXPECTED_BUNDLE_ID: &str = "com.libertydesignstudio.ultraterm";
const EXPECTED_TEAM_ID: &str = "T63VT9UAY2";
static INSTALL_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

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
    if payload
        .get("draft")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
        || payload
            .get("prerelease")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true)
    {
        return Err("GitHub did not return a stable release.".to_string());
    }
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
    let latest = Version::parse(&latest_version)
        .ok_or_else(|| "Latest release is not stable semantic versioning.".to_string())?;
    let current = Version::parse(&current_version)
        .ok_or_else(|| "Current app version is not stable semantic versioning.".to_string())?;
    let update_available = latest > current;
    if update_available {
        let assets = payload
            .get("assets")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "Stable release metadata is missing assets.".to_string())?;
        for required in [ARCHIVE_NAME.to_string(), format!("{ARCHIVE_NAME}.sha256")] {
            if !assets.iter().any(|asset| {
                asset.get("name").and_then(serde_json::Value::as_str) == Some(required.as_str())
            }) {
                return Err(format!(
                    "Stable release is missing required asset {required}."
                ));
            }
        }
    }
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

#[derive(Debug, Default, Eq, PartialEq)]
struct SignatureIdentity {
    identifier: Option<String>,
    team_identifier: Option<String>,
    has_developer_id_authority: bool,
    has_hardened_runtime: bool,
    is_adhoc: bool,
}

fn parse_signature_details(details: &str) -> SignatureIdentity {
    let mut identity = SignatureIdentity::default();
    for line in details.lines() {
        if let Some(value) = line.strip_prefix("Identifier=") {
            identity.identifier = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("TeamIdentifier=") {
            identity.team_identifier = Some(value.trim().to_string());
        } else if line.starts_with("Authority=Developer ID Application:") {
            identity.has_developer_id_authority = true;
        }
        if line.contains("flags=") && line.contains("runtime") {
            identity.has_hardened_runtime = true;
        }
        if line.contains("(adhoc)") || line.contains("Signature=adhoc") {
            identity.is_adhoc = true;
        }
    }
    identity
}

fn designated_requirement_matches(requirements: &str) -> bool {
    let team_unquoted = format!("OU] = {EXPECTED_TEAM_ID}");
    let team_quoted = format!("OU] = \"{EXPECTED_TEAM_ID}\"");
    requirements.contains("designated =>")
        && requirements.contains(&format!("identifier \"{EXPECTED_BUNDLE_ID}\""))
        && requirements.contains("anchor apple generic")
        && requirements.contains("certificate")
        && (requirements.contains(&team_unquoted) || requirements.contains(&team_quoted))
}

fn validate_signature_identity(details: &str, requirements: &str) -> Result<(), String> {
    let identity = parse_signature_details(details);
    if identity.identifier.as_deref() != Some(EXPECTED_BUNDLE_ID) {
        return Err(format!(
            "Update signature has unexpected bundle identifier (expected {EXPECTED_BUNDLE_ID})."
        ));
    }
    if identity.team_identifier.as_deref() != Some(EXPECTED_TEAM_ID) {
        return Err(format!(
            "Update signature has unexpected Developer Team (expected {EXPECTED_TEAM_ID})."
        ));
    }
    if identity.is_adhoc || !identity.has_developer_id_authority {
        return Err("Update must use a non-ad-hoc Developer ID Application signature.".to_string());
    }
    if !identity.has_hardened_runtime {
        return Err("Update must use the hardened runtime.".to_string());
    }
    if !designated_requirement_matches(requirements) {
        return Err("Update signature has an unexpected designated requirement.".to_string());
    }
    Ok(())
}

fn command_output(program: &str, args: &[&str], what: &str) -> Result<String, String> {
    let output = process::Command::new(program)
        .args(args)
        .stdin(process::Stdio::null())
        .output()
        .map_err(|error| format!("Failed to {what}: {error}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        Ok(text)
    } else {
        Err(format!("Failed to {what} (exit {})", output.status))
    }
}

fn validate_app_bundle_name(app: &Path) -> Result<(), String> {
    if app.file_name().and_then(|name| name.to_str()) == Some("UltraTerm.app") {
        Ok(())
    } else {
        Err("Update archive contained an unexpected app bundle name.".to_string())
    }
}

fn verify_app_identity(app: &Path, expected_version: &str) -> Result<(), String> {
    if !app.is_dir() {
        return Err(format!(
            "Expected an UltraTerm.app bundle at {}.",
            app.display()
        ));
    }
    validate_app_bundle_name(app)?;
    let info = app.join("Contents/Info.plist");
    let info_arg = info.to_string_lossy().into_owned();
    let plist_identifier = command_output(
        "/usr/libexec/PlistBuddy",
        &["-c", "Print :CFBundleIdentifier", info_arg.as_str()],
        "inspect the update bundle identifier",
    )?;
    if plist_identifier.trim() != EXPECTED_BUNDLE_ID {
        return Err("Update Info.plist has an unexpected bundle identifier.".to_string());
    }
    let version = command_output(
        "/usr/libexec/PlistBuddy",
        &["-c", "Print :CFBundleShortVersionString", info_arg.as_str()],
        "inspect the update version",
    )?;
    if version.trim() != expected_version {
        return Err(format!(
            "Update version {} does not match expected {expected_version}.",
            version.trim()
        ));
    }
    let app_arg = app.to_string_lossy().into_owned();
    run_checked(
        "/usr/bin/codesign",
        &["--verify", "--deep", "--strict", app_arg.as_str()],
        "verify the update's sealed resources",
    )?;
    let details = command_output(
        "/usr/bin/codesign",
        &["-dv", "--verbose=4", app_arg.as_str()],
        "inspect the update signature",
    )?;
    let requirements = command_output(
        "/usr/bin/codesign",
        &["-d", "-r-", app_arg.as_str()],
        "inspect the update designated requirement",
    )?;
    validate_signature_identity(&details, &requirements)?;
    run_checked(
        "/usr/sbin/spctl",
        &["--assess", "--type", "execute", app_arg.as_str()],
        "verify the update's notarization",
    )
}

#[tauri::command(async)]
pub async fn install_app_update(app: AppHandle) -> Result<(), String> {
    let _install_guard = INSTALL_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .try_lock()
        .map_err(|_| "An UltraTerm update is already being installed.".to_string())?;
    let update = check_app_update().await?;
    if !update.update_available {
        return Err("No newer stable UltraTerm release is available.".to_string());
    }
    let bundle = current_bundle()?;
    verify_app_identity(&bundle, env!("CARGO_PKG_VERSION"))?;
    let staging = std::env::temp_dir().join(format!("ultraterm-update-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&staging).map_err(|error| format!("Failed to stage update: {error}"))?;

    let result = stage_update(&staging, &update.latest_version).await;
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    let unpacked_app = result?;

    // The swap happens only after this process exits: the helper waits on the
    // PID, moves the current bundle aside, installs the verified copy, and
    // relaunches. The backup remains until a *new* app PID is observed and
    // stays alive for five seconds; failed launch/health checks restore it.
    // Terminal sessions stay alive in tmux throughout.
    let pid = process::id();
    let backup = bundle.with_file_name(format!(".UltraTerm.previous-{}.app", process::id()));
    let script = format!(
        "set -u; \
         verify_identity() {{ \
         /usr/bin/codesign --verify --deep --strict \"$1\" >/dev/null 2>&1 || return 1; \
         details=\"$(/usr/bin/codesign -dv --verbose=4 \"$1\" 2>&1)\" || return 1; \
         case \"$details\" in *\"Identifier={bundle_id}\"*) ;; *) return 1 ;; esac; \
         case \"$details\" in *\"TeamIdentifier={team_id}\"*) ;; *) return 1 ;; esac; \
         case \"$details\" in *\"Authority=Developer ID Application:\"*\"{team_id}\"*) ;; *) return 1 ;; esac; \
         requirements=\"$(/usr/bin/codesign -d -r- \"$1\" 2>&1)\" || return 1; \
         case \"$details\" in *flags=*runtime*) ;; *) return 1 ;; esac; \
         case \"$requirements\" in *'designated =>'*) ;; *) return 1 ;; esac; \
         case \"$requirements\" in *'identifier \"{bundle_id}\"'*) ;; *) return 1 ;; esac; \
         case \"$requirements\" in *'anchor apple generic'*) ;; *) return 1 ;; esac; \
         case \"$requirements\" in *certificate*OU*{team_id}*) ;; *) return 1 ;; esac; \
         /usr/sbin/spctl --assess --type execute \"$1\" >/dev/null 2>&1 || return 1; \
         }}; \
         rollback() {{ \
         /bin/rm -rf {target}; \
         /bin/mv {backup} {target} || exit 1; \
         /usr/bin/open -n {target} || true; \
         /bin/rm -rf {staging}; \
         exit 1; \
         }}; \
         while kill -0 {pid} 2>/dev/null; do /bin/sleep 0.2; done; \
         if ! verify_identity {source}; then /bin/rm -rf {staging}; exit 1; fi; \
         /bin/rm -rf {backup}; \
         if ! /bin/mv {target} {backup}; then /bin/rm -rf {staging}; exit 1; fi; \
         if ! /usr/bin/ditto {source} {target}; then rollback; fi; \
         if ! verify_identity {target}; then rollback; fi; \
         /usr/bin/xattr -dr com.apple.quarantine {target} 2>/dev/null || true; \
         existing_pids=\"$(/usr/bin/pgrep -f {target}/Contents/MacOS/ || true)\"; \
         if ! /usr/bin/open -n {target}; then rollback; fi; \
         new_pid=\"\"; \
         for _ in $(/usr/bin/seq 1 50); do \
           for candidate in $(/usr/bin/pgrep -f {target}/Contents/MacOS/ || true); do \
             if [ \"$candidate\" = \"$$\" ]; then continue; fi; \
             case \" $existing_pids \" in *\" $candidate \"*) ;; *) new_pid=\"$candidate\"; break ;; esac; \
           done; \
           [ -n \"$new_pid\" ] && break; \
           /bin/sleep 0.2; \
         done; \
         [ -n \"$new_pid\" ] || rollback; \
         for _ in $(/usr/bin/seq 1 25); do \
           kill -0 \"$new_pid\" 2>/dev/null || rollback; \
           /bin/sleep 0.2; \
         done; \
         /bin/rm -rf {backup} {staging}",
        bundle_id = EXPECTED_BUNDLE_ID,
        team_id = EXPECTED_TEAM_ID,
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
/// SHA-256 checksum and the expected non-ad-hoc Developer ID identity, and
/// returns the unpacked app path.
async fn stage_update(staging: &Path, expected_version: &str) -> Result<PathBuf, String> {
    let archive = staging.join(ARCHIVE_NAME);
    let checksum = staging.join(format!("{ARCHIVE_NAME}.sha256"));
    download(&format!("{DOWNLOAD_BASE_URL}/{ARCHIVE_NAME}"), &archive).await?;
    download(
        &format!("{DOWNLOAD_BASE_URL}/{ARCHIVE_NAME}.sha256"),
        &checksum,
    )
    .await?;

    // The published checksum file is in `shasum` format; verify from inside
    // the staging directory so the embedded relative filename resolves.
    let checksum_arg = checksum.to_string_lossy().into_owned();
    let status = process::Command::new("/usr/bin/shasum")
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
        "/usr/bin/ditto",
        &["-x", "-k", archive_arg.as_str(), unpacked_arg.as_str()],
        "unpack the update archive",
    )?;
    let unpacked_app = unpacked_dir
        .join("UltraTerm-macos-arm64")
        .join("UltraTerm.app");
    if !unpacked_app.is_dir() {
        return Err("Update archive did not contain UltraTerm.app.".to_string());
    }
    verify_app_identity(&unpacked_app, expected_version)?;
    Ok(unpacked_app)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_DETAILS: &str = concat!(
        "Identifier=com.libertydesignstudio.ultraterm\n",
        "TeamIdentifier=T63VT9UAY2\n",
        "Authority=Developer ID Application: Michael Berardi (T63VT9UAY2)\n",
        "CodeDirectory v=20500 flags=0x10000(runtime)\n",
    );
    const VALID_REQUIREMENTS: &str = concat!(
        "designated => identifier \"com.libertydesignstudio.ultraterm\" ",
        "and anchor apple generic and certificate leaf[subject.OU] = T63VT9UAY2\n",
    );

    #[test]
    fn accepts_expected_developer_id_identity() {
        assert!(validate_signature_identity(VALID_DETAILS, VALID_REQUIREMENTS).is_ok());
    }

    #[test]
    fn accepts_quoted_developer_team_requirement() {
        let requirements = VALID_REQUIREMENTS.replace("OU] = T63VT9UAY2", "OU] = \"T63VT9UAY2\"");
        assert!(validate_signature_identity(VALID_DETAILS, &requirements).is_ok());
    }

    #[test]
    fn rejects_wrong_bundle_identifier() {
        let details = VALID_DETAILS.replace(EXPECTED_BUNDLE_ID, "com.example.other");
        let error = validate_signature_identity(&details, VALID_REQUIREMENTS).unwrap_err();
        assert!(error.contains("bundle identifier"));
    }

    #[test]
    fn rejects_wrong_developer_team() {
        let details = VALID_DETAILS.replace(EXPECTED_TEAM_ID, "OTHERTEAM1");
        let error = validate_signature_identity(&details, VALID_REQUIREMENTS).unwrap_err();
        assert!(error.contains("Developer Team"));
    }

    #[test]
    fn rejects_ad_hoc_signature() {
        let details = concat!(
            "Identifier=com.libertydesignstudio.ultraterm\n",
            "TeamIdentifier=T63VT9UAY2\n",
            "CodeDirectory flags=0x2(adhoc)\n",
        );
        let error = validate_signature_identity(details, VALID_REQUIREMENTS).unwrap_err();
        assert!(error.contains("non-ad-hoc"));
    }

    #[test]
    fn rejects_unexpected_designated_requirement() {
        let requirements = VALID_REQUIREMENTS.replace("anchor apple generic", "anchor apple");
        let error = validate_signature_identity(VALID_DETAILS, &requirements).unwrap_err();
        assert!(error.contains("designated requirement"));
    }

    #[test]
    fn rejects_missing_hardened_runtime() {
        let details = VALID_DETAILS.replace("CodeDirectory v=20500 flags=0x10000(runtime)\n", "");
        let error = validate_signature_identity(&details, VALID_REQUIREMENTS).unwrap_err();
        assert!(error.contains("hardened runtime"));
    }

    #[test]
    fn rejects_unexpected_app_bundle_name() {
        let error = validate_app_bundle_name(Path::new("/tmp/Impostor.app")).unwrap_err();
        assert!(error.contains("bundle name"));
        assert!(validate_app_bundle_name(Path::new("/tmp/UltraTerm.app")).is_ok());
    }

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
