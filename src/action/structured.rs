//! Structured file editing: `ini` and `json` actions.
//!
//! These never delete a whole file — they remove a section, a parameter or a
//! JSON key and preserve everything else, including line endings and file
//! permissions.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::action::context::RunContext;
use crate::core::error::{Error, Result};
use crate::core::report::{Entry, EntryKind, Report};
use crate::definition::model::{ActionDef, SearchKind, VarSet};
use crate::fsutil::walk::{glob_paths, scan_paths, ScanOptions};

pub fn run(
    def: &ActionDef,
    vars: &VarSet,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
) -> Report {
    let mut report = Report::new();
    let command = def.command.as_str();

    for root in crate::definition::model::expand_action_paths(def, vars) {
        let paths = match def.search {
            SearchKind::File if root.exists() => vec![root],
            SearchKind::File => Vec::new(),
            SearchKind::Glob => match root.to_str() {
                Some(text) => glob_paths(text).unwrap_or_default(),
                None => {
                    log::warn!("skipping glob with non-UTF-8 path: {}", root.display());
                    Vec::new()
                }
            },
            _ => scan_paths(&root, &ScanOptions::files().with_same_filesystem(true)),
        };

        for path in paths {
            if ctx.cancelled() {
                report.aborted = true;
                return report;
            }
            if ctx.guard.check(&path, false).is_err() {
                report.push(Entry::skipped(cleaner, option, &path));
                continue;
            }

            let outcome = if command == "json" {
                edit_json(&path, def.address.as_deref(), ctx.dry_run)
            } else {
                edit_ini(
                    &path,
                    def.section.as_deref(),
                    def.parameter.as_deref(),
                    ctx.dry_run,
                )
            };

            match outcome {
                Ok(true) => report.push(Entry::new(
                    EntryKind::CleanFile,
                    cleaner,
                    option,
                    format!("edit {}", path.display()),
                    Some(path.as_path()),
                    0,
                )),
                Ok(false) => {}
                Err(err) => report.fail(cleaner, option, err.to_string()),
            }
        }
    }

    report
}

/// Remove a section (or one parameter inside it), preserving order, comments
/// and the original line endings.
fn edit_ini(path: &Path, section: Option<&str>, parameter: Option<&str>, dry_run: bool) -> Result<bool> {
    let section = section.ok_or_else(|| {
        Error::msg(format!("ini action on {} is missing a 'section'", path.display()))
    })?;

    let text = std::fs::read_to_string(path).map_err(|source| Error::io(path, source))?;
    let line_ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let removing_section = parameter.is_none();

    let mut out = Vec::new();
    let mut in_target = false;
    let mut changed = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let name = &trimmed[1..trimmed.len() - 1];
            in_target = name.eq_ignore_ascii_case(section);
            if in_target && removing_section {
                changed = true;
                continue;
            }
            out.push(line.to_string());
            continue;
        }

        if in_target {
            match parameter {
                Some(param) => match line.split_once('=') {
                    Some((key, _)) if key.trim().eq_ignore_ascii_case(param) => {
                        changed = true;
                    }
                    _ => out.push(line.to_string()),
                },
                None => changed = true,
            }
        } else {
            out.push(line.to_string());
        }
    }

    if !changed {
        return Ok(false);
    }

    if !dry_run {
        let mut new_text = out.join(line_ending);
        if text.ends_with('\n') {
            new_text.push_str(line_ending);
        }
        write_atomic(path, new_text)?;
    }
    Ok(true)
}

/// Remove a slash-separated `address` key from a JSON document.
fn edit_json(path: &Path, address: Option<&str>, dry_run: bool) -> Result<bool> {
    let address = address.ok_or_else(|| {
        Error::msg(format!("json action on {} is missing an 'address'", path.display()))
    })?;

    let text = std::fs::read_to_string(path).map_err(|source| Error::io(path, source))?;
    let mut value: serde_json::Value = serde_json::from_str(&text).map_err(|source| Error::Structured {
        path: path.to_path_buf(),
        reason: source.to_string(),
    })?;

    let mut keys: Vec<&str> = address.split('/').filter(|k| !k.is_empty()).collect();
    let Some(last) = keys.pop() else {
        return Err(Error::msg(format!("json address '{address}' does not point at a key")));
    };

    let mut current = &mut value;
    for key in &keys {
        match current.get_mut(*key) {
            Some(next) => current = next,
            None => return Ok(false),
        }
    }

    let changed = match current {
        serde_json::Value::Object(map) => map.remove(last).is_some(),
        _ => false,
    };
    if !changed {
        return Ok(false);
    }

    if !dry_run {
        let new_text = serde_json::to_string_pretty(&value).map_err(|source| Error::Structured {
            path: path.to_path_buf(),
            reason: source.to_string(),
        })?;
        write_atomic(path, new_text)?;
    }
    Ok(true)
}

