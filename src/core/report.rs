//! Accounting for preview and clean runs.
//!
//! A [`Report`] is append-only: path operations push entries, the worker can
//! merge sub-reports, and the CLI turns one report into either JSON or a
//! ledger.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Serialize, Serializer};

pub const SCHEMA_VERSION: u32 = 1;

fn ser_path<S>(path: &Option<PathBuf>, serializer: S) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match path {
        Some(path) => serializer.serialize_str(&path.to_string_lossy()),
        None => serializer.serialize_none(),
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Delete,
    Shred,
    Truncate,
    CleanFile,
    Vacuum,
    Registry,
    Command,
    WipeFreeSpace,
    Skip,
    Error,
}

impl EntryKind {
    pub fn label(self) -> &'static str {
        match self {
            EntryKind::Delete => "Delete",
            EntryKind::Shred => "Shred",
            EntryKind::Truncate => "Truncate",
            EntryKind::CleanFile => "Clean file",
            EntryKind::Vacuum => "Vacuum",
            EntryKind::Registry => "Delete registry key",
            EntryKind::Command => "Run command",
            EntryKind::WipeFreeSpace => "Wipe free space",
            EntryKind::Skip => "Skip",
            EntryKind::Error => "Error",
        }
    }

    pub fn counts_as_deleted(self) -> bool {
        matches!(self, EntryKind::Delete | EntryKind::Shred)
    }

    pub fn counts_as_special(self) -> bool {
        matches!(
            self,
            EntryKind::Truncate
                | EntryKind::CleanFile
                | EntryKind::Vacuum
                | EntryKind::Registry
                | EntryKind::Command
                | EntryKind::WipeFreeSpace
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub kind: EntryKind,
    pub cleaner: String,
    pub option: String,
    pub label: String,
    #[serde(serialize_with = "ser_path")]
    pub path: Option<PathBuf>,
    pub reclaimed: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub backed_up: u64,
}

impl Entry {
    pub fn new(
        kind: EntryKind,
        cleaner: impl Into<String>,
        option: impl Into<String>,
        label: impl Into<String>,
        path: Option<&Path>,
        reclaimed: u64,
    ) -> Self {
        Self {
            kind,
            cleaner: cleaner.into(),
            option: option.into(),
            label: label.into(),
            path: path.map(Path::to_path_buf),
            reclaimed,
            backed_up: 0,
        }
    }

    pub fn with_backup(mut self, bytes: u64) -> Self {
        self.backed_up = bytes;
        self
    }

    pub fn skipped(cleaner: &str, option: &str, path: &Path) -> Self {
        Self::new(
            EntryKind::Skip,
            cleaner,
            option,
            EntryKind::Skip.label(),
            Some(path),
            0,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Stopwatch(std::time::Instant);

impl Stopwatch {
    pub fn start() -> Self {
        Self(std::time::Instant::now())
    }

    pub fn elapsed_ms(self) -> u64 {
        u64::try_from(self.0.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub cleaner: String,
    pub option: String,
    pub message: String,
}

fn has_failure(list: &[Failure], failure: &Failure) -> bool {
    list.iter().any(|f| {
        f.cleaner == failure.cleaner && f.option == failure.option && f.message == failure.message
    })
}

fn sample_indices(len: usize, max: usize) -> Vec<usize> {
    if max == 0 || len <= max {
        return (0..len).collect();
    }
    if max == 1 {
        return vec![0];
    }
    let last = len - 1;
    (0..max)
        .map(|i| (i as u128 * last as u128 / (max as u128 - 1)) as usize)
        .collect()
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Report {
    pub entries: Vec<Entry>,
    pub errors: Vec<Failure>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<Failure>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Failure>,
    pub aborted: bool,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub free_before: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub free_after: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub excluded: u64,
    /// `(bytes, files)` totals for runs whose `entries` are incomplete.
    /// Entries pushed afterwards are added to it, so it always covers every
    /// entry the report has seen.
    #[serde(default, skip_serializing)]
    pub totals_override: Option<(u64, u64)>,
}

impl Report {
    pub fn new() -> Self {
        Self::default()
    }

    fn entry_sums(&self) -> (u64, u64) {
        let mut bytes = 0u64;
        let mut files = 0u64;
        for entry in &self.entries {
            bytes = bytes.saturating_add(entry.reclaimed);
            if entry.kind.counts_as_deleted() {
                files = files.saturating_add(1);
            }
        }
        (bytes, files)
    }

    pub fn push(&mut self, entry: Entry) {
        if let Some((bytes, files)) = self.totals_override.as_mut() {
            *bytes = bytes.saturating_add(entry.reclaimed);
            if entry.kind.counts_as_deleted() {
                *files = files.saturating_add(1);
            }
        }
        self.entries.push(entry);
    }

    pub fn merge(&mut self, other: Report) {
        self.aborted |= other.aborted;
        self.duration_ms = self.duration_ms.max(other.duration_ms);
        self.excluded = self.excluded.saturating_add(other.excluded);
        if self.free_before == 0 {
            self.free_before = other.free_before;
        }
        if other.free_after != 0 {
            self.free_after = other.free_after;
        }

        let (self_bytes, self_files) = self.entry_sums();
        let (other_bytes, other_files) = other.entry_sums();
        self.totals_override = match (self.totals_override, other.totals_override) {
            (Some(a), Some(b)) => Some((a.0.saturating_add(b.0), a.1.saturating_add(b.1))),
            (Some(a), None) => Some((
                a.0.saturating_add(other_bytes),
                a.1.saturating_add(other_files),
            )),
            (None, Some(b)) => Some((
                self_bytes.saturating_add(b.0),
                self_files.saturating_add(b.1),
            )),
            (None, None) => None,
        };

        self.errors.extend(other.errors);
        for blocked in other.blocked {
            if !self
                .blocked
                .iter()
                .any(|f| f.cleaner == blocked.cleaner && f.message == blocked.message)
            {
                self.blocked.push(blocked);
            }
        }
        for warning in other.warnings {
            if !has_failure(&self.warnings, &warning) {
                self.warnings.push(warning);
            }
        }
        self.entries.extend(other.entries);
    }

    pub fn fail(&mut self, cleaner: &str, option: &str, message: impl Into<String>) {
        self.errors.push(Failure {
            cleaner: cleaner.to_string(),
            option: option.to_string(),
            message: message.into(),
        });
    }

    pub fn block(&mut self, cleaner: &str, option: &str, message: impl Into<String>) {
        let message = message.into();
        if self
            .blocked
            .iter()
            .any(|f| f.cleaner == cleaner && f.message == message)
        {
            return;
        }
        self.blocked.push(Failure {
            cleaner: cleaner.to_string(),
            option: option.to_string(),
            message,
        });
    }

    pub fn warn(&mut self, cleaner: &str, option: &str, message: impl Into<String>) {
        let failure = Failure {
            cleaner: cleaner.to_string(),
            option: option.to_string(),
            message: message.into(),
        };
        if !has_failure(&self.warnings, &failure) {
            self.warnings.push(failure);
        }
    }

    pub fn blocked_count(&self) -> u64 {
        self.blocked.len() as u64
    }

    pub fn backed_up(&self) -> u64 {
        self.entries
            .iter()
            .fold(0u64, |sum, entry| sum.saturating_add(entry.backed_up))
    }

    pub fn net_reclaimed(&self) -> u64 {
        self.reclaimed().saturating_sub(self.backed_up())
    }

    pub fn reclaimed(&self) -> u64 {
        self.totals_override
            .map(|totals| totals.0)
            .unwrap_or_else(|| self.entry_sums().0)
    }

    pub fn files_removed(&self) -> u64 {
        self.totals_override
            .map(|totals| totals.1)
            .unwrap_or_else(|| self.entry_sums().1)
    }

    pub fn special_operations(&self) -> u64 {
        self.entries
            .iter()
            .filter(|entry| entry.kind.counts_as_special())
            .count() as u64
    }

    pub fn skipped(&self) -> u64 {
        self.entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::Skip)
            .count() as u64
    }

    pub fn error_count(&self) -> u64 {
        self.errors.len() as u64
            + self
                .entries
                .iter()
                .filter(|entry| entry.kind == EntryKind::Error)
                .count() as u64
    }

    pub fn is_ok(&self) -> bool {
        self.error_count() == 0 && !self.aborted
    }

    pub fn finish(&mut self, watch: Stopwatch) {
        self.duration_ms = self.duration_ms.max(watch.elapsed_ms());
    }

    pub fn freed_bytes(&self) -> u64 {
        self.free_after.saturating_sub(self.free_before)
    }

    pub fn with_space(mut self, before: u64, after: u64) -> Self {
        self.free_before = before;
        self.free_after = after;
        self
    }

    /// Keys shared by the full and the summary JSON documents.
    fn base_json(&self) -> serde_json::Map<String, serde_json::Value> {
        let value = serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "reclaimed_bytes": self.reclaimed(),
            "files_removed": self.files_removed(),
            "special_operations": self.special_operations(),
            "skipped": self.skipped(),
            "errors": self.error_count(),
            "blocked": self.blocked_count(),
            "blocked_items": self.blocked,
            "warnings": self.warnings,
            "backed_up_bytes": self.backed_up(),
            "net_reclaimed_bytes": self.net_reclaimed(),
            "aborted": self.aborted,
            "duration_ms": self.duration_ms,
            "bytes_affected": self.reclaimed(),
            "items_count": self.entries.len(),
            "free_before": self.free_before,
            "free_after": self.free_after,
            "freed_bytes": self.freed_bytes(),
            "excluded": self.excluded,
            "failures": self.errors,
        });
        match value {
            serde_json::Value::Object(map) => map,
            _ => serde_json::Map::new(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut map = self.base_json();
        map.insert(
            "entries".to_string(),
            serde_json::to_value(&self.entries).unwrap_or(serde_json::Value::Null),
        );
        serde_json::Value::Object(map)
    }

    pub fn to_json_summary(&self, max_entries: usize) -> serde_json::Value {
        let mut grouped: BTreeMap<(&str, &str), (u64, u64, u64)> = BTreeMap::new();
        for entry in &self.entries {
            let slot = grouped
                .entry((entry.cleaner.as_str(), entry.option.as_str()))
                .or_insert((0, 0, 0));
            slot.0 = slot.0.saturating_add(u64::from(entry.kind.counts_as_deleted()));
            slot.1 = slot.1.saturating_add(entry.reclaimed);
            slot.2 = slot.2.saturating_add(1);
        }
        let by_option: Vec<serde_json::Value> = grouped
            .into_iter()
            .map(|((cleaner, option), (files, bytes, entries))| {
                serde_json::json!({
                    "cleaner": cleaner,
                    "option": option,
                    "files": files,
                    "bytes": bytes,
                    "entries": entries,
                })
            })
            .collect();

        let sample: Vec<&Entry> = sample_indices(self.entries.len(), max_entries)
            .into_iter()
            .map(|index| &self.entries[index])
            .collect();
        let truncated = sample.len() < self.entries.len();

        let mut map = self.base_json();
        map.insert("entries_truncated".to_string(), serde_json::json!(truncated));
        map.insert("by_option".to_string(), serde_json::json!(by_option));
        map.insert(
            "entries".to_string(),
            serde_json::to_value(&sample).unwrap_or(serde_json::Value::Null),
        );
        serde_json::Value::Object(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: EntryKind, reclaimed: u64) -> Entry {
        Entry::new(kind, "cleaner", "option", "label", None, reclaimed)
    }

    #[test]
    fn counts_by_kind() {
        let mut report = Report::new();
        report.push(entry(EntryKind::Delete, 10));
        report.push(entry(EntryKind::Shred, 20));
        report.push(entry(EntryKind::Command, 5));
        report.push(entry(EntryKind::Skip, 0));
        report.push(entry(EntryKind::Error, 0));

        assert_eq!(report.files_removed(), 2);
        assert_eq!(report.special_operations(), 1);
        assert_eq!(report.skipped(), 1);
        assert_eq!(report.reclaimed(), 35);
        assert_eq!(report.error_count(), 1);
        assert!(!report.is_ok());
    }

    #[test]
    fn truncation_counts_as_special_not_as_removal() {
        let mut report = Report::new();
        report.push(entry(EntryKind::Truncate, 50));
        assert_eq!(report.special_operations(), 1);
        assert_eq!(report.files_removed(), 0);
        assert_eq!(report.reclaimed(), 50);
    }

    #[test]
    fn merge_combines_totals_and_deduplicates_blocked() {
        let mut left = Report::new();
        left.totals_override = Some((1000, 10));
        left.block("browser", "a", "running");
        left.warnings.push(Failure {
            cleaner: "x".into(),
            option: "y".into(),
            message: "w".into(),
        });

        let mut right = Report::new();
        right.push(entry(EntryKind::Delete, 7));
        right.block("browser", "b", "running");
        right.block("browser", "c", "other");

        left.merge(right);
        assert_eq!(left.reclaimed(), 1007);
        assert_eq!(left.files_removed(), 11);
        assert_eq!(left.blocked_count(), 2);
        assert_eq!(left.warnings.len(), 1);
    }

    #[test]
    fn entries_pushed_after_an_override_are_counted() {
        let mut report = Report::new();
        report.totals_override = Some((100, 5));
        report.push(entry(EntryKind::Delete, 7));
        report.push(entry(EntryKind::Skip, 0));
        assert_eq!(report.reclaimed(), 107);
        assert_eq!(report.files_removed(), 6);
    }

    #[test]
    fn identical_warnings_are_deduplicated() {
        let mut report = Report::new();
        report.warn("c", "o", "same");
        report.warn("c", "o", "same");
        report.warn("c", "o", "different");
        assert_eq!(report.warnings.len(), 2);

        let mut other = Report::new();
        other.warn("c", "o", "same");
        other.warn("c", "p", "same");
        report.merge(other);
        assert_eq!(report.warnings.len(), 3);
    }

    #[test]
    fn backup_bytes_are_not_net_gain() {
        let mut report = Report::new();
        report.push(entry(EntryKind::Delete, 100).with_backup(40));
        assert_eq!(report.reclaimed(), 100);
        assert_eq!(report.backed_up(), 40);
        assert_eq!(report.net_reclaimed(), 60);
    }

    #[test]
    fn free_space_delta_saturates() {
        let report = Report::new().with_space(100, 40);
        assert_eq!(report.freed_bytes(), 0);
        assert_eq!(Report::new().with_space(40, 100).freed_bytes(), 60);
    }

    #[test]
    fn summary_samples_first_and_last() {
        let mut report = Report::new();
        for i in 0..100 {
            report.push(entry(EntryKind::Delete, i));
        }
        let summary = report.to_json_summary(10);
        let entries = summary["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 10);
        assert_eq!(entries[0]["reclaimed"], 0);
        assert_eq!(entries[9]["reclaimed"], 99);
        assert_eq!(summary["entries_truncated"], true);
        assert_eq!(summary["items_count"], 100);
        assert_eq!(summary["by_option"][0]["files"], 100);
        assert_eq!(summary["by_option"][0]["entries"], 100);
    }

    #[test]
    fn summary_sampling_edge_cases() {
        let mut report = Report::new();
        for i in 0..5 {
            report.push(entry(EntryKind::Delete, i));
        }

        let one = report.to_json_summary(1);
        assert_eq!(one["entries"].as_array().unwrap().len(), 1);
        assert_eq!(one["entries_truncated"], true);

        let all = report.to_json_summary(0);
        assert_eq!(all["entries"].as_array().unwrap().len(), 5);
        assert_eq!(all["entries_truncated"], false);

        let exact = report.to_json_summary(5);
        assert_eq!(exact["entries"].as_array().unwrap().len(), 5);
        assert_eq!(exact["entries_truncated"], false);
    }

    #[test]
    fn json_contract_has_stable_keys() {
        let mut report = Report::new();
        report.push(entry(EntryKind::Delete, 1));
        report.fail("c", "o", "boom");
        let json = report.to_json();
        for key in [
            "schema_version",
            "reclaimed_bytes",
            "files_removed",
            "special_operations",
            "skipped",
            "errors",
            "blocked",
            "blocked_items",
            "warnings",
            "backed_up_bytes",
            "net_reclaimed_bytes",
            "aborted",
            "duration_ms",
            "bytes_affected",
            "items_count",
            "free_before",
            "free_after",
            "freed_bytes",
            "excluded",
            "entries",
            "failures",
        ] {
            assert!(json.get(key).is_some(), "missing {key}");
        }
        assert_eq!(json["errors"], 1);

        let summary = report.to_json_summary(10);
        for key in ["entries_truncated", "by_option", "entries", "failures"] {
            assert!(summary.get(key).is_some(), "summary missing {key}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_serialize_lossily() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(OsString::from_vec(b"/tmp/\xFF".to_vec()));
        let mut report = Report::new();
        report.push(Entry::new(
            EntryKind::Delete,
            "c",
            "o",
            "l",
            Some(&path),
            1,
        ));
        let json = report.to_json();
        assert!(json["entries"][0]["path"]
            .as_str()
            .unwrap()
            .contains('\u{FFFD}'));
    }
}
