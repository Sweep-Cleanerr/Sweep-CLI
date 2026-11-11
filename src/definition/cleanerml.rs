//! Importer for BleachBit's CleanerML (XML) cleaner definitions.
//!
//! Security: a `<!DOCTYPE>` with an internal subset is rejected
//! (XXE / billion-laughs); privileged actions are dropped for untrusted
//! sources via `provider::is_privileged`.
//!
//! Mixed content (`<label>foo <b>bar</b></label>`) is not supported:
//! only the text between the tag's own children is retained. This is
//! enough for CleanerML (labels are plain text).

use std::collections::HashMap;

use quick_xml::events::attributes::Attributes;
use quick_xml::events::Event;
use quick_xml::Reader;

use crate::action::provider;
use crate::core::error::{Error, Result};
use crate::definition::model::{
    ActionDef, CleanerDef, ObjectType, OptionDef, OsFilter, RunningCheck, SearchKind,
    Trust, VarDef, VarSearch, VarValue,
};

fn reject_internal_dtd(text: &str, file: &str) -> Result<()> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().check_end_names = false;
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::DocType(doc)) => {
                let raw = String::from_utf8_lossy(doc.as_ref()).to_string();
                if raw.contains('[') {
                    return Err(Error::definition(
                        file,
                        "DTD with an internal subset is not allowed",
                    ));
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(Error::definition(file, err.to_string())),
            _ => {}
        }
        buffer.clear();
    }
    Ok(())
}

fn attrs_to_map(attrs: Attributes<'_>) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    for attribute in attrs {
        let attribute = attribute.map_err(|err| Error::msg(err.to_string()))?;
        let key = String::from_utf8_lossy(attribute.key.as_ref()).to_string();
        if map.contains_key(&key) {
            return Err(Error::msg(format!("duplicate attribute '{key}'")));
        }
        let value = attribute
            .unescape_value()
            .map_err(|err| Error::msg(err.to_string()))?
            .into_owned();
        map.insert(key, value);
    }
    Ok(map)
}

fn attr(map: &HashMap<String, String>, key: &str) -> String {
    map.get(key).cloned().unwrap_or_default()
}

