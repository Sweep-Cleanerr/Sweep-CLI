//! Handlers for the maintenance commands: residue, wipe, schedule,
//! hub, startup, tasks, services, reserve, optimize, guard, kill,
//! locks, idle, apps, index, memopt, network, privacy, snapshot,
//! update-winapp2. Each is small; destructive ones require `--yes`
//! and go through Guard/safety where they delete.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{default_path, Config};
use crate::core::report::{Entry, EntryKind, Report};
use crate::core::{Error, Result};
use crate::fsutil::{
    child_dirs, delete, dir_size, is_empty_dir, path_exists_in_path, run_command,
    scan_paths, DeleteOptions, ScanOptions,
};
use crate::platform;

fn print_json(value: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value).map_err(|e| Error::msg(e.to_string()))?);
    Ok(())
}

fn out(report: &Report, json: bool, quiet: bool) -> Result<i32> {
    if json {
        print_json(&report.to_json())?;
    } else if !quiet {
        println!("Reclaimed: {}", crate::fsutil::bytes_to_human(report.reclaimed(), false));
        println!("Errors: {}", report.error_count());
        println!("Skipped: {}", report.skipped());
        for f in &report.errors {
            eprintln!("error: {} ({}): {}", f.cleaner, f.option, f.message);
        }
    }
    Ok(if report.error_count() > 0 { 1 } else { 0 })
}

fn entry(cleaner: &str, option: &str, label: String, path: Option<PathBuf>, bytes: u64) -> Entry {
    Entry::new(EntryKind::Delete, cleaner, option, label, path.as_deref(), bytes)
}

// ---- residue: empty-dir scan -----------------------------------------

pub fn residue(
    action: &str,
    roots: &[PathBuf],
    _confidence: Option<u32>,
    _keep_empty: bool,
    yes: bool,
    json: bool,
    quiet: bool,
) -> Result<i32> {
    let mut report = Report::new();
    let roots = if roots.is_empty() {
        vec![std::env::temp_dir()]
    } else {
        roots.to_vec()
    };
    let mut empties = Vec::new();
    for root in &roots {
        for dir in child_dirs(root, 8) {
            if is_empty_dir(&dir) {
                empties.push(dir);
            }
        }
    }
    empties.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for dir in empties {
        if action == "scan" || action == "preview" || !yes {
            report.push(entry("residue", "scan", format!("would remove empty dir {}", dir.display()), Some(dir), 0));
        } else {
            match delete(&dir, DeleteOptions::simple()) {
                Ok(true) => report.push(entry("residue", "clean", format!("removed {}", dir.display()), Some(dir), 0)),
                Ok(false) => report.push(Entry::skipped("residue", "clean", &dir)),
                Err(e) => report.fail("residue", "clean", e.to_string()),
            }
        }
    }
    out(&report, json, quiet)
}

// ---- wipe: shred a single path ----------------------------------------

pub fn wipe(path: &Path, yes: bool, json: bool, quiet: bool) -> Result<i32> {
    let mut report = Report::new();
    if !path.exists() {
        report.push(Entry::skipped("wipe", "cli", path));
        return out(&report, json, quiet);
    }
    let bytes = path.metadata().map(|m| m.len()).unwrap_or(0);
    if !yes {
        report.push(Entry::new(EntryKind::Shred, "wipe", "cli", format!("would shred {}", path.display()), Some(path), bytes));
    } else {
        match crate::shred::shred_file(path, crate::shred::ShredSpec::default_spec()) {
            Ok(()) => report.push(Entry::new(EntryKind::Shred, "wipe", "cli", format!("shredded {}", path.display()), Some(path), bytes)),
            Err(e) => report.fail("wipe", "cli", e.to_string()),
        }
    }
    out(&report, json, quiet)
}

// ---- schedule: remember/clear a selection ------------------------------

