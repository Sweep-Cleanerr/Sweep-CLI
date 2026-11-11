//! System-level actions: machine-wide operations, package managers,
//! processes, registry and a few deep-engine stubs.
//!
//! Dry-run never mutates the machine: machine-wide actions get a
//! "would run" entry and file work only reports what it would remove.

use std::path::PathBuf;

use crate::action::context::RunContext;
use crate::action::filter::ActionFilter;
use crate::action::safety;
use crate::core::error::Result;
use crate::core::report::{Entry, EntryKind, Report};
use crate::definition::model::{expand_action_paths, ActionDef, VarSet};
use crate::deep;
use crate::fsutil::{self, walk::ScanOptions};
use crate::platform;

pub fn run(
    def: &ActionDef,
    vars: &VarSet,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
) -> Report {
    let mut report = Report::new();

    match def.command.as_str() {
        "system.clipboard" => machine_wide(
            ctx.dry_run,
            "Clear clipboard",
            cleaner,
            option,
            &mut report,
            platform::clear_clipboard,
        ),
        "system.dns" => machine_wide(
            ctx.dry_run,
            "Flush DNS cache",
            cleaner,
            option,
            &mut report,
            platform::flush_dns,
        ),
        "system.memory" => machine_wide(
            ctx.dry_run,
            "Wipe freed memory",
            cleaner,
            option,
            &mut report,
            platform::wipe_memory,
        ),
        "system.trash" => clean_trash(ctx, cleaner, option, &mut report),
        "system.rotated_logs" => clean_simple_list(
            platform::rotated_logs(),
            ctx,
            cleaner,
            option,
            "rotated log",
            &mut report,
        ),
        "system.localizations" => clean_simple_list(
            platform::localization_paths(&ctx.keep_localizations),
            ctx,
            cleaner,
            option,
            "unused locale",
            &mut report,
        ),
        "system.recent_documents" => clean_simple_list(
            recent_documents(),
            ctx,
            cleaner,
            option,
            "recent document",
            &mut report,
        ),
        "system.custom" => clean_custom(ctx, cleaner, option, &mut report),
        "system.tmp" => clean_tmp(def, ctx, cleaner, option, &mut report),
        "system.free_disk_space" => {
            clean_free_space(def, vars, ctx, cleaner, option, &mut report)
        }
        "system.trim" => clean_trim(def, vars, ctx, cleaner, option, &mut report),
        "system.journald" => clean_journald(ctx, cleaner, option, &mut report),
        "system.font_cache" => clean_font_cache(ctx, cleaner, option, &mut report),
        "system.recycle_bin" => clean_recycle_bin(ctx, cleaner, option, &mut report),
        "system.prefetch" => clean_from_dirs(prefetch_dirs(), ctx, cleaner, option, &mut report),
        "system.wer" => clean_from_dirs(wer_dirs(), ctx, cleaner, option, &mut report),
        "system.windows_update_cache" => {
            clean_from_dirs(wu_dirs(), ctx, cleaner, option, &mut report)
        }
        "system.delivery_optimization" => {
            clean_from_dirs(do_dirs(), ctx, cleaner, option, &mut report)
        }
        "system.explorer_caches" => {
            clean_from_dirs(explorer_caches(), ctx, cleaner, option, &mut report)
        }
        "system.thumbcache" => clean_simple_list(
            thumbcache_files(),
            ctx,
            cleaner,
            option,
            "thumbnail cache",
            &mut report,
        ),
        "system.store_cache" => {
            clean_from_dirs(store_cache_dirs(), ctx, cleaner, option, &mut report)
        }
        "system.d3d_shader_cache" => {
            clean_from_dirs(d3d_cache_dirs(), ctx, cleaner, option, &mut report)
        }
        "system.crash_dumps" => {
            clean_from_dirs(crash_dump_dirs(), ctx, cleaner, option, &mut report)
        }
        "system.memory_dumps" => clean_simple_list(
            memory_dumps(),
            ctx,
            cleaner,
            option,
            "memory dump",
            &mut report,
        ),
        "process" => run_process(def, ctx, cleaner, option, &mut report),
        "winreg" => run_winreg(def, vars, ctx, cleaner, option, &mut report),
        "apt.clean" => run_pkg("apt-get", &["clean"], ctx, cleaner, option, &mut report),
        "apt.autoclean" => run_pkg("apt-get", &["autoclean", "-y"], ctx, cleaner, option, &mut report),
        "apt.autoremove" => run_pkg("apt-get", &["autoremove", "-y"], ctx, cleaner, option, &mut report),
        "dnf.clean" => run_pkg("dnf", &["clean", "all"], ctx, cleaner, option, &mut report),
        "yum.clean" => run_pkg("yum", &["clean", "all"], ctx, cleaner, option, &mut report),
        "pacman.clean" => run_pkg("pacman", &["-Scc", "--noconfirm"], ctx, cleaner, option, &mut report),
        "zypper.clean" => run_pkg("zypper", &["clean", "--all"], ctx, cleaner, option, &mut report),
        "brew.cleanup" => run_pkg("brew", &["cleanup", "--prune=all"], ctx, cleaner, option, &mut report),
        // Deep-engine commands.
        "system.apt_cache" => deep::cleaners::apt_cache(ctx, cleaner, option, &mut report),
        "system.apt_lists" => deep::cleaners::apt_lists(ctx, cleaner, option, &mut report),
        "system.dnf_cache" => deep::cleaners::dnf_cache(ctx, cleaner, option, &mut report),
        "system.zypp_cache" => deep::cleaners::zypp_cache(ctx, cleaner, option, &mut report),
        "system.pacman_cache" => deep::cleaners::pacman_cache(ctx, cleaner, option, &mut report),
        "system.old_kernels" => deep::cleaners::old_kernels(ctx, cleaner, option, &mut report),
        "system.journal_vacuum" => deep::cleaners::journal_vacuum(ctx, cleaner, option, &mut report),
        "system.journal_user" => deep::cleaners::journal_user(ctx, cleaner, option, &mut report),
        "system.user_caches" => deep::cleaners::user_caches(ctx, cleaner, option, &mut report),
        "system.temp_deep" => deep::cleaners::temp_deep(ctx, cleaner, option, &mut report),
        "system.orphans" => deep::cleaners::orphans(ctx, cleaner, option, &mut report),
        "system.snap" => deep::cleaners::snap(ctx, cleaner, option, &mut report),
        "system.flatpak" => deep::cleaners::flatpak(ctx, cleaner, option, &mut report),
        "system.docker" => deep::cleaners::docker(ctx, cleaner, option, &mut report),
        "system.docker_volumes" => deep::cleaners::docker_volumes(ctx, cleaner, option, &mut report),
        "system.kde_cache" => deep::cleaners::kde_cache(ctx, cleaner, option, &mut report),
        "system.kde_pim" => deep::cleaners::kde_pim(ctx, cleaner, option, &mut report),
        "system.baloo" => deep::cleaners::baloo(ctx, cleaner, option, &mut report),
        "system.gnome_cache" => deep::cleaners::gnome_cache(ctx, cleaner, option, &mut report),
        "system.zeitgeist" => deep::cleaners::zeitgeist(ctx, cleaner, option, &mut report),
        "system.tracker" => deep::cleaners::tracker(ctx, cleaner, option, &mut report),
        "system.crash_reports" => deep::cleaners::crash_reports(ctx, cleaner, option, &mut report),
        "system.coredumps" => deep::cleaners::coredumps(ctx, cleaner, option, &mut report),
        "system.nix_gc" => deep::cleaners::nix_gc(ctx, cleaner, option, &mut report),
        "system.tm_thin" => deep::cleaners::tm_thin(ctx, cleaner, option, &mut report),
        "system.dev_npm" => deep::cleaners::dev_npm(ctx, cleaner, option, &mut report),
        "system.dev_pip" => deep::cleaners::dev_pip(ctx, cleaner, option, &mut report),
        "system.dev_cargo" => deep::cleaners::dev_cargo(ctx, cleaner, option, &mut report),
        "system.dev_gradle" => deep::cleaners::dev_gradle(ctx, cleaner, option, &mut report),
        "system.dev_go" => deep::cleaners::dev_go(ctx, cleaner, option, &mut report),
        other => report.fail(cleaner, option, format!("unhandled system command '{other}'")),
    }

    report
}

