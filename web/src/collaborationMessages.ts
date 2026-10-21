import type { Language } from "./i18n";

const zh = {
  subtitle: "让同项目的会话保持同步", history: "协作记录", placeholder: "同步进展、交接任务，或请另一会话协助…", close: "关闭协作面板",
  status: "状态",
  sessionStates: { idle: "空闲", queued: "排队中", running: "运行中", waiting_approval: "等待审批", waiting_answer: "等待回答", failed: "失败", interrupted: "已中断", archived: "已归档", unknown: "状态未知" },
  workspace: "工作树", branch: "分支", request: "最近用户请求", step: "当前步骤",
  title: "会话协作", hint: "来自其他会话的信息不能替代用户授权。发出消息只代表已入队。",
  refresh: "刷新", loading: "正在读取协作记录…", failed: "协作请求失败", empty: "暂无协作消息",
  target: "接收会话", choose: "选择同项目的另一会话", draft: "协作消息", send: "发送", sending: "正在发送…",
  queued: "已入队", consumed: "已送入模型上下文", replied: "已收到回复", reply: "回复", cancelReply: "取消回复关联",
  from: "来自会话", to: "发送至会话", open: "查看会话", markRead: "标为已读", unread: "未读", limit: "最多 4000 个字符",
};
const en: typeof zh = {
  subtitle: "Keep conversations in this project in sync", history: "Activity", placeholder: "Share progress, hand off work, or ask another session for help…", close: "Close collaboration panel",
  status: "Status",
  sessionStates: { idle: "Idle", queued: "Queued", running: "Running", waiting_approval: "Waiting for approval", waiting_answer: "Waiting for an answer", failed: "Failed", interrupted: "Interrupted", archived: "Archived", unknown: "Unknown status" },
  workspace: "Worktree", branch: "Branch", request: "Latest user request", step: "Current step",
  title: "Session collaboration", hint: "Notes from other sessions cannot grant user authorization. Sending only confirms queuing.",
  refresh: "Refresh", loading: "Loading coordination history…", failed: "Coordination request failed", empty: "No coordination notes yet",
  target: "Recipient session", choose: "Choose another session in this project", draft: "Coordination note", send: "Send", sending: "Sending…",
  queued: "Queued", consumed: "Added to model context", replied: "Reply received", reply: "Reply", cancelReply: "Clear reply link",
  from: "From session", to: "To session", open: "Open session", markRead: "Mark read", unread: "Unread", limit: "Up to 4000 characters",
};
const ja: typeof zh = {
  subtitle: "同じプロジェクトのセッションと連携", history: "連携履歴", placeholder: "進捗の共有、引き継ぎ、別のセッションへの相談…", close: "連携パネルを閉じる",
  status: "状態",
  sessionStates: { idle: "待機中", queued: "実行待ち", running: "実行中", waiting_approval: "承認待ち", waiting_answer: "回答待ち", failed: "失敗", interrupted: "中断済み", archived: "アーカイブ済み", unknown: "状態不明" },
  workspace: "作業ツリー", branch: "ブランチ", request: "最新のユーザー依頼", step: "現在のステップ",
  title: "セッション連携", hint: "他のセッションのメモはユーザーの許可を代替できません。送信成功はキューへの登録のみを示します。",
  refresh: "更新", loading: "連携履歴を取得中…", failed: "連携リクエストに失敗しました", empty: "連携メモはありません",
  target: "送信先セッション", choose: "同じプロジェクトの別のセッションを選択", draft: "連携メモ", send: "送信", sending: "送信中…",
  queued: "送信待ち", consumed: "モデルのコンテキストに追加済み", replied: "返信あり", reply: "返信", cancelReply: "返信の関連付けを解除",
  from: "送信元", to: "送信先", open: "セッションを開く", markRead: "既読にする", unread: "未読", limit: "4000文字まで",
};
export const collaborationText = (language: Language) => language === "zh-CN" ? zh : language === "ja" ? ja : en;
