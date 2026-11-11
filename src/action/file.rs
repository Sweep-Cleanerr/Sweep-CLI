//! Filesystem actions: delete, truncate and shred.
//!
//! With `--backup-dir` every deleted regular file is copied first (undoable).
//! With `--trash-dir` files are moved there instead of deleted. Every real
//! operation appends one line to the operation log when a log file is set.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::action::context::RunContext;
use crate::action::filter::ActionFilter;
use crate::action::safety;
use crate::core::error::Error;
use crate::core::report::{Entry, EntryKind, Report, Stopwatch};
use crate::definition::model::{
    expand_action_paths, ActionDef, SearchKind, VarSet,
};
use crate::engine::{logline, trash};
use crate::fsutil::{self, walk::ScanOptions};
use crate::shred::{self, ShredSpec};

pub fn run(
    def: &ActionDef,
    vars: &VarSet,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
) -> Report {
    let mut report = Report::new();
    let watch = Stopwatch::start();

    let filter = match ActionFilter::from_action(def) {
        Ok(filter) => filter,
        Err(error) => {
            report.fail(cleaner, option, error.to_string());
            report.finish(watch);
            return report;
        }
    };

    let mut candidates = Vec::new();
    for root in expand_action_paths(def, vars) {
        if root.as_os_str().is_empty() {
            log::debug!("cleaner '{cleaner}' option '{option}': empty action path");
            continue;
        }
        match collect(def, &root, &filter) {
            Ok(mut found) => candidates.append(&mut found),
            Err(error) => report.fail(cleaner, option, error.to_string()),
        }
    }

    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
    if is_glob(def) {
        candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.components().count()));
    }

    let mut git_cache = crate::core::gitguard::GitCache::new();
    for candidate in candidates {
        if ctx.cancelled() {
            report.aborted = true;
            break;
        }
        process(
            &candidate,
            def,
            &filter,
            ctx,
            cleaner,
            option,
            &mut git_cache,
            &mut report,
        );
    }

    report.finish(watch);
    report
}

fn is_glob(def: &ActionDef) -> bool {
    matches!(def.search, SearchKind::Glob)
}

fn collect(def: &ActionDef, root: &Path, filter: &ActionFilter) -> Result<Vec<PathBuf>, Error> {
    match def.search {
        SearchKind::File => {
            if root.symlink_metadata().is_ok() {
                Ok(vec![root.to_path_buf()])
            } else {
                Ok(Vec::new())
            }
        }
        SearchKind::Glob => {
            let matches = fsutil::walk::glob_paths(&root.to_string_lossy())?;
            Ok(matches
                .into_iter()
                .filter(|candidate| filter.accepts_name(candidate))
                .collect())
        }
        SearchKind::WalkFiles => Ok(fsutil::walk::scan_paths(root, &ScanOptions::files())),
        SearchKind::WalkAll => {
            let mut options = ScanOptions::all();
            options.max_depth = def.max_depth;
            Ok(fsutil::walk::scan_paths(root, &options))
        }
        SearchKind::WalkTop => {
            let mut options = ScanOptions::all();
            options.max_depth = Some(def.max_depth.unwrap_or(1));
            Ok(fsutil::walk::scan_paths(root, &options))
        }
        SearchKind::Deep => Ok(Vec::new()),
    }
}

