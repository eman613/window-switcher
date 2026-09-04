use std::{collections::HashMap, fmt};

use ini::Ini;

use crate::{
    config::CURRENT_CONFIG_VERSION,
    config_schema::{is_known_section, validate_setting, SettingStatus},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigDiagnosticSeverity {
    Info,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDiagnostic {
    pub severity: ConfigDiagnosticSeverity,
    pub line: Option<usize>,
    pub message: String,
}

impl ConfigDiagnostic {
    pub fn is_warning(&self) -> bool {
        self.severity == ConfigDiagnosticSeverity::Warning
    }
}

impl fmt::Display for ConfigDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(formatter, "line {line}: {}", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

pub(crate) fn prepare_config(source: &str, ini: &mut Ini) -> Vec<ConfigDiagnostic> {
    let locations = DocumentLocations::scan(source);
    let mut diagnostics = analyze_config(ini, &locations);
    migrate_config(ini, &locations, &mut diagnostics);
    diagnostics
}

#[derive(Default)]
struct DocumentLocations {
    section_lines: Vec<(String, usize)>,
    setting_lines: HashMap<(Option<String>, String), Vec<usize>>,
}

impl DocumentLocations {
    fn scan(source: &str) -> Self {
        let mut locations = Self::default();
        let mut section = None;

        for (line_index, raw_line) in source.lines().enumerate() {
            let line_number = line_index + 1;
            let line = raw_line.trim().trim_start_matches('\u{feff}').trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            if let Some(name) = parse_section_name(line) {
                section = Some(name.to_string());
                locations
                    .section_lines
                    .push((name.to_string(), line_number));
                continue;
            }

            if let Some(key) = parse_key(line) {
                locations
                    .setting_lines
                    .entry((section.clone(), key.to_string()))
                    .or_default()
                    .push(line_number);
            }
        }

        locations
    }

    fn setting_line(&self, section: Option<&str>, key: &str, occurrence: usize) -> Option<usize> {
        self.setting_lines
            .get(&(section.map(str::to_string), key.to_string()))
            .and_then(|lines| {
                lines
                    .get(occurrence)
                    .copied()
                    .or_else(|| lines.last().copied())
            })
    }
}

fn parse_section_name(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?.trim();
    (!inner.is_empty()).then_some(inner)
}

fn parse_key(line: &str) -> Option<&str> {
    let equals = line.find('=');
    let colon = line.find(':');
    let separator = match (equals, colon) {
        (Some(left), Some(right)) => left.min(right),
        (Some(index), None) | (None, Some(index)) => index,
        (None, None) => return None,
    };
    let key = line[..separator].trim();
    (!key.is_empty()).then_some(key)
}

fn analyze_config(ini: &Ini, locations: &DocumentLocations) -> Vec<ConfigDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut section_counts = HashMap::<&str, usize>::new();

    for (section, line) in &locations.section_lines {
        if !is_known_section(Some(section)) {
            diagnostics.push(warning(
                Some(*line),
                format!("未知配置节 / unknown section [{section}]"),
            ));
            continue;
        }
        let count = section_counts.entry(section).or_default();
        *count += 1;
        if *count > 1 {
            diagnostics.push(warning(
                Some(*line),
                format!("重复配置节 / duplicate section [{section}]"),
            ));
        }
    }

    let mut occurrences = HashMap::<(Option<String>, String), usize>::new();
    for (section, properties) in ini.iter() {
        if !is_known_section(section) {
            continue;
        }
        for (key, value) in properties.iter() {
            let location_key = (section.map(str::to_string), key.to_string());
            let occurrence = occurrences.entry(location_key).or_default();
            let line = locations.setting_line(section, key, *occurrence);
            if *occurrence > 0 {
                diagnostics.push(warning(
                    line,
                    format!(
                        "重复配置项 / duplicate setting {}",
                        setting_name(section, key)
                    ),
                ));
            }
            *occurrence += 1;

            match validate_setting(section, key, value) {
                SettingStatus::Valid => {}
                SettingStatus::Invalid(expected) => diagnostics.push(warning(
                    line,
                    format!(
                        "无效值 / invalid value {}={value:?}; expected {expected}",
                        setting_name(section, key)
                    ),
                )),
                SettingStatus::DeprecatedKey(replacement) => diagnostics.push(warning(
                    line,
                    format!(
                        "过时配置项 / deprecated setting {}; use {replacement}",
                        setting_name(section, key)
                    ),
                )),
                SettingStatus::DeprecatedValue(replacement) => diagnostics.push(warning(
                    line,
                    format!(
                        "过时配置值 / deprecated value {}={value:?}; use {replacement}",
                        setting_name(section, key)
                    ),
                )),
                SettingStatus::Unknown => diagnostics.push(warning(
                    line,
                    format!(
                        "未知配置项 / unknown setting {}",
                        setting_name(section, key)
                    ),
                )),
            }
        }
    }

    diagnostics
}

fn migrate_config(
    ini: &mut Ini,
    locations: &DocumentLocations,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) {
    let version = ini
        .section(None::<String>)
        .and_then(|section| section.get("config_version"))
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0);

    if version > CURRENT_CONFIG_VERSION {
        let line = locations.setting_line(None, "config_version", 0);
        diagnostics.push(warning(
            line,
            format!(
                "配置版本 {version} 高于当前支持版本 {CURRENT_CONFIG_VERSION} / newer config version"
            ),
        ));
        return;
    }

    if version == 0 {
        let legacy_log_path = ini
            .section(Some("log"))
            .and_then(|section| {
                section
                    .get("path")
                    .is_none()
                    .then(|| section.get("file"))
                    .flatten()
            })
            .map(str::to_string);
        if let Some(path) = legacy_log_path {
            ini.with_section(Some("log")).set("path", path);
        }

        ini.with_section(None::<String>)
            .set("config_version", CURRENT_CONFIG_VERSION.to_string());
        diagnostics.push(ConfigDiagnostic {
            severity: ConfigDiagnosticSeverity::Info,
            line: None,
            message: format!(
                "已在内存中迁移旧配置到版本 {CURRENT_CONFIG_VERSION} / legacy configuration migrated in memory"
            ),
        });
    }
}

fn setting_name(section: Option<&str>, key: &str) -> String {
    match section {
        Some(section) => format!("[{section}].{key}"),
        None => key.to_string(),
    }
}

fn warning(line: Option<usize>, message: String) -> ConfigDiagnostic {
    ConfigDiagnostic {
        severity: ConfigDiagnosticSeverity::Warning,
        line,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ini::ParseOption;

    fn parse(source: &str) -> Ini {
        Ini::load_from_str_opt(
            source,
            ParseOption {
                enabled_escape: false,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn diagnostics_include_lines_for_invalid_unknown_and_deprecated_settings() {
        let source = "trayicon = maybe\n[localization]\nlanguage = zh-CN\nunknown = 1\n[log]\nfile = old.log\n";
        let mut ini = parse(source);
        let diagnostics = prepare_config(source, &mut ini);

        assert!(diagnostics
            .iter()
            .any(|item| item.line == Some(1) && item.message.contains("trayicon")));
        assert!(diagnostics
            .iter()
            .any(|item| item.line == Some(4) && item.message.contains("unknown")));
        assert!(diagnostics
            .iter()
            .any(|item| item.line == Some(6) && item.message.contains("deprecated")));
        assert_eq!(
            ini.section(Some("log"))
                .and_then(|section| section.get("path")),
            Some("old.log")
        );
    }

    #[test]
    fn legacy_utf8_configuration_is_migrated_without_rewriting_keys() {
        let source = "trayicon = yes\n[localization]\nlanguage = zh-CN\n";
        let mut ini = parse(source);
        let diagnostics = prepare_config(source, &mut ini);

        assert_eq!(
            ini.section(None::<String>)
                .and_then(|section| section.get("config_version")),
            Some("1")
        );
        assert!(diagnostics
            .iter()
            .any(|item| item.severity == ConfigDiagnosticSeverity::Info));
    }

    #[test]
    fn duplicate_settings_report_the_duplicate_line_and_value() {
        let source = "trayicon = yes\ntrayicon = invalid\n";
        let mut ini = parse(source);
        let diagnostics = prepare_config(source, &mut ini);

        assert!(diagnostics.iter().any(|item| {
            item.line == Some(2)
                && item.message.contains("duplicate setting")
                && item.message.contains("trayicon")
        }));
        assert!(diagnostics.iter().any(|item| {
            item.line == Some(2)
                && item.message.contains("invalid value")
                && item.message.contains("trayicon")
        }));
    }
}
