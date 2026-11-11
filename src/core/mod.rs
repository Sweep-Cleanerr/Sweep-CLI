//! Foundation types shared by every stage.

pub mod error;
pub mod gitguard;
pub mod keep;
pub mod path;
pub mod report;
pub mod report_html;
pub mod report_md;

pub use error::{Error, Result, WithPath};
pub use keep::{
    builtin_keep_list, protected_roots, user_data_subtree_roots, Guard, KeepList,
};
pub use report::{Entry, EntryKind, Failure, Report, Stopwatch, SCHEMA_VERSION};
