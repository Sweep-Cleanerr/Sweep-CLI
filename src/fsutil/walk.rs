//! Directory traversal and glob resolution.
//!
//! Rules inherited from BleachBit:
//!
//! * links and junctions inside the tree are never descended into — a cleaner
//!   must not escape its declared tree;
//! * a root that is itself a link IS followed (the cleaner declared that exact
//!   path), but results are reported under the declared name so patterns and
//!   delete targets still line up;
//! * directories are emitted after their contents, so a `walk.all` delete can
//!   remove children before the parent;
//! * one unreadable subdirectory never aborts the whole scan.

use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};
use walkdir::WalkDir;

use crate::core::error::{Error, Result};
use crate::platform;

/// Options for a directory scan.
#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Also yield directories, after their contents (`contents_first`).
    pub include_dirs: bool,
    /// Follow symlinks — almost always `false`.
    pub follow_links: bool,
    /// Maximum recursion depth (`None` = unlimited).
    pub max_depth: Option<usize>,
    /// Stay on the root's filesystem; the deletion path enables this so a
    /// volume mounted under the target is not emptied along with it.
    pub same_filesystem: bool,
    /// Yield the root itself instead of only its children.
    pub include_root: bool,
}

impl ScanOptions {
    /// Files only, unlimited depth, no link following.
    pub fn files() -> Self {
        Self::default()
    }

    /// Files and directories, directories last.
    pub fn all() -> Self {
        Self {
            include_dirs: true,
            ..Self::default()
        }
    }

    pub fn with_max_depth(mut self, depth: usize) -> Self {
        self.max_depth = Some(depth);
        self
    }

    pub fn with_same_filesystem(mut self, yes: bool) -> Self {
        self.same_filesystem = yes;
        self
    }
}

/// Does the pattern contain a wildcard? A `search="glob"` action with a
/// literal path falls back to a plain existence check instead.
pub fn has_glob(pattern: &str) -> bool {
    pattern.contains(['*', '?', '[', ']', '{'])
}

/// Separator-normalised pattern shape: `/` separators, no leading `./`.
///
/// On Windows `\` is a separator and is rewritten; on Unix it is the glob
/// escape character (`file\*.log` names a literal `file*.log`) and must survive.
fn normalize_pattern(pattern: &str) -> String {
    let s = if cfg!(windows) {
        pattern.replace('\\', "/")
    } else {
        pattern.to_string()
    };
    s.trim_start_matches("./").to_string()
}

/// The [`normalize_pattern`] counterpart for a candidate path.
fn normalize_candidate(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.into_owned()
    };
    s.trim_start_matches("./").to_string()
}

fn build_matcher(pattern: &str) -> Result<GlobMatcher> {
    let glob = GlobBuilder::new(&normalize_pattern(pattern))
        // BleachBit's glob.iglob() semantics: `*` never crosses a separator,
        // `**` does.
        .literal_separator(true)
        .case_insensitive(cfg!(windows))
        .backslash_escape(true)
        .build()
        .map_err(|source| Error::Glob {
            pattern: pattern.to_string(),
            source,
        })?;
    Ok(glob.compile_matcher())
}

/// Split the pattern into its literal walk root, whether a wildcard was seen,
/// and the number of literal components (the depth where matching may start).
///
/// Walking from the literal prefix turns an O(filesystem) glob into an
/// O(subtree) glob.
fn split_glob_root(pattern: &str) -> (PathBuf, bool, usize) {
    let unix = normalize_pattern(pattern);
    let mut literal: Vec<String> = Vec::new();
    let mut wildcard_seen = false;

    for part in unix.split('/') {
        if part.is_empty() {
            continue;
        }
        if has_glob(part) {
            wildcard_seen = true;
            break;
        }
        literal.push(part.to_string());
    }
    let literal_len = literal.len();

    if literal.is_empty() {
        // Bare `*.log` / `sub/*` style: walk from "." and let the matcher decide.
        return (PathBuf::from("."), wildcard_seen, literal_len);
    }

    // A Windows drive letter must stay an *absolute* root: pushing `C:` as a
    // plain component yields a drive-relative path that resolves against the
    // process CWD on that drive — the glob would silently walk the wrong tree.
    let mut root = if literal[0].len() == 2
        && literal[0].as_bytes()[1] == b':'
        && literal[0].as_bytes()[0].is_ascii_alphabetic()
    {
        let mut base = literal.remove(0);
        base.push(std::path::MAIN_SEPARATOR);
        PathBuf::from(base)
    } else if unix.starts_with("//") {
        PathBuf::from("//")
    } else if unix.starts_with('/') {
        PathBuf::from(std::path::MAIN_SEPARATOR.to_string())
    } else {
        PathBuf::new()
    };

    for part in literal {
        root.push(part);
    }

    (root, wildcard_seen, literal_len)
}

