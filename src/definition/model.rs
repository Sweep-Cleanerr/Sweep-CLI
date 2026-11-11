//! Normalized cleaner definitions shared by every definition format.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

use crate::action::provider;
use crate::core::error::{Error, Result};
use crate::core::path;
use crate::definition::native;
use crate::platform;

/// Upper bound for the number of strings one `$$variable$$` expansion may produce.
const MAX_EXPANSIONS: usize = 4096;

fn normalize_command(command: &str) -> String {
    command.trim().to_ascii_lowercase()
}

/// An optional OS restriction. Empty means "every OS".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OsFilter(pub Option<String>);

impl OsFilter {
    pub fn any() -> Self {
        Self(None)
    }

    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty() {
            Self(None)
        } else {
            Self(Some(value))
        }
    }

    pub fn matches(&self) -> bool {
        platform::os_matches(self.0.as_deref().unwrap_or(""))
    }

    pub fn as_str(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

/// A condition that hides a cleaner while the matching program/path exists.
#[derive(Debug, Clone)]
pub enum RunningCheck {
    Exe {
        name: String,
        same_user: bool,
        os: OsFilter,
    },
    Path {
        pattern: String,
        os: OsFilter,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VarSearch {
    #[default]
    Literal,
    Glob,
    Registry,
}

#[derive(Debug, Clone)]
pub struct VarValue {
    pub raw: String,
    pub search: VarSearch,
    pub os: OsFilter,
    pub reg_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VarDef {
    pub name: String,
    pub values: Vec<VarValue>,
}

// ---------------------------------------------------------------------------
// `$$variable$$` tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum Segment<'a> {
    Literal(&'a str),
    Var(&'a str),
}

fn is_var_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn segments(input: &str) -> Vec<Segment<'_>> {
    let mut out = Vec::new();
    let mut literal_start = 0;
    let mut cursor = 0;

    while let Some(rel) = input[cursor..].find("$$") {
        let start = cursor + rel;
        let name_start = start + 2;
        let Some(rel_end) = input[name_start..].find("$$") else {
            break;
        };
        let name_end = name_start + rel_end;
        let name = &input[name_start..name_end];

        if is_var_name(name) {
            if literal_start < start {
                out.push(Segment::Literal(&input[literal_start..start]));
            }
            out.push(Segment::Var(name));
            cursor = name_end + 2;
            literal_start = cursor;
        } else {
            cursor = start + 1;
        }
    }

    if literal_start < input.len() {
        out.push(Segment::Literal(&input[literal_start..]));
    }
    out
}

fn placeholders(input: &str) -> Vec<&str> {
    segments(input)
        .into_iter()
        .filter_map(|segment| match segment {
            Segment::Var(name) => Some(name),
            Segment::Literal(_) => None,
        })
        .collect()
}

fn has_placeholder(input: &str) -> bool {
    input.contains("$$") && !placeholders(input).is_empty()
}

/// Resolved `$$variable$$` values for one cleaner.
#[derive(Debug, Clone, Default)]
pub struct VarSet {
    map: BTreeMap<String, Vec<String>>,
}

impl VarSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: impl Into<String>, values: Vec<String>) {
        let name = name.into();
        if name.is_empty() || values.is_empty() {
            return;
        }

        let entry = self.map.entry(name).or_default();
        // A newer definition of the same variable wins its own values first.
        let mut merged: Vec<String> = Vec::with_capacity(values.len() + entry.len());
        for value in values.into_iter().chain(entry.drain(..)) {
            if !merged.contains(&value) {
                merged.push(value);
            }
        }
        *entry = merged;
    }

    pub fn get(&self, name: &str) -> Option<&[String]> {
        self.map.get(name).map(Vec::as_slice)
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Expands every combination of the referenced variables.
    ///
    /// Placeholders are exact: `$$a_b$$` never resolves through `$$a$$`, and
    /// substituted values are never expanded a second time. Unknown
    /// placeholders are left untouched.
    pub fn expand(&self, input: &str) -> Vec<String> {
        if self.map.is_empty() || !input.contains("$$") {
            return vec![input.to_string()];
        }

        let parts = segments(input);
        let mut names: Vec<&str> = Vec::new();
        for part in &parts {
            if let Segment::Var(name) = part {
                if self.map.contains_key(*name) && !names.contains(name) {
                    names.push(*name);
                }
            }
        }
        if names.is_empty() {
            return vec![input.to_string()];
        }

        let mut combos: Vec<Vec<&str>> = vec![Vec::new()];
        for name in &names {
            let values = &self.map[*name];
            let mut next: Vec<Vec<&str>> =
                Vec::with_capacity((combos.len() * values.len()).min(MAX_EXPANSIONS));
            'outer: for combo in &combos {
                for value in values {
                    if next.len() >= MAX_EXPANSIONS {
                        log::warn!(
                            "variable expansion of '{input}' truncated at {MAX_EXPANSIONS} results"
                        );
                        break 'outer;
                    }
                    let mut extended = combo.clone();
                    extended.push(value.as_str());
                    next.push(extended);
                }
            }
            combos = next;
        }

        combos
            .iter()
            .map(|combo| {
                let mut out = String::with_capacity(input.len());
                for part in &parts {
                    match part {
                        Segment::Literal(text) => out.push_str(text),
                        Segment::Var(name) => match names.iter().position(|n| n == name) {
                            Some(idx) => out.push_str(combo[idx]),
                            None => {
                                out.push_str("$$");
                                out.push_str(name);
                                out.push_str("$$");
                            }
                        },
                    }
                }
                out
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchKind {
    #[default]
    File,
    Glob,
    WalkAll,
    WalkFiles,
    WalkTop,
    Deep,
}

impl SearchKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "file" => Some(Self::File),
            "glob" => Some(Self::Glob),
            "walk.all" => Some(Self::WalkAll),
            "walk.files" => Some(Self::WalkFiles),
            "walk.top" => Some(Self::WalkTop),
            "deep" => Some(Self::Deep),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Glob => "glob",
            Self::WalkAll => "walk.all",
            Self::WalkFiles => "walk.files",
            Self::WalkTop => "walk.top",
            Self::Deep => "deep",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    File,
    Dir,
}

#[derive(Debug, Clone)]
pub struct ActionDef {
    pub command: String,
    pub search: SearchKind,
    pub path: String,
    pub os: OsFilter,
    pub regex: Option<String>,
    pub nregex: Option<String>,
    pub wholeregex: Option<String>,
    pub nwholeregex: Option<String>,
    pub object_type: Option<ObjectType>,
    pub max_age_days: Option<u32>,
    pub min_age_days: Option<u32>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub max_depth: Option<usize>,
    pub include_dirs: Option<bool>,
    pub section: Option<String>,
    pub parameter: Option<String>,
    pub address: Option<String>,
    pub cmd: Option<String>,
    pub wait: bool,
    pub reg_name: Option<String>,
    pub exclude_keys: Vec<String>,
}

impl Default for ActionDef {
    fn default() -> Self {
        Self {
            command: "delete".to_string(),
            search: SearchKind::File,
            path: String::new(),
            os: OsFilter::any(),
            regex: None,
            nregex: None,
            wholeregex: None,
            nwholeregex: None,
            object_type: None,
            max_age_days: None,
            min_age_days: None,
            min_size: None,
            max_size: None,
            max_depth: None,
            include_dirs: None,
            section: None,
            parameter: None,
            address: None,
            cmd: None,
            wait: true,
            reg_name: None,
            exclude_keys: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OptionDef {
    pub id: String,
    pub label: String,
    pub description: String,
    pub warning: String,
    pub actions: Vec<ActionDef>,
    pub os: OsFilter,
}

#[derive(Debug, Clone)]
pub struct CleanerDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub os: OsFilter,
    pub running: Vec<RunningCheck>,
    pub vars: Vec<VarDef>,
    pub options: Vec<OptionDef>,
}

fn check_os_filter(issues: &mut Vec<String>, os: &OsFilter, what: &str) {
    if let Some(filter) = os.as_str() {
        if !platform::known_os_filter(filter) {
            issues.push(format!("{what} has unknown os filter '{filter}'"));
        }
    }
}

fn has_glob_chars(input: &str) -> bool {
    input
        .chars()
        .any(|ch| matches!(ch, '*' | '?' | '[' | ']' | '{'))
}

/// True when the (possibly globbed) path exists. Globs are only supported in
/// the final path component.
fn path_pattern_exists(pattern: &str) -> bool {
    let expanded = path::expand(pattern);
    if expanded.as_os_str().is_empty() {
        return false;
    }
    if !has_glob_chars(&expanded.to_string_lossy()) {
        return expanded.exists();
    }

    let Some(parent) = expanded.parent() else {
        return false;
    };
    let Some(file_name) = expanded
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return false;
    };
    if has_glob_chars(&parent.to_string_lossy()) {
        log::debug!("running check '{pattern}': globs outside the last component are unsupported");
        return false;
    }
    let Ok(glob) = globset::Glob::new(&file_name) else {
        return false;
    };
    let matcher = glob.compile_matcher();
    let Ok(entries) = std::fs::read_dir(parent) else {
        return false;
    };
    entries
        .filter_map(|entry| entry.ok())
        .any(|entry| matcher.is_match(entry.file_name()))
}

impl CleanerDef {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            os: OsFilter::any(),
            running: Vec::new(),
            vars: Vec::new(),
            options: Vec::new(),
        }
    }

    /// Resolves literal, glob and registry values.
    pub fn resolve_vars(&self) -> VarSet {
        let mut set = VarSet::new();
        for def in &self.vars {
            let mut values = Vec::new();
            for value in &def.values {
                if !value.os.matches() {
                    continue;
                }
                match value.search {
                    VarSearch::Literal => values.push(value.raw.clone()),
                    VarSearch::Glob => {
                        // Glob patterns often start with $LOCALAPPDATA or ~.
                        let pattern = path::expand(&value.raw);
                        let pattern = pattern.to_string_lossy();
                        if let Ok(dirs) = crate::fsutil::walk::glob_dirs(&pattern) {
                            for dir in dirs {
                                if dir.is_dir() {
                                    let text = dir.to_string_lossy().into_owned();
                                    if !values.contains(&text) {
                                        values.push(text);
                                    }
                                }
                            }
                        }
                    }
                    VarSearch::Registry => {
                        if let Some(text) = platform::reg_read_string(
                            &value.raw,
                            value.reg_name.as_deref(),
                        ) {
                            if !values.contains(&text) {
                                values.push(text);
                            }
                        }
                    }
                }
            }
            set.insert(&def.name, values);
        }
        set
    }

    pub fn active_options(&self) -> Vec<&OptionDef> {
        self.options.iter().filter(|o| o.os.matches()).collect()
    }

    pub fn option(&self, id: &str) -> Option<&OptionDef> {
        self.options.iter().find(|o| o.id == id)
    }

    pub fn is_running(&self) -> bool {
        self.running_reason().is_some()
    }

    /// Returns why the cleaner should be hidden, if it is running right now.
    pub fn running_reason(&self) -> Option<String> {
        for check in &self.running {
            match check {
                RunningCheck::Path { pattern, os } => {
                    if os.matches() && path_pattern_exists(pattern) {
                        return Some(format!("path '{pattern}' exists"));
                    }
                }
                RunningCheck::Exe { name, same_user, os } => {
                    if os.matches() && platform::is_process_running(name) {
                        return Some(format!(
                            "process '{name}' is running{}",
                            if *same_user { " (same user)" } else { "" }
                        ));
                    }
                }
            }
        }
        None
    }

    /// Structural validation. Command-provider validation is added in stage 2.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        let mut option_ids = HashSet::new();
        let cleaner = format!("cleaner '{}'", self.id);

        if self.id.trim().is_empty() {
            issues.push("cleaner has no id".to_string());
        }
        if self.name.trim().is_empty() {
            issues.push(format!("{cleaner} has no name"));
        }
        check_os_filter(&mut issues, &self.os, &cleaner);

        for check in &self.running {
            let (os, label) = match check {
                RunningCheck::Exe { name, os, .. } => (os, name),
                RunningCheck::Path { pattern, os } => (os, pattern),
            };
            if label.trim().is_empty() {
                issues.push(format!("{cleaner} has a running check without a value"));
            }
            check_os_filter(&mut issues, os, &format!("{cleaner} running check"));
        }

        let mut var_names = HashSet::new();
        for var in &self.vars {
            if var.name.trim().is_empty() {
                issues.push(format!("{cleaner} has a variable without a name"));
                continue;
            }
            if !is_var_name(&var.name) {
                issues.push(format!(
                    "{cleaner} variable '{}' has an invalid name (use letters, digits, '_', '-' or '.')",
                    var.name
                ));
            }
            var_names.insert(var.name.as_str());
            if var.values.is_empty() {
                issues.push(format!("{cleaner} variable '{}' has no values", var.name));
            }
            for value in &var.values {
                check_os_filter(
                    &mut issues,
                    &value.os,
                    &format!("{cleaner} variable '{}'", var.name),
                );
            }
        }

        for option in &self.options {
            if option.id.trim().is_empty() {
                issues.push(format!("{cleaner} has an option without an id"));
            } else if !option_ids.insert(option.id.as_str()) {
                issues.push(format!("{cleaner} has duplicate option '{}'", option.id));
            }
            if option.label.trim().is_empty() {
                issues.push(format!("{cleaner} option '{}' has no label", option.id));
            }
            if option.actions.is_empty() {
                issues.push(format!("{cleaner} option '{}' has no actions", option.id));
            }
            let option_label = format!("{cleaner} option '{}'", option.id);
            check_os_filter(&mut issues, &option.os, &option_label);

            for action in &option.actions {
                let command = normalize_command(&action.command);
                if command.is_empty() {
                    issues.push(format!("{option_label} has an action without a command"));
                } else if !provider::exists(&command) {
                    issues.push(format!(
                        "{option_label} uses unknown action command '{}'",
                        action.command.trim()
                    ));
                } else if provider::needs_path(&command) && action.path.trim().is_empty() {
                    issues.push(format!(
                        "{option_label} has a '{command}' action without a path"
                    ));
                }
                check_os_filter(
                    &mut issues,
                    &action.os,
                    &format!("{option_label} action"),
                );

                if let (Some(min), Some(max)) = (action.min_age_days, action.max_age_days) {
                    if min > max {
                        issues.push(format!(
                            "{option_label} has min_age_days ({min}) greater than max_age_days ({max})"
                        ));
                    }
                }
                if let (Some(min), Some(max)) = (action.min_size, action.max_size) {
                    if min > max {
                        issues.push(format!(
                            "{option_label} has min_size ({min}) greater than max_size ({max})"
                        ));
                    }
                }

                let mut reported = HashSet::new();
                for name in placeholders(&action.path) {
                    if !var_names.contains(name) && reported.insert(name) {
                        issues.push(format!(
                            "{option_label} references undeclared variable '$${name}$$'"
                        ));
                    }
                }
            }
        }

        issues
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Trust {
    Trusted,
    #[default]
    Untrusted,
}

impl Trust {
    pub fn allows_process(self) -> bool {
        matches!(self, Self::Trusted)
    }

    pub fn allows_registry(self) -> bool {
        matches!(self, Self::Trusted)
    }

    /// Unknown commands fail closed for untrusted definitions.
    pub fn allows_command(self, command: &str) -> bool {
        match self {
            Self::Trusted => true,
            Self::Untrusted => !provider::is_privileged(command),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadedCleaner {
    pub def: CleanerDef,
    pub trust: Trust,
    pub source: String,
}

impl LoadedCleaner {
    pub fn new(def: CleanerDef, trust: Trust, source: impl Into<String>) -> Self {
        Self {
            def,
            trust,
            source: source.into(),
        }
    }
}

/// Removes everything an untrusted definition must not run. Returns the
/// cleaned definition and a description of every dropped item.
pub fn apply_trust(mut cleaner: CleanerDef, trust: Trust) -> (CleanerDef, Vec<String>) {
    if trust == Trust::Trusted {
        return (cleaner, Vec::new());
    }

    let mut dropped = Vec::new();

    if !trust.allows_registry() {
        for var in &mut cleaner.vars {
            let before = var.values.len();
            var.values.retain(|value| value.search != VarSearch::Registry);
            if var.values.len() != before {
                dropped.push(format!("registry variable '{}'", var.name));
            }
        }
    }

    let mut options = Vec::with_capacity(cleaner.options.len());
    for mut option in cleaner.options.drain(..) {
        let before = option.actions.len();
        option.actions.retain(|action| {
            if trust.allows_command(&action.command) {
                true
            } else {
                dropped.push(action.command.clone());
                false
            }
        });
        // An option that lost all of its actions is useless and misleading.
        if option.actions.len() != before && option.actions.is_empty() {
            continue;
        }
        options.push(option);
    }
    cleaner.options = options;

    (cleaner, dropped)
}

fn rooted_here(path: &Path) -> bool {
    if path.is_absolute() {
        return true;
    }
    #[cfg(windows)]
    {
        let text = path.as_os_str().to_string_lossy();
        text.starts_with('/') || text.starts_with('\\')
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn has_parent_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir))
}

/// Expands variables and the environment, dropping paths outside the filesystem
/// root for commands that must stay local. Unresolved placeholders, `..`
/// components and the filesystem root itself are never returned for local commands.
pub fn expand_action_paths(action: &ActionDef, vars: &VarSet) -> Vec<PathBuf> {
    let command = normalize_command(&action.command);
    let strict = provider::filesystem_path(&command);
    let mut paths = Vec::new();

    for candidate in vars.expand(&action.path) {
        if has_placeholder(&candidate) {
            log::debug!(
                "cleaner action '{}': dropping unresolved path '{}'",
                action.command,
                candidate
            );
            continue;
        }

        let expanded = path::expand(&candidate);
        if expanded.as_os_str().is_empty() {
            if candidate.is_empty() {
                // Commands without a path (for example package-manager actions).
                paths.push(expanded);
            } else {
                log::debug!(
                    "cleaner action '{}': path '{}' expanded to nothing",
                    action.command,
                    candidate
                );
            }
            continue;
        }

        if !strict {
            paths.push(expanded);
            continue;
        }

        if !rooted_here(&expanded) {
            log::warn!(
                "cleaner action '{}': dropping relative path '{}'",
                action.command,
                candidate
            );
        } else if has_parent_component(&expanded) {
            log::warn!(
                "cleaner action '{}': dropping path with '..' component '{}'",
                action.command,
                candidate
            );
        } else if expanded.parent().is_none() {
            log::warn!(
                "cleaner action '{}': dropping filesystem root path '{}'",
                action.command,
                candidate
            );
        } else {
            paths.push(expanded);
        }
    }

    paths
}

/// Reads and parses one definition file. Trust is recorded but NOT applied
/// here: every definition must pass through `CleanerRegistry::register`, which
/// is the single place that validates and applies the trust policy.
pub fn parse_file(file: &Path, trust: Trust) -> Result<LoadedCleaner> {
    let text = std::fs::read_to_string(file).map_err(|err| Error::io(file.to_path_buf(), err))?;
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("definition")
        .to_string();
    let ext = file
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let def = match ext.as_str() {
        "toml" => native::parse_native(&text, &name)?,
        "xml" => crate::definition::cleanerml::parse_cleanerml(&text, &name, trust)?,
        "ini" => {
            return Err(Error::definition(
                &name,
                "winapp2.ini files must be loaded with load_winapp2()",
            ))
        }
        "" => {
            return Err(Error::definition(
                &name,
                "definition file has no extension; expected .toml or .xml",
            ))
        }
        other => {
            return Err(Error::definition(
                &name,
                format!("unsupported definition format '.{other}'"),
            ))
        }
    };

    Ok(LoadedCleaner::new(
        def,
        trust,
        file.to_string_lossy().into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_cleaner() -> CleanerDef {
        let mut cleaner = CleanerDef::new("sample", "Sample");
        cleaner.options.push(OptionDef {
            id: "cache".to_string(),
            label: "Cache".to_string(),
            description: String::new(),
            warning: String::new(),
            actions: vec![ActionDef {
                command: "delete".to_string(),
                path: "$$base$$/cache".to_string(),
                ..ActionDef::default()
            }],
            os: OsFilter::any(),
        });
        cleaner
    }

    fn root() -> &'static str {
        if cfg!(windows) {
            "C:\\"
        } else {
            "/"
        }
    }

    #[test]
    fn expands_cartesian_variables() {
        let mut vars = VarSet::new();
        vars.insert("base", vec!["/a".to_string(), "/b".to_string()]);
        vars.insert("user", vec!["one".to_string(), "two".to_string()]);
        assert_eq!(
            vars.expand("$$base$$/$$user$$/x"),
            vec!["/a/one/x", "/a/two/x", "/b/one/x", "/b/two/x"]
        );
    }

    #[test]
    fn variable_placeholders_are_exact() {
        let mut vars = VarSet::new();
        vars.insert("a", vec!["short".to_string()]);
        vars.insert("a_b", vec!["long".to_string()]);
        assert_eq!(vars.expand("$$a_b$$"), vec!["long"]);
        assert_eq!(vars.expand("$$a$$"), vec!["short"]);
        assert_eq!(vars.expand("$$missing$$"), vec!["$$missing$$"]);
    }

    #[test]
    fn repeated_variable_uses_one_value_per_combination() {
        let mut vars = VarSet::new();
        vars.insert("x", vec!["1".to_string(), "2".to_string()]);
        assert_eq!(vars.expand("$$x$$-$$x$$"), vec!["1-1", "2-2"]);
    }

    #[test]
    fn expanded_values_are_not_expanded_again() {
        let mut vars = VarSet::new();
        vars.insert("a", vec!["$$b$$".to_string()]);
        vars.insert("b", vec!["boom".to_string()]);
        assert_eq!(vars.expand("$$a$$"), vec!["$$b$$"]);
    }

    #[test]
    fn glob_variables_expand_the_environment_first() {
        let dir = std::env::temp_dir().join(format!("sweep_glob_env_{}", std::process::id()));
        let profile = dir.join("Profile 1");
        std::fs::create_dir_all(&profile).unwrap();

        let name = format!("SWEEP_TEST_GLOB_{}", std::process::id());
        std::env::set_var(&name, &dir);
        let mut cleaner = CleanerDef::new("sample", "Sample");
        cleaner.vars.push(VarDef {
            name: "profiles".to_string(),
            values: vec![VarValue {
                raw: format!("${name}{}*", std::path::MAIN_SEPARATOR),
                search: VarSearch::Glob,
                os: OsFilter::any(),
                reg_name: None,
            }],
        });

        let resolved = cleaner.resolve_vars();
        let values = resolved.get("profiles").unwrap_or(&[]);
        assert!(
            values.iter().any(|value| value.ends_with("Profile 1")),
            "{values:?}"
        );

        std::env::remove_var(&name);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_variable_values_come_first() {
        let mut vars = VarSet::new();
        vars.insert("x", vec!["old".to_string()]);
        vars.insert("x", vec!["new".to_string()]);
        let expected = vec!["new".to_string(), "old".to_string()];
        assert_eq!(vars.get("x"), Some(expected.as_slice()));
    }

    #[test]
    fn duplicate_variable_values_are_merged() {
        let mut vars = VarSet::new();
        vars.insert("x", vec!["same".to_string()]);
        vars.insert("x", vec!["same".to_string(), "other".to_string()]);
        let expected = vec!["same".to_string(), "other".to_string()];
        assert_eq!(vars.get("x"), Some(expected.as_slice()));
    }

    #[test]
    fn action_paths_keep_absolute_and_drop_relative() {
        let root = root();
        let mut vars = VarSet::new();
        vars.insert("base", vec![format!("{root}tmp")]);

        let mut action = ActionDef::default();
        action.path = "$$base$$/cache".to_string();
        let paths = expand_action_paths(&action, &vars);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].starts_with(root));

        action.path = "relative/cache".to_string();
        assert!(expand_action_paths(&action, &vars).is_empty());

        action.path = "$$missing$$/cache".to_string();
        assert!(expand_action_paths(&action, &vars).is_empty());
    }

    #[test]
    fn unresolved_placeholders_are_dropped_even_in_absolute_paths() {
        let root = root();
        let mut action = ActionDef::default();
        action.path = format!("{root}tmp/$$missing$$/cache");
        assert!(expand_action_paths(&action, &VarSet::new()).is_empty());
    }

    #[test]
    fn parent_components_and_filesystem_root_are_dropped() {
        let root = root();
        let mut action = ActionDef::default();
        action.path = format!("{root}tmp/../etc");
        assert!(expand_action_paths(&action, &VarSet::new()).is_empty());

        action.path = root.to_string();
        assert!(expand_action_paths(&action, &VarSet::new()).is_empty());
    }

    #[test]
    fn registry_paths_need_no_absolute_root() {
        let action = ActionDef {
            command: "winreg".to_string(),
            path: "HKCU/Software/Sweep".to_string(),
            ..ActionDef::default()
        };
        assert_eq!(expand_action_paths(&action, &VarSet::new()).len(), 1);
    }

    #[test]
    fn search_kind_round_trips() {
        for kind in [
            SearchKind::File,
            SearchKind::Glob,
            SearchKind::WalkAll,
            SearchKind::WalkFiles,
            SearchKind::WalkTop,
            SearchKind::Deep,
        ] {
            assert_eq!(SearchKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(SearchKind::parse(""), Some(SearchKind::File));
        assert_eq!(SearchKind::parse("nope"), None);
    }

    #[test]
    fn os_filter_is_case_insensitive() {
        assert_eq!(OsFilter::new(""), OsFilter::any());
        assert_eq!(OsFilter::new("Windows"), OsFilter(Some("windows".to_string())));
        assert!(OsFilter::new(platform::os_name()).matches());
    }

    #[test]
    fn validates_structural_gaps() {
        let mut cleaner = sample_cleaner();
        cleaner.vars.push(VarDef {
            name: "empty".to_string(),
            values: Vec::new(),
        });
        cleaner.options.push(OptionDef {
            id: "dead".to_string(),
            label: String::new(),
            description: String::new(),
            warning: String::new(),
            actions: Vec::new(),
            os: OsFilter::new("notanos"),
        });
        cleaner.options[0].actions[0].path = "$$undeclared$$/x".to_string();

        let issues = cleaner.validate().join("\n");
        assert!(issues.contains("no values"), "{issues}");
        assert!(issues.contains("no actions"), "{issues}");
        assert!(issues.contains("undeclared variable"), "{issues}");
        assert!(issues.contains("unknown os filter"), "{issues}");
    }

    #[test]
    fn validates_inverted_ranges() {
        let mut cleaner = sample_cleaner();
        cleaner.vars.push(VarDef {
            name: "base".to_string(),
            values: vec![VarValue {
                raw: "/x".to_string(),
                search: VarSearch::Literal,
                os: OsFilter::any(),
                reg_name: None,
            }],
        });
        cleaner.options[0].actions[0].min_age_days = Some(10);
        cleaner.options[0].actions[0].max_age_days = Some(1);
        cleaner.options[0].actions[0].min_size = Some(10);
        cleaner.options[0].actions[0].max_size = Some(1);

        let issues = cleaner.validate().join("\n");
        assert!(issues.contains("min_age_days"), "{issues}");
        assert!(issues.contains("min_size"), "{issues}");
    }

    #[test]
    fn active_options_respect_the_os_filter() {
        let mut cleaner = sample_cleaner();
        cleaner.options.push(OptionDef {
            id: "foreign".to_string(),
            label: "Foreign".to_string(),
            description: String::new(),
            warning: String::new(),
            actions: vec![ActionDef::default()],
            os: OsFilter::new(if cfg!(windows) { "linux" } else { "windows" }),
        });
        assert_eq!(cleaner.active_options().len(), 1);
        assert!(cleaner.option("cache").is_some());
        assert!(cleaner.option("missing").is_none());
    }

    #[test]
    fn untrusted_definitions_lose_privileged_actions() {
        let mut cleaner = sample_cleaner();
        cleaner.options[0].actions.push(ActionDef {
            command: "process".to_string(),
            path: "something".to_string(),
            ..ActionDef::default()
        });
        let (cleaner, dropped) = apply_trust(cleaner, Trust::Untrusted);
        assert_eq!(dropped, vec!["process"]);
        assert_eq!(cleaner.options[0].actions.len(), 1);
    }

    #[test]
    fn untrusted_unknown_commands_fail_closed() {
        let mut cleaner = sample_cleaner();
        cleaner.options[0].actions.push(ActionDef {
            command: "Frobnicate".to_string(),
            ..ActionDef::default()
        });
        cleaner.options[0].actions.push(ActionDef {
            command: " PROCESS ".to_string(),
            ..ActionDef::default()
        });
        let (cleaner, dropped) = apply_trust(cleaner, Trust::Untrusted);
        assert_eq!(dropped.len(), 2);
        assert_eq!(cleaner.options[0].actions.len(), 1);
    }

    #[test]
    fn options_emptied_by_trust_are_removed() {
        let mut cleaner = sample_cleaner();
        cleaner.options.push(OptionDef {
            id: "priv".to_string(),
            label: "Priv".to_string(),
            description: String::new(),
            warning: String::new(),
            actions: vec![ActionDef {
                command: "winreg".to_string(),
                path: "HKCU/Software/X".to_string(),
                ..ActionDef::default()
            }],
            os: OsFilter::any(),
        });
        let (cleaner, dropped) = apply_trust(cleaner, Trust::Untrusted);
        assert_eq!(dropped, vec!["winreg"]);
        assert_eq!(cleaner.options.len(), 1);
        assert!(cleaner.option("priv").is_none());
    }

    #[test]
    fn trusted_definitions_are_untouched() {
        let mut cleaner = sample_cleaner();
        cleaner.options[0].actions.push(ActionDef {
            command: "process".to_string(),
            path: "something".to_string(),
            ..ActionDef::default()
        });
        let (cleaner, dropped) = apply_trust(cleaner, Trust::Trusted);
        assert!(dropped.is_empty());
        assert_eq!(cleaner.options[0].actions.len(), 2);
    }

    #[test]
    fn untrusted_registry_variables_are_dropped() {
        let mut cleaner = sample_cleaner();
        cleaner.vars.push(VarDef {
            name: "base".to_string(),
            values: vec![
                VarValue {
                    raw: "/safe".to_string(),
                    search: VarSearch::Literal,
                    os: OsFilter::any(),
                    reg_name: None,
                },
                VarValue {
                    raw: "HKCU/Software/X".to_string(),
                    search: VarSearch::Registry,
                    os: OsFilter::any(),
                    reg_name: None,
                },
            ],
        });
        let (cleaner, dropped) = apply_trust(cleaner, Trust::Untrusted);
        assert_eq!(dropped.len(), 1);
        assert_eq!(cleaner.vars[0].values.len(), 1);
    }
}
