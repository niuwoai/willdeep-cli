import type { Language } from "./i18n";

const zh = {
  title: "工作流", empty: "暂无工作流运行。在对话中描述任务即可创建并运行工作流。", chooseSession: "选择会话以查看工作流。",
  loading: "正在读取工作流…", refresh: "刷新", failed: "工作流请求失败", unavailable: "工作流仅在本机连接中可用。",
  readOnly: "当前连接或工作区为只读，无法控制工作流。", details: "运行详情", runId: "运行 ID", source: "源运行",
  phasesHint: "阶段用于分组；节点按依赖关系执行，可跨阶段并行。", ungrouped: "其他节点", nodes: "节点",
  stop: "停止整组", retry: "重试失败节点", resume: "恢复中断运行", pending: "正在提交…", received: "服务端已接收操作，当前状态：",
  history: "运行历史", noHistory: "暂无运行历史", close: "收起", report: "报告", output: "结构化输出", input: "输入", error: "错误",
  model: "实际模型", tokens: "Token", unknown: "未知", usage: "用量", inputTokens: "输入", outputTokens: "输出", cacheTokens: "缓存读取",
  accounted: "已记录 Token", unknownCalls: "用量未知的调用", uncertain: "执行用量尚不确定", duration: "耗时", seconds: "秒",
  started: "开始时间", finished: "结束时间", created: "创建时间", notStarted: "尚未开始", active: "进行中",
  agentRun: "Agent 运行", reused: "复用自", instance: "节点实例", attempt: "尝试", dependsOn: "依赖", item: "项目", iteration: "轮次",
  then: "条件成立", else: "条件不成立", noInstances: "尚未展开实例", acknowledged: "报告已送达", unacknowledged: "报告尚未送达",
  kinds: { agent: "Agent", switch: "条件分支", foreach: "逐项执行", repeat: "循环执行" },
  statuses: { pending: "等待中", dispatch_pending: "等待分派", dispatched: "已分派", running: "运行中", stopping: "正在停止",
    completed: "已完成", failed: "失败", blocked: "受阻", interrupted: "已中断", cancelled: "已取消" },
};
export type WorkflowMessages = typeof zh;
const en: WorkflowMessages = {
  title: "Workflows", empty: "No workflow runs yet. Describe a task in the conversation to create and run a workflow.", chooseSession: "Select a session to view workflows.",
  loading: "Loading workflows…", refresh: "Refresh", failed: "Workflow request failed", unavailable: "Workflows are only available through a local connection.",
  readOnly: "This connection or workspace is read-only. Workflow controls are unavailable.", details: "Run details", runId: "Run ID", source: "Source run",
  phasesHint: "Phases group nodes. Dependencies determine execution, including parallel work across phases.", ungrouped: "Other nodes", nodes: "Nodes",
  stop: "Stop workflow", retry: "Retry failed nodes", resume: "Resume interrupted run", pending: "Submitting…", received: "Server accepted the operation. Current status:",
  history: "Run history", noHistory: "No run history yet", close: "Collapse", report: "Report", output: "Structured output", input: "Input", error: "Error",
  model: "Actual model", tokens: "Tokens", unknown: "Unknown", usage: "Usage", inputTokens: "Input", outputTokens: "Output", cacheTokens: "Cache read",
  accounted: "Recorded tokens", unknownCalls: "Calls with unknown usage", uncertain: "Execution usage is uncertain", duration: "Duration", seconds: "s",
  started: "Started", finished: "Finished", created: "Created", notStarted: "Not started", active: "In progress",
  agentRun: "Agent run", reused: "Reused from", instance: "Node instance", attempt: "Attempt", dependsOn: "Depends on", item: "Item", iteration: "Iteration",
  then: "Condition met", else: "Condition not met", noInstances: "No instances yet", acknowledged: "Report delivered", unacknowledged: "Report not delivered yet",
  kinds: { agent: "Agent", switch: "Branch", foreach: "For each", repeat: "Repeat" },
  statuses: { pending: "Pending", dispatch_pending: "Awaiting dispatch", dispatched: "Dispatched", running: "Running", stopping: "Stopping",
    completed: "Completed", failed: "Failed", blocked: "Blocked", interrupted: "Interrupted", cancelled: "Cancelled" },
};
const ja: WorkflowMessages = {
  title: "ワークフロー", empty: "実行履歴はありません。会話でタスクを説明するとワークフローを作成・実行できます。", chooseSession: "セッションを選択してワークフローを表示します。",
  loading: "ワークフローを取得中…", refresh: "更新", failed: "ワークフローのリクエストに失敗しました", unavailable: "ワークフローはローカル接続でのみ利用できます。",
  readOnly: "この接続またはワークスペースは読み取り専用です。ワークフローを操作できません。", details: "実行の詳細", runId: "実行 ID", source: "元の実行",
  phasesHint: "フェーズは表示上のグループです。依存関係に従い、フェーズをまたいで並列実行できます。", ungrouped: "その他のノード", nodes: "ノード",
  stop: "全体を停止", retry: "失敗ノードを再試行", resume: "中断した実行を再開", pending: "送信中…", received: "サーバーが操作を受け付けました。現在の状態：",
  history: "実行履歴", noHistory: "実行履歴はありません", close: "折りたたむ", report: "レポート", output: "構造化出力", input: "入力", error: "エラー",
  model: "実際のモデル", tokens: "トークン", unknown: "不明", usage: "使用量", inputTokens: "入力", outputTokens: "出力", cacheTokens: "キャッシュ読み取り",
  accounted: "記録済みトークン", unknownCalls: "使用量不明の呼び出し", uncertain: "実行の使用量は未確定です", duration: "所要時間", seconds: "秒",
  started: "開始日時", finished: "終了日時", created: "作成日時", notStarted: "未開始", active: "実行中",
  agentRun: "Agent の実行", reused: "再利用元", instance: "ノードインスタンス", attempt: "試行", dependsOn: "依存先", item: "項目", iteration: "反復",
  then: "条件成立", else: "条件不成立", noInstances: "インスタンスはまだありません", acknowledged: "レポート送信済み", unacknowledged: "レポート未送信",
  kinds: { agent: "Agent", switch: "条件分岐", foreach: "項目ごとに実行", repeat: "繰り返し" },
  statuses: { pending: "待機中", dispatch_pending: "割り当て待ち", dispatched: "割り当て済み", running: "実行中", stopping: "停止中",
    completed: "完了", failed: "失敗", blocked: "ブロック中", interrupted: "中断済み", cancelled: "キャンセル済み" },
};
export const workflowText = (language: Language): WorkflowMessages => language === "zh-CN" ? zh : language === "ja" ? ja : en;
