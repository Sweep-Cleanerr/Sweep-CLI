//! Free-space wiping: overwrite unallocated space on a volume.

use std::path::{Path, PathBuf};

use crate::core::error::{Error, Result};

/// Overwrite free space on the volume containing `root`, up to `max_bytes`.
///
/// Creates a temp file in the target directory and fills it with zeros
/// until the disk is full or `max_bytes` is reached, then deletes it.
/// Returns bytes written. `cancel` (a shared flag) aborts early.
pub fn wipe_free_space(
    root: &Path,
    max_bytes: Option<u64>,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<u64> {
    let dir: PathBuf = if root.is_dir() {
        root.to_path_buf()
    } else {
        root.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
    };

    if !dir.is_dir() {
        return Err(Error::Io {
            path: dir.clone(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        });
    }

    let file = dir.join(format!(".sweep-wipe-{}", std::process::id()));
    let mut created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .map_err(|err| Error::io(&file, err))?;

    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(file.clone());

    let chunk = vec![0u8; 1024 * 64];
    let mut wiped: u64 = 0;

    use std::io::Write;
    loop {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        if max_bytes.is_some_and(|max| wiped >= max) {
            break;
        }
        let to_write = match max_bytes {
            Some(max) => (max - wiped).min(chunk.len() as u64) as usize,
            None => chunk.len(),
        };
        match created.write(&chunk[..to_write]) {
            Ok(n) if n > 0 => {
                wiped = wiped.saturating_add(n as u64);
            }
            Ok(_) => break,
            Err(_) => break, // ENOSPC and friends: we're done
        }
        let _ = created.flush();
    }
    drop(created);
    let _ = std::fs::remove_file(&file);
    Ok(wiped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wipe_creates_then_removes_its_file() {
        let dir = crate::fixture("fs_freespace");
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wiped = wipe_free_space(&dir, Some(1024 * 1024), &cancel).unwrap();
        assert_eq!(wiped, 1024 * 1024);
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".sweep-wipe"))
            .collect();
        assert!(leftovers.is_empty(), "wipe file must be gone");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