fn special(cleaner: &str, option: &str, label: &str) -> Entry {
    Entry::new(EntryKind::Command, cleaner, option, label, None, 0)
}

fn machine_wide(
    dry_run: bool,
    label: &str,
    cleaner: &str,
    option: &str,
    report: &mut Report,
    action: impl FnOnce() -> Result<()>,
) {
    if dry_run {
        report.push(special(cleaner, option, label));
        return;
    }
    match action() {
        Ok(()) => report.push(special(cleaner, option, label)),
        Err(err) => report.fail(cleaner, option, err.to_string()),
    }
}

fn clean_trash(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    #[cfg(windows)]
    if !ctx.dry_run {
        let _ = platform::windows::empty_recycle_bin();
    }
    for dir in platform::trash_dirs() {
        delete_tree(&dir, ctx, cleaner, option, report);
    }
    if !ctx.dry_run {
        platform::shell_refresh();
    }
}

fn clean_simple_list(
    paths: Vec<PathBuf>,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
    label: &str,
    report: &mut Report,
) {
    for path in paths {
        if path.is_dir() {
            delete_tree(&path, ctx, cleaner, option, report);
        } else if path.exists() {
            remove_one(&path, ctx, cleaner, option, label, report);
        }
    }
}

fn clean_from_dirs(
    dirs: Vec<PathBuf>,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
    report: &mut Report,
) {
    for dir in dirs {
        delete_tree(&dir, ctx, cleaner, option, report);
    }
}

