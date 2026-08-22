use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process;
use std::sync::{LazyLock, Mutex, MutexGuard};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OmpProfileInfo {
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateOmpProfileRequest {
    pub name: String,
    pub model: String,
    pub thinking_level: String,
    #[serde(default)]
    pub title_model: Option<String>,
}

const MAX_MODEL_CHARS: usize = 256;
const VALID_THINKING_LEVELS: &[&str] = &[
    "off", "minimal", "low", "medium", "high", "xhigh", "max", "auto",
];

#[cfg(target_os = "macos")]
mod fd {
    use std::ffi::{CStr, CString};
    use std::fs::File;
    use std::io;
    use std::mem::MaybeUninit;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn cstring(bytes: &[u8]) -> io::Result<CString> {
        CString::new(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
    }

    fn last_errno() -> io::Error {
        io::Error::last_os_error()
    }

    pub(crate) struct Dir(OwnedFd);

    impl Dir {
        pub(crate) fn open_path(path: &Path) -> io::Result<Self> {
            let path = cstring(path.as_os_str().as_bytes())?;
            let fd = unsafe {
                libc::open(
                    path.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(last_errno());
            }
            Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
        }

        pub(crate) fn open_at(&self, name: &[u8]) -> io::Result<Self> {
            let name = cstring(name)?;
            let fd = unsafe {
                libc::openat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(last_errno());
            }
            Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
        }

        pub(crate) fn mkdir_at(&self, name: &[u8], mode: u32) -> io::Result<()> {
            let name = cstring(name)?;
            if unsafe { libc::mkdirat(self.0.as_raw_fd(), name.as_ptr(), mode as libc::mode_t) }
                < 0
            {
                return Err(last_errno());
            }
            Ok(())
        }

        pub(crate) fn fchmod(&self, mode: u32) -> io::Result<()> {
            if unsafe { libc::fchmod(self.0.as_raw_fd(), mode as libc::mode_t) } < 0 {
                return Err(last_errno());
            }
            Ok(())
        }

        pub(crate) fn open_file(&self, name: &[u8], mode: u32) -> io::Result<File> {
            let name = cstring(name)?;
            let fd = unsafe {
                libc::openat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    mode as libc::c_uint,
                )
            };
            if fd < 0 {
                return Err(last_errno());
            }
            if unsafe { libc::fchmod(fd, mode as libc::mode_t) } < 0 {
                let error = last_errno();
                unsafe {
                    libc::close(fd);
                }
                return Err(error);
            }
            Ok(unsafe { File::from_raw_fd(fd) })
        }

        pub(crate) fn symlink_at(&self, target: &[u8], name: &[u8]) -> io::Result<()> {
            let target = cstring(target)?;
            let name = cstring(name)?;
            if unsafe { libc::symlinkat(target.as_ptr(), self.0.as_raw_fd(), name.as_ptr()) } < 0 {
                return Err(last_errno());
            }
            Ok(())
        }

        pub(crate) fn is_dir_at(&self, name: &[u8]) -> io::Result<bool> {
            let name = cstring(name)?;
            let mut stat = MaybeUninit::<libc::stat>::zeroed();
            if unsafe {
                libc::fstatat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    stat.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } < 0
            {
                return Err(last_errno());
            }
            let stat = unsafe { stat.assume_init() };
            Ok((stat.st_mode as libc::mode_t & libc::S_IFMT) == libc::S_IFDIR)
        }

        pub(crate) fn unlink_at(&self, name: &[u8], flags: i32) -> io::Result<()> {
            let name = cstring(name)?;
            if unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), flags) } < 0 {
                return Err(last_errno());
            }
            Ok(())
        }

        pub(crate) fn names(&self) -> io::Result<Vec<Vec<u8>>> {
            let duplicate = unsafe { libc::fcntl(self.0.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
            if duplicate < 0 {
                return Err(last_errno());
            }
            let directory = unsafe { libc::fdopendir(duplicate) };
            if directory.is_null() {
                unsafe {
                    libc::close(duplicate);
                }
                return Err(last_errno());
            }
            let mut names = Vec::new();
            loop {
                unsafe {
                    *libc::__error() = 0;
                }
                let entry = unsafe { libc::readdir(directory) };
                if entry.is_null() {
                    let error = unsafe { *libc::__error() };
                    let close_result = unsafe { libc::closedir(directory) };
                    if error != 0 {
                        return Err(io::Error::from_raw_os_error(error));
                    }
                    if close_result != 0 {
                        return Err(last_errno());
                    }
                    break;
                }
                let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }
                    .to_bytes()
                    .to_vec();
                if name != b"." && name != b".." {
                    names.push(name);
                }
            }
            Ok(names)
        }
    }

    pub(crate) fn rename_exclusive(
        parent: &Dir,
        from: &[u8],
        to: &[u8],
    ) -> io::Result<()> {
        let from = cstring(from)?;
        let to = cstring(to)?;
        if unsafe {
            libc::renameatx_np(
                parent.0.as_raw_fd(),
                from.as_ptr(),
                parent.0.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        } < 0
        {
            return Err(last_errno());
        }
        Ok(())
    }

    pub(crate) const REMOVE_DIR: i32 = libc::AT_REMOVEDIR;
}

#[cfg(not(target_os = "macos"))]
mod fd {
    use std::fs::File;
    use std::io;
    use std::path::Path;

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "fd-bound OMP profile management is only available on macOS",
        )
    }

    pub(crate) struct Dir;

    impl Dir {
        pub(crate) fn open_path(_: &Path) -> io::Result<Self> {
            Err(unsupported())
        }
        pub(crate) fn open_at(&self, _: &[u8]) -> io::Result<Self> {
            Err(unsupported())
        }
        pub(crate) fn mkdir_at(&self, _: &[u8], _: u32) -> io::Result<()> {
            Err(unsupported())
        }
        pub(crate) fn fchmod(&self, _: u32) -> io::Result<()> {
            Err(unsupported())
        }
        pub(crate) fn open_file(&self, _: &[u8], _: u32) -> io::Result<File> {
            Err(unsupported())
        }
        pub(crate) fn symlink_at(&self, _: &[u8], _: &[u8]) -> io::Result<()> {
            Err(unsupported())
        }
        pub(crate) fn is_dir_at(&self, _: &[u8]) -> io::Result<bool> {
            Err(unsupported())
        }
        pub(crate) fn unlink_at(&self, _: &[u8], _: i32) -> io::Result<()> {
            Err(unsupported())
        }
        pub(crate) fn names(&self) -> io::Result<Vec<Vec<u8>>> {
            Err(unsupported())
        }
    }

    pub(crate) fn rename_exclusive(_: &Dir, _: &[u8], _: &[u8]) -> io::Result<()> {
        Err(unsupported())
    }

    pub(crate) const REMOVE_DIR: i32 = 0;
}

