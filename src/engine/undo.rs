//! Undo: restore files from `--backup-dir` using the operation log.

use std::path::{Path, PathBuf};

use crate::core::report::{Entry, EntryKind, Report};
use crate::engine::{logline, trash};

pub const CLEANER_ID: &str = "undo";

#[derive(Debug, Clone)]
pub struct Restore {
    pub original: PathBuf,
    pub backup: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub enum Scope {
    #[default]
    All,
    Last,
    Run(String),
}

#[derive(Debug, Clone)]
pub struct Run {
    pub id: String,
    pub ts: u64,
    pub selection: String,
    pub files: usize,
    pub bytes: u64,
    pub restorable: usize,
}

fn find_backup(original: &Path, backup_root: &Path) -> Option<PathBuf> {
    let fresh = trash::mirrored_dest(backup_root, original);
    if fresh.is_file() {
        return Some(fresh);
    }
    let legacy = trash::mirrored_dest_legacy(backup_root, original);
    if legacy != fresh && legacy.is_file() {
        return Some(legacy);
    }
    None
}

fn restore_for(parsed: &logline::LogLine, backup_root: &Path) -> Option<Restore> {
    if parsed.mode != logline::Mode::Clean {
        return None;
    }
    let backup = find_backup(&parsed.path, backup_root)?;
    Some(Restore {
        original: parsed.path.clone(),
        backup,
        bytes: parsed.bytes,
    })
}

struct Segment {
    id: Option<String>,
    ts: u64,
    selection: String,
    cleans: Vec<logline::LogLine>,
}

fn segments(log_path: &Path) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut cur = Segment { id: None, ts: 0, selection: String::new(), cleans: Vec::new() };
    let mut touched = false;
    for line in logline::parse_chain(log_path) {
        if line.mode == logline::Mode::Run {
            if touched || !cur.cleans.is_empty() {
                out.push(std::mem::replace(&mut cur, Segment { id: None, ts: 0, selection: String::new(), cleans: Vec::new() }));
            }
            cur.id = Some(line.cleaner.clone());
            cur.ts = line.ts;
            cur.selection = line.path.to_string_lossy().into_owned();
            touched = true;
            continue;
        }
        if !line.mode.is_deletion() {
            continue;
        }
        touched = true;
        cur.cleans.push(line);
    }
    if touched {
        out.push(cur);
    }
    out
}

pub fn runs(log_path: &Path, backup_root: &Path) -> Vec<Run> {
    let mut runs: Vec<Run> = segments(log_path)
        .into_iter()
        .filter(|seg| !seg.cleans.is_empty())
        .map(|seg| {
            let files = seg.cleans.len();
            let bytes = seg.cleans.iter().map(|l| l.bytes).sum();
            let restorable = seg.cleans.iter().filter(|l| restore_for(l, backup_root).is_some()).count();
            Run {
                id: seg.id.clone().unwrap_or_else(|| "(no marker)".to_string()),
                ts: seg.ts,
                selection: seg.selection,
                files,
                bytes,
                restorable,
            }
        })
        .collect();
    runs.reverse();
    runs
}

fn scoped_cleans(log_path: &Path, scope: &Scope) -> Vec<logline::LogLine> {
    let segs = segments(log_path);
    match scope {
        Scope::All => segs.into_iter().flat_map(|s| s.cleans).collect(),
        Scope::Last => {
            // Last run with cleans; fall back to whole log when no marker.
            let mut last: Option<Vec<logline::LogLine>> = None;
            for seg in segs.iter() {
                if !seg.cleans.is_empty() {
                    last = Some(seg.cleans.clone());
                }
            }
            last.unwrap_or_else(|| segs.iter().flat_map(|s| s.cleans.clone()).collect())
        }
        Scope::Run(id) => segs
            .into_iter()
            .filter(|s| s.id.as_deref() == Some(id.as_str()))
            .flat_map(|s| s.cleans)
            .collect(),
    }
}

pub fn collect(log_path: &Path, backup_root: &Path) -> (Vec<Restore>, usize) {
    collect_scoped(log_path, backup_root, &Scope::All)
}