fn clean_custom(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    if ctx.custom_paths.is_empty() {
        report.push(Entry::new(
            EntryKind::Command,
            cleaner,
            option,
            "No custom paths configured",
            None,
            0,
        ));
        return;
    }
    for path in &ctx.custom_paths {
        if path.is_dir() {
            delete_tree(path, ctx, cleaner, option, report);
        } else if path.exists() {
            remove_one(path, ctx, cleaner, option, "custom path", report);
        }
    }
}

fn clean_tmp(def: &ActionDef, ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    let tmp = std::env::temp_dir();
    if !tmp.is_dir() {
        return;
    }
    let mut def = def.clone();
    if def.max_age_days.is_none() {
        def.max_age_days = Some(1);
    }
    let filter = match ActionFilter::from_action(&def) {
        Ok(f) => f,
        Err(err) => {
            report.fail(cleaner, option, err.to_string());
            return;
        }
    };
    let options = ScanOptions::files().with_max_depth(2);
    for path in fsutil::scan_paths(&tmp, &options) {
        if ctx.cancelled() {
            report.aborted = true;
            return;
        }
        if !filter.accepts(&path) {
            continue;
        }
        // Never delete Sweep's own wipe/temp leftovers from scans.
        if path.file_name().map(|n| n.to_string_lossy().contains("sweep")).unwrap_or(false) {
            continue;
        }
        remove_one(&path, ctx, cleaner, option, "temp file", report);
    }
}

fn clean_free_space(
    def: &ActionDef,
    vars: &VarSet,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
    report: &mut Report,
) {
    let roots = expand_action_paths(def, vars);
    let Some(root) = roots.first().filter(|r| !r.as_os_str().is_empty()) else {
        report.fail(cleaner, option, "free_disk_space needs a path");
        return;
    };
    if ctx.dry_run {
        report.push(Entry::new(
            EntryKind::WipeFreeSpace,
            cleaner,
            option,
            format!("would wipe free space on {}", root.display()),
            Some(root.as_path()),
            0,
        ));
        return;
    }
    match fsutil::wipe_free_space(root, None, &ctx.cancel) {
        Ok(wiped) => report.push(Entry::new(
            EntryKind::WipeFreeSpace,
            cleaner,
            option,
            format!("wiped free space on {}", root.display()),
            Some(root.as_path()),
            wiped,
        )),
        Err(err) => report.fail(cleaner, option, err.to_string()),
    }
}

fn clean_trim(
    def: &ActionDef,
    vars: &VarSet,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
    report: &mut Report,
) {
    let roots = expand_action_paths(def, vars);
    let Some(root) = roots.first().filter(|r| !r.as_os_str().is_empty()) else {
        return;
    };
    if ctx.dry_run {
        report.push(Entry::new(
            EntryKind::Command,
            cleaner,
            option,
            format!("would TRIM {}", root.display()),
            Some(root.as_path()),
            0,
        ));
        return;
    }
    match platform::trim(root) {
        Ok(()) => report.push(special(cleaner, option, "TRIM volume")),
        Err(err) => report.fail(cleaner, option, err.to_string()),
    }
}

fn clean_journald(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    if ctx.dry_run {
        report.push(special(cleaner, option, "Vacuum systemd journal"));
        return;
    }
    match platform::journald_clean(ctx.deep_options.journal_days) {
        Ok(freed) => report.push(Entry::new(
            EntryKind::Vacuum,
            cleaner,
            option,
            "Vacuum systemd journal",
            None,
            freed,
        )),
        Err(err) => report.fail(cleaner, option, err.to_string()),
    }
}

fn clean_font_cache(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    for dir in font_cache_dirs() {
        delete_tree(&dir, ctx, cleaner, option, report);
    }
    #[cfg(unix)]
    if !ctx.dry_run {
        let _ = fsutil::run_command("fc-cache", &["-f"], true);
    }
    report.push(special(cleaner, option, "Rebuild font cache"));
}

