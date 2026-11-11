//! Cleaner definition loading and the registry used by the engine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::definition::model::{apply_trust, parse_file};

pub mod cleanerml;
pub mod model;
pub mod native;
pub mod winapp2;

pub mod builtin {
    include!(concat!(env!("OUT_DIR"), "/builtin_cleaners.rs"));
}

pub use model::{
    ActionDef, CleanerDef, LoadedCleaner, ObjectType, OptionDef, OsFilter, RunningCheck,
    SearchKind, Trust, VarDef, VarSearch, VarSet, VarValue,
};

#[derive(Debug, Clone)]
pub struct LoadOptions {
    pub builtin: bool,
    pub bleachbit: bool,
    pub winapp2: bool,
    pub extra_dirs: Vec<PathBuf>,
    pub external_trust: Trust,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            builtin: true,
            bleachbit: false,
            winapp2: false,
            extra_dirs: Vec::new(),
            external_trust: Trust::Untrusted,
        }
    }
}

impl LoadOptions {
    pub fn builtin_only() -> Self {
        Self::default()
    }

    pub fn everything() -> Self {
        Self {
            bleachbit: true,
            winapp2: true,
            ..Self::default()
        }
    }

    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        let mut options = Self::default();
        options.extra_dirs.push(dir.into());
        options
    }

    pub fn trusting_external(dir: impl Into<PathBuf>) -> Self {
        let mut options = Self::with_dir(dir);
        options.external_trust = Trust::Trusted;
        options
    }
}

#[derive(Debug, Default)]
pub struct CleanerRegistry {
    cleaners: BTreeMap<String, LoadedCleaner>,
    warnings: Vec<String>,
}

impl CleanerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(options: &LoadOptions) -> Self {
        let mut registry = Self::new();
        registry.load_into(options);
        registry
    }

    pub fn load_into(&mut self, options: &LoadOptions) {
        if options.builtin {
            self.load_builtin();
        }
        if options.bleachbit {
            self.load_bleachbit(options.external_trust);
        }
        if options.winapp2 {
            self.load_winapp2(options.external_trust);
        }
        for dir in &options.extra_dirs {
            self.load_dir(dir, options.external_trust);
        }
    }

    pub fn load_builtin(&mut self) {
        for (file, text) in builtin::BUILTIN_CLEANERS {
            match native::parse_native(text, file) {
                Ok(def) => self.register(def, Trust::Trusted, format!("<builtin>/{file}")),
                Err(err) => self.warn(format!("failed to load builtin cleaner {file}: {err}")),
            }
        }
    }

    pub fn load_bleachbit(&mut self, trust: Trust) {
        for dir in bleachbit_dirs() {
            if dir.is_dir() {
                self.load_dir(&dir, trust);
            }
        }
    }

    pub fn load_winapp2(&mut self, trust: Trust) {
        for path in winapp2_paths() {
            if path.is_file() {
                let defs = winapp2::load_winapp2_dir_entry(&path, trust);
                for def in defs {
                    self.register(def, trust, path.to_string_lossy().into_owned());
                }
            }
        }
    }

    fn load_dir(&mut self, dir: &Path, trust: Trust) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                self.warn(format!(
                    "cannot read definition directory {}: {err}",
                    dir.display()
                ));
                return;
            }
        };

        let mut files: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        files.sort();

        for file in files {
            let ext = file
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "toml" || ext == "xml" {
                match parse_file(&file, trust) {
                    Ok(loaded) => self.register(loaded.def, loaded.trust, loaded.source),
                    Err(err) => self.warn(format!("failed to load {}: {err}", file.display())),
                }
            } else if ext == "ini" {
                for def in winapp2::load_winapp2_dir_entry(&file, trust) {
                    self.register(def, trust, file.to_string_lossy().into_owned());
                }
            } else {
                log::debug!(
                    "skipping {}: unsupported definition format",
                    file.display()
                );
            }
        }
    }

    /// Single gate for every definition: validates, applies the trust policy
    /// and refuses to let untrusted definitions replace trusted ones.
    pub fn register(&mut self, def: CleanerDef, trust: Trust, source: impl Into<String>) {
        if !def.os.matches() {
            return;
        }
        let source = source.into();

        if def.id.trim().is_empty() {
            self.warn(format!("cleaner from {source}: skipped, it has no id"));
            return;
        }

        let blocked_by = self
            .cleaners
            .get(&def.id)
            .filter(|existing| existing.trust == Trust::Trusted && trust == Trust::Untrusted)
            .map(|existing| existing.source.clone());
        if let Some(existing_source) = blocked_by {
            self.warn(format!(
                "cleaner '{}' from {source}: ignored, it would replace the trusted cleaner from {existing_source}",
                def.id
            ));
            return;
        }

        for issue in def.validate() {
            self.warn(format!("cleaner '{}' from {source}: {issue}", def.id));
        }

        let (def, dropped) = apply_trust(def, trust);
        for item in dropped {
            self.warn(format!(
                "cleaner '{}' from {source}: ignored restricted item '{item}'",
                def.id
            ));
        }

        let id = def.id.clone();
        self.cleaners
            .insert(id, LoadedCleaner::new(def, trust, source));
    }

    pub fn get(&self, id: &str) -> Option<&LoadedCleaner> {
        self.cleaners.get(id)
    }

    pub fn def(&self, id: &str) -> Option<&CleanerDef> {
        self.cleaners.get(id).map(|loaded| &loaded.def)
    }

    pub fn all(&self) -> impl Iterator<Item = &LoadedCleaner> {
        self.cleaners.values()
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.cleaners.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.cleaners.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cleaners.is_empty()
    }

    pub fn remove(&mut self, id: &str) -> Option<LoadedCleaner> {
        self.cleaners.remove(id)
    }

    pub fn matching(&self, query: &str) -> Vec<&LoadedCleaner> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        if let Some(loaded) = self.cleaners.get(query) {
            return vec![loaded];
        }

        if !has_glob(query) {
            return self
                .cleaners
                .iter()
                .find(|(id, _)| id.eq_ignore_ascii_case(query))
                .map(|(_, loaded)| vec![loaded])
                .unwrap_or_default();
        }

        let matcher = match globset::GlobBuilder::new(query)
            .case_insensitive(true)
            .literal_separator(true)
            .build()
        {
            Ok(glob) => glob.compile_matcher(),
            Err(err) => {
                log::warn!("invalid cleaner pattern '{query}': {err}");
                return Vec::new();
            }
        };
        self.cleaners
            .iter()
            .filter(|(id, _)| matcher.is_match(id.as_str()))
            .map(|(_, loaded)| loaded)
            .collect()
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.cleaners
            .get(id)
            .map(|loaded| loaded.def.is_running())
            .unwrap_or(false)
    }
}

