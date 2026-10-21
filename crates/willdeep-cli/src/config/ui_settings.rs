use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiMode {
    #[default]
    Full,
    Compact,
}

/// Terminal presentation only; never changes task or approval settings.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiSettings {
    pub mode: UiMode,
    pub compact_status_line: bool,
    pub check_updates: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            mode: UiMode::Full,
            compact_status_line: true,
            check_updates: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigFile;

    #[test]
    fn missing_ui_and_partial_sections_use_full_ui_and_visible_status() {
        for text in ["", "[ui]", "[ui]\ncompact_status_line = false"] {
            let config: ConfigFile = toml::from_str(text).unwrap();
            assert_eq!(config.ui.mode, UiMode::Full);
        }
        let config: ConfigFile = toml::from_str("[ui]\nmode = 'compact'").unwrap();
        assert_eq!(config.ui.mode, UiMode::Compact);
        assert!(config.ui.compact_status_line);
    }

    #[test]
    fn compact_status_can_be_disabled_and_invalid_settings_are_rejected() {
        let config: ConfigFile =
            toml::from_str("[ui]\nmode = 'compact'\ncompact_status_line = false").unwrap();
        assert!(!config.ui.compact_status_line);
        for text in [
            "[ui]\nmode = 'typo'",
            "[ui]\ncompact_status_line = 'false'",
            "[ui]\ncompact_status = true",
        ] {
            assert!(toml::from_str::<ConfigFile>(text).is_err(), "{text}");
        }
    }
}
