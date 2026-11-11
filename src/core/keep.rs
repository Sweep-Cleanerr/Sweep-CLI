//! Keep lists and the guard that stops destructive actions.
//!
//! The guard has two independent gates:
//! protected system/user roots (never removable) and user keep entries
//! (removable only after being removed from the list).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::core::error::{Error, Result};
use crate::core::path::{
    case_sensitive, comparable, equal_prepared, expand_defined, starts_with_prepared,
};
use crate::platform;

/// How long a "this directory is / is not a link" answer may be reused.
const ANCESTOR_CACHE_TTL: Duration = Duration::from_secs(2);
const ANCESTOR_CACHE_MAX: usize = 16_384;

type AncestorCache = HashMap<PathBuf, (bool, Instant)>;

#[derive(Clone, Debug, Default)]
pub struct KeepList {
    files: Vec<PathBuf>,
    folders: Vec<PathBuf>,
    folder_prefixes: Vec<(PathBuf, String)>,
}

impl KeepList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep one path. `~` and environment variables are expanded.
    pub fn add_file(&mut self, path: impl AsRef<Path>) {
        if let Some(path) = expand_path(path.as_ref()) {
            push_unique(&mut self.files, path);
        }
    }

    /// Keep a directory and everything below it. `~` and environment
    /// variables are expanded.
    pub fn add_folder(&mut self, path: impl AsRef<Path>) {
        if let Some(path) = expand_path(path.as_ref()) {
            push_unique(&mut self.folders, path);
        }
    }

    /// Keep every directory directly below `parent` whose name starts with
    /// `prefix`, plus their descendants.
    pub fn add_folder_prefix(&mut self, parent: impl AsRef<Path>, prefix: impl Into<String>) {
        let prefix = prefix.into();
        if prefix.is_empty() {
            return;
        }
        if let Some(parent) = expand_path(parent.as_ref()) {
            self.insert_prefix(parent, prefix);
        }
    }

    /// Like [`add_file`](Self::add_file) for an already concrete path: nothing
    /// is expanded, so `$` and `%` in real directory names stay literal.
    pub fn add_literal_file(&mut self, path: impl AsRef<Path>) {
        if let Some(path) = literal_path(path.as_ref()) {
            push_unique(&mut self.files, path);
        }
    }

    /// Like [`add_folder`](Self::add_folder) for an already concrete path.
    pub fn add_literal_folder(&mut self, path: impl AsRef<Path>) {
        if let Some(path) = literal_path(path.as_ref()) {
            push_unique(&mut self.folders, path);
        }
    }

    /// Like [`add_folder_prefix`](Self::add_folder_prefix) for an already
    /// concrete parent path.
    pub fn add_literal_folder_prefix(
        &mut self,
        parent: impl AsRef<Path>,
        prefix: impl Into<String>,
    ) {
        let prefix = prefix.into();
        if prefix.is_empty() {
            return;
        }
        if let Some(parent) = literal_path(parent.as_ref()) {
            self.insert_prefix(parent, prefix);
        }
    }

    fn insert_prefix(&mut self, parent: PathBuf, prefix: String) {
        let cs = case_sensitive();
        if self
            .folder_prefixes
            .iter()
            .any(|(p, n)| n == &prefix && equal_prepared(p, &parent, cs))
        {
            return;
        }
        self.folder_prefixes.push((parent, prefix));
    }

    pub fn len(&self) -> usize {
        self.files.len() + self.folders.len() + self.folder_prefixes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.contains_prepared(&comparable(path), case_sensitive())
    }

    pub fn contains_cs(&self, path: &Path) -> bool {
        self.contains_prepared(&comparable(path), true)
    }

    /// `path` must already be [`comparable`].
    fn contains_prepared(&self, path: &Path, cs: bool) -> bool {
        self.files.iter().any(|entry| equal_prepared(path, entry, cs))
            || self
                .folders
                .iter()
                .any(|folder| starts_with_prepared(path, folder, cs))
            || self
                .folder_prefixes
                .iter()
                .any(|(parent, prefix)| matches_name_prefix(path, parent, prefix, cs))
    }

    /// Match a path that has already been normalised.
    pub fn matches_normalized(&self, path: &Path) -> bool {
        self.contains(path)
    }

    /// True when `dir` is, or contains, at least one keep entry. Removing such
    /// a directory recursively would destroy kept data.
    pub fn has_entry_within(&self, dir: &Path) -> bool {
        let dir = comparable(dir);
        let cs = case_sensitive();
        self.files
            .iter()
            .chain(self.folders.iter())
            .any(|entry| starts_with_prepared(entry, &dir, cs))
            || self
                .folder_prefixes
                .iter()
                .any(|(parent, _)| starts_with_prepared(parent, &dir, cs))
    }

    pub fn files(&self) -> impl Iterator<Item = &PathBuf> {
        self.files.iter()
    }

    pub fn folders(&self) -> impl Iterator<Item = &PathBuf> {
        self.folders.iter()
    }

    pub fn folder_prefixes(&self) -> impl Iterator<Item = &(PathBuf, String)> {
        self.folder_prefixes.iter()
    }

    pub fn merge(&mut self, other: KeepList) {
        for file in other.files {
            push_unique(&mut self.files, file);
        }
        for folder in other.folders {
            push_unique(&mut self.folders, folder);
        }
        for (parent, prefix) in other.folder_prefixes {
            self.insert_prefix(parent, prefix);
        }
    }
}