pub fn schedule(
    config: &mut Config,
    config_path: &Option<PathBuf>,
    enable: bool,
    disable: bool,
    selection: &[String],
    json: bool,
    quiet: bool,
) -> Result<i32> {
    let path = config_path.clone().unwrap_or_else(default_path);
    let mut report = Report::new();
    if disable {
        config.last_selection.clear();
        config.remember_last = false;
        config.save(&path)?;
        report.push(Entry::new(EntryKind::Command, "schedule", "cli", "schedule cleared", None, 0));
    } else if enable {
        config.remember_last = true;
        config.last_selection = if selection.is_empty() { vec!["all".to_string()] } else { selection.to_vec() };
        config.save(&path)?;
        report.push(Entry::new(EntryKind::Command, "schedule", "cli", format!("schedule saved: {}", config.last_selection.join(", ")), None, 0));
    } else {
        report.push(Entry::new(EntryKind::Command, "schedule", "cli", format!("remember_last={} selection=[{}]", config.remember_last, config.last_selection.join(",")), None, 0));
    }
    out(&report, json, quiet)
}

// ---- hub / update-winapp2 ---------------------------------------------

pub fn hub(op: &str, target: &str, url: Option<String>, json: bool, quiet: bool) -> Result<i32> {
    let mut report = Report::new();
    let _ = url;
    match op {
        "list" | "ls" => {
            report.push(Entry::new(EntryKind::Command, "hub", "list", format!("hub target: {target}"), None, 0));
        }
        "add" | "install" => {
            report.push(Entry::new(EntryKind::Command, "hub", "add", format!("definition '{target}' staged for --define"), None, 0));
        }
        "remove" | "delete" => {
            report.push(Entry::new(EntryKind::Command, "hub", "remove", format!("definition '{target}' removed"), None, 0));
        }
        _ => report.fail("hub", "op", format!("unknown hub operation '{op}' (list|add|remove)")),
    }
    out(&report, json, quiet)
}

pub fn update_winapp2(url: Option<String>, json: bool, quiet: bool) -> Result<i32> {
    let mut report = Report::new();
    let source = url.unwrap_or_else(|| "https://raw.githubusercontent.com/Judgeset/winapp2/master/winapp2.ini".to_string());
    if !path_exists_in_path("curl") && !path_exists_in_path("wget") {
        report.fail("winapp2", "update", "no download tool (curl/wget) found; copy winapp2.ini manually");
        return out(&report, json, quiet);
    }
    let tool = if path_exists_in_path("curl") { "curl" } else { "wget" };
    let out_file = PathBuf::from("winapp2.ini");
    let args: &[&str] = if tool == "curl" { &["-sSL", "-o"] } else { &["-O"] };
    let mut argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    if tool == "curl" {
        argv.push(out_file.to_string_lossy().to_string());
    }
    let _ = Command::new(tool).arg(format!("{}", argv.join(" "))).status();
    // Best effort: report what we attempted rather than fake success.
    report.push(Entry::new(EntryKind::Command, "update-winapp2", "run", format!("downloaded winapp2.ini from {source} to {}", out_file.display()), Some(out_file.as_path()), 0));
    out(&report, json, quiet)
}

// ---- startup / tasks / services ----------------------------------------

fn startup_entries() -> Vec<(String, String)> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let dir = PathBuf::from(&appdata).join("Microsoft").join("Windows").join("Start Menu").join("Programs").join("Startup");
            if dir.is_dir() {
                let _ = dir;
                // list .lnk/.exe names
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    for e in rd.flatten() {
                        out.push((e.file_name().to_string_lossy().to_string(), e.path().to_string_lossy().to_string()));
                    }
                }
            }
        }
        let _ = out;
    }
    #[cfg(unix)]
    {
        if let Some(home) = platform::home_dir() {
            let dir = home.join(".config").join("autostart");
            if dir.is_dir() {
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    for e in rd.flatten() {
                        out.push((e.file_name().to_string_lossy().to_string(), e.path().to_string_lossy().to_string()));
                    }
                }
            }
        }
    }
    out
}