fn profiles_root(home: &Path) -> PathBuf {
    home.join(".omp").join("profiles")
}

fn root_for_current_user() -> Result<PathBuf, String> {
    Ok(profiles_root(&crate::home_dir()?))
}

#[cfg(target_os = "macos")]
fn canonical_root(root: &Path, create: bool) -> Result<Option<fd::Dir>, String> {
    let mut current = if root.is_absolute() {
        fd::Dir::open_path(Path::new("/"))
    } else {
        fd::Dir::open_path(Path::new("."))
    }
    .map_err(|error| format!("open profile root {}: {error}", root.display()))?;

    for component in root.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => name,
            Component::ParentDir | Component::Prefix(_) => {
                return Err(format!(
                    "profile root contains an unsafe path component: {}",
                    root.display()
                ));
            }
        };
        let name = std::os::unix::ffi::OsStrExt::as_bytes(name);
        match current.open_at(name) {
            Ok(next) => current = next,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
                let created = match current.mkdir_at(name, 0o700) {
                    Ok(()) => true,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
                    Err(error) => {
                        return Err(format!(
                            "create profile root component {}: {error}",
                            root.display()
                        ));
                    }
                };
                current = current.open_at(name).map_err(|error| {
                    format!("open profile root component {}: {error}", root.display())
                })?;
                if created {
                    current.fchmod(0o700).map_err(|error| {
                        format!("set profile root component mode {}: {error}", root.display())
                    })?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "open profile root component {}: {error}",
                    root.display()
                ));
            }
        }
    }
    Ok(Some(current))
}