/// Expands `~` and variables. Entries that reference an undefined variable are
/// dropped (and logged) instead of silently becoming a path that never matches.
fn expand_path(path: &Path) -> Option<PathBuf> {
    match path.to_str() {
        Some(text) if text.trim().is_empty() => None,
        Some(text) => match expand_defined(text) {
            Some(expanded) => Some(comparable(&expanded)),
            None => {
                log::warn!("keep entry '{text}' references an undefined variable; ignored");
                None
            }
        },
        None => Some(comparable(path)),
    }
}

fn literal_path(path: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        None
    } else {
        Some(comparable(path))
    }
}

/// `path` must already be [`comparable`].
fn push_unique(list: &mut Vec<PathBuf>, path: PathBuf) {
    let cs = case_sensitive();
    if list.iter().any(|existing| equal_prepared(existing, &path, cs)) {
        return;
    }
    list.push(path);
}

fn name_has_prefix(name: &str, prefix: &str, cs: bool) -> bool {
    if cs {
        name.starts_with(prefix)
    } else {
        name.to_lowercase().starts_with(&prefix.to_lowercase())
    }
}

/// `path` must already be [`comparable`].
fn matches_name_prefix(path: &Path, parent: &Path, prefix: &str, cs: bool) -> bool {
    path.ancestors().any(|ancestor| {
        let Some(grandparent) = ancestor.parent() else {
            return false;
        };
        if !equal_prepared(parent, grandparent, cs) {
            return false;
        }
        ancestor
            .file_name()
            .map_or(false, |name| name_has_prefix(&name.to_string_lossy(), prefix, cs))
    })
}

#[cfg(windows)]
fn push_env_root(roots: &mut Vec<PathBuf>, name: &str) {
    if let Some(value) = std::env::var_os(name) {
        if !value.is_empty() {
            push_root(roots, Path::new(&value));
        }
    }
}

fn push_root(roots: &mut Vec<PathBuf>, path: impl AsRef<Path>) {
    push_unique(roots, comparable(path.as_ref()));
}

