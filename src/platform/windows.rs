//! Win32 queries needed by fsutil.
//!
//! The few kernel32 calls we need are declared directly instead of pulling
//! in a bindings crate, keeping the dependency tree small.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::core::error::{Error, Result};

const INVALID_FILE_ATTRIBUTES: u32 = 0xFFFF_FFFF;
const INVALID_HANDLE_VALUE: isize = -1;
const FILE_ATTRIBUTE_READONLY: u32 = 0x0000_0001;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const FILE_SHARE_DELETE: u32 = 0x0000_0004;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

/// `BY_HANDLE_FILE_INFORMATION`; each `FILETIME` is two `DWORD`s.
#[repr(C)]
struct ByHandleFileInformation {
    file_attributes: u32,
    creation_time: [u32; 2],
    last_access_time: [u32; 2],
    last_write_time: [u32; 2],
    volume_serial_number: u32,
    file_size_high: u32,
    file_size_low: u32,
    number_of_links: u32,
    file_index_high: u32,
    file_index_low: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFileAttributesW(path: *const u16) -> u32;

    fn SetFileAttributesW(path: *const u16, attrs: u32) -> i32;

    fn GetDiskFreeSpaceExW(
        path: *const u16,
        free_available: *mut u64,
        total: *mut u64,
        total_free: *mut u64,
    ) -> i32;

    fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut std::ffi::c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;

    fn GetFileInformationByHandle(
        handle: *mut std::ffi::c_void,
        info: *mut ByHandleFileInformation,
    ) -> i32;

    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
}

/// Creates a NUL-terminated UTF-16 buffer for `path`, using the
/// extended-length path representation.
fn to_wide(path: &Path) -> Vec<u16> {
    let text = crate::core::path::extended_path(path);
    let mut wide: Vec<u16> = OsStr::new(&text).encode_wide().collect();
    wide.push(0);
    wide
}

fn file_attributes(path: &Path) -> Option<u32> {
    let wide = to_wide(path);

    // SAFETY: `wide` is NUL-terminated, and the call only reads it.
    let attributes = unsafe { GetFileAttributesW(wide.as_ptr()) };

    (attributes != INVALID_FILE_ATTRIBUTES).then_some(attributes)
}

/// Returns metadata for the name itself, never its reparse target
/// (`lstat` semantics).
///
/// Requesting no access is sufficient for metadata and can succeed even when
/// the file contents cannot be read.
fn handle_info(path: &Path) -> Option<ByHandleFileInformation> {
    let wide = to_wide(path);

    // SAFETY:
    // - `wide` is NUL-terminated.
    // - `info` is a valid output pointer for the duration of the call.
    // - A successful handle is closed before this function returns.
    unsafe {
        let handle = CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        );

        if handle as isize == INVALID_HANDLE_VALUE {
            return None;
        }

        let mut info: ByHandleFileInformation = std::mem::zeroed();
        let result = GetFileInformationByHandle(handle, &mut info);
        let _ = CloseHandle(handle);

        (result != 0).then_some(info)
    }
}

/// Returns the volume serial number, the Windows counterpart of `st_dev`.
pub fn device_id(path: &Path) -> Option<u64> {
    handle_info(path).map(|info| u64::from(info.volume_serial_number))
}

/// Returns whether the file has more than one name.
/// Directories are never considered hard links here.
pub fn is_hard_link(path: &Path) -> bool {
    handle_info(path).is_some_and(|info| {
        info.file_attributes & FILE_ATTRIBUTE_DIRECTORY == 0
            && info.number_of_links > 1
    })
}

/// Returns `(volume serial, file index)`, the pair used by Windows to
/// determine whether two names refer to the same file.
pub fn file_identity(path: &Path) -> Option<(u64, u64)> {
    handle_info(path).map(|info| {
        let index = (u64::from(info.file_index_high) << 32)
            | u64::from(info.file_index_low);

        (u64::from(info.volume_serial_number), index)
    })
}

/// Drops the read-only attribute.
///
/// Returns `true` only when the attribute was present and successfully
/// cleared.
pub fn clear_readonly(path: &Path) -> bool {
    match file_attributes(path) {
        Some(attributes) if attributes & FILE_ATTRIBUTE_READONLY != 0 => {
            let wide = to_wide(path);

            // SAFETY: `wide` is NUL-terminated, and only the attribute word
            // is changed.
            unsafe {
                SetFileAttributesW(
                    wide.as_ptr(),
                    attributes & !FILE_ATTRIBUTE_READONLY,
                ) != 0
            }
        }
        _ => false,
    }
}

/// Returns `(space available to this user, total capacity)` for the volume
/// containing `path`.
fn disk_space(path: &Path) -> Result<(u64, u64)> {
    let target = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    };

    let wide = to_wide(&target);
    let mut available = 0_u64;
    let mut total = 0_u64;
    let mut total_free = 0_u64;

    // SAFETY: `wide` is NUL-terminated, and all three output pointers refer
    // to valid, writable `u64` values.
    let result = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            &mut total,
            &mut total_free,
        )
    };

    if result == 0 {
        return Err(Error::io(
            path,
            std::io::Error::last_os_error(),
        ));
    }

    Ok((available, total))
}

