import type { Language } from "./i18n";

const zh = {
  title: "选择此目录的审批模式",
  subtitle: "按操作风险与值守意愿选择，之后可在审批面板中修改。",
  smart: "智能审核",
  smartHint: "默认推荐：按现有规则与智能审核决定放行或询问。",
  strict: "严格模式",
  strictHint: "风险较高且你愿意值守时，逐次确认需要审批的操作。",
  full: "完全访问",
  fullHint: "减少操作审批，允许更大自主执行权限。请明确确认后使用。",
  continue: "保存并打开",
  saving: "正在保存…",
  failed: "无法保存此目录的审批模式",
};
const en: typeof zh = {
  title: "Choose an approval mode for this directory",
  subtitle: "Choose how closely you want to review operations. You can change this in the approval panel later.",
  smart: "Smart review",
  smartHint: "Recommended default: existing rules and smart review decide when to proceed or ask.",
  strict: "Strict mode",
  strictHint: "For higher risk work when you can stay available to approve operations that need review.",
  full: "Full access",
  fullHint: "Fewer approval prompts and more autonomous execution. Requires your explicit confirmation.",
  continue: "Save and open",
  saving: "Saving…",
  failed: "Could not save the approval mode for this directory",
};
const ja: typeof zh = {
  title: "このディレクトリの承認モードを選択",
  subtitle: "操作のリスクと確認に対応できるかに応じて選べます。後で承認パネルから変更できます。",
  smart: "スマート承認",
  smartHint: "推奨の初期値：既存のルールとスマート承認で、実行するか確認するかを判断します。",
  strict: "厳格モード",
  strictHint: "リスクが高く、必要な操作をその都度確認できる場合に。",
  full: "フルアクセス",
  fullHint: "承認の確認を減らし、より自律的な実行を許可します。明示的な確認が必要です。",
  continue: "保存して開く",
  saving: "保存中…",
  failed: "このディレクトリの承認モードを保存できませんでした",
};
export const workspaceApprovalText = (language: Language) => language === "zh-CN" ? zh : language === "ja" ? ja : en;