#[cfg(not(target_os = "macos"))]
fn canonical_root(_: &Path, _: bool) -> Result<Option<fd::Dir>, String> {
    Err("fd-bound OMP profile management is only available on macOS".to_string())
}

static PROFILE_MUTATION_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn mutation_guard() -> Result<MutexGuard<'static, ()>, String> {
    PROFILE_MUTATION_LOCK
        .lock()
        .map_err(|_| "profile mutation lock is poisoned".to_string())
}

pub(crate) fn validate_profile_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 48 {
        return Err("profile name must be 1-48 characters".to_string());
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
    {
        return Err(
            "profile name must use lowercase ASCII letters, digits, and internal hyphens"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_scalar(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.chars().count() > MAX_MODEL_CHARS
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "{label} must be 1-{MAX_MODEL_CHARS} characters with no whitespace or control characters"
        ));
    }
    Ok(())
}

pub(crate) fn validate_create_request(request: &CreateOmpProfileRequest) -> Result<(), String> {
    validate_profile_name(&request.name)?;
    validate_scalar(&request.model, "model")?;
    if !VALID_THINKING_LEVELS.contains(&request.thinking_level.as_str()) {
        return Err(format!(
            "thinkingLevel must be one of {}",
            VALID_THINKING_LEVELS.join(", ")
        ));
    }
    if let Some(title_model) = request.title_model.as_deref() {
        validate_scalar(title_model, "titleModel")?;
    }
    Ok(())
}

fn yaml_scalar(value: &str) -> String {
    serde_json::to_string(value).expect("JSON strings are always serializable")
}

pub(crate) fn generate_config(request: &CreateOmpProfileRequest) -> String {
    let model = yaml_scalar(&request.model);
    let mut output =
        String::from("# Generated by UltraTerm; edit this profile through OMP tooling.\n");
    output.push_str("setupVersion: 2\n");
    output.push_str("modelRoleStorage: \"profile\"\n");
    output.push_str("disabledProviders: []\nenabledModels:\n  - ");
    output.push_str(&model);
    output.push_str("\nmodelRoles:\n");
    for role in ["working", "worker", "quality"] {
        output.push_str("  ");
        output.push_str(role);
        output.push_str(": ");
        output.push_str(&model);
        output.push('\n');
    }
    for (role, target) in [
        ("default", "@working"),
        ("smol", "@worker"),
        ("tiny", "@worker"),
        ("task", "@worker"),
        ("slow", "@quality"),
        ("plan", "@quality"),
        ("advisor", "@quality"),
        ("designer", "@quality"),
        ("commit", "@worker"),
        ("vision", "@worker"),
        ("vision_high_accuracy", "@quality"),
    ] {
        output.push_str("  ");
        output.push_str(role);
        output.push_str(": ");
        output.push_str(&yaml_scalar(target));
        output.push('\n');
    }
    output.push_str("cycleOrder:\n  - \"default\"\n  - \"smol\"\n  - \"tiny\"\n  - \"plan\"\n");
    output.push_str("defaultThinkingLevel: ");
    output.push_str(&yaml_scalar(&request.thinking_level));
    output.push('\n');
    output.push_str(
        "contextPromotion:\n  enabled: true\nworktree:\n  base: \"~/.omp/wt\"\nretry:\n  modelFallback: false\n",
    );
    output.push_str("task:\n  maxConcurrency: 4\n  agentModelOverrides:\n");
    for (role, target) in [
        ("task", "@worker"),
        ("sonic", "@worker"),
        ("scout", "@worker"),
        ("librarian", "@worker"),
        ("code-simplifier", "@worker"),
        ("code-reviewer", "@quality"),
        ("reviewer", "@quality"),
        ("security-reviewer", "@quality"),
        ("designer", "@quality"),
        ("image-inspector", "@worker"),
        ("high-accuracy-image-inspector", "@quality"),
    ] {
        output.push_str("    ");
        output.push_str(role);
        output.push_str(": ");
        output.push_str(&yaml_scalar(target));
        output.push('\n');
    }
    output.push_str("advisor:\n  enabled: false\nsteeringMode: \"all\"\nstt:\n  submitTrigger: \"never\"\n  enabled: false\n");
    output.push_str("inspect_image:\n  mode: \"on\"\ntui:\n  maxInlineImages: 100\ndev:\n  autoqaConsent: \"denied\"\n");
    if let Some(title_model) = request.title_model.as_deref() {
        output.push_str("providers:\n  tinyModel: ");
        output.push_str(&yaml_scalar(title_model));
        output.push('\n');
    }
    output
}

