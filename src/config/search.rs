use anyhow::{bail, ensure, Result};
use std::str::FromStr;

use super::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchField {
    App = 1,
    Title = 2,
    Exe = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchFields(u8);

impl SearchFields {
    pub(crate) fn contains(self, field: SearchField) -> bool {
        self.0 & field as u8 != 0
    }
}

impl FromStr for SearchFields {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        let mut fields = 0;
        for field in value.split(',').map(str::trim) {
            fields |= match field {
                "app" => SearchField::App as u8,
                "title" => SearchField::Title as u8,
                "exe" => SearchField::Exe as u8,
                _ => bail!("搜索字段须为 app/title/exe 的非空逗号分隔子集"),
            };
        }
        Ok(Self(fields))
    }
}

pub(super) fn validate_hotkeys(config: &Config) -> Result<()> {
    let hotkeys = config.to_hotkeys();
    for (index, first) in hotkeys.iter().enumerate() {
        for second in &hotkeys[index + 1..] {
            ensure!(
                first.id == second.id
                    || first.get_modifier() != second.get_modifier()
                    || first.code != second.code,
                "启用的热键冲突：{} / {}；请为两个功能设置不同组合键，原值未修改",
                first.name,
                second.name
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Hotkey, SEARCH_HOTKEY_ID};

    #[test]
    fn fields_are_a_nonempty_valid_subset() {
        for value in ["app", "title", "exe", "exe, app", "title,app,exe"] {
            assert!(value.parse::<SearchFields>().is_ok());
        }
        for value in ["", "all", "app,", "app,name", "APP"] {
            assert!(value.parse::<SearchFields>().is_err());
        }
        let fields = "title,exe".parse::<SearchFields>().unwrap();
        assert!(!fields.contains(SearchField::App));
        assert!(fields.contains(SearchField::Title) && fields.contains(SearchField::Exe));
    }

    #[test]
    fn only_enabled_bindings_conflict_and_search_is_independent() {
        let mut config = Config {
            search_hotkey: Hotkey::create(SEARCH_HOTKEY_ID, "search", "alt+tab").unwrap(),
            switch_apps_enable: true,
            ..Default::default()
        };
        assert!(validate_hotkeys(&config).is_ok());
        config.search_enable = true;
        assert!(validate_hotkeys(&config).is_err());
        config.switch_apps_enable = false;
        assert!(validate_hotkeys(&config).is_ok());
        assert!(config
            .to_hotkeys()
            .iter()
            .any(|key| key.id == SEARCH_HOTKEY_ID));
    }
}