/// Depth-bounded prefix matchers that prune subtrees which cannot lead to a
/// match. Without this, a pattern like `$USERPROFILE\tor-browser*\Browser\…`
/// walks the entire profile tree (AppData, .cargo, node_modules …) just to
/// find nothing — minutes on a real machine.
///
/// Pruning is an optimisation, never a filter: every decision is `true` unless
/// the subtree is *certain* to hold no match. Depths past the pattern end are
/// not pruned, and a `**` component breaks the one-component-per-depth
/// alignment, so pruning stops once a depth would reach it.
struct PrefixPruner {
    /// `matchers[want]` matches the pattern's first `want` components.
    matchers: Vec<Option<GlobMatcher>>,
    literal_len: usize,
    /// Index of the first `**` component, if any.
    star_index: Option<usize>,
}

impl PrefixPruner {
    /// `normalized_pattern` must already be [`normalize_pattern`]-shaped.
    fn new(normalized_pattern: &str, literal_len: usize) -> Self {
        let components: Vec<&str> = normalized_pattern
            .split('/')
            .filter(|c| !c.is_empty())
            .collect();
        let star_index = components.iter().position(|c| *c == "**");
        let mut matchers: Vec<Option<GlobMatcher>> = (0..=components.len()).map(|_| None).collect();
        for want in literal_len + 1..=components.len() {
            // Truncation happens at a component boundary, so `{…}`/`[…]`
            // quoting stays balanced.
            let matcher = GlobBuilder::new(&components[..want].join("/"))
                .literal_separator(true)
                .case_insensitive(cfg!(windows))
                .backslash_escape(true)
                .build()
                .map(|glob| glob.compile_matcher());
            matchers[want] = matcher.ok();
        }
        Self {
            matchers,
            literal_len,
            star_index,
        }
    }

    /// May the walk enter `candidate` at `depth` (0 = the walk root itself)?
    /// `candidate` is the reported path, so a symlinked root is compared under
    /// the declared name.
    fn allows(&self, depth: usize, candidate: &Path) -> bool {
        if depth == 0 {
            return true;
        }
        if self
            .star_index
            .is_some_and(|star| depth + self.literal_len > star)
        {
            return true;
        }
        let Some(matcher) = self
            .matchers
            .get(self.literal_len + depth)
            .and_then(Option::as_ref)
        else {
            return true;
        };
        let normalized = normalize_candidate(candidate);
        let components: Vec<&str> = normalized.split('/').filter(|c| !c.is_empty()).collect();
        // A candidate that does not line up with the pattern (symlink rewrite,
        // trailing separator) is not pruned either.
        if components.len() != self.literal_len + depth {
            return true;
        }
        matcher.is_match(components.join("/"))
    }
}

/// Decide where to walk and under which name results must be reported.
///
/// A root that is itself a symlink or junction is followed — the cleaner
/// declared exactly that path, so its target is the declared tree — and
/// everything found is reported under the declared name. Returns
/// `(walk_root, Some(display_root))` when a rewrite is needed.
fn walk_root_for(root: &Path) -> (PathBuf, Option<PathBuf>) {
    let is_link = root
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
        || platform::is_reparse_point(root);
    if !is_link {
        return (root.to_path_buf(), None);
    }
    match std::fs::canonicalize(root) {
        // Dangling link, or already canonical: walk as-is.
        Ok(real) if real != root => (real, Some(root.to_path_buf())),
        _ => (root.to_path_buf(), None),
    }
}

