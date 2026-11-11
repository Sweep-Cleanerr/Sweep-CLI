//! Git dirty-repo protection (`--git-guard`).
//!
//! Detects whether a deletion target sits inside a git repository
//! with uncommitted tracked changes, and warns/blocks accordingly.
//! Implemented with external `git` commands (no gix dependency);
//! untracked files are ignored, `--no-optional-locks` keeps the
//! user's index untouched. Fail-safe: unreadable status counts as
//! dirty.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Protection level (`--git-guard`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Protection {
    /// No checks.
    Off,
    /// Warn but continue (default).
    #[default]
    Warn,
    /// Block targets inside dirty repos.
    Block,
    /// Also block outside repos and recently-changed targets.
    Paranoid,
}

impl Protection {
    /// Parse a `--git-guard` value (`off|warn|block|paranoid`).
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Ok(Protection::Off),
            "warn" => Ok(Protection::Warn),
            "block" => Ok(Protection::Block),
            "paranoid" => Ok(Protection::Paranoid),
            other => Err(format!(
                "--git-guard must be off|warn|block|paranoid, got '{other}'"
            )),
        }
    }

    /// Name for messages and config round-trips.
    pub fn as_str(self) -> &'static str {
        match self {
            Protection::Off => "off",
            Protection::Warn => "warn",
            Protection::Block => "block",
            Protection::Paranoid => "paranoid",
        }
    }
}

/// Decision for a single target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Touch it.
    Allow,
    /// Continue, but tell the user.
    Warn(String),
    /// Skip this target.
    Block(String),
}

const RECENT: Duration = Duration::from_secs(7 * 24 * 60 * 60);

fn modified_recently(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return true;
    };
    let Ok(modified) = meta.modified() else {
        return true;
    };
    match SystemTime::now().duration_since(modified) {
        Ok(age) => age < RECENT,
        Err(_) => true,
    }
}

fn repo_root(path: &Path) -> Option<PathBuf> {
    let start = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    for dir in start.ancestors() {
        let dot = dir.join(".git");
        if dot.is_dir() || dot.is_file() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cleanliness {
    Clean,
    Dirty,
    Unknown,
}

#[derive(Debug, Clone)]
struct RepoState {
    cleanliness: Cleanliness,
    head_ns: Option<u64>,
}

fn load_state(root: &Path) -> RepoState {
    let cleanliness = match crate::fsutil::run_command_in(
        "git",
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--untracked-files=no",
        ],
        root,
        true,
    ) {
        Ok((0, stdout, _)) if stdout.trim().is_empty() => Cleanliness::Clean,
        Ok((0, _, _)) => Cleanliness::Dirty,
        _ => Cleanliness::Unknown,
    };
    let head_ns = crate::fsutil::run_command_in("git", &["log", "-1", "--format=%ct"], root, true)
        .ok()
        .and_then(|(code, stdout, _)| {
            if code != 0 {
                return None;
            }
            stdout.trim().parse::<u64>().ok()
        })
        .map(|secs| secs.saturating_mul(1_000_000_000));
    RepoState {
        cleanliness,
        head_ns,
    }
}

fn deny(level: Protection, reason: &str) -> Verdict {
    match level {
        Protection::Warn => Verdict::Warn(reason.to_string()),
        Protection::Block | Protection::Paranoid => Verdict::Block(reason.to_string()),
        Protection::Off => Verdict::Allow,
    }
}

/// Batch cache: one `git` call per repo and one ancestor scan per directory.
#[derive(Default)]
pub struct GitCache {
    repos: HashMap<PathBuf, RepoState>,
    roots: HashMap<PathBuf, Option<PathBuf>>,
}

impl GitCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop cached repository roots and statuses. Call after the process
    /// changes a worktree so the next guard check sees the new state.
    pub fn invalidate(&mut self) {
        self.repos.clear();
        self.roots.clear();
    }

    fn state(&mut self, root: &Path) -> &RepoState {
        self.repos
            .entry(root.to_path_buf())
            .or_insert_with(|| load_state(root))
    }

    fn root(&mut self, path: &Path, is_dir: bool) -> Option<PathBuf> {
        let start = if is_dir {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        if let Some(root) = self.roots.get(start) {
            return root.clone();
        }
        let root = repo_root(start);
        self.roots.insert(start.to_path_buf(), root.clone());
        root
    }

    pub fn check(&mut self, path: &Path, level: Protection) -> Verdict {
        if level == Protection::Off {
            return Verdict::Allow;
        }
        self.check_kind(path, path.is_dir(), level)
    }

    /// [`check`](Self::check) when the caller already knows `is_dir`.
    pub fn check_kind(&mut self, path: &Path, is_dir: bool, level: Protection) -> Verdict {
        if level == Protection::Off {
            return Verdict::Allow;
        }
        let Some(root) = self.root(path, is_dir) else {
            return if level == Protection::Paranoid {
                Verdict::Block("not inside a git repository".into())
            } else {
                Verdict::Allow
            };
        };
        match self.state(&root).cleanliness {
            Cleanliness::Clean => {}
            Cleanliness::Dirty => return deny(level, "repository has uncommitted changes"),
            Cleanliness::Unknown => {
                return deny(
                    level,
                    "could not read git status (fail-safe: treated as dirty)",
                )
            }
        }
        if level == Protection::Paranoid && modified_recently(path) {
            return Verdict::Block("changed within the last 7 days".into());
        }
        Verdict::Allow
    }

    pub fn activity_ns(&mut self, path: &Path) -> Option<u64> {
        let root = self.root(path, path.is_dir())?;
        self.state(&root).head_ns
    }
}

/// One-shot wrapper (use `GitCache` for batches).
pub fn check(path: &Path, level: Protection) -> Verdict {
    GitCache::new().check(path, level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protection_parses_and_round_trips() {
        assert_eq!(Protection::parse("warn").unwrap(), Protection::Warn);
        assert_eq!(Protection::parse("OFF").unwrap(), Protection::Off);
        assert_eq!(Protection::parse("block").unwrap(), Protection::Block);
        assert_eq!(Protection::parse("paranoid").unwrap(), Protection::Paranoid);
        assert!(Protection::parse("nope").is_err());
        for level in [
            Protection::Off,
            Protection::Warn,
            Protection::Block,
            Protection::Paranoid,
        ] {
            assert_eq!(Protection::parse(level.as_str()).unwrap(), level);
        }
    }

    #[test]
    fn off_level_allows_everything() {
        assert_eq!(check(Path::new("/tmp/x"), Protection::Off), Verdict::Allow);
    }

    #[test]
    fn outside_repo_warn_allows_paranoid_blocks() {
        let dir = crate::fixture("gitguard_out");
        assert_eq!(check(&dir, Protection::Warn), Verdict::Allow);
        assert!(matches!(
            check(&dir, Protection::Paranoid),
            Verdict::Block(_)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
