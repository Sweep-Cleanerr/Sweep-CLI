//! Selection and execution of cleaner options.
//!
//! This is the trimmed engine core: it resolves user selectors against the
//! registry and runs the matching options. Deep scans, progress reporting and
//! the undo/backup journal are separate stages.

use std::collections::HashSet;

use globset::GlobBuilder;
use rayon::prelude::*;

use crate::action;
use crate::core::report::{Report, Stopwatch};
use crate::core::{Error, Result};
use crate::definition::{CleanerRegistry, SearchKind};

/// One cleaner option selected by the user.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SelectedOption {
    pub cleaner: String,
    pub option: String,
}

impl SelectedOption {
    pub fn new(cleaner: impl Into<String>, option: impl Into<String>) -> Self {
        Self {
            cleaner: cleaner.into(),
            option: option.into(),
        }
    }
}

/// Resolves selector expressions into concrete cleaner options.
///
/// Accepted forms:
/// - `*` / `all`: every active option;
/// - `cleaner`: every active option of that cleaner;
/// - `cleaner.option`: one exact option;
/// - globs in either part (`demo.*`, `*.cache`).
///
/// `exclude` uses the same grammar. Exclusions that match nothing are ignored.
pub fn resolve(
    registry: &CleanerRegistry,
    selectors: &[String],
    all: bool,
    exclude: &[String],
) -> Result<Vec<SelectedOption>> {
    let mut selected = Vec::new();
    let mut seen = HashSet::new();

    if all {
        for loaded in registry.all() {
            for option in loaded.def.active_options() {
                push_unique(&mut selected, &mut seen, &loaded.def.id, &option.id);
            }
        }
    } else {
        for raw in selectors {
            let selector = raw.trim();
            if selector.is_empty() {
                continue;
            }
            if selector == "*" || selector.eq_ignore_ascii_case("all") {
                for loaded in registry.all() {
                    for option in loaded.def.active_options() {
                        push_unique(&mut selected, &mut seen, &loaded.def.id, &option.id);
                    }
                }
                continue;
            }

            let Some((cleaner_part, option_part)) = selector.split_once('.') else {
                let cleaners = registry.matching(selector);
                if cleaners.is_empty() {
                    return Err(Error::UnknownCleaner(selector.to_string()));
                }
                for loaded in cleaners {
                    for option in loaded.def.active_options() {
                        push_unique(&mut selected, &mut seen, &loaded.def.id, &option.id);
                    }
                }
                continue;
            };

            let cleaners = registry.matching(cleaner_part);
            if cleaners.is_empty() {
                return Err(Error::UnknownCleaner(cleaner_part.to_string()));
            }

            let wildcard = has_glob(cleaner_part) || has_glob(option_part);
            let mut matched = false;
            for loaded in cleaners {
                for option in loaded.def.active_options() {
                    if option_matches(option_part, &option.id) {
                        push_unique(&mut selected, &mut seen, &loaded.def.id, &option.id);
                        matched = true;
                    }
                }
            }
            if !matched && !wildcard {
                return Err(Error::UnknownOption {
                    cleaner: cleaner_part.to_string(),
                    option: option_part.to_string(),
                });
            }
        }
    }

    if !exclude.is_empty() {
        selected.retain(|item| !is_excluded(item, exclude));
    }

    if selected.is_empty() {
        return Err(Error::msg("nothing selected"));
    }
    Ok(selected)
}

/// Runs every selected option in parallel and merges the reports.
pub fn run(
    registry: &CleanerRegistry,
    selection: &[SelectedOption],
    ctx: &crate::action::RunContext,
    force: bool,
) -> Report {
    let watch = Stopwatch::start();
    if let Some(progress) = &ctx.progress {
        progress.set_total(selection.len());
    }
    let reports: Vec<Report> = selection
        .par_iter()
        .map(|item| run_one(registry, item, ctx, force))
        .collect();

    let mut report = Report::new();
    for part in reports {
        report.merge(part);
    }
    report.finish(watch);
    report
}

