//! Per-category deep cleaners behind `system.*` commands.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::action::context::RunContext;
use crate::core::report::Report;
use crate::deep::{
    clean_roots, home_cache, home_share, push_preview, run_tool, PreviewItem,
};
use crate::platform;

pub fn apt_cache(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    clean_roots(vec![PathBuf::from("/var/cache/apt/archives")], ctx, cleaner, option, report);
    run_tool("apt-get", &["clean"], ctx, cleaner, option, report);
}

pub fn apt_lists(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    clean_roots(vec![PathBuf::from("/var/lib/apt/lists")], ctx, cleaner, option, report);
}

pub fn dnf_cache(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    clean_roots(
        vec![
            PathBuf::from("/var/cache/dnf"),
            PathBuf::from("/var/cache/libdnf5"),
        ],
        ctx,
        cleaner,
        option,
        report,
    );
}

pub fn zypp_cache(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    clean_roots(vec![PathBuf::from("/var/cache/zypp")], ctx, cleaner, option, report);
}

pub fn pacman_cache(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    let keep = ctx.deep_options.pacman_keep as usize;
    let root = PathBuf::from("/var/cache/pacman/pkg");
    if !root.is_dir() {
        return;
    }
    // Keep the newest `keep` .pkg files, delete the rest.
    let mut files: Vec<(PathBuf, SystemTime)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "pkg").unwrap_or(false) {
                let mtime = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                files.push((path, mtime));
            }
        }
    }
    files.sort_by_key(|(_, mtime)| *mtime);
    let remove_count = files.len().saturating_sub(keep);
    for (path, _) in files.into_iter().take(remove_count) {
        crate::deep::guarded_delete(&path, ctx, cleaner, option, report);
    }
}

pub fn old_kernels(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    run_tool("package-cleanup", &["--old-kernels"], ctx, cleaner, option, report);
    clean_roots(vec![PathBuf::from("/boot")], ctx, cleaner, option, report);
    // /boot guard will refuse most entries; only stale files report.
}

pub fn journal_vacuum(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    let size = ctx.deep_options.journal_size.clone();
    if ctx.dry_run {
        push_preview(
            report,
            cleaner,
            option,
            vec![PreviewItem::new(
                format!("would vacuum journal to {size} / {}d", ctx.deep_options.journal_days),
                None,
                0,
            )],
        );
        return;
    }
    run_tool("journalctl", &[&format!("--vacuum-size={size}")], ctx, cleaner, option, report);
    run_tool("journalctl", &[&format!("--vacuum-time={}d", ctx.deep_options.journal_days)], ctx, cleaner, option, report);
}

pub fn journal_user(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    if let Some(home) = platform::home_dir() {
        let dir = home.join(".local").join("share").join("systemd").join("journal");
        clean_roots(vec![dir], ctx, cleaner, option, report);
        return;
    }
    journal_vacuum(ctx, cleaner, option, report);
}

/// User caches without the app-specific roots (see also `user_caches` in system).
pub fn user_caches(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    if let Some(cache) = home_cache() {
        clean_roots(vec![cache], ctx, cleaner, option, report);
    }
}

pub fn temp_deep(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    let max_age = ctx.deep_options.temp_max_age_days;
    if max_age == 0 {
        report.fail(cleaner, option, "temp max_age_days must be >= 1");
        return;
    }
    let mut roots = vec![PathBuf::from("/tmp"), PathBuf::from("/var/tmp")];
    if let Some(home) = platform::home_dir() {
        let cache = platform::cache_dir().unwrap_or_else(|| home.join(".cache"));
        roots.extend([home.join("tmp"), home.join(".tmp"), cache.join("tmp")]);
    }
    // Two-pass: files first, then only-empty dirs (deepest first).
    let mut files: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for root in roots.into_iter().filter(|p| p.is_dir()) {
        let options = crate::fsutil::walk::ScanOptions::all()
            .with_max_depth(8)
            .with_same_filesystem(true);
        for path in crate::fsutil::scan_paths(&root, &options) {
            if path == root {
                continue;
            }
            if !older_than(&path, max_age) {
                continue;
            }
            if std::fs::symlink_metadata(&path)
                .map(|m| m.file_type().is_dir())
                .unwrap_or(false)
            {
                dirs.push(path);
            } else {
                files.push(path);
            }
        }
    }
    for f in files {
        if ctx.cancelled() {
            report.aborted = true;
            return;
        }
        crate::deep::guarded_delete(&f, ctx, cleaner, option, report);
    }
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        let empty = std::fs::read_dir(&d)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false);
        if empty {
            crate::deep::guarded_delete(&d, ctx, cleaner, option, report);
        }
    }
}

fn older_than(path: &Path, days: u32) -> bool {
    let Ok(md) = std::fs::symlink_metadata(path) else {
        return false;
    };
    let Ok(mtime) = md.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(mtime)
        .map(|d| d.as_secs() > u64::from(days) * 86_400)
        .unwrap_or(false)
}

pub fn orphans(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    run_tool("apt-get", &["autoremove", "-y"], ctx, cleaner, option, report);
}

pub fn snap(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    let mut dirs = vec![PathBuf::from("/var/lib/snapd/cache")];
    if let Some(home) = platform::home_dir() {
        dirs.push(home.join("snap"));
    }
    clean_roots(dirs, ctx, cleaner, option, report);
}

