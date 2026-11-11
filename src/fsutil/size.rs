//! Size accounting and human-readable formatting.
//!
//! BleachBit reports `st_blocks * 512` on POSIX because that is what the file
//! actually costs on disk; the apparent size is exposed separately so JSON
//! output can distinguish "freed" from "logical".

use std::fs::Metadata;
use std::path::{Path, PathBuf};

use crate::core::error::{Error, Result};
use crate::fsutil::walk::{scan_paths, ScanOptions};
use crate::platform;

/// Bytes actually allocated on disk (blocks × 512 on POSIX, `len()` elsewhere).
pub fn allocated_size(metadata: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        metadata.blocks().saturating_mul(512)
    }

    #[cfg(not(unix))]
    {
        metadata.len()
    }
}

/// Apparent (logical) file size.
pub fn apparent_size(metadata: &Metadata) -> u64 {
    metadata.len()
}

/// Hardlink-aware tree measurement: a hardlinked inode counts once, so a
/// rescan after deduplication shows the space actually gained.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Measured {
    /// Allocated on-disk bytes; a hardlinked inode counts once.
    pub bytes: u64,

    /// Regular files counted (hardlinks once).
    pub files: u64,
}

/// Measures a tree with hardlink deduplication.
///
/// Symlinks are not followed and their targets are not counted.
///
/// [`dir_size`] (deduplication) and [`tree_size`]/[`total_size`] (no
/// deduplication) can differ on the same tree when hardlinks exist.
pub fn measure_tree_dedup(root: &Path) -> Measured {
    use std::collections::HashSet;

    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut measured = Measured::default();

    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let Ok(entry) = entry else {
            continue;
        };

        let path = entry.path();
        let Ok(metadata) = path.symlink_metadata() else {
            continue;
        };

        if !metadata.file_type().is_file() {
            continue;
        }

        // A hard link is another name for the same bytes: count it once on
        // every platform. `is_hard_link` reads `nlink` on POSIX and the link
        // count from the handle on Windows; `file_identity` supplies the key.
        if platform::is_hard_link(path) {
            if let Some(key) = platform::file_identity(path) {
                if !seen.insert(key) {
                    continue;
                }
            }
        }

        measured.bytes = measured
            .bytes
            .saturating_add(allocated_size(&metadata));

        measured.files = measured.files.saturating_add(1);
    }

    measured
}

/// Allocated size of `path`; permission errors count as zero rather than
/// failing the scan (the BleachBit convention).
pub fn size_of(path: &Path) -> Result<u64> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Ok(0);
        }
        Err(error) => return Err(Error::io(path, error)),
    };

    Ok(allocated_size(&metadata))
}

/// Allocated size of `path`, zero for anything that vanished mid-scan.
pub fn size_of_or_zero(path: &Path) -> u64 {
    size_of(path).unwrap_or_else(|error| {
        if !error.is_benign() {
            log::debug!("size_of({}) failed: {}", path.display(), error);
        }

        0
    })
}

/// Total allocated size inside `dir` (recursive), counting hardlinked inodes
/// only once.
pub fn dir_size(dir: &Path) -> u64 {
    measure_tree_dedup(dir).bytes
}

/// Free space available to the current user at `path`.
pub fn free_space(path: &Path) -> Result<u64> {
    platform::free_space(path)
}

/// Total capacity of the filesystem holding `path`.
pub fn total_space(path: &Path) -> Result<u64> {
    platform::total_space(path)
}

const SI: [&str; 7] = ["", "k", "M", "G", "T", "P", "E"];
const IEC: [&str; 7] = ["", "Ki", "Mi", "Gi", "Ti", "Pi", "Ei"];

/// `B` → integer, `k`/`M` → one decimal, `G` and up → two (BleachBit).
fn decimals_for(prefix_index: usize) -> usize {
    match prefix_index {
        0 => 0,
        1 | 2 => 1,
        _ => 2,
    }
}

