//! Sweep library.
//!
//! The library contains the destructive pipeline: errors, path handling, the
//! keep guard, reports, configuration, and cleaner definitions. The action
//! layer executes normalized definitions; the engine resolves selections and
//! drives those actions.

extern crate self as sweep;

pub mod action;
pub mod cli;
pub mod config;
pub mod core;
pub mod deep;
pub mod definition;
pub mod engine;
pub mod fsutil;
pub mod i18n;
pub mod platform;
pub mod shred;

pub use core::{Error, Result};
pub use definition::CleanerRegistry;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Creates a unique per-process scratch directory so parallel test binaries
/// cannot interfere with each other's cleanup.
#[cfg(test)]
pub(crate) fn fixture(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sweep_{name}_{}",
        std::process::id()
    ));

    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("cannot create test fixture");

    dir
}

/// Creates a directory junction (Windows `mklink /J`, no elevation needed);
/// `false` when the platform or policy refuses, so link tests can skip.
#[cfg(test)]
pub(crate) fn junction(target: &std::path::Path, link: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let command = format!("mklink /J \"{}\" \"{}\"", link.display(), target.display());
        std::process::Command::new("cmd")
            .arg("/C")
            .raw_arg(&command)
            .output()
            .is_ok_and(|out| out.status.success() && link.symlink_metadata().is_ok())
    }
    #[cfg(not(windows))]
    {
        let _ = (target, link);
        false
    }
}