pub fn free_space(path: &Path) -> Result<u64> {
    disk_space(path).map(|(available, _)| available)
}

pub fn total_space(path: &Path) -> Result<u64> {
    disk_space(path).map(|(_, total)| total)
}

pub fn recycle_bin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for drive in [b"C", b"D", b"E", b"F"] {
        let p = PathBuf::from(format!("{}:\\$Recycle.Bin", drive[0] as char));
        if p.is_dir() {
            dirs.push(p);
        }
    }
    dirs
}

pub fn empty_recycle_bin() -> Result<()> {
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHEmptyRecycleBinW(hwnd: *mut std::ffi::c_void, root: *const u16, flags: u32) -> i32;
    }
    unsafe {
        // SHERB_NOCONFIRMATION = 1, SHERB_NOPROGRESSUI = 2
        let result = SHEmptyRecycleBinW(std::ptr::null_mut(), std::ptr::null(), 1 | 2);
        let _ = OsStr::new("shell32");
        if result == 0 {
            Ok(())
        } else {
            Err(Error::io(Path::new("recycle bin"), std::io::Error::last_os_error()))
        }
    }
}

pub fn clear_clipboard() -> Result<()> {
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", "Set-Clipboard -Value ''"])
        .status()
        .map_err(|err| Error::msg(err.to_string()))?;
    if out.success() {
        Ok(())
    } else {
        Err(Error::msg("powershell Set-Clipboard failed"))
    }
}

pub fn flush_dns() -> Result<()> {
    let out = std::process::Command::new("ipconfig").arg("/flushdns").status()
        .map_err(|err| Error::msg(err.to_string()))?;
    if out.success() {
        Ok(())
    } else {
        Err(Error::msg("ipconfig /flushdns failed"))
    }
}

pub fn shell_change_notify() {
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHChangeNotify(event: u32, flags: u32, item: *const std::ffi::c_void, format: u32);
    }
    unsafe {
        // SHCNE_UPDATEDIR = 0x1000, SHCNF_IDLIST = 0
        SHChangeNotify(0x1000, 0, std::ptr::null(), 0);
    }
}

fn env_dir(names: &[&str]) -> Option<PathBuf> {
    for name in names {
        if let Ok(value) = std::env::var(name) {
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        }
    }
    None
}

pub fn windows_update_cache() -> Option<PathBuf> {
    env_dir(&["WINDIR"]).map(|w| w.join("SoftwareDistribution").join("Download"))
}

pub fn delivery_optimization_cache() -> Option<PathBuf> {
    env_dir(&["ALLUSERSPROFILE"]).map(|p| p.join("DeliveryOptimization"))
}

pub fn explorer_caches() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(local) = env_dir(&["LOCALAPPDATA"]) {
        out.push(local.join("Microsoft").join("Windows").join("Explorer"));
        out.push(local.join("IconCache.db"));
    }
    out
}

pub fn explorer_thumb_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(local) = env_dir(&["LOCALAPPDATA"]) {
        let dir = local.join("Microsoft").join("Windows").join("Explorer");
        if dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    if name.starts_with("thumbcache_") || name.starts_with("iconcache") {
                        out.push(entry.path());
                    }
                }
            }
        }
    }
    out
}

pub fn store_cache_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(local) = env_dir(&["LOCALAPPDATA"]) {
        out.push(local.join("Packages").join("Packages_1.0"));
    }
    out
}

pub fn d3d_shader_cache_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(local) = env_dir(&["LOCALAPPDATA"]) {
        out.push(local.join("D3DSCache"));
    }
    out
}

pub fn crash_dump_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(windir) = env_dir(&["WINDIR", "SYSTEMROOT"]) {
        out.push(windir.join("Minidump"));
        out.push(windir.join("MEMORY.DMP"));
        out.push(windir.join("CrashDumps"));
    }
    out
}

pub fn memory_dumps() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(windir) = env_dir(&["WINDIR", "SYSTEMROOT"]) {
        out.push(windir.join("MEMORY.DMP"));
        out.push(windir.join("Minidump"));
    }
    out
}

pub fn wer_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(prog) = env_dir(&["PROGRAMDATA"]) {
        out.push(prog.join("Microsoft").join("Windows").join("WER"));
    }
    out
}

pub fn prefetch_dir() -> Option<PathBuf> {
    env_dir(&["WINDIR", "SYSTEMROOT"]).map(|w| w.join("Prefetch"))
}

pub fn trim(_path: &Path) -> Result<()> {
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", "Optimize-Volume -ReTrim -ErrorAction Stop"])
        .status()
        .map_err(|err| Error::msg(err.to_string()))?;
    if out.success() {
        Ok(())
    } else {
        Err(Error::msg("Optimize-Volume failed"))
    }
}
