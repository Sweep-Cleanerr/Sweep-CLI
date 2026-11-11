//! User configuration (`config.toml`).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::error::{Error, Result};
use crate::core::keep::KeepList;
use crate::core::path::expand_defined;
use crate::platform;

fn default_deepscan_patterns() -> Vec<String> {
    [
        "*.log",
        "*.log.*",
        "*.tmp",
        "*.temp",
        "*.bak",
        "*.old",
        "*.~*",
        "*~",
        "*.swp",
        "*.swo",
        "*.orig",
        "*.rej",
        ".nfs*",
        "*cache*",
        "Thumbs.db",
        ".DS_Store",
    ]
    .iter()
    .map(|pattern| (*pattern).to_string())
    .collect()
}

fn languages_from_env() -> Vec<String> {
    for name in ["LANGUAGE", "LC_ALL", "LANG"] {
        let Ok(raw) = std::env::var(name) else {
            continue;
        };

        if raw.is_empty() {
            continue;
        }

        let mut languages = Vec::new();

        for part in raw.split(':') {
            // A locale is language_territory.encoding; only the language
            // belongs in the configured language list.
            let tag = part
                .split(['.', '@', '_'])
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();

            if !tag.is_empty() && !languages.contains(&tag) {
                languages.push(tag);
            }
        }

        if !languages.is_empty() {
            return languages;
        }
    }

    vec!["en".to_string()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub shred: bool,
    pub languages: Vec<String>,
    pub whitelist_files: Vec<String>,
    pub whitelist_folders: Vec<String>,
    pub custom_paths: Vec<String>,
    pub deepscan_roots: Vec<String>,
    pub deepscan_patterns: Vec<String>,
    pub remember_last: bool,
    pub last_selection: Vec<String>,
    pub journal_size: String,
    pub journal_days: u64,
    pub pacman_keep: u64,
    pub temp_max_age_days: u64,
    pub docker_build_cache: bool,
    pub hub_url: String,
    pub backup_dir: String,
    pub backup_max_size: String,
    pub log_file: String,
    pub trash_dir: String,
    pub git_guard: String,
    pub reserve_mb: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shred: false,
            languages: languages_from_env(),
            whitelist_files: Vec::new(),
            whitelist_folders: Vec::new(),
            custom_paths: Vec::new(),
            deepscan_roots: Vec::new(),
            deepscan_patterns: default_deepscan_patterns(),
            remember_last: false,
            last_selection: Vec::new(),
            journal_size: "500M".to_string(),
            journal_days: 7,
            pacman_keep: 2,
            temp_max_age_days: 1,
            docker_build_cache: false,
            hub_url:
                "https://raw.githubusercontent.com/sweep-cleaner/hub/main/index.toml"
                    .to_string(),
            backup_dir: String::new(),
            backup_max_size: "100M".to_string(),
            log_file: String::new(),
            trash_dir: String::new(),
            git_guard: "warn".to_string(),
            reserve_mb: 512,
        }
    }
}

