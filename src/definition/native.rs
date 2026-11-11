//! Parser for Sweep's native TOML cleaner format.

use std::collections::HashSet;

use serde::de::{self, Deserializer};
use serde::Deserialize;

use crate::core::error::{Error, Result};
use crate::definition::model::{
    ActionDef, CleanerDef, ObjectType, OptionDef, OsFilter, RunningCheck,
    SearchKind, VarDef, VarSearch, VarValue,
};

pub fn parse_native(text: &str, file: &str) -> Result<CleanerDef> {
    let raw: RawCleaner = toml::from_str(text)
        .map_err(|error| Error::definition(file, error.to_string()))?;

    convert(raw, file)
}

#[derive(Debug, Deserialize)]
struct RawCleaner {
    id: String,

    #[serde(default)]
    name: String,

    #[serde(default)]
    description: String,

    #[serde(default)]
    os: String,

    #[serde(default)]
    running: Vec<RawRunning>,

    #[serde(default)]
    var: Vec<RawVar>,

    #[serde(default)]
    option: Vec<RawOption>,
}

#[derive(Debug, Deserialize)]
struct RawRunning {
    #[serde(rename = "type", default)]
    kind: String,

    #[serde(default)]
    value: String,

    #[serde(default)]
    os: String,

    #[serde(default)]
    same_user: bool,
}

#[derive(Debug, Deserialize)]
struct RawVar {
    #[serde(default)]
    name: String,

    #[serde(default)]
    value: Vec<RawVarValue>,
}

#[derive(Debug, Deserialize)]
struct RawVarValue {
    #[serde(default)]
    value: String,

    #[serde(default)]
    search: String,

    #[serde(default)]
    os: String,

    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct RawOption {
    #[serde(default)]
    id: String,

    #[serde(default)]
    label: String,

    #[serde(default)]
    description: String,

    #[serde(default)]
    warning: String,

    #[serde(default)]
    os: String,

    #[serde(default)]
    action: Vec<RawAction>,
}

#[derive(Debug, Default, Deserialize)]
struct RawAction {
    #[serde(default)]
    command: String,

    #[serde(default)]
    search: String,

    #[serde(default)]
    path: String,

    #[serde(default)]
    os: String,

    #[serde(default)]
    regex: String,

    #[serde(default)]
    nregex: String,

    #[serde(default)]
    wholeregex: String,

    #[serde(default)]
    nwholeregex: String,

    #[serde(rename = "type", default)]
    kind: String,

    #[serde(default)]
    max_age_days: Option<u32>,

    #[serde(default)]
    min_age_days: Option<u32>,

    #[serde(default)]
    min_size: Option<ByteSize>,

    #[serde(default)]
    max_size: Option<ByteSize>,

    #[serde(default)]
    max_depth: Option<usize>,

    #[serde(default)]
    include_dirs: Option<bool>,

    #[serde(default)]
    section: Option<String>,

    #[serde(default)]
    parameter: Option<String>,

    #[serde(default)]
    address: Option<String>,

    #[serde(default)]
    cmd: Option<String>,

    #[serde(default = "default_true")]
    wait: bool,

    #[serde(default)]
    reg_name: String,

