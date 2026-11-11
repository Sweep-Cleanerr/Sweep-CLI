//! Engine: selection resolution, execution, backups, trash, undo.

pub mod backup;
pub mod logline;
pub mod progress;
pub mod trash;
pub mod undo;
pub mod worker;

pub use worker::{resolve, run, SelectedOption};
