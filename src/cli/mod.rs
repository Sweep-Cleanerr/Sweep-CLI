//! Command-line interface.
//!
//! Stage 6 restores the original Sweep command surface on top of the new
//! modules. Core engine commands (`list`, `info`, `preview`, `clean`,
//! `deepscan`, `bigfiles`, `du`, `dupes`, `analyze`, `devscan`,
//! `leftovers`, `sysinfo`, `doctor`, `providers`, `diagnostics`,
//! `import`, `config`, `lang`, `completions`, `man`, `watch`,
//! `diagnose`, `purge`, `backup`, `undo`, `shred`, `history`) are
//! fully wired, as are the maintenance commands in [`maintenance`].
//! Only `interactive` is intentionally a stub (no TUI by design).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

mod maintenance;

use clap::{CommandFactory, Parser, Subcommand};
use serde_json::json;

use crate::action::provider;
use crate::action::RunContext;
use crate::fsutil;
use crate::config::{default_path, Config};
use crate::core::gitguard::Protection;
use crate::core::keep::Guard;
use crate::core::report::{Entry, EntryKind, Report, Stopwatch};
use crate::core::{Error, Result};
use crate::definition::{CleanerRegistry, LoadOptions, Trust};
use crate::engine::worker::{resolve, run};
use crate::fsutil::{
    bytes_to_human, delete, dir_size, human_to_bytes, scan_paths, size_of, DeleteOptions,
    ScanOptions,
};
use crate::platform;
use sha2::Digest;

