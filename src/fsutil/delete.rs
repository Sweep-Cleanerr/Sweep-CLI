//! Deletion, truncation and the "file in use" fallbacks.
//!
//! Every failure is a typed [`Error`] instead of a panic, so the worker can
//! decide per case whether to abort or continue.
//!
//! Safety: this module performs NO keep-list, protected-path or link-target
//! checks. Every call site must pass through [`crate::core::keep::Guard::check`]
//! first — deleting without the guard is a security bug, not an optimization.

use std::fs;
use std::path::{Path, PathBuf};

use crate::core::error::{Error, Result};
use crate::platform;

/// Everything that controls a single delete.
///
/// Shred/spec fields will arrive with the shred module in a later stage.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeleteOptions {
    /// A missing path is not an error.
    pub ignore_missing: bool,
}

impl DeleteOptions {
    /// Plain unlink.
    pub fn simple() -> Self {
        Self::default()
    }

    /// Unlink, tolerating a path that is already gone (for example,
    /// temporary fill files left behind by a crashed wipe run).
    pub fn lenient() -> Self {
        Self {
            ignore_missing: true,
        }
    }
}

/// Deletes a file, directory, symlink, junction or FIFO.
///
/// Returns `true` when something was actually removed.
pub fn delete(path: &Path, options: DeleteOptions) -> Result<bool> {
    let path = &crate::core::path::extended_path(path);

    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if options.ignore_missing {
                return Ok(false);
            }

            return Err(Error::io(path, error));
        }
        Err(error) => return Err(Error::io(path, error)),
    };

    // Links, FIFOs and sockets: remove the name, never the target.
    if metadata.file_type().is_symlink() {
        return remove_any(path);
    }

    if !metadata.file_type().is_file() && !metadata.file_type().is_dir() {
        return remove_any(path);
    }

    #[cfg(windows)]
    if platform::is_reparse_point(path) {
        return remove_any(path);
    }

    if metadata.file_type().is_dir() {
        return delete_dir(path);
    }

    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) => {
            if platform::is_locked(&error) {
                // Best-effort truncation so the contents do not survive, then
                // report the failure. The caller may schedule a reboot delete.
                let _ = truncate_file(path);
                Err(Error::io(path, error))
            } else if platform::is_readonly_error(&error)
                && platform::clear_readonly(path)
            {
                fs::remove_file(path)
                    .map(|()| true)
                    .map_err(|error| Error::io(path, error))
            } else {
                Err(Error::io(path, error))
            }
        }
    }
}

fn remove_any(path: &Path) -> Result<bool> {
    remove_link(path)
        .map(|()| true)
        .or_else(|error| {
            if platform::is_readonly_error(&error)
                && platform::clear_readonly(path)
            {
                remove_link(path).map(|()| true)
            } else {
                Err(error)
            }
        })
        .map_err(|error| Error::io(path, error))
}

/// Unlinks a link/FIFO/socket name without touching its target.
///
/// A directory reparse point only goes away with `remove_dir`; `remove_file`
/// refuses it on Windows. Conversely, `remove_dir` on a file link fails
/// harmlessly, making it a safe fallback.
fn remove_link(path: &Path) -> std::io::Result<()> {
    let is_dir = fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_dir())
        .unwrap_or(false);

    if is_dir {
        return fs::remove_dir(path);
    }

    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(first_error) => fs::remove_dir(path).or(Err(first_error)),
    }
}

fn delete_dir(path: &Path) -> Result<bool> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(true),
        Err(error) => {
            let raw = error.raw_os_error();

            // Do not treat every ErrorKind::Other as "not empty": that would
            // silently report real failures as skips.
            if matches!(
                error.kind(),
                std::io::ErrorKind::DirectoryNotEmpty
            ) || raw == Some(145) // Windows ERROR_DIR_NOT_EMPTY
                || raw == Some(39) // Linux ENOTEMPTY
                || raw == Some(66) // macOS/BSD ENOTEMPTY
                || error.to_string().contains("not empty")
            {
                log::info!("directory is not empty: {}", path.display());
                Ok(false)
            } else if raw == Some(16) || raw == Some(32) {
                // EBUSY / ERROR_SHARING_VIOLATION: skip a mount point or
                // another busy resource rather than reporting its removal.
                log::info!("device or resource busy: {}", path.display());
                Ok(false)
            } else if platform::is_readonly_error(&error)
                && platform::clear_readonly(path)
            {
                fs::remove_dir(path)
                    .map(|()| true)
                    .map_err(|error| Error::io(path, error))
            } else {
                Err(Error::io(path, error))
            }
        }
    }
}

/// Truncates a file to zero bytes, refusing to follow a link.
pub fn truncate_file(path: &Path) -> Result<()> {
    if crate::core::keep::is_link(path) {
        return Err(Error::Link(path.to_path_buf()));
    }

    let mut options = fs::OpenOptions::new();
    options.write(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(libc::O_NOFOLLOW);
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        // FILE_FLAG_OPEN_REPARSE_POINT: if the name was swapped to a link
        // after the is_link check, open the link itself so `set_len` cannot
        // reach the target.
        options.attributes(0x0020_0000);
    }

    let file = options
        .open(path)
        .map_err(|error| Error::io(path, error))?;

    file.set_len(0)
        .map_err(|error| Error::io(path, error))?;

    file.sync_all()
        .map_err(|error| Error::io(path, error))?;

    Ok(())
}