/// Nodes that may never themselves be deleted or modified. Their contents are
/// still cleanable where a cleaner targets a child path. Every ancestor of a
/// protected node is protected as well, otherwise removing the parent would
/// remove the node.
pub fn protected_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(home) = platform::home_dir() {
        push_root(&mut roots, &home);
        push_root(&mut roots, home.join(".config"));
        push_root(&mut roots, home.join(".local"));
        push_root(&mut roots, home.join(".local").join("share"));
    }

    #[cfg(windows)]
    {
        for name in [
            "SystemDrive",
            "SystemRoot",
            "windir",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
            "ProgramData",
            "CommonProgramFiles",
            "CommonProgramFiles(x86)",
            "CommonProgramW6432",
            "ALLUSERSPROFILE",
            "USERPROFILE",
            "PUBLIC",
            "APPDATA",
            "LOCALAPPDATA",
            "TEMP",
            "TMP",
        ] {
            push_env_root(&mut roots, name);
        }
        if let Some(windir) = std::env::var_os("windir") {
            let windir = Path::new(&windir);
            push_root(&mut roots, windir.join("Temp"));
            push_root(&mut roots, windir.join("System32"));
            push_root(&mut roots, windir.join("SysWOW64"));
            push_root(&mut roots, windir.join("WinSxS"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            push_root(&mut roots, Path::new(&local).join("Temp"));
        }
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
        push_root(&mut roots, format!("{drive}\\"));
    }

    #[cfg(unix)]
    {
        for path in [
            "/",
            "/bin",
            "/boot",
            "/dev",
            "/etc",
            "/etc/fstab",
            "/etc/group",
            "/etc/passwd",
            "/etc/shadow",
            "/etc/sudoers",
            "/home",
            "/lib",
            "/lib32",
            "/lib64",
            "/libx32",
            "/media",
            "/mnt",
            "/opt",
            "/proc",
            "/root",
            "/run",
            "/sbin",
            "/snap",
            "/srv",
            "/sys",
            "/tmp",
            "/usr",
            "/usr/bin",
            "/usr/include",
            "/usr/lib",
            "/usr/lib32",
            "/usr/lib64",
            "/usr/libexec",
            "/usr/local",
            "/usr/sbin",
            "/usr/share",
            "/var",
            "/var/backups",
            "/var/cache",
            "/var/db",
            "/var/lib",
            "/var/lib/dpkg",
            "/var/lib/pacman",
            "/var/lib/rpm",
            "/var/log",
            "/var/spool",
            "/var/tmp",
            "/Users",
            "/Volumes",
            "/System",
            "/Library",
            "/Applications",
            "/Network",
            "/private",
            "/private/etc",
            "/private/tmp",
            "/private/var",
        ] {
            push_root(&mut roots, path);
        }
    }

    let mut seen: HashSet<PathBuf> = roots.iter().cloned().collect();
    let mut closed = roots.clone();
    for root in &roots {
        for ancestor in root.ancestors().skip(1) {
            if ancestor.as_os_str().is_empty() {
                continue;
            }
            if seen.insert(ancestor.to_path_buf()) {
                closed.push(ancestor.to_path_buf());
            }
        }
    }
    closed
}

/// User data directories protected as whole subtrees.
pub fn user_data_subtree_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = platform::home_dir() {
        for name in [
            "Desktop",
            "Documents",
            "Downloads",
            "Pictures",
            "Music",
            "Videos",
            ".ssh",
            ".gnupg",
        ] {
            push_root(&mut roots, home.join(name));
        }
    }
    roots
}

/// Runtime files that live inside otherwise cleanable temp/cache directories.
pub fn builtin_keep_list() -> KeepList {
    #[allow(unused_mut)]
    let mut keep = KeepList::new();

    #[cfg(unix)]
    {
        for name in [
            "/tmp/.X11-unix",
            "/tmp/.font-unix",
            "/tmp/.ICE-unix",
            "/tmp/.Test-unix",
            "/tmp/.XIM-unix",
        ] {
            keep.add_literal_folder(name);
        }
        keep.add_literal_folder_prefix("/tmp", "systemd-private-");
        keep.add_literal_file("/tmp/.X0-lock");

        if let Some(home) = platform::home_dir() {
            for name in [
                "doc",
                "obexd",
                "ibus",
                "wallpaper",
                "mesa_shader_cache",
                "mesa_shader_cache_db",
                "drkonqi",
            ] {
                keep.add_literal_folder(home.join(".cache").join(name));
            }
        }
    }

    // On Windows TEMP/TMP are protected as individual nodes, so their
    // contents remain cleanable and no keep entries are needed here.

    keep
}

