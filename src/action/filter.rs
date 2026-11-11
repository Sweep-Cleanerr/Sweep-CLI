//! Per-action include/exclude filters.
//!
//! A missing mtime never satisfies an age bound: an unreadable timestamp must
//! not make a file look old enough to delete.

use std::fs::Metadata;
use std::path::Path;
use std::time::{Duration, SystemTime};

use regex::bytes::Regex as BytesRegex;
use regex::Regex;

use crate::core::error::{Error, Result};
use crate::definition::model::{ActionDef, ObjectType};

fn compile(pattern: &str) -> Result<Regex> {
    Regex::new(pattern).map_err(|source| Error::Regex {
        pattern: pattern.to_string(),
        source,
    })
}

fn compile_bytes(pattern: &str) -> Result<BytesRegex> {
    BytesRegex::new(pattern).map_err(|source| Error::Regex {
        pattern: pattern.to_string(),
        source,
    })
}

fn compile_opt(pattern: Option<&str>) -> Result<Option<Regex>> {
    pattern.map(compile).transpose()
}

fn compile_bytes_opt(pattern: Option<&str>) -> Result<Option<BytesRegex>> {
    pattern.map(compile_bytes).transpose()
}

#[derive(Debug)]
pub struct ActionFilter {
    name_include: Option<Regex>,
    name_exclude: Option<Regex>,
    path_include: Option<BytesRegex>,
    path_exclude: Option<BytesRegex>,
    object_type: Option<ObjectType>,
    min_age_days: Option<u32>,
    max_age_days: Option<u32>,
    min_size: Option<u64>,
    max_size: Option<u64>,
}

impl ActionFilter {
    pub fn from_action(action: &ActionDef) -> Result<Self> {
        Ok(Self {
            name_include: compile_opt(action.regex.as_deref())?,
            name_exclude: compile_opt(action.nregex.as_deref())?,
            path_include: compile_bytes_opt(action.wholeregex.as_deref())?,
            path_exclude: compile_bytes_opt(action.nwholeregex.as_deref())?,
            object_type: action.object_type,
            min_age_days: action.min_age_days,
            max_age_days: action.max_age_days,
            min_size: action.min_size,
            max_size: action.max_size,
        })
    }

    fn type_matches(&self, is_dir: bool) -> bool {
        match self.object_type {
            Some(ObjectType::File) => !is_dir,
            Some(ObjectType::Dir) => is_dir,
            None => true,
        }
    }

    fn name_and_path_match(&self, path: &Path) -> bool {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();

        if let Some(regex) = &self.name_include {
            if !regex.is_match(&name) {
                return false;
            }
        }
        if let Some(regex) = &self.name_exclude {
            if regex.is_match(&name) {
                return false;
            }
        }

        if self.path_include.is_some() || self.path_exclude.is_some() {
            let full = path_bytes(path);
            if let Some(regex) = &self.path_include {
                if !regex.is_match(&full) {
                    return false;
                }
            }
            if let Some(regex) = &self.path_exclude {
                if regex.is_match(&full) {
                    return false;
                }
            }
        }
        true
    }

    /// Name/type filters only. Used while collecting glob candidates, before
    /// an extra filesystem stat.
    pub fn accepts_name(&self, path: &Path) -> bool {
        self.type_matches(path.is_dir()) && self.name_and_path_match(path)
    }

    /// Full filter against already-collected metadata.
    pub fn accepts_with(&self, path: &Path, metadata: &Metadata) -> bool {
        if !self.type_matches(metadata.is_dir()) {
            return false;
        }
        if !self.name_and_path_match(path) {
            return false;
        }
        if !self.size_matches(metadata) {
            return false;
        }
        self.age_matches(metadata)
    }

    /// Full filter, statting only when one is needed.
    pub fn accepts(&self, path: &Path) -> bool {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => self.accepts_with(path, &metadata),
            Err(_) => false,
        }
    }

    fn size_matches(&self, metadata: &Metadata) -> bool {
        if metadata.is_dir() {
            return true;
        }
        let size = metadata.len();
        if self.min_size.is_some_and(|min| size < min) {
            return false;
        }
        if self.max_size.is_some_and(|max| size > max) {
            return false;
        }
        true
    }

    fn age_matches(&self, metadata: &Metadata) -> bool {
        if self.min_age_days.is_none() && self.max_age_days.is_none() {
            return true;
        }
        let Ok(modified) = metadata.modified() else {
            return false;
        };
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::ZERO);
        let days = age.as_secs() / 86_400;

        if self.min_age_days.is_some_and(|min| days < u64::from(min)) {
            return false;
        }
        if self.max_age_days.is_some_and(|max| days > u64::from(max)) {
            return false;
        }
        true
    }
}

fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().into_owned().into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action() -> ActionDef {
        ActionDef::default()
    }

    fn file(name: &str) -> std::path::PathBuf {
        let root = crate::fixture(&format!("action_filter_{}", name.replace('.', "_")));
        let path = root.join(name);
        std::fs::write(&path, b"hello").unwrap();
        path
    }

    #[test]
    fn name_regexes_are_inclusive_and_exclusive() {
        let path = file("cache.tmp");
        let mut def = action();
        def.regex = Some(r"\.tmp$".to_string());
        assert!(ActionFilter::from_action(&def).unwrap().accepts(&path));

        def.nregex = Some("cache".to_string());
        assert!(!ActionFilter::from_action(&def).unwrap().accepts(&path));
    }

    #[test]
    fn whole_path_regexes_see_the_directory() {
        let path = file("note.txt");
        let mut def = action();
        def.wholeregex = Some("action_filter".to_string());
        assert!(ActionFilter::from_action(&def).unwrap().accepts(&path));

        def.nwholeregex = Some("action_filter".to_string());
        assert!(!ActionFilter::from_action(&def).unwrap().accepts(&path));
    }

    #[test]
    fn object_type_filters_files_and_dirs() {
        let path = file("kind.txt");
        let mut def = action();
        def.object_type = Some(ObjectType::Dir);
        assert!(!ActionFilter::from_action(&def).unwrap().accepts(&path));
        def.object_type = Some(ObjectType::File);
        assert!(ActionFilter::from_action(&def).unwrap().accepts(&path));
    }

    #[test]
    fn size_bounds_are_inclusive() {
        let path = file("size.txt");
        let mut def = action();
        def.min_size = Some(5);
        def.max_size = Some(5);
        assert!(ActionFilter::from_action(&def).unwrap().accepts(&path));
        def.max_size = Some(4);
        assert!(!ActionFilter::from_action(&def).unwrap().accepts(&path));
    }

    #[test]
    fn age_bounds_fail_closed_and_use_whole_days() {
        let path = file("fresh.txt");
        let mut def = action();
        def.min_age_days = Some(1);
        assert!(!ActionFilter::from_action(&def).unwrap().accepts(&path));
        def.min_age_days = Some(0);
        def.max_age_days = Some(0);
        assert!(ActionFilter::from_action(&def).unwrap().accepts(&path));
    }

    #[test]
    fn invalid_regex_is_a_typed_error() {
        let mut def = action();
        def.regex = Some("(".to_string());
        let error = ActionFilter::from_action(&def).unwrap_err();
        assert!(matches!(error, Error::Regex { .. }));
        assert!(error.to_string().contains("invalid regex"));
    }
}
