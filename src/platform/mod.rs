//! Small platform queries used by the core layer.

use std::path::{Path, PathBuf};

use crate::core::error::Result;

#[cfg(unix)]
pub(crate) mod unix;
#[cfg(windows)]
pub(crate) mod windows;
#[cfg(target_os = "linux")]
mod linux;

pub(crate) mod winreg;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    Macos,
    Bsd,
    UnixOther,
}

pub fn current_os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::Macos
    } else if cfg!(target_os = "linux") {
        Os::Linux
    } else if cfg!(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )) {
        Os::Bsd
    } else {
        Os::UnixOther
    }
}

pub fn os_name() -> &'static str {
    match current_os() {
        Os::Windows => "windows",
        Os::Linux => "linux",
        Os::Macos => "macos",
        Os::Bsd => "bsd",
        Os::UnixOther => "unix",
    }
}

/// Whether `filter` (a definition `os` value) applies to the current system.
/// An empty filter matches everything; unknown values match nothing.
pub fn os_matches(filter: &str) -> bool {
    let filter = filter.trim().to_ascii_lowercase();

    match filter.as_str() {
        "" => true,
        "windows" => cfg!(windows),
        "linux" => cfg!(target_os = "linux"),
        "macos" | "darwin" => cfg!(target_os = "macos"),
        "bsd" => cfg!(any(
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        )),
        "freebsd" => cfg!(target_os = "freebsd"),
        "openbsd" => cfg!(target_os = "openbsd"),
        "netbsd" => cfg!(target_os = "netbsd"),
        "dragonfly" => cfg!(target_os = "dragonfly"),
        "unix" => cfg!(unix),
        _ => false,
    }
}

pub fn known_os_filter(filter: &str) -> bool {
    matches!(
        filter.trim().to_ascii_lowercase().as_str(),
        "" | "windows"
            | "linux"
            | "macos"
            | "darwin"
            | "unix"
            | "bsd"
            | "freebsd"
            | "openbsd"
            | "netbsd"
            | "dragonfly"
    )
}

pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

pub fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir()
}

pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("sweep"))
}

#[cfg(windows)]
pub fn is_reparse_point(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| is_reparse_point_metadata(&metadata))
        .unwrap_or(false)
}

#[cfg(not(windows))]
pub fn is_reparse_point(_path: &Path) -> bool {
    false
}

/// Reparse-point check for metadata the caller already has.
#[cfg(windows)]
pub fn is_reparse_point_metadata(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
pub fn is_reparse_point_metadata(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// Filesystem identifier for `path` (POSIX `st_dev`, Windows volume serial).
pub fn device_id(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        std::fs::symlink_metadata(path)
            .ok()
            .map(|metadata| metadata.dev())
    }

    #[cfg(windows)]
    {
        windows::device_id(path)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        None
    }
}

/// Does `path` have more than one hard link?
pub fn is_hard_link(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        std::fs::symlink_metadata(path)
            .map(|metadata| {
                metadata.file_type().is_file() && metadata.nlink() > 1
            })
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        windows::is_hard_link(path)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        false
    }
}

/// Stable identity of the file behind `path`: `(device, inode)` on POSIX,
/// `(volume serial, file index)` on Windows. Hard links share this pair.
pub fn file_identity(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        std::fs::symlink_metadata(path)
            .ok()
            .map(|metadata| (metadata.dev(), metadata.ino()))
    }

    #[cfg(windows)]
    {
        windows::file_identity(path)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        None
    }
}

/// Does this I/O error mean "the file is locked by another process"?
pub fn is_locked(err: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        matches!(err.raw_os_error(), Some(32) | Some(33))
    }

    #[cfg(unix)]
    {
        // Use libc constants rather than fixed numbers: EAGAIN is 11 on
        // Linux but 35 on macOS/BSD, where 11 is EDEADLK.
        let code = err.raw_os_error();

        code == Some(libc::EBUSY)
            || code == Some(libc::EAGAIN)
            || code == Some(libc::EWOULDBLOCK)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = err;
        false
    }
}

/// Does this I/O error mean "read-only / access denied"?
pub fn is_readonly_error(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::PermissionDenied
}

/// Drops the read-only attribute (or grants owner write on POSIX).
///
/// Returns `true` only when a change was necessary and completed
/// successfully.
pub fn clear_readonly(path: &Path) -> bool {
    #[cfg(windows)]
    {
        windows::clear_readonly(path)
    }

    #[cfg(unix)]
    {
        // `make_writable` also succeeds when there is nothing to change.
        // Check the permissions first so the return value retains its
        // "something was actually cleared" meaning.
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            return false;
        };

        if !metadata.permissions().readonly() {
            return false;
        }

        unix::make_writable(path).is_ok()
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        false
    }
}

/// Free space available to the current user at `path`.
pub fn free_space(path: &Path) -> Result<u64> {
    #[cfg(unix)]
    {
        unix::free_space(path)
    }

    #[cfg(windows)]
    {
        windows::free_space(path)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;

        Err(crate::core::error::Error::msg(
            "free space reporting is unsupported on this platform",
        ))
    }
}