/// Formats a byte count the BleachBit way (`1.4MB` / `1.3MiB`).
///
/// Rounding never mints a `1000.0`-style mantissa: `999_999` prints `1.0MB`,
/// not `1000.0kB`.
pub fn bytes_to_human(bytes: u64, iec: bool) -> String {
    if bytes == 0 {
        return "0B".into();
    }

    let (prefixes, base): (&[&str], f64) = if iec {
        (&IEC, 1024.0)
    } else {
        (&SI, 1000.0)
    };

    let mut value = bytes as f64;
    let mut index = 0_usize;

    while value >= base && index + 1 < prefixes.len() {
        value /= base;
        index += 1;
    }

    let mut decimals = decimals_for(index);

    // Rounding can carry the display over the base
    // (999.999 → "1000.0kB"); move up one prefix when that would happen.
    if index + 1 < prefixes.len()
        && value + 0.5 / 10_f64.powi(decimals as i32) >= base
    {
        value /= base;
        index += 1;
        decimals = decimals_for(index);
    }

    format!("{value:.decimals$}{}B", prefixes[index])
}

/// Parses `1G`, `500M`, `2.5GiB`, `1024`, etc. into a byte count.
///
/// Understands every unit [`bytes_to_human`] prints, up to `EB`/`EiB`.
/// Values that do not fit in `u64` are rejected, never clamped.
pub fn human_to_bytes(text: &str) -> Result<u64> {
    let text = text.trim();

    if text.is_empty() {
        return Err(Error::msg("empty size"));
    }

    let split = text.find(|character: char| {
        !(character.is_ascii_digit() || character == '.')
    });

    let (number, suffix) = match split {
        Some(index) => (&text[..index], text[index..].trim()),
        None => (text, ""),
    };

    let number: f64 = number
        .parse()
        .map_err(|_| Error::msg(format!("not a number: '{number}'")))?;

    let multiplier: f64 = match suffix.to_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" => 1_000.0,
        "kib" => 1024.0,
        "m" | "mb" => 1_000_000.0,
        "mib" => 1024.0 * 1024.0,
        "g" | "gb" => 1_000_000_000.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" => 1_000_000_000_000.0,
        "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        "p" | "pb" => 1_000_000_000_000_000.0,
        "pib" => 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0,
        "e" | "eb" => 1_000_000_000_000_000_000.0,
        "eib" => {
            1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0
        }
        other => {
            return Err(Error::msg(format!(
                "unknown size suffix '{other}'"
            )));
        }
    };

    // An `as u64` cast would silently clamp "20EB" to `u64::MAX`.
    let bytes = number * multiplier;

    if !bytes.is_finite() || bytes < 0.0 || bytes > u64::MAX as f64 {
        return Err(Error::msg(format!("size out of range: '{text}'")));
    }

    Ok(bytes.round() as u64)
}

/// Sum of allocated sizes for a list of paths (parallel).
///
/// No hard-link deduplication is performed; see [`measure_tree_dedup`] when
/// the list may name one inode several times.
pub fn total_size(paths: &[PathBuf]) -> u64 {
    use rayon::prelude::*;

    paths
        .par_iter()
        .map(|path| size_of_or_zero(path))
        .reduce(|| 0_u64, u64::saturating_add)
}

