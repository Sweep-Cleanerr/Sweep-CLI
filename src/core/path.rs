//! Lexical path handling shared by every definition source.
//!
//! Definitions may use either separator and environment variables. Paths are
//! expanded once, then normalised without touching the filesystem.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf, MAIN_SEPARATOR};

use crate::platform;

/// Filesystem case sensitivity is a platform property, not a per-path probe.
pub fn case_sensitive() -> bool {
    !cfg!(any(windows, target_os = "macos", target_os = "ios"))
}

fn push_char(out: &mut String, ch: char) {
    if ch == '/' || ch == '\\' {
        out.push(MAIN_SEPARATOR);
    } else {
        out.push(ch);
    }
}

fn env_name_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn env_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// Names accepted between `${` `}` and between two `%`.
fn delimited_name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.chars().any(|ch| {
            matches!(ch, '/' | '\\' | '%' | '$' | '{' | '}' | '=' | '\0') || ch.is_whitespace()
        })
}

/// An empty value is treated as unresolved: `$CACHE/x` must never silently
/// turn into `/x`.
fn lookup_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn expand_inner(input: &str) -> (String, bool) {
    let mut out = String::with_capacity(input.len());
    let mut incomplete = false;
    let mut index = 0;

    if let Some(rest) = input.strip_prefix('~') {
        let next = rest.chars().next();
        let is_separator = matches!(next, Some('/') | Some('\\'));
        if next.is_none() || is_separator {
            if let Some(home) = platform::home_dir() {
                out.push_str(&home.to_string_lossy());
                index = 1;
                if is_separator {
                    if !out.ends_with(MAIN_SEPARATOR) {
                        out.push(MAIN_SEPARATOR);
                    }
                    index += 1;
                }
            }
        }
    }

    while index < input.len() {
        let rest = &input[index..];
        let Some(ch) = rest.chars().next() else {
            break;
        };

        if ch == '$' {
            if let Some(after) = rest.strip_prefix("${") {
                if let Some(end) = after.find('}') {
                    let name = &after[..end];
                    if delimited_name_ok(name) {
                        let total = 3 + end;
                        match lookup_env(name) {
                            Some(value) => out.push_str(&value),
                            None => {
                                incomplete = true;
                                out.push_str(&rest[..total]);
                            }
                        }
                        index += total;
                        continue;
                    }
                }
                out.push('$');
                index += 1;
                continue;
            }

            let after = &rest[1..];
            if after.chars().next().is_some_and(env_name_start) {
                let len = after
                    .find(|c: char| !env_name_char(c))
                    .unwrap_or(after.len());
                let name = &after[..len];
                match lookup_env(name) {
                    Some(value) => out.push_str(&value),
                    None => {
                        incomplete = true;
                        out.push_str(&rest[..1 + len]);
                    }
                }
                index += 1 + len;
            } else {
                out.push('$');
                index += 1;
            }
            continue;
        }

        if ch == '%' {
            if let Some(end) = rest[1..].find('%') {
                let name = &rest[1..1 + end];
                if delimited_name_ok(name) {
                    let total = 2 + end;
                    match lookup_env(name) {
                        Some(value) => out.push_str(&value),
                        None => {
                            incomplete = true;
                            out.push_str(&rest[..total]);
                        }
                    }
                    index += total;
                    continue;
                }
            }
            out.push('%');
            index += 1;
            continue;
        }

        push_char(&mut out, ch);
        index += ch.len_utf8();
    }

    (out, incomplete)
}

/// Expand `~`, `$NAME`, `${NAME}` and `%NAME%` once. Unresolved references are
/// kept verbatim so a definition can still be inspected; they are reported
/// through [`expand_defined`].
pub fn expand(input: &str) -> PathBuf {
    PathBuf::from(expand_inner(input).0)
}

/// Like [`expand`], but returns `None` when any reference could not resolve.
pub fn expand_defined(input: &str) -> Option<PathBuf> {
    let (expanded, incomplete) = expand_inner(input);
    if incomplete {
        None
    } else {
        Some(PathBuf::from(expanded))
    }
}