#[derive(Clone)]
pub struct Guard {
    keep: KeepList,
    protected: KeepList,
    protected_subtree: KeepList,
    ancestors: Arc<Mutex<AncestorCache>>,
}

impl Guard {
    pub fn new(keep: KeepList) -> Self {
        let mut protected = KeepList::new();
        for root in protected_roots() {
            // Protected roots are nodes: their contents stay cleanable.
            protected.add_literal_file(root);
        }
        let mut protected_subtree = KeepList::new();
        for root in user_data_subtree_roots() {
            protected_subtree.add_literal_folder(root);
        }
        Self {
            keep,
            protected,
            protected_subtree,
            ancestors: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_builtins(keep: KeepList) -> Self {
        let mut merged = keep;
        merged.merge(builtin_keep_list());
        Self::new(merged)
    }

    pub fn default_guard() -> Self {
        Self::with_builtins(KeepList::new())
    }

    pub fn keep(&self) -> &KeepList {
        &self.keep
    }

    pub fn protected(&self) -> &KeepList {
        &self.protected
    }

    pub fn protected_subtree(&self) -> &KeepList {
        &self.protected_subtree
    }

    /// Forget every cached link probe. Call between independent runs.
    pub fn clear_cache(&self) {
        if let Ok(mut cache) = self.ancestors.lock() {
            cache.clear();
        }
    }

    pub fn is_kept(&self, path: &Path) -> bool {
        self.keep.contains(path)
    }

    pub fn is_protected(&self, path: &Path) -> bool {
        self.protected_prepared(&comparable(path))
    }

    /// True when `dir` contains a keep entry, so removing it as a whole would
    /// destroy kept data.
    pub fn keep_inside(&self, dir: &Path) -> bool {
        self.keep.has_entry_within(dir)
    }

    fn protected_prepared(&self, prepared: &Path) -> bool {
        let cs = case_sensitive();
        self.protected.contains_prepared(prepared, cs)
            || self.protected_subtree.contains_prepared(prepared, cs)
    }

    fn verify(&self, path: &Path) -> Result<()> {
        self.verify_prepared(&comparable(path))
    }

    fn verify_prepared(&self, prepared: &Path) -> Result<()> {
        if self.protected_prepared(prepared) {
            return Err(Error::ProtectedPath(prepared.to_path_buf()));
        }
        if self.keep.contains_prepared(prepared, case_sensitive()) {
            return Err(Error::Aborted);
        }
        Ok(())
    }

    /// Validate a path immediately before a destructive action.
    pub fn check(&self, path: &Path, follow_links: bool) -> Result<PathBuf> {
        self.check_prepared(&comparable(path), follow_links)
    }

    /// [`check`](Self::check) for a [`comparable`] path.
    pub fn check_prepared(&self, normalized: &Path, follow_links: bool) -> Result<PathBuf> {
        // The working directory and anything climbing out of it is never a
        // legitimate target.
        if is_unsafe_relative(normalized) {
            return Err(Error::ProtectedPath(normalized.to_path_buf()));
        }

        self.verify_prepared(normalized)?;

        let leaf = link_state(normalized);
        if leaf == LinkState::Error {
            return Err(Error::Link(normalized.to_path_buf()));
        }
        let leaf_is_link = leaf == LinkState::Link;
        let ancestor_link = self.ancestor_has_link(normalized);

        if ancestor_link || (follow_links && leaf_is_link) {
            match canonical_lenient(normalized) {
                Ok(real) => {
                    let real = strip_verbatim(&real);
                    if !equal_prepared(&comparable(&real), normalized, case_sensitive()) {
                        self.verify(&real)?;
                    }
                }
                // Cannot prove where the path really points: fail closed.
                Err(_) => return Err(Error::Link(normalized.to_path_buf())),
            }
        }

        if !follow_links && (leaf_is_link || ancestor_link) {
            return Err(Error::Link(normalized.to_path_buf()));
        }
        Ok(normalized.to_path_buf())
    }

    fn ancestor_has_link(&self, path: &Path) -> bool {
        let mut current = path.parent();
        while let Some(dir) = current {
            // Links directly below the filesystem root (/tmp -> /private/tmp,
            // /lib -> usr/lib, ...) are operating-system layout, not an
            // attack surface; everything deeper is checked.
            if dir.as_os_str().is_empty() || is_root_level(dir) {
                break;
            }
            if self.cached_is_link(dir) {
                return true;
            }
            current = dir.parent();
        }
        false
    }

    fn cached_is_link(&self, dir: &Path) -> bool {
        let now = Instant::now();
        if let Ok(cache) = self.ancestors.lock() {
            if let Some((linked, at)) = cache.get(dir) {
                if now.duration_since(*at) < ANCESTOR_CACHE_TTL {
                    return *linked;
                }
            }
        }

        let linked = link_state(dir) != LinkState::NotLink;
        if let Ok(mut cache) = self.ancestors.lock() {
            if cache.len() >= ANCESTOR_CACHE_MAX {
                cache.clear();
            }
            cache.insert(dir.to_path_buf(), (linked, now));
        }
        linked
    }
}

/// `.`/empty, paths that climb above the start, and drive-relative `C:`.
fn is_unsafe_relative(path: &Path) -> bool {
    let mut parts = path.components();
    match parts.next() {
        None | Some(Component::CurDir) | Some(Component::ParentDir) => true,
        Some(Component::Prefix(_)) => !matches!(parts.next(), Some(Component::RootDir)),
        _ => false,
    }
}

fn is_root_level(dir: &Path) -> bool {
    dir.parent()
        .is_some_and(|parent| parent.parent().is_none())
}

/// Canonicalises `path`; when the leaf (or several trailing components) do not
/// exist, the nearest existing ancestor is canonicalised and the missing tail
/// is appended.
fn canonical_lenient(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(real) => Ok(real),
        Err(err) if err.kind() == io::ErrorKind::NotFound => lenient_tail(path, err),
        Err(err) => Err(err),
    }
}

fn lenient_tail(path: &Path, original: io::Error) -> io::Result<PathBuf> {
    let mut tail = Vec::new();
    let mut current = path;
    loop {
        let Some(name) = current.file_name() else {
            return Err(original);
        };
        tail.push(name.to_os_string());
        let Some(parent) = current.parent() else {
            return Err(original);
        };
        if parent.as_os_str().is_empty() {
            return Err(original);
        }
        match fs::canonicalize(parent) {
            Ok(mut real) => {
                for name in tail.iter().rev() {
                    real.push(name);
                }
                return Ok(real);
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => current = parent,
            Err(err) => return Err(err),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkState {
    NotLink,
    Link,
    Error,
}

fn link_state(path: &Path) -> LinkState {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || platform::is_reparse_point_metadata(&metadata) {
                LinkState::Link
            } else {
                LinkState::NotLink
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => LinkState::NotLink,
        Err(_) => LinkState::Error,
    }
}

pub fn is_link(path: &Path) -> bool {
    link_state(path) == LinkState::Link
}

#[cfg(windows)]
fn strip_verbatim(path: &Path) -> PathBuf {
    crate::core::path::extended_path_undo(path)
}

#[cfg(not(windows))]
fn strip_verbatim(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_keeps_children_but_not_siblings() {
        let mut keep = KeepList::new();
        keep.add_folder("/tmp/sweep-keep");
        assert!(keep.contains(Path::new("/tmp/sweep-keep")));
        assert!(keep.contains(Path::new("/tmp/sweep-keep/a/b")));
        assert!(!keep.contains(Path::new("/tmp/sweep-keep2")));
        assert!(!keep.contains(Path::new("/tmp/other")));
    }

    #[test]
    fn component_boundary_is_not_string_prefix() {
        let mut keep = KeepList::new();
        keep.add_folder("/tmp/a");
        assert!(keep.contains(Path::new("/tmp/a/b")));
        assert!(!keep.contains(Path::new("/tmp/ab")));
    }

    #[test]
    fn prefix_matches_direct_children_and_descendants() {
        let mut keep = KeepList::new();
        keep.add_folder_prefix("/tmp", "systemd-private-");
        assert!(keep.contains(Path::new("/tmp/systemd-private-abc")));
        assert!(keep.contains(Path::new("/tmp/systemd-private-abc/x")));
        assert!(!keep.contains(Path::new("/tmp/not-systemd-private-abc")));
    }

    #[test]
    fn prefix_match_is_normalised() {
        let mut keep = KeepList::new();
        keep.add_folder_prefix("/tmp", "systemd-private-");
        assert!(keep.contains(Path::new("/tmp/x/../systemd-private-abc/y")));
        assert!(!keep.contains(Path::new("/tmp/systemd-private-abc/../other")));
    }

    #[test]
    fn prefix_case_folding_follows_the_platform() {
        let mut keep = KeepList::new();
        keep.add_folder_prefix("/tmp", "systemd-private-");
        let upper = Path::new("/tmp/SYSTEMD-PRIVATE-abc");
        assert_eq!(keep.contains(upper), !case_sensitive());
        assert!(!keep.contains_cs(upper));
    }

    #[test]
    fn file_and_folder_expansion() {
        if let Some(home) = platform::home_dir() {
            let mut keep = KeepList::new();
            keep.add_file("~/state.db");
            assert!(keep.contains(&home.join("state.db")));
            assert!(!keep.contains(&home.join("other.db")));
        }
    }

    #[test]
    fn literal_entries_are_not_expanded() {
        let name = "SWEEP_KEEP_TEST_LITERAL";
        std::env::set_var(name, "expanded");

        let mut keep = KeepList::new();
        keep.add_literal_folder(format!("/tmp/${name}"));
        assert!(keep.contains(Path::new(&format!("/tmp/${name}/x"))));
        assert!(!keep.contains(Path::new("/tmp/expanded/x")));

        std::env::remove_var(name);
    }

    #[test]
    fn entries_with_undefined_variables_are_ignored() {
        let mut keep = KeepList::new();
        keep.add_file("${SWEEP_KEEP_TEST_UNDEFINED}/x");
        keep.add_folder("$SWEEP_KEEP_TEST_UNDEFINED/y");
        keep.add_folder_prefix("${SWEEP_KEEP_TEST_UNDEFINED}", "p-");
        assert!(keep.is_empty());
    }

    #[test]
    fn duplicate_entries_are_stored_once() {
        let mut keep = KeepList::new();
        keep.add_folder("/tmp/a");
        keep.add_folder("/tmp/./a/");
        keep.add_folder_prefix("/tmp", "p-");
        keep.add_folder_prefix("/tmp/", "p-");
        assert_eq!(keep.len(), 2);
    }

    #[test]
    fn has_entry_within_finds_nested_entries() {
        let mut keep = KeepList::new();
        keep.add_folder("/tmp/a/b");
        keep.add_folder_prefix("/tmp/c", "p-");

        assert!(keep.has_entry_within(Path::new("/tmp")));
        assert!(keep.has_entry_within(Path::new("/tmp/a")));
        assert!(keep.has_entry_within(Path::new("/tmp/a/b")));
        assert!(keep.has_entry_within(Path::new("/tmp/c")));
        assert!(!keep.has_entry_within(Path::new("/tmp/a/b/c")));
        assert!(!keep.has_entry_within(Path::new("/tmp/d")));
    }

    #[cfg(unix)]
    #[test]
    fn keeps_non_utf8_paths() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(OsString::from_vec(b"/tmp/sweep-\xFF".to_vec()));
        let mut keep = KeepList::new();
        keep.add_folder(&path);
        assert!(keep.contains(&path));
        assert!(keep.contains(&path.join("child")));
    }

    #[cfg(unix)]
    #[test]
    fn protected_roots_win_over_user_keep() {
        let mut keep = KeepList::new();
        keep.add_folder("/etc");
        let guard = Guard::new(keep);
        assert!(matches!(
            guard.check(Path::new("/etc"), false),
            Err(Error::ProtectedPath(_))
        ));
        assert!(matches!(
            guard.check(Path::new("/etc/passwd"), false),
            Err(Error::ProtectedPath(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn user_keep_applies_below_unprotected_nodes() {
        let mut keep = KeepList::new();
        keep.add_folder("/etc/sweep-keep-test");
        let guard = Guard::new(keep);
        assert!(matches!(
            guard.check(Path::new("/etc/sweep-keep-test/file"), false),
            Err(Error::Aborted)
        ));
    }

    #[test]
    fn user_data_subtrees_are_protected() {
        if let Some(home) = platform::home_dir() {
            let guard = Guard::new(KeepList::new());
            assert!(matches!(
                guard.check(&home.join("Documents").join("thesis.docx"), false),
                Err(Error::ProtectedPath(_))
            ));
        }
    }

    #[test]
    fn ancestors_of_protected_roots_are_protected() {
        if let Some(home) = platform::home_dir() {
            if let Some(parent) = home.parent().filter(|p| !p.as_os_str().is_empty()) {
                let guard = Guard::new(KeepList::new());
                assert!(matches!(
                    guard.check(parent, false),
                    Err(Error::ProtectedPath(_))
                ));
            }
        }
    }

    #[test]
    fn working_directory_style_paths_are_rejected() {
        let guard = Guard::new(KeepList::new());
        for text in [".", "", "..", "../x"] {
            assert!(
                matches!(
                    guard.check(Path::new(text), false),
                    Err(Error::ProtectedPath(_))
                ),
                "{text:?}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_temp_contents_are_cleanable() {
        if let Some(temp) = std::env::var_os("TEMP") {
            let guard = Guard::new(KeepList::new());
            let child = Path::new(&temp).join("sweep-guard-test.tmp");
            let checked = guard.check(&child, false);
            assert!(checked.is_ok(), "{child:?}: {checked:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn ancestor_symlink_escape_is_rejected() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("sweep-guard-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let link = base.join("escape");
        symlink("/etc", &link).unwrap();

        let guard = Guard::new(KeepList::new());
        assert!(guard.check(&link.join("passwd"), false).is_err());

        fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ancestor_symlinks_need_follow_links() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("sweep-guard-follow-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("target")).unwrap();
        fs::write(base.join("target").join("file"), b"x").unwrap();
        symlink(base.join("target"), base.join("link")).unwrap();

        let guard = Guard::new(KeepList::new());
        let through_link = base.join("link").join("file");
        assert!(matches!(
            guard.check(&through_link, false),
            Err(Error::Link(_))
        ));
        assert!(guard.check(&through_link, true).is_ok());

        fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn missing_leaf_resolves_through_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("sweep-guard-lenient-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("target")).unwrap();
        symlink(base.join("target"), base.join("link")).unwrap();

        let resolved =
            canonical_lenient(&base.join("link").join("missing").join("deeper")).unwrap();
        let expected = fs::canonicalize(base.join("target"))
            .unwrap()
            .join("missing")
            .join("deeper");
        assert_eq!(resolved, expected);

        fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn detects_links() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("sweep-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let target = base.join("target");
        fs::write(&target, b"x").unwrap();
        let link = base.join("link");
        symlink(&target, &link).unwrap();

        assert!(is_link(&link));
        assert!(!is_link(&target));

        fs::remove_dir_all(&base).unwrap();
    }
}
