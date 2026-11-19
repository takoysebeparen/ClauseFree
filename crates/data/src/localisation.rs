//! Localisation: `localisation/**/*_l_<language>.yml`.
//!
//! Line format: `key:0 "text"` (the number is optional). The first line is the language header `l_<language>:`.
//! Text runs from the first to the last double quote on the line, so inner quotes are kept as written.
//!
//! Precedence (hypothesis, see docs/CANON.md): files under a `replace` directory override everything; otherwise the
//! first definition of a key wins. The existence of the `replace` mechanism suggests normal files do not override,
//! but this is not yet measured.

use std::collections::BTreeMap;

use vfs::Vfs;

use crate::report::{IssueKind, Report};
use crate::source::{Source, evaluation_order};

#[derive(Debug, Clone, Default)]
pub struct Localisation {
    pub language: String,
    pub entries: BTreeMap<String, String>,
}

impl Localisation {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

const SCHEMA: &str = "localisation";

/// Loads one language (`english`, `russian`, ...).
pub fn load(vfs: &Vfs, language: &str, report: &mut Report) -> Localisation {
    let suffix = format!("_l_{language}.yml");
    let files: Vec<_> = vfs
        .list_recursive("localisation", "yml")
        .into_iter()
        .filter(|e| e.path.to_lowercase().ends_with(&suffix))
        .collect();
    let is_replace = |path: &str| path.to_lowercase().split('/').any(|seg| seg == "replace");
    let (replace, normal): (Vec<_>, Vec<_>) = files.into_iter().partition(|e| is_replace(&e.path));
    let mut loc = Localisation { language: language.to_string(), entries: BTreeMap::new() };
    // Replace files first, so that normal files cannot override them under "first wins".
    for file in evaluation_order(replace).into_iter().chain(evaluation_order(normal)) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, language, report, &mut loc);
    }
    loc
}

/// Parses one localisation file into `loc` (first definition of a key wins).
pub fn parse_file(src: &Source, language: &str, report: &mut Report, loc: &mut Localisation) {
    let header = format!("l_{language}:");
    let mut saw_header = false;
    for (index, raw) in src.text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !saw_header {
            if line.split('#').next().map(str::trim) == Some(header.as_str()) {
                saw_header = true;
                continue;
            }
            report.push(&src.path, line_no, format!("{SCHEMA}.header"), IssueKind::Missing("language header"));
            saw_header = true;
        }
        match parse_line(line) {
            Some((key, text)) => {
                if loc.entries.contains_key(key) {
                    report.push(&src.path, line_no, SCHEMA, IssueKind::Redefinition("first"));
                } else {
                    loc.entries.insert(key.to_string(), text.to_string());
                }
            }
            None => report.push(&src.path, line_no, SCHEMA, IssueKind::InvalidValue("key:0 \"text\"")),
        }
    }
}

/// `key:0 "text"` -> `(key, text)`.
pub fn parse_line(line: &str) -> Option<(&str, &str)> {
    let (key, rest) = line.split_once(':')?;
    let key = key.trim();
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    let start = rest.find('"')?;
    let end = rest.rfind('"')?;
    if end <= start {
        return None;
    }
    // Between the colon and the opening quote only an optional version number is allowed.
    if !rest[..start].trim().bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((key, &rest[start + 1..end]))
}