/// [`expand`] followed by [`normalize`].
pub fn expand_normalized(input: &str) -> PathBuf {
    normalize(&expand(input))
}

/// Lexical normalisation: remove `.` and duplicate separators, resolve `..`
/// against preceding normal components. Never touches the filesystem.
pub fn normalize(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.as_os_str().to_string_lossy();
        if text.starts_with(r"\\?\") || text.starts_with(r"\\.\") {
            return path.to_path_buf();
        }
    }

    let mut parts: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match parts.last() {
                Some(Component::Normal(_)) => {
                    parts.pop();
                }
                Some(Component::RootDir) => {}
                _ => parts.push(component),
            },
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                parts.push(component)
            }
        }
    }

    if parts.is_empty() {
        PathBuf::from(".")
    } else {
        parts.iter().collect()
    }
}

/// The form every comparison works on: normalised, and with the Windows
/// verbatim prefix removed so `\\?\C:\a` and `C:\a` compare equal.
pub fn comparable(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        normalize(&extended_path_undo(path))
    }
    #[cfg(not(windows))]
    {
        normalize(path)
    }
}

fn comparable_parts(path: &Path) -> impl Iterator<Item = Component<'_>> {
    path.components()
        .filter(|component| !matches!(component, Component::CurDir))
}

fn os_eq(a: &OsStr, b: &OsStr, case_sensitive: bool) -> bool {
    if a == b {
        return true;
    }
    if case_sensitive {
        return false;
    }
    match (a.to_str(), b.to_str()) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase(),
        _ => false,
    }
}

fn eq_component(a: &Component<'_>, b: &Component<'_>, case_sensitive: bool) -> bool {
    match (a, b) {
        (Component::Prefix(a), Component::Prefix(b)) => {
            os_eq(a.as_os_str(), b.as_os_str(), case_sensitive)
        }
        (Component::RootDir, Component::RootDir) => true,
        (Component::CurDir, Component::CurDir) => true,
        (Component::ParentDir, Component::ParentDir) => true,
        (Component::Normal(a), Component::Normal(b)) => os_eq(a, b, case_sensitive),
        _ => false,
    }
}

/// Component-wise equality of two paths that are already [`comparable`].
pub fn equal_prepared(a: &Path, b: &Path, case_sensitive: bool) -> bool {
    let mut left = comparable_parts(a);
    let mut right = comparable_parts(b);
    loop {
        match (left.next(), right.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) if eq_component(&x, &y, case_sensitive) => {}
            _ => return false,
        }
    }
}

/// Component-wise containment of two paths that are already [`comparable`].
/// An empty base (`.`) contains every relative path that does not climb out.
pub fn starts_with_prepared(path: &Path, base: &Path, case_sensitive: bool) -> bool {
    let mut path_parts = comparable_parts(path).peekable();
    let mut base_parts = comparable_parts(base).peekable();

    if base_parts.peek().is_none() {
        return path_parts
            .peek()
            .map_or(true, |c| matches!(c, Component::Normal(_)));
    }

    for base_part in base_parts {
        match path_parts.next() {
            Some(part) if eq_component(&base_part, &part, case_sensitive) => {}
            _ => return false,
        }
    }
    true
}

pub fn path_equal_cs(a: &Path, b: &Path) -> bool {
    equal_prepared(&comparable(a), &comparable(b), true)
}

pub fn path_equal(a: &Path, b: &Path) -> bool {
    equal_prepared(&comparable(a), &comparable(b), case_sensitive())
}

pub fn path_starts_with_cs(path: &Path, base: &Path) -> bool {
    starts_with_prepared(&comparable(path), &comparable(base), true)
}

pub fn path_starts_with(path: &Path, base: &Path) -> bool {
    starts_with_prepared(&comparable(path), &comparable(base), case_sensitive())
}

fn unify_separators(path: &Path) -> PathBuf {
    match path.to_str() {
        Some(text) if text.contains('\\') => PathBuf::from(text.replace('\\', "/")),
        _ => path.to_path_buf(),
    }
}

