use super::*;
use crate::config::{quick::QuickSetting, settings, test_support::TestDirectory, RunLevel};

#[test]
fn every_quick_command_persists_its_value_and_rebuilds_the_native_checkmark() {
    let tray = TrayIcon::create().unwrap();
    let text = Text::new(Language::Chinese);
    let mut checked_commands = 0;
    for command in 100..=132 {
        let Some(setting) = quick_settings::setting(command) else {
            continue;
        };
        checked_commands += 1;
        let directory = TestDirectory::new();
        let source = "[search]\nfields=app,title,exe\n[custom]\nuntouched=keep\n";
        std::fs::write(directory.ini(), source).unwrap();
        let original = settings::read_current(&directory.ini()).unwrap();
        if setting == QuickSetting::StartupLevel(RunLevel::Highest)
            && !crate::utils::is_running_as_admin().unwrap()
        {
            assert!(settings::save_quick(&directory.ini(), &original.config, setting).is_err());
            assert_eq!(std::fs::read_to_string(directory.ini()).unwrap(), source);
            continue;
        }
        let expected = setting.changed_value(&original.config).unwrap();
        let saved = settings::save_quick(&directory.ini(), &original.config, setting).unwrap();
        let reread = settings::read_current(&directory.ini()).unwrap();
        assert_eq!(saved.config, reread.config, "command {command}");
        assert_eq!(setting.value(&reread.config), expected, "command {command}");
        let checked = match setting {
            QuickSetting::Names
            | QuickSetting::Badges
            | QuickSetting::Preview
            | QuickSetting::Minimized
            | QuickSetting::Topmost
            | QuickSetting::HiddenMinimized
            | QuickSetting::SearchEnabled
            | QuickSetting::Field(_) => !setting.checked(&original.config),
            _ => true,
        };
        let menu = tray.create_menu(menu_state(&reread.config), text).unwrap();
        assert_eq!(
            flags(menu.0, command) & MF_CHECKED.0 != 0,
            checked,
            "command {command}"
        );
        assert!(std::fs::read_to_string(directory.ini())
            .unwrap()
            .contains("untouched=keep"));
    }
    assert_eq!(checked_commands, 24);
}
