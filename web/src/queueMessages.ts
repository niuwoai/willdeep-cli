import type { Language } from "./i18n";
const text = {
  composerHint: ["本轮进行中 · Enter 将附件加入待发队列 · 右侧按钮停止", "Current turn running · Enter queues attachments · Right button stops", "実行中 · Enter で添付を送信待ちに追加 · 右のボタンで停止"],
  sending: ["正在向 Runtime 提交", "Submitting to the Runtime", "Runtime に送信中"],
  title: ["本页待发消息", "Messages queued in this page", "このページの送信待ち"],
  manage: ["管理待发消息", "Manage queue", "送信待ちを管理"], close: ["关闭管理，继续发送", "Close manager and resume sending", "管理を閉じて送信を再開"],
  delete: ["删除", "Remove", "削除"], edit: ["编辑", "Edit", "編集"], save: ["保存", "Save", "保存"], cancel: ["取消", "Cancel", "取消"],
  retry: ["确认后重试发送", "Retry after checking delivery", "送達を確認して再送信"], failed: ["发送未确认，已保留，请先查看会话。", "Delivery is unconfirmed. Check the session before retrying.", "送達未確認です。再試行する前にセッションを確認してください。"],
  pending: ["等待当前轮次结束后发送", "Send after the current turn", "現在のターン終了後に送信"],
  pendingShort: ["待发送", "Queued", "送信待ち"], failedShort: ["送达待确认", "Unconfirmed", "送達未確認"],
  paused: ["管理期间暂停自动发送", "Automatic sending is paused while managing", "管理中は自動送信を停止"],
  enqueue: ["加入待发队列", "Add to queue", "送信待ちに追加"],
} as const;
const index: Record<Language, number> = { "zh-CN": 0, en: 1, ja: 2 };
export function queueText(key: keyof typeof text, language: Language) { return text[key][index[language]]; }
