//! Action execution: route a normalized action to a provider family.

pub mod context;
pub mod file;
pub mod filter;
pub mod provider;
pub mod safety;
pub mod sqlite;
pub mod structured;
pub mod system;

pub use context::RunContext;

use crate::core::report::{Report, Stopwatch};
use crate::definition::model::{ActionDef, SearchKind, VarSet};

pub fn run(
    def: &ActionDef,
    vars: &VarSet,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
) -> Report {
    let watch = Stopwatch::start();

    if !provider::exists(&def.command) {
        let mut report = Report::new();
        report.fail(
            cleaner,
            option,
            format!("unknown action command '{}'", def.command.trim()),
        );
        report.finish(watch);
        return report;
    }

    if !provider::available(&def.command) {
        let mut report = Report::new();
        report.fail(
            cleaner,
            option,
            format!(
                "action '{}' is not available on this platform",
                def.command.trim()
            ),
        );
        report.finish(watch);
        return report;
    }

    if matches!(def.search, SearchKind::Deep) {
        let mut report = Report::new();
        report.finish(watch);
        return report;
    }

    let command = def.command.trim().to_ascii_lowercase();
    let mut report = match command.as_str() {
        "delete" | "shred" | "truncate" => file::run(def, vars, ctx, cleaner, option),
        "ini" | "json" => structured::run(def, vars, ctx, cleaner, option),
        "sqlite.vacuum" | "cookies" | "chrome.history" | "chrome.keywords" | "chrome.autofill"
        | "chrome.favicons" | "chrome.databases_db" | "chrome.cookies" | "mozilla.history"
        | "mozilla.url_history" | "mozilla.databases" | "mozilla.vacuum" => {
            sqlite::run(def, vars, ctx, cleaner, option)
        }
        "process" | "winreg" => system::run(def, vars, ctx, cleaner, option),
        _ if command.starts_with("system.")
            || matches!(
                command.as_str(),
                "apt.clean"
                    | "apt.autoclean"
                    | "apt.autoremove"
                    | "dnf.clean"
                    | "yum.clean"
                    | "pacman.clean"
                    | "zypper.clean"
                    | "brew.cleanup"
            ) =>
        {
            system::run(def, vars, ctx, cleaner, option)
        }
        _ => {
            let mut report = Report::new();
            report.fail(
                cleaner,
                option,
                format!("action '{command}' is not implemented"),
            );
            report
        }
    };
    report.finish(watch);
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(command: &str, path: &str) -> ActionDef {
        ActionDef {
            command: command.to_string(),
            path: path.to_string(),
            ..ActionDef::default()
        }
    }

    #[test]
    fn unknown_providers_are_reported_not_run() {
        let report = run(
            &action("frobnicate", "/tmp/x"),
            &VarSet::new(),
            &RunContext::preview(),
            "test",
            "option",
        );
        assert_eq!(report.errors.len(), 1);
        assert!(report.entries.is_empty());
    }

    #[test]
    fn deep_actions_are_empty_for_now() {
        let mut def = action("delete", "/tmp/x");
        def.search = SearchKind::Deep;
        let report = run(
            &def,
            &VarSet::new(),
            &RunContext::preview(),
            "test",
            "option",
        );
        assert!(report.entries.is_empty());
        assert!(report.errors.is_empty());
    }

    #[test]
    fn known_but_unimplemented_providers_fail_clearly() {
        let report = run(
            &action("system.nix_gc", ""),
            &VarSet::new(),
            &RunContext::preview(),
            "test",
            "option",
        );
        assert!(report
            .errors
            .iter()
            .any(|error| error.message.contains("not available")));
    }

    #[test]
    fn delete_previews_through_the_file_provider() {
        let root = crate::fixture("action_route_delete");
        let file = root.join("cache.tmp");
        std::fs::write(&file, b"x").unwrap();

        let report = run(
            &action("delete", &file.to_string_lossy()),
            &VarSet::new(),
            &RunContext::preview(),
            "test",
            "option",
        );
        assert_eq!(report.entries.len(), 1);
        assert!(file.exists());
    }
}