/// Recursively sums a tree using the shared scan options.
///
/// No hard-link deduplication is performed, unlike [`dir_size`].
pub fn tree_size(root: &Path, options: &ScanOptions) -> u64 {
    scan_paths(root, options).iter().fold(0_u64, |total, path| {
        total.saturating_add(size_of_or_zero(path))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random fill: a compressing filesystem cannot
    /// shrink it below the logical size the way it could zero-filled data.
    fn incompressible(len: usize) -> Vec<u8> {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;

        (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);

                (state >> 33) as u8
            })
            .collect()
    }

    #[test]
    fn human_formatting_matches_bleachbit_convention() {
        assert_eq!(bytes_to_human(0, false), "0B");
        assert_eq!(bytes_to_human(999, false), "999B");
        assert_eq!(bytes_to_human(1_000, false), "1.0kB");
        assert_eq!(bytes_to_human(1_500, false), "1.5kB");
        assert_eq!(bytes_to_human(2_000_000_000, false), "2.00GB");
        assert_eq!(bytes_to_human(1_024, true), "1.0KiB");
        assert_eq!(bytes_to_human(1_048_576, true), "1.0MiB");
        assert!(bytes_to_human(u64::MAX, false).ends_with('B'));
    }

    #[test]
    fn rounding_bumps_the_unit_instead_of_minting_1000() {
        assert_eq!(bytes_to_human(999_999, false), "1.0MB");
        assert_eq!(bytes_to_human(999_999_999, false), "1.00GB");
        assert_eq!(bytes_to_human(999_400, false), "999.4kB");
    }

    #[test]
    fn parses_si_and_iec_sizes() {
        assert_eq!(human_to_bytes("1024").unwrap(), 1_024);
        assert_eq!(human_to_bytes("1kB").unwrap(), 1_000);
        assert_eq!(human_to_bytes("1kib").unwrap(), 1_024);
        assert_eq!(human_to_bytes("500M").unwrap(), 500_000_000);
        assert_eq!(human_to_bytes("2.5GiB").unwrap(), 2_684_354_560);
        assert_eq!(human_to_bytes("1 T").unwrap(), 1_000_000_000_000);
        assert_eq!(human_to_bytes("42b").unwrap(), 42);

        // Every unit printed by bytes_to_human must parse back.
        assert_eq!(
            human_to_bytes("1.0PB").unwrap(),
            1_000_000_000_000_000
        );
        assert_eq!(human_to_bytes("1.0PiB").unwrap(), 1024_u64.pow(5));
        assert_eq!(
            human_to_bytes("1.0EB").unwrap(),
            1_000_000_000_000_000_000
        );
        assert_eq!(human_to_bytes("1.0EiB").unwrap(), 1024_u64.pow(6));
    }

    #[test]
    fn rejects_bad_and_out_of_range_sizes() {
        for bad in [
            "", "   ", "abc", "5X", "-1", "1.2.3G", "20EB", "20EiB",
        ] {
            assert!(
                human_to_bytes(bad).is_err(),
                "'{bad}' should not parse"
            );
        }

        assert!(human_to_bytes("18EB").is_ok());
    }

    #[test]
    fn sizes_a_real_file() {
        let dir = crate::fixture("size_file");
        let file = dir.join("f.bin");

        std::fs::write(&file, incompressible(5000)).unwrap();

        let size = size_of(&file).unwrap();

        // POSIX reports blocks * 512, while Windows uses len(); both should
        // normally be at least the logical size.
        assert!(size >= 5000, "reported {size} for a 5000-byte file");
        assert_eq!(
            apparent_size(&std::fs::metadata(&file).unwrap()),
            5000
        );
        assert_eq!(size_of_or_zero(&dir.join("missing")), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn measure_tree_counts_hardlinks_once() {
        let dir = crate::fixture("size_dedup");

        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.bin"), incompressible(8192)).unwrap();

        // NTFS and POSIX support hard links; FAT32 does not.
        let linked = std::fs::hard_link(
            dir.join("a.bin"),
            dir.join("sub/b.bin"),
        )
        .is_ok();

        let measured = measure_tree_dedup(&dir);

        assert_eq!(measured.files, if linked { 1 } else { 2 });
        assert!(measured.bytes >= 8192);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn totals_sum_paths() {
        let dir = crate::fixture("size_total");

        let paths: Vec<PathBuf> = (0..4)
            .map(|index| {
                let path = dir.join(format!("f{index}.bin"));
                std::fs::write(&path, incompressible(2048)).unwrap();
                path
            })
            .collect();

        assert!(total_size(&paths) >= 4 * 2048);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tree_size_respects_scan_options() {
        let dir = crate::fixture("size_tree");

        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("top.bin"), incompressible(4096)).unwrap();
        std::fs::write(
            dir.join("sub/inner.bin"),
            incompressible(4096),
        )
        .unwrap();

        let full = tree_size(&dir, &ScanOptions::files());
        let shallow = tree_size(
            &dir,
            &ScanOptions::files().with_max_depth(1),
        );

        assert!(full >= 8192);
        assert!(shallow >= 4096 && shallow < full);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn free_and_total_space_are_sane() {
        let dir = crate::fixture("size_space");
        let free = free_space(&dir).unwrap();
        let total = total_space(&dir).unwrap();

        assert!(free > 0);
        assert!(total >= free);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
