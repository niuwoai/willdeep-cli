use ratatui::style::Color;
use serde::{Deserialize, Deserializer, de::Error as _};
use std::path::Path;

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
    pub show_thinking: bool,
    pub check_updates: bool,
    pub input_suggestion_color: InputSuggestionColor,
    pub input_suggestion_italic: bool,
    pub input_suggestion_underline: bool,
    /// 聊天气泡里拖完松手就写剪贴板。关掉之后仍可用 Ctrl+C / y 手动复制。
    pub copy_on_select: bool,
    /// 聊天区每条记录之间额外空出的行数。0 是原来的紧凑排版，上限见
    /// [`MAX_LINE_SPACING`]。终端一行就是一行，这里调的是行与行之间的空白。
    #[serde(deserialize_with = "deserialize_line_spacing")]
    pub line_spacing: u8,
}

/// 行距的上限。再大就把一屏挤到只剩两三行字，得不偿失。
pub const MAX_LINE_SPACING: u8 = 3;

fn deserialize_line_spacing<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    let spacing = u8::deserialize(deserializer)?;
    if spacing > MAX_LINE_SPACING {
        return Err(D::Error::custom(format!(
            "line_spacing must be between 0 and {MAX_LINE_SPACING}"
        )));
    }
    Ok(spacing)
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            mode: UiMode::Full,
            compact_status_line: true,
            show_thinking: true,
            check_updates: true,
            input_suggestion_color: InputSuggestionColor::Gray,
            input_suggestion_italic: false,
            input_suggestion_underline: false,
            copy_on_select: true,
            line_spacing: 0,
        }
    }
}

pub fn load_thinking_visibility(home: &Path) -> anyhow::Result<Option<bool>> {
    let path = home.join("ui-preferences.json");
    if !path.exists() {
        return Ok(None);
    }
    // 只认自己那一项：这份文件里还有行距，多出来的键不该让思考开关读不出来。
    let preference: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    Ok(preference
        .get("show_thinking")
        .and_then(serde_json::Value::as_bool))
}

/// 行距。没写过、或文件里没有这一项时返回 `None`，调用方保持默认。
/// 超过上限的值收回到上限：手改文件不该把聊天区撑成全空白。
pub fn load_line_spacing(home: &Path) -> anyhow::Result<Option<u8>> {
    let path = home.join("ui-preferences.json");
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)?;
    let preference: serde_json::Value = serde_json::from_slice(&bytes)?;
    let Some(value) = preference.get("line_spacing") else {
        return Ok(None);
    };
    let spacing = value
        .as_u64()
        .filter(|value| *value <= u64::from(u8::MAX))
        .map(|value| value as u8)
        .ok_or_else(|| anyhow::anyhow!("ui-preferences.json holds an invalid line spacing"))?;
    Ok(Some(spacing.min(MAX_LINE_SPACING)))
}

/// 记下行距，同时保住文件里已有的其他项（思考显示开关写在同一份里）。
pub fn save_line_spacing(home: &Path, spacing: u8) -> anyhow::Result<()> {
    let path = home.join("ui-preferences.json");
    let mut preference = if path.exists() {
        serde_json::from_slice(&std::fs::read(&path)?)?
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };
    let object = preference
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("ui-preferences.json is not an object"))?;
    object.insert(
        "line_spacing".to_owned(),
        serde_json::Value::from(spacing.min(MAX_LINE_SPACING)),
    );
    write_ui_preferences(home, &preference)
}

fn write_ui_preferences(home: &Path, value: &serde_json::Value) -> anyhow::Result<()> {
    use std::io::Write;

    std::fs::create_dir_all(home)?;
    let path = home.join("ui-preferences.json");
    let temporary = home.join(format!("ui-preferences-{}.tmp", std::process::id()));
    let contents = serde_json::to_vec(value)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&contents)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

pub fn save_thinking_visibility(home: &Path, show: bool) -> anyhow::Result<()> {
    // 和行距写在同一份文件里，只改自己那一项，不把另一项抹掉。
    let path = home.join("ui-preferences.json");
    let mut preference = if path.exists() {
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path)?)?
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };
    let object = preference
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("ui-preferences.json is not an object"))?;
    object.insert("show_thinking".to_owned(), serde_json::Value::from(show));
    write_ui_preferences(home, &preference)
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

    #[test]
    fn toml_line_spacing_rejects_values_above_the_ui_limit() {
        let defaults: ConfigFile = toml::from_str("[ui]").unwrap();
        assert_eq!(defaults.ui.line_spacing, 0);
        for spacing in [0, MAX_LINE_SPACING] {
            let config: ConfigFile =
                toml::from_str(&format!("[ui]\nline_spacing = {spacing}")).unwrap();
            assert_eq!(config.ui.line_spacing, spacing);
        }
        for spacing in [MAX_LINE_SPACING + 1, 255] {
            let error = toml::from_str::<ConfigFile>(&format!("[ui]\nline_spacing = {spacing}"))
                .unwrap_err()
                .to_string();
            assert!(error.contains("line_spacing"), "{error}");
        }
    }

    #[test]
    fn line_spacing_survives_next_to_the_thinking_switch() {
        let root = std::env::temp_dir().join(format!("ui-spacing-{}", uuid::Uuid::new_v4()));
        assert_eq!(load_line_spacing(&root).unwrap(), None);
        save_thinking_visibility(&root, false).unwrap();
        save_line_spacing(&root, 2).unwrap();
        // 后写的那一项不能把先写的那一项抹掉。
        assert_eq!(load_thinking_visibility(&root).unwrap(), Some(false));
        assert_eq!(load_line_spacing(&root).unwrap(), Some(2));
        // 手改超出上限的值收回来，不让聊天区被撑成空白。
        save_line_spacing(&root, 9).unwrap();
        assert_eq!(load_line_spacing(&root).unwrap(), Some(MAX_LINE_SPACING));
        std::fs::remove_dir_all(root).unwrap();
    }
}
