//! Operation log line format: `<unix-ts> | <mode> | <cleaner>.<option> | <path> | <bytes>B`.
//!
//! The path may itself contain ` | `, so parsing is: first three fields are
//! fixed, the last field is the byte count, everything in between is the path.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Clean,
    Preview,
    Skip,
    Run,
    Truncate,
    Ostrash,
    Shred,
    Vacuum,
}

impl Mode {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "clean" => Some(Mode::Clean),
            "preview" => Some(Mode::Preview),
            "skip" => Some(Mode::Skip),
            "run" => Some(Mode::Run),
            "truncate" => Some(Mode::Truncate),
            "ostrash" => Some(Mode::Ostrash),
            "shred" => Some(Mode::Shred),
            "vacuum" => Some(Mode::Vacuum),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Clean => "clean",
            Mode::Preview => "preview",
            Mode::Skip => "skip",
            Mode::Run => "run",
            Mode::Truncate => "truncate",
            Mode::Ostrash => "ostrash",
            Mode::Shred => "shred",
            Mode::Vacuum => "vacuum",
        }
    }

    pub fn is_deletion(self) -> bool {
        matches!(
            self,
            Mode::Clean | Mode::Truncate | Mode::Ostrash | Mode::Shred | Mode::Vacuum
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub ts: u64,
    pub mode: Mode,
    pub cleaner: String,
    pub option: String,
    pub path: PathBuf,
    pub bytes: u64,
}

impl LogLine {
    pub fn key(&self) -> String {
        format!("{}.{}", self.cleaner, self.option)
    }
}

fn split_key(text: &str) -> (String, String) {
    match text.split_once('.') {
        Some((cleaner, option)) => (cleaner.to_string(), option.to_string()),
        None => (text.to_string(), String::new()),
    }
}

pub fn parse(line: &str) -> Option<LogLine> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        return None;
    }
    let parts: Vec<&str> = line.split(" | ").collect();
    if parts.len() < 5 {
        return None;
    }
    let mode = Mode::parse(parts[1])?;
    let ts: u64 = parts[0].trim().parse().ok()?;
    let bytes = parts
        .last()?
        .trim()
        .strip_suffix('B')?
        .trim()
        .parse::<u64>()
        .ok()?;
    let path = PathBuf::from(parts[3..parts.len() - 1].join(" | "));
    let (cleaner, option) = split_key(parts[2].trim());
    if cleaner.is_empty() || path.as_os_str().is_empty() {
        return None;
    }
    Some(LogLine {
        ts,
        mode,
        cleaner,
        option,
        path,
        bytes,
    })
}

pub fn parse_file(path: &Path) -> Vec<LogLine> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines().filter_map(parse).collect()
}

pub fn parse_chain(path: &Path) -> Vec<LogLine> {
    let mut out = Vec::new();
    let mut rotated = path.as_os_str().to_owned();
    rotated.push(".1");
    let rotated = Path::new(&rotated);
    if rotated != path {
        out.extend(parse_file(rotated));
    }
    out.extend(parse_file(path));
    out
}

pub fn default_log_file() -> PathBuf {
    if let Ok(base) = std::env::var("SWEEP_PORTABLE") {
        if !base.is_empty() {
            return PathBuf::from(base).join("operations.log");
        }
        let _ = base;
    }
    if let Some(mut dir) = dirs::data_dir() {
        dir.push("sweep");
        dir.push("operations.log");
        return dir;
    }
    PathBuf::from("operations.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_line() {
        let line = parse("123 | clean | demo.cache | /tmp/a.txt | 42B").unwrap();
        assert_eq!(line.ts, 123);
        assert_eq!(line.mode, Mode::Clean);
        assert_eq!(line.cleaner, "demo");
        assert_eq!(line.option, "cache");
        assert_eq!(line.path, PathBuf::from("/tmp/a.txt"));
        assert_eq!(line.bytes, 42);
    }

    #[test]
    fn separator_inside_path_survives() {
        let line = parse("1 | clean | a.b | /tmp/x | y.txt | 3B").unwrap();
        assert!(line.path.to_string_lossy().contains("x | y"));
    }

    #[test]
    fn run_marker_parses() {
        let line = parse("7 | run | r1 | all | 0B").unwrap();
        assert_eq!(line.mode, Mode::Run);
        assert_eq!(line.cleaner, "r1");
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(parse("not a log line").is_none());
        assert!(parse("").is_none());
    }
}