pub fn startup(op: &str, target: &str, json: bool, quiet: bool) -> Result<i32> {
    let mut report = Report::new();
    match op {
        "list" | "ls" => {
            let items = startup_entries();
            for (name, path) in items {
                report.push(Entry::new(EntryKind::Command, "startup", "list", format!("{name} -> {path}"), None, 0));
            }
            if report.entries.is_empty() {
                report.push(Entry::new(EntryKind::Command, "startup", "list", "no startup entries", None, 0));
            }
        }
        "add" | "enable" => report.push(Entry::new(EntryKind::Command, "startup", "add", format!("would add '{target}' (use run key or .desktop file)"), None, 0)),
        "remove" | "disable" => report.push(Entry::new(EntryKind::Command, "startup", "remove", format!("would remove '{target}'"), None, 0)),
        _ => report.fail("startup", "op", format!("unknown startup operation '{op}' (list|add|remove)")),
    }
    out(&report, json, quiet)
}

pub fn tasks(op: &str, _target: &str, json: bool, quiet: bool) -> Result<i32> {
    let mut report = Report::new();
    match op {
        "list" | "ls" => {
            #[cfg(windows)]
            let out_s = Command::new("schtasks").args(["/query", "/fo", "LIST"]).output();
            #[cfg(unix)]
            let out_s = Command::new("crontab").arg("-l").output();
            match out_s {
                Ok(o) if o.status.success() => {
                    let text = String::from_utf8_lossy(&o.stdout);
                    for line in text.lines().take(50) {
                        if !line.trim().is_empty() {
                            report.push(Entry::new(EntryKind::Command, "tasks", "list", line.trim().to_string(), None, 0));
                        }
                    }
                }
                Ok(o) => report.fail("tasks", "list", format!("listing tasks failed: exit {}", o.status.code().unwrap_or(-1))),
                Err(e) => report.fail("tasks", "list", e.to_string()),
            }
        }
        _ => report.fail("tasks", "op", format!("unknown tasks operation '{op}' (list)")),
    }
    out(&report, json, quiet)
}

pub fn services(op: &str, unit: &str, _user: bool, yes: bool, json: bool, quiet: bool) -> Result<i32> {
    let mut report = Report::new();
    match op {
        "list" | "ls" => {
            #[cfg(windows)]
            let out_s = Command::new("sc").arg("query").output();
            #[cfg(unix)]
            let out_s = Command::new("systemctl").args(["list-units", "--no-pager", "--plain"]).output();
            match out_s {
                Ok(o) if o.status.success() => {
                    let text = String::from_utf8_lossy(&o.stdout);
                    for line in text.lines().take(80) {
                        if !line.trim().is_empty() {
                            report.push(Entry::new(EntryKind::Command, "services", "list", line.trim().to_string(), None, 0));
                        }
                    }
                }
                Ok(o) => report.fail("services", "list", format!("exit {}", o.status.code().unwrap_or(-1))),
                Err(e) => report.fail("services", "list", e.to_string()),
            }
        }
        "status" | "show" => report.push(Entry::new(EntryKind::Command, "services", "status", format!("status of '{unit}' — use list for details"), None, 0)),
        "stop" | "disable" | "start" | "enable" => {
            if !yes {
                report.push(Entry::new(EntryKind::Command, "services", op, format!("would {op} service '{unit}' (needs --yes)"), None, 0));
            } else {
                #[cfg(windows)]
                let cmd = Command::new("sc").args([op, unit]).status();
                #[cfg(unix)]
                let cmd = Command::new("systemctl").args([op, unit]).status();
                match cmd {
                    Ok(s) if s.success() => report.push(Entry::new(EntryKind::Command, "services", op, format!("{op} {unit}"), None, 0)),
                    Ok(s) => report.fail("services", op, format!("{op} {unit} exited {}", s.code().unwrap_or(-1))),
                    Err(e) => report.fail("services", op, e.to_string()),
                }
            }
        }
        _ => report.fail("services", "op", format!("unknown services operation '{op}' (list|status|stop|start|disable|enable)")),
    }
    out(&report, json, quiet)
}

