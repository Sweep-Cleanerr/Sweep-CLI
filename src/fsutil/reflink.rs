//! Copy-on-write reflink with hardlink fallback — safe relinking
//! for `dupes --link`.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkOutcome {
    Reflinked,
    Hardlinked,
    SkippedMutable,
}

static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn tmp_sibling(target: &Path, tag: &str) -> std::path::PathBuf {
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let name = format!(".sweep-{tag}-{}-{n}-{nanos}", std::process::id());
    target.with_file_name(name)
}

pub fn link_replace(keeper: &Path, target: &Path, may_hardlink: bool) -> std::io::Result<LinkOutcome> {
    let tmp = tmp_sibling(target, "relink");
    let outcome = match reflink(keeper, &tmp) {
        Ok(()) => LinkOutcome::Reflinked,
        Err(_) => {
            let _ = std::fs::remove_file(&tmp);
            if !may_hardlink {
                return Ok(LinkOutcome::SkippedMutable);
            }
            std::fs::hard_link(keeper, &tmp)?;
            LinkOutcome::Hardlinked
        }
    };
    if let Err(e) = std::fs::remove_file(target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, target)?;
    Ok(outcome)
}

pub fn link_new(keeper: &Path, dst: &Path, may_hardlink: bool) -> std::io::Result<LinkOutcome> {
    if !may_hardlink && dst.symlink_metadata().is_ok() {
        return Ok(LinkOutcome::SkippedMutable);
    }
    match reflink(keeper, dst) {
        Ok(()) => Ok(LinkOutcome::Reflinked),
        Err(_) => {
            let _ = std::fs::remove_file(dst);
            if !may_hardlink {
                return Ok(LinkOutcome::SkippedMutable);
            }
            std::fs::hard_link(keeper, dst)?;
            Ok(LinkOutcome::Hardlinked)
        }
    }
}

pub fn reflink(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        const FICLONE: libc::c_ulong = 0x4004_9409;
        let s = std::fs::File::open(src)?;
        let d = std::fs::File::create(dst)?;
        let rc = unsafe { libc::ioctl(d.as_raw_fd(), FICLONE as _, s.as_raw_fd()) };
        if rc != 0 {
            let _ = std::fs::remove_file(dst);
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        extern "C" {
            fn clonefile(src: *const std::ffi::c_char, dst: *const std::ffi::c_char, flags: u32) -> std::ffi::c_int;
        }
        let src_c = std::ffi::CString::new(src.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let dst_c = std::ffi::CString::new(dst.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let rc = unsafe { clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) };
        if rc != 0 {
            let _ = std::fs::remove_file(dst);
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (src, dst);
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }
}

pub fn same_inode(a: &Path, b: &Path) -> bool {
    match (crate::platform::file_identity(a), crate::platform::file_identity(b)) {
        (Some(ma), Some(mb)) => ma == mb,
        _ => false,
    }
}

/// Regenerable build/cache directory names. Closed list on purpose:
/// `devscan --clean` must never match an arbitrary directory.
pub const DEV_DIR_NAMES: &[&str] = &[
    "node_modules", "target", "__pycache__", ".venv", "venv", ".next", ".nuxt", ".turbo",
    ".parcel-cache", ".vite", ".svelte-kit", ".angular", ".mypy_cache", ".pytest_cache",
    ".ruff_cache", ".tox", ".nox", ".gradle", ".dart_tool", "DerivedData", "Pods",
];

pub fn is_dev_dir_name(name: &str) -> bool {
    DEV_DIR_NAMES.contains(&name)
}

pub fn in_regenerable_dir(path: &Path) -> bool {
    // Generic `cache` names are fine to relink but too risky to delete wholesale.
    const EXTRA: &[&str] = &["cache", "caches"];
    path.ancestors().skip(1).any(|a| {
        a.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| is_dev_dir_name(n) || EXTRA.contains(&n))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardlink_fallback_shares_inode() {
        let dir = std::env::temp_dir().join(format!("sweep_reflink_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let keeper = dir.join("keeper.bin");
        std::fs::write(&keeper, b"hello").unwrap();
        let target = dir.join("target.bin");
        std::fs::write(&target, b"old").unwrap();
        let out = link_replace(&keeper, &target, true).unwrap();
        assert!(matches!(out, LinkOutcome::Reflinked | LinkOutcome::Hardlinked));
        assert_eq!(std::fs::read(&target).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skipped_when_unsafe_and_no_reflink() {
        let dir = std::env::temp_dir().join(format!("sweep_reflink_safe_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let keeper = dir.join("keeper.bin");
        std::fs::write(&keeper, b"hello").unwrap();
        #[allow(unused_mut)]
        let mut target = dir.join("target.bin");
        std::fs::write(&target, b"old").unwrap();
        // may_hardlink=false and on Windows reflink is always unsupported → skip
        let out = link_replace(&keeper, &target, false).unwrap();
        assert!(matches!(out, LinkOutcome::SkippedMutable));
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        target.set_extension("bin");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