fn run_one(
    registry: &CleanerRegistry,
    item: &SelectedOption,
    ctx: &crate::action::RunContext,
    force: bool,
) -> Report {
    let mut report = Report::new();

    let Some(loaded) = registry.get(&item.cleaner) else {
        report.fail(&item.cleaner, &item.option, "cleaner not found");
        return report;
    };

    if !force {
        if let Some(reason) = loaded.def.running_reason() {
            report.block(
                &item.cleaner,
                &item.option,
                format!("skipped: {reason} (use --force to override)"),
            );
            return report;
        }
    }

    let Some(option) = loaded.def.option(&item.option) else {
        report.fail(&item.cleaner, &item.option, "option not found");
        return report;
    };
    if !option.os.matches() {
        report.fail(
            &item.cleaner,
            &item.option,
            "option is not available on this system",
        );
        return report;
    }

    let vars = loaded.def.resolve_vars();
    let mut saw_deep = false;
    for action in &option.actions {
        if !action.os.matches() {
            continue;
        }
        if matches!(action.search, SearchKind::Deep) {
            saw_deep = true;
            continue;
        }
        report.merge(action::run(
            action,
            &vars,
            ctx,
            &item.cleaner,
            &item.option,
        ));
    }

    if saw_deep {
        report.warn(
            &item.cleaner,
            &item.option,
            "deep scan is not implemented; skipped",
        );
    }

    // Progress goes to stderr so `--json` still emits one clean document.
    if let Some(progress) = &ctx.progress {
        progress.inc();
        eprintln!("{}", progress.json_line(&item.cleaner, &item.option));
    }

    report
}

fn push_unique(
    out: &mut Vec<SelectedOption>,
    seen: &mut HashSet<(String, String)>,
    cleaner: &str,
    option: &str,
) {
    if seen.insert((cleaner.to_string(), option.to_string())) {
        out.push(SelectedOption::new(cleaner, option));
    }
}

fn is_excluded(item: &SelectedOption, patterns: &[String]) -> bool {
    patterns.iter().any(|raw| {
        let pattern = raw.trim();
        if pattern.is_empty() {
            return false;
        }
        if pattern == "*" || pattern.eq_ignore_ascii_case("all") {
            return true;
        }
        if pattern.contains('.') {
            return glob_match(pattern, &format!("{}.{}", item.cleaner, item.option));
        }
        glob_match(pattern, &item.cleaner) || glob_match(pattern, &item.option)
    })
}

