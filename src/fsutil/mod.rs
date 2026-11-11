//! Filesystem primitives: traversal, sizing, deletion.

pub mod delete;
pub mod freespace;
pub mod media;
pub mod reflink;
pub mod reserve;
pub mod run;
pub mod size;
pub mod walk;

pub use delete::{delete, delete_all, truncate_file, DeleteOptions};
pub use freespace::wipe_free_space;
pub use media::{device_for_mount, parent_block_device, parse_rotational, suggested_action, MediaType};
#[cfg(unix)]
pub use media::detect_for_mount;
pub use run::{path_exists_in_path, run_command, run_command_in};
pub use size::{
    bytes_to_human, dir_size, human_to_bytes, size_of, size_of_or_zero,
};
pub use walk::{child_dirs, children, glob_dirs, glob_paths, is_empty_dir, scan_paths, ScanOptions};
