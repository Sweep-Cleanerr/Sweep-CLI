//! Secure deletion primitives: multi-pass overwrite, then unlink.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pattern {
    #[default]
    Zeros,
    Ones,
    Random,
    Dod5220,
    Paranoid,
}

impl Pattern {
    pub fn passes(self) -> usize {
        match self {
            Pattern::Zeros | Pattern::Ones | Pattern::Random => 1,
            Pattern::Dod5220 => 3,
            Pattern::Paranoid => 7,
        }
    }

    pub fn filler(self, index: usize, len: usize) -> Vec<u8> {
        match self {
            Pattern::Zeros => vec![0u8; len],
            Pattern::Ones => vec![0xFFu8; len],
            Pattern::Random => random_bytes(len),
            Pattern::Dod5220 => match index {
                0 => vec![0u8; len],
                1 => vec![0xFFu8; len],
                _ => random_bytes(len),
            },
            Pattern::Paranoid => match index % 3 {
                0 => vec![0u8; len],
                1 => vec![0xFFu8; len],
                _ => random_bytes(len),
            },
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "zeros" | "zero" | "0" => Pattern::Zeros,
            "ones" | "one" | "0xff" => Pattern::Ones,
            "random" | "rand" => Pattern::Random,
            "dod" | "dod5220" | "dod522022mece" => Pattern::Dod5220,
            "paranoid" | "gutmann" => Pattern::Paranoid,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Pattern::Zeros => "zeros",
            Pattern::Ones => "ones",
            Pattern::Random => "random",
            Pattern::Dod5220 => "dod",
            Pattern::Paranoid => "paranoid",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ShredSpec {
    pub pattern: Pattern,
    pub extra_passes: u32,
    pub wipe_name: bool,
}

impl ShredSpec {
    pub fn default_spec() -> Self {
        Self {
            pattern: Pattern::Zeros,
            extra_passes: 0,
            wipe_name: true,
        }
    }

    pub fn total_passes(self) -> usize {
        self.pattern.passes() + self.extra_passes as usize
    }
}

static SEED: AtomicU64 = AtomicU64::new(0x9E3779B97F4A7C15);

/// Non-cryptographic xorshift stream, reseeded from OS entropy when present.
/// Enough for shred patterns (defeats leftover-data recovery, not a cipher).
fn random_bytes(len: usize) -> Vec<u8> {
    let mut state = SEED.swap(
        SEED.load(Ordering::Relaxed).wrapping_add(0x9E3779B97F4A7C15),
        Ordering::Relaxed,
    );
    // Mix OS entropy: /dev/urandom on Unix, system time everywhere.
    #[cfg(unix)]
    if let Ok(mut bytes) = std::fs::File::open("/dev/urandom") {
        let mut buf = [0u8; 8];
        if bytes.read_exact(&mut buf).is_ok() {
            state ^= u64::from_le_bytes(buf);
        }
    }
    if state == 0 {
        state = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64 ^ (std::process::id() as u64) << 32)
            .unwrap_or(0x2545F4914F6CDD1D);
    }
    let mut out = vec![0u8; len];
    for chunk in out.chunks_mut(8) {
        let mut x = state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        state = x;
        chunk.copy_from_slice(&x.to_le_bytes()[..chunk.len()]);
    }
    out
}

fn open_write_no_follow(path: &Path) -> std::io::Result<File> {
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing to open a link",
        ));
    }
    let mut options = OpenOptions::new();
    options.write(true).create(false).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn open_with_write_grant(path: &Path) -> Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        match open_write_no_follow(path) {
            Ok(f) => return Ok(f),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {}
            Err(e) => return Err(Error::io(path, e)),
        }
        // Open read-only without following, grant owner-write via fd, restore.
        let ro = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|e| Error::io(path, e))?;
        use std::os::unix::io::AsRawFd;
        let fd = ro.as_raw_fd();
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut st) } != 0 {
            return Err(Error::io(path, std::io::Error::last_os_error()));
        }
        let old_mode = st.st_mode;
        if unsafe { libc::fchmod(fd, old_mode | 0o200) } != 0 {
            return Err(Error::io(path, std::io::Error::last_os_error()));
        }
        let result = open_write_no_follow(path);
        if unsafe { libc::fchmod(fd, old_mode) } != 0 {
            log::warn!("could not restore permissions for {}", path.display());
        }
        return result.map_err(|e| Error::io(path, e));
    }
    #[cfg(not(unix))]
    {
        crate::platform::clear_readonly(path);
        open_write_no_follow(path).map_err(|e| Error::io(path, e))
    }
}

/// Overwrite contents in place; length untouched. Length is read from the
/// opened descriptor so growth between stat and open cannot hide a tail.
pub fn overwrite_contents(path: &Path, spec: ShredSpec) -> Result<()> {
    let mut file = match open_write_no_follow(path) {
        Ok(f) => f,
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            if crate::core::keep::is_link(path) {
                return Err(Error::Link(path.to_path_buf()));
            }
            open_with_write_grant(path)?
        }
        Err(err) => return Err(Error::io(path, err)),
    };

    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(());
    }
    const CHUNK: usize = 1024 * 1024;
    let window = CHUNK.min(usize::try_from(len).unwrap_or(CHUNK));
    for pass in 0..spec.total_passes() {
        let chunk = spec.pattern.filler(pass, window);
        file.seek(SeekFrom::Start(0))?;
        let mut remaining = len;
        while remaining > 0 {
            let want = usize::try_from(remaining).unwrap_or(usize::MAX).min(chunk.len());
            let slice = &chunk[..want];
            file.write_all(slice)?;
            remaining -= slice.len() as u64;
        }
        file.flush()?;
        file.sync_data()?;
    }
    Ok(())
}

