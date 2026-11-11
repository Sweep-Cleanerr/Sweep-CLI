//! Space reserve: a real file holding blocks so the disk never
//! quite hits 0 bytes.

use std::path::PathBuf;

pub const RESERVE_FILE_NAME: &str = "reserve.bin";
pub const DEFAULT_RESERVE_MB: u64 = 512;
pub const CONSUME_BELOW_MB: u64 = 256;
pub const RECREATE_ABOVE_MB: u64 = 2048;

pub fn reserve_path() -> PathBuf {
    if let Some(base) = crate::config::portable_base() {
        return base.join(RESERVE_FILE_NAME);
    }
    if let Some(dir) = crate::platform::config_dir() {
        return dir.join(RESERVE_FILE_NAME);
    }
    PathBuf::from(RESERVE_FILE_NAME)
}

pub fn has_reserve() -> bool {
    reserve_path().is_file()
}

pub fn reserve_size() -> u64 {
    std::fs::metadata(reserve_path())
        .map(|m| crate::fsutil::size::allocated_size(&m))
        .unwrap_or(0)
}

pub fn ensure_reserve(mb: u64) -> std::io::Result<u64> {
    if mb == 0 {
        return Ok(0);
    }
    let path = reserve_path();
    let bytes = mb.saturating_mul(1024 * 1024);
    if let Ok(md) = std::fs::metadata(&path) {
        if md.is_file() {
            let allocated = crate::fsutil::size::allocated_size(&md);
            if allocated >= bytes {
                return Ok(0);
            }
            let _ = std::fs::remove_file(&path);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(&path)?;
    file.set_len(bytes)?;
    allocate_blocks(&path, &file, bytes)?;
    Ok(bytes)
}

fn allocate_blocks(_path: &std::path::Path, mut file: &std::fs::File, bytes: u64) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        const FALLOC_FL_KEEP_SIZE: i32 = 0x01;
        const FALLOC_FL_PUNCH_HOLE: i32 = 0x02;
        let _ = (FALLOC_FL_PUNCH_HOLE, path);
        let rc = unsafe { libc::fallocate(file.as_raw_fd(), FALLOC_FL_KEEP_SIZE, 0, bytes as _) };
        if rc == 0 {
            return Ok(());
        }
    }
    let chunk = vec![0u8; 1024 * 1024];
    let mut offset = 0u64;
    use std::io::{Seek, SeekFrom, Write};
    while offset < bytes {
        let n = (bytes - offset).min(chunk.len() as u64) as usize;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&chunk[..n])?;
        offset += n as u64;
    }
    file.sync_all()
}

pub fn consume_reserve() -> Option<u64> {
    let path = reserve_path();
    if !path.is_file() {
        return None;
    }
    let size = reserve_size();
    let _ = std::fs::remove_file(&path);
    Some(size)
}

pub fn free_bytes_on_config_volume() -> Option<u64> {
    let path = reserve_path();
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty())?;
    crate::platform::free_space(parent).ok()
}

pub fn should_consume_with(free_bytes: u64, has: bool) -> bool {
    has && free_bytes < CONSUME_BELOW_MB * 1024 * 1024
}

pub fn should_recreate_with(free_bytes: u64, has: bool) -> bool {
    !has || free_bytes > RECREATE_ABOVE_MB * 1024 * 1024
}

pub fn should_consume(free_bytes: u64) -> bool {
    should_consume_with(free_bytes, has_reserve())
}

pub fn should_recreate(free_bytes: u64) -> bool {
    should_recreate_with(free_bytes, has_reserve())
}

pub fn is_disk_full_error(err: &std::io::Error) -> bool {
    matches!(err.kind(), std::io::ErrorKind::StorageFull) || matches!(err.raw_os_error(), Some(28) | Some(112))
}

pub fn try_recreate_reserve(mb: u64) {
    let _ = ensure_reserve(mb);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_are_decided_without_io() {
        assert!(should_consume_with(100 * 1024 * 1024, true));
        assert!(!should_consume_with(500 * 1024 * 1024, true));
        assert!(!should_consume_with(0, false));
        assert!(should_recreate_with(3000 * 1024 * 1024, false));
        assert!(!should_recreate_with(100 * 1024 * 1024, true));
    }

    #[test]
    fn disk_full_errors_are_recognized() {
        let err = std::io::Error::from_raw_os_error(28);
        assert!(is_disk_full_error(&err));
        let err = std::io::Error::from_raw_os_error(2);
        assert!(!is_disk_full_error(&err));
    }
}
