//! SQLite-backed actions (browser history, cookies and other databases).
//!
//! A locked or read-only database is copied to a temp file, operated on
//! there, and swapped back over the original — so a running browser does not
//! turn a cleaning run into a wall of errors.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;

use crate::action::context::RunContext;
use crate::core::error::{Error, Result};
use crate::core::report::{Entry, EntryKind, Report};
use crate::definition::model::{ActionDef, SearchKind, VarSet};
use crate::fsutil::size::size_of;
use crate::fsutil::walk::{scan_paths, ScanOptions};

/// DELETE statements per command, run in order. Statements that fail (unknown
/// table in this browser version) are ignored.
fn delete_statements(command: &str) -> &'static [&'static str] {
    match command {
        "chrome.history" => &[
            "DELETE FROM urls",
            "DELETE FROM visits",
            "DELETE FROM visit_source",
            "DELETE FROM keyword_search_terms",
            "DELETE FROM segments",
            "DELETE FROM segment_usage",
            "DELETE FROM downloads",
            "DELETE FROM downloads_url_chains",
            "DELETE FROM downloads_slices",
        ],
        "chrome.keywords" => &["DELETE FROM keyword_search_terms"],
        "chrome.autofill" => &[
            "DELETE FROM autofill_profiles",
            "DELETE FROM autofill_profile_emails",
            "DELETE FROM autofill_profile_phones",
        ],
        "chrome.favicons" => &[
            "DELETE FROM icon_mapping",
            "DELETE FROM favicon_bitmaps",
            "DELETE FROM favicons",
        ],
        "chrome.databases_db" => &["DELETE FROM databases", "DELETE FROM meta"],
        "chrome.cookies" | "cookies" => &["DELETE FROM cookies"],
        "mozilla.history" | "mozilla.url_history" => &[
            "DELETE FROM moz_historyvisits",
            "DELETE FROM moz_inputhistory",
            "DELETE FROM moz_places WHERE id NOT IN (SELECT fk FROM moz_bookmarks WHERE fk IS NOT NULL)",
        ],
        "mozilla.databases" => &[
            "DELETE FROM moz_places WHERE last_visit_date IS NULL",
            "DELETE FROM moz_historyvisits WHERE visit_date IS NULL",
        ],
        _ => &[],
    }
}

pub fn run(
    def: &ActionDef,
    vars: &VarSet,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
) -> Report {
    let mut report = Report::new();

    for root in crate::definition::model::expand_action_paths(def, vars) {
        if def.command == "mozilla.vacuum" || def.search != SearchKind::File {
            vacuum_directory(&root, ctx, cleaner, option, &mut report);
            continue;
        }

        if ctx.guard.check(&root, false).is_err() {
            report.push(Entry::skipped(cleaner, option, &root));
            continue;
        }
        if root.symlink_metadata().is_err() {
            // Opening a missing database would create an empty one.
            continue;
        }

        let before = size_of(&root).unwrap_or(0);
        let freed = match operate(&root, def.command.as_str(), ctx.dry_run) {
            Ok(freed) => freed,
            Err(err) => {
                report.fail(cleaner, option, err.to_string());
                continue;
            }
        };
        let after = size_of(&root).unwrap_or(before);
        report.push(Entry::new(
            EntryKind::Vacuum,
            cleaner,
            option,
            format!("clean {}", root.display()),
            Some(root.as_path()),
            freed + before.saturating_sub(after),
        ));
    }

    report
}

fn vacuum_directory(
    dir: &Path,
    ctx: &RunContext,
    cleaner: &str,
    option: &str,
    report: &mut Report,
) {
    if !dir.is_dir() {
        return;
    }
    for db in scan_paths(dir, &ScanOptions::files().with_max_depth(4)) {
        if !db.extension().is_some_and(|e| e.eq_ignore_ascii_case("sqlite")) {
            continue;
        }
        if ctx.cancelled() {
            report.aborted = true;
            return;
        }
        if ctx.guard.check(&db, false).is_err() {
            report.push(Entry::skipped(cleaner, option, &db));
            continue;
        }
        let before = size_of(&db).unwrap_or(0);
        let freed = operate(&db, "sqlite.vacuum", ctx.dry_run).unwrap_or(0)
            + before.saturating_sub(size_of(&db).unwrap_or(before));
        report.push(Entry::new(
            EntryKind::Vacuum,
            cleaner,
            option,
            format!("vacuum {}", db.display()),
            Some(db.as_path()),
            freed,
        ));
    }
}

