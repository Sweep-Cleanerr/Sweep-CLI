//! Minimal progress reporting for the engine.
//!
//! The worker reports units of work; the CLI renders them either as
//! human text or as `--progress-json` events. Kept deliberately small:
//! no terminal UI, just counters and elapsed time.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[derive(Default)]
pub struct Progress {
    total: AtomicUsize,
    done: AtomicUsize,
    started: Option<Instant>,
}

impl Progress {
    pub fn new() -> Self {
        Self {
            total: AtomicUsize::new(0),
            done: AtomicUsize::new(0),
            started: Some(Instant::now()),
        }
    }

    pub fn set_total(&self, total: usize) {
        self.total.store(total, Ordering::Relaxed);
    }

    pub fn inc(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> (usize, usize, u128) {
        let total = self.total.load(Ordering::Relaxed);
        let done = self.done.load(Ordering::Relaxed);
        let elapsed_ms = self
            .started
            .map(|started| started.elapsed().as_millis())
            .unwrap_or(0);
        (done, total, elapsed_ms)
    }

    pub fn fraction(&self) -> f64 {
        let (done, total, _) = self.snapshot();
        if total == 0 {
            0.0
        } else {
            done as f64 / total as f64
        }
    }

    /// One `--progress-json` line (without newline).
    pub fn json_line(&self, cleaner: &str, option: &str) -> String {
        let (done, total, elapsed_ms) = self.snapshot();
        serde_json::json!({
            "cleaner": cleaner,
            "option": option,
            "done": done,
            "total": total,
            "percent": (self.fraction() * 100.0) as u64,
            "elapsed_ms": elapsed_ms,
        })
        .to_string()
    }
}

pub type SharedProgress = Arc<Progress>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_and_fraction() {
        let progress = Progress::new();
        progress.set_total(4);
        assert_eq!(progress.fraction(), 0.0);
        progress.inc();
        progress.inc();
        assert_eq!(progress.fraction(), 0.5);
        let line = progress.json_line("demo", "cache");
        assert!(line.contains("\"cleaner\":\"demo\""));
    }
}
