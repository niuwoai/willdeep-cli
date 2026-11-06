use anyhow::{Result, bail};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    ZhCn,
    En,
    Ja,
}

impl Language {
    pub fn parse(value: Option<&str>) -> Result<Self> {
        let Some(value) = value else {
            return Ok(Self::default());
        };
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "zh" | "zh-cn" | "zh-hans" => Ok(Self::ZhCn),
            "en" | "en-us" | "en-gb" => Ok(Self::En),
            "ja" | "ja-jp" => Ok(Self::Ja),
            _ => bail!("unsupported language '{value}'; expected zh-CN, en, or ja"),
        }
    }

    pub fn text(self, zh: &'static str, en: &'static str, ja: &'static str) -> &'static str {
        match self {
            Self::ZhCn => zh,
            Self::En => en,
            Self::Ja => ja,
        }
    }

    pub fn workspace_approval_title(self) -> &'static str {
        self.text(
            "选择此目录的审批模式",
            "Choose approval mode for this directory",
            "このディレクトリの承認モードを選択",
        )
    }

    pub fn workspace_approval_cancelled(self) -> &'static str {
        self.text(
            "已取消打开目录",
            "Opening directory cancelled",
            "ディレクトリを開く操作をキャンセルしました",
        )
    }

    pub fn workspace_approval_hint(self) -> &'static str {
        self.text(
            "↑/↓ 选择 · Enter 确认 · Esc 取消 · 之后可用 /permissions 调整",
            "↑/↓ select · Enter confirm · Esc cancel · Change later with /permissions",
            "↑/↓ 選択 · Enter 確定 · Esc キャンセル · 後から /permissions で変更可能",
        )
    }

    pub fn workspace_approval_summary(self, mode: willdeep_core::ApprovalMode) -> &'static str {
        use willdeep_core::ApprovalMode;
        match mode {
            ApprovalMode::Strict => self.text(
                "风险较高且愿意值守时选择；需要审批的操作先由你确认。",
                "For higher-risk work when you can stay present; confirm actions that require approval.",
                "リスクが高く、立ち会える場合に。承認が必要な操作を自分で確認します。",
            ),
            ApprovalMode::Smart => self.text(
                "默认推荐；按现有规则和智能审核决定放行或询问。",
                "Recommended default; rules and smart review decide whether to proceed or ask.",
                "推奨の既定値。ルールとスマート審査で実行または確認を判断します。",
            ),
            ApprovalMode::FullAccess => self.text(
                "愿意给予更大自主执行权限时选择；减少操作审批，仍保留必要保护。",
                "For greater execution autonomy; fewer approval prompts, with necessary protections retained.",
                "より広い自律実行権限を与える場合に。承認確認を減らし、必要な保護は維持します。",
            ),
            _ => "",
        }
    }

    pub fn compression_configuration_error(self) -> &'static str {
        self.text(
            "压缩配置无法加载，请检查配置文件后重试",
            "Cannot load compression settings; check the configuration file and retry",
            "圧縮設定を読み込めません。設定ファイルを確認して再試行してください",
        )
    }

    /// 规范语言标签，与桌面端遥测上报的 `locale` 口径一致（`zh-Hans` 而非 `zh-CN`），
    /// 否则同一个中文用户在两端会被聚合成两条曲线。
    pub fn telemetry_tag(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-Hans",
            Self::En => "en",
            Self::Ja => "ja",
        }
    }

    /// `text` 的取值版：文案需要插值时用它，三个候选都已构造完毕，只挑一个。
    pub fn pick<T>(self, zh: T, en: T, ja: T) -> T {
        match self {
            Self::ZhCn => zh,
            Self::En => en,
            Self::Ja => ja,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_required_languages_and_aliases() {
        assert_eq!(Language::parse(Some("zh_CN")).unwrap(), Language::ZhCn);
        assert_eq!(Language::parse(Some("en-US")).unwrap(), Language::En);
        assert_eq!(Language::parse(Some("ja-JP")).unwrap(), Language::Ja);
        assert!(Language::parse(Some("fr")).is_err());
    }
}
