//! Windows registry key/value deletion.

use crate::core::error::{Error, Result};

#[cfg(windows)]
fn hive_for(full: &str) -> (winreg::RegKey, String) {
    // Format: HKLM\SOFTWARE\X\Y or HKCU\...
    let (hive, rest) = full.split_once('\\').unwrap_or((full, ""));
    let hive = match hive.to_ascii_uppercase().as_str() {
        "HKLM" => winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE),
        "HKCU" => winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER),
        "HKCR" => winreg::RegKey::predef(winreg::enums::HKEY_CLASSES_ROOT),
        "HKU" => winreg::RegKey::predef(winreg::enums::HKEY_USERS),
        _ => winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE),
    };
    (hive, rest.to_string())
}

#[cfg(not(windows))]
pub fn reg_delete_key(_full: &str, _exclude: &[String], _really: bool) -> Result<bool> {
    Err(Error::msg("registry editing is Windows-only"))
}

#[cfg(windows)]
pub fn reg_delete_key(full: &str, exclude: &[String], really_delete: bool) -> Result<bool> {
    let (hive, rest) = hive_for(full);
    let key = hive.open_subkey(&rest).map_err(|err| Error::msg(format!("cannot open {full}: {err}")))?;
    let subs: Vec<String> = key.enum_keys().flatten().collect();
    let mut deleted_any = false;
    for sub in &subs {
        if exclude.iter().any(|e| e.eq_ignore_ascii_case(sub)) {
            continue;
        }
        if really_delete {
            key.delete_subkey_all(sub)
                .map_err(|err| Error::msg(format!("cannot delete {full}\\{sub}: {err}")))?;
        }
        deleted_any = true;
    }
    let values: Vec<String> = key.enum_values().flatten().map(|(n, _)| n).collect();
    for name in values {
        if exclude.iter().any(|e| e.eq_ignore_ascii_case(&name)) {
            continue;
        }
        if really_delete {
            key.delete_value(&name)
                .map_err(|err| Error::msg(format!("cannot delete value {name}: {err}")))?;
        }
        deleted_any = true;
    }
    if really_delete {
        let _ = hive.delete_subkey(&rest);
    }
    Ok(deleted_any)
}

#[cfg(windows)]
pub fn reg_delete_value(full: &str, name: &str, really_delete: bool) -> Result<bool> {
    let (hive, rest) = hive_for(full);
    let key = hive.open_subkey(&rest).map_err(|err| Error::msg(format!("cannot open {full}: {err}")))?;
    if key.get_value::<String, _>(name).is_err() {
        return Ok(false);
    }
    if really_delete {
        key.delete_value(name)
            .map_err(|err| Error::msg(format!("cannot delete {full}\\{name}: {err}")))?;
    }
    Ok(true)
}

#[cfg(not(windows))]
pub fn reg_delete_value(_full: &str, _name: &str, _really: bool) -> Result<bool> {
    Err(Error::msg("registry editing is Windows-only"))
}

#[cfg(windows)]
pub fn reg_read_string(full: &str, name: Option<&str>) -> Option<String> {
    let (hive, rest) = hive_for(full);
    let key = hive.open_subkey(&rest).ok()?;
    match name {
        Some(n) => key.get_value::<String, _>(n).ok(),
        None => None,
    }
}

#[cfg(not(windows))]
pub fn reg_read_string(_full: &str, _name: Option<&str>) -> Option<String> {
    None
}