/// Map a path found under `walk_root` back to the declared name.
fn report_path(entry: &Path, walk_root: &Path, rewrite_to: Option<&Path>) -> PathBuf {
    match rewrite_to {
        Some(display_root) => display_root.join(entry.strip_prefix(walk_root).unwrap_or(entry)),
        None => entry.to_path_buf(),
    }
}

/// May the walk enter (and yield) this entry? Links are never followed, and a
/// directory on another volume is not descended into.
///
/// The volume test runs for directories only: `device_id` opens a handle, and
/// paying that per file made full previews take minutes on Windows. A file
/// cannot be on a different volume than its parent directory, so pruning the
/// subtree at the directory is both cheaper and stricter.
fn descendable(entry: &walkdir::DirEntry, root_dev: Option<u64>, follow_links: bool) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    // follow_links(false) covers POSIX symlinks; Windows junctions need the
    // explicit reparse-point check.
    if !follow_links && platform::is_reparse_point(entry.path()) {
        return false;
    }
    if entry.file_type().is_dir() {
        if let (Some(want), Some(got)) = (root_dev, platform::device_id(entry.path())) {
            return want == got;
        }
    }
    true
}

/// Shared walker setup: link-root rewrite, depth and filesystem limits. The
/// device id comes from the *walk* root so a symlinked root compares against
/// the filesystem it actually resolves to.
///
/// The walker stays parents-first: with `contents_first`, walkdir's
/// `filter_entry` runs after descent (so it cannot prune), and rejecting a
/// deferred directory pops the *parent's* list, silently dropping later
/// siblings. Callers restore contents-first output order themselves.
fn prepare(
    root: &Path,
    options: &ScanOptions,
) -> (WalkDir, PathBuf, Option<PathBuf>, Option<u64>) {
    let (walk_root, rewrite_to) = walk_root_for(root);
    let mut walker = WalkDir::new(&walk_root)
        .follow_links(options.follow_links)
        .min_depth(if options.include_root { 0 } else { 1 })
        .same_file_system(options.same_filesystem);
    if let Some(depth) = options.max_depth {
        walker = walker.max_depth(depth);
    }
    let root_dev = if options.same_filesystem {
        platform::device_id(&walk_root)
    } else {
        None
    };
    (walker, walk_root, rewrite_to, root_dev)
}

/// Expand a glob pattern into matching paths, directories after their
/// contents. Always stays on the root's filesystem: `~/.cache/**/*.log` must
/// not reach into a volume mounted underneath it.
pub fn glob_paths(pattern: &str) -> Result<Vec<PathBuf>> {
    if !has_glob(pattern) {
        let path = PathBuf::from(pattern);
        return Ok(if path.symlink_metadata().is_ok() {
            vec![path]
        } else {
            Vec::new()
        });
    }

    let matcher = build_matcher(pattern)?;
    let normalized = normalize_pattern(pattern);
    let (root, _, literal_len) = split_glob_root(pattern);
    if !root.exists() {
        return Ok(Vec::new());
    }

    // Without `**` the deepest match is the pattern's own length. Bounding
    // the walk stops profile discovery like `User Data/*` from descending
    // into every cache below the matched profile directories.
    let components = normalized.split('/').filter(|part| !part.is_empty()).count();
    let max_depth = if normalized.contains("**") {
        None
    } else {
        Some(components.saturating_sub(literal_len))
    };

    let options = ScanOptions {
        include_root: true,
        same_filesystem: true,
        max_depth,
        ..ScanOptions::default()
    };
    let (walker, walk_root, rewrite_to, root_dev) = prepare(&root, &options);
    let pruner = PrefixPruner::new(&normalized, literal_len);

    let mut files = Vec::new();
    let mut dirs = Vec::new();

    for entry in walker
        .into_iter()
        .filter_entry(|e| {
            // Prune first: a directory that cannot lead to a match is never
            // entered (and never pays for a device_id handle either).
            if e.depth() > 0
                && e.file_type().is_dir()
                && !pruner.allows(
                    e.depth(),
                    &report_path(e.path(), &walk_root, rewrite_to.as_deref()),
                )
            {
                return false;
            }
            descendable(e, root_dev, false)
        })
        .filter_map(|e| e.ok())
    {
        let candidate = report_path(entry.path(), &walk_root, rewrite_to.as_deref());
        if !matcher.is_match(normalize_candidate(&candidate)) {
            continue;
        }
        if entry.file_type().is_dir() {
            dirs.push(candidate);
        } else {
            files.push(candidate);
        }
    }

    // Parents-first collection → deepest-first dirs, so deleting the list in
    // order removes children before parents.
    dirs.reverse();
    files.extend(dirs);
    Ok(files)
}

