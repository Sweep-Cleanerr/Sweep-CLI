//! The second destructive gate, after the keep guard.
//!
//! The guard protects declared roots and user data. This module adds the
//! filesystem-command rules: no relative paths, no critical system trees, no
//! user-data leaves, no symlink target swaps, and no writes where the process
//! cannot actually write.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::action::context::RunContext;
use crate::core::error::Error;
use crate::core::path::{
    case_sensitive, comparable, equal_prepared, starts_with_prepared,
};
use crate::platform;

/// Directories that must never be removed as a whole. Their contents are
/// handled by the explicit allow-list below or by later providers.
const CRITICAL_DIRS: &[&str] = &[
    "/",
    "/bin",
    "/boot",
    "/dev",
    "/etc",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/proc",
    "/root",
    "/run",
    "/sbin",
    "/srv",
    "/sys",
    "/usr",
    "/tmp",
    "/var",
    "/var/lib",
    "/var/db",
    "/opt",
    "/System",
    "/Library",
    "/Applications",
    "/Network",
    "/private",
];

/// Trees whose descendants are forbidden unless explicitly allow-listed.
const CRITICAL_SUBTREES: &[&str] = &[
    "/proc",
    "/sys",
    "/dev",
    "/boot",
    "/etc",
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/root",
    "/run",
    "/srv",
    "/opt",
    "/System",
    "/Library",
    "/Applications",
    "/Network",
    "/private",
    "/var/lib",
    "/var/db",
];

/// Paths a cleaner may remove inside otherwise critical trees.
const EXPLICITLY_CLEANABLE: &[&str] = &[
    "/var/log/journal",
    "/var/cache/apt/archives",
    "/var/cache/dnf",
    "/var/cache/yum",
    "/var/cache/pacman/pkg",
    "/var/cache/zypp",
    "/var/lib/snapd/cache",
    "/var/lib/flatpak",
    "/var/lib/docker/containers",
    "/var/tmp",
];

const USER_DATA_LEAVES: &[&str] = &[
    "Desktop",
    "Documents",
    "Downloads",
    "Pictures",
    "Music",
    "Videos",
    ".ssh",
    ".gnupg",
];

/// Prepare constant roots once; per-file checks then compare components
/// without rebuilding the same paths for every candidate.
fn prepared(list: &[&str]) -> Vec<PathBuf> {
    list.iter()
        .map(|root| comparable(Path::new(root)))
        .collect()
}

fn critical_dirs() -> &'static [PathBuf] {
    static ROOTS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    ROOTS.get_or_init(|| prepared(CRITICAL_DIRS))
}

fn cleanable_roots() -> &'static [PathBuf] {
    static ROOTS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    ROOTS.get_or_init(|| prepared(EXPLICITLY_CLEANABLE))
}

fn critical_subtrees() -> &'static [PathBuf] {
    static ROOTS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    ROOTS.get_or_init(|| prepared(CRITICAL_SUBTREES))
}

struct UserDataRoots {
    home: Option<PathBuf>,
    leaves: Vec<PathBuf>,
}

fn user_data_roots() -> &'static UserDataRoots {
    static ROOTS: OnceLock<UserDataRoots> = OnceLock::new();
    ROOTS.get_or_init(|| {
        let Some(home) = platform::home_dir() else {
            return UserDataRoots {
                home: None,
                leaves: Vec::new(),
            };
        };
        UserDataRoots {
            home: Some(comparable(&home)),
            leaves: USER_DATA_LEAVES
                .iter()
                .map(|leaf| comparable(&home.join(leaf)))
                .collect(),
        }
    })
}

pub fn check_path(path: &Path, ctx: &RunContext) -> std::result::Result<(), String> {
    let path = comparable(path);

    if !rooted_here(&path) {
        return Err("refusing to operate on a relative path".to_string());
    }
    if is_critical(&path) {
        return Err(format!("refusing to operate on critical path: {}", path.display()));
    }
    if is_user_data(&path) {
        return Err(format!("refusing to operate on user data: {}", path.display()));
    }

    // `false` rejects links and link ancestors outright, so no separate
    // symlink-target probe is needed here.
    ctx.guard
        .check_prepared(&path, false)
        .map_err(|error| error.to_string())?;

    #[cfg(unix)]
    {
        if is_root_only(&path) && unsafe { libc::geteuid() } != 0 {
            return Err(format!(
                "root is required to clean {}",
                path.display()
            ));
        }
    }

    if !ctx.dry_run && !parent_writable(&path) {
        return Err(format!(
            "no write permission on the parent of {}",
            path.display()
        ));
    }

    Ok(())
}

