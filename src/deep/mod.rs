//! Deep cleaners: Linux/macOS categories behind `system.*` actions.
//!
//! Each category deletes directory contents through the same two
//! gates as `action/file.rs` (Guard + safety). Package-manager
//! categories run their tool via `fsutil::run_command`.
//! Dry-run never mutates anything.

use std::path::{Path, PathBuf};

use crate::action::context::RunContext;
use crate::action::safety;
use crate::core::report::{Entry, EntryKind, Report};
use crate::fsutil::{self, walk::ScanOptions};
use crate::platform;

pub mod cleaners;

#[derive(Debug, Clone)]
pub struct PreviewItem {
    pub label: String,
    pub path: Option<PathBuf>,
    pub bytes: u64,
}

impl PreviewItem {
    pub fn new(label: impl Into<String>, path: Option<PathBuf>, bytes: u64) -> Self {
        Self {
            label: label.into(),
            path,
            bytes,
        }
    }
}

pub fn push_preview(
    report: &mut Report,
    cleaner: &str,
    option: &str,
    items: Vec<PreviewItem>,
) {
    for item in items {
        let kind = if item.path.is_some() {
            EntryKind::Delete
        } else {
            EntryKind::Command
        };
        report.push(Entry::new(
            kind,
            cleaner,
            option,
            item.label,
            item.path.as_deref(),
            item.bytes,
        ));
    }
}

pub fn guarded_delete(
    path: &Path,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
    report: &mut Report,
) -> bool {
    if let Err(reason) = safety::check_path(path, ctx) {
        log::warn!("skipped {}: {reason}", path.display());
        report.push(Entry::skipped(cleaner, option, path));
        return false;
    }
    let bytes = fsutil::size_of_or_zero(path);
    if ctx.dry_run {
        report.push(Entry::new(
            EntryKind::Delete,
            cleaner,
            option,
            format!("to clean: {}", path.display()),
            Some(path),
            bytes,
        ));
        return false;
    }
    let is_real_dir = path.is_dir()
        && std::fs::symlink_metadata(path)
            .map(|md| !md.file_type().is_symlink())
            .unwrap_or(false);
    if is_real_dir {
        clean_dir_contents(path, ctx, cleaner, option, report, 32);
        if ctx.cancelled() {
            report.aborted = true;
            return false;
        }
    }
    match fsutil::delete(path, fsutil::DeleteOptions::simple()) {
        Ok(true) => {
            report.push(Entry::new(
                EntryKind::Delete,
                cleaner,
                option,
                format!("cleaned {}", path.display()),
                Some(path),
                bytes,
            ));
            true
        }
        Ok(false) => false,
        Err(err) if safety::path_vanished(&err) => false,
        Err(err) => {
            report.fail(cleaner, option, err.to_string());
            false
        }
    }
}

/// Recursively delete the *contents* of `root` (never `root` itself).
pub fn clean_dir_contents(
    root: &Path,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
    report: &mut Report,
    max_depth: usize,
) {
    let options = ScanOptions::all()
        .with_max_depth(max_depth)
        .with_same_filesystem(true);
    for path in fsutil::scan_paths(root, &options) {
        if ctx.cancelled() {
            report.aborted = true;
            return;
        }
        guarded_delete(&path, ctx, cleaner, option, report);
    }
}

/// Delete contents of each existing root directory.
pub fn clean_roots(
    roots: Vec<PathBuf>,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
    report: &mut Report,
) {
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        if ctx.cancelled() {
            report.aborted = true;
            return;
        }
        clean_dir_contents(&root, ctx, cleaner, option, report, 32);
    }
}

/// Run an external tool for a category (dry-run reports only).
pub fn run_tool(
    program: &str,
    args: &[&str],
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
    report: &mut Report,
) {
    let cmd = format!("{} {}", program, args.join(" "));
    if ctx.dry_run {
        report.push(Entry::new(
            EntryKind::Command,
            cleaner,
            option,
            format!("would run: {cmd}"),
            None,
            0,
        ));
        return;
    }
    match fsutil::run_command(program, args, true) {
        Ok((0, _, _)) => report.push(Entry::new(
            EntryKind::Command,
            cleaner,
            option,
            format!("ran: {cmd}"),
            None,
            0,
        )),
        Ok((status, _, stderr)) => report.fail(
            cleaner,
            option,
            format!("command '{cmd}' exited with {status}: {stderr}"),
        ),
        Err(err) => report.fail(cleaner, option, format!("cannot run '{cmd}': {err}")),
    }
}

pub fn home_cache() -> Option<PathBuf> {
    platform::cache_dir().or_else(|| platform::home_dir().map(|h| h.join(".cache")))
}

pub fn home_share() -> Option<PathBuf> {
    platform::home_dir().map(|h| h.join(".local").join("share"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::keep::Guard;

    fn test_ctx() -> RunContext {
        RunContext::preview()
    }

    #[test]
    fn clean_roots_previews_without_deleting() {
        let dir = crate::fixture("deep_roots");
        std::fs::write(dir.join("f.txt"), b"hi").unwrap();
        let mut report = Report::new();
        clean_roots(vec![dir.clone()], &test_ctx(), "c", "o", &mut report);
        assert_eq!(report.entries.len(), 1);
        assert!(dir.join("f.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn guarded_delete_refuses_relative_paths() {
        let mut report = Report::new();
        let ctx = test_ctx();
        let ok = guarded_delete(Path::new("relative/file"), &ctx, "c", "o", &mut report);
        assert!(!ok);
        assert_eq!(report.skipped(), 1);
    }

    #[test]
    fn run_tool_dry_run_never_spawns() {
        let mut report = Report::new();
        run_tool("nonexistent-prog-xyz", &["x"], &test_ctx(), "c", "o", &mut report);
        assert_eq!(report.entries.len(), 1);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn deep_options_defaults_are_stable() {
        let d = crate::action::context::DeepOptions::default();
        assert_eq!(d.journal_days, 7);
        assert_eq!(d.pacman_keep, 2);
        let _ = Guard::default_guard();
    }
}