pub fn portable_base() -> Option<PathBuf> {
    std::env::var_os("SWEEP_PORTABLE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn default_path() -> PathBuf {
    if let Some(base) = portable_base() {
        return base.join("config.toml");
    }

    if let Some(directory) = platform::config_dir() {
        return directory.join("config.toml");
    }

    PathBuf::from("sweep.toml")
}

fn legacy_path(path: &Path) -> Option<PathBuf> {
    Some(path.parent()?.join("sweep").join("config.toml"))
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|error| {
                Error::Config(format!(
                    "cannot parse {}: {error}",
                    path.display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Some(legacy) = legacy_path(path) {
                    if legacy.is_file() {
                        return Self::load(&legacy);
                    }
                }

                Ok(Self::default())
            }
            Err(error) => Err(Error::Config(format!(
                "cannot read {}: {error}",
                path.display()
            ))),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let text = toml::to_string_pretty(self).map_err(|error| {
            Error::Config(format!("cannot encode config: {error}"))
        })?;

        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "config.toml".to_string());

        let temporary = path.with_file_name(format!(
            "{file_name}.{}.tmp",
            std::process::id()
        ));

        if let Err(error) = fs::write(&temporary, text) {
            let _ = fs::remove_file(&temporary);

            return Err(Error::Config(format!(
                "cannot write {}: {error}",
                temporary.display()
            )));
        }

        match fs::rename(&temporary, path) {
            Ok(()) => Ok(()),
            Err(first_error) => {
                // Windows does not consistently permit rename to replace an
                // existing destination. Retry after removing the old file.
                #[cfg(windows)]
                if path.exists() {
                    fs::remove_file(path).map_err(|error| {
                        Error::Config(format!(
                            "cannot remove old {}: {error}",
                            path.display()
                        ))
                    })?;

                    if let Err(error) = fs::rename(&temporary, path) {
                        let _ = fs::remove_file(&temporary);

                        return Err(Error::Config(format!(
                            "cannot replace {}: {error}",
                            path.display()
                        )));
                    }

                    return Ok(());
                }

                let _ = fs::remove_file(&temporary);

                Err(Error::Config(format!(
                    "cannot replace {}: {first_error}",
                    path.display()
                )))
            }
        }
    }

    pub fn keep_list(&self) -> KeepList {
        let mut keep = KeepList::new();

        for file in &self.whitelist_files {
            keep.add_file(file);
        }

        for folder in &self.whitelist_folders {
            keep.add_folder(folder);
        }

        keep
    }

    pub fn custom_paths(&self) -> Vec<PathBuf> {
        self.custom_paths
            .iter()
            .filter_map(|path| expand_defined(path))
            .collect()
    }

    pub fn deepscan_roots(&self) -> Vec<PathBuf> {
        self.deepscan_roots
            .iter()
            .filter_map(|path| expand_defined(path))
            .collect()
    }

    pub fn field_names() -> &'static [&'static str] {
        &[
            "shred",
            "languages",
            "whitelist_files",
            "whitelist_folders",
            "custom_paths",
            "deepscan_roots",
            "deepscan_patterns",
            "remember_last",
            "last_selection",
            "journal_size",
            "journal_days",
            "pacman_keep",
            "temp_max_age_days",
            "docker_build_cache",
            "hub_url",
            "backup_dir",
            "backup_max_size",
            "log_file",
            "trash_dir",
            "git_guard",
            "reserve_mb",
        ]
    }

    pub fn fields(&self) -> Vec<(&'static str, String)> {
        Self::field_names()
            .iter()
            .filter_map(|name| {
                self.get_field(name)
                    .ok()
                    .map(|value| (*name, value))
            })
            .collect()
    }

    pub fn get_field(&self, name: &str) -> Result<String> {
        Ok(match name {
            "shred" => self.shred.to_string(),
            "languages" => format_list(&self.languages),
            "whitelist_files" => format_list(&self.whitelist_files),
            "whitelist_folders" => {
                format_list(&self.whitelist_folders)
            }
            "custom_paths" => format_list(&self.custom_paths),
            "deepscan_roots" => format_list(&self.deepscan_roots),
            "deepscan_patterns" => {
                format_list(&self.deepscan_patterns)
            }
            "remember_last" => self.remember_last.to_string(),
            "last_selection" => format_list(&self.last_selection),
            "journal_size" => self.journal_size.clone(),
            "journal_days" => self.journal_days.to_string(),
            "pacman_keep" => self.pacman_keep.to_string(),
            "temp_max_age_days" => self.temp_max_age_days.to_string(),
            "docker_build_cache" => {
                self.docker_build_cache.to_string()
            }
            "hub_url" => self.hub_url.clone(),
            "backup_dir" => self.backup_dir.clone(),
            "backup_max_size" => self.backup_max_size.clone(),
            "log_file" => self.log_file.clone(),
            "trash_dir" => self.trash_dir.clone(),
            "git_guard" => self.git_guard.clone(),
            "reserve_mb" => self.reserve_mb.to_string(),
            other => {
                return Err(Error::Config(format!(
                    "unknown setting '{other}' (try `sweep config show`)"
                )));
            }
        })
    }

    pub fn set_field(&mut self, name: &str, value: &str) -> Result<()> {
        let value = value.trim();

        match name {
            "shred" => self.shred = parse_bool(name, value)?,
            "languages" => self.languages = parse_list(value)?,
            "whitelist_files" => {
                self.whitelist_files = parse_list(value)?;
            }
            "whitelist_folders" => {
                self.whitelist_folders = parse_list(value)?;
            }
            "custom_paths" => self.custom_paths = parse_list(value)?,
            "deepscan_roots" => {
                self.deepscan_roots = parse_list(value)?;
            }
            "deepscan_patterns" => {
                self.deepscan_patterns = parse_list(value)?;
            }
            "remember_last" => {
                self.remember_last = parse_bool(name, value)?;
            }
            "last_selection" => {
                self.last_selection = parse_list(value)?;
            }
            "journal_size" => self.journal_size = value.to_string(),
            "journal_days" => {
                self.journal_days = parse_u64(name, value)?;
            }
            "pacman_keep" => {
                self.pacman_keep = parse_u64(name, value)?;
            }
            "temp_max_age_days" => {
                self.temp_max_age_days = parse_u64(name, value)?;
            }
            "docker_build_cache" => {
                self.docker_build_cache = parse_bool(name, value)?;
            }
            "hub_url" => self.hub_url = value.to_string(),
            "backup_dir" => self.backup_dir = value.to_string(),
            "backup_max_size" => {
                self.backup_max_size = value.to_string();
            }
            "log_file" => self.log_file = value.to_string(),
            "trash_dir" => self.trash_dir = value.to_string(),
            "git_guard" => {
                if !matches!(
                    value,
                    "off" | "warn" | "block" | "paranoid"
                ) {
                    return Err(Error::Config(
                        "setting 'git_guard' expects off, warn, block or paranoid"
                            .to_string(),
                    ));
                }

                self.git_guard = value.to_string();
            }
            "reserve_mb" => self.reserve_mb = parse_u64(name, value)?,
            other => {
                return Err(Error::Config(format!(
                    "unknown setting '{other}' (try `sweep config show`)"
                )));
            }
        }

        Ok(())
    }
}

