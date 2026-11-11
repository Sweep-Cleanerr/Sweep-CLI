//! Importer for winapp2.ini (WinApp2 / CCleaner style definition files).
//!
//! One section becomes one cleaner; `FileKeyN` entries become `delete`
//! actions, `RegKeyN` become `winreg` actions. Sections whose `Detect`
//! or `DetectOS` gate fails on this host are skipped.

use crate::core::error::{Error, Result};
use crate::definition::model::{
    ActionDef, CleanerDef, OptionDef, OsFilter, SearchKind, Trust,
};

#[derive(Default)]
struct Ini {
    sections: Vec<(String, Vec<(String, String)>)>,
}

fn parse_ini(text: &str) -> Ini {
    let mut ini = Ini::default();
    let mut current: Option<usize> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let name = line[1..line.len() - 1].trim().to_string();
            ini.sections.push((name, Vec::new()));
            current = Some(ini.sections.len() - 1);
            continue;
        }
        if let Some(idx) = current {
            if let Some((key, value)) = line.split_once('=') {
                ini.sections[idx]
                    .1
                    .push((key.trim().to_lowercase(), value.trim().to_string()));
            }
        }
    }
    ini
}

fn slug(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let mut out = String::new();
    let mut prev_underscore = false;
    for c in cleaned.chars() {
        if c == '_' {
            if !prev_underscore && !out.is_empty() {
                out.push(c);
            }
            prev_underscore = true;
        } else {
            out.push(c.to_ascii_lowercase());
            prev_underscore = false;
        }
    }
    out.trim_matches('_').to_string()
}

fn fnmatch_regex(pattern: &str) -> String {
    let mut out = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '[' => {
                out.push('[');
                while let Some(&next) = chars.peek() {
                    if next == '[' {
                        // Escaped bracket in fnmatch means literal '['.
                    }
                    out.push(chars.next().unwrap());
                    if out.ends_with(']') {
                        break;
                    }
                }
            }
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '{' | '}' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

fn winapp_expand_vars(text: &str) -> Vec<String> {
    // Expand %VAR% and ${VAR} forms; unknown vars left as-is.
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    let mut result = Vec::new();
    while let Some(c) = chars.next() {
        if c == '%' {
            let mut name = String::new();
            let mut closed = false;
            for inner in chars.by_ref() {
                if inner == '%' {
                    closed = true;
                    break;
                }
                name.push(inner);
            }
            if closed && !name.is_empty() {
                if let Ok(value) = std::env::var(&name) {
                    out.push_str(&value);
                } else {
                    out.push('%');
                    out.push_str(&name);
                    out.push('%');
                }
                continue;
            } else {
                out.push('%');
                out.push_str(&name);
                continue;
            }
        }
        out.push(c);
    }
    // No cartesian expansion needed for winapp2 (no multi-value vars here).
    result.push(out);
    result
}

fn detect_os(_required: &str) -> bool {
    // Simplify: match against the host by delegating to the OS filter.
    // WinApp2 DetectOS typically names Windows versions; treat any
    // non-empty value as "Windows-only" and let the OS filter gate it.
    let os = OsFilter::new(_required);
    os.matches() || _required.trim().is_empty()
}

fn detect(ini_get: &str) -> bool {
    // `Detect` names a file/dir that must exist. Empty means always active.
    let v = ini_get.trim();
    if v.is_empty() {
        return true;
    }
    let v = v.replace("%APPDATA%", &crate::platform::home_dir().map(|h| h.join("AppData").join("Roaming").to_string_lossy().to_string()).unwrap_or_default());
    let v = v.replace("$APPDATA", &crate::platform::home_dir().map(|h| h.join("AppData").join("Roaming").to_string_lossy().to_string()).unwrap_or_default());
    std::path::Path::new(&v).exists()
}

fn filekey_actions(value: &str, excludes: &[String], file: &str) -> Result<Vec<ActionDef>> {
    let mut parts: Vec<&str> = value.split('|').collect();
    if parts.iter().all(|p| p.trim().is_empty()) {
        return Err(Error::definition(file, "empty FileKey"));
    }
    let dir_raw = parts.remove(0).trim();
    let filenames = if parts.is_empty() {
        String::new()
    } else {
        parts.remove(0).trim().to_string()
    };
    let mut recurse = false;
    let mut removeself = false;
    for flag in &parts {
        match flag.trim().to_uppercase().as_str() {
            "RECURSE" => recurse = true,
            "REMOVESELF" => {
                recurse = true;
                removeself = true;
            }
            "" => {}
            _ => {}
        }
    }
    let filenames = if filenames.is_empty() && recurse {
        "*.*".to_string()
    } else {
        filenames
    };
    let nwholeregex = join_excludes(excludes);

    let mut actions = Vec::new();
    for dirname in winapp_expand_vars(dir_raw) {
        let dirname = if dirname.len() == 2 && dirname.ends_with(':') {
            format!("{dirname}\\")
        } else {
            dirname
        };
        if !crate::core::path::looks_absolute(std::path::Path::new(&dirname)) {
            log::warn!("ignoring relative FileKey path '{dirname}' in {file}");
            continue;
        }
        for filename in filenames.split(';') {
            let filename = filename.trim();
            if filename.is_empty() {
                continue;
            }
            let pattern = if filename == "*.*" { "*" } else { filename };
            if recurse {
                let search = if removeself {
                    SearchKind::WalkAll
                } else {
                    SearchKind::WalkFiles
                };
                let action = ActionDef {
                    command: "delete".into(),
                    search,
                    path: dirname.clone(),
                    regex: if filename == "*.*" {
                        None
                    } else {
                        Some(format!("^{}$", fnmatch_regex(filename)))
                    },
                    nwholeregex: nwholeregex.clone(),
                    ..ActionDef::default()
                };
                actions.push(action);
            } else {
                let joined = std::path::Path::new(&dirname).join(pattern);
                let joined_text = joined.to_string_lossy().to_string();
                let search = if crate::fsutil::walk::has_glob(&joined_text) {
                    SearchKind::Glob
                } else {
                    SearchKind::File
                };
                actions.push(ActionDef {
                    command: "delete".into(),
                    search,
                    path: joined_text,
                    nwholeregex: nwholeregex.clone(),
                    ..ActionDef::default()
                });
            }
        }
    }
    Ok(actions)
}

fn join_excludes(excludes: &[String]) -> Option<String> {
    if excludes.is_empty() {
        return None;
    }
    let parts: Vec<String> = excludes
        .iter()
        .map(|e| e.trim().replace('\\', "\\\\").replace('.', "\\."))
        .filter(|e| !e.is_empty())
        .map(|e| format!("{e}"))
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(format!("({})", parts.join("|")))
    }
}

