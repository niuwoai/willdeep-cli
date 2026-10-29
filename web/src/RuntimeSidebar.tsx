import { useState } from "react";
import { Box, Button, NativeSelect, Textarea } from "@chakra-ui/react";
import { ChevronDown, ChevronRight, ChevronUp, Plus } from "lucide-react";
import type { Messages } from "./i18n";
import "./runtimeSidebar.css";
const AGENT_PREVIEW_LIMIT = 4;
import { RuntimeDetailPanel, type RuntimeDetailTarget } from "./RuntimeDetailPanel";
import { agentDuration, isSidebarAgent } from "./runtimeAgents";

/// 只读的两个公开职责。写入与命令类必须走父 Agent 的安全链。
export type AgentSpawnProfile = "generalist" | "reviewer";

export type RuntimeTool = {
  id: string;
  session_id: string | null;
  turn_id: string | null;
  task_id: string;
  agent_id: string;
  name: string;
  status: "running" | "completed" | "partial" | "failed" | "interrupted";
  started_at_ms: number;
  completed_at_ms: number | null;
};

export type RuntimeArtifact = {
  id: string;
  title: string;
  kind: "workspace_change";
  session_id: string | null;
  turn_id: string | null;
  task_id: string;
  agent_id: string;
  item_count: number;
  created_at: number;
};

export type RuntimeAgent = {
  id: string;
  parent_id: string | null;
  label: string | null;
  background: boolean;
  profile: string | null;
  model: string | null;
  status: string;
  current_turn: number;
  current_tool: string | null;
  retry_wait?: { attempt: number; delay_ms: number } | null;
  total_tokens: number | null;
  elapsed_seconds: number;
  finished_seconds_ago: number | null;
  worktree_branch: string | null;
  dedicated_worktree: boolean;
};

export type RuntimeTask = {
  id: string;
  session_id: string | null;
  turn_id: string | null;
  agent_id: string | null;
  status: string;
  profile: string | null;
  elapsed_seconds: number;
  exit_code: number | null;
  failure_domain: string | null;
};

export type RuntimeGate =
  | { kind: "approval"; id: string; task_id: string; description: string; always_allow_available: boolean; parked?: boolean }
  | { kind: "question"; id: string; task_id: string; question: string; options: string[]; multi_select: boolean; parked?: boolean };

/// 一条运行时事件。正文不在这里：服务端只给打码截断过的标题摘要。
export type RuntimeEvent = {
  id: string;
  source: string;
  kind: string;
  priority: string;
  title_excerpt: string | null;
  requires_user_action: boolean;
  merge_count: number;
  created_at: string;
  delivery_state: string;
};

export type RuntimeActivity = {
  tools: RuntimeTool[];
  artifacts: RuntimeArtifact[];
  agents: RuntimeAgent[];
  tasks: RuntimeTask[];
  gates: RuntimeGate[];
  attention_count: number;
  teams?: AgentTeamSummary[];
  team_statistics_unavailable?: boolean;
};

export type AgentUsageSummary = {
  model_calls: number; unknown_usage_calls: number; accounted_tokens: number;
  input_tokens: number | null; output_tokens: number | null;
  cache_read_tokens: number | null; total_tokens: number | null; uncertain_execution: boolean;
};
export type AgentRun = {
  id: string; agent_id: string; sequence: number; status: string; reason: string;
  queued_at_ms: number; started_at_ms: number | null; finished_at_ms: number | null;
  current_turn: number; current_tool: string | null; usage: AgentUsageSummary;
  report: string | null;
};
export type AgentRunsPage = { runs: AgentRun[]; next_after_sequence: number | null };
export type AgentTeamSummary = {
  parent_session_id: string;
  policy: { max_concurrent: number; max_queued: number; token_budget: number | null } | null;
  running: number; queued: number; completed_runs: number;
  usage: AgentUsageSummary; parent_usage: AgentUsageSummary; child_usage: AgentUsageSummary;
  auxiliary_usage: AgentUsageSummary; compression_usage: AgentUsageSummary; ledger_incomplete: boolean;
  workers: { agent_id: string; latest_run: AgentRun; run_count: number;
    queue_position: number | null; cumulative_usage: AgentUsageSummary }[];
};

