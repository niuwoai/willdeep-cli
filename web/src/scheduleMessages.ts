import type { Language } from "./i18n";
const languageIndex: Record<Language, number> = { "zh-CN": 0, en: 1, ja: 2 };
const text = {
  conversationTasks: ["会话定时任务", "Conversation schedules", "会話の定期タスク"],
  conversationHint: ["到时继续本会话；忙碌时跳过。关闭网页后由 Runtime 继续计时。", "Continue this conversation when due; skip when busy. Runtime keeps scheduling after the page closes.", "時刻になるとこの会話を続行します。使用中はスキップし、ページを閉じても Runtime が予定を管理します。"],
  source_session_busy: ["原会话正在执行或等待处理，本次已跳过", "Source conversation is active or waiting; this run was skipped", "元の会話が実行中・応答待ちのためスキップしました"],
  loadingTasks: ["正在加载定时任务…", "Loading schedules…", "定期タスクを読み込み中…"],
  title: ["定时任务", "Scheduled tasks", "定期タスク"],
  activeRun: ["当前运行", "Active run", "現在の実行"],
  inputTokens: ["输入", "Input", "入力"], outputTokens: ["输出", "Output", "出力"], totalTokens: ["合计", "Total", "合計"],
  sessionSwitchBusy: ["当前对话仍在运行，请结束或停止后再打开其它运行会话。", "Finish or stop the current conversation before opening another run session.", "現在の会話を終了または停止してから、別の実行セッションを開いてください。"],
  deleted: ["已删除任务的历史", "Deleted task history", "削除済みタスクの履歴"],
  discard: ["有未保存的配置，确认放弃这些修改？", "Discard unsaved configuration changes?", "未保存の変更を破棄しますか？"],
  uncertain: ["保存结果尚未确认，请重试同一请求以确认结果。", "Save outcome is uncertain. Retry the same request to confirm it.", "保存結果を確認できません。同じリクエストを再試行してください。"],
  permission: ["审批策略", "Approval policy", "承認ポリシー"],
  intro: ["由 Runtime 持续管理，关闭网页或终端也会按计划运行。", "Managed by the Runtime, even when the browser or terminal closes.", "Runtime が管理し、ブラウザーや端末を閉じても実行します。"],
  new: ["新建任务", "New task", "タスクを作成"], search: ["搜索任务名称或工作区", "Search task or workspace", "名前・作業領域を検索"],
  all: ["全部任务", "All tasks", "すべて"], enabled: ["已启用", "Enabled", "有効"], paused: ["已暂停", "Paused", "一時停止"], archived: ["已归档", "Archived", "アーカイブ"], completedTask: ["已完成", "Completed", "完了"],
  empty: ["暂无定时任务", "No scheduled tasks", "定期タスクなし"], select: ["选择任务查看配置和运行历史", "Select a task to inspect configuration and history", "タスクを選択して設定と履歴を確認"],
  config: ["配置", "Configuration", "設定"], history: ["运行历史", "Run history", "実行履歴"], name: ["任务名称", "Task name", "タスク名"], prompt: ["每次运行的任务", "Task for each run", "毎回実行する内容"], goal: ["目标达成后停止（可选）", "Stop when this goal is met (optional)", "達成後に停止する目標（任意）"],
  timezone: ["任务时区", "Task timezone", "タイムゾーン"], interval: ["每隔一段时间", "At an interval", "一定間隔"], dailyAt: ["每天", "Every day", "毎日"], weekdaysAt: ["工作日", "Weekdays", "平日"], weeklyAt: ["每周", "Every week", "毎週"], onceAt: ["仅一次", "Once", "1 回のみ"],
  minutes: ["间隔（分钟）", "Interval (minutes)", "間隔（分）"], time: ["运行时间", "Run time", "実行時刻"], weekday: ["星期", "Weekday", "曜日"], dateTime: ["单次运行时间", "One-time date and time", "1 回のみの実行日時"],
  clockHint: ["跳过不存在的夏令时时间；重复时段只在第一次运行。", "Skip nonexistent DST times; repeated times run at the first occurrence.", "存在しない夏時間の時刻をスキップし、重複時刻は最初の 1 回だけ実行します。"],
  next: ["下次运行", "Next run", "次の実行"], preview: ["预览未来运行时间", "Preview upcoming runs", "次の実行時刻を確認"], noNext: ["没有后续计划", "No future runs", "今後の予定なし"],
  runNow: ["立即运行", "Run now", "今すぐ実行"], pause: ["暂停计划", "Pause schedule", "予定を一時停止"], resume: ["恢复计划", "Resume schedule", "予定を再開"], archive: ["归档任务", "Archive task", "アーカイブ"],
  delete: ["永久删除任务", "Permanently delete task", "タスクを完全削除"], deleteConfirm: ["删除任务配置？运行中的任务必须先停止，已保存的历史仍保留。", "Delete task configuration? Stop active runs first. Saved history is retained.", "設定を削除しますか？先に実行を停止してください。履歴は保持します。"],
  archiveConfirm: ["归档并停止未来触发？当前运行会继续，历史保留。", "Archive and stop future triggers? The current run and history are retained.", "今後の実行を停止してアーカイブしますか？現在の実行と履歴は保持します。"],
  copy: ["复制为新任务", "Copy as new task", "新しいタスクとして複製"], save: ["保存配置", "Save configuration", "設定を保存"], saved: ["配置已保存", "Configuration saved", "設定を保存しました"], saving: ["正在保存", "Saving", "保存中"],
  changed: ["任务已被其他操作修改，请刷新后重试。", "The task changed. Refresh before retrying.", "設定が変更されました。更新後に再試行してください。"], refresh: ["刷新", "Refresh", "更新"],
  readOnly: ["当前 Web 服务仅允许查看，维护操作需要本机回环监听。", "This Web server is read-only. Maintenance requires loopback access.", "この Web サーバーは閲覧専用です。管理はループバック接続で利用できます。"],
  budget: ["运行预算与权限", "Run budget and permissions", "実行予算と権限"], inherited: ["沿用工作区配置", "Inherit workspace configuration", "作業領域の設定を使用"],
  profile: ["Provider 配置", "Provider profile", "Provider 設定"], model: ["模型", "Model", "モデル"], maxTurns: ["最大轮次", "Maximum turns", "最大ターン数"], timeout: ["最大运行时长（秒）", "Maximum runtime (seconds)", "最大実行時間（秒）"], tokens: ["Token 上限", "Token budget", "Token 予算"],
  runtimeDefault: ["使用 Runtime 默认值", "Runtime default", "Runtime の既定値"], missed: ["停机或休眠后错过计划", "Missed runs after downtime or sleep", "停止・スリープ中に逃した予定"], coalesce: ["最多补跑一次", "Coalesce to one run", "最大 1 回だけ補完"], skipMissed: ["全部跳过", "Skip all missed runs", "すべてスキップ"],
  failures: ["连续失败后暂停（次数）", "Pause after consecutive failures", "連続失敗で一時停止する回数"], retention: ["历史清理期限（天）", "History cleanup age (days)", "履歴を整理する日数"], maintenance: ["历史维护", "History maintenance", "履歴の管理"],
  cleanup: ["预览可清理历史", "Preview expired history", "期限切れの履歴を確認"], cleanupCount: ["可清理 {count} 条过期历史；运行中和近期记录保留。", "Remove {count} expired records; active and recent runs are retained.", "{count} 件を削除できます。実行中・最近の履歴は保持します。"],
  cleanupConfirm: ["确认删除这批过期运行记录？此操作不能恢复。", "Delete these expired run records? This cannot be undone.", "期限切れの記録を削除しますか？元に戻せません。"], cleanupApply: ["清理过期历史", "Clean expired history", "期限切れを削除"], cleanupEmpty: ["没有可清理的过期历史", "No expired history to remove", "削除できる履歴なし"],
  noRuns: ["暂无运行记录", "No run records yet", "実行記録なし"], more: ["加载更多", "Load more", "さらに表示"], runDetails: ["运行详情", "Run details", "実行詳細"], snapshot: ["本次运行的配置快照", "Configuration snapshot for this run", "今回の設定スナップショット"],
  stopRun: ["停止本次运行", "Stop this run", "今回の実行を停止"], stopConfirm: ["停止这次运行？任务计划继续保留。", "Stop this run? The task schedule is retained.", "今回の実行を停止しますか？予定は保持します。"],
  retryRun: ["重试本次运行", "Retry this run", "今回の実行を再試行"], retryHint: ["重试使用本次原配置，产生新记录；原历史保留。", "Retry with the original configuration and keep the original history.", "元の設定で新しい実行記録を作成し、元の履歴を保持します。"],
  openSession: ["打开运行会话", "Open run session", "実行セッションを開く"], noSession: ["尚未创建运行会话", "No run session yet", "実行セッションなし"], trigger: ["触发方式", "Trigger", "起動方法"],
  scheduled: ["计划触发", "Scheduled", "予定"], manual: ["手动触发", "Manual", "手動"], retry: ["重试", "Retry", "再試行"], imported: ["旧版本导入", "Imported", "旧版から移行"],
  preparing: ["准备入队", "Preparing", "準備中"], queued: ["排队中", "Queued", "待機中"], running: ["执行中", "Running", "実行中"], waiting_approval: ["等待审批", "Waiting for approval", "承認待ち"], waiting_answer: ["等待回答", "Waiting for answer", "回答待ち"],
  completed: ["执行完成", "Execution completed", "実行完了"], partial: ["部分完成", "Partial", "一部完了"], failed: ["失败", "Failed", "失敗"], cancelled: ["已取消", "Cancelled", "取消済み"], skipped: ["已跳过", "Skipped", "スキップ"], unknown: ["未知", "Unknown", "不明"],
  unverified: ["未验证", "Unverified", "未検証"], verified: ["验证通过", "Verification passed", "検証成功"], verificationFailed: ["验证失败", "Verification failed", "検証失敗"],
  result: ["结果摘要", "Result summary", "結果の概要"], reason: ["原因", "Reason", "理由"], usage: ["用量", "Usage", "使用量"], unknownUsage: ["用量尚未采集", "Usage not collected", "使用量未取得"],
  migrationPaused: ["旧插件未启用，导入后保持暂停；请确认配置后恢复。", "The legacy plugin was disabled. Inspect configuration before resuming.", "旧プラグインが無効だったため一時停止で移行しました。設定を確認して再開してください。"],
  consecutivePaused: ["已因连续失败暂停，请查看历史并处理。", "Paused after consecutive failures. Inspect run history.", "連続失敗で停止しました。履歴を確認してください。"],
  runtime_capacity_wait: ["等待 Runtime 的空闲运行额度", "Waiting for Runtime capacity", "Runtime の空き枠を待機"],
  previous_run_active: ["上一次运行仍未结束", "The previous run is still active", "前回の実行が終了していません"], missed_run_policy: ["按配置跳过错过的计划", "Skipped according to the missed-run policy", "逃した予定の設定によりスキップ"],
  legacy_history_unavailable: ["旧版本未保存完整历史，结果未知", "The legacy outcome is unknown", "旧版に完全な履歴がなく、結果は不明です"], outputUnavailable: ["本次输出不可用，请查看关联会话", "Output unavailable; inspect the linked session", "出力を取得できません。セッションを確認してください"],
  started: ["开始", "Started", "開始"], finished: ["结束", "Finished", "終了"], elapsed: ["耗时", "Duration", "所要時間"], runAt: ["计划时刻", "Scheduled time", "予定時刻"],
  error: ["定时任务操作失败", "Scheduled task operation failed", "定期タスク操作に失敗"], weekdayNames: ["周日,周一,周二,周三,周四,周五,周六", "Sunday,Monday,Tuesday,Wednesday,Thursday,Friday,Saturday", "日曜,月曜,火曜,水曜,木曜,金曜,土曜"],
  copiedName: ["{name}（副本）", "{name} (copy)", "{name}（複製）"], validation: ["请填写名称、任务、工作区及有效的计划和预算。", "Enter a name, task, workspace, valid schedule and budgets.", "名前・内容・作業領域・有効な予定と予算を入力してください。"],
  upgrade: ["Runtime 暂未提供此能力，请先升级 Runtime。", "Upgrade the Runtime to use this capability.", "先に Runtime を更新してください。"],
} as const;
export type ScheduleTextKey = keyof typeof text;
export function scheduleText(key: ScheduleTextKey, language: Language) { return (text[key] ?? text.unknown)[languageIndex[language]]; }
export function scheduleReason(reason: string, language: Language) {
  const aliases = { legacy_plugin_disabled: "migrationPaused", consecutive_failures: "consecutivePaused" } as const;
  if (reason in aliases) return scheduleText(aliases[reason as keyof typeof aliases], language);
  if (reason in text) return scheduleText(reason as ScheduleTextKey, language);
  return reason;
}