fn optional_attr(map: &HashMap<String, String>, key: &str) -> Option<String> {
    map.get(key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

struct Parser<'a> {
    file: &'a str,
    cleaner: CleanerDef,
    trust: Trust,
    ignored: Vec<String>,
    current_option: Option<OptionDef>,
    text: String,
}

impl<'a> Parser<'a> {
    fn new(file: &'a str, trust: Trust) -> Self {
        Self {
            file,
            cleaner: CleanerDef::new(String::new(), String::new()),
            trust,
            ignored: Vec::new(),
            current_option: None,
            text: String::new(),
        }
    }

    fn err(&self, reason: impl Into<String>) -> Error {
        Error::definition(self.file, reason)
    }

    fn take_text(&mut self) -> String {
        std::mem::take(&mut self.text).trim().to_string()
    }

    fn parse(&mut self, text: &str) -> Result<()> {
        let mut reader = Reader::from_str(text);
        reader.config_mut().trim_text(false);
        let mut buffer = Vec::new();
        let mut path: Vec<String> = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(element)) => {
                    let name = String::from_utf8_lossy(element.name().as_ref()).to_string();
                    let attrs = attrs_to_map(element.attributes())?;
                    self.text.clear();
                    path.push(name.clone());
                    self.start(&name, &attrs, &path)?;
                }
                Ok(Event::Empty(element)) => {
                    let name = String::from_utf8_lossy(element.name().as_ref()).to_string();
                    let attrs = attrs_to_map(element.attributes())?;
                    self.text.clear();
                    path.push(name.clone());
                    self.start(&name, &attrs, &path)?;
                    self.end(&name, &path)?;
                    path.pop();
                }
                Ok(Event::End(element)) => {
                    let name = String::from_utf8_lossy(element.name().as_ref()).to_string();
                    self.end(&name, &path)?;
                    path.pop();
                    self.text.clear();
                }
                Ok(Event::Text(node)) => {
                    let decoded = node
                        .unescape()
                        .map_err(|e| self.err(e.to_string()))?
                        .into_owned();
                    self.text.push_str(&decoded);
                }
                Ok(Event::CData(node)) => {
                    self.text.push_str(&String::from_utf8_lossy(node.as_ref()));
                }
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(err) => return Err(self.err(err.to_string())),
            }
            buffer.clear();
        }
        Ok(())
    }

    fn start(&mut self, name: &str, attrs: &HashMap<String, String>, path: &[String]) -> Result<()> {
        let parent = path
            .get(path.len().wrapping_sub(2))
            .map(String::as_str)
            .unwrap_or("");
        match name {
            "cleaner" => {
                self.cleaner.id = attr(attrs, "id");
                self.cleaner.os = OsFilter::new(attr(attrs, "os"));
            }
            "option" if parent == "cleaner" => {
                if self.current_option.is_some() {
                    return Err(self.err("nested <option> is not allowed"));
                }
                self.current_option = Some(OptionDef {
                    id: attr(attrs, "id"),
                    label: String::new(),
                    description: String::new(),
                    warning: String::new(),
                    actions: Vec::new(),
                    os: OsFilter::new(attr(attrs, "os")),
                });
            }
            "option" => {
                return Err(self.err("<option> must be a direct child of <cleaner>"));
            }
            "action" => self.handle_action(attrs)?,
            _ => {}
        }
        Ok(())
    }

    fn handle_action(&mut self, attrs: &HashMap<String, String>) -> Result<()> {
        let command = attr(attrs, "command");
        let os = OsFilter::new(attr(attrs, "os"));
        if !os.matches() {
            return Ok(());
        }
        if self.trust != Trust::Trusted && provider::is_privileged(&command) {
            self.ignored.push(command);
            return Ok(());
        }
        let search = SearchKind::parse(attr(attrs, "search").as_str()).unwrap_or(SearchKind::File);
        let object_type = match attr(attrs, "type").as_str() {
            "f" | "file" => Some(ObjectType::File),
            "d" | "dir" | "directory" => Some(ObjectType::Dir),
            _ => None,
        };
        let wait_attr = attr(attrs, "wait").to_lowercase();
        let wait = !matches!(wait_attr.as_str(), "false" | "no" | "0" | "f" | "n");
        let action = ActionDef {
            command,
            search,
            path: attr(attrs, "path"),
            os: OsFilter::any(),
            regex: optional_attr(attrs, "regex"),
            nregex: optional_attr(attrs, "nregex"),
            wholeregex: optional_attr(attrs, "wholeregex"),
            nwholeregex: optional_attr(attrs, "nwholeregex"),
            object_type,
            section: optional_attr(attrs, "section"),
            parameter: optional_attr(attrs, "parameter"),
            address: optional_attr(attrs, "address"),
            cmd: optional_attr(attrs, "cmd"),
            wait,
            reg_name: optional_attr(attrs, "name"),
            ..ActionDef::default()
        };
        match self.current_option.as_mut() {
            Some(option) => {
                option.actions.push(action);
                Ok(())
            }
            None => Err(self.err("<action> outside of <option>")),
        }
    }

    fn end(&mut self, name: &str, path: &[String]) -> Result<()> {
        let parent = path
            .get(path.len().wrapping_sub(2))
            .map(String::as_str)
            .unwrap_or("");
        match (parent, name) {
            ("cleaner", "label") => self.cleaner.name = self.take_text(),
            ("cleaner", "description") => self.cleaner.description = self.take_text(),
            ("option", "label") => {
                let t = self.take_text();
                if let Some(o) = self.current_option.as_mut() {
                    o.label = t;
                }
            }
            ("option", "description") => {
                let t = self.take_text();
                if let Some(o) = self.current_option.as_mut() {
                    o.description = t;
                }
            }
            ("option", "warning") => {
                let t = self.take_text();
                if let Some(o) = self.current_option.as_mut() {
                    o.warning = t;
                }
            }
            ("cleaner", "option") => {
                if let Some(o) = self.current_option.take() {
                    if !o.label.is_empty() || !o.actions.is_empty() {
                        self.cleaner.options.push(o);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn parse_cleanerml(text: &str, file: &str, trust: Trust) -> Result<CleanerDef> {
    reject_internal_dtd(text, file)?;
    let mut parser = Parser::new(file, trust);
    parser.parse(text)?;
    parser.cleaner.vars = parse_vars(text, file)?;
    parser.cleaner.running = parse_running(text, file)?;
    if parser.cleaner.id.is_empty() {
        return Err(Error::definition(file, "missing <cleaner id=\"...\">"));
    }
    if !parser.ignored.is_empty() {
        parser.ignored.sort();
        parser.ignored.dedup();
        log::warn!(
            "ignoring {} privileged action(s) from untrusted cleaner {}: {}",
            parser.ignored.len(),
            file,
            parser.ignored.join(", ")
        );
    }
    Ok(parser.cleaner)
}

fn parse_var_search(s: &str) -> VarSearch {
    match s {
        "glob" => VarSearch::Glob,
        "winreg" | "registry" => VarSearch::Registry,
        _ => VarSearch::Literal,
    }
}

fn parse_vars(text: &str, file: &str) -> Result<Vec<VarDef>> {
    let mut vars: Vec<VarDef> = Vec::new();
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut in_var = false;
    let mut current_name = String::new();
    let mut pending: Option<(VarSearch, Option<String>, String, OsFilter)> = None;

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) => {
                let name = String::from_utf8_lossy(element.name().as_ref()).to_string();
                let attrs = attrs_to_map(element.attributes())?;
                match name.as_str() {
                    "var" => {
                        if in_var {
                            return Err(Error::definition(file, "nested <var> is not allowed"));
                        }
                        in_var = true;
                        current_name = attr(&attrs, "name");
                        if current_name.is_empty() {
                            return Err(Error::definition(file, "<var> without a name"));
                        }
                        if vars.iter().any(|v| v.name == current_name) {
                            return Err(Error::definition(file, "duplicate <var>"));
                        }
                    }
                    "value" if in_var => {
                        let search = parse_var_search(attr(&attrs, "search").as_str());
                        let reg_name = optional_attr(&attrs, "name");
                        let os = OsFilter::new(attr(&attrs, "os"));
                        pending = Some((search, reg_name, String::new(), os));
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(node)) => {
                if let Some(p) = pending.as_mut() {
                    if let Ok(s) = node.unescape() {
                        p.2.push_str(&s);
                    }
                }
            }
            Ok(Event::CData(node)) => {
                if let Some(p) = pending.as_mut() {
                    p.2.push_str(&String::from_utf8_lossy(node.as_ref()));
                }
            }
            Ok(Event::End(element)) => {
                let name = String::from_utf8_lossy(element.name().as_ref()).to_string();
                match name.as_str() {
                    "value" if in_var => {
                        if let Some((search, reg_name, raw, os)) = pending.take() {
                            let value = VarValue {
                                raw: raw.trim().to_string(),
                                search,
                                os,
                                reg_name,
                            };
                            if let Some(var) = vars.iter_mut().find(|v| v.name == current_name) {
                                var.values.push(value);
                            } else {
                                vars.push(VarDef {
                                    name: current_name.clone(),
                                    values: vec![value],
                                });
                            }
                        }
                    }
                    "var" => {
                        in_var = false;
                        current_name.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(Error::definition(file, err.to_string())),
            _ => {}
        }
        buffer.clear();
    }
    Ok(vars)
}

fn parse_running(text: &str, file: &str) -> Result<Vec<RunningCheck>> {
    let mut checks = Vec::new();
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut kind: Option<(String, OsFilter, String)> = None; // (type, os, value)

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                let name = String::from_utf8_lossy(element.name().as_ref()).to_string();
                let attrs = attrs_to_map(element.attributes())?;
                for tag in ["exename", "windowtitle"] {
                    if name == tag {
                        let value = attr(&attrs, "value").trim().to_string();
                        if !value.is_empty() {
                            checks.push(RunningCheck::Exe {
                                name: value,
                                same_user: true,
                                os: OsFilter::new(attr(&attrs, "os")),
                            });
                        }
                    }
                }
                if name == "path" {
                    let value = attr(&attrs, "value").trim().to_string();
                    if !value.is_empty() {
                        checks.push(RunningCheck::Path {
                            pattern: value,
                            os: OsFilter::new(attr(&attrs, "os")),
                        });
                    }
                }
                if name == "running" {
                    let t = attr(&attrs, "type").to_lowercase();
                    let os = OsFilter::new(attr(&attrs, "os"));
                    kind = Some((t, os, String::new()));
                }
            }
            Ok(Event::Text(node)) => {
                if let Some((_, _, ref mut value)) = kind {
                    value.push_str(&node.unescape().map_err(|e| Error::definition(file, e.to_string()))?.trim());
                }
            }
            Ok(Event::End(element)) => {
                let name = String::from_utf8_lossy(element.name().as_ref()).to_string();
                if name == "running" {
                    if let Some((t, os, value)) = kind.take() {
                        let value = value.trim().to_string();
                        if !value.is_empty() {
                            match t.as_str() {
                                "exe" | "exename" => checks.push(RunningCheck::Exe {
                                    name: value,
                                    same_user: true,
                                    os,
                                }),
                                "path" => checks.push(RunningCheck::Path { pattern: value, os }),
                                _ => {}
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(Error::definition(file, err.to_string())),
            _ => {}
        }
        buffer.clear();
    }
    Ok(checks)
}

pub fn load_cleanerml_dir_entry(
    path: &std::path::Path,
    trust: Trust,
) -> Vec<Result<CleanerDef>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(err) => return vec![Err(crate::core::error::Error::io(path.to_path_buf(), err))],
    };
    match parse_cleanerml(&text, &path.to_string_lossy(), trust) {
        Ok(def) => vec![Ok(def)],
        Err(err) => vec![Err(err)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_cleaner() {
        let xml = r#"<cleaner id="demo" os=""><label>Demo</label><description>Test cleaner</description><option id="cache"><label>Cache</label><description>D</description><action command="delete" search="walk.all" path="/tmp/x"/></option></cleaner>"#;
        let def = parse_cleanerml(xml, "demo.xml", Trust::Trusted).unwrap();
        assert_eq!(def.id, "demo");
        assert_eq!(def.options.len(), 1);
        assert_eq!(def.options[0].actions[0].command, "delete");
    }

    #[test]
    fn rejects_internal_dtd() {
        let xml = r#"<!DOCTYPE cleaner [<!ENTITY x "y">]><cleaner id="d"><label>D</label></cleaner>"#;
        assert!(parse_cleanerml(xml, "d.xml", Trust::Trusted).is_err());
    }

    #[test]
    fn untrusted_drops_privileged_actions() {
        let xml = r#"<cleaner id="d"><option id="o"><label>O</label><action command="process" cmd="rm -rf /" path=""/></option></cleaner>"#;
        let def = parse_cleanerml(xml, "d.xml", Trust::Untrusted).unwrap();
        assert!(def.options[0].actions.is_empty());
    }
}
