//! POSIX queries needed by fsutil.

use std::path::{Path, PathBuf};

use crate::core::error::{Error, Result};

/// Returns `(available, total)` bytes from `statvfs(3)`.
fn statvfs(path: &Path) -> Result<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let target = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            // A bare relative filename can have an empty parent path.
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    };

    // Raw bytes, not to_string_lossy(): POSIX paths may contain arbitrary
    // bytes, and a lossy conversion could query a different or nonexistent
    // path.
    let c_path = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| Error::msg("path contains a NUL byte"))?;

    // SAFETY: `c_path` is NUL-terminated, and `buf` is valid, writable
    // storage for a `statvfs` structure.
    unsafe {
        let mut buf: libc::statvfs = std::mem::zeroed();

        if libc::statvfs(c_path.as_ptr(), &mut buf) != 0 {
            return Err(Error::io(path, std::io::Error::last_os_error()));
        }

        // Fields are u32 on some 32-bit platforms, so the conversion is
        // necessary there.
        #[allow(clippy::useless_conversion, clippy::unnecessary_cast)]
        Ok((
            u64::from(buf.f_bavail).saturating_mul(buf.f_frsize as u64),
            u64::from(buf.f_blocks).saturating_mul(buf.f_frsize as u64),
        ))
    }
}

pub fn free_space(path: &Path) -> Result<u64> {
    statvfs(path).map(|vfs| vfs.0)
}

pub fn total_space(path: &Path) -> Result<u64> {
    statvfs(path).map(|vfs| vfs.1)
}

/// Grants the owner write permission before deleting a read-only file.
///
/// Symbolic links are rejected. Note that path-based permission changes
/// cannot completely eliminate races if another process can replace the path
/// concurrently.
pub fn make_writable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    if crate::core::keep::is_link(path) {
        return Err(Error::Link(path.to_path_buf()));
    }

    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| Error::io(path, error))?;

    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o200);

    std::fs::set_permissions(path, permissions)
        .map_err(|error| Error::io(path, error))
}

pub fn trash_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(home).join("Trash"));
    } else if let Some(home) = crate::platform::home_dir() {
        dirs.push(home.join(".local").join("share").join("Trash"));
    }
    if cfg!(target_os = "macos") {
        if let Some(home) = crate::platform::home_dir() {
            dirs.push(home.join(".Trash"));
        }
        if let Ok(entries) = std::fs::read_dir("/Volumes") {
            for entry in entries.flatten() {
                dirs.push(entry.path().join(".Trashes").join(unsafe { libc::getuid() }.to_string()));
            }
        }
    }
    dirs.retain(|d| d.is_dir());
    dirs
}

pub fn rotated_logs() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in [PathBuf::from("/var/log"), PathBuf::from("/private/var/log")] {
        if root.is_dir() {
            found.extend(rotated_in(&root));
        }
    }
    if cfg!(target_os = "macos") {
        if let Some(home) = crate::platform::home_dir() {
            let logs = home.join("Library").join("Logs");
            if logs.is_dir() {
                found.extend(rotated_in(&logs));
            }
        }
    }
    found
}

fn rotated_in(root: &Path) -> Vec<PathBuf> {
    crate::fsutil::walk::scan_paths(root, &crate::fsutil::walk::ScanOptions::files())
        .into_iter()
        .filter(|path| {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            is_rotated_name(&name)
        })
        .filter(|path| std::fs::symlink_metadata(path).map(|md| md.file_type().is_file()).unwrap_or(false))
        .collect()
}

fn is_rotated_name(name: &str) -> bool {
    if name.starts_with('.') {
        return false;
    }
    for suffix in [".gz", ".bz2", ".xz", ".zst", ".lz4", ".old", ".zip"] {
        if name.ends_with(suffix) {
            return true;
        }
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s, e),
        None => (name, ""),
    };
    !ext.is_empty() && ext.chars().all(|c| c.is_ascii_digit()) && !stem.is_empty()
}

pub fn localization_paths(keep: &[String]) -> Vec<PathBuf> {
    let roots = [
        "/usr/share/locale",
        "/usr/share/locale-langpack",
        "/usr/share/man",
        "/usr/share/help",
        "/usr/share/gnome/help",
    ];
    let mut dirs = Vec::new();
    for root in roots {
        let root = Path::new(root);
        if !root.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if !is_locale_dir(&name) {
                continue;
            }
            if keep.iter().any(|k| locale_keep_matches(k, &name)) {
                continue;
            }
            dirs.push(path);
        }
    }
    dirs
}

fn is_locale_dir(name: &str) -> bool {
    if matches!(name, "man" | "locale.alias") {
        return false;
    }
    if matches!(name, "C" | "POSIX" | "C.UTF-8") {
        return true;
    }
    if name.len() > 24 {
        return false;
    }
    let base = name.split(['.', '@']).next().unwrap_or(name);
    let mut parts = base.split(['_', '-']);
    let lang = parts.next().unwrap_or("");
    if !(2..=3).contains(&lang.len()) || !lang.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    parts.all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric()))
}

fn locale_keep_matches(keep: &str, name: &str) -> bool {
    let keep = keep.trim();
    if keep.is_empty() {
        return false;
    }
    if keep.eq_ignore_ascii_case(name) {
        return true;
    }
    name.len() > keep.len()
        && name[..keep.len()].eq_ignore_ascii_case(keep)
        && matches!(name.as_bytes().get(keep.len()), Some(b'_') | Some(b'@') | Some(b'-'))
}

pub fn clear_clipboard() -> Result<()> {
    let candidates: &[(&str, &[&str])] = &[
        ("wl-copy", &["--clear"]),
        ("xclip", &["-selection", "clipboard", "/dev/null"]),
        ("xsel", &["--clipboard", "--clear"]),
        ("pbcopy", &["/dev/null"]),
    ];
    for (program, args) in candidates {
        if !crate::fsutil::path_exists_in_path(program) {
            continue;
        }
        match std::process::Command::new(program).args(*args).status() {
            Ok(s) if s.success() => return Ok(()),
            Ok(_) => continue,
            Err(err) => log::debug!("{program} failed: {err}"),
        }
    }
    Err(Error::msg("no clipboard helper found (install wl-copy, xclip, xsel or macOS pbcopy)"))
}

pub fn flush_dns() -> Result<()> {
    let candidates: &[(&str, &[&str])] = &[
        ("resolvectl", &["flush-caches"]),
        ("systemd-resolve", &["--flush-caches"]),
        ("dscacheutil", &["-flushcache"]),
        ("killall", &["-HUP", "mDNSResponder"]),
    ];
    let mut last_error: Option<Error> = None;
    for (program, args) in candidates {
        if !crate::fsutil::path_exists_in_path(program) {
            continue;
        }
        match std::process::Command::new(program).args(*args).status() {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => {
                last_error = Some(Error::msg(format!("{program} exited with {s}")));
            }
            Err(err) => log::debug!("{program} failed: {err}"),
        }
    }
    Err(last_error.unwrap_or_else(|| Error::msg("no supported DNS flush command found")))
}