type Props = {
  onLoadRuns?: (id: string, after?: number) => Promise<AgentRunsPage>;
  activity: RuntimeActivity;
  events: RuntimeEvent[];
  onIgnoreEvent: (id: string) => Promise<void>;
  messages: Messages;
  onOpenGate: (id: string) => void;
  onAgentAction: (id: string, action: "stop" | "retry" | "retry_model" | "prompt" | "continue", currentModel?: string | null) => Promise<void>;
  canSpawnAgent: boolean;
  onSpawnAgent: (profile: AgentSpawnProfile, prompt: string) => Promise<boolean>;
};

function runtimeStatus(status: string, t: Messages) {
  if (status === "working" || status === "running") return t.working;
  if (status === "queued") return t.queued;
  if (status === "cancelling") return t.cancelling;
  if (status === "waiting_approval") return t.waitingApproval;
  if (status === "waiting_answer") return t.waitingAnswer;
  if (status === "failed") return t.toolFailed;
  if (status === "done" || status === "completed") return t.toolDone;
  if (status === "cancelled") return t.cancelled;
  if (status === "interrupted") return t.interrupted;
  if (status === "partial") return t.partial;
  if (status === "blocked") return t.blocked;
  if (status === "idle") return t.idle;
  return t.unknownValue;
}

function failureDomain(domain: string | null, t: Messages) {
  if (domain === "provider") return t.failureProvider;
  if (domain === "policy") return t.failurePolicy;
  if (domain === "tool") return t.failureTool;
  if (domain === "harness") return t.failureHarness;
  if (domain === "internal") return t.failureInternal;
  return t.unknownValue;
}