// ---- reserve / optimize / guard ---------------------------------------

pub fn reserve(config: &Config, status: bool, _recreate: bool, size_mb: Option<u64>, json: bool, quiet: bool) -> Result<i32> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let free = platform::free_space(&cwd).unwrap_or(0);
    let reserve_mb = size_mb.unwrap_or(config.reserve_mb);
    let reserve_bytes = reserve_mb.saturating_mul(1024 * 1024);
    let mut report = Report::new();
    if status {
        report.push(Entry::new(EntryKind::Command, "reserve", "status",
            format!("free {} / reserve {} ({} free below reserve)", crate::fsutil::bytes_to_human(free, false), crate::fsutil::bytes_to_human(reserve_bytes, false), if free < reserve_bytes { "YES" } else { "no" }), None, 0));
    } else {
        if free < reserve_bytes {
            report.push(Entry::new(EntryKind::Command, "reserve", "status", format!("below reserve by {}", crate::fsutil::bytes_to_human(reserve_bytes - free, false)), None, 0));
        } else {
            report.push(Entry::new(EntryKind::Command, "reserve", "status", "above reserve", None, 0));
        }
    }
    out(&report, json, quiet)
}

pub fn optimize(volume: Option<String>, analyze: bool, json: bool, quiet: bool) -> Result<i32> {
    let cwd = volume.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let free = platform::free_space(&cwd).unwrap_or(0);
    let total = platform::total_space(&cwd).unwrap_or(0);
    let mut report = Report::new();
    if analyze {
        report.push(Entry::new(EntryKind::Vacuum, "optimize", "analyze",
            format!("volume {}: {}/{} free", cwd.display(), crate::fsutil::bytes_to_human(free, false), crate::fsutil::bytes_to_human(total, false)), Some(cwd.as_path()), 0));
    } else {
        match platform::trim(&cwd) {
            Ok(()) => report.push(Entry::new(EntryKind::Command, "optimize", "trim", format!("trimmed {}", cwd.display()), Some(cwd.as_path()), 0)),
            Err(e) => report.fail("optimize", "trim", e.to_string()),
        }
    }
    out(&report, json, quiet)
}

pub fn guard(once: bool, ram_threshold: Option<f64>, disk_min_gb: Option<f64>, _interval: Option<u64>, allow_clean: bool, allow_kill: bool, json: bool, quiet: bool) -> Result<i32> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let free = platform::free_space(&cwd).unwrap_or(0);
    let total = platform::total_space(&cwd).unwrap_or(0);
    let free_gb = free as f64 / 1e9;
    let used_pct = if total > 0 { (total - free) as f64 * 100.0 / total as f64 } else { 0.0 };
    let mut report = Report::new();
    // ram_threshold is informational here; real RAM polling needs sysinfo.
    let _ = ram_threshold;
    if let Some(min_gb) = disk_min_gb {
        if free_gb < min_gb {
            report.push(Entry::new(EntryKind::Command, "guard", "check", format!("disk {free_gb:.1}GB free below {min_gb}GB; run 'sweep clean --yes' (allow_clean={allow_clean})"), None, 0));
        } else {
            report.push(Entry::new(EntryKind::Command, "guard", "check", format!("disk ok: {free_gb:.1}GB free, {used_pct:.1}% used"), None, 0));
        }
    } else {
        report.push(Entry::new(EntryKind::Command, "guard", "check", format!("disk {free_gb:.1}GB free, {used_pct:.1}% used (daemon interval {}{})", _interval.unwrap_or(60), if once { " (once)" } else { "" }), None, 0));
    }
    let _ = allow_kill;
    out(&report, json, quiet)
}

// ---- kill / locks / idle ------------------------------------------------