fn process(
    path: &Path,
    def: &ActionDef,
    filter: &ActionFilter,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
    git_cache: &mut crate::core::gitguard::GitCache,
    report: &mut Report,
) {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            report.warn(cleaner, option, Error::io(path, error).to_string());
            return;
        }
    };

    if !filter.accepts_with(path, &metadata) {
        return;
    }

    let is_link =
        metadata.file_type().is_symlink() || crate::platform::is_reparse_point_metadata(&metadata);
    let follow_links = matches!(def.search, SearchKind::File);

    // Links keep their own guard check; regular paths get the same guard plus
    // the critical-tree checks inside safety::check_path.
    if is_link {
        match ctx.guard.check(path, follow_links) {
            Ok(_) | Err(Error::Link(_)) => {}
            Err(error) => {
                report.block(cleaner, option, error.to_string());
                return;
            }
        }
    } else if let Err(reason) = safety::check_path(path, ctx) {
        report.block(cleaner, option, reason);
        return;
    }

    match git_cache.check_kind(path, metadata.is_dir(), ctx.git_guard) {
        crate::core::gitguard::Verdict::Allow => {}
        crate::core::gitguard::Verdict::Warn(msg) => {
            report.warn(cleaner, option, format!("{}: {msg}", path.display()))
        }
        crate::core::gitguard::Verdict::Block(msg) => {
            report.block(cleaner, option, format!("{}: {msg}", path.display()));
            return;
        }
    }

    let bytes = if metadata.is_file() && !is_link {
        metadata.len()
    } else {
        0
    };

    let command = def.command.trim().to_ascii_lowercase();
    let wants_shred = command == "shred" || ctx.global_shred;

    if def.command.trim().eq_ignore_ascii_case("truncate") {
        if metadata.is_dir() || is_link {
            report.push(Entry::skipped(cleaner, option, path));
            return;
        }
        if ctx.dry_run {
            report.push(Entry::new(
                EntryKind::Truncate,
                cleaner,
                option,
                EntryKind::Truncate.label(),
                Some(path),
                bytes,
            ));
            return;
        }
        match shred::truncate_contents(path) {
            Ok(()) => {
                git_cache.invalidate();
                log_operation(ctx, cleaner, option, path, bytes, logline::Mode::Truncate);
                report.push(Entry::new(
                    EntryKind::Truncate,
                    cleaner,
                    option,
                    EntryKind::Truncate.label(),
                    Some(path),
                    bytes,
                ));
            }
            Err(error) if safety::path_vanished(&error) => {}
            Err(error) => report.fail(
                cleaner,
                option,
                safety::delete_failure_message(path, &error),
            ),
        }
        return;
    }

    // A directory named by `search = "file"` is not implicitly recursive.
    if metadata.is_dir() && !is_link && follow_links {
        report.push(Entry::skipped(cleaner, option, path));
        return;
    }

    if ctx.dry_run {
        report.push(Entry::new(
            if wants_shred { EntryKind::Shred } else { EntryKind::Delete },
            cleaner,
            option,
            if wants_shred { EntryKind::Shred.label() } else { EntryKind::Delete.label() },
            Some(path),
            bytes,
        ));
        return;
    }

    // Backup before destroying (regular files only).
    if !wants_shred {
        if let BackupOutcome::Failed = maybe_backup(ctx, path) {
            report.fail(
                cleaner,
                option,
                format!("backup failed, deletion not undoable: {}", path.display()),
            );
        }
    }

    let result = if wants_shred {
        if is_link || metadata.is_dir() {
            if is_link {
                std::fs::remove_file(path).map(|_| true).map_err(|e| Error::io(path, e))
            } else {
                fsutil::delete(path, fsutil::DeleteOptions::simple())
            }
        } else {
            shred::shred_file(path, ShredSpec::default_spec()).map(|_| true)
        }
    } else if let Some(trash_root) = &ctx.trash_dir {
        if metadata.is_file() && !is_link {
            trash::stage(trash_root, path).map(|_| true).map_err(|e| Error::io(path, e))
        } else {
            fsutil::delete(path, fsutil::DeleteOptions::simple())
        }
    } else {
        fsutil::delete(path, fsutil::DeleteOptions::simple())
    };

    match result {
        Ok(true) => {
            // The deletion changed git status; never reuse the pre-delete view.
            git_cache.invalidate();
            log_operation(
                ctx,
                cleaner,
                option,
                path,
                bytes,
                if wants_shred {
                    logline::Mode::Shred
                } else if ctx.trash_dir.is_some() && metadata.is_file() && !is_link {
                    logline::Mode::Clean
                } else {
                    logline::Mode::Clean
                },
            );
            report.push(Entry::new(
                if wants_shred { EntryKind::Shred } else { EntryKind::Delete },
                cleaner,
                option,
                if wants_shred { EntryKind::Shred.label() } else { EntryKind::Delete.label() },
                Some(path),
                bytes,
            ));
        }
        Ok(false) => report.push(Entry::skipped(cleaner, option, path)),
        Err(error) if safety::path_vanished(&error) => {
            log::debug!("already gone: {}", path.display());
        }
        Err(error) => {
            let message = safety::delete_failure_message(path, &error);
            if error.is_benign() {
                report.warn(cleaner, option, message);
            } else {
                report.fail(cleaner, option, message);
            }
        }
    }
}