export function RuntimeSidebar({ activity, events, onIgnoreEvent, messages: t, onOpenGate, onAgentAction, canSpawnAgent, onSpawnAgent, onLoadRuns }: Props) {
  const [controlBusy, setControlBusy] = useState(false);
  const [spawnProfile, setSpawnProfile] = useState<AgentSpawnProfile>("generalist");
  const [spawnPrompt, setSpawnPrompt] = useState("");
  const [detail, setDetail] = useState<RuntimeDetailTarget | null>(null);
  // 只列还等着人的那些。已经由 Agent 读过但不需要人处理的事件不该占用户的注意力。
  const pendingEvents = events.filter((event) => event.requires_user_action && event.delivery_state !== "ignored" && event.delivery_state !== "resolved");
  const [expanded, setExpanded] = useState(false);
  const [spawnOpen, setSpawnOpen] = useState(false);
  const [showAllAgents, setShowAllAgents] = useState(false);
  const [showAgentHistory, setShowAgentHistory] = useState(false);
  const runningTools = activity.tools.filter((tool) => tool.status === "running").length;
  const sidebarAgents = showAgentHistory ? activity.agents : activity.agents.filter(isSidebarAgent);
  const hiddenAgents = activity.agents.length - sidebarAgents.length;
  async function runControl(action: () => Promise<void>) {
    if (controlBusy) return;
    setControlBusy(true);
    try { await action(); } finally { setControlBusy(false); }
  }
  async function spawnAgent() {
    const task = spawnPrompt.trim();
    if (!canSpawnAgent || !task || controlBusy) return;
    setControlBusy(true);
    try { if (await onSpawnAgent(spawnProfile, task)) setSpawnPrompt(""); } finally { setControlBusy(false); }
  }
  return <Box mt="4" p="3" border="1px solid" borderColor="var(--border-subtle)" borderRadius="md" bg="var(--bg-raised)" flexShrink="0" className="runtime-panel">
    <button type="button" className="runtime-toggle" aria-expanded={expanded} aria-label={activity.attention_count > 0 ? `${t.runtimeActivity} · ${t.needsAttention} ${activity.attention_count}` : t.runtimeActivity} onClick={() => setExpanded((current) => !current)}>
      <span className="runtime-toggle-title">{t.runtimeActivity}</span>
      {activity.attention_count > 0 && <span className="runtime-toggle-badge" title={`${t.needsAttention} ${activity.attention_count}`} aria-hidden="true">{activity.attention_count}</span>}
      <span className="runtime-toggle-hint" aria-hidden="true">{expanded ? t.collapse : t.expand}{expanded ? <ChevronUp size={13} /> : <ChevronDown size={13} />}</span>
    </button>
    {expanded && <div className="runtime-body">
      <div className="runtime-stats">
        <div className="runtime-stat"><span className="runtime-stat-label">{t.tools}</span><span className="runtime-stat-value">{activity.tools.length}</span></div>
        <div className="runtime-stat"><span className="runtime-stat-label">{t.running}</span><span className="runtime-stat-value">{runningTools}</span></div>
        <div className="runtime-stat"><span className="runtime-stat-label">{t.artifacts}</span><span className="runtime-stat-value">{activity.artifacts.length}</span></div>
        <div className="runtime-stat"><span className="runtime-stat-label">{t.tasks}</span><span className="runtime-stat-value">{activity.tasks.length}</span></div>
        <div className="runtime-stat"><span className="runtime-stat-label">{t.agents}</span><span className="runtime-stat-value">{sidebarAgents.length}{hiddenAgents > 0 && <small title={`+${hiddenAgents} ${t.finishedHidden}`}>+{hiddenAgents}</small>}</span></div>
        <div className={`runtime-stat${activity.attention_count > 0 ? " alert" : ""}`}><span className="runtime-stat-label">{t.needsAttention}</span><span className="runtime-stat-value">{activity.attention_count}</span></div>
        <div className="runtime-stat wide"><span className="runtime-stat-label">{t.runtimeEvents}</span><span className="runtime-stat-value">{pendingEvents.length}</span></div>
      </div>
      {activity.gates.length > 0 && <div className="runtime-section">
        <div className="runtime-section-head"><span>{t.needsAttention}</span><span className="runtime-section-count">{activity.gates.length}</span></div>
        <div className="runtime-list">{activity.gates.map((gate) => <div key={gate.id} className="runtime-card">
          <div className="runtime-card-head">
            <span className="runtime-gate-state">{gate.kind === "approval" ? t.waitingApproval : t.waitingAnswer}</span>
            <button type="button" className="runtime-link" onClick={() => onOpenGate(gate.id)}>{t.openHumanGate}<ChevronRight size={12} aria-hidden="true" /></button>
          </div>
          <p className="runtime-card-text">{gate.kind === "approval" ? gate.description : gate.question}</p>
        </div>)}</div>
      </div>}
      {pendingEvents.length > 0 && <div className="runtime-section">
        <div className="runtime-section-head"><span>{t.runtimeEventsPending}</span><span className="runtime-section-count">{pendingEvents.length}</span></div>
        <div className="runtime-list">{pendingEvents.slice(0, 5).map((event) => <div key={event.id} className="runtime-card">
          <div className="runtime-card-head">
            <span className="runtime-row-meta">{event.source} · {event.priority}{event.merge_count > 1 ? ` ×${event.merge_count}` : ""}</span>
            {/* 忽略只结算「还等着人」这一侧，不批准任何操作：审批仍在它自己的卡片上回答。 */}
            <button type="button" className="runtime-link" disabled={controlBusy} onClick={() => void runControl(() => onIgnoreEvent(event.id))}>{t.ignoreEvent}</button>
          </div>
          <p className="runtime-card-text">{event.title_excerpt ?? event.kind}</p>
          <span className="runtime-card-note">{event.delivery_state === "handled" ? t.eventSeenByAgent : t.eventNotDelivered}</span>
        </div>)}</div>
      </div>}
      {activity.tasks.length > 0 && <div className="runtime-section">
        <div className="runtime-section-head"><span>{t.tasks}</span><span className="runtime-section-count">{activity.tasks.length}</span></div>
        <div className="runtime-list">{activity.tasks.slice(0, 4).map((task) => <div key={task.id} className="runtime-card">
          <div className="runtime-row-main">
            <span className="runtime-row-title">{task.profile || t.task}</span>
            <span className="runtime-row-meta">{runtimeStatus(task.status, t)} · {task.elapsed_seconds}{t.secondsUnit}</span>
            <button type="button" className="runtime-link" onClick={() => setDetail({ kind: "task", id: task.id })}>{t.details}</button>
          </div>
          {(task.exit_code !== null || task.failure_domain) && <span className="runtime-card-note">
            {task.exit_code !== null ? `${t.exitCode}: ${task.exit_code}` : ""}{task.exit_code !== null && task.failure_domain ? " · " : ""}{task.failure_domain ? `${t.failureDomain}: ${failureDomain(task.failure_domain, t)}` : ""}
          </span>}
        </div>)}</div>
      </div>}
      {activity.team_statistics_unavailable && <span className="runtime-card-note">{t.teamStatisticsUnavailable}</span>}
      {activity.teams?.map((team) => <div key={team.parent_session_id} className="runtime-team">
        <p>{t.agentTeam} · {t.running} {team.running}/{team.policy?.max_concurrent ?? t.unknownValue} · {t.queued} {team.queued}/{team.policy?.max_queued ?? t.unknownValue}</p>
        <p>{t.teamUsage}: {team.usage.accounted_tokens} {t.tokenUnit} · {t.modelCalls}: {team.usage.model_calls}</p>
        <p>{t.parentUsage}: {team.parent_usage.accounted_tokens} · {t.childUsage}: {team.child_usage.accounted_tokens} · {t.auxiliaryUsage}: {team.auxiliary_usage.accounted_tokens} · {t.compressionUsage}: {team.compression_usage.accounted_tokens}</p>
        {team.usage.unknown_usage_calls > 0 && <p>{t.unknownUsageCalls}: {team.usage.unknown_usage_calls}</p>}
        {team.ledger_incomplete && <p>{t.ledgerIncomplete}</p>}
      </div>)}
      {sidebarAgents.length > 0 && <div className="runtime-section">
        <div className="runtime-section-head"><span>{t.agents}</span><span className="runtime-section-count">{sidebarAgents.length}</span></div>
        <div className="runtime-list">{sidebarAgents.slice(0, showAllAgents ? sidebarAgents.length : AGENT_PREVIEW_LIMIT).map((agent) => <Box key={agent.id} className="runtime-agent" pl={agent.parent_id ? "3" : "0"}>
          <div className="runtime-row-main"><span className="runtime-row-title">{agent.label || agent.profile || t.agent}</span><span className="runtime-row-meta">{agent.retry_wait ? t.retryWaiting : runtimeStatus(agent.status, t)} · {t.turn} {agent.current_turn}</span><button type="button" className="runtime-link" onClick={() => setDetail({ kind: "agent", id: agent.id })}>{t.details}</button></div>
          <span className="runtime-card-note">{agent.model || t.unknownValue} · {agent.total_tokens ?? t.unknownValue} {t.tokenUnit} · {agentDuration(agent, t)}{agent.dedicated_worktree ? ` · ${agent.worktree_branch || t.worktree}` : ""}</span>
          {agent.background && <div className="runtime-agent-actions">
            {agent.status === "working" && <button type="button" className="runtime-link" disabled={controlBusy} onClick={() => void runControl(() => onAgentAction(agent.id, "prompt"))}>{t.instruct}</button>}
            {["working", "queued"].includes(agent.status) && <button type="button" className="runtime-link" disabled={controlBusy} onClick={() => void runControl(() => onAgentAction(agent.id, "stop"))}>{t.stop}</button>}
            {["blocked", "failed", "done", "completed", "partial", "cancelled", "interrupted"].includes(agent.status) && <><button type="button" className="runtime-link" disabled={controlBusy} onClick={() => void runControl(() => onAgentAction(agent.id, "continue"))}>{t.continueAgent}</button><button type="button" className="runtime-link" disabled={controlBusy} onClick={() => void runControl(() => onAgentAction(agent.id, "retry"))}>{t.retry}</button><button type="button" className="runtime-link" disabled={controlBusy} onClick={() => void runControl(() => onAgentAction(agent.id, "retry_model", agent.model))}>{t.changeModel}</button></>}
          </div>}
        </Box>)}</div>
        {sidebarAgents.length > AGENT_PREVIEW_LIMIT && <button type="button" className="runtime-link" onClick={() => setShowAllAgents(!showAllAgents)}>{showAllAgents ? t.showFewerAgents : t.showAllAgents}</button>}
        {(hiddenAgents > 0 || showAgentHistory) && <button type="button" className="runtime-link" onClick={() => { setShowAgentHistory(!showAgentHistory); setShowAllAgents(true); }}>{showAgentHistory ? t.showActiveAgents : t.showAgentHistory}</button>}
      </div>}
      <div className="runtime-section">
        <button type="button" className="runtime-spawn-toggle" aria-expanded={spawnOpen} onClick={() => setSpawnOpen((current) => !current)}>
          <span className="runtime-spawn-label"><Plus size={13} aria-hidden="true" />{t.newReadOnlyAgent}</span>
          {spawnOpen ? <ChevronUp size={13} aria-hidden="true" /> : <ChevronDown size={13} aria-hidden="true" />}
        </button>
        {spawnOpen && <div className="runtime-spawn-form">
          <NativeSelect.Root>
            <NativeSelect.Field aria-label={t.agentProfile} value={spawnProfile} onChange={(event) => setSpawnProfile(event.target.value as AgentSpawnProfile)} fontSize="xs" h="8" bg="var(--bg-raised)" borderColor="var(--border-strong)" color="var(--text)">
              <option value="generalist">{t.agentProfileGeneralist}</option>
              <option value="reviewer">{t.agentProfileReviewer}</option>
            </NativeSelect.Field>
            <NativeSelect.Indicator />
          </NativeSelect.Root>
          <Textarea size="xs" rows={2} resize="none" value={spawnPrompt} aria-label={t.agentTask} placeholder={t.agentTaskPlaceholder} bg="var(--bg-raised)" borderColor="var(--border-strong)" color="var(--text)" _placeholder={{ color: "var(--text-ghost)" }} onChange={(event) => setSpawnPrompt(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); void spawnAgent(); } }} />
          <div className="runtime-spawn-actions">
            <Button size="2xs" disabled={!canSpawnAgent || !spawnPrompt.trim() || controlBusy} onClick={() => void spawnAgent()}>{t.spawnAgent}</Button>
          </div>
          {!canSpawnAgent && <span className="runtime-card-note">{t.activeSessionRequired}</span>}
        </div>}
      </div>
      {detail && <RuntimeDetailPanel key={detail.id} activity={activity} messages={t} target={detail} onLoadRuns={onLoadRuns} onClose={() => setDetail(null)} />}
      {activity.tools[0] && <div className="runtime-footer">
        {activity.tools[0].name} · {activity.tools[0].status === "running" ? t.toolRunning : activity.tools[0].status === "completed" ? t.toolDone : activity.tools[0].status === "partial" ? t.partial : t.toolFailed}
      </div>}
    </div>}
  </Box>;
}