/// Deletes a list of paths, returning how many actually disappeared.
///
/// Large lists run files in parallel, while directories remain sequential to
/// preserve the children-before-parent order supplied by the scanner.
pub fn delete_all(paths: &[PathBuf], options: DeleteOptions) -> usize {
    use rayon::prelude::*;

    if paths.len() < 64 {
        return paths
            .iter()
            .filter(|path| delete(path, options).unwrap_or(false))
            .count();
    }

    let (directories, files): (Vec<&PathBuf>, Vec<&PathBuf>) =
        paths.iter().partition(|path| path.is_dir());

    let mut removed = files
        .par_iter()
        .filter(|path| delete(path, options).unwrap_or(false))
        .count();

    for directory in directories {
        if delete(directory, options).unwrap_or(false) {
            removed += 1;
        }
    }

    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::walk::{scan_paths, ScanOptions};

    #[test]
    fn delete_file_then_missing() {
        let dir = crate::fixture("del_file");
        let file = dir.join("f.txt");

        std::fs::write(&file, b"x").unwrap();

        assert!(delete(&file, DeleteOptions::simple()).unwrap());
        assert!(!file.exists());

        assert!(!delete(&file, DeleteOptions::lenient()).unwrap());
        assert!(delete(&file, DeleteOptions::simple()).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_empty_dir_but_not_nonempty() {
        let dir = crate::fixture("del_dir");
        let empty = dir.join("empty");
        let full = dir.join("full");

        std::fs::create_dir(&empty).unwrap();
        std::fs::create_dir(&full).unwrap();
        std::fs::write(full.join("keep.txt"), b"x").unwrap();

        assert!(delete(&empty, DeleteOptions::simple()).unwrap());

        // Non-empty is a skip, not an error, and never a recursive delete.
        assert!(!delete(&full, DeleteOptions::simple()).unwrap());
        assert!(full.join("keep.txt").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_all_removes_contents_first() {
        let dir = crate::fixture("del_all");

        std::fs::create_dir_all(dir.join("sub/deep")).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        std::fs::write(dir.join("sub/b.txt"), b"x").unwrap();
        std::fs::write(dir.join("sub/deep/c.txt"), b"x").unwrap();

        let paths = scan_paths(&dir, &ScanOptions::all());

        assert_eq!(paths.len(), 5);
        assert_eq!(delete_all(&paths, DeleteOptions::simple()), 5);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_all_parallel_large_list() {
        let dir = crate::fixture("del_all_par");

        std::fs::create_dir_all(dir.join("sub")).unwrap();

        for index in 0..70 {
            std::fs::write(dir.join(format!("f{index}.tmp")), b"x").unwrap();
        }

        std::fs::write(dir.join("sub/g.txt"), b"x").unwrap();

        let paths = scan_paths(&dir, &ScanOptions::all());

        assert_eq!(paths.len(), 72);
        assert_eq!(delete_all(&paths, DeleteOptions::simple()), 72);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncate_file_zeroes_contents() {
        let dir = crate::fixture("del_truncate");
        let file = dir.join("f.txt");

        std::fs::write(&file, b"some contents here").unwrap();
        truncate_file(&file).unwrap();

        assert_eq!(std::fs::metadata(&file).unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deletes_readonly_file() {
        let dir = crate::fixture("del_readonly");
        let file = dir.join("ro.txt");

        std::fs::write(&file, b"x").unwrap();

        let mut permissions = std::fs::metadata(&file)
            .unwrap()
            .permissions();

        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions).unwrap();

        assert!(delete(&file, DeleteOptions::simple()).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_link_keeps_target() {
        let dir = crate::fixture("del_link");
        let target = dir.join("target.txt");
        let link = dir.join("link.txt");

        std::fs::write(&target, b"precious").unwrap();

        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &link).is_ok();

        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, &link).is_ok();

        #[cfg(not(any(unix, windows)))]
        let made = false;

        if !made {
            eprintln!(
                "skipping delete_link_keeps_target: symlink creation refused"
            );
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        assert!(delete(&link, DeleteOptions::simple()).unwrap());
        assert!(link.symlink_metadata().is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"precious");

        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &link).is_ok();

        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, &link).is_ok();

        #[cfg(not(any(unix, windows)))]
        let made = false;

        if made {
            assert!(truncate_file(&link).is_err());
            assert_eq!(std::fs::read(&target).unwrap(), b"precious");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn delete_junction_keeps_target() {
        let dir = crate::fixture("del_junction");
        let target = dir.join("target");
        let link = dir.join("junction");

        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("precious.txt"), b"precious").unwrap();

        // `mklink /J link target` creates a junction without requiring the
        // elevation normally needed for a Windows symbolic link.
        let created = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&link)
            .arg(&target)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);

        if !created {
            eprintln!(
                "skipping delete_junction_keeps_target: junction creation refused"
            );
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        assert!(truncate_file(&link).is_err());
        assert_eq!(
            std::fs::read(target.join("precious.txt")).unwrap(),
            b"precious"
        );

        assert!(delete(&link, DeleteOptions::simple()).unwrap());
        assert!(link.symlink_metadata().is_err());
        assert_eq!(
            std::fs::read(target.join("precious.txt")).unwrap(),
            b"precious"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
