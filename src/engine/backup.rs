//! `--backup-dir` store: measure, list, prune.
//!
//! Backups are written by `maybe_backup` (see `action::file`) and read by
//! `undo` through the same mirrored layout (`engine::trash::mirrored_dest`).

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::core::report::{Entry, EntryKind, Report};

pub const CLEANER_ID: &str = "backup";

pub struct Group {
    pub name: String,
    pub bytes: u64,
    pub files: u64,
}

pub struct Stats {
    pub bytes: u64,
    pub files: u64,
    pub groups: Vec<Group>,
}

fn walk_files(root: &Path, mut visit: impl FnMut(&Path, &std::fs::Metadata)) -> std::io::Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(md) = entry.metadata() else { continue };
            if md.is_dir() {
                stack.push(path);
            } else if md.is_file() {
                visit(&path, &md);
            }
        }
    }
    Ok(())
}

pub fn stats(root: &Path) -> std::io::Result<Stats> {
    if !root.is_dir() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, root.display().to_string()));
    }
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut groups: std::collections::BTreeMap<String, (u64, u64)> = Default::default();
    walk_files(root, |path, md| {
        let size = md.len();
        bytes = bytes.saturating_add(size);
        files += 1;
        let name = path.strip_prefix(root).ok().and_then(|rest| rest.components().next()).map(|p| p.as_os_str().to_string_lossy().into_owned()).unwrap_or_else(|| "(root)".to_string());
        let slot = groups.entry(name).or_insert((0, 0));
        slot.0 = slot.0.saturating_add(size);
        slot.1 += 1;
    })?;
    let mut groups: Vec<Group> = groups.into_iter().map(|(name, (bytes, files))| Group { name, bytes, files }).collect();
    groups.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    Ok(Stats { bytes, files, groups })
}

pub fn list(root: &Path) -> Report {
    let mut report = Report::new();
    match stats(root) {
        Ok(stats) => {
            if stats.files == 0 {
                report.push(Entry::new(EntryKind::Command, CLEANER_ID, "empty", "backup store is empty", None, 0));
            } else {
                for group in &stats.groups {
                    report.push(Entry::new(EntryKind::Command, CLEANER_ID, "list", format!("{}: {} file(s)", group.name, group.files), None, group.bytes));
                }
            }
        }
        Err(err) => report.fail(CLEANER_ID, "stats", err.to_string()),
    }
    report
}

pub fn prune(root: &Path, older_than_days: u64, dry_run: bool) -> Report {
    let mut report = Report::new();
    if !root.is_dir() {
        report.fail(CLEANER_ID, "prune", format!("no backup dir: {}", root.display()));
        return report;
    }
    walk_files(root, |path, md| {
        let old = if older_than_days == 0 {
            true
        } else {
            md.modified().ok().and_then(|m| SystemTime::now().duration_since(m).ok()).map(|age| age >= Duration::from_secs(older_than_days * 24 * 60 * 60)).unwrap_or(false)
        };
        if !old {
            return;
        }
        let bytes = md.len();
        if dry_run {
            report.push(Entry::new(EntryKind::Delete, CLEANER_ID, "prune", format!("would prune {}", path.display()), Some(path), bytes));
        } else {
            match std::fs::remove_file(path) {
                Ok(()) => report.push(Entry::new(EntryKind::Delete, CLEANER_ID, "prune", format!("pruned {}", path.display()), Some(path), bytes)),
                Err(err) => report.warn(CLEANER_ID, "prune", format!("{}: {err}", path.display())),
            }
        }
    }).ok();
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_group_by_top_level() {
        let root = crate::fixture("backup_stats");
        let file = root.join("drive-c").join("a.txt");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"hello").unwrap();
        let stats = stats(&root).unwrap();
        assert_eq!(stats.files, 1);
        assert_eq!(stats.bytes, 5);
        assert_eq!(stats.groups[0].name, "drive-c");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prune_preview_deletes_nothing() {
        let root = crate::fixture("backup_prune");
        let file = root.join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let report = prune(&root, 0, true);
        assert!(report.entries.iter().any(|e| e.label.contains("would prune")));
        assert!(file.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
