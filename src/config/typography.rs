use super::Config;
use std::path::Path;

impl Config {
    pub(crate) fn name_font_size(&self) -> u32 {
        if self.unified_font {
            self.ui_font_size
        } else {
            self.app_name_font_size
        }
    }

    pub(crate) fn badge_font_size(&self) -> u32 {
        if self.unified_font {
            self.ui_font_size.min(24)
        } else {
            self.switch_apps_badge_font_size
        }
    }

    pub(crate) fn name_font_family(&self) -> &str {
        if self.unified_font {
            &self.ui_font_family
        } else {
            &self.app_name_font_family
        }
    }

    pub(crate) fn badge_font_family(&self) -> &str {
        if self.unified_font {
            &self.ui_font_family
        } else {
            &self.badge_font_family
        }
    }

    pub(crate) fn name_font_file(&self) -> Option<&Path> {
        if self.unified_font {
            None
        } else {
            self.app_name_font_file.as_deref()
        }
    }

    pub(crate) fn badge_font_file(&self) -> Option<&Path> {
        if self.unified_font {
            None
        } else {
            self.badge_font_file.as_deref()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_font_overrides_effective_content_without_rewriting_role_values() {
        let mut config = Config {
            unified_font: true,
            ui_font_family: "Arial".into(),
            ui_font_size: 20,
            badge_font_family: "Maple UI".into(),
            app_name_font_size: 16,
            ..Default::default()
        };
        assert_eq!(config.name_font_family(), "Arial");
        assert_eq!(config.badge_font_family(), "Arial");
        assert_eq!(config.name_font_size(), 20);
        assert_eq!(config.badge_font_size(), 20);
        config.unified_font = false;
        assert_eq!(config.name_font_size(), 16);
        assert_eq!(config.badge_font_family(), "Maple UI");
        assert_eq!(config.badge_font_size(), 12);
    }
}