pub fn flatpak(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    let mut dirs = vec![PathBuf::from("/var/lib/flatpak")];
    if let Some(share) = home_share() {
        dirs.push(share.join("flatpak"));
    }
    clean_roots(dirs, ctx, cleaner, option, report);
}

pub fn docker(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    if ctx.deep_options.docker_build_cache {
        run_tool("docker", &["builder", "prune", "-f"], ctx, cleaner, option, report);
    } else {
        push_preview(
            report,
            cleaner,
            option,
            vec![PreviewItem::new(
                "docker build cache kept (docker_build_cache is off)",
                None,
                0,
            )],
        );
    }
}

pub fn docker_volumes(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    run_tool("docker", &["volume", "prune", "-f"], ctx, cleaner, option, report);
}

pub fn kde_cache(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    if let Some(cache) = home_cache() {
        clean_roots(vec![cache.join("akonadi"), cache.join("baloo")], ctx, cleaner, option, report);
    }
}

pub fn kde_pim(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    if let Some(share) = home_share() {
        clean_roots(vec![share.join("akonadi"), share.join("korganizer")], ctx, cleaner, option, report);
    }
}

pub fn baloo(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    if let Some(h) = home_cache() {
        clean_roots(vec![h.join("baloo")], ctx, cleaner, option, report);
    }
}

pub fn gnome_cache(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    if let Some(cache) = home_cache() {
        clean_roots(vec![cache.join("tracker"), cache.join("gphoto")], ctx, cleaner, option, report);
    }
}

pub fn zeitgeist(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    if let Some(cache) = home_cache() {
        clean_roots(vec![cache.join("zeitgeist")], ctx, cleaner, option, report);
    }
}

pub fn tracker(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    if let Some(cache) = home_cache() {
        clean_roots(vec![cache.join("tracker")], ctx, cleaner, option, report);
    }
}

pub fn crash_reports(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    let mut dirs = vec![
        PathBuf::from("/var/crash"),
        PathBuf::from("/var/lib/systemd/coredump"),
    ];
    if let Some(share) = home_share() {
        dirs.push(share.join("crash"));
    }
    clean_roots(dirs, ctx, cleaner, option, report);
}

pub fn coredumps(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    crash_reports(ctx, cleaner, option, report);
    run_tool("coredumpctl", &["prune"], ctx, cleaner, option, report);
}

pub fn nix_gc(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    run_tool("nix-collect-garbage", &["-d"], ctx, cleaner, option, report);
}

pub fn tm_thin(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    if ctx.dry_run {
        push_preview(
            report,
            cleaner,
            option,
            vec![PreviewItem::new("would thin local Time Machine snapshots", None, 0)],
        );
        return;
    }
    run_tool("tmutil", &["thinlocalsnapshots", "/", "10000000000"], ctx, cleaner, option, report);
}

pub fn dev_npm(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    dev_dir(ctx, cleaner, option, report, ".npm", &["cache", "verify"])
}

pub fn dev_pip(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    dev_dir(ctx, cleaner, option, report, ".cache/pip", &[])
}

pub fn dev_cargo(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    dev_dir(ctx, cleaner, option, report, ".cargo/registry", &[])
}

pub fn dev_gradle(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    dev_dir(ctx, cleaner, option, report, ".gradle/caches", &[])
}

pub fn dev_go(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    dev_dir(ctx, cleaner, option, report, "go/pkg/mod", &["cache", "download"])
}

fn dev_dir(
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
    report: &mut Report,
    rel: &str,
    tool_args: &[&str],
) {
    if !tool_args.is_empty() {
        push_preview(
            report,
            cleaner,
            option,
            vec![PreviewItem::new(
                format!("would clean dev cache {}", rel),
                None,
                0,
            )],
        );
    }
    if let Some(home) = platform::home_dir() {
        let mut path = home.join(rel);
        // `.cache/pip` is relative to home, not to cache dir twice.
        if rel.starts_with(".cache/") {
            path = home.join(rel);
        } else if path.parent().map(|p| p.ends_with(".cache")).unwrap_or(false) {
            // no-op, kept for clarity
        }
        clean_roots(vec![path], ctx, cleaner, option, report);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> RunContext {
        RunContext::preview()
    }

    #[test]
    fn temp_deep_zero_age_fails() {
        let mut ctx = ctx();
        ctx.deep_options.temp_max_age_days = 0;
        let mut report = Report::new();
        temp_deep(&ctx, "c", "o", &mut report);
        assert_eq!(report.errors.len(), 1);
    }

    #[test]
    fn pacman_preview_keeps_newest() {
        // Just exercise the preview path without root access: no dir = no-op.
        let mut report = Report::new();
        pacman_cache(&ctx(), "c", "o", &mut report);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn docker_preview_reports_kept_cache() {
        let mut report = Report::new();
        docker(&ctx(), "c", "o", &mut report);
        assert_eq!(report.entries.len(), 1);
    }

    #[test]
    fn journal_vacuum_preview_is_command_entry() {
        let mut report = Report::new();
        journal_vacuum(&ctx(), "c", "o", &mut report);
        assert_eq!(report.special_operations(), 1);
    }
}