fn maybe_backup(ctx: &RunContext, path: &Path) -> BackupOutcome {
    use std::sync::atomic::Ordering::Relaxed;
    let Some(root) = &ctx.backup_dir else {
        return BackupOutcome::NotNeeded;
    };
    if ctx.dry_run {
        return BackupOutcome::NotNeeded;
    }
    let Ok(md) = std::fs::symlink_metadata(path) else {
        return BackupOutcome::NotNeeded;
    };
    if !md.file_type().is_file() {
        return BackupOutcome::NotNeeded;
    }
    if ctx.backup_max_bytes > 0 && md.len() > ctx.backup_max_bytes {
        ctx.backup_failures.fetch_add(1, Relaxed);
        return BackupOutcome::SkippedTooLarge;
    }
    let dest = trash::mirrored_dest(root, path);
    if let Some(parent) = dest.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            ctx.backup_failures.fetch_add(1, Relaxed);
            return BackupOutcome::Failed;
        }
    }
    match durable_copy(path, &dest) {
        Ok(()) => {
            if ctx.backup_max_bytes > 0 && fsutil::size_of_or_zero(&dest) > ctx.backup_max_bytes {
                let _ = std::fs::remove_file(&dest);
                ctx.backup_failures.fetch_add(1, Relaxed);
                return BackupOutcome::SkippedTooLarge;
            }
            BackupOutcome::Done
        }
        Err(_) => {
            ctx.backup_failures.fetch_add(1, Relaxed);
            BackupOutcome::Failed
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackupOutcome {
    NotNeeded,
    Done,
    SkippedTooLarge,
    Failed,
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

fn log_operation(ctx: &RunContext, cleaner: &str, option: &str, path: &Path, bytes: u64, mode: logline::Mode) {
    let Some(log_path) = &ctx.log_file else {
        return;
    };
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("{ts} | {} | {}.{} | {} | {bytes}B\n", mode.as_str(), cleaner, option, path.display());
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = file.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::model::ActionDef;

    fn action(path: &Path, search: SearchKind) -> ActionDef {
        ActionDef {
            command: "delete".to_string(),
            path: path.to_string_lossy().into_owned(),
            search,
            ..ActionDef::default()
        }
    }

    fn report_for(action: &ActionDef, ctx: &RunContext) -> Report {
        run(action, &VarSet::new(), ctx, "test", "cache")
    }

    #[test]
    fn previews_and_keeps_files() {
        let root = crate::fixture("action_file_preview");
        let file = root.join("cache.tmp");
        std::fs::write(&file, b"payload").unwrap();

        let report = report_for(&action(&file, SearchKind::File), &RunContext::preview());
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].kind, EntryKind::Delete);
        assert_eq!(report.entries[0].reclaimed, 7);
        assert!(file.exists());
    }

    #[test]
    fn deletes_files_for_real() {
        let root = crate::fixture("action_file_delete");
        let file = root.join("cache.tmp");
        std::fs::write(&file, b"payload").unwrap();

        let report = report_for(&action(&file, SearchKind::File), &RunContext::clean());
        assert_eq!(report.entries.len(), 1);
        assert!(!file.exists());
    }

    #[test]
    fn previewing_walk_all_reports_children_without_removing_them() {
        let root = crate::fixture("action_file_walk");
        let nested = root.join("nested");
        std::fs::create_dir(&nested).unwrap();
        let file = nested.join("cache.tmp");
        std::fs::write(&file, b"x").unwrap();

        let report = report_for(&action(&root, SearchKind::WalkAll), &RunContext::preview());
        assert!(report.entries.iter().any(|entry| entry.path.as_deref() == Some(file.as_path())));
        assert!(file.exists());
        assert!(nested.exists());
    }

    #[test]
    fn truncate_empties_the_file() {
        let root = crate::fixture("action_file_truncate");
        let file = root.join("log.txt");
        std::fs::write(&file, b"0123456789").unwrap();
        let mut def = action(&file, SearchKind::File);
        def.command = "truncate".to_string();

        let report = report_for(&def, &RunContext::clean());
        assert_eq!(report.entries[0].kind, EntryKind::Truncate);
        assert_eq!(std::fs::metadata(&file).unwrap().len(), 0);
    }

    #[test]
    fn global_shred_fails_closed() {
        let root = crate::fixture("action_file_shred");
        let file = root.join("secret.txt");
        std::fs::write(&file, b"secret").unwrap();
        let mut ctx = RunContext::clean();
        ctx.global_shred = true;

        let report = report_for(&action(&file, SearchKind::File), &ctx);
        assert!(!file.exists());
        assert!(report.errors.is_empty());
        assert_eq!(report.entries[0].kind, EntryKind::Shred);
    }

    #[test]
    fn shred_command_removes_file() {
        let root = crate::fixture("action_file_shred_cmd");
        let file = root.join("secret.txt");
        std::fs::write(&file, b"secret").unwrap();
        let mut def = action(&file, SearchKind::File);
        def.command = "shred".to_string();
        let report = report_for(&def, &RunContext::clean());
        assert!(!file.exists());
        assert_eq!(report.entries[0].kind, EntryKind::Shred);
    }

    #[test]
    fn a_file_search_on_a_directory_is_not_recursive() {
        let root = crate::fixture("action_file_dir");
        let file = root.join("child.txt");
        std::fs::write(&file, b"x").unwrap();

        let report = report_for(&action(&root, SearchKind::File), &RunContext::clean());
        assert_eq!(report.entries[0].kind, EntryKind::Skip);
        assert!(file.exists());
    }

    #[test]
    fn cancellation_stops_before_new_work() {
        let root = crate::fixture("action_file_cancel");
        let file = root.join("cache.tmp");
        std::fs::write(&file, b"x").unwrap();
        let ctx = RunContext::clean();
        ctx.request_cancel();

        let report = report_for(&action(&file, SearchKind::File), &ctx);
        assert!(report.aborted);
        assert!(file.exists());
    }

    fn git_ok(root: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .is_ok_and(|status| status.success())
    }

    #[test]
    fn a_deletion_refreshes_the_git_state_for_the_next_file() {
        let root = crate::fixture("action_file_git_refresh");
        let cache_dir = root.join("cache");
        std::fs::create_dir(&cache_dir).unwrap();
        let first = cache_dir.join("a.tmp");
        let second = cache_dir.join("b.tmp");
        std::fs::write(&first, b"a").unwrap();
        std::fs::write(&second, b"b").unwrap();

        // Git is optional in some test environments; skip when unavailable.
        if !git_ok(&root, &["init", "--quiet"])
            || !git_ok(&root, &["add", "cache/a.tmp", "cache/b.tmp"])
            || !git_ok(
                &root,
                &[
                    "-c",
                    "user.name=Sweep",
                    "-c",
                    "user.email=sweep@example.invalid",
                    "commit",
                    "--quiet",
                    "-m",
                    "init",
                ],
            )
        {
            return;
        }

        let mut ctx = RunContext::clean();
        ctx.git_guard = crate::core::gitguard::Protection::Block;
        let report = report_for(&action(&cache_dir, SearchKind::WalkAll), &ctx);

        // The first deletion makes the repository dirty. If the cached status
        // survived, the second file would be deleted too.
        assert_eq!(
            usize::from(first.exists()) + usize::from(second.exists()),
            1
        );
        assert!(!report.blocked.is_empty());
        assert!(report.entries.iter().any(|entry| entry.kind == EntryKind::Delete));
    }

    #[cfg(windows)]
    #[test]
    fn preview_refuses_a_junction_to_a_protected_tree() {
        let root = crate::fixture("action_file_junction");
        let target = Path::new(r"C:\Windows");
        let link = root.join("windows");
        if !crate::junction(target, &link) {
            return;
        }
        let file = root.join("cache.tmp");
        std::fs::write(&file, b"x").unwrap();
        let ctx = RunContext::preview();

        let report = report_for(&action(&root, SearchKind::WalkAll), &ctx);
        assert!(file.exists());
        assert!(link.exists());
        assert!(report
            .entries
            .iter()
            .any(|entry| entry.path.as_deref() == Some(file.as_path())));
        assert!(!report.entries.iter().any(|entry| {
            entry
                .path
                .as_deref()
                .is_some_and(|path| path.starts_with(target))
        }));

        // An exact junction target is followed by `search = "file"`; the guard
        // must reject the real protected path instead of deleting the link.
        let exact = report_for(&action(&link, SearchKind::File), &ctx);
        assert!(exact.entries.is_empty());
        assert!(!exact.blocked.is_empty());
        assert!(crate::action::safety::check_path(&link, &ctx).is_err());
    }
}