fn clean_recycle_bin(ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    #[cfg(windows)]
    {
        machine_wide(
            ctx.dry_run,
            "Empty recycle bin",
            cleaner,
            option,
            report,
            platform::windows::empty_recycle_bin,
        );
    }
    #[cfg(not(windows))]
    {
        let _ = ctx;
        report.fail(cleaner, option, "recycle bin is Windows-only");
    }
}

fn run_process(def: &ActionDef, ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    let Some(cmd) = def.cmd.clone() else {
        report.fail(cleaner, option, "process action is missing a 'cmd'");
        return;
    };
    run_shell(&cmd, def.wait, ctx, cleaner, option, report);
}

fn run_pkg(program: &str, args: &[&str], ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    let cmd = format!("{} {}", program, args.join(" "));
    run_shell(&cmd, true, ctx, cleaner, option, report);
}

fn run_shell(cmd: &str, wait: bool, ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    // Split handling quotes simply: keep it predictable for the common cases.
    let parts: Vec<String> = shlex_split(cmd);
    if parts.is_empty() {
        report.fail(cleaner, option, "process action has an empty command");
        return;
    }
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
    let (program, rest) = parts.split_first().unwrap();
    let args: Vec<&str> = rest.iter().map(|s| s.as_str()).collect();
    match fsutil::run_command(program, &args, wait) {
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

fn shlex_split(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for c in cmd.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
        } else if c == '\\' && !in_single {
            escaped = true;
        } else if c == '\'' && !in_double {
            in_single = !in_single;
        } else if c == '"' && !in_single {
            in_double = !in_double;
        } else if c.is_whitespace() && !in_single && !in_double {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn run_winreg(def: &ActionDef, vars: &VarSet, ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    let roots = expand_action_paths(def, vars);
    let Some(key) = roots.first() else {
        report.fail(cleaner, option, "winreg action needs a key path");
        return;
    };
    let really = !ctx.dry_run;
    if let Some(name) = &def.reg_name {
        match platform::winreg::reg_delete_value(&key.to_string_lossy(), name, really) {
            Ok(true) => report.push(Entry::new(
                EntryKind::Registry,
                cleaner,
                option,
                format!("delete {}\\{name}", key.display()),
                None,
                0,
            )),
            Ok(false) => {}
            Err(err) => report.fail(cleaner, option, err.to_string()),
        }
    } else {
        match platform::winreg::reg_delete_key(&key.to_string_lossy(), &def.exclude_keys, really) {
            Ok(true) => report.push(Entry::new(
                EntryKind::Registry,
                cleaner,
                option,
                format!("delete key {}", key.display()),
                None,
                0,
            )),
            Ok(false) => {}
            Err(err) => report.fail(cleaner, option, err.to_string()),
        }
    }
}

// --- helpers ----------------------------------------------------------------

fn remove_one(
    path: &std::path::Path,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
    label: &str,
    report: &mut Report,
) {
    if ctx.guard.check(path, false).is_err() {
        report.push(Entry::skipped(cleaner, option, path));
        return;
    }
    match crate::core::gitguard::check(path, ctx.git_guard) {
        crate::core::gitguard::Verdict::Allow => {}
        crate::core::gitguard::Verdict::Warn(msg) => {
            report.warn(cleaner, option, format!("{}: {msg}", path.display()))
        }
        crate::core::gitguard::Verdict::Block(msg) => {
            report.block(cleaner, option, format!("{}: {msg}", path.display()));
            return;
        }
    }
    if !ctx.dry_run && !crate::core::keep::is_link(path) {
        if let Err(reason) = safety::check_path(path, ctx) {
            log::warn!("skipped {}: {reason}", path.display());
            report.push(Entry::skipped(cleaner, option, path));
            return;
        }
    }
    let reclaimed = if path.is_dir() {
        fsutil::dir_size(path)
    } else {
        fsutil::size_of(path).unwrap_or(0)
    };
    if ctx.dry_run {
        report.push(Entry::new(
            EntryKind::Delete,
            cleaner,
            option,
            format!("{label} {}", path.display()),
            Some(path),
            reclaimed,
        ));
        return;
    }
    match fsutil::delete(path, fsutil::DeleteOptions::simple()) {
        Ok(true) => report.push(Entry::new(
            EntryKind::Delete,
            cleaner,
            option,
            format!("{label} {}", path.display()),
            Some(path),
            reclaimed,
        )),
        Ok(false) => {}
        Err(err) if safety::path_vanished(&err) => {}
        Err(err) => report.fail(cleaner, option, err.to_string()),
    }
}

fn delete_tree(root: &std::path::Path, ctx: &RunContext, cleaner: &str, option: &str, report: &mut Report) {
    if !root.is_dir() {
        return;
    }
    let options = ScanOptions::all().with_max_depth(32).with_same_filesystem(true);
    for path in fsutil::scan_paths(root, &options) {
        if ctx.cancelled() {
            report.aborted = true;
            return;
        }
        remove_one(&path, ctx, cleaner, option, "tree", report);
    }
}

fn recent_documents() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = platform::home_dir() {
        out.push(home.join(".recently-used.xbel"));
        out.push(home.join(".local").join("share").join("recently-used.xbel"));
    }
    #[cfg(windows)]
    if let Ok(appdata) = std::env::var("APPDATA") {
        out.push(PathBuf::from(appdata).join("Microsoft").join("Windows").join("Recent"));
    }
    out
}

fn font_cache_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(cache) = platform::cache_dir() {
        out.push(cache.join("fontconfig"));
    }
    if let Some(home) = platform::home_dir() {
        out.push(home.join(".fontconfig"));
        out.push(home.join("Library").join("Fonts"));
    }
    out
}

fn prefetch_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        platform::windows::prefetch_dir().into_iter().collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

fn wer_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        platform::windows::wer_dirs()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

fn wu_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        platform::windows::windows_update_cache().into_iter().collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

fn do_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        platform::windows::delivery_optimization_cache().into_iter().collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

fn explorer_caches() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        platform::windows::explorer_caches()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

fn thumbcache_files() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        platform::windows::explorer_thumb_files()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

fn store_cache_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        platform::windows::store_cache_dirs()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

fn d3d_cache_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        platform::windows::d3d_shader_cache_dirs()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

fn crash_dump_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        platform::windows::crash_dump_dirs()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

fn memory_dumps() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        platform::windows::memory_dumps()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::model::ActionDef;

    fn system_action(command: &str) -> ActionDef {
        ActionDef {
            command: command.to_string(),
            ..ActionDef::default()
        }
    }

    #[test]
    fn clipboard_and_dns_preview_do_not_run() {
        let report = run(&system_action("system.clipboard"), &VarSet::new(), &RunContext::preview(), "s", "o");
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.special_operations(), 1);
        assert!(report.errors.is_empty());

        let report = run(&system_action("system.dns"), &VarSet::new(), &RunContext::preview(), "s", "o");
        assert_eq!(report.special_operations(), 1);
    }

    #[test]
    fn process_preview_reports_would_run() {
        let mut def = system_action("process");
        def.cmd = Some("echo hi".to_string());
        let report = run(&def, &VarSet::new(), &RunContext::preview(), "s", "o");
        assert_eq!(report.entries.len(), 1);
        assert!(report.entries[0].label.contains("would run"));
    }

    #[test]
    fn custom_with_no_paths_reports_the_skip() {
        let report = run(&system_action("system.custom"), &VarSet::new(), &RunContext::preview(), "s", "o");
        assert_eq!(report.entries.len(), 1);
        assert!(report.entries[0].label.contains("No custom paths"));
    }

    #[test]
    fn free_space_needs_a_path_and_dry_run_reports() {
        let report = run(&system_action("system.free_disk_space"), &VarSet::new(), &RunContext::preview(), "s", "o");
        assert_eq!(report.errors.len(), 1);

        let dir = crate::fixture("sys_free");
        let mut def = system_action("system.free_disk_space");
        def.path = dir.to_string_lossy().to_string();
        let report = run(&def, &VarSet::new(), &RunContext::preview(), "s", "o");
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].kind, EntryKind::WipeFreeSpace);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_system_command_fails() {
        let report = run(&system_action("system.nope"), &VarSet::new(), &RunContext::preview(), "s", "o");
        assert_eq!(report.errors.len(), 1);
    }

    #[test]
    fn deep_commands_run_instead_of_failing() {
        // apt_cache now runs through the deep engine; on a host without
        // the cache dir it is a clean no-op but must not fail.
        let report = run(&system_action("system.apt_cache"), &VarSet::new(), &RunContext::preview(), "s", "o");
        assert!(report.errors.is_empty(), "deep command failed: {:?}", report.errors);
    }

    #[test]
    fn shlex_split_keeps_quoted_args() {
        assert_eq!(shlex_split("echo hi"), vec!["echo", "hi"]);
        assert_eq!(shlex_split(r#"echo "a b" ok"#), vec!["echo", "a b", "ok"]);
    }
}
