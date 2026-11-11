//! The action providers a cleaner definition may name.
//!
//! This registry is intentionally data-only: it answers whether a command is
//! known, whether it needs a path, and whether an untrusted definition may run
//! it. Implementations live in [`crate::action`] and later stages.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderInfo {
    pub name: &'static str,
    pub summary: &'static str,
    pub needs_path: bool,
    pub destructive: bool,
    pub windows_only: bool,
    pub unix_only: bool,
    pub privileged: bool,
}

impl ProviderInfo {
    pub fn available(&self) -> bool {
        if self.windows_only && !cfg!(windows) {
            return false;
        }
        if self.unix_only && cfg!(windows) {
            return false;
        }
        true
    }
}

const fn p(
    name: &'static str,
    needs_path: bool,
    destructive: bool,
    windows_only: bool,
    unix_only: bool,
    privileged: bool,
) -> ProviderInfo {
    ProviderInfo {
        name,
        summary: "",
        needs_path,
        destructive,
        windows_only,
        unix_only,
        privileged,
    }
}

pub static PROVIDERS: &[ProviderInfo] = &[
    p("delete", true, true, false, false, false),
    p("shred", true, true, false, false, false),
    p("truncate", true, false, false, false, false),
    p("ini", true, false, false, false, false),
    p("json", true, false, false, false, false),
    p("sqlite.vacuum", true, false, false, false, false),
    p("chrome.history", true, false, false, false, false),
    p("chrome.keywords", true, false, false, false, false),
    p("chrome.autofill", true, false, false, false, false),
    p("chrome.favicons", true, false, false, false, false),
    p("chrome.databases_db", true, false, false, false, false),
    p("chrome.cookies", true, false, false, false, false),
    p("mozilla.history", true, false, false, false, false),
    p("mozilla.url_history", true, false, false, false, false),
    p("mozilla.databases", true, false, false, false, false),
    p("mozilla.vacuum", true, false, false, false, false),
    p("cookies", true, false, false, false, false),
    p("winreg", true, true, true, false, true),
    p("process", false, true, false, false, true),
    p("system.clipboard", false, false, false, false, false),
    p("system.dns", false, false, false, false, false),
    p("system.memory", false, false, false, true, false),
    p("system.trash", false, true, false, false, false),
    p("system.rotated_logs", false, true, false, true, false),
    p("system.localizations", false, true, false, true, false),
    p("system.recent_documents", false, true, false, false, false),
    p("system.custom", false, true, false, false, false),
    p("system.tmp", false, true, false, false, false),
    p("system.free_disk_space", true, true, false, false, false),
    p("system.journald", false, true, false, true, false),
    p("system.font_cache", false, false, false, true, false),
    p("system.recycle_bin", false, true, true, false, false),
    p("system.prefetch", false, true, true, false, false),
    p("system.wer", false, true, true, false, false),
    p("system.windows_update_cache", false, true, true, false, false),
    p("system.delivery_optimization", false, true, true, false, false),
    p("system.explorer_caches", false, true, true, false, false),
    p("system.thumbcache", false, true, true, false, false),
    p("system.store_cache", false, true, true, false, false),
    p("system.d3d_shader_cache", false, true, true, false, false),
    p("system.crash_dumps", false, true, true, false, false),
    p("system.memory_dumps", false, true, true, false, false),
    p("system.trim", true, false, false, false, false),
    p("system.nix_gc", false, true, false, true, false),
    p("system.tm_thin", false, true, false, true, false),
    p("system.docker_volumes", false, true, false, true, false),
    p("system.journal_user", false, true, false, true, false),
    p("apt.clean", false, true, false, true, false),
    p("apt.autoclean", false, true, false, true, false),
    p("apt.autoremove", false, true, false, true, true),
    p("dnf.clean", false, true, false, true, false),
    p("yum.clean", false, true, false, true, false),
    p("pacman.clean", false, true, false, true, false),
    p("zypper.clean", false, true, false, true, false),
    p("system.apt_cache", false, true, false, true, false),
    p("system.dnf_cache", false, true, false, true, false),
    p("system.pacman_cache", false, true, false, true, false),
    p("system.old_kernels", false, true, false, true, false),
    p("system.journal_vacuum", false, true, false, true, false),
    p("system.user_caches", false, true, false, true, false),
    p("system.temp_deep", false, true, false, true, false),
    p("system.orphans", false, true, false, true, false),
    p("system.snap", false, true, false, true, false),
    p("system.flatpak", false, true, false, true, false),
    p("system.docker", false, true, false, true, false),
    p("system.kde_cache", false, true, false, true, false),
    p("system.kde_pim", false, true, false, true, false),
    p("system.baloo", false, true, false, true, false),
    p("system.gnome_cache", false, true, false, true, false),
    p("system.zeitgeist", false, true, false, true, false),
    p("system.tracker", false, true, false, true, false),
    p("system.apt_lists", false, true, false, true, false),
    p("system.zypp_cache", false, true, false, true, false),
    p("system.crash_reports", false, true, false, true, false),
    p("system.coredumps", false, true, false, true, false),
    p("system.dev_npm", false, true, false, true, false),
    p("system.dev_pip", false, true, false, true, false),
    p("system.dev_cargo", false, true, false, true, false),
    p("system.dev_gradle", false, true, false, true, false),
    p("system.dev_go", false, true, false, true, false),
    p("brew.cleanup", false, true, false, true, false),
];