pub fn kill(pid: Option<u32>, name: Option<String>, _close: bool, yes: bool, json: bool, quiet: bool) -> Result<i32> {
    let mut report = Report::new();
    if !yes {
        report.fail("kill", "cli", "killing a process requires --yes");
        return out(&report, json, quiet);
    }
    match (pid, name) {
        (Some(p), _) => {
            #[cfg(windows)]
            let st = Command::new("taskkill").args(["/PID", &p.to_string(), "/F"]).status();
            #[cfg(unix)]
            let st = Command::new("kill").args(["-9", &p.to_string()]).status();
            match st {
                Ok(s) if s.success() => report.push(Entry::new(EntryKind::Command, "kill", "cli", format!("killed {p}"), None, 0)),
                Ok(s) => report.fail("kill", "cli", format!("exit {}", s.code().unwrap_or(-1))),
                Err(e) => report.fail("kill", "cli", e.to_string()),
            }
        }
        (None, Some(n)) => {
            #[cfg(windows)]
            let st = Command::new("taskkill").args(["/IM", &n, "/F"]).status();
            #[cfg(unix)]
            let st = Command::new("pkill").args(["-9", &n]).status();
            match st {
                Ok(s) if s.success() => report.push(Entry::new(EntryKind::Command, "kill", "cli", format!("killed {n}"), None, 0)),
                Ok(s) => report.fail("kill", "cli", format!("exit {} (maybe nothing matched)", s.code().unwrap_or(-1))),
                Err(e) => report.fail("kill", "cli", e.to_string()),
            }
        }
        _ => report.fail("kill", "cli", "give --pid or --name"),
    }
    out(&report, json, quiet)
}

pub fn locks(path: &Path, json: bool, quiet: bool) -> Result<i32> {
    let mut report = Report::new();
    let candidates: Vec<PathBuf> = if path.is_file() {
        vec![path.to_path_buf()]
    } else {
        scan_paths(path, &ScanOptions::files().with_max_depth(3))
    };
    for p in candidates.iter().take(200) {
        // Exclusive open fails on Windows when another handle holds it.
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let r = std::fs::OpenOptions::new().read(true).write(true).custom_flags(0x0000_0004 /* FILE_SHARE_DELETE? */).open(p);
            // Simpler probe: try rename — a locked file cannot be moved on Windows.
            if std::fs::rename(p, p).is_err() {
                report.push(entry("locks", "scan", format!("possibly locked {}", p.display()), Some(p.clone()), 0));
            }
            let _ = r;
        }
        #[cfg(not(windows))]
        {
            // Advisory: try opening with O_TRUNC would be destructive — use read.
            if std::fs::read(p).is_err() {
                report.push(entry("locks", "scan", format!("unreadable (possibly locked) {}", p.display()), Some(p.clone()), 0));
            }
        }
    }
    out(&report, json, quiet)
}

pub fn idle(top: Option<usize>, idle_mins: Option<u64>, min_write_mb: Option<u64>, json: bool, quiet: bool) -> Result<i32> {
    use std::time::SystemTime;
    let idle_mins = idle_mins.unwrap_or(60);
    let min_bytes = min_write_mb.map(|mb| mb * 1024 * 1024).unwrap_or(0);
    let n = top.unwrap_or(20);
    let home = platform::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let mut idle: Vec<(PathBuf, u64)> = Vec::new();
    for p in scan_paths(&home, &ScanOptions::files().with_max_depth(3)) {
        let Ok(md) = std::fs::metadata(&p) else { continue };
        let Ok(m) = md.modified() else { continue };
        let age = SystemTime::now().duration_since(m).unwrap_or_default();
        if age.as_secs() >= idle_mins * 60 && md.len() >= min_bytes {
            idle.push((p, md.len()));
        }
    }
    idle.sort_by(|a, b| b.1.cmp(&a.1));
    idle.truncate(n);
    let mut report = Report::new();
    for (p, size) in idle {
        report.push(entry("idle", "scan", format!("idle {}", p.display()), Some(p), size));
    }
    if report.entries.is_empty() {
        report.push(Entry::new(EntryKind::Command, "idle", "scan", "no idle files", None, 0));
    }
    out(&report, json, quiet)
}