/// Like [`glob_paths`] but keeps only directories (`<var search="glob">`).
pub fn glob_dirs(pattern: &str) -> Result<Vec<PathBuf>> {
    Ok(glob_paths(pattern)?
        .into_iter()
        .filter(|p| p.is_dir())
        .collect())
}

/// Everything inside `root` (not `root` itself) — BleachBit's
/// `children_in_directory()`.
pub fn children(root: &Path, include_dirs: bool) -> Vec<PathBuf> {
    let options = ScanOptions {
        include_dirs,
        ..ScanOptions::default()
    };
    scan_paths(root, &options)
}

/// Stream the entries of `root` in contents-first order without collecting
/// them, keeping peak memory at walk depth plus the caller's own state.
///
/// `f` returns `false` to stop early; the function then returns `false` too.
pub fn walk_paths<F>(root: &Path, options: &ScanOptions, mut f: F) -> bool
where
    F: FnMut(PathBuf) -> bool,
{
    if root.symlink_metadata().is_err() {
        return true;
    }
    let (walker, walk_root, rewrite_to, root_dev) = prepare(root, options);

    // The walker runs parents-first so filter_entry can prune descent;
    // directories are deferred here to restore contents-first yield order.
    let mut pending: Vec<(usize, PathBuf)> = Vec::new();
    for entry in walker
        .into_iter()
        .filter_entry(|e| descendable(e, root_dev, options.follow_links))
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                log::debug!("skipping unreadable entry: {err}");
                continue;
            }
        };
        let depth = entry.depth();
        // A pending dir at or below this entry's depth has had its whole
        // subtree walked: it is complete and can be yielded.
        while pending.last().is_some_and(|(d, _)| *d >= depth) {
            let (_, dir) = pending.pop().expect("pending is non-empty");
            if !f(dir) {
                return false;
            }
        }
        let reported = report_path(entry.path(), &walk_root, rewrite_to.as_deref());
        if entry.file_type().is_dir() {
            if options.include_dirs {
                pending.push((depth, reported));
            }
        } else if !f(reported) {
            return false;
        }
    }
    while let Some((_, dir)) = pending.pop() {
        if !f(dir) {
            return false;
        }
    }
    true
}

/// Full scan of `root` according to `options`: all files first, then
/// directories deepest-first, so deleting the returned list in order works.
pub fn scan_paths(root: &Path, options: &ScanOptions) -> Vec<PathBuf> {
    if root.symlink_metadata().is_err() {
        return Vec::new();
    }
    let (walker, walk_root, rewrite_to, root_dev) = prepare(root, options);

    let mut files = Vec::new();
    let mut dirs = Vec::new();

    for entry in walker
        .into_iter()
        .filter_entry(|e| descendable(e, root_dev, options.follow_links))
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                // A single unreadable directory must not abort the scan.
                log::debug!("skipping unreadable entry: {err}");
                continue;
            }
        };
        let reported = report_path(entry.path(), &walk_root, rewrite_to.as_deref());
        if entry.file_type().is_dir() {
            if options.include_dirs {
                dirs.push(reported);
            }
        } else {
            files.push(reported);
        }
    }

    // Parents-first collection → deepest-first dirs for delete order.
    dirs.reverse();
    files.extend(dirs);
    files
}

