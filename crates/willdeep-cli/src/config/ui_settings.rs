use ratatui::style::Color;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiMode {
    #[default]
    Full,
    Compact,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSuggestionColor {
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    #[default]
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
}

impl InputSuggestionColor {
    pub fn ratatui(self) -> Color {
        match self {
            Self::Reset => Color::Reset,
            Self::Black => Color::Black,
            Self::Red => Color::Red,
            Self::Green => Color::Green,
            Self::Yellow => Color::Yellow,
            Self::Blue => Color::Blue,
            Self::Magenta => Color::Magenta,
            Self::Cyan => Color::Cyan,
            Self::Gray => Color::Gray,
            Self::DarkGray => Color::DarkGray,
            Self::LightRed => Color::LightRed,
            Self::LightGreen => Color::LightGreen,
            Self::LightYellow => Color::LightYellow,
            Self::LightBlue => Color::LightBlue,
            Self::LightMagenta => Color::LightMagenta,
            Self::LightCyan => Color::LightCyan,
            Self::White => Color::White,
        }
    }
}

/// Terminal presentation only; never changes task or approval settings.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiSettings {
    pub mode: UiMode,
    pub compact_status_line: bool,
    pub check_updates: bool,
    pub input_suggestion_color: InputSuggestionColor,
    pub input_suggestion_italic: bool,
    pub input_suggestion_underline: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            mode: UiMode::Full,
            compact_status_line: true,
            check_updates: true,
            input_suggestion_color: InputSuggestionColor::Gray,
            input_suggestion_italic: false,
            input_suggestion_underline: false,
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

    #[test]
    fn input_suggestion_style_defaults_to_gray_and_accepts_color_and_modifiers() {
        let defaults: ConfigFile = toml::from_str("[ui]").unwrap();
        assert_eq!(
            defaults.ui.input_suggestion_color,
            InputSuggestionColor::Gray
        );
        assert!(!defaults.ui.input_suggestion_italic);
        assert!(!defaults.ui.input_suggestion_underline);

        let configured: ConfigFile = toml::from_str(
            "[ui]\ninput_suggestion_color = 'dark_gray'\ninput_suggestion_italic = true\ninput_suggestion_underline = true",
        )
        .unwrap();
        assert_eq!(
            configured.ui.input_suggestion_color.ratatui(),
            Color::DarkGray
        );
        assert!(configured.ui.input_suggestion_italic);
        assert!(configured.ui.input_suggestion_underline);
        assert!(
            toml::from_str::<ConfigFile>("[ui]\ninput_suggestion_color = 'not-a-color'").is_err()
        );
    }
}
