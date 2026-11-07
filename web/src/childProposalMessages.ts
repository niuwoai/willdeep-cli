import type { Language } from "./i18n";

const zh = {
  proposed: "等待开始", start: "开始", dismiss: "不创建", starting: "正在开始…", dismissing: "正在关闭…",
  hint: "任务已根据当前对话整理，点击“开始”后执行。", options: "执行选项", model: "模型",
  retry: "重试加载", error: "无法读取子会话提议，请重试。", dependencies: "等待这些任务完成",
};
const en: typeof zh = {
  proposed: "Ready to start", start: "Start", dismiss: "Dismiss", starting: "Starting…", dismissing: "Dismissing…",
  hint: "This task was prepared from your conversation. Select Start to run it.", options: "Execution options", model: "Model",
  retry: "Retry loading", error: "Could not load child conversation proposals. Try again.", dependencies: "Waits for these tasks",
};
const ja: typeof zh = {
  proposed: "開始待ち", start: "開始", dismiss: "作成しない", starting: "開始中…", dismissing: "閉じています…",
  hint: "会話の内容からタスクを準備しました。「開始」を選ぶと実行します。", options: "実行オプション", model: "モデル",
  retry: "再読み込み", error: "子セッションの提案を取得できません。再試行してください。", dependencies: "次のタスクの完了を待機",
};
export const childProposalText = (language: Language) => language === "zh-CN" ? zh : language === "ja" ? ja : en;
