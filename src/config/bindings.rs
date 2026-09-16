//! Cross-feature binding and recovery invariants.
use super::Config;
use anyhow::{ensure, Result};

pub(super) fn validate(config: &Config) -> Result<()> {
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
    ensure!(
        !config.input_paused || config.trayicon || config.pause_hotkey.is_some(),
        "暂停时必须保留托盘或 input.pause_hotkey 恢复入口；原值未修改"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Hotkey, PAUSE_HOTKEY_ID, SEARCH_HOTKEY_ID};

    #[test]
    fn only_enabled_bindings_conflict_and_search_is_independent() {
        let mut config = Config {
            search_hotkey: Hotkey::create(SEARCH_HOTKEY_ID, "search", "alt+tab").unwrap(),
            switch_apps_enable: true,
            ..Default::default()
        };
        assert!(validate(&config).is_ok());
        config.search_enable = true;
        assert!(validate(&config).is_err());
        config.switch_apps_enable = false;
        assert!(validate(&config).is_ok());
        assert!(config
            .to_hotkeys()
            .iter()
            .any(|key| key.id == SEARCH_HOTKEY_ID));
    }

    #[test]
    fn paused_configuration_keeps_an_independent_nonconflicting_recovery_path() {
        let mut config = Config {
            input_paused: true,
            trayicon: false,
            ..Default::default()
        };
        assert!(validate(&config).is_err());
        config.pause_hotkey = Some(Hotkey::create(PAUSE_HOTKEY_ID, "pause", "ctrl+f10").unwrap());
        assert!(validate(&config).is_ok());
        assert!(config
            .to_hotkeys()
            .iter()
            .any(|key| key.id == PAUSE_HOTKEY_ID));
        config.search_enable = true;
        config.search_hotkey = Hotkey::create(SEARCH_HOTKEY_ID, "search", "ctrl+f10").unwrap();
        assert!(validate(&config).is_err());
        config.pause_hotkey = None;
        config.trayicon = true;
        assert!(validate(&config).is_ok());
    }

    #[test]
    fn details_only_registers_with_its_panel_and_conflicts_with_enabled_entries() {
        let mut config = Config {
            details_enable: true,
            details_hotkey: Hotkey::create(
                super::super::DETAILS_HOTKEY_ID,
                "details",
                "ctrl+space",
            )
            .unwrap(),
            search_enable: true,
            ..Default::default()
        };
        assert!(validate(&config).is_ok());
        config.switch_apps_enable = true;
        assert!(validate(&config).is_err());
        config.search_enable = false;
        assert!(validate(&config).is_ok());
    }
}