#[derive(Debug, Parser)]
#[command(name = "sweep", version, about = "Cross-platform system cleaner")]
struct Cli {
    /// Configuration file
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Overwrite files instead of deleting
    #[arg(long, global = true)]
    shred: bool,
    /// Override the running-cleaner check
    #[arg(long, global = true)]
    force: bool,
    /// Trust cleaners loaded from extra directories
    #[arg(long, global = true)]
    trust_external: bool,
    /// Load BleachBit definitions
    #[arg(long, global = true)]
    bleachbit: bool,
    /// Load winapp2.ini definitions
    #[arg(long, global = true)]
    winapp2: bool,
    /// Extra cleaner definition directory (repeatable)
    #[arg(long, global = true, value_name = "DIR")]
    define: Vec<PathBuf>,
    /// Emit one JSON document on stdout
    #[arg(long, global = true)]
    json: bool,
    /// Emit the compact JSON summary instead of the full report
    #[arg(long, global = true)]
    summary: bool,
    /// Suppress human-readable output
    #[arg(long, global = true)]
    quiet: bool,
    /// Emit machine-readable progress events
    #[arg(long, global = true)]
    progress_json: bool,
    /// Verbose debug output
    #[arg(long, global = true)]
    debug: bool,
    /// UI language
    #[arg(long, global = true, value_name = "CODE")]
    lang: Option<String>,
    /// Confirm destructive operations
    #[arg(long, global = true)]
    yes: bool,
    /// Directory for backup files
    #[arg(long, global = true, value_name = "DIR")]
    backup_dir: Option<PathBuf>,
    /// Maximum total backup size
    #[arg(long, global = true, value_name = "SIZE")]
    backup_max_size: Option<String>,
    /// Directory for the trash bin
    #[arg(long, global = true, value_name = "DIR")]
    trash_dir: Option<PathBuf>,
    /// Use the OS trash instead of the internal one
    #[arg(long, global = true)]
    os_trash: bool,
    /// Path to the local database
    #[arg(long, global = true, value_name = "PATH")]
    db_path: Option<PathBuf>,
    /// Check for updates
    #[arg(long, global = true)]
    check_update: bool,
    /// Git guard mode (off, warn, block, paranoid)
    #[arg(long, global = true, value_name = "MODE")]
    git_guard: Option<String>,
    /// Operation log file
    #[arg(long, global = true, value_name = "PATH")]
    log_file: Option<PathBuf>,
    /// Portable mode: keep everything next to the executable
    #[arg(long, global = true)]
    portable: bool,
    /// Write an HTML report
    #[arg(long, global = true, value_name = "PATH")]
    html: Option<PathBuf>,
    /// Write a Markdown report
    #[arg(long, global = true, value_name = "PATH")]
    markdown: Option<PathBuf>,
    /// Elevated-child output file (hidden helper)
    #[arg(long, hide = true, value_name = "PATH")]
    elevated_out: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Interactive shell (not implemented)
    #[command(alias = "ui", alias = "shell")]
    Interactive { #[arg(long)] plain: bool },
    /// List available cleaners
    #[command(alias = "ls")]
    List {
        cleaner: Option<String>,
        #[arg(long)]
        search: Option<String>,
        #[arg(long)]
        os: Option<String>,
        #[arg(long)]
        running: bool,
    },
    /// Show details for one cleaner
    Info { cleaner: String },
    /// Dry-run a selection
    #[command(alias = "p")]
    Preview {
        selectors: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long, value_name = "EXCLUDE")]
        exclude: Vec<String>,
        #[arg(long)]
        last: bool,
        #[arg(long)]
        remember: bool,
    },
    /// Clean a selection (requires --yes)
    #[command(alias = "c")]
    Clean {
        selectors: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long, value_name = "EXCLUDE")]
        exclude: Vec<String>,
        #[arg(long)]
        last: bool,
        #[arg(long)]
        remember: bool,
        #[arg(long)]
        stop_services: bool,
        #[arg(long)]
        show_locks: bool,
        #[arg(long)]
        kill: bool,
    },
    /// Scan for stale files under roots
    Deepscan {
        roots: Vec<PathBuf>,
        #[arg(long)]
        pattern: Option<String>,
        #[arg(long)]
        preview: bool,
        #[arg(long)]
        stop_services: bool,
    },
    /// Find leftover files for a name
    Leftovers {
        name: String,
        #[arg(long)]
        clean: bool,
        #[arg(long, value_name = "SIZE")]
        min_size: Option<String>,
    },
    /// Scan for residue of uninstalled programs
    Residue {
        action: String,
        #[arg(long)]
        roots: Vec<PathBuf>,
        #[arg(long, value_name = "N")]
        confidence: Option<u32>,
        #[arg(long)]
        keep_empty_dirs: bool,
    },
    /// Scan development artifacts
    Devscan {
        roots: Vec<PathBuf>,
        #[arg(long)]
        clean: bool,
        #[arg(long, value_name = "DEPTH")]
        max_depth: Option<usize>,
        #[arg(long, value_name = "SIZE")]
        min_size: Option<String>,
    },
    /// List the largest files under roots
    Bigfiles {
        roots: Vec<PathBuf>,
        #[arg(long)]
        clean: bool,
        #[arg(long, value_name = "SIZE")]
        min_size: Option<String>,
        #[arg(long, value_name = "DAYS")]
        older_than: Option<u64>,
        #[arg(long, value_name = "N")]
        top: Option<usize>,
        #[arg(long, value_name = "DEPTH")]
        max_depth: Option<usize>,
    },
    /// Disk usage of a directory
    Du {
        path: Option<PathBuf>,
        #[arg(long, value_name = "N")]
        top: Option<usize>,
    },
    /// Find duplicate files
    Dupes {
        roots: Vec<PathBuf>,
        #[arg(long)]
        clean: bool,
        #[arg(long, value_name = "SIZE")]
        min_size: Option<String>,
        #[arg(long, value_name = "DEPTH")]
        max_depth: Option<usize>,
        #[arg(long, value_name = "N")]
        keep: Option<usize>,
        /// Replace victims with hardlinks/reflinks (requires --clean --yes)
        #[arg(long)]
        link: bool,
        #[arg(long)]
        show_locks: bool,
    },
    /// Wipe a path
    Wipe { path: PathBuf },
    /// Schedule cleanups
    Schedule {
        #[arg(long)]
        enable: bool,
        #[arg(long)]
        disable: bool,
        #[arg(long)]
        hour: Option<u32>,
        selection: Vec<String>,
        #[arg(long)]
        memopt: bool,
        #[arg(long)]
        guard_install: bool,
        #[arg(long)]
        guard_remove: bool,
        #[arg(long)]
        guard_status: bool,
    },
    /// Purge old trash entries
    Purge {
        #[arg(long, value_name = "DIR")]
        trash_dir: Option<PathBuf>,
        #[arg(long, value_name = "DAYS")]
        older_than_days: Option<u64>,
    },
    /// Manage backups
    #[command(alias = "backups")]
    Backup {
        action: String,
        #[arg(long, value_name = "DAYS")]
        older_than_days: Option<u64>,
    },
    /// Undo a previous run
    Undo {
        #[arg(long, value_name = "PATH")]
        log_file: Option<PathBuf>,
        #[arg(long, value_name = "DIR")]
        backup_dir: Option<PathBuf>,
        #[arg(long)]
        last: bool,
        #[arg(long, value_name = "RUN")]
        run: Option<String>,
        #[arg(long)]
        list: bool,
    },
    /// Update winapp2.ini
    UpdateWinapp2 { #[arg(long)] url: Option<String> },
    /// Cleaner hub operations
    Hub { op: String, target: String, #[arg(long)] url: Option<String> },
    /// Shred files
    Shred { paths: Vec<PathBuf> },
    /// Analyze disk usage
    Analyze {
        root: Option<PathBuf>,
        #[arg(long, value_name = "N")]
        top: Option<usize>,
        #[arg(long, value_name = "DEPTH")]
        max_depth: Option<usize>,
    },
    /// Manage startup entries
    Startup {
        op: String,
        target: String,
        #[arg(long)]
        impact: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        risk: Option<String>,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        disabled: bool,
        #[arg(long)]
        enabled: bool,
        #[arg(long)]
        search: Option<String>,
        #[arg(long)]
        sort: Option<String>,
        #[arg(long)]
        command: bool,
        #[arg(long)]
        last: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
        #[arg(long)]
        to: Option<String>,
        #[arg(long, value_name = "FMT")]
        format: Option<String>,
    },
    /// Manage scheduled tasks
    Tasks {
        op: String,
        target: String,
        #[arg(long)]
        enabled: bool,
        #[arg(long)]
        disabled: bool,
        #[arg(long)]
        trigger: Option<String>,
        #[arg(long)]
        search: Option<String>,
        #[arg(long)]
        sort: Option<String>,
        #[arg(long)]
        last: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
        #[arg(long)]
        to: Option<String>,
        #[arg(long, value_name = "FMT")]
        format: Option<String>,
    },
    /// Manage services
    Services { op: String, unit: String, #[arg(long)] user: bool },
    /// Health check
    Doctor,
    /// Reserve free space
    Reserve {
        #[arg(long)]
        status: bool,
        #[arg(long)]
        recreate: bool,
        #[arg(long, value_name = "MB")]
        size_mb: Option<u64>,
    },
    /// Optimize a volume
    Optimize { #[arg(long)] volume: Option<String>, #[arg(long)] analyze: bool },
    /// Background guard
    Guard {
        #[arg(long)]
        once: bool,
        #[arg(long, value_name = "PCT")]
        ram_threshold: Option<f64>,
        #[arg(long, value_name = "GB")]
        disk_min_gb: Option<f64>,
        #[arg(long, value_name = "SECS")]
        interval_secs: Option<u64>,
        #[arg(long)]
        allow_clean: bool,
        #[arg(long)]
        allow_kill: bool,
    },
    /// Kill a process
    Kill { #[arg(long)] pid: Option<u32>, #[arg(long)] name: Option<String>, #[arg(long)] close: bool },
    /// Find file locks
    Locks { path: PathBuf },
    /// Idle file finder
    Idle {
        #[arg(long, value_name = "N")]
        top: Option<usize>,
        #[arg(long, value_name = "MINS")]
        idle_mins: Option<u64>,
        #[arg(long, value_name = "MB")]
        min_write_mb: Option<u64>,
        #[arg(long)]
        watch: bool,
    },
    /// Installed applications
    Apps { #[arg(long, value_name = "DAYS")] since_days: Option<u64>, #[arg(long)] uninstall: Option<String> },
    /// Build/update the file index
    Index {
        #[arg(long)]
        status: bool,
        #[arg(long)]
        full: bool,
        roots: Vec<PathBuf>,
    },
    /// Diagnose system issues
    Diagnose { #[arg(long)] deep: bool },
    /// Memory optimization
    Memopt {
        #[arg(long)]
        aggressive: bool,
        #[arg(long)]
        cache: bool,
        #[arg(long)]
        clipboard: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// System information
    Sysinfo { #[arg(long)] health: bool },
    /// Network caches
    Network { op: String, #[arg(long)] dry_run: bool },
    /// Privacy cleanup
    Privacy { op: String, #[arg(long)] dry_run: bool },
    /// Snapshots
    Snapshot { op: String, target: String },
    /// Run history
    History { #[arg(long, value_name = "PATH")] log_file: Option<PathBuf>, #[arg(long, value_name = "DAYS")] days: Option<u64> },
    /// Watch paths for growth
    Watch {
        paths: Vec<PathBuf>,
        #[arg(long, value_name = "SIZE")]
        threshold: Option<String>,
        #[arg(long, value_name = "SECS")]
        interval: Option<u64>,
        #[arg(long)]
        once: bool,
        #[arg(long)]
        suggest: bool,
        #[arg(long, value_name = "MINS")]
        cooldown_mins: Option<u64>,
    },
    /// List action providers
    Providers,
    /// Diagnostics output
    Diagnostics,
    /// Import cleaner definitions from a directory
    Import { path: PathBuf },
    /// Show or change configuration
    Config { op: String, key: Option<String>, value: Option<String> },
    /// Language handling
    Lang { code: Option<String> },
    /// Print shell completions
    Completions { shell: String },
    /// Print the manual
    Man { #[arg(long, value_name = "PATH")] output: Option<PathBuf> },
}

pub fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match execute(cli) {
        Ok(code) => std::process::ExitCode::from(code as u8),
        Err(err) => {
            eprintln!("error: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn execute(cli: Cli) -> Result<i32> {
    let Cli {
        config: config_path,
        shred,
        force,
        trust_external,
        bleachbit,
        winapp2,
        define,
        json,
        summary,
        quiet,
        progress_json,
        debug: _,
        lang,
        yes,
        backup_dir,
        backup_max_size,
        trash_dir,
        os_trash,
        db_path: _,
        check_update: _,
        git_guard,
        log_file,
        portable,
        html,
        markdown,
        elevated_out: _,
        command,
    } = cli;

    // Silently ignoring this flag would promise a recycle bin and then
    // permanently delete. Say so instead.
    if os_trash {
        eprintln!("warning: --os-trash is not implemented; deletions are permanent (use --backup-dir or --trash-dir)");
    }

    // `--portable` keeps config/log files next to the executable; the config
    // loader and the operation log both read SWEEP_PORTABLE.
    if portable {
        if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
            std::env::set_var("SWEEP_PORTABLE", dir);
        }
    }

    let mut config = match &config_path {
        Some(path) => {
            if !path.exists() {
                let _ = std::io::stderr().write_all(format!("warning: config file '{path:?}' not found; using defaults\n").as_bytes());
            }
            Config::load(path)?
        }
        None => Config::load(&default_path())?,
    };
    if let Some(code) = lang {
        config.languages = vec![supported_lang(&code)?];
    }
    let protection = match &git_guard {
        Some(text) => Protection::parse(text).map_err(Error::msg)?,
        None => Protection::parse(&config.git_guard).unwrap_or(Protection::Warn),
    };

    let mut options = LoadOptions::builtin_only();
    options.extra_dirs = define;
    options.bleachbit = bleachbit;
    options.winapp2 = winapp2;
    options.external_trust = if trust_external { Trust::Trusted } else { Trust::Untrusted };
    let registry = CleanerRegistry::load(&options);

    if !quiet {
        for warning in registry.warnings() {
            eprintln!("warning: {warning}");
        }
    }

    match command {
        Command::Interactive { .. } => Err(Error::msg(
            "interactive is not implemented; use the one-shot CLI",
        )),
        Command::List { cleaner, search, os, running } => list(&registry, cleaner, search, os, running, json, quiet),
        Command::Info { cleaner } => info(&registry, &cleaner, json, quiet),
        Command::Preview { selectors, all, exclude, last: _, remember: _ } => {
            run_selection(&registry, &config, &selectors, all, &exclude, true, json, summary, quiet, progress_json, force, shred, backup_dir.clone(), backup_max_size.clone(), trash_dir.clone(), log_file.clone(), protection, html.clone(), markdown.clone())
        }
        Command::Clean { selectors, all, exclude, last: _, remember: _, stop_services: _, show_locks: _, kill: _ } => {
            if !yes {
                return Err(Error::msg("refusing to clean without --yes; run 'sweep preview' first"));
            }
            run_selection(&registry, &config, &selectors, all, &exclude, false, json, summary, quiet, progress_json, force, shred, backup_dir.clone(), backup_max_size.clone(), trash_dir.clone(), log_file.clone(), protection, html.clone(), markdown.clone())
        }
        Command::Deepscan { roots, pattern, preview, stop_services: _ } => {
            deepscan(&config, &roots, pattern, preview || !yes, yes, json, quiet)
        }
        Command::Leftovers { name, clean, min_size } => leftovers(&config, &name, clean, min_size, yes, json, quiet),
        Command::Residue { action, roots, confidence, keep_empty_dirs } => {
            maintenance::residue(&action, &roots, confidence, keep_empty_dirs, yes, json, quiet)
        }
        Command::Devscan { roots, clean, max_depth, min_size } => {
            devscan(&config, &roots, clean, max_depth, &min_size, yes, json, quiet)
        }
        Command::Bigfiles { roots, clean, min_size, older_than, top, max_depth } => {
            bigfiles(&config, &roots, clean, &min_size, older_than, top, max_depth, yes, json, quiet)
        }
        Command::Du { path, top } => du(&path, top, json, quiet),
        Command::Dupes { roots, clean, min_size, max_depth, keep, link, show_locks: _ } => {
            dupes(&config, &roots, clean, &min_size, max_depth, keep, link, yes, json, quiet)
        }
        Command::Wipe { path } => maintenance::wipe(&path, yes, json, quiet),
        Command::Schedule { enable, disable, hour: _, selection, memopt: _, guard_install: _, guard_remove: _, guard_status: _ } => {
            maintenance::schedule(&mut config, &config_path, enable, disable, &selection, json, quiet)
        }
        Command::Purge { trash_dir: purge_dir, older_than_days } => {
            let dir = purge_dir.or(trash_dir.clone()).unwrap_or_else(|| PathBuf::from("trash"));
            let report = crate::engine::trash::purge(&dir, older_than_days.unwrap_or(0), !yes);
            print_engine(&report, json, quiet)
        }
        Command::Backup { action, older_than_days } => {
            let dir = backup_dir.clone().unwrap_or_else(|| PathBuf::from("backup"));
            let report = match action.as_str() {
                "list" | "ls" => crate::engine::backup::list(&dir),
                "prune" => crate::engine::backup::prune(&dir, older_than_days.unwrap_or(0), !yes),
                _ => {
                    let mut r = Report::new();
                    r.fail("backup", "action", format!("unknown backup action '{action}' (list|prune)"));
                    r
                }
            };
            print_engine(&report, json, quiet)
        }
        Command::Undo { log_file: u_log, backup_dir: u_backup, last, run: u_run, list } => {
            let backup_root = u_backup.or(backup_dir.clone()).unwrap_or_else(|| PathBuf::from("backup"));
            let log_path = u_log.unwrap_or_else(crate::engine::logline::default_log_file);
            if list {
                let report = crate::engine::undo::list_runs(&log_path, &backup_root);
                print_engine(&report, json, quiet)
            } else {
                let scope = if last {
                    crate::engine::undo::Scope::Last
                } else if let Some(id) = u_run {
                    crate::engine::undo::Scope::Run(id)
                } else {
                    crate::engine::undo::Scope::All
                };
                let report = crate::engine::undo::scan(&log_path, &backup_root, &scope, !yes);
                print_engine(&report, json, quiet)
            }
        }
        Command::UpdateWinapp2 { url } => maintenance::update_winapp2(url, json, quiet),
        Command::Hub { op, target, url } => maintenance::hub(&op, &target, url, json, quiet),
        Command::Shred { paths } => shred_paths(&paths, yes, json, quiet),
        Command::Analyze { root, top, max_depth } => analyze(&root, top, max_depth, json, quiet),
        Command::Startup { op, target, .. } => maintenance::startup(&op, &target, json, quiet),
        Command::Tasks { op, target, .. } => maintenance::tasks(&op, &target, json, quiet),
        Command::Services { op, unit, user } => maintenance::services(&op, &unit, user, yes, json, quiet),
        Command::Doctor => doctor(&registry, &config, json, quiet),
        Command::Reserve { status, recreate, size_mb } => maintenance::reserve(&config, status, recreate, size_mb, json, quiet),
        Command::Optimize { volume, analyze } => maintenance::optimize(volume, analyze, json, quiet),
        Command::Guard { once, ram_threshold, disk_min_gb, interval_secs, allow_clean, allow_kill } => {
            maintenance::guard(once, ram_threshold, disk_min_gb, interval_secs, allow_clean, allow_kill, json, quiet)
        }
        Command::Kill { pid, name, close } => maintenance::kill(pid, name, close, yes, json, quiet),
        Command::Locks { path } => maintenance::locks(&path, json, quiet),
        Command::Idle { top, idle_mins, min_write_mb, watch: _ } => maintenance::idle(top, idle_mins, min_write_mb, json, quiet),
        Command::Apps { since_days, uninstall, .. } => maintenance::apps(since_days, uninstall, yes, json, quiet),
        Command::Index { status, full, roots } => maintenance::index(status, full, &roots, json, quiet),
        Command::Diagnose { deep } => diagnose(deep, json, quiet),
        Command::Memopt { aggressive, cache, clipboard, dry_run } => maintenance::memopt(aggressive, cache, clipboard, dry_run, json, quiet),
        Command::Sysinfo { health } => sysinfo(health, json, quiet),
        Command::Network { op, dry_run } => maintenance::network(&op, dry_run, json, quiet),
        Command::Privacy { op, dry_run } => maintenance::privacy(&op, dry_run, json, quiet),
        Command::Snapshot { op, target } => maintenance::snapshot(&op, &target, json, quiet),
        Command::History { log_file: h_log, days: _ } => history(h_log.or(log_file.clone()), json, quiet),
        Command::Watch { paths, threshold, interval: _, once: _, suggest: _, cooldown_mins: _ } => {
            watch(&paths, threshold, json, quiet)
        }
        Command::Providers => providers(json, quiet),
        Command::Diagnostics => diagnostics(&registry, &config, json, quiet),
        Command::Import { path } => import(&path, json, quiet),
        Command::Config { op, key, value } => config_cmd(&mut config, &op, key, value, &config_path, json, quiet),
        Command::Lang { code } => lang_cmd(&mut config, code, &config_path, json, quiet),
        Command::Completions { shell } => completions(&shell),
        Command::Man { output } => man(output, quiet),
    }
}

fn print_engine(report: &Report, json: bool, quiet: bool) -> Result<i32> {
    Ok(print_report(report, json, false, quiet))
}

fn shred_paths(paths: &[PathBuf], yes: bool, json: bool, quiet: bool) -> Result<i32> {
    use crate::shred::{Pattern, ShredSpec};
    let mut report = Report::new();
    let spec = ShredSpec::default_spec();
    for path in paths {
        if !yes {
            report.push(Entry::new(EntryKind::Shred, "shred", "cli", format!("would shred {}", path.display()), Some(path), 0));
            continue;
        }
        if !path.exists() {
            report.push(Entry::skipped("shred", "cli", path));
            continue;
        }
        let bytes = path.metadata().map(|m| m.len()).unwrap_or(0);
        match crate::shred::shred_file(path, spec) {
            Ok(()) => report.push(Entry::new(EntryKind::Shred, "shred", "cli", format!("shredded {}", path.display()), Some(path), bytes)),
            Err(err) => report.fail("shred", "cli", err.to_string()),
        }
    }
    let _ = Pattern::Zeros;
    print_engine(&report, json, quiet)
}

fn history(log_path: Option<PathBuf>, json: bool, quiet: bool) -> Result<i32> {
    let path = log_path.unwrap_or_else(crate::engine::logline::default_log_file);
    let lines = crate::engine::logline::parse_chain(&path);
    let mut deletions = lines.iter().filter(|l| l.mode.is_deletion());
    if json {
        let items: Vec<_> = deletions
            .map(|l| json!({"ts": l.ts, "mode": l.mode.as_str(), "cleaner": l.cleaner, "option": l.option, "path": l.path, "bytes": l.bytes}))
            .collect();
        print_json(&json!(items))?;
    } else if !quiet {
        let mut count = 0;
        let mut total = 0u64;
        for l in deletions.by_ref() {
            count += 1;
            total += l.bytes;
            println!("{} | {} | {}.{} | {} | {}B", l.ts, l.mode.as_str(), l.cleaner, l.option, l.path.display(), l.bytes);
        }
        println!("{count} deletions, {total} bytes");
    }
    Ok(0)
}

fn print_json(value: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value).map_err(|err| Error::msg(err.to_string()))?);
    Ok(())
}

fn list(
    registry: &CleanerRegistry,
    cleaner: Option<String>,
    search: Option<String>,
    os: Option<String>,
    running: bool,
    json: bool,
    quiet: bool,
) -> Result<i32> {
    let mut shown = Vec::new();
    for loaded in registry.all() {
        let def = &loaded.def;
        if let Some(ref query) = cleaner {
            if !def.id.to_lowercase().contains(&query.to_lowercase())
                && !def.name.to_lowercase().contains(&query.to_lowercase())
            {
                continue;
            }
        }
        if let Some(ref text) = search {
            let needle = text.to_lowercase();
            let haystack = format!("{} {} {}", def.id, def.name, def.description).to_lowercase();
            if !haystack.contains(&needle) {
                continue;
            }
        }
        if let Some(ref target) = os {
            if !target_os_matches(def.os.as_str().unwrap_or(""), target) {
                continue;
            }
        }
        if running && !registry.is_running(&def.id) {
            continue;
        }
        shown.push(loaded);
    }

    if json {
        let value = json!(shown.iter().map(|loaded| {
            let def = &loaded.def;
            json!({
                "id": def.id,
                "name": def.name,
                "description": def.description,
                "os": def.os.as_str().unwrap_or(""),
                "options": def.options.iter().map(|o| json!({
                    "id": o.id, "label": o.label, "description": o.description,
                })).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>());
        print_json(&value)?;
    } else if !quiet {
        for loaded in &shown {
            let def = &loaded.def;
            let os = def.os.as_str().unwrap_or("any");
            println!("{}  {}  [{}]", def.id, def.name, os);
            for option in &def.options {
                println!("  {}  {}", option.id, option.label);
            }
        }
        if shown.is_empty() {
            println!("no cleaners matched");
        }
    }
    Ok(0)
}

fn info(registry: &CleanerRegistry, cleaner: &str, json: bool, quiet: bool) -> Result<i32> {
    let def = registry.def(cleaner).ok_or_else(|| Error::UnknownCleaner(cleaner.to_string()))?;
    if json {
        print_json(&json!({
            "id": def.id,
            "name": def.name,
            "description": def.description,
            "os": def.os.as_str().unwrap_or(""),
            "options": def.options.iter().map(|o| json!({
                "id": o.id,
                "label": o.label,
                "description": o.description,
                "actions": o.actions.len(),
            })).collect::<Vec<_>>(),
        }))?;
    } else if !quiet {
        println!("{} — {}", def.id, def.name);
        println!("{}", def.description);
        for option in &def.options {
            println!("  {}  {}", option.id, option.label);
            if !option.description.is_empty() {
                println!("      {}", option.description);
            }
        }
    }
    Ok(0)
}

fn providers(json: bool, quiet: bool) -> Result<i32> {
    if json {
        print_json(&json!(provider::list().iter().map(|p| json!({
            "name": p.name,
            "summary": p.summary,
            "needs_path": p.needs_path,
            "destructive": p.destructive,
            "privileged": p.privileged,
            "available": p.available(),
        })).collect::<Vec<_>>()))?;
    } else if !quiet {
        for p in provider::list() {
            println!("{}  {}", p.name, p.summary);
        }
    }
    Ok(0)
}

fn diagnostics(registry: &CleanerRegistry, config: &Config, json: bool, quiet: bool) -> Result<i32> {
    let config_path = default_path();
    let value = json!({
        "cleaners": registry.len(),
        "providers": provider::list().len(),
        "config": config_path.display().to_string(),
        "config_exists": config_path.exists(),
        "os": platform::os_name(),
        "warnings": registry.warnings(),
    });
    if json {
        print_json(&value)?;
    } else if !quiet {
        println!("cleaners: {}", registry.len());
        println!("providers: {}", provider::list().len());
        println!("config: {} ({} )", config_path.display(), if config_path.exists() { "present" } else { "missing" });
        println!("os: {}", platform::os_name());
        for warning in registry.warnings() {
            println!("warning: {warning}");
        }
    }
    let _ = config;
    Ok(0)
}

fn doctor(registry: &CleanerRegistry, _config: &Config, json: bool, quiet: bool) -> Result<i32> {
    diagnostics(registry, _config, json, quiet)
}

fn sysinfo(health: bool, json: bool, quiet: bool) -> Result<i32> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let free = platform::free_space(&cwd).unwrap_or(0);
    let total = platform::total_space(&cwd).unwrap_or(0);
    let used_pct = if total > 0 { (total - free) as f64 * 100.0 / total as f64 } else { 0.0 };
    if json {
        print_json(&json!({
            "os": platform::os_name(),
            "free_bytes": free,
            "total_bytes": total,
            "used_percent": format!("{used_pct:.1}"),
            "health": health,
        }))?;
    } else if !quiet {
        println!("os: {}", platform::os_name());
        println!("free: {}", bytes_to_human(free, false));
        println!("total: {}", bytes_to_human(total, false));
        if health {
            println!("health: {}% used", format!("{used_pct:.1}"));
        }
    }
    Ok(0)
}

fn diagnose(_deep: bool, json: bool, quiet: bool) -> Result<i32> {
    let mut findings = Vec::new();
    let config_path = default_path();
    if !config_path.exists() {
        findings.push("config file not found; defaults will be used".to_string());
    }
    if let Some(parent) = config_path.parent() {
        if !parent.exists() {
            findings.push(format!("config directory missing: {}", parent.display()));
        }
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if platform::free_space(&cwd).unwrap_or(u64::MAX) == 0 {
        findings.push("no free space reported".to_string());
    }
    if json {
        print_json(&json!({"findings": findings}))?;
    } else if !quiet {
        if findings.is_empty() {
            println!("no issues found");
        } else {
            for f in &findings {
                println!("{f}");
            }
        }
    }
    Ok(0)
}

fn run_selection(
    registry: &CleanerRegistry,
    config: &Config,
    selectors: &[String],
    all: bool,
    exclude: &[String],
    dry_run: bool,
    json: bool,
    summary: bool,
    quiet: bool,
    progress_json: bool,
    force: bool,
    shred: bool,
    backup_dir: Option<PathBuf>,
    backup_max_size: Option<String>,
    trash_dir: Option<PathBuf>,
    log_file: Option<PathBuf>,
    git_guard: Protection,
    html: Option<PathBuf>,
    markdown: Option<PathBuf>,
) -> Result<i32> {
    let selected = resolve(registry, selectors, all, exclude)?;
    let mut ctx = if dry_run { RunContext::preview() } else { RunContext::clean() };
    ctx.guard = Guard::with_builtins(config.keep_list());
    ctx.global_shred = shred;
    ctx.custom_paths = config.custom_paths();
    ctx.keep_localizations = config.languages.clone();
    ctx.deep_options.journal_size = config.journal_size.clone();
    ctx.deep_options.journal_days = config.journal_days as u32;
    ctx.deep_options.pacman_keep = config.pacman_keep as u32;
    ctx.deep_options.temp_max_age_days = config.temp_max_age_days as u32;
    ctx.deep_options.docker_build_cache = config.docker_build_cache;
    ctx.backup_dir = backup_dir;
    if let Some(size) = backup_max_size {
        ctx.backup_max_bytes = fsutil::human_to_bytes(&size).unwrap_or(0);
    }
    // Config backup_dir / trash_dir fallbacks.
    if ctx.backup_dir.is_none() && !config.backup_dir.is_empty() {
        ctx.backup_dir = Some(PathBuf::from(&config.backup_dir));
    }
    if ctx.trash_dir.is_none() && !config.trash_dir.is_empty() {
        ctx.trash_dir = Some(PathBuf::from(&config.trash_dir));
    }
    ctx.trash_dir = ctx.trash_dir.or(trash_dir);
    ctx.log_file = log_file;
    ctx.git_guard = git_guard;
    if progress_json {
        ctx.progress = Some(std::sync::Arc::new(crate::engine::progress::Progress::new()));
    }
    let report = run(registry, &selected, &ctx, force);
    if let Some(path) = html {
        crate::core::report_html::write_html(&report, "Sweep report", &path)
            .map_err(|err| Error::msg(format!("cannot write HTML report: {err}")))?;
    }
    if let Some(path) = markdown {
        crate::core::report_md::write_markdown(&report, "Sweep report", &path)
            .map_err(|err| Error::msg(format!("cannot write Markdown report: {err}")))?;
    }
    Ok(print_report(&report, json, summary, quiet))
}

fn print_report(report: &Report, json: bool, summary: bool, quiet: bool) -> i32 {
    if json {
        let value = if summary { report.to_json_summary(200) } else { report.to_json() };
        if let Err(err) = print_json(&value) {
            eprintln!("error: {err}");
            return 1;
        }
    } else if !quiet {
        println!("Files removed: {}", report.files_removed());
        println!("Reclaimed: {}", bytes_to_human(report.reclaimed(), false));
        println!("Special operations: {}", report.special_operations());
        println!("Skipped: {}", report.skipped());
        println!("Errors: {}", report.error_count());
        println!("Blocked: {}", report.blocked_count());
        println!("Warnings: {}", report.warnings.len());
        println!("Duration: {} ms", report.duration_ms);
        for failure in &report.errors {
            eprintln!("error: {} ({}): {}", failure.cleaner, failure.option, failure.message);
        }
        for failure in &report.blocked {
            eprintln!("blocked: {} ({}): {}", failure.cleaner, failure.option, failure.message);
        }
        for failure in &report.warnings {
            eprintln!("warning: {} ({}): {}", failure.cleaner, failure.option, failure.message);
        }
    }
    if report.aborted || report.error_count() > 0 { 1 } else { 0 }
}

fn make_guard(config: &Config) -> Guard {
    Guard::with_builtins(config.keep_list())
}

/// True when `modified` is at least `days` full days before `now`.
fn is_older_than(modified: std::time::SystemTime, now: std::time::SystemTime, days: u64) -> bool {
    now.duration_since(modified)
        .map(|age| age.as_secs() >= days.saturating_mul(86_400))
        .unwrap_or(false)
}

fn parse_min_size(value: &Option<String>) -> Result<u64> {
    match value {
        Some(text) => human_to_bytes(text),
        None => Ok(0),
    }
}

fn ensure_yes(yes: bool, what: &str) -> Result<()> {
    if yes {
        Ok(())
    } else {
        Err(Error::msg(format!("refusing to {what} without --yes")))
    }
}

fn collect_matches(roots: &[PathBuf], patterns: Option<&str>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let roots = if roots.is_empty() {
        vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))]
    } else {
        roots.to_vec()
    };
    let matcher = patterns.and_then(|p| {
        globset::GlobBuilder::new(p).literal_separator(true).build().ok()
    });
    for root in &roots {
        for path in scan_paths(root, &ScanOptions::files()) {
            let matched = match &matcher {
                Some(glob) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    glob.compile_matcher().is_match(&name)
                }
                None => true,
            };
            if matched {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn delete_guarded(paths: &[PathBuf], config: &Config, cleaner: &str, option: &str, preview: bool, journal: &mut Report) {
    let guard = make_guard(config);
    for path in paths {
        match guard.check(path, false) {
            Ok(_) => {}
            Err(_) => {
                journal.push(Entry::skipped(cleaner, option, path));
                continue;
            }
        }
        if preview {
            let size = size_of(path).unwrap_or(0);
            journal.push(Entry::new(EntryKind::Delete, cleaner, option, EntryKind::Delete.label(), Some(path), size));
            continue;
        }
        match delete(path, DeleteOptions::simple()) {
            Ok(true) => {
                let size = size_of(path).unwrap_or(0);
                journal.push(Entry::new(EntryKind::Delete, cleaner, option, EntryKind::Delete.label(), Some(path), size));
            }
            Ok(false) => journal.push(Entry::skipped(cleaner, option, path)),
            Err(err) => journal.fail(cleaner, option, err.to_string()),
        }
    }
}

fn deepscan(
    config: &Config,
    roots: &[PathBuf],
    pattern: Option<String>,
    preview: bool,
    yes: bool,
    json: bool,
    quiet: bool,
) -> Result<i32> {
    if !preview {
        ensure_yes(yes, "deep scan")?;
    }
    let roots_owned: Vec<PathBuf> = if roots.is_empty() {
        let configured = config.deepscan_roots();
        if configured.is_empty() {
            vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))]
        } else {
            configured
        }
    } else {
        roots.to_vec()
    };
    let patterns = match pattern {
        Some(ref p) => vec![p.clone()],
        None => config.deepscan_patterns.clone(),
    };
    let _watch = Stopwatch::start();
    let mut report = Report::new();
    for pattern in &patterns {
        let matches = collect_matches(&roots_owned, Some(pattern));
        delete_guarded(&matches, config, "deepscan", pattern, preview, &mut report);
    }
    Ok(print_report(&report, json, false, quiet))
}

fn bigfiles(
    config: &Config,
    roots: &[PathBuf],
    clean: bool,
    min_size: &Option<String>,
    older_than_days: Option<u64>,
    top: Option<usize>,
    max_depth: Option<usize>,
    yes: bool,
    json: bool,
    quiet: bool,
) -> Result<i32> {
    if clean {
        ensure_yes(yes, "delete large files")?;
    }
    let minimum = parse_min_size(min_size)?;
    let limit = top.unwrap_or(20);
    let roots_owned: Vec<PathBuf> = if roots.is_empty() {
        vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))]
    } else {
        roots.to_vec()
    };
    let now = std::time::SystemTime::now();
    let mut sizes: Vec<(PathBuf, u64)> = Vec::new();
    for root in &roots_owned {
        let mut options = ScanOptions::files();
        if let Some(depth) = max_depth {
            options = options.with_max_depth(depth);
        }
        for path in scan_paths(root, &options) {
            let size = size_of(&path).unwrap_or(0);
            let old_enough = older_than_days.map_or(true, |days| {
                std::fs::metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .map(|modified| is_older_than(modified, now, days))
                    .unwrap_or(false)
            });
            if size >= minimum && old_enough {
                sizes.push((path, size));
            }
        }
    }
    sizes.sort_by(|a, b| b.1.cmp(&a.1));
    sizes.truncate(limit);

    if json && !clean {
        let value = json!(sizes.iter().map(|(p, s)| json!({"path": p.display().to_string(), "bytes": s})).collect::<Vec<_>>());
        print_json(&value)?;
        return Ok(0);
    }
    let mut report = Report::new();
    if clean {
        let paths: Vec<PathBuf> = sizes.iter().map(|(p, _)| p.clone()).collect();
        delete_guarded(&paths, config, "bigfiles", "clean", false, &mut report);
        Ok(print_report(&report, json, false, quiet))
    } else {
        for (path, size) in &sizes {
            report.push(Entry::new(EntryKind::Delete, "bigfiles", "list", EntryKind::Delete.label(), Some(path), *size));
        }
        print_report(&report, json, false, quiet);
        Ok(0)
    }
}

fn du(path: &Option<PathBuf>, top: Option<usize>, json: bool, quiet: bool) -> Result<i32> {
    let root = path.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let limit = top.unwrap_or(10);
    let mut rows: Vec<(PathBuf, u64)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let child = entry.path();
            let size = if child.is_dir() { dir_size(&child) } else { size_of(&child).unwrap_or(0) };
            rows.push((child, size));
        }
    } else {
        rows.push((root.clone(), dir_size(&root)));
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    rows.truncate(limit);
    if json {
        print_json(&json!(rows.iter().map(|(p, s)| json!({"path": p.display().to_string(), "bytes": s})).collect::<Vec<_>>()))?;
    } else if !quiet {
        let total: u64 = rows.iter().map(|(_, s)| s).sum();
        println!("total: {}", bytes_to_human(total, false));
        for (p, s) in &rows {
            println!("{}\t{}", bytes_to_human(*s, false), p.display());
        }
    }
    Ok(0)
}

fn dupes(
    config: &Config,
    roots: &[PathBuf],
    clean: bool,
    min_size: &Option<String>,
    max_depth: Option<usize>,
    keep: Option<usize>,
    link: bool,
    yes: bool,
    json: bool,
    quiet: bool,
) -> Result<i32> {
    if link && !clean {
        return Err(Error::msg("--link requires --clean --yes"));
    }
    if clean {
        ensure_yes(yes, if link { "link duplicates" } else { "delete duplicates" })?;
    }
    let minimum = parse_min_size(min_size)?;
    let keep_n = keep.unwrap_or(1).max(1);
    let roots_owned: Vec<PathBuf> = if roots.is_empty() {
        vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))]
    } else {
        roots.to_vec()
    };
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    for root in &roots_owned {
        let mut options = ScanOptions::files();
        if let Some(depth) = max_depth {
            options = options.with_max_depth(depth);
        }
        for path in scan_paths(root, &options) {
            let size = size_of(&path).unwrap_or(0);
            if size >= minimum {
                by_size.entry(size).or_default().push(path);
            }
        }
    }
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    for paths in by_size.into_values() {
        if paths.len() < 2 {
            continue;
        }
        let mut by_hash: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for path in paths {
            let hash = match std::fs::read(&path) {
                Ok(bytes) => format!("{:x}", sha2::Sha256::digest(bytes)),
                Err(_) => continue,
            };
            by_hash.entry(hash).or_default().push(path);
        }
        for group in by_hash.into_values() {
            if group.len() > keep_n {
                groups.push(group);
            }
        }
    }
    groups.sort_by(|a, b| a[0].cmp(&b[0]));

    if json && !clean {
        let value = json!(groups.iter().map(|g| json!({
            "count": g.len(),
            "paths": g.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>());
        print_json(&value)?;
        return Ok(0);
    }
    let mut report = Report::new();
    let guard = make_guard(config);
    for group in &groups {
        let mut sorted = group.clone();
        sorted.sort();
        let victims = &sorted[keep_n.min(sorted.len())..];
        for path in victims {
            let size = size_of(path).unwrap_or(0);
            if clean && link {
                let keeper = &sorted[0];
                if fsutil::reflink::same_inode(keeper, path) {
                    report.push(Entry::skipped("dupes", "link", path));
                    continue;
                }
                if guard.check(path, false).is_err() {
                    report.push(Entry::skipped("dupes", "link", path));
                    continue;
                }
                match fsutil::reflink::link_replace(keeper, path, true) {
                    Ok(fsutil::reflink::LinkOutcome::SkippedMutable) => {
                        report.push(Entry::skipped("dupes", "link", path));
                    }
                    Ok(outcome) => report.push(Entry::new(
                        EntryKind::Command,
                        "dupes",
                        "link",
                        format!("{} -> {}", outcome_label(outcome), keeper.display()),
                        Some(path),
                        size,
                    )),
                    Err(error) => report.fail("dupes", "link", error.to_string()),
                }
            } else if clean {
                let mut single = Report::new();
                delete_guarded(std::slice::from_ref(path), config, "dupes", "clean", false, &mut single);
                report.merge(single);
            } else {
                report.push(Entry::new(EntryKind::Delete, "dupes", "list", EntryKind::Delete.label(), Some(path), size));
            }
        }
    }
    Ok(print_report(&report, json, false, quiet))
}

fn outcome_label(outcome: fsutil::reflink::LinkOutcome) -> &'static str {
    match outcome {
        fsutil::reflink::LinkOutcome::Reflinked => "reflinked",
        fsutil::reflink::LinkOutcome::Hardlinked => "hardlinked",
        fsutil::reflink::LinkOutcome::SkippedMutable => "skipped",
    }
}

fn analyze(root: &Option<PathBuf>, top: Option<usize>, max_depth: Option<usize>, json: bool, quiet: bool) -> Result<i32> {
    let base = root.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let _ = max_depth;
    du(&Some(base), top, json, quiet)
}

fn is_dev_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(fsutil::reflink::is_dev_dir_name)
}

fn devscan(
    config: &Config,
    roots: &[PathBuf],
    clean: bool,
    max_depth: Option<usize>,
    min_size: &Option<String>,
    yes: bool,
    json: bool,
    quiet: bool,
) -> Result<i32> {
    if clean {
        ensure_yes(yes, "clean dev artifacts")?;
    }
    let minimum = parse_min_size(min_size).unwrap_or(0);
    let roots_owned: Vec<PathBuf> = if roots.is_empty() {
        vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))]
    } else {
        roots.to_vec()
    };
    let mut rows: Vec<(PathBuf, u64)> = Vec::new();
    for root in &roots_owned {
        let mut options = ScanOptions::all();
        if let Some(depth) = max_depth {
            options = options.with_max_depth(depth);
        }
        for path in scan_paths(root, &options) {
            if path.is_dir() && is_dev_dir(&path) {
                let size = dir_size(&path);
                if size >= minimum {
                    rows.push((path, size));
                }
            }
        }
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    rows.truncate(20);
    if json && !clean {
        print_json(&json!(rows.iter().map(|(p, s)| json!({"path": p.display().to_string(), "bytes": s})).collect::<Vec<_>>()))?;
        return Ok(0);
    }
    let mut report = Report::new();
    if clean {
        let paths: Vec<PathBuf> = rows.iter().map(|(p, _)| p.clone()).collect();
        delete_guarded(&paths, config, "devscan", "clean", false, &mut report);
    } else {
        for (p, s) in &rows {
            report.push(Entry::new(EntryKind::Delete, "devscan", "list", EntryKind::Delete.label(), Some(p), *s));
        }
    }
    Ok(print_report(&report, json, false, quiet))
}

fn leftovers(
    config: &Config,
    name: &str,
    clean: bool,
    min_size: Option<String>,
    yes: bool,
    json: bool,
    quiet: bool,
) -> Result<i32> {
    if clean {
        ensure_yes(yes, "clean leftovers")?;
    }
    let minimum = parse_min_size(&min_size)?;
    let home = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut matches = Vec::new();
    for path in scan_paths(&home, &ScanOptions::files()) {
        let name_of = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name_of.to_lowercase().contains(&name.to_lowercase()) {
            if size_of(&path).unwrap_or(0) >= minimum {
                matches.push(path);
            }
        }
    }
    matches.sort();
    let mut report = Report::new();
    delete_guarded(&matches, config, "leftovers", name, !clean, &mut report);
    if !clean && !quiet && !json {
        // delete_guarded already recorded preview entries.
    }
    Ok(print_report(&report, json, false, quiet))
}

fn watch(paths: &[PathBuf], threshold: Option<String>, json: bool, quiet: bool) -> Result<i32> {
    let minimum = parse_min_size(&threshold)?;
    let roots: Vec<PathBuf> = if paths.is_empty() {
        vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))]
    } else {
        paths.to_vec()
    };
    let mut big: Vec<(PathBuf, u64)> = Vec::new();
    for root in &roots {
        for path in scan_paths(root, &ScanOptions::files()) {
            if let Ok(size) = size_of(&path) {
                if size >= minimum {
                    big.push((path, size));
                }
            }
        }
    }
    big.sort_by(|a, b| b.1.cmp(&a.1));
    big.truncate(20);
    if json {
        print_json(&json!(big.iter().map(|(p, s)| json!({"path": p.display().to_string(), "bytes": s})).collect::<Vec<_>>()))?;
    } else if !quiet {
        for (p, s) in &big {
            println!("{}\t{}", bytes_to_human(*s, false), p.display());
        }
    }
    Ok(0)
}

fn import(path: &Path, json: bool, quiet: bool) -> Result<i32> {
    if !path.exists() {
        return Err(Error::msg(format!("import path not found: {}", path.display())));
    }
    let files: Vec<PathBuf> = if path.is_dir() {
        std::fs::read_dir(path)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().map(|x| x == "toml").unwrap_or(false))
                    .collect()
            })
            .unwrap_or_default()
    } else if path.extension().map(|x| x == "toml").unwrap_or(false) {
        vec![path.to_path_buf()]
    } else {
        Vec::new()
    };
    if json {
        print_json(&json!({"import_path": path.display().to_string(), "toml_files": files.len()}))?;
    } else if !quiet {
        println!("found {} definition file(s) in {}", files.len(), path.display());
        println!("pass it to sweep with --define <dir>");
    }
    Ok(0)
}