/// Parallel variant: the scan stays single threaded (directory ordering
/// matters) but the per-entry work in `f` runs on the rayon pool. Results are
/// collected in scan order.
pub fn scan_paths_parallel<T, F>(root: &Path, options: &ScanOptions, f: F) -> Vec<T>
where
    T: Send,
    F: Fn(&Path) -> T + Send + Sync,
{
    use rayon::prelude::*;
    scan_paths(root, options)
        .par_iter()
        .map(|p| f(p.as_path()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises the test that depends on the process-wide current directory.
    static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Create a directory link; `false` when the platform refuses, so callers
    /// skip instead of failing. Windows symlinks need elevation — junctions
    /// do not, so they are the fallback that keeps link tests executing.
    fn make_link(target: &Path, link: &Path) -> bool {
        let _ = std::fs::remove_file(link);
        let _ = std::fs::remove_dir_all(link);
        #[cfg(unix)]
        return std::os::unix::fs::symlink(target, link).is_ok();
        #[cfg(windows)]
        {
            let made = std::os::windows::fs::symlink_dir(target, link).is_ok()
                || crate::junction(target, link);
            if !made {
                eprintln!("skipping link test: symlink and junction creation refused");
            }
            made
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link);
            false
        }
    }

    fn remove_link(link: &Path) {
        let _ = std::fs::remove_file(link);
        let _ = std::fs::remove_dir_all(link);
    }

    #[test]
    fn has_glob_detects_wildcards() {
        assert!(has_glob("a*.log"));
        assert!(has_glob("a?b"));
        assert!(has_glob("a[0-9]"));
        assert!(has_glob("a{b,c}"));
        assert!(!has_glob("a/b/c.log"));
        assert!(!has_glob(""));
    }

    #[test]
    fn scan_yields_files_before_dirs_deepest_first() {
        let base = crate::fixture("walk_order");
        std::fs::create_dir_all(base.join("sub/deep")).unwrap();
        std::fs::write(base.join("a.txt"), b"x").unwrap();
        std::fs::write(base.join("sub/b.txt"), b"x").unwrap();
        std::fs::write(base.join("sub/deep/c.txt"), b"x").unwrap();

        let found = scan_paths(&base, &ScanOptions::all());
        assert_eq!(found.len(), 5);
        let first_dir = found.iter().position(|p| p.is_dir()).unwrap();
        assert_eq!(first_dir, 3, "all files must precede directories: {found:?}");
        assert_eq!(found[3], base.join("sub/deep"));
        assert_eq!(found[4], base.join("sub"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn scan_respects_depth_and_root_options() {
        let base = crate::fixture("walk_depth");
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::write(base.join("top.txt"), b"x").unwrap();
        std::fs::write(base.join("sub/inner.txt"), b"x").unwrap();

        let shallow = scan_paths(&base, &ScanOptions::files().with_max_depth(1));
        assert_eq!(shallow, vec![base.join("top.txt")]);

        let with_root = scan_paths(&base, &ScanOptions {
            include_root: true,
            include_dirs: true,
            ..ScanOptions::default()
        });
        assert_eq!(with_root.last(), Some(&base), "root must come last: {with_root:?}");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_root_scans_empty() {
        let base = crate::fixture("walk_missing");
        assert!(scan_paths(&base.join("nope"), &ScanOptions::all()).is_empty());
        assert!(children(&base.join("nope"), true).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn walk_paths_stops_early() {
        let base = crate::fixture("walk_early");
        std::fs::write(base.join("a.txt"), b"x").unwrap();
        std::fs::write(base.join("b.txt"), b"x").unwrap();
        let mut count = 0;
        let completed = walk_paths(&base, &ScanOptions::files(), |_| {
            count += 1;
            false
        });
        assert!(!completed);
        assert_eq!(count, 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Streaming must keep contents-first order: a dir arrives only after
    /// everything inside it, so a stream-and-delete caller works.
    #[test]
    fn walk_paths_streams_contents_first() {
        let base = crate::fixture("walk_stream_order");
        std::fs::create_dir_all(base.join("sub/deep")).unwrap();
        std::fs::write(base.join("sub/deep/c.txt"), b"x").unwrap();
        std::fs::write(base.join("sub/b.txt"), b"x").unwrap();

        let mut seen: Vec<PathBuf> = Vec::new();
        assert!(walk_paths(&base, &ScanOptions::all(), |p| {
            seen.push(p);
            true
        }));
        let pos = |name: &str| seen.iter().position(|p| p.ends_with(name)).unwrap();
        assert!(pos("c.txt") < pos("deep"));
        assert!(pos("b.txt") < pos("sub"));
        assert!(pos("deep") < pos("sub"));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A Windows drive-letter pattern must walk the directory exactly as
    /// written, not a drive-relative path resolved against the process CWD.
    #[test]
    fn glob_preserves_absolute_root() {
        let base = crate::fixture("walk_glob_root");
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::write(base.join("sub/a.log"), b"x").unwrap();

        let pattern = base.join("sub/*.log").to_string_lossy().into_owned();
        let hits = glob_paths(&pattern).unwrap();
        assert!(
            hits.iter().any(|p| p.ends_with("a.log")),
            "absolute glob '{pattern}' matched nothing: {hits:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// walkdir reports `./a.log` while the bare pattern says `a.log`; the
    /// `./` must not make every bare pattern match nothing.
    #[test]
    fn glob_matches_bare_relative_pattern() {
        let base = crate::fixture("walk_glob_bare");
        std::fs::write(base.join("a.sweepbare"), b"x").unwrap();

        let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&base).unwrap();
        let result = glob_paths("*.sweepbare");
        std::env::set_current_dir(&previous).unwrap();

        let hits = result.unwrap();
        assert!(
            hits.iter().any(|p| p.ends_with("a.sweepbare")),
            "bare relative glob matched nothing: {hits:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn glob_literal_path_is_an_existence_check() {
        let base = crate::fixture("walk_glob_literal");
        let file = base.join("plain.txt");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(glob_paths(&file.to_string_lossy()).unwrap(), vec![file.clone()]);
        assert!(glob_paths(&base.join("gone.txt").to_string_lossy())
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The wildcard is the first component below the root: the pruner must
    /// still find the match (and not descend into unrelated subtrees).
    #[test]
    fn glob_wildcard_prefix_matches() {
        let base = crate::fixture("walk_glob_prefix");
        std::fs::create_dir_all(base.join("tor-browser1/Browser")).unwrap();
        std::fs::create_dir_all(base.join("other/Browser")).unwrap();
        std::fs::write(base.join("tor-browser1/Browser/profile.default"), b"x").unwrap();
        std::fs::write(base.join("other/Browser/profile.default"), b"x").unwrap();

        let pattern = format!("{}\\tor*\\Browser\\*.default", base.to_string_lossy());
        let pattern = if cfg!(windows) {
            pattern
        } else {
            pattern.replace('\\', "/")
        };
        let hits = glob_paths(&pattern).unwrap();
        assert_eq!(hits.len(), 1, "pruner lost or over-matched: {hits:?}");
        assert!(hits[0].ends_with("profile.default"));
        assert!(hits[0].to_string_lossy().contains("tor-browser1"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn glob_double_star_and_dirs_filter() {
        let base = crate::fixture("walk_glob_star");
        std::fs::create_dir_all(base.join("sub/deep")).unwrap();
        std::fs::write(base.join("sub/deep/a.log"), b"x").unwrap();
        std::fs::write(base.join("sub/b.txt"), b"x").unwrap();

        let pattern = format!("{}/**/*.log", base.to_string_lossy());
        let hits = glob_paths(&pattern).unwrap();
        assert!(
            hits.iter().any(|p| p.ends_with("a.log")),
            "** glob lost sub/deep/a.log: {hits:?}"
        );
        assert!(!hits.iter().any(|p| p.ends_with("b.txt")));

        let dir_pattern = format!("{}/*/deep", base.to_string_lossy());
        let dirs = glob_dirs(&dir_pattern).unwrap();
        assert_eq!(dirs, vec![base.join("sub/deep")]);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn same_filesystem_keeps_children_on_the_root_device() {
        let base = crate::fixture("walk_samefs");
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::write(base.join("sub/a.txt"), b"x").unwrap();
        std::fs::write(base.join("b.txt"), b"y").unwrap();

        let found = scan_paths(&base, &ScanOptions::files().with_same_filesystem(true));
        assert_eq!(found.len(), 2, "same-filesystem scan lost files: {found:?}");

        let pattern = format!("{}/**/*.txt", base.to_string_lossy());
        let globbed = glob_paths(&pattern).unwrap();
        assert!(
            globbed.iter().any(|p| p.ends_with("a.txt")),
            "same-filesystem glob lost sub/a.txt: {globbed:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn scan_never_follows_links() {
        let base = crate::fixture("walk_link");
        let outside = crate::fixture("walk_link_outside");
        std::fs::create_dir_all(base.join("real/inner")).unwrap();
        std::fs::write(base.join("real/inner/f.txt"), b"data").unwrap();
        std::fs::write(outside.join("secret.txt"), b"secret").unwrap();

        let link = base.join("link");
        if !make_link(&outside, &link) {
            let _ = std::fs::remove_dir_all(&base);
            let _ = std::fs::remove_dir_all(&outside);
            return;
        }

        let found = scan_paths(&base, &ScanOptions::all());
        assert!(
            !found.iter().any(|p| p.ends_with("secret.txt")),
            "walk escaped the declared root through a link: {found:?}"
        );

        remove_link(&link);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// A cleaner root that is itself a link is still scanned, and results
    /// keep the declared name so patterns and delete targets line up.
    #[test]
    fn symlinked_root_is_reported_under_the_declared_name() {
        let target = crate::fixture("walk_linkroot_target");
        let base = crate::fixture("walk_linkroot_link");
        remove_link(&base);
        std::fs::create_dir_all(target.join("inner")).unwrap();
        std::fs::write(target.join("t.txt"), b"x").unwrap();
        std::fs::write(target.join("inner/f.txt"), b"y").unwrap();

        if !make_link(&target, &base) {
            let _ = std::fs::remove_dir_all(&target);
            let _ = std::fs::remove_dir_all(&base);
            return;
        }

        let found = scan_paths(&base, &ScanOptions::files());
        assert!(
            found.iter().any(|p| p.ends_with("f.txt")),
            "symlinked root was silently skipped: {found:?}"
        );
        assert!(
            found.iter().all(|p| p.starts_with(&base)),
            "results must use the declared root name: {found:?}"
        );

        let pattern = format!("{}/*.txt", base.to_string_lossy());
        let hits = glob_paths(&pattern).unwrap();
        assert!(
            hits.iter().any(|p| p.ends_with("t.txt")),
            "glob through a symlinked root matched nothing: {hits:?}"
        );
        assert!(hits.iter().all(|p| p.starts_with(&base)));

        remove_link(&base);
        let _ = std::fs::remove_dir_all(&target);
    }
}
/// All directory children under `root` up to `max_depth` deep.
pub fn child_dirs(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    scan_paths(root, &ScanOptions::all().with_max_depth(max_depth))
        .into_iter()
        .filter(|p| p.is_dir())
        .collect::<Vec<_>>()
        .into_iter()
        .for_each(|p| out.push(p));
    out
}

/// True when `path` is a directory with no entries.
pub fn is_empty_dir(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    match std::fs::read_dir(path) {
        Ok(mut rd) => rd.next().is_none(),
        Err(_) => false,
    }
}