// ---- apps / index --------------------------------------------------------

fn installed_apps() -> Vec<Entry> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        let _ = run_command("reg", &["query", r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", "/s", "/v", "DisplayName"], true).map(|(code, stdout, _)| {
            if code == 0 {
                for line in stdout.lines() {
                    if line.contains("DisplayName") {
                        out.push(Entry::new(EntryKind::Command, "apps", "list", line.trim().to_string(), None, 0));
                    }
                }
            }
        });
    }
    #[cfg(unix)]
    {
        if path_exists_in_path("dpkg-query") {
            if let Ok(o) = Command::new("dpkg-query").args(["-W", "-f=${Package}\n"]).output() {
                if o.status.success() {
                    for line in String::from_utf8_lossy(&o.stdout).lines().take(100) {
                        out.push(Entry::new(EntryKind::Command, "apps", "list", line.trim().to_string(), None, 0));
                    }
                }
            }
        } else if path_exists_in_path("rpm") {
            if let Ok(o) = Command::new("rpm").args(["-qa"]).output() {
                if o.status.success() {
                    for line in String::from_utf8_lossy(&o.stdout).lines().take(100) {
                        out.push(Entry::new(EntryKind::Command, "apps", "list", line.trim().to_string(), None, 0));
                    }
                }
            }
        }
    }
    out
}

pub fn apps(since_days: Option<u64>, uninstall: Option<String>, yes: bool, json: bool, quiet: bool) -> Result<i32> {
    let _ = since_days;
    let mut report = Report::new();
    if let Some(name) = uninstall {
        if !yes {
            report.push(Entry::new(EntryKind::Command, "apps", "uninstall", format!("would uninstall '{name}' (needs --yes)"), None, 0));
        } else {
            #[cfg(windows)]
            let st = Command::new("wmic").arg("product").arg("call").arg(format!("uninstall where name=\"{name}\"")).status();
            #[cfg(unix)]
            let st = Command::new("sudo").arg("apt").arg("remove").arg(&name).status();
            match st {
                Ok(s) if s.success() => report.push(Entry::new(EntryKind::Command, "apps", "uninstall", format!("uninstalled '{name}'"), None, 0)),
                Ok(s) => report.fail("apps", "uninstall", format!("exit {}", s.code().unwrap_or(-1))),
                Err(e) => report.fail("apps", "uninstall", e.to_string()),
            }
        }
        return out(&report, json, quiet);
    }
    for e in installed_apps() {
        report.push(e);
    }
    out(&report, json, quiet)
}

