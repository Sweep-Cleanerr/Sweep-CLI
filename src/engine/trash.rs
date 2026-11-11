//! Transactional trash: move files, then purge later.
//!
//! Files are renamed into a mirrored tree under the trash root so they can
//! be restored by `undo`; `purge` deletes them for real. The `.sweep-trash`
//! marker prevents purging a random directory.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::core::report::{Entry, EntryKind, Report};

pub const CLEANER_ID: &str = "purge";
pub const MARKER: &str = ".sweep-trash";

pub fn is_trash_root(root: &Path) -> bool {
    root.is_dir() && root.join(MARKER).is_file()
}

pub fn mark_trash_root(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    let marker = root.join(MARKER);
    if !marker.exists() {
        std::fs::write(&marker, b"sweep transactional trash\n")?;
    }
    Ok(())
}

/// Mirror of an absolute original path under `root`: `C:\x\f` -> `root/drive-c/x/f`.
pub fn mirrored_dest(root: &Path, original: &Path) -> PathBuf {
    let text = original.to_string_lossy();
    let mut out = root.to_path_buf();
    if text.starts_with("\\\\") || text.starts_with("//") {
        let mut parts = text.trim_start_matches(['/', '\\']).split(['/', '\\']).filter(|p| !p.is_empty() && *p != "." && *p != "..");
        if let Some(server) = parts.next() {
            out.push(format!("unc-{}", server.to_lowercase()));
            for part in parts {
                out.push(part);
            }
            return out;
        }
    }
    let stripped = text.strip_prefix('/').or_else(|| text.strip_prefix('\\')).map(str::to_string).unwrap_or_else(|| {
        let mut s = text.as_ref();
        if s.len() >= 2 && s.as_bytes()[1] == b':' {
            let drive = s.as_bytes()[0].to_ascii_lowercase() as char;
            if drive.is_ascii_alphanumeric() {
                out.push(format!("drive-{drive}"));
            }
            s = &s[2..];
        }
        s.trim_start_matches(['/', '\\']).to_string()
    });
    for part in stripped.split(['/', '\\']).filter(|p| !p.is_empty() && *p != "." && *p != "..") {
        out.push(part);
    }
    out
}

pub fn mirrored_dest_legacy(root: &Path, original: &Path) -> PathBuf {
    let text = original.to_string_lossy();
    let stripped = text.strip_prefix('/').or_else(|| text.strip_prefix('\\')).map(str::to_string).unwrap_or_else(|| {
        let mut s = text.as_ref();
        if s.len() >= 2 && s.as_bytes()[1] == b':' {
            s = &s[2..];
        }
        s.trim_start_matches(['/', '\\']).to_string()
    });
    let mut out = root.to_path_buf();
    for part in stripped.split(['/', '\\']).filter(|p| !p.is_empty() && *p != "." && *p != "..") {
        out.push(part);
    }
    out
}

fn durable_copy(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::io::copy;
    let mut src = std::fs::File::open(from)?;
    let tmp = to.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut dst = std::fs::File::create(&tmp)?;
        copy(&mut src, &mut dst)?;
        dst.sync_all()?;
    }
    std::fs::rename(&tmp, to)
}

pub fn stage(root: &Path, original: &Path) -> std::io::Result<PathBuf> {
    mark_trash_root(root)?;
    let dest = mirrored_dest(root, original);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::rename(original, &dest) {
        Ok(()) => Ok(dest),
        Err(_) => {
            if dest.exists() {
                let _ = std::fs::remove_file(&dest);
                if std::fs::rename(original, &dest).is_ok() {
                    return Ok(dest);
                }
            }
            durable_copy(original, &dest)?;
            if let Err(e) = std::fs::remove_file(original) {
                let _ = std::fs::remove_file(&dest);
                return Err(e);
            }
            Ok(dest)
        }
    }
}

fn older_than(path: &Path, days: u64) -> bool {
    if days == 0 {
        return true;
    }
    let Ok(md) = std::fs::metadata(path) else { return false };
    let Ok(m) = md.modified() else { return false };
    SystemTime::now()
        .duration_since(m)
        .map(|age| age >= Duration::from_secs(days * 24 * 60 * 60))
        .unwrap_or(false)
}

pub fn purge(root: &Path, days: u64, dry_run: bool) -> Report {
    let mut report = Report::new();
    if !root.is_dir() {
        report.fail(CLEANER_ID, "trash", format!("no trash dir: {}", root.display()));
        return report;
    }
    if !is_trash_root(root) {
        report.fail(CLEANER_ID, "trash", format!("refusing to purge {}: not a sweep trash dir (missing {})", root.display(), MARKER));
        return report;
    }
    let mut stack = vec![root.to_path_buf()];
    let mut freed = 0u64;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.join(MARKER).is_file() && path == root {
                continue;
            }
            let Ok(md) = entry.metadata() else { continue };
            if md.is_dir() {
                stack.push(path);
            } else if md.is_file() {
                if !older_than(&path, days) {
                    continue;
                }
                let bytes = md.len();
                if dry_run {
                    report.push(Entry::new(EntryKind::Delete, CLEANER_ID, "trash", format!("would purge {}", path.display()), Some(path.as_path()), bytes));
                } else {
                    match std::fs::remove_file(&path) {
                        Ok(()) => {
                            freed = freed.saturating_add(bytes);
                            report.push(Entry::new(EntryKind::Delete, CLEANER_ID, "trash", format!("purged {}", path.display()), Some(path.as_path()), bytes));
                        }
                        Err(err) => report.warn(CLEANER_ID, "trash", format!("{}: {err}", path.display())),
                    }
                }
            }
        }
    }
    // Remove emptied dirs, deepest first.
    if !dry_run {
        let mut dirs: Vec<PathBuf> = vec![root.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                }
            }
            if dir != root && std::fs::read_dir(&dir).map(|mut it| it.next().is_none()).unwrap_or(false) {
                let _ = std::fs::remove_dir(&dir);
            }
        }
        let _ = freed;
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        crate::fixture(tag)
    }

    #[test]
    fn mirrored_paths_never_escape_root() {
        let root = Path::new("/t");
        let dest = mirrored_dest(root, Path::new("/a/../b/c"));
        assert!(dest.starts_with(root));
        assert!(!dest.to_string_lossy().contains(".."));
    }

    #[test]
    fn drive_letters_get_namespaces() {
        let root = Path::new("/t");
        let dest = mirrored_dest(root, Path::new("C:\\x\\f.txt"));
        assert!(dest.to_string_lossy().contains("drive-c"));
    }

    #[test]
    fn refuses_unmarked_root() {
        let root = scratch("trash_unmarked");
        let report = purge(&root, 0, false);
        assert!(!report.errors.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn purges_marked_root() {
        let root = scratch("trash_purge");
        mark_trash_root(&root).unwrap();
        let file = root.join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let report = purge(&root, 0, false);
        assert!(report.errors.is_empty());
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