/// Write via a temp sibling and rename, so a crash cannot leave a half-written
/// file. The original permissions are restored before the rename.
fn write_atomic(path: &Path, text: String) -> Result<()> {
    use std::io::Write;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let temp = dir.join(format!(
        ".sweep_{}_{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let orig_perms = std::fs::metadata(path).ok().map(|m| m.permissions());

    struct Guard<'a> {
        path: &'a Path,
        done: std::cell::Cell<bool>,
    }
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            if !self.done.get() {
                let _ = std::fs::remove_file(self.path);
            }
        }
    }
    let guard = Guard { path: &temp, done: std::cell::Cell::new(false) };

    {
        let mut file = std::fs::File::create(&temp).map_err(|err| Error::io(&temp, err))?;
        file.write_all(text.as_bytes()).map_err(|err| Error::io(&temp, err))?;
        file.sync_all().map_err(|err| Error::io(&temp, err))?;
    }

    if let Some(perms) = orig_perms {
        std::fs::set_permissions(&temp, perms).map_err(|err| Error::io(&temp, err))?;
    }
    std::fs::rename(&temp, path).map_err(|err| Error::io(path, err))?;

    #[cfg(unix)]
    if let Ok(dir_file) = std::fs::File::open(dir) {
        let _ = dir_file.sync_all();
    }

    guard.done.set(true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let path = crate::fixture(tag);
        path.join(if tag.ends_with(".json") { "edit.json" } else { "conf.ini" })
    }

    #[test]
    fn ini_removes_parameter_only() {
        let path = tmp("struct_ini_param.ini");
        std::fs::write(&path, "[Foo]\nkey = 1\nkeep = 2\n").unwrap();
        assert!(edit_ini(&path, Some("Foo"), Some("key"), false).unwrap());
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(!out.contains("key = 1"));
        assert!(out.contains("keep = 2"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn ini_whole_section_drops_header_too() {
        let path = tmp("struct_ini_section.ini");
        std::fs::write(&path, "[Foo]\nkey = 1\n\n[Bar]\nkeep = 2\n").unwrap();
        assert!(edit_ini(&path, Some("Foo"), None, false).unwrap());
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(!out.contains("[Foo]"), "empty header left behind: {out:?}");
        assert!(out.contains("[Bar]") && out.contains("keep = 2"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn ini_preserves_crlf() {
        let path = tmp("struct_ini_crlf.ini");
        std::fs::write(&path, b"[Foo]\r\nkey = 1\r\n[Bar]\r\nkeep = 2\r\n").unwrap();
        assert!(edit_ini(&path, Some("Foo"), Some("key"), false).unwrap());
        let text = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
        assert!(text.contains("\r\n"), "line endings changed: {text:?}");
        assert!(!text.contains("key = 1"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn ini_dry_run_writes_nothing() {
        let path = tmp("struct_ini_dry.ini");
        std::fs::write(&path, "[Foo]\nkey = 1\n").unwrap();
        assert!(edit_ini(&path, Some("Foo"), Some("key"), true).unwrap());
        assert!(std::fs::read_to_string(&path).unwrap().contains("key = 1"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn json_removes_nested_key() {
        let path = tmp("struct_json.json");
        std::fs::write(&path, r#"{"a": {"b": 1, "c": 2}}"#).unwrap();
        assert!(edit_json(&path, Some("a/b"), false).unwrap());
        let out: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(out["a"].get("b").is_none());
        assert_eq!(out["a"]["c"], 2);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn json_missing_key_is_noop_and_bad_address_errors() {
        let path = tmp("struct_json_noop.json");
        std::fs::write(&path, r#"{"a": {"b": 1}}"#).unwrap();
        assert!(!edit_json(&path, Some("a/x"), false).unwrap());
        assert!(edit_json(&path, Some("/"), false).is_err());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_preserves_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = tmp("struct_ini_perms.ini");
        std::fs::write(&path, "[Foo]\nkey = 1\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        edit_ini(&path, Some("Foo"), Some("key"), false).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "permissions widened: {mode:o}");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