/// Lexically join `extra` to `base`, clamping parent and absolute fragments so
/// the result can never escape `base`. Both separators are honoured in `extra`.
pub fn join_opt(base: &Path, extra: &Path) -> PathBuf {
    let extra = unify_separators(extra);
    let mut out = normalize(base);
    let base_depth = comparable_parts(&out).count();
    let mut depth = base_depth;

    for component in extra.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                if depth > base_depth {
                    out.pop();
                    depth -= 1;
                }
            }
            Component::Normal(name) => {
                out.push(name);
                depth += 1;
            }
        }
    }
    normalize(&out)
}

pub fn looks_absolute(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    if matches!(bytes.first(), Some(b'/') | Some(b'\\')) {
        return true;
    }
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

#[cfg(windows)]
pub fn extended_path(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if text.starts_with(r"\\?\") || text.starts_with(r"\\.\") {
        return path.to_path_buf();
    }
    let normalized = normalize(path);
    if !normalized.is_absolute() {
        // Relative and drive-relative paths cannot take the verbatim prefix.
        return path.to_path_buf();
    }
    let Some(text) = normalized.to_str() else {
        return path.to_path_buf();
    };
    match text.strip_prefix(r"\\") {
        Some(rest) => PathBuf::from(format!(r"\\?\UNC\{rest}")),
        None => PathBuf::from(format!(r"\\?\{text}")),
    }
}

#[cfg(not(windows))]
pub fn extended_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

pub fn extended_path_undo(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(rest) = strip_prefix_ci(text, r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        let bytes = rest.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> &'static str {
        if cfg!(windows) {
            r"C:\"
        } else {
            "/"
        }
    }

    #[test]
    fn expands_tilde_only_at_start() {
        if let Some(home) = platform::home_dir() {
            assert!(path_equal(&expand("~"), &home));
            assert!(path_equal(&expand("~/x"), &home.join("x")));
            assert!(path_equal(&expand(r"~\x"), &home.join("x")));
        }
        assert!(path_equal(&expand("a/~/b"), Path::new("a/~/b")));
    }

    #[test]
    fn expands_env_forms() {
        let name = "SWEEP_TEST_ENV_EXPAND";
        std::env::set_var(name, "value");
        assert_eq!(expand(&format!("${name}")), PathBuf::from("value"));
        assert_eq!(expand(&format!("${{{name}}}")), PathBuf::from("value"));
        assert_eq!(expand(&format!("%{name}%")), PathBuf::from("value"));
        std::env::remove_var(name);
    }

    #[test]
    fn empty_environment_values_are_unresolved() {
        let name = "SWEEP_TEST_ENV_EMPTY";
        std::env::set_var(name, "");
        assert!(expand_defined(&format!("${name}/x")).is_none());
        assert!(expand_defined(&format!("%{name}%/x")).is_none());
        std::env::remove_var(name);
    }

    #[test]
    fn keeps_malformed_references() {
        for input in ["100%", "50%OFF", "$$", "${UNCLOSED", "${}", "%%"] {
            assert_eq!(expand(input), PathBuf::from(input), "input {input}");
        }
    }

    #[test]
    fn ordinary_percent_and_dollar_signs_are_literal() {
        assert!(expand_defined("a 50% off/100% sure").is_some());
        assert!(expand_defined("price$5").is_some());
        assert!(expand_defined("cost$").is_some());
    }

    #[test]
    fn non_ascii_text_does_not_confuse_expansion() {
        let input = "ü/$ü/ä$";
        assert_eq!(expand(input), PathBuf::from(input));
        assert!(expand_defined("$ü").is_some());
    }

    #[test]
    fn unresolved_reference_marks_incomplete() {
        assert!(expand_defined("${SWEEP_TEST_MISSING_REFERENCE}").is_none());
        assert!(expand_defined("$SWEEP_TEST_MISSING_REFERENCE/x").is_none());
        assert!(expand_defined("plain").is_some());
    }

    #[test]
    fn normalizes_relative_parents() {
        assert_eq!(normalize(Path::new("a/b/../c")), PathBuf::from("a/c"));
        assert_eq!(normalize(Path::new("../a")), PathBuf::from("../a"));
        assert_eq!(normalize(Path::new("a/./b//c")), PathBuf::from("a/b/c"));
        assert_eq!(normalize(Path::new("")), PathBuf::from("."));
        assert_eq!(
            normalize(&Path::new(root()).join("a/../../b")),
            Path::new(root()).join("b")
        );
    }

    #[test]
    fn containment_uses_components() {
        assert!(path_starts_with(
            Path::new("/tmp/a/b"),
            Path::new("/tmp/a")
        ));
        assert!(!path_starts_with(Path::new("/tmp/ab"), Path::new("/tmp/a")));
        assert!(path_starts_with(Path::new("a/b"), Path::new(".")));
        assert!(!path_starts_with(Path::new("../a"), Path::new(".")));
        assert!(!path_starts_with(Path::new("/a"), Path::new(".")));
    }

    #[test]
    fn containment_normalises_both_sides() {
        assert!(path_starts_with(
            Path::new("/tmp/x/../a/b"),
            Path::new("/tmp/./a")
        ));
        assert!(!path_starts_with(
            Path::new("/tmp/a/../b"),
            Path::new("/tmp/a")
        ));
    }

    #[test]
    fn case_switch_is_explicit() {
        assert!(!path_equal_cs(Path::new("A"), Path::new("a")));
        assert_eq!(
            path_equal(Path::new("A"), Path::new("a")),
            !case_sensitive()
        );
    }

    #[test]
    fn join_clamps_to_base() {
        assert_eq!(
            join_opt(Path::new("/base"), Path::new("/etc/passwd")),
            PathBuf::from("/base/etc/passwd")
        );
        assert_eq!(
            join_opt(Path::new("/base"), Path::new("../x")),
            PathBuf::from("/base/x")
        );
        assert_eq!(
            join_opt(Path::new("a"), Path::new("../../b")),
            PathBuf::from("a/b")
        );
    }

    #[test]
    fn join_honours_both_separators() {
        assert_eq!(
            join_opt(Path::new("/base"), Path::new(r"..\x")),
            PathBuf::from("/base/x")
        );
    }

    #[test]
    fn looks_absolute_cross_platform() {
        assert!(looks_absolute(Path::new("/x")));
        assert!(looks_absolute(Path::new(r"\x")));
        assert!(looks_absolute(Path::new(r"C:\x")));
        assert!(!looks_absolute(Path::new("C:")));
        assert!(!looks_absolute(Path::new("1:\\x")));
        assert!(!looks_absolute(Path::new("x/y")));
    }

    #[test]
    fn verbatim_prefixes_are_undone() {
        assert_eq!(
            extended_path_undo(Path::new(r"\\?\C:\a")),
            PathBuf::from(r"C:\a")
        );
        assert_eq!(
            extended_path_undo(Path::new(r"\\?\UNC\srv\share")),
            PathBuf::from(r"\\srv\share")
        );
        assert_eq!(
            extended_path_undo(Path::new(r"\\?\unc\srv\share")),
            PathBuf::from(r"\\srv\share")
        );
        assert_eq!(
            extended_path_undo(Path::new(r"\\?\Volume{abc}\x")),
            PathBuf::from(r"\\?\Volume{abc}\x")
        );
        assert_eq!(
            extended_path_undo(Path::new(r"C:\a")),
            PathBuf::from(r"C:\a")
        );
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_and_plain_paths_compare_equal() {
        assert!(path_equal(
            Path::new(r"\\?\C:\a\b"),
            Path::new(r"C:\a\b")
        ));
        assert!(path_starts_with(
            Path::new(r"\\?\C:\a\b"),
            Path::new(r"C:\a")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn extended_round_trip() {
        for path in [Path::new(r"C:\a\b"), Path::new(r"\\server\share\a")] {
            let extended = extended_path(path);
            assert!(extended.to_string_lossy().starts_with(r"\\?\"));
            assert_eq!(extended_path_undo(&extended), path);
        }
    }

    #[cfg(windows)]
    #[test]
    fn relative_paths_are_not_extended() {
        assert_eq!(extended_path(Path::new(r"a\b")), PathBuf::from(r"a\b"));
    }
}
