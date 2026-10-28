import type { Language } from "./i18n";
const text = {
  composerHint: ["本轮进行中 · Enter 将附件加入待发队列 · 右侧按钮停止", "Current turn running · Enter queues attachments · Right button stops", "実行中 · Enter で添付を送信待ちに追加 · 右のボタンで停止"],
  sending: ["正在向 Runtime 提交", "Submitting to the Runtime", "Runtime に送信中"],
  title: ["Runtime 待发消息（各端同步）", "Runtime queue (synced across clients)", "Runtime の送信待ち（端末間で同期）"],
  syncFailed: ["队列同步失败，正在重试；请保留尚未送达的输入。", "Queue sync failed; retrying. Keep any unsubmitted input.", "キュー同期に失敗。未送信の入力を保持してください。"],
  mutationFailed: ["队列操作失败，已刷新状态，请检查后重试。", "Queue update failed. State refreshed; check before retrying.", "キュー更新に失敗。最新の状態を確認して再試行してください。"],
  editedUnavailable: ["这条消息已开始或被另一端移除；你的编辑草稿仍保留，可复制后再提交。", "This message started or was removed on another client. Your edit draft is retained for copying and resubmission.", "このメッセージは開始済み、または別の端末で削除されました。編集内容はコピー用に保持しています。"],
  images: ["图片", "Images", "画像"],
  attachments: ["文本附件", "Text attachments", "テキスト添付"],
  independent: ["等待期间处理的独立任务", "Independent work while waiting", "待機中の独立タスク"],
  originalRetained: ["原待确认／待输入请求仍保留。", "The original confirmation/input request is retained.", "元の確認・入力待ちは保持されています。"],
  completedShort: ["已完成", "Completed", "完了"],
  runningShort: ["处理中", "Running", "処理中"],
  waitingShort: ["待确认／待输入", "Waiting for confirmation/input", "確認・入力待ち"],
  failedTask: ["已停止或未完成", "Stopped or incomplete", "停止または未完了"],
  you: ["你", "You", "あなた"],
  assistant: ["助手", "Assistant", "アシスタント"],
  manage: ["管理待发消息", "Manage queue", "送信待ちを管理"], close: ["关闭管理，继续发送", "Close manager and resume sending", "管理を閉じて送信を再開"],
  delete: ["删除", "Remove", "削除"], edit: ["编辑", "Edit", "編集"], save: ["保存", "Save", "保存"], cancel: ["取消", "Cancel", "取消"],
  retry: ["确认后重试发送", "Retry after checking delivery", "送達を確認して再送信"], failed: ["发送未确认，已保留，请先查看会话。", "Delivery is unconfirmed. Check the session before retrying.", "送達未確認です。再試行する前にセッションを確認してください。"],
  pending: ["等待当前轮次结束后发送", "Send after the current turn", "現在のターン終了後に送信"],
  pendingShort: ["待发送", "Queued", "送信待ち"], failedShort: ["送达待确认", "Unconfirmed", "送達未確認"],
  paused: ["管理期间暂停自动发送", "Automatic sending is paused while managing", "管理中は自動送信を停止"],
  enqueue: ["加入待发队列", "Add to queue", "送信待ちに追加"],
  enqueueNeedsSession: ["先发送一条消息开始会话，才能把内容放进队列", "Send a message to start a session before queueing", "キューに追加する前に、メッセージを送ってセッションを開始してください"],
  prioritize: ["优先处理（仍需判断依赖）", "Prioritize (dependency checks still apply)", "優先処理（依存関係を確認）"],
  runIndependent: ["标记为独立任务并处理（权限不变）", "Run as independent work (permissions unchanged)", "独立タスクとして処理（権限は変更しない）"],
  dependencyPolicy: ["等待期间，仅明确独立的消息可以先处理；有依赖或尚不明确的消息继续排队。", "While waiting, only clearly independent messages can run first; dependent or uncertain messages remain queued.", "待機中は独立性が明確なメッセージのみ先に処理し、依存関係があるか不明なものは待機します。"],
  copyCode: ["复制", "Copy", "コピー"], copiedCode: ["已复制", "Copied", "コピーしました"], copyFailed: ["复制失败", "Couldn't copy", "コピーできませんでした"],
} as const;
const index: Record<Language, number> = { "zh-CN": 0, en: 1, ja: 2 };
export function queueText(key: keyof typeof text, language: Language) { return text[key][index[language]]; }

/// 入队按钮什么时候出现、能不能点。
///
/// 忙时和空闲时都出现——空闲时也能选择「先入队、以后再发」，不必等这一轮跑起来，
/// 所以这一轮是否在跑不参与判断。
/// 输入框什么都没有时不出现，否则它只是一个点了也没效果的按钮。
/// 还没有会话时可以看见但点不了：入队需要一个已存在的会话，不为它隐式建一个空的。
export function enqueueButtonState(input: { hasDraft: boolean; hasSession: boolean }) {
  return { visible: input.hasDraft, enabled: input.hasSession };
}
