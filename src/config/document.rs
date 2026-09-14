use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{bail, Context, Result};
use indexmap::IndexMap;
use ini::{Ini, ParseOption};

use super::DEFAULT_CONFIG;

type EntryKey = (String, String);

struct TemplateEntry {
    identity: EntryKey,
    marker: String,
    comments: String,
    assignment: String,
}

#[derive(Default)]
struct DocumentLayout {
    keys: HashMap<EntryKey, usize>,
    section_ends: HashMap<String, usize>,
    comments: HashSet<String>,
}

pub(super) fn parse_ini(text: &str) -> Result<Ini> {
    Ini::load_from_str_opt(
        text,
        ParseOption {
            enabled_escape: false,
            ..Default::default()
        },
    )
    .context("INI 语法无效；请检查节名、等号与引号，原文件未修改")
}

/// Insert into the original text; never serialize existing user settings through Ini.
pub(super) fn merge_missing(text: &str) -> Result<String> {
    let original = parse_ini(text)?;
    let original_values = values(&original);
    let layout = scan_layout(text);
    let mut insertions: BTreeMap<usize, String> = BTreeMap::new();
    let mut new_sections: IndexMap<String, String> = IndexMap::new();
    let template = template_entries();

    for entry in &template {
        let exists = original_values.contains_key(&entry.identity);
        let documented = layout.comments.contains(&entry.marker);
        if exists && documented {
            continue;
        }
        let section = &entry.identity.0;
        let offset = layout
            .keys
            .get(&entry.identity)
            .or_else(|| layout.section_ends.get(section))
            .copied()
            .unwrap_or(text.len());
        let addition = if layout.section_ends.contains_key(section) {
            insertions.entry(offset).or_default()
        } else {
            new_sections
                .entry(section.clone())
                .or_insert_with(|| format!("\n[{section}]\n\n"))
        };
        if !documented {
            addition.push_str(&entry.comments);
            addition.push('\n');
        }
        if !exists {
            addition.push_str(&entry.assignment);
            addition.push_str("\n\n");
        }
    }
    for addition in new_sections.into_values() {
        insertions
            .entry(text.len())
            .or_default()
            .push_str(&addition);
    }

    if insertions.is_empty() {
        return Ok(text.to_owned());
    }
    let newline = if text
        .find('\n')
        .is_some_and(|at| at > 0 && text.as_bytes()[at - 1] == b'\r')
    {
        "\r\n"
    } else {
        "\n"
    };
    let mut merged = String::with_capacity(text.len() + DEFAULT_CONFIG.len());
    let mut previous = 0;
    for (offset, addition) in insertions {
        merged.push_str(&text[previous..offset]);
        if !merged.is_empty() && !merged.ends_with('\n') {
            merged.push_str(newline);
        }
        merged.push_str(&addition.replace('\n', newline));
        previous = offset;
    }
    merged.push_str(&text[previous..]);
    if !text.is_empty() && !text.ends_with('\n') {
        while merged.ends_with(newline) {
            merged.truncate(merged.len() - newline.len());
        }
    }

    // Also protect quoted/multiline and duplicate values from accidental semantic changes.
    let merged_values = values(&parse_ini(&merged)?);
    for entry in template {
        if !merged_values.contains_key(&entry.identity) {
            bail!(
                "INI 补全无法定位 [{}] {}；原文件未修改",
                entry.identity.0,
                entry.identity.1
            );
        }
    }
    for (key, before) in original_values {
        if merged_values.get(&key) != Some(&before) {
            bail!(
                "INI 补全会改变既有项 [{}] {}；已中止，原文件未修改",
                key.0,
                key.1
            );
        }
    }
    Ok(merged)
}

fn values(ini: &Ini) -> HashMap<EntryKey, Vec<String>> {
    let mut result: HashMap<EntryKey, Vec<String>> = HashMap::new();
    for (section, properties) in ini {
        for (key, value) in properties {
            result
                .entry((section.unwrap_or_default().to_owned(), key.to_owned()))
                .or_default()
                .push(value.to_owned());
        }
    }
    result
}

fn template_entries() -> Vec<TemplateEntry> {
    let mut entries = Vec::new();
    let mut section = String::new();
    let mut comments = Vec::new();
    for line in DEFAULT_CONFIG.lines() {
        let line = line.trim();
        if let Some(name) = section_name(line) {
            section = name.to_owned();
            comments.clear();
        } else if line.starts_with(';') || line.starts_with('#') {
            comments.push(line);
        } else if let Some((key, _)) = assignment(line) {
            let name = if section.is_empty() {
                key.to_owned()
            } else {
                format!("{section}.{key}")
            };
            entries.push(TemplateEntry {
                identity: (section.clone(), key.to_owned()),
                marker: format!("; 配置说明：{name}"),
                comments: comments.join("\n"),
                assignment: line.to_owned(),
            });
            comments.clear();
        }
    }
    entries
}

fn scan_layout(text: &str) -> DocumentLayout {
    let mut layout = DocumentLayout::default();
    let mut section = String::new();
    let mut offset = 0;
    let mut quote = None;
    let mut continuation = false;
    for raw in text.split_inclusive('\n') {
        let line = raw.trim();
        if quote.is_some() {
            quote = unfinished_quote(line, quote);
        } else if continuation {
            continuation = raw.trim_end_matches(['\r', '\n']).ends_with('\\');
        } else if line.starts_with(';') || line.starts_with('#') {
            layout.comments.insert(line.to_owned());
        } else if let Some(name) = section_name(line) {
            layout.section_ends.entry(section).or_insert(offset);
            section = name.to_owned();
        } else if let Some((key, value)) = assignment(line) {
            layout
                .keys
                .entry((section.clone(), key.to_owned()))
                .or_insert(offset);
            quote = unfinished_quote(value, None);
            continuation = quote.is_none() && raw.trim_end_matches(['\r', '\n']).ends_with('\\');
        }
        offset += raw.len();
    }
    layout.section_ends.entry(section).or_insert(text.len());
    layout
}

fn section_name(line: &str) -> Option<&str> {
    line.strip_prefix('[')?
        .split_once(']')
        .map(|(name, _)| name.trim())
}

fn assignment(line: &str) -> Option<(&str, &str)> {
    let at = line.find(['=', ':'])?;
    Some((line[..at].trim(), line[at + 1..].trim_start()))
}

fn unfinished_quote(mut value: &str, mut quote: Option<char>) -> Option<char> {
    loop {
        let delimiter = match quote {
            Some(delimiter) => delimiter,
            None => match value.chars().next() {
                Some(delimiter @ ('\'' | '"')) => {
                    value = &value[1..];
                    delimiter
                }
                _ => return None,
            },
        };
        match value.find(delimiter) {
            Some(at) => {
                value = &value[at + 1..];
                quote = None;
            }
            None => return Some(delimiter),
        }
    }
}