    #[serde(default)]
    exclude_keys: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn convert(raw: RawCleaner, file: &str) -> Result<CleanerDef> {
    let mut definition = CleanerDef::new(raw.id, raw.name);

    definition.description = raw.description;
    definition.os = OsFilter::new(raw.os);

    for check in raw.running {
        match check.kind.trim().to_ascii_lowercase().as_str() {
            "exe" => definition.running.push(RunningCheck::Exe {
                name: check.value,
                same_user: check.same_user,
                os: OsFilter::new(check.os),
            }),
            "path" => definition.running.push(RunningCheck::Path {
                pattern: check.value,
                os: OsFilter::new(check.os),
            }),
            "" => {
                return Err(Error::definition(
                    file,
                    "running check is missing its type",
                ));
            }
            other => {
                return Err(Error::definition(
                    file,
                    format!("unknown running check type '{other}'"),
                ));
            }
        }
    }

    let mut variable_names = HashSet::new();

    for variable in raw.var {
        if !variable_names.insert(variable.name.clone()) {
            return Err(Error::definition(
                file,
                format!("duplicate variable '{}'", variable.name),
            ));
        }

        let mut values = Vec::new();

        for value in variable.value {
            let search = match value
                .search
                .trim()
                .to_ascii_lowercase()
                .as_str()
            {
                "" | "literal" => VarSearch::Literal,
                "glob" => VarSearch::Glob,
                "winreg" | "registry" => VarSearch::Registry,
                other => {
                    return Err(Error::definition(
                        file,
                        format!("unknown variable search '{other}'"),
                    ));
                }
            };

            values.push(VarValue {
                raw: value.value,
                search,
                os: OsFilter::new(value.os),
                reg_name: non_empty(value.name),
            });
        }

        definition.vars.push(VarDef {
            name: variable.name,
            values,
        });
    }

    let mut option_ids = HashSet::new();

    for option in raw.option {
        if !option_ids.insert(option.id.clone()) {
            return Err(Error::definition(
                file,
                format!("duplicate option '{}'", option.id),
            ));
        }

        let mut actions = Vec::new();

        for action in option.action {
            actions.push(convert_action(action, file)?);
        }

        definition.options.push(OptionDef {
            id: option.id,
            label: option.label,
            description: option.description,
            warning: option.warning,
            actions,
            os: OsFilter::new(option.os),
        });
    }

    Ok(definition)
}

fn convert_action(raw: RawAction, file: &str) -> Result<ActionDef> {
    let search = if raw.search.trim().is_empty() {
        if has_glob(&raw.path) {
            SearchKind::Glob
        } else {
            SearchKind::File
        }
    } else {
        SearchKind::parse(&raw.search).ok_or_else(|| {
            Error::definition(
                file,
                format!("unknown search mode '{}'", raw.search),
            )
        })?
    };

    let object_type = match raw.kind.trim().to_ascii_lowercase().as_str() {
        "" => None,
        "f" | "file" => Some(ObjectType::File),
        "d" | "dir" | "directory" => Some(ObjectType::Dir),
        other => {
            return Err(Error::definition(
                file,
                format!("unknown action type '{other}'"),
            ));
        }
    };

    let min_size = raw.min_size.map(|size| size.0);
    let max_size = raw.max_size.map(|size| size.0);

    if let (Some(minimum), Some(maximum)) = (min_size, max_size) {
        if minimum > maximum {
            return Err(Error::definition(
                file,
                format!("min_size {minimum} exceeds max_size {maximum}"),
            ));
        }
    }

    if let (Some(minimum), Some(maximum)) =
        (raw.min_age_days, raw.max_age_days)
    {
        if minimum > maximum {
            return Err(Error::definition(
                file,
                format!(
                    "min_age_days {minimum} exceeds max_age_days {maximum}"
                ),
            ));
        }
    }

    Ok(ActionDef {
        command: if raw.command.trim().is_empty() {
            "delete".to_string()
        } else {
            raw.command
        },
        search,
        path: raw.path,
        os: OsFilter::new(raw.os),
        regex: non_empty(raw.regex),
        nregex: non_empty(raw.nregex),
        wholeregex: non_empty(raw.wholeregex),
        nwholeregex: non_empty(raw.nwholeregex),
        object_type,
        max_age_days: raw.max_age_days,
        min_age_days: raw.min_age_days,
        min_size,
        max_size,
        max_depth: raw.max_depth,
        include_dirs: raw.include_dirs,
        section: raw.section,
        parameter: raw.parameter,
        address: raw.address,
        cmd: raw.cmd,
        wait: raw.wait,
        reg_name: non_empty(raw.reg_name),
        exclude_keys: raw.exclude_keys,
    })
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim();

    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn has_glob(path: &str) -> bool {
    path.chars()
        .any(|character| matches!(character, '*' | '?' | '[' | ']' | '{'))
}

#[derive(Debug, Clone, Copy)]
struct ByteSize(u64);

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SizeVisitor;

        impl<'de> de::Visitor<'de> for SizeVisitor {
            type Value = ByteSize;

            fn expecting(
                &self,
                formatter: &mut std::fmt::Formatter,
            ) -> std::fmt::Result {
                formatter.write_str(
                    "a byte count or a size string like 500MB",
                )
            }

            fn visit_u64<E: de::Error>(
                self,
                value: u64,
            ) -> std::result::Result<Self::Value, E> {
                Ok(ByteSize(value))
            }

            fn visit_i64<E: de::Error>(
                self,
                value: i64,
            ) -> std::result::Result<Self::Value, E> {
                if value < 0 {
                    Err(E::custom("size cannot be negative"))
                } else {
                    Ok(ByteSize(value as u64))
                }
            }

            fn visit_str<E: de::Error>(
                self,
                value: &str,
            ) -> std::result::Result<Self::Value, E> {
                parse_size(value).map(ByteSize).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(SizeVisitor)
    }
}

fn parse_size(input: &str) -> std::result::Result<u64, String> {
    let text = input.trim();

    if text.is_empty() {
        return Err("size cannot be empty".to_string());
    }

    let split = text
        .find(|character: char| {
            !(character.is_ascii_digit() || character == '.')
        })
        .unwrap_or(text.len());

    let (number, suffix) = text.split_at(split);

    if number.is_empty() {
        return Err(format!("invalid size '{input}'"));
    }

    let value: f64 = number
        .parse()
        .map_err(|_| format!("invalid size '{input}'"))?;

    if !value.is_finite() || value < 0.0 {
        return Err(format!("invalid size '{input}'"));
    }

    let multiplier = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" => 1_000.0,
        "kib" => 1_024.0,
        "m" | "mb" => 1_000_000.0,
        "mib" => 1_048_576.0,
        "g" | "gb" => 1_000_000_000.0,
        "gib" => 1_073_741_824.0,
        "t" | "tb" => 1_000_000_000_000.0,
        "tib" => 1_099_511_627_776.0,
        "p" | "pb" => 1_000_000_000_000_000.0,
        "pib" => 1_125_899_906_842_624.0,
        "e" | "eb" => 1_000_000_000_000_000_000.0,
        "eib" => 1_152_921_504_606_846_976.0,
        other => {
            return Err(format!("unknown size suffix '{other}'"));
        }
    };

    let bytes = value * multiplier;

    if !bytes.is_finite() || bytes > u64::MAX as f64 {
        return Err(format!("size '{input}' is too large"));
    }

    Ok(bytes.round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
id = "sample"
name = "Sample"
description = "test cleaner"

[[option]]
id = "cache"
label = "Cache"
description = "cached files"

[[option.action]]
command = "delete"
search = "walk.all"
path = "/tmp/cache"
max_age_days = 7
min_size = "1MB"

[[option.action]]
command = "delete"
path = "/tmp/files/*.log"
type = "f"
"#;

    #[test]
    fn parses_native_schema() {
        let cleaner = parse_native(SAMPLE, "sample.toml").unwrap();

        assert_eq!(cleaner.id, "sample");
        assert_eq!(cleaner.options.len(), 1);
        assert_eq!(cleaner.options[0].actions.len(), 2);
        assert_eq!(
            cleaner.options[0].actions[0].search,
            SearchKind::WalkAll
        );
        assert_eq!(
            cleaner.options[0].actions[0].max_age_days,
            Some(7)
        );
        assert_eq!(
            cleaner.options[0].actions[0].min_size,
            Some(1_000_000)
        );
        assert_eq!(
            cleaner.options[0].actions[1].search,
            SearchKind::Glob
        );
        assert_eq!(
            cleaner.options[0].actions[1].object_type,
            Some(ObjectType::File)
        );
    }

    #[test]
    fn rejects_duplicate_options() {
        let text = r#"
id = "dup"
[[option]]
id = "same"
label = "One"
[[option.action]]
path = "/tmp"
[[option]]
id = "same"
label = "Two"
[[option.action]]
path = "/tmp"
"#;

        let error = parse_native(text, "dup.toml").unwrap_err();

        assert!(error.to_string().contains("duplicate option"));
    }

    #[test]
    fn rejects_inverted_size_filter() {
        let text = r#"
id = "bad"
[[option]]
id = "cache"
label = "Cache"
[[option.action]]
path = "/tmp"
min_size = "2MB"
max_size = "1MB"
"#;

        let error = parse_native(text, "bad.toml").unwrap_err();

        assert!(error.to_string().contains("exceeds max_size"));
    }

    #[test]
    fn parses_human_sizes() {
        assert_eq!(parse_size("500MB").unwrap(), 500_000_000);
        assert_eq!(parse_size("1 MiB").unwrap(), 1_048_576);
        assert_eq!(parse_size("2").unwrap(), 2);
        assert!(parse_size("1XB").is_err());
    }
}