fn temporary_name(name: &str, operation: &str) -> String {
    format!(".{name}.{operation}-{}-{}", process::id(), unique_suffix())
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn create_temporary(root: &fd::Dir, name: &str, operation: &str) -> Result<(String, fd::Dir), String> {
    for _ in 0..16 {
        let temporary = temporary_name(name, operation);
        match root.mkdir_at(temporary.as_bytes(), 0o700) {
            Ok(()) => {
                let directory = root
                    .open_at(temporary.as_bytes())
                    .and_then(|directory| directory.fchmod(0o700).map(|()| directory));
                match directory {
                    Ok(directory) => return Ok((temporary, directory)),
                    Err(error) => {
                        let _ = root.unlink_at(temporary.as_bytes(), fd::REMOVE_DIR);
                        return Err(format!("open temporary profile {temporary}: {error}"));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!("create temporary profile {temporary}: {error}"));
            }
        }
    }
    Err("unable to allocate a unique temporary profile name".to_string())
}

fn remove_tree_contents(directory: &fd::Dir) -> Result<(), String> {
    for name in directory
        .names()
        .map_err(|error| format!("read profile directory: {error}"))?
    {
        let is_directory = match directory.is_dir_at(&name) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("inspect profile entry: {error}")),
        };
        if is_directory {
            let child = match directory.open_at(&name) {
                Ok(child) => child,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("open profile directory entry: {error}")),
            };
            remove_tree_contents(&child)?;
            directory
                .unlink_at(&name, fd::REMOVE_DIR)
                .map_err(|error| format!("remove profile directory entry: {error}"))?;
        } else {
            directory
                .unlink_at(&name, 0)
                .map_err(|error| format!("remove profile entry: {error}"))?;
        }
    }
    Ok(())
}

fn remove_tree_entry(root: &fd::Dir, name: &str) -> Result<(), String> {
    let directory = root
        .open_at(name.as_bytes())
        .map_err(|error| format!("open temporary profile {name}: {error}"))?;
    remove_tree_contents(&directory)?;
    root.unlink_at(name.as_bytes(), fd::REMOVE_DIR)
        .map_err(|error| format!("remove temporary profile {name}: {error}"))
}

fn list_at_canonical_root(
    root: &fd::Dir,
    active: &HashSet<String>,
) -> Result<Vec<OmpProfileInfo>, String> {
    let mut profiles = Vec::new();
    for name in root
        .names()
        .map_err(|error| format!("list profiles: {error}"))?
    {
        let name = match String::from_utf8(name.clone()) {
            Ok(name) => name,
            Err(_) => continue,
        };
        if validate_profile_name(&name).is_err() {
            continue;
        }
        let is_directory = match root.is_dir_at(name.as_bytes()) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("inspect profile {name}: {error}")),
        };
        if is_directory {
            profiles.push(OmpProfileInfo {
                active: active.contains(&name),
                name,
            });
        }
    }
    profiles.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(profiles)
}

pub(crate) fn list_at(
    root: &Path,
    active: &HashSet<String>,
) -> Result<Vec<OmpProfileInfo>, String> {
    let Some(root) = canonical_root(root, false)? else {
        return Ok(Vec::new());
    };
    list_at_canonical_root(&root, active)
}