fn config_cmd(
    config: &mut Config,
    op: &str,
    key: Option<String>,
    value: Option<String>,
    config_path: &Option<PathBuf>,
    json: bool,
    quiet: bool,
) -> Result<i32> {
    match op {
        "show" | "list" => {
            if json {
                let obj = serde_json::to_value(config.fields()).map_err(|e| Error::msg(e.to_string()))?;
                print_json(&obj)?;
            } else if !quiet {
                for (name, value) in config.fields() {
                    println!("{name} = {value}");
                }
            }
        }
        "get" => {
            let name = key.ok_or_else(|| Error::msg("config get requires a key"))?;
            let value = config.get_field(&name)?;
            println!("{value}");
        }
        "set" => {
            let name = key.ok_or_else(|| Error::msg("config set requires a key"))?;
            let value = value.ok_or_else(|| Error::msg("config set requires a value"))?;
            config.set_field(&name, &value)?;
            let path = config_path.clone().unwrap_or_else(default_path);
            config.save(&path)?;
            if !quiet {
                println!("{name} = {value}");
            }
        }
        other => return Err(Error::msg(format!("unknown config operation '{other}' (expected show, list, get, set)"))),
    }
    Ok(0)
}

fn supported_lang(code: &str) -> Result<String> {
    let code = crate::i18n::normalize_lang(code);
    if crate::i18n::SUPPORTED_LANGUAGES.contains(&code.as_str()) {
        Ok(code)
    } else {
        Err(Error::msg(format!(
            "unsupported language '{code}' (supported: {})",
            crate::i18n::SUPPORTED_LANGUAGES.join(", ")
        )))
    }
}