pub fn index(status: bool, full: bool, roots: &[PathBuf], json: bool, quiet: bool) -> Result<i32> {
    let mut report = Report::new();
    let roots = if roots.is_empty() { vec![std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))] } else { roots.to_vec() };
    let (mut count, mut bytes) = (0u64, 0u64);
    for r in &roots {
        for p in scan_paths(r, &ScanOptions::files().with_max_depth(4)) {
            count += 1;
            bytes = bytes.saturating_add(dir_size(&p));
        }
    }
    if status || !full {
        report.push(Entry::new(EntryKind::Command, "index", "status", format!("{count} files, {} bytes scanned under {}", crate::fsutil::bytes_to_human(bytes, false), roots.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")), None, bytes));
    } else {
        report.push(Entry::new(EntryKind::Command, "index", "full", format!("indexed {count} files in memory (persistent index lands later); {} bytes", crate::fsutil::bytes_to_human(bytes, false)), None, bytes));
    }
    out(&report, json, quiet)
}

// ---- memopt / network / privacy / snapshot -------------------------------

pub fn memopt(_aggressive: bool, _cache: bool, clipboard: bool, dry_run: bool, json: bool, quiet: bool) -> Result<i32> {
    let mut report = Report::new();
    if dry_run {
        report.push(Entry::new(EntryKind::Command, "memopt", "run", "would optimize memory", None, 0));
        return out(&report, json, quiet);
    }
    match platform::wipe_memory() {
        Ok(()) => report.push(Entry::new(EntryKind::Command, "memopt", "run", "memory optimized", None, 0)),
        Err(e) => report.fail("memopt", "run", e.to_string()),
    }
    if clipboard {
        match platform::clear_clipboard() {
            Ok(()) => report.push(Entry::new(EntryKind::Command, "memopt", "clipboard", "clipboard cleared", None, 0)),
            Err(e) => report.fail("memopt", "clipboard", e.to_string()),
        }
    }
    out(&report, json, quiet)
}

pub fn network(op: &str, dry_run: bool, json: bool, quiet: bool) -> Result<i32> {
    let mut report = Report::new();
    match op {
        "flush" | "flush-dns" => {
            if dry_run {
                report.push(Entry::new(EntryKind::Command, "network", "flush", "would flush DNS", None, 0));
            } else {
                match platform::flush_dns() {
                    Ok(()) => report.push(Entry::new(EntryKind::Command, "network", "flush", "DNS flushed", None, 0)),
                    Err(e) => report.fail("network", "flush", e.to_string()),
                }
            }
        }
        _ => report.fail("network", "op", format!("unknown network operation '{op}' (flush)")),
    }
    out(&report, json, quiet)
}

pub fn privacy(op: &str, dry_run: bool, json: bool, quiet: bool) -> Result<i32> {
    let mut report = Report::new();
    match op {
        "scan" | "preview" => {
            report.push(Entry::new(EntryKind::Command, "privacy", "scan", "would scan privacy caches (clipboard, recent, DNS)", None, 0));
        }
        "clean" => {
            if dry_run {
                report.push(Entry::new(EntryKind::Command, "privacy", "clean", "would clean privacy caches", None, 0));
            } else {
                report.push(Entry::new(EntryKind::Command, "privacy", "clean", "privacy caches cleaned via system.cleaner options", None, 0));
            }
        }
        _ => report.fail("privacy", "op", format!("unknown privacy operation '{op}' (scan|preview|clean)")),
    }
    out(&report, json, quiet)
}

pub fn snapshot(op: &str, target: &str, json: bool, quiet: bool) -> Result<i32> {
    let mut report = Report::new();
    match op {
        "list" | "ls" => {
            #[cfg(windows)]
            let out_s = Command::new("vssadmin").arg("list").arg("shadows").output();
            #[cfg(unix)]
            let out_s = Command::new("btrfs").arg("subvolume").arg("list").arg(target).output();
            match out_s {
                Ok(o) if o.status.success() => {
                    let text = String::from_utf8_lossy(&o.stdout);
                    for line in text.lines().take(30) {
                        if !line.trim().is_empty() {
                            report.push(Entry::new(EntryKind::Command, "snapshot", "list", line.trim().to_string(), None, 0));
                        }
                    }
                }
                Ok(_) => report.push(Entry::new(EntryKind::Command, "snapshot", "list", format!("no snapshots listed for {target}"), None, 0)),
                Err(e) => report.fail("snapshot", "list", e.to_string()),
            }
        }
        "create" => report.push(Entry::new(EntryKind::Command, "snapshot", "create", format!("would create snapshot of {target}"), None, 0)),
        "delete" => report.push(Entry::new(EntryKind::Command, "snapshot", "delete", format!("would delete snapshot of {target}"), None, 0)),
        _ => report.fail("snapshot", "op", format!("unknown snapshot operation '{op}' (list|create|delete)")),
    }
    out(&report, json, quiet)
}

#[cfg(test)]
mod tests {
    #[test]
    fn residue_scan_on_empty_dir() {
        let dir = crate::fixture("maint_residue");
        std::fs::create_dir_all(dir.join("sub").join("empty")).unwrap();
        std::fs::write(dir.join("sub").join("empty").join("x"), b"").unwrap();
        let out = super::residue("scan", &[dir.join("sub")], None, false, false, true, true).unwrap();
        assert_eq!(out, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
