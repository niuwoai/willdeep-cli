import type { Language } from "./i18n";

const zhCN = {
  sessions: "待处理会话",
  empty: "当前没有需要处理的会话",
  untitled: "未命名会话 {id}",
  waiting_approval: "等待你审批",
  waiting_answer: "等待你回答",
  failed: "执行失败，需要处理",
  interrupted: "执行已中断，需要处理",
  partial: "执行尚未完成，需要处理",
};

type AttentionMessages = { [Key in keyof typeof zhCN]: string };

export const attentionMessages: Record<Language, AttentionMessages> = {
  "zh-CN": zhCN,
  en: {
    sessions: "Sessions needing attention",
    empty: "No sessions need attention right now",
    untitled: "Untitled session {id}",
    waiting_approval: "Waiting for your approval",
    waiting_answer: "Waiting for your answer",
    failed: "Execution failed and needs attention",
    interrupted: "Execution was interrupted and needs attention",
    partial: "Execution is incomplete and needs attention",
  },
  ja: {
    sessions: "対応が必要なセッション",
    empty: "現在、対応が必要なセッションはありません",
    untitled: "無題のセッション {id}",
    waiting_approval: "承認を待っています",
    waiting_answer: "回答を待っています",
    failed: "実行に失敗しました。対応が必要です",
    interrupted: "実行が中断されました。対応が必要です",
    partial: "実行が完了していません。対応が必要です",
  },
};