fn rooted_here(path: &Path) -> bool {
    if path.is_absolute() {
        return true;
    }
    #[cfg(windows)]
    {
        let text = path.as_os_str().to_string_lossy();
        text.starts_with('/') || text.starts_with('\\')
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// `path` must already be [`comparable`].
fn is_critical(path: &Path) -> bool {
    let cs = case_sensitive();
    if critical_dirs()
        .iter()
        .any(|root| equal_prepared(path, root, cs))
    {
        return true;
    }
    if cleanable_roots()
        .iter()
        .any(|allowed| starts_with_prepared(path, allowed, cs))
    {
        return false;
    }
    critical_subtrees()
        .iter()
        .any(|root| starts_with_prepared(path, root, cs))
}

/// `path` must already be [`comparable`].
fn is_user_data(path: &Path) -> bool {
    let roots = user_data_roots();
    let cs = case_sensitive();
    if roots
        .home
        .as_deref()
        .is_some_and(|home| equal_prepared(path, home, cs))
    {
        return true;
    }
    roots
        .leaves
        .iter()
        .any(|leaf| starts_with_prepared(path, leaf, cs))
}

/// True only when the path is simply gone: the expected scan/unlink race.
pub fn path_vanished(error: &Error) -> bool {
    matches!(
        error,
        Error::Io { source, .. } | Error::PlainIo(source)
            if source.kind() == std::io::ErrorKind::NotFound
    )
}

pub fn delete_failure_message(path: &Path, error: &Error) -> String {
    let shown = path.display();
    let source = match error {
        Error::Io { source, .. } | Error::PlainIo(source) => Some(source),
        _ => None,
    };
    if let Some(source) = source {
        if platform::is_locked(source) {
            return format!("{shown}: file is in use");
        }
        if source.kind() == std::io::ErrorKind::PermissionDenied {
            return format!("{shown}: permission denied");
        }
    }
    format!("{shown}: {error}")
}

#[cfg(unix)]
fn parent_writable(path: &Path) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let Some(parent) = path.parent() else {
        return true;
    };
    if !parent.exists() {
        return true;
    }
    let Ok(metadata) = std::fs::metadata(parent) else {
        return true;
    };
    let mode = metadata.permissions().mode();
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        return true;
    }
    if metadata.uid() == euid {
        return mode & 0o200 != 0;
    }
    let gid = metadata.gid();
    if gid == unsafe { libc::getegid() } || effective_groups().contains(&gid) {
        return mode & 0o020 != 0;
    }
    mode & 0o002 != 0
}

#[cfg(not(unix))]
fn parent_writable(_path: &Path) -> bool {
    true
}

#[cfg(unix)]
fn effective_groups() -> Vec<libc::gid_t> {
    let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    if count <= 0 {
        return Vec::new();
    }
    let mut groups = vec![0 as libc::gid_t; count as usize];
    let read = unsafe { libc::getgroups(count, groups.as_mut_ptr()) };
    if read < 0 {
        return Vec::new();
    }
    groups.truncate(read as usize);
    groups
}

#[cfg(unix)]
fn is_root_only(path: &Path) -> bool {
    let text = path.to_string_lossy().replace('\\', "/");
    text == "/var/log"
        || text == "/var/cache"
        || text.starts_with("/var/log/")
        || text.starts_with("/var/cache/")
        || text.starts_with("/var/lib/")
        || text.starts_with("/boot/")
        || text.starts_with("/usr/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> RunContext {
        RunContext::preview()
    }

    #[test]
    fn relative_paths_are_refused() {
        let error = check_path(Path::new("relative/file"), &context()).unwrap_err();
        assert!(error.contains("relative path"));
    }

    #[test]
    fn critical_directories_and_their_dangerous_children_are_refused() {
        assert!(check_path(Path::new("/etc"), &context()).is_err());
        assert!(check_path(Path::new("/etc/passwd"), &context()).is_err());
        assert!(check_path(Path::new("/usr/bin/tool"), &context()).is_err());
    }

    #[test]
    fn allowlisted_cache_and_log_paths_pass() {
        assert!(check_path(Path::new("/var/tmp/file"), &context()).is_ok());
        assert!(check_path(Path::new("/var/log/journal/entry"), &context()).is_ok());
        assert!(check_path(Path::new("/var/cache/apt/archives/pkg.deb"), &context()).is_ok());
    }

    #[test]
    fn user_data_leaves_are_refused() {
        let Some(home) = platform::home_dir() else {
            return;
        };
        for leaf in USER_DATA_LEAVES {
            assert!(
                check_path(&home.join(leaf).join("file.txt"), &context()).is_err(),
                "leaf {leaf} was not protected"
            );
        }
        assert!(check_path(&home, &context()).is_err());
    }

    #[test]
    fn ordinary_temp_paths_are_allowed() {
        let path = crate::fixture("action_safety").join("cache.txt");
        assert!(check_path(&path, &context()).is_ok());
    }

    #[test]
    fn vanished_errors_are_recognized() {
        let error = Error::io(
            "/tmp/gone",
            std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        assert!(path_vanished(&error));
        let error = Error::msg("still here");
        assert!(!path_vanished(&error));
    }

    #[test]
    fn delete_failures_explain_locked_and_denied_files() {
        let path = Path::new("/tmp/x");
        let error = Error::io(
            path,
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        assert!(delete_failure_message(path, &error).contains("permission denied"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed_out_of_the_tree() {
        use std::os::unix::fs::symlink;

        let root = crate::fixture("action_safety_link");
        let link = root.join("etc");
        symlink("/etc", &link).unwrap();
        assert!(check_path(&link, &context()).is_err());
        assert!(check_path(&link.join("passwd"), &context()).is_err());
    }
}