fn parse_list(value: &str) -> Result<Vec<String>> {
    if value.starts_with('[') {
        return serde_json::from_str::<Vec<String>>(value).map_err(|error| {
            Error::Config(format!("invalid JSON string list: {error}"))
        });
    }

    Ok(value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect())
}

fn format_list(values: &[String]) -> String {
    if values.is_empty()
        || values
            .iter()
            .any(|value| value.contains(',') || value.starts_with('['))
    {
        serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
    } else {
        values.join(",")
    }
}

fn parse_bool(name: &str, value: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(Error::Config(format!(
            "setting '{name}' expects true or false"
        ))),
    }
}

fn parse_u64(name: &str, value: &str) -> Result<u64> {
    value
        .parse()
        .map_err(|_| Error::Config(format!("setting '{name}' expects a number")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_path() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);

        std::env::temp_dir().join(format!(
            "sweep-config-{}-{id}.toml",
            std::process::id()
        ))
    }

    #[test]
    fn round_trip_through_disk() {
        let path = temp_path();
        let mut config = Config::default();

        config.shred = true;
        config.languages = vec!["tr".into(), "en".into()];
        config.whitelist_folders =
            vec!["~/important".into(), "/tmp/a,b".into()];
        config.journal_days = 3;
        config.git_guard = "block".into();

        config.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();

        assert!(loaded.shred);
        assert_eq!(loaded.languages, vec!["tr", "en"]);
        assert_eq!(
            loaded.whitelist_folders,
            config.whitelist_folders
        );
        assert_eq!(loaded.journal_days, 3);
        assert_eq!(loaded.git_guard, "block");

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_file_yields_defaults() {
        let path = temp_path();
        let config = Config::load(&path).unwrap();

        assert_eq!(config.journal_size, "500M");
        assert_eq!(config.reserve_mb, 512);
        assert!(
            config
                .deepscan_patterns
                .contains(&"*.tmp".to_string())
        );
    }

    #[test]
    fn get_set_and_validation() {
        let mut config = Config::default();

        config.set_field("shred", "yes").unwrap();
        assert!(config.shred);
        assert_eq!(config.get_field("shred").unwrap(), "true");

        config.set_field("languages", "de, fr").unwrap();
        assert_eq!(config.languages, vec!["de", "fr"]);

        config
            .set_field("whitelist_files", r#"["a,b", "c"]"#)
            .unwrap();

        assert_eq!(config.whitelist_files, vec!["a,b", "c"]);
        assert_eq!(
            config.get_field("whitelist_files").unwrap(),
            r#"["a,b","c"]"#
        );

        assert!(config.set_field("journal_days", "many").is_err());
        assert!(config.set_field("git_guard", "sometimes").is_err());
        assert!(config.set_field("nope", "x").is_err());
    }

    #[test]
    fn malformed_json_list_is_rejected_without_clearing_the_setting() {
        let mut config = Config::default();
        config.languages = vec!["en".into()];

        assert!(config
            .set_field("languages", r#"["tr", broken]"#)
            .is_err());

        // A failed assignment must not silently erase the old value.
        assert_eq!(config.languages, vec!["en"]);
    }

    #[test]
    fn every_field_is_reachable() {
        let mut config = Config::default();

        for name in Config::field_names() {
            let value = config.get_field(name).unwrap();

            config
                .set_field(name, &value)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        }

        assert_eq!(
            config.fields().len(),
            Config::field_names().len()
        );
    }

    #[test]
    fn locale_precedence_and_cleanup() {
        let keys = ["LANGUAGE", "LC_ALL", "LANG"];
        let saved: Vec<_> = keys
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect();

        std::env::remove_var("LANGUAGE");
        std::env::set_var("LC_ALL", "");
        std::env::set_var("LANG", "tr_TR.UTF-8");

        assert_eq!(languages_from_env(), vec!["tr"]);

        std::env::set_var("LC_ALL", "de_DE@euro");
        assert_eq!(languages_from_env(), vec!["de"]);

        std::env::set_var("LANGUAGE", "fr_FR:it_IT");
        assert_eq!(languages_from_env(), vec!["fr", "it"]);

        for (key, value) in saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}
