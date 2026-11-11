//! External command helpers used by system actions.

use std::path::Path;

use crate::core::error::{Error, Result};

/// Is `program` on `$PATH`?
pub fn path_exists_in_path(program: &str) -> bool {
    if program.contains('/') || program.contains(std::path::MAIN_SEPARATOR) {
        return Path::new(program).exists();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        for ext in [".exe", ".cmd", ".bat"] {
            let with_ext = dir.join(format!("{program}{ext}"));
            if with_ext.is_file() {
                let name = with_ext.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if name.to_ascii_lowercase().contains(&program.to_ascii_lowercase()) {
                    return true;
                }
            }
        }
    }
    false
}

/// Run `program` with `args`, optionally waiting.
///
/// Returns `(exit_code, stdout, stderr)`.
/// `wait == false` fires and forgets; exit code is then 0.
pub fn run_command(
    program: &str,
    args: &[&str],
    wait: bool,
) -> Result<(i32, String, String)> {
    use std::process::Command;

    let output = if wait {
        Command::new(program).args(args).output().map_err(|err| Error::Spawn {
            command: format!("{} {}", program, args.join(" ")),
            reason: err.to_string(),
        })?
    } else {
        Command::new(program)
            .args(args)
            .spawn()
            .map_err(|err| Error::Spawn {
                command: format!("{} {}", program, args.join(" ")),
                reason: err.to_string(),
            })?;
        return Ok((0, String::new(), String::new()));
    };

    let code = output.status.code().unwrap_or(-1);
    Ok((
        code,
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    ))
}

/// Run with a working directory (git needs `root` as cwd).
pub fn run_command_in(
    program: &str,
    args: &[&str],
    dir: &Path,
    _wait: bool,
) -> Result<(i32, String, String)> {
    use std::process::Command;

    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|err| Error::Spawn {
            command: format!("{} {}", program, args.join(" ")),
            reason: err.to_string(),
        })?;
    let code = output.status.code().unwrap_or(-1);
    Ok((
        code,
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_programs_are_found() {
        assert!(path_exists_in_path("cmd") || path_exists_in_path("powershell") || path_exists_in_path("sh") || path_exists_in_path("bash"));
    }

    #[test]
    fn run_command_captures_exit_and_output() {
        #[cfg(windows)]
        let (code, stdout, _) = run_command("cmd", &["/c", "echo hi"], true).unwrap();
        #[cfg(not(windows))]
        let (code, stdout, _) = run_command("sh", &["-c", "echo hi"], true).unwrap();
        assert_eq!(code, 0);
        assert!(stdout.contains("hi"));
    }
}