fn operate(path: &Path, command: &str, dry_run: bool) -> Result<u64> {
    if dry_run || path.symlink_metadata().is_err() {
        return Ok(0);
    }

    let (conn, swap) = open_writable(path)?;

    if command == "sqlite.vacuum" {
        conn.execute_batch("VACUUM;").map_err(|source| Error::Database {
            path: path.to_path_buf(),
            source,
        })?;
    } else {
        let statements = delete_statements(command);
        if !statements.is_empty() {
            for stmt in statements {
                let _ = conn.execute_batch(stmt);
            }
            let _ = conn.execute_batch("VACUUM;");
        }
    }
    drop(conn);

    if let Some((temp, original)) = swap {
        if std::fs::rename(&temp, &original).is_err() {
            // Windows cannot rename over an existing file.
            let _ = std::fs::remove_file(&original);
            if std::fs::rename(&temp, &original).is_err() {
                std::fs::copy(&temp, &original).map_err(|source| Error::io(&original, source))?;
                let _ = std::fs::remove_file(&temp);
            }
        }
    }
    Ok(0)
}

/// Open read-write; fall back to an exclusive temp copy when the database is
/// locked. The copy is created with `create_new` and streamed through the
/// same handle, so a planted symlink cannot redirect it.
fn open_writable(path: &Path) -> Result<(Connection, Option<(PathBuf, PathBuf)>)> {
    match Connection::open(path) {
        Ok(conn) => Ok((conn, None)),
        Err(_) => {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let mut attempts = 0;
            loop {
                attempts += 1;
                let temp = std::env::temp_dir().join(format!(
                    "sweep_{}_{}.sqlite",
                    std::process::id(),
                    SEQ.fetch_add(1, Ordering::Relaxed)
                ));
                match std::fs::OpenOptions::new().write(true).create_new(true).open(&temp) {
                    Ok(mut file) => {
                        let copied = (|| -> std::io::Result<()> {
                            let mut src = std::fs::File::open(path)?;
                            std::io::copy(&mut src, &mut file)?;
                            Ok(())
                        })();
                        drop(file);
                        if copied.is_err() {
                            let _ = std::fs::remove_file(&temp);
                            return Err(Error::io(path, copied.err().unwrap()));
                        }
                        match Connection::open(&temp) {
                            Ok(conn) => return Ok((conn, Some((temp, path.to_path_buf())))),
                            Err(source) => {
                                let _ = std::fs::remove_file(&temp);
                                return Err(Error::Database { path: path.to_path_buf(), source });
                            }
                        }
                    }
                    Err(_) if attempts < 4 => continue,
                    Err(source) => return Err(Error::io(path, source)),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("CREATE TABLE urls (id INTEGER, url TEXT); INSERT INTO urls VALUES (1, 'https://example'); CREATE TABLE visits (id INTEGER);").unwrap();
    }

    #[test]
    fn delete_statements_cover_browsers() {
        assert!(!delete_statements("chrome.history").is_empty());
        assert!(delete_statements("chrome.history").iter().any(|s| s.contains("urls")));
        assert_eq!(delete_statements("chrome.cookies"), &["DELETE FROM cookies"]);
        assert!(delete_statements("cookies") == delete_statements("chrome.cookies"));
        assert!(delete_statements("mozilla.url_history").iter().any(|s| s.contains("moz_places")));
        assert!(delete_statements("sqlite.vacuum").is_empty());
        assert!(delete_statements("unknown.command").is_empty());
    }

    #[test]
    fn chrome_history_clears_rows_then_vacuums() {
        let dir = crate::fixture("sqlite_history");
        let db = dir.join("History");
        create_db(&db);
        operate(&db, "chrome.history", false).unwrap();
        let count: i64 = {
            let conn = Connection::open(&db).unwrap();
            conn.query_row("SELECT COUNT(*) FROM urls", [], |row| row.get(0)).unwrap()
        };
        assert_eq!(count, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_database_is_not_created() {
        let dir = crate::fixture("sqlite_missing");
        let db = dir.join("missing.sqlite");
        assert_eq!(operate(&db, "chrome.cookies", false).unwrap(), 0);
        assert!(!db.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dry_run_touches_nothing() {
        let dir = crate::fixture("sqlite_dry");
        let db = dir.join("History");
        create_db(&db);
        let before = size_of(&db).unwrap();
        assert_eq!(operate(&db, "chrome.history", true).unwrap(), 0);
        assert_eq!(size_of(&db).unwrap(), before);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_reports_vacuum_and_skips_missing() {
        let dir = crate::fixture("sqlite_run");
        let db = dir.join("History");
        create_db(&db);
        let mut def = ActionDef::default();
        def.command = "chrome.history".to_string();
        def.path = db.to_string_lossy().into_owned();
        let ctx = RunContext::preview();
        let report = run(&def, &VarSet::new(), &ctx, "chrome", "history");
        assert_eq!(report.reclaimed(), 0, "preview must not reclaim");
        assert!(report.error_count() == 0);

        let missing = def.clone();
        let ctx = RunContext::clean();
        let def2 = ActionDef { command: "chrome.cookies".to_string(), path: dir.join("no.sqlite").to_string_lossy().into_owned(), ..missing };
        let report = run(&def2, &VarSet::new(), &ctx, "chrome", "cookies");
        assert!(report.error_count() == 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