fn option_matches(pattern: &str, id: &str) -> bool {
    if has_glob(pattern) {
        glob_match(pattern, id)
    } else {
        pattern == id
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    match GlobBuilder::new(pattern)
        .case_insensitive(true)
        .literal_separator(true)
        .build()
    {
        Ok(glob) => glob.compile_matcher().is_match(text),
        Err(_) => pattern.eq_ignore_ascii_case(text),
    }
}

fn has_glob(text: &str) -> bool {
    text.contains(['*', '?', '[', ']', '{'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::native;
    use crate::definition::Trust;

    fn option_cleaner(id: &str, options: &[(&str, &str)]) -> crate::definition::CleanerDef {
        let mut text = format!("id = \"{id}\"\nname = \"{id}\"\n");
        for (option, path) in options {
            text.push_str(&format!(
                r#"
[[option]]
id = "{option}"
label = "{option}"

[[option.action]]
command = "delete"
path = '{path}'
"#
            ));
        }
        native::parse_native(&text, &format!("{id}.toml")).unwrap()
    }

    fn demo_registry() -> CleanerRegistry {
        let mut registry = CleanerRegistry::new();
        registry.register(
            option_cleaner(
                "demo",
                &[
                    ("cache", "/nonexistent/demo-cache"),
                    ("logs", "/nonexistent/demo-logs"),
                ],
            ),
            Trust::Trusted,
            "test",
        );
        registry.register(
            option_cleaner("other", &[("cache", "/nonexistent/other-cache")]),
            Trust::Trusted,
            "test",
        );
        registry
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn pairs(items: &[SelectedOption]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|item| (item.cleaner.clone(), item.option.clone()))
            .collect()
    }

    #[test]
    fn bare_cleaner_expands_all_active_options() {
        let registry = demo_registry();
        let got = resolve(&registry, &strings(&["demo"]), false, &[]).unwrap();
        assert_eq!(
            pairs(&got),
            vec![
                ("demo".to_string(), "cache".to_string()),
                ("demo".to_string(), "logs".to_string()),
            ]
        );
    }

    #[test]
    fn qualified_selector_is_case_insensitive_for_the_cleaner() {
        let registry = demo_registry();
        let got = resolve(&registry, &strings(&["DEMO.logs"]), false, &[]).unwrap();
        assert_eq!(got, vec![SelectedOption::new("demo", "logs")]);
    }

    #[test]
    fn star_and_all_expand_everything() {
        let registry = demo_registry();
        let star = resolve(&registry, &strings(&["*"]), false, &[]).unwrap();
        let all = resolve(&registry, &[], true, &[]).unwrap();
        assert_eq!(star.len(), 3);
        assert_eq!(pairs(&star), pairs(&all));
    }

    #[test]
    fn globs_work_in_both_parts() {
        let registry = demo_registry();
        assert_eq!(
            resolve(&registry, &strings(&["dem*"]), false, &[])
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            resolve(&registry, &strings(&["demo.*"]), false, &[])
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            resolve(&registry, &strings(&["*.cache"]), false, &[])
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn exclusions_accept_cleaner_and_option_forms() {
        let registry = demo_registry();
        let selected = resolve(&registry, &strings(&["*"]), false, &strings(&["demo.logs"]))
            .unwrap();
        assert_eq!(selected.len(), 2);

        let selected = resolve(&registry, &strings(&["*"]), false, &strings(&["logs"])).unwrap();
        assert_eq!(selected.len(), 2);

        let selected = resolve(&registry, &strings(&["*"]), false, &strings(&["demo"])).unwrap();
        assert_eq!(selected, vec![SelectedOption::new("other", "cache")]);

        let selected = resolve(&registry, &strings(&["*"]), false, &strings(&["dem*"])).unwrap();
        assert_eq!(selected, vec![SelectedOption::new("other", "cache")]);

        let selected = resolve(&registry, &strings(&["*"]), false, &strings(&["*.cache"]))
            .unwrap();
        assert_eq!(selected, vec![SelectedOption::new("demo", "logs")]);
    }

    #[test]
    fn duplicate_selectors_are_deduplicated_in_order() {
        let registry = demo_registry();
        let got = resolve(
            &registry,
            &strings(&["demo", "demo.cache", "demo"]),
            false,
            &[],
        )
        .unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], SelectedOption::new("demo", "cache"));
    }

    #[test]
    fn unknown_selectors_fail() {
        let registry = demo_registry();
        assert!(matches!(
            resolve(&registry, &strings(&["missing"]), false, &[]),
            Err(Error::UnknownCleaner(_))
        ));
        assert!(matches!(
            resolve(&registry, &strings(&["demo.missing"]), false, &[]),
            Err(Error::UnknownOption { .. })
        ));
    }

    #[test]
    fn nothing_selected_is_an_error() {
        let registry = demo_registry();
        assert_eq!(
            resolve(&registry, &[], false, &[]).unwrap_err().to_string(),
            "nothing selected"
        );
        assert_eq!(
            resolve(&registry, &strings(&["*"]), false, &strings(&["*"]))
                .unwrap_err()
                .to_string(),
            "nothing selected"
        );
    }

    #[test]
    fn running_cleaner_is_blocked_unless_forced_and_preview_does_not_touch_files() {
        let dir = crate::fixture("worker_run");
        let file = dir.join("junk.tmp");
        std::fs::write(&file, b"junk").unwrap();
        let path = file.display().to_string();

        let text = format!(
            r#"
id = "demo"
name = "Demo"

[[running]]
type = "path"
value = '{path}'

[[option]]
id = "junk"
label = "Junk"

[[option.action]]
command = "delete"
path = '{path}'
"#
        );
        let def = native::parse_native(&text, "demo.toml").unwrap();
        let mut registry = CleanerRegistry::new();
        registry.register(def, Trust::Trusted, "test");

        let selection = vec![SelectedOption::new("demo", "junk")];
        let ctx = crate::action::RunContext::preview();

        let blocked = run(&registry, &selection, &ctx, false);
        assert_eq!(blocked.blocked_count(), 1);
        assert!(blocked.entries.is_empty());
        assert!(file.exists());

        let preview = run(&registry, &selection, &ctx, true);
        assert_eq!(preview.entries.len(), 1);
        assert!(file.exists());

        let cleaned = run(&registry, &selection, &crate::action::RunContext::clean(), true);
        assert_eq!(cleaned.entries.len(), 1);
        assert!(!file.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deep_actions_are_skipped_with_a_warning() {
        let text = r#"
id = "demo"
name = "Demo"

[[option]]
id = "deep"
label = "Deep"

[[option.action]]
command = "delete"
search = "deep"
path = '/'
"#;
        let def = native::parse_native(text, "demo.toml").unwrap();
        let mut registry = CleanerRegistry::new();
        registry.register(def, Trust::Trusted, "test");

        let selection = vec![SelectedOption::new("demo", "deep")];
        let report = run(
            &registry,
            &selection,
            &crate::action::RunContext::preview(),
            true,
        );
        assert!(report.entries.is_empty());
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].message.contains("deep scan"));
    }
}