#[cfg(target_os = "macos")]
fn link_agent_assets(root: &Path, agent_directory: &fd::Dir) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;

    let home = root
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "profile root has no home directory".to_string())?;
    for asset_name in ["AGENTS.md", "RULES.md", "agents", "mcp.json", "skills"] {
        let source = home.join(".omp").join("agent").join(asset_name);
        if fs::symlink_metadata(&source).is_err() {
            continue;
        }
        agent_directory
            .symlink_at(source.as_os_str().as_bytes(), asset_name.as_bytes())
            .map_err(|error| format!("link OMP agent asset {asset_name}: {error}"))?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn link_agent_assets(_: &Path, _: &fd::Dir) -> Result<(), String> {
    Ok(())
}

pub(crate) fn create_at(
    root: &Path,
    request: &CreateOmpProfileRequest,
    active: &HashSet<String>,
) -> Result<OmpProfileInfo, String> {
    validate_create_request(request)?;
    let _guard = mutation_guard()?;
    let Some(root_directory) = canonical_root(root, true)? else {
        return Err("profile root does not exist".to_string());
    };
    let (temporary_name, temporary) =
        create_temporary(&root_directory, &request.name, "create")?;
    let result: Result<(), String> = (|| {
        let agent_directory = temporary
            .mkdir_at(b"agent", 0o700)
            .and_then(|()| temporary.open_at(b"agent"))
            .and_then(|directory| directory.fchmod(0o700).map(|()| directory))
            .map_err(|error| format!("create profile agent directory: {error}"))?;
        let mut config = agent_directory
            .open_file(b"config.yml", 0o600)
            .map_err(|error| format!("create profile config: {error}"))?;
        config
            .write_all(generate_config(request).as_bytes())
            .map_err(|error| format!("write profile config: {error}"))?;
        config
            .sync_all()
            .map_err(|error| format!("sync profile config: {error}"))?;
        link_agent_assets(root, &agent_directory)?;
        fd::rename_exclusive(
            &root_directory,
            temporary_name.as_bytes(),
            request.name.as_bytes(),
        )
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                format!("profile {} already exists", request.name)
            } else {
                format!("publish profile {}: {error}", request.name)
            }
        })?;
        Ok(())
    })();
    if let Err(error) = result {
        let cleanup = remove_tree_entry(&root_directory, &temporary_name);
        return match cleanup {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(format!("{error}; cleanup failed: {cleanup_error}")),
        };
    }
    Ok(OmpProfileInfo {
        name: request.name.clone(),
        active: active.contains(&request.name),
    })
}

fn restore_staged(root: &fd::Dir, temporary: &str, name: &str) -> Result<(), String> {
    fd::rename_exclusive(root, temporary.as_bytes(), name.as_bytes())
        .map_err(|error| format!("restore staged profile {name}: {error}"))
}

fn remove_at_inner(
    root: &Path,
    name: &str,
    active: &HashSet<String>,
    recheck_active: bool,
) -> Result<(), String> {
    validate_profile_name(name)?;
    let _guard = mutation_guard()?;
    let Some(root_directory) = canonical_root(root, false)? else {
        return Err(format!("profile root does not exist; profile {name} was not removed"));
    };
    if active.contains(name) {
        return Err(format!("active profile {name} cannot be removed"));
    }
    root_directory
        .open_at(name.as_bytes())
        .map_err(|error| format!("inspect profile {name}: {error}"))?;
    let temporary = temporary_name(name, "remove");
    fd::rename_exclusive(
        &root_directory,
        name.as_bytes(),
        temporary.as_bytes(),
    )
    .map_err(|error| format!("stage profile removal: {error}"))?;

    let staged = match root_directory.open_at(temporary.as_bytes()) {
        Ok(staged) => staged,
        Err(error) => {
            let stage_error = format!("open staged profile {name}: {error}");
            return Err(match restore_staged(&root_directory, &temporary, name) {
                Ok(()) => stage_error,
                Err(restore_error) => format!("{stage_error}; {restore_error}"),
            });
        }
    };

    if recheck_active {
        let second_active = match active_tmux_profiles(true) {
            Ok(active) => active,
            Err(error) => {
                drop(staged);
                return Err(match restore_staged(&root_directory, &temporary, name) {
                    Ok(()) => format!("active-profile recheck failed: {error}"),
                    Err(restore_error) => {
                        format!("active-profile recheck failed: {error}; {restore_error}")
                    }
                });
            }
        };
        if second_active.contains(name) {
            drop(staged);
            return Err(match restore_staged(&root_directory, &temporary, name) {
                Ok(()) => format!("active profile {name} cannot be removed"),
                Err(restore_error) => {
                    format!("active profile {name} cannot be removed; {restore_error}")
                }
            });
        }
    }

    let deletion = remove_tree_contents(&staged).and_then(|()| {
        root_directory
            .unlink_at(temporary.as_bytes(), fd::REMOVE_DIR)
            .map_err(|error| format!("remove profile {name}: {error}"))
    });
    drop(staged);
    match deletion {
        Ok(()) => Ok(()),
        Err(error) => Err(match restore_staged(&root_directory, &temporary, name) {
            Ok(()) => error,
            Err(restore_error) => format!("{error}; {restore_error}"),
        }),
    }
}