fn regkey_action(value: &str, reg_excludes: &[String]) -> Option<ActionDef> {
    let key = value.trim();
    if key.is_empty() {
        return None;
    }
    Some(ActionDef {
        command: "winreg".into(),
        path: key.to_string(),
        exclude_keys: reg_excludes.to_vec(),
        ..ActionDef::default()
    })
}

pub fn parse_winapp2(text: &str, file: &str, _trust: Trust) -> Vec<CleanerDef> {
    let ini = parse_ini(text);
    let mut out = Vec::new();
    for (section, entries) in &ini.sections {
        let get = |key: &str| -> String {
            entries
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        if !detect(&get("Detect")) {
            continue;
        }
        if !detect_os(&get("DetectOS")) {
            continue;
        }
        let id = slug(section);
        if id.is_empty() {
            continue;
        }
        let name = if section.is_empty() { id.clone() } else { section.clone() };
        let mut actions = Vec::new();
        let mut excludes = Vec::new();
        for n in 1..=16 {
            let filekey = get(&format!("FileKey{n}"));
            if !filekey.is_empty() {
                match filekey_actions(&filekey, &excludes, file) {
                    Ok(a) => actions.extend(a),
                    Err(err) => log::warn!("{err}"),
                }
            }
            let regkey = get(&format!("RegKey{n}"));
            if !regkey.is_empty() {
                if let Some(a) = regkey_action(&regkey, &excludes) {
                    actions.push(a);
                }
            }
            let exclude = get(&format!("ExcludeKey{n}"));
            if !exclude.is_empty() {
                excludes.push(exclude);
            }
        }
        if actions.is_empty() {
            continue;
        }
        out.push(CleanerDef {
            id: id.clone(),
            name,
            description: String::new(),
            os: OsFilter::any(),
            running: Vec::new(),
            vars: Vec::new(),
            options: vec![OptionDef {
                id: "default".to_string(),
                label: "Default".to_string(),
                description: String::new(),
                warning: String::new(),
                actions,
                os: OsFilter::any(),
            }],
        });
    }
    out
}

pub fn load_winapp2_dir_entry(
    path: &std::path::Path,
    trust: Trust,
) -> Vec<CleanerDef> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    parse_winapp2(&text, &path.to_string_lossy(), trust)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_filekey_and_regkey() {
        let ini = "[Sample App]\nFileKey1=C:\\Users\\you\\AppData\\Local\\Temp|*.*\nRegKey1=HKCU\\Software\\Sample\n";
        let defs = parse_winapp2(ini, "winapp2.ini", Trust::Trusted);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].options[0].actions.len(), 2);
    }

    #[test]
    fn relative_filekey_is_ignored() {
        let ini = "[X]\nFileKey1=relative\\path|*.*\n";
        let defs = parse_winapp2(ini, "winapp2.ini", Trust::Trusted);
        assert!(defs.is_empty());
    }
}