pub fn random_name(len: usize) -> String {
    if len == 0 {
        return String::new();
    }
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    random_bytes(len)
        .iter()
        .map(|b| CHARS[(b >> 2) as usize % CHARS.len()] as char)
        .collect()
}

pub fn is_readonly_fs(err: &std::io::Error) -> bool {
    match err.raw_os_error() {
        #[cfg(unix)]
        Some(code) => code == libc::EROFS,
        #[cfg(windows)]
        Some(code) => code == 19,
        #[cfg(not(any(unix, windows)))]
        Some(_) => false,
        None => false,
    }
}

/// Rename to a random name with the same byte length (directory entry slack).
pub fn rename_random(path: &Path) -> Result<PathBuf> {
    let (head, tail) = match (path.parent(), path.file_name()) {
        (Some(head), Some(tail)) => (head, tail.to_string_lossy().to_string()),
        _ => return Ok(path.to_path_buf()),
    };
    let byte_len = tail.len();
    for _ in 0..100 {
        let candidate = head.join(random_name(byte_len));
        if candidate == path || !candidate.exists() {
            return match std::fs::rename(path, &candidate) {
                Ok(()) => Ok(candidate),
                Err(err) if matches!(err.kind(), std::io::ErrorKind::PermissionDenied) || is_readonly_fs(&err) => {
                    log::debug!("cannot rename {}: {}", path.display(), err);
                    Ok(path.to_path_buf())
                }
                Err(err) => Err(Error::io(path, err)),
            };
        }
    }
    Ok(path.to_path_buf())
}

pub fn shred_file(path: &Path, spec: ShredSpec) -> Result<()> {
    if crate::core::keep::is_link(path) {
        return std::fs::remove_file(path).map_err(|e| Error::io(path, e));
    }
    if crate::platform::is_hard_link(path) {
        std::fs::remove_file(path).map_err(|e| Error::io(path, e))?;
        return Ok(());
    }
    overwrite_contents(path, spec)?;
    let target = if spec.wipe_name {
        rename_random(path)?
    } else {
        path.to_path_buf()
    };
    std::fs::remove_file(&target).map_err(|e| Error::io(&target, e))?;
    sync_parent_dir_best_effort(&target);
    Ok(())
}

#[cfg(unix)]
fn sync_parent_dir_best_effort(path: &Path) {
    if let Some(parent) = path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
}

#[cfg(not(unix))]
fn sync_parent_dir_best_effort(_path: &Path) {}

/// Zero a file without following links; refuses read-only files.
pub fn truncate_contents(path: &Path) -> Result<()> {
    if crate::core::keep::is_link(path) {
        return Err(Error::Link(path.to_path_buf()));
    }
    let file = open_write_no_follow(path).map_err(|e| Error::io(path, e))?;
    file.set_len(0)?;
    file.sync_all()?;
    drop(file);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = crate::fixture(tag);
        let path = dir.join("f.txt");
        std::fs::write(&path, b"secret-data").unwrap();
        path
    }

    #[test]
    fn pattern_round_trip() {
        assert_eq!(Pattern::parse("zeros"), Some(Pattern::Zeros));
        assert_eq!(Pattern::parse("dod"), Some(Pattern::Dod5220));
        assert_eq!(Pattern::parse("nope"), None);
        assert_eq!(Pattern::Dod5220.as_str(), "dod");
    }

    #[test]
    fn pass_counts() {
        assert_eq!(Pattern::Zeros.passes(), 1);
        assert_eq!(Pattern::Dod5220.passes(), 3);
        assert_eq!(Pattern::Paranoid.passes(), 7);
        assert_eq!(ShredSpec::default_spec().total_passes(), 1);
    }

    #[test]
    fn overwrite_zeroes_content_and_keeps_length() {
        let path = scratch("shred_zero");
        overwrite_contents(&path, ShredSpec::default_spec()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), vec![0u8; 11]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn shred_removes_a_regular_file() {
        let path = scratch("shred_rm");
        shred_file(&path, ShredSpec::default_spec()).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn shred_unlinks_a_symlink_without_touching_the_target() {
        let target = scratch("shred_target");
        let link = target.parent().unwrap().join("link");
        #[cfg(unix)]
        if std::os::unix::fs::symlink(&target, &link).is_err() {
            return;
        }
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            let _ = std::fs::remove_dir_all(target.parent().unwrap());
            return;
        }
        shred_file(&link, ShredSpec::default_spec()).unwrap();
        assert!(!link.symlink_metadata().is_ok());
        assert_eq!(std::fs::read(&target).unwrap(), b"secret-data");
        let _ = std::fs::remove_dir_all(target.parent().unwrap());
    }

    #[test]
    fn empty_file_shreds_without_error() {
        let dir = crate::fixture("shred_empty");
        let path = dir.join("empty");
        std::fs::write(&path, b"").unwrap();
        shred_file(&path, ShredSpec::default_spec()).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