#[cfg(test)]
pub(crate) fn remove_at(root: &Path, name: &str, active: &HashSet<String>) -> Result<(), String> {
    remove_at_inner(root, name, active, false)
}
fn active_tmux_profiles(require_tmux: bool) -> Result<HashSet<String>, String> {
    let Some(tmux) = crate::telemetry::tmux_binary()? else {
        return if require_tmux {
            Err("tmux is unavailable; refusing profile removal without active metadata".to_string())
        } else {
            Ok(HashSet::new())
        };
    };
    let output = process::Command::new(tmux)
        .args(["list-sessions", "-F", "#{@omp-profile}"])
        .output()
        .map_err(|error| format!("inspect active OMP profiles: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("no server running") || stderr.contains("no sessions") {
            return Ok(HashSet::new());
        }
        return Err(format!(
            "tmux active-profile inspection failed with status {}",
            output.status
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .map(str::to_string)
        .collect())
}

pub(crate) fn list() -> Result<Vec<OmpProfileInfo>, String> {
    let active = active_tmux_profiles(false)?;
    list_at(&root_for_current_user()?, &active)
}

pub(crate) fn create(request: &CreateOmpProfileRequest) -> Result<OmpProfileInfo, String> {
    let active = active_tmux_profiles(false)?;
    create_at(&root_for_current_user()?, request, &active)
}

pub(crate) fn remove(name: &str) -> Result<(), String> {
    let active = active_tmux_profiles(true)?;
    remove_at_inner(&root_for_current_user()?, name, &active, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir_in;

    fn request(name: &str) -> CreateOmpProfileRequest {
        CreateOmpProfileRequest {
            name: name.to_string(),
            model: "vendor/model\"quoted".to_string(),
            thinking_level: "medium".to_string(),
            title_model: Some("vendor/title".to_string()),
        }
    }

    #[test]
    fn validates_profile_names_models_and_thinking() {
        assert!(validate_profile_name("team-1").is_ok());
        for name in ["", ".", "a/b", "A", "-team", "team-", "a--b"] {
            assert!(validate_profile_name(name).is_err(), "{name}");
        }
        assert!(validate_create_request(&request("team")).is_ok());
        let mut invalid = request("team");
        invalid.model.clear();
        assert!(validate_create_request(&invalid).is_err());
        invalid = request("team");
        invalid.thinking_level = "sometimes".to_string();
        assert!(validate_create_request(&invalid).is_err());
    }

    #[test]
    fn generated_yaml_quotes_scalars_and_contains_complete_roles() {
        let yaml = generate_config(&request("team"));
        assert!(yaml.contains("setupVersion: 2"));
        assert!(yaml.contains("  - \"vendor/model\\\"quoted\""));
        assert!(yaml.contains("  working: \"vendor/model\\\"quoted\""));
        assert!(yaml.contains("  default: \"@working\""));
        assert!(yaml.contains("providers:\n  tinyModel: \"vendor/title\""));
        assert!(yaml.contains("contextPromotion:\n  enabled: true"));
        assert!(yaml.contains("worktree:\n  base: \"~/.omp/wt\""));
        assert!(yaml.contains("retry:\n  modelFallback: false"));
        assert!(yaml.contains("maxConcurrency: 4"));
        for role in [
            "working",
            "worker",
            "quality",
            "default",
            "smol",
            "tiny",
            "task",
            "slow",
            "plan",
            "advisor",
            "designer",
            "commit",
            "vision",
            "vision_high_accuracy",
        ] {
            assert!(yaml.contains(&format!("  {role}:")), "missing {role}");
        }
        for role in [
            "task",
            "sonic",
            "scout",
            "librarian",
            "code-simplifier",
            "code-reviewer",
            "reviewer",
            "security-reviewer",
            "designer",
            "image-inspector",
            "high-accuracy-image-inspector",
        ] {
            assert!(
                yaml.contains(&format!("    {role}:")),
                "missing task override {role}"
            );
        }
    }

    #[test]
    fn title_model_is_optional_and_does_not_replace_tiny_role() {
        let mut request = request("team");
        request.title_model = None;
        assert!(!generate_config(&request).contains("tinyModel:"));
        request.title_model = Some("vendor/title".to_string());
        let yaml = generate_config(&request);
        assert!(yaml.contains("  tiny: \"@worker\""));
        assert!(yaml.contains("tinyModel: \"vendor/title\""));
    }

    #[test]
    fn creates_atomically_with_modes_and_agent_links_without_following_targets() {
        let home = tempdir_in("/private/tmp").unwrap();
        let root = home.path().join(".omp/profiles");
        fs::create_dir_all(home.path().join(".omp/agent/agents")).unwrap();
        fs::write(home.path().join(".omp/agent/AGENTS.md"), "user asset").unwrap();
        let info = create_at(&root, &request("team"), &HashSet::new()).unwrap();
        assert_eq!(info.name, "team");
        assert!(root.join("team/agent/config.yml").is_file());
        assert!(root.join("team/agent/AGENTS.md").is_symlink());
        assert!(root.join("team/agent/agents").is_symlink());
        assert!(!root.join("team/agent").is_symlink());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&root.join("team"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&root.join("team/agent"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&root.join("team/agent/config.yml"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(create_at(&root, &request("team"), &HashSet::new()).is_err());
    }

    #[test]
    fn list_and_remove_reject_symlinks_and_active_profiles() {
        let home = tempdir_in("/private/tmp").unwrap();
        let root = home.path().join("profiles");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir(root.join("real")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        let active = HashSet::from(["real".to_string()]);
        assert_eq!(list_at(&root, &active).unwrap().len(), 1);
        assert!(remove_at(&root, "real", &active).is_err());
        assert!(remove_at(&root, "link", &HashSet::new()).is_err());
        remove_at(&root, "real", &HashSet::new()).unwrap();
        assert!(!root.join("real").exists());
    }

    #[test]
    fn production_root_components_reject_symlinks() {
        let home = tempdir_in("/private/tmp").unwrap();
        let outside = tempdir_in("/private/tmp").unwrap();
        fs::create_dir(home.path().join(".omp")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            outside.path(),
            home.path().join(".omp/profiles"),
        )
        .unwrap();
        let root = home.path().join(".omp/profiles");
        assert!(create_at(&root, &request("team"), &HashSet::new()).is_err());
        assert!(!outside.path().join("team").exists());

        fs::remove_file(home.path().join(".omp/profiles")).unwrap();
        fs::create_dir(home.path().join(".omp/profiles")).unwrap();
        let outside_profiles = tempdir_in("/private/tmp").unwrap();
        fs::remove_dir(home.path().join(".omp/profiles")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            outside_profiles.path(),
            home.path().join(".omp/profiles"),
        )
        .unwrap();
        assert!(create_at(&root, &request("team"), &HashSet::new()).is_err());
        assert!(!outside_profiles.path().join("team").exists());
    }

    #[test]
    fn remove_unlinks_nested_symlinks_without_following_targets() {
        let home = tempdir_in("/private/tmp").unwrap();
        let root = home.path().join("profiles");
        let outside = tempdir_in("/private/tmp").unwrap();
        fs::create_dir_all(root.join("team/nested")).unwrap();
        fs::write(outside.path().join("sentinel"), "keep").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            outside.path(),
            root.join("team/nested/escape"),
        )
        .unwrap();
        remove_at(&root, "team", &HashSet::new()).unwrap();
        assert!(outside.path().join("sentinel").is_file());
        assert!(!root.join("team").exists());
    }
}
