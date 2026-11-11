//! Per-run state shared by every action provider.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crate::core::gitguard::Protection;
use crate::core::keep::Guard;
use crate::engine::progress::SharedProgress;

#[derive(Debug, Clone)]
pub struct DeepOptions {
    pub journal_size: String,
    pub journal_days: u32,
    pub pacman_keep: u32,
    pub temp_max_age_days: u32,
    pub docker_build_cache: bool,
}

impl Default for DeepOptions {
    fn default() -> Self {
        Self {
            journal_size: "500M".to_string(),
            journal_days: 7,
            pacman_keep: 2,
            temp_max_age_days: 1,
            docker_build_cache: false,
        }
    }
}

#[derive(Clone)]
pub struct RunContext {
    pub guard: Guard,
    pub dry_run: bool,
    pub global_shred: bool,
    pub cancel: Arc<AtomicBool>,
    pub custom_paths: Vec<PathBuf>,
    pub keep_localizations: Vec<String>,
    pub deep_options: DeepOptions,
    /// `--backup-dir`: copy every deleted regular file here (undoable).
    pub backup_dir: Option<PathBuf>,
    /// Per-file backup size limit (`0` = no limit).
    pub backup_max_bytes: u64,
    /// `--trash-dir`: move (not delete) files here; `purge` frees the space.
    pub trash_dir: Option<PathBuf>,
    /// `--log-file`: append one line per operation.
    pub log_file: Option<PathBuf>,
    /// Count of backups that failed or were skipped for size.
    pub backup_failures: Arc<AtomicU64>,
    /// `--git-guard` mode: dirty-repo protection level.
    pub git_guard: Protection,
    /// `--progress-json` sink, one event per finished option.
    pub progress: Option<SharedProgress>,
}

impl RunContext {
    pub fn preview() -> Self {
        Self {
            guard: Guard::default_guard(),
            dry_run: true,
            global_shred: false,
            cancel: Arc::new(AtomicBool::new(false)),
            custom_paths: Vec::new(),
            keep_localizations: Vec::new(),
            deep_options: DeepOptions::default(),
            backup_dir: None,
            backup_max_bytes: 0,
            trash_dir: None,
            log_file: None,
            backup_failures: Arc::new(AtomicU64::new(0)),
            git_guard: Protection::Warn,
            progress: None,
        }
    }

    pub fn clean() -> Self {
        Self {
            dry_run: false,
            ..Self::preview()
        }
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Default for RunContext {
    fn default() -> Self {
        Self::clean()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_never_writes_but_clean_does() {
        assert!(RunContext::preview().dry_run);
        assert!(!RunContext::clean().dry_run);
        assert!(!RunContext::clean().global_shred);
    }

    #[test]
    fn cancellation_is_shared_between_clones() {
        let context = RunContext::preview();
        let clone = context.clone();
        assert!(!clone.cancelled());
        context.request_cancel();
        assert!(clone.cancelled());
    }
}