pub fn collect_scoped(log_path: &Path, backup_root: &Path, scope: &Scope) -> (Vec<Restore>, usize) {
    let lines = scoped_cleans(log_path, scope);
    let count = lines.len();
    let mut seen = std::collections::HashSet::new();
    let mut restores = Vec::new();
    for line in &lines {
        if let Some(r) = restore_for(line, backup_root) {
            if seen.insert(r.original.clone()) {
                restores.push(r);
            }
        }
    }
    (restores, count)
}

pub fn list_runs(log_path: &Path, backup_root: &Path) -> Report {
    let mut report = Report::new();
    let runs = runs(log_path, backup_root);
    if runs.is_empty() {
        report.push(Entry::new(EntryKind::Command, CLEANER_ID, "runs", "no recorded runs", None, 0));
        return report;
    }
    for run in &runs {
        report.push(Entry::new(
            EntryKind::Command,
            &run.id,
            "run",
            format!("{}: {} file(s), {} restorable", run.selection, run.files, run.restorable),
            None,
            run.bytes,
        ));
    }
    report
}

fn restore_one(r: &Restore) -> std::io::Result<()> {
    if let Some(parent) = r.original.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if r.original.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "target exists"));
    }
    std::fs::copy(&r.backup, &r.original)?;
    Ok(())
}

/// Restore everything in scope. Never overwrites an existing file.
pub fn scan(log_path: &Path, backup_root: &Path, scope: &Scope, dry_run: bool) -> Report {
    let mut report = Report::new();
    if !backup_root.is_dir() {
        report.fail(CLEANER_ID, "undo", format!("no backup dir: {}", backup_root.display()));
        return report;
    }
    if !log_path.as_os_str().is_empty() && !log_path.is_absolute() {
        report.fail(CLEANER_ID, "undo", format!("log path must be absolute: {}", log_path.display()));
        return report;
    }
    let (restores, total) = collect_scoped(log_path, backup_root, scope);
    if restores.is_empty() && total == 0 {
        report.push(Entry::new(EntryKind::Command, CLEANER_ID, "undo", "nothing to restore", None, 0));
        return report;
    }
    for r in &restores {
        if dry_run {
            report.push(Entry::new(EntryKind::Command, CLEANER_ID, "undo", format!("would restore {}", r.original.display()), Some(&r.original), r.bytes));
            continue;
        }
        match restore_one(r) {
            Ok(()) => report.push(Entry::new(EntryKind::Command, CLEANER_ID, "undo", format!("restored {}", r.original.display()), Some(&r.original), r.bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => report.push(Entry::skipped(CLEANER_ID, "undo", &r.original)),
            Err(err) => report.fail(CLEANER_ID, "undo", format!("{}: {err}", r.original.display())),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::trash;

    fn setup(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = crate::fixture(tag);
        let backup_root = root.join("backup");
        let log = root.join("ops.log");
        let original = root.join("a.txt");
        std::fs::write(&original, b"data").unwrap();
        let dest = trash::mirrored_dest(&backup_root, &original);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"data").unwrap();
        std::fs::remove_file(&original).unwrap();
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        std::fs::write(&log, format!("{ts} | clean | demo.cache | {} | 4B\n", original.display())).unwrap();
        (root, backup_root, log)
    }

    #[test]
    fn collect_finds_backup() {
        let (root, backup_root, log) = setup("undo_collect");
        let (restores, total) = collect(&log, &backup_root);
        assert_eq!(total, 1);
        assert_eq!(restores.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let (root, backup_root, log) = setup("undo_dry");
        let report = scan(&log, &backup_root, &Scope::All, true);
        assert_eq!(report.entries.len(), 1);
        assert!(!root.join("a.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn real_restore_never_overwrites_existing() {
        let (root, backup_root, log) = setup("undo_real");
        std::fs::write(root.join("a.txt"), b"new").unwrap();
        let report = scan(&log, &backup_root, &Scope::All, false);
        // Skipped entry, no failure.
        assert!(report.errors.is_empty());
        assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&root);
    }
}