fn lang_cmd(config: &mut Config, code: Option<String>, config_path: &Option<PathBuf>, json: bool, quiet: bool) -> Result<i32> {
    if let Some(code) = code {
        let code = supported_lang(&code)?;
        config.languages = vec![code.clone()];
        let path = config_path.clone().unwrap_or_else(default_path);
        config.save(&path)?;
        if !quiet {
            println!("language set to {code}");
        }
    } else if json {
        print_json(&json!({"languages": config.languages}))?;
    } else if !quiet {
        println!("{}", config.languages.join(", "));
    }
    Ok(0)
}

fn completions(shell: &str) -> Result<i32> {
    let mut cmd = Cli::command();
    let shell = match shell.to_lowercase().as_str() {
        "bash" => clap_complete::Shell::Bash,
        "zsh" => clap_complete::Shell::Zsh,
        "fish" => clap_complete::Shell::Fish,
        "powershell" | "ps" | "pwsh" => clap_complete::Shell::PowerShell,
        "elvish" => clap_complete::Shell::Elvish,
        other => return Err(Error::msg(format!("unknown shell '{other}' (expected bash, zsh, fish, powershell, elvish)"))),
    };
    clap_complete::generate(shell, &mut cmd, "sweep", &mut std::io::stdout());
    Ok(0)
}