/// Commands whose path is not a filesystem path.
const NON_FILESYSTEM_PATH: &[&str] = &["winreg", "process"];

pub fn lookup(command: &str) -> Option<&'static ProviderInfo> {
    let command = command.trim();
    PROVIDERS
        .iter()
        .find(|provider| provider.name.eq_ignore_ascii_case(command))
}

pub fn exists(command: &str) -> bool {
    lookup(command).is_some()
}

pub fn list() -> &'static [ProviderInfo] {
    PROVIDERS
}

pub fn available(command: &str) -> bool {
    lookup(command).is_some_and(ProviderInfo::available)
}

pub fn needs_path(command: &str) -> bool {
    lookup(command).is_some_and(|provider| provider.needs_path)
}

/// Unknown commands are treated as privileged so an untrusted definition
/// cannot reach an implementation added later.
pub fn is_privileged(command: &str) -> bool {
    lookup(command).is_none_or(|provider| provider.privileged)
}

/// Whether the action's `path` must be a path on the local filesystem.
/// Unknown commands stay strict (true) for the same fail-closed reason.
pub fn filesystem_path(command: &str) -> bool {
    lookup(command)
        .is_none_or(|provider| !NON_FILESYSTEM_PATH.contains(&provider.name))
}

pub fn validate_action(command: &str, path: &str) -> std::result::Result<(), String> {
    let Some(provider) = lookup(command) else {
        return Err(format!("unknown action command '{}'", command.trim()));
    };
    if provider.needs_path && path.trim().is_empty() {
        return Err(format!("action '{}' requires a path", provider.name));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_commands_case_insensitively() {
        assert!(exists("delete"));
        assert!(exists("  DELETE  "));
        assert!(!exists("frobnicate"));
        assert_eq!(lookup("winreg").unwrap().name, "winreg");
    }

    #[test]
    fn registry_has_no_duplicates() {
        let mut names: Vec<&str> = PROVIDERS.iter().map(|p| p.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len());
    }

    #[test]
    fn only_registry_and_process_skip_filesystem_paths() {
        assert!(!filesystem_path("winreg"));
        assert!(!filesystem_path("process"));
        assert!(filesystem_path("delete"));
        assert!(filesystem_path("unknown-command"));
    }

    #[test]
    fn privileged_and_unknown_are_blocked_for_untrusted() {
        assert!(is_privileged("winreg"));
        assert!(is_privileged("process"));
        assert!(is_privileged("apt.autoremove"));
        assert!(!is_privileged("delete"));
        assert!(is_privileged("unknown-command"));
    }

    #[test]
    fn path_requirement_is_enforced() {
        assert!(validate_action("delete", "/tmp/x").is_ok());
        assert!(validate_action("delete", " ").is_err());
        assert!(validate_action("system.tmp", "").is_ok());
        assert!(validate_action("unknown-command", "/x").is_err());
    }

    #[test]
    fn platform_only_providers_report_availability() {
        assert!(available("delete"));
        if cfg!(windows) {
            assert!(available("winreg"));
            assert!(!available("system.journald"));
        } else {
            assert!(!available("winreg"));
            assert!(available("system.journald"));
        }
    }
}