fn has_glob(input: &str) -> bool {
    input
        .chars()
        .any(|ch| matches!(ch, '*' | '?' | '[' | ']' | '{'))
}

fn bleachbit_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = crate::platform::home_dir() {
        dirs.push(home.join(".bleachbit").join("languages"));
        dirs.push(home.join(".bleachbit"));
    }
    if let Some(data) = dirs::data_dir() {
        dirs.push(data.join("bleachbit"));
        dirs.push(data.join("sweep").join("bleachbit"));
    }
    dirs
}

fn winapp2_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = crate::platform::home_dir() {
        for name in ["winapp2.ini", "Winapp2.ini", "WinApp2.ini"] {
            paths.push(home.join(name));
            paths.push(home.join(".config").join(name));
            paths.push(home.join("winapp2").join(name));
        }
    }
    if let Some(docs) = dirs::data_dir() {
        paths.push(docs.join("sweep").join("winapp2.ini"));
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_load_without_warnings() {
        let registry = CleanerRegistry::load(&LoadOptions::builtin_only());
        assert!(
            registry.len() >= 10,
            "only {} builtins loaded: {:?}",
            registry.len(),
            registry.warnings()
        );
        assert!(
            registry.get("system").is_some(),
            "system cleaner missing; loaded: {:?}",
            registry.ids().collect::<Vec<_>>()
        );
        assert!(
            registry.warnings().is_empty(),
            "builtin warnings: {:?}",
            registry.warnings()
        );
    }

    #[test]
    fn matches_exact_and_case_insensitively() {
        let registry = CleanerRegistry::load(&LoadOptions::builtin_only());
        assert_eq!(registry.matching("system").len(), 1);
        assert_eq!(registry.matching("SYSTEM").len(), 1);
        assert!(registry.matching("no-such-cleaner").is_empty());
        assert!(registry.matching("").is_empty());
    }

    #[test]
    fn matches_glob_case_insensitively() {
        let registry = CleanerRegistry::load(&LoadOptions::builtin_only());
        let lower = registry.matching("system*");
        let upper = registry.matching("SYSTEM*");
        assert!(!lower.is_empty());
        assert_eq!(lower.len(), upper.len());
        assert!(lower.iter().all(|loaded| loaded.def.id.starts_with("system")));
    }

    #[test]
    fn untrusted_cannot_replace_trusted() {
        let mut registry = CleanerRegistry::new();
        registry.register(CleanerDef::new("x", "Trusted"), Trust::Trusted, "a");
        registry.register(CleanerDef::new("x", "Evil"), Trust::Untrusted, "b");
        assert_eq!(registry.def("x").unwrap().name, "Trusted");
        assert_eq!(registry.warnings().len(), 1);
    }

    #[test]
    fn cleaners_without_id_are_skipped() {
        let mut registry = CleanerRegistry::new();
        registry.register(CleanerDef::new("  ", "Nameless"), Trust::Trusted, "a");
        assert!(registry.is_empty());
        assert_eq!(registry.warnings().len(), 1);
    }

    #[test]
    fn untrusted_privileged_actions_are_dropped_with_warning() {
        let mut def = CleanerDef::new("p", "P");
        def.options.push(OptionDef {
            id: "o".to_string(),
            label: "O".to_string(),
            description: String::new(),
            warning: String::new(),
            actions: vec![
                ActionDef {
                    command: "process".to_string(),
                    path: "something".to_string(),
                    ..ActionDef::default()
                },
                ActionDef {
                    command: "delete".to_string(),
                    path: "/tmp/x".to_string(),
                    ..ActionDef::default()
                },
            ],
            os: OsFilter::any(),
        });

        let mut registry = CleanerRegistry::new();
        registry.register(def, Trust::Untrusted, "t");
        assert_eq!(registry.def("p").unwrap().options[0].actions.len(), 1);
        assert!(registry.warnings().iter().any(|w| w.contains("process")));
    }
}