fn man(output: Option<PathBuf>, quiet: bool) -> Result<i32> {
    let mut cmd = Cli::command();
    let mut buf = Vec::new();
    cmd.write_long_help(&mut buf).map_err(|e| Error::msg(e.to_string()))?;
    buf.push(b'\n');
    match output {
        Some(path) => {
            std::fs::write(&path, &buf).map_err(|e| Error::io(path.clone(), e))?;
            if !quiet {
                println!("man page written to {}", path.display());
            }
        }
        None => {
            std::io::Write::write_all(&mut std::io::stdout(), &buf).map_err(|e| Error::msg(e.to_string()))?;
        }
    }
    Ok(0)
}

fn target_os_matches(filter: &str, target: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    if filter.is_empty() {
        return true;
    }
    let target = target.trim().to_lowercase();
    if filter == target {
        return true;
    }
    if filter == "unix" {
        return matches!(target.as_str(), "linux" | "macos" | "bsd" | "freebsd" | "openbsd" | "netbsd" | "dragonfly");
    }
    if filter == "darwin" {
        return target == "macos";
    }
    if filter == "bsd" {
        return target == "macos" || target == "bsd" || target == "freebsd" || target == "openbsd" || target == "netbsd" || target == "dragonfly";
    }
    false
}

#[allow(unused_imports)]
use std::io::Write as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_flags_parse_before_subcommand() {
        let cli = Cli::try_parse_from(["sweep", "--json", "--quiet", "list"]).unwrap();
        assert!(cli.json && cli.quiet);
    }

    #[test]
    fn preview_selectors_parse() {
        let cli = Cli::try_parse_from(["sweep", "preview", "system.tmp", "--all", "--exclude", "x"]).unwrap();
        match cli.command {
            Command::Preview { selectors, all, exclude, .. } => {
                assert_eq!(selectors, vec!["system.tmp"]);
                assert!(all);
                assert_eq!(exclude, vec!["x"]);
            }
            _ => panic!("expected preview"),
        }
    }

    #[test]
    fn clean_requires_yes_at_runtime_not_parse_time() {
        let cli = Cli::try_parse_from(["sweep", "clean", "system.tmp"]).unwrap();
        assert!(!cli.yes);
    }

    #[test]
    fn target_os_matching() {
        assert!(target_os_matches("", "linux"));
        assert!(target_os_matches("linux", "linux"));
        assert!(target_os_matches("unix", "macos"));
        assert!(target_os_matches("darwin", "macos"));
        assert!(target_os_matches("bsd", "macos"));
        assert!(!target_os_matches("windows", "linux"));
    }

    #[test]
    fn stub_commands_parse() {
        for name in ["shred", "schedule", "doctor", "providers", "sysinfo"] {
            let cli = Cli::try_parse_from(["sweep", name]).unwrap();
            let _ = cli;
        }
    }

    #[test]
    fn language_codes_are_normalized_and_checked() {
        assert_eq!(supported_lang("tr_TR.UTF-8").unwrap(), "tr");
        assert_eq!(supported_lang("EN").unwrap(), "en");
        assert!(supported_lang("xx").is_err());
    }

    #[test]
    fn devscan_only_matches_known_dirs() {
        assert!(is_dev_dir(Path::new("project/node_modules")));
        assert!(is_dev_dir(Path::new("rust/target")));
        assert!(!is_dev_dir(Path::new("project/src")));
        assert!(!is_dev_dir(Path::new("home/Documents")));
    }

    #[test]
    fn older_than_uses_full_days() {
        let now = std::time::SystemTime::now();
        let three_days = now - std::time::Duration::from_secs(3 * 86_400);
        assert!(is_older_than(three_days, now, 2));
        assert!(!is_older_than(now - std::time::Duration::from_secs(86_400), now, 2));
        assert!(is_older_than(now, now, 0));
    }

    #[test]
    fn dupes_link_replaces_victim_with_hardlink() {
        let dir = crate::fixture("cli_dupes_link");
        let keeper = dir.join("a.bin");
        let victim = dir.join("b.bin");
        std::fs::write(&keeper, b"same-content").unwrap();
        std::fs::write(&victim, b"same-content").unwrap();
        let config = Config::default();

        assert!(dupes(&config, std::slice::from_ref(&dir), false, &None, None, None, true, true, false, true).is_err());
        let code = dupes(&config, std::slice::from_ref(&dir), true, &None, None, None, true, true, false, true).unwrap();
        assert_eq!(code, 0);
        assert_eq!(std::fs::read(&victim).unwrap(), b"same-content");
        assert!(fsutil::reflink::same_inode(&keeper, &victim));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

    // (ctx deep options are wired in run_selection below)
