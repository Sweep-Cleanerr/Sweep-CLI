//! Linux specifics: FITRIM, journalctl vacuum, memory wiping.

use std::path::Path;

use crate::core::error::{Error, Result};

#[repr(C)]
struct FstrimRange {
    start: u64,
    len: u64,
    minlen: u64,
}

const FITRIM: libc::c_ulong = 0xC018_5879;

pub fn fitrim(path: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let target = if path.is_file() {
        path.parent().unwrap_or(Path::new("/")).to_path_buf()
    } else {
        path.to_path_buf()
    };
    if !target.exists() {
        return Err(Error::io(
            &target,
            std::io::Error::from_raw_os_error(libc::ENOENT),
        ));
    }
    let c_path = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| Error::msg("path contains a NUL byte"))?;
    unsafe {
        let fd = libc::open(
            c_path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        );
        if fd < 0 {
            return Err(Error::io(&target, std::io::Error::last_os_error()));
        }
        let mut range = FstrimRange {
            start: 0,
            len: u64::MAX,
            minlen: 0,
        };
        let rc = libc::ioctl(
            fd,
            FITRIM,
            &mut range as *mut FstrimRange as *mut libc::c_void,
        );
        let err = if rc != 0 {
            Some(std::io::Error::last_os_error())
        } else {
            None
        };
        libc::close(fd);
        if let Some(err) = err {
            return Err(Error::io(&target, err));
        }
    }
    Ok(())
}

pub fn wipe_memory() -> Result<()> {
    use std::process::Command;

    if unsafe { libc::geteuid() } != 0 {
        return Err(Error::msg("wiping memory requires root"));
    }
    if let Err(err) = std::fs::write("/proc/sys/vm/drop_caches", "3\n") {
        log::warn!("cannot drop caches: {err}");
    }
    let swaps = std::fs::read_to_string("/proc/swaps").unwrap_or_default();
    let devices: Vec<String> = swaps
        .lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().next())
        .filter(|d| d.starts_with('/'))
        .map(|d| d.to_string())
        .collect();
    let mut swapped_off = Vec::new();
    for device in &devices {
        match Command::new("swapoff").arg(device).status() {
            Ok(s) if s.success() => swapped_off.push(device.clone()),
            Ok(s) => log::warn!("swapoff {device} exited with {s}"),
            Err(e) => log::warn!("swapoff {device} failed: {e}"),
        }
    }
    let mut failed = Vec::new();
    for device in &swapped_off {
        match Command::new("swapon").arg(device).status() {
            Ok(s) if s.success() => {}
            Ok(s) => {
                log::warn!("swapon {device} exited with {s}");
                failed.push(device.clone());
            }
            Err(e) => {
                log::warn!("swapon {device} failed: {e}");
                failed.push(device.clone());
            }
        }
    }
    if !failed.is_empty() {
        return Err(Error::msg(format!(
            "swap could not be re-enabled on {} — run `swapon <device>` manually",
            failed.join(", ")
        )));
    }
    Ok(())
}

pub fn journald_clean(vacuum_days: u32) -> Result<u64> {
    let time_arg = format!("--vacuum-time={vacuum_days}d");
    let before = crate::fsutil::dir_size(Path::new("/var/log/journal"));
    let output = std::process::Command::new("journalctl")
        .args([time_arg.as_str()])
        .output()
        .map_err(|err| Error::Spawn {
            command: format!("journalctl {time_arg}"),
            reason: err.to_string(),
        })?;
    if !output.status.success() {
        return Err(Error::ExternalCommand {
            command: format!("journalctl {time_arg}"),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let after = crate::fsutil::dir_size(Path::new("/var/log/journal"));
    Ok(before.saturating_sub(after))
}

pub fn flush_dns() -> Result<()> {
    let mut last_error: Option<Error> = None;
    for (program, args) in [
        ("resolvectl", vec!["flush-caches"]),
        ("systemd-resolve", vec!["--flush-caches"]),
    ] {
        if !crate::fsutil::path_exists_in_path(program) {
            continue;
        }
        let output = std::process::Command::new(program)
            .args(&args)
            .output()
            .map_err(|err| Error::Spawn {
                command: format!("{program} {}", args.join(" ")),
                reason: err.to_string(),
            })?;
        if output.status.success() {
            return Ok(());
        }
        last_error = Some(Error::ExternalCommand {
            command: format!("{program} {}", args.join(" ")),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Err(last_error.unwrap_or_else(|| Error::msg("neither resolvectl nor systemd-resolve is installed")))
}