/// Total capacity of the filesystem holding `path`.
pub fn total_space(path: &Path) -> Result<u64> {
    #[cfg(unix)]
    {
        unix::total_space(path)
    }

    #[cfg(windows)]
    {
        windows::total_space(path)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(crate::core::error::Error::msg(
            "capacity reporting is unsupported on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_filter_matches_and_unknown_does_not() {
        assert!(os_matches(""));
        assert!(!os_matches("plan9"));
        assert!(known_os_filter("Darwin"));
        assert!(!known_os_filter("plan9"));
    }

    #[test]
    fn device_id_is_stable_within_a_tree() {
        let dir = crate::fixture("plat_dev");
        let file = dir.join("f.txt");

        std::fs::write(&file, b"x").unwrap();

        let root_dev = device_id(&dir).expect("device id of a real dir");

        assert_eq!(device_id(&file), Some(root_dev));
        assert!(device_id(&dir.join("missing")).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hardlinks_share_a_file_identity() {
        let dir = crate::fixture("plat_ident");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");

        std::fs::write(&a, b"x").unwrap();

        if std::fs::hard_link(&a, &b).is_err() {
            let _ = std::fs::remove_dir_all(&dir);
            return; // Filesystem without hard-link support, such as FAT32.
        }

        assert!(is_hard_link(&a));
        assert!(is_hard_link(&b));
        assert_eq!(file_identity(&a), file_identity(&b));
        assert!(file_identity(&a).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_readonly_flips_the_attribute() {
        let dir = crate::fixture("plat_readonly");
        let file = dir.join("ro.txt");

        std::fs::write(&file, b"x").unwrap();

        let mut permissions = std::fs::metadata(&file)
            .unwrap()
            .permissions();

        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions).unwrap();

        assert!(clear_readonly(&file));
        assert!(
            !std::fs::metadata(&file)
                .unwrap()
                .permissions()
                .readonly()
        );

        // Nothing is left to clear the second time.
        assert!(!clear_readonly(&file));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

pub fn clear_clipboard() -> Result<()> {
    #[cfg(windows)]
    {
        windows::clear_clipboard()
    }
    #[cfg(unix)]
    {
        unix::clear_clipboard()
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(crate::core::error::Error::msg("clipboard clearing is unsupported"))
    }
}

pub fn flush_dns() -> Result<()> {
    #[cfg(windows)]
    {
        windows::flush_dns()
    }
    #[cfg(target_os = "linux")]
    {
        linux::flush_dns()
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        unix::flush_dns()
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(crate::core::error::Error::msg("dns flushing is unsupported"))
    }
}

pub fn trash_dirs() -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        unix::trash_dirs()
    }
    #[cfg(windows)]
    {
        windows::recycle_bin_dirs()
    }
    #[cfg(not(any(unix, windows)))]
    {
        Vec::new()
    }
}

pub fn rotated_logs() -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        unix::rotated_logs()
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

pub fn localization_paths(keep: &[String]) -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        unix::localization_paths(keep)
    }
    #[cfg(not(unix))]
    {
        let _ = keep;
        Vec::new()
    }
}

pub fn shell_refresh() {
    #[cfg(windows)]
    {
        windows::shell_change_notify();
    }
}

pub fn trim(path: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::fitrim(path)
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        let _ = path;
        Err(crate::core::error::Error::msg("trim is only implemented on Linux"))
    }
    #[cfg(windows)]
    {
        windows::trim(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(crate::core::error::Error::msg("trim is unsupported on this platform"))
    }
}

pub fn wipe_memory() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::wipe_memory()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(crate::core::error::Error::msg("memory wiping is only supported on Linux"))
    }
}

pub fn journald_clean(vacuum_days: u32) -> Result<u64> {
    #[cfg(target_os = "linux")]
    {
        linux::journald_clean(vacuum_days)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = vacuum_days;
        Err(crate::core::error::Error::msg("journald cleaning is Linux-only"))
    }
}

pub fn is_process_running(name: &str) -> bool {
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    struct Snapshot {
        at: Instant,
        names: HashSet<String>,
    }

    static CACHE: std::sync::OnceLock<Mutex<Option<Snapshot>>> = std::sync::OnceLock::new();
    const TTL: Duration = Duration::from_secs(5);

    let needle = name.to_lowercase();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some(snapshot) = guard.as_ref() {
            if snapshot.at.elapsed() < TTL {
                return snapshot.names.contains(&needle);
            }
        }
    }

    let system = sysinfo::System::new_all();
    let names: HashSet<String> = system
        .processes()
        .values()
        .map(|process| process.name().to_string().to_lowercase())
        .collect();
    let hit = names.contains(&needle);
    if let Ok(mut guard) = cache.lock() {
        *guard = Some(Snapshot {
            at: Instant::now(),
            names,
        });
    }
    hit
}

pub fn reg_read_string(full: &str, name: Option<&str>) -> Option<String> {
    winreg::reg_read_string(full, name)
}
