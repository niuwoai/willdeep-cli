import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ComposerReferences } from "./ComposerReferences";
import { insertReference, useComposerReferences, type Reference } from "./useComposerReferences";
import { Box, Button, Container, Dialog, Flex, Heading, Input, Portal, Text, Textarea, VStack } from "@chakra-ui/react";
import { detectLanguage, messages, type Language, type Messages } from "./i18n";
import { loadIdentity, renameIdentity, type IdentityView } from "./identityApi";
import { RuntimeSidebar, type AgentSpawnProfile, type RuntimeActivity, type RuntimeEvent } from "./RuntimeSidebar";
import { Markdown } from "./Markdown";
import { UserMessage } from "./UserMessage";
import { markdownCopy } from "./markdownCopy";
import { ConversationCard, type ConversationItem, type Plan } from "./ConversationCard";
import { SidebarSettings } from "./SidebarSettings";
import { QuickSettings } from "./QuickSettings";
import { applyThemeMode, storedThemeMode, useResolvedColorScheme, type ThemeMode } from "./theme";
import { PluginCenter } from "./PluginCenter";
import { ConfigurationSettings } from "./ConfigurationSettings";
import { BuiltinConfigurationWorkspace } from "./BuiltinConfigurationWorkspace";
import { PluginPage } from "./PluginPage";
import { ScheduleManager } from "./ScheduleManager";
import { IntakeManager } from "./IntakeManager";
import { FederationPanel } from "./FederationPanel";
import { McpPage } from "./McpAccess";
import { ConversationSchedules } from "./ConversationSchedules";
import { SessionCollaboration } from "./SessionCollaboration";
import { ChildSessions, ChildSessionOrigin } from "./ChildSessions";
import type { ChildLink } from "./childSessionsApi";
import { childSessionsText } from "./childSessionsMessages";
import { PermissionsPanel } from "./PermissionsPanel";
import { WorkspaceApprovalGuide } from "./WorkspaceApprovalGuide";
import { needsWorkspaceApproval, useWorkspaceApproval, type WorkspaceEntry } from "./useWorkspaceApproval";
import type { WorkspaceSummary as Workspace } from "./workspaceApprovalApi";
import { ModelPicker } from "./ModelPicker";
import { ReasoningPicker } from "./ReasoningPicker";
import { SharePanel } from "./SharePanel";
import { changeApprovalMode } from "./approvalModeApi";
import { changeModel, ModelFailure as ModelSwitchFailure } from "./modelApi";
import { accessLabel, isDangerousAccess, type WorkspaceAccess } from "./accessModes";
import { permissionsText } from "./permissionsMessages";
import { modelText } from "./modelMessages";
import { scheduleText } from "./scheduleMessages";
import { PluginRail, type RailSelection } from "./PluginRail";
import { PluginSidebar } from "./PluginSidebar";
import { fetchPlugins, orderedDestinations, type PluginFailureView, type PluginView } from "./plugins";
import { PluginCommandPalette, PluginMenuPopup } from "./PluginMenus";
import { menuEntries, useChatSelection, usePluginCommandRunner, type MenuEntry } from "./pluginMenuModel";
import { SfIcon } from "./sfSymbols";
import { replayToolState } from "./replayTools";
import { enqueueButtonState, queueText, runningInputFeedback } from "./queueMessages";
import { FolderGit2, ListPlus, MessageSquare, Search, GitFork, History, Download, PanelLeft, Pin, PinOff, Pencil, Archive, ArchiveRestore, X } from "lucide-react";
import { sessionTree } from "./sessionTree";
import { afterShowMore, listedSessionCount, matchesSessionQuery, orderWorkspaces, PROJECT_CAP, PROJECT_PREVIEW } from "./workspaceTree";
import "./chatWorkspace.css";
import "./runCard.css";
import { RuntimeQueue, IndependentTasks } from "./RuntimeQueue";
import { queueRequest } from "./runtimeQueueApi";
import { RuntimeHumanGates } from "./RuntimeHumanGates";
import { SentAttachments } from "./SentAttachments";
import { AttachmentManager } from "./AttachmentManager";
import { attachmentText } from "./attachmentMessages";
import { attachmentError, attachmentPayload, MAX_ATTACHMENTS, MAX_IMAGE_BASE64, MAX_CHAT_BODY_BYTES, type DraftAttachment, type AttachmentPayload } from "./attachmentModel";
import { useInputSuggestion } from "./inputSuggestion";
import { conciseHistory, interruptedReply, narrationDecision, unshownReply, usesShortStyle, type ConciseLine, type ReplyStyle } from "./conciseTurn";
import { formatStepElapsed, groupActivityMessages, needsActivityStrip, turnDividerText, type RunStep, type ThoughtRecord } from "./runActivity";
import { RunCard, ThoughtCard } from "./RunCard";
import { AgentLibrary } from "./AgentLibrary";
import { allowAgentNavigation } from "./agentNavigation";
import { agentText } from "./agentLibraryMessages";
import { RuntimeFailureDetails } from "./RuntimeFailureDetails";
import { runtimeFailureMessage, runtimeFailureReference, type RuntimeFailure } from "./runtimeErrors";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { WorkflowSidebar } from "./WorkflowSidebar";
import { WorkflowCard } from "./WorkflowCard";
import { cacheWorkflow, workflowFromEvent, type WorkflowRun } from "./workflowApi";

type Session = { id: string; title: string; preview?: string; workspace: string; created_at: number; updated_at: number; pinned_at: number | null; started_by?: string | null; parent_session_id?: string | null; delegation_read_only?: boolean; delegation_detached?: boolean; archived: boolean; active: boolean; active_turn_id: string | null };

/** 谱系行的时间按子会话的创建时间算，不用最后更新时间。 */
function lineageTime(createdAt: number, t: Messages): string {
  const elapsed = Math.max(0, Math.floor(Date.now() / 1000) - createdAt);
  if (elapsed < 60) return t.timeJustNow;
  if (elapsed < 3600) return t.timeMinutesAgo.replace("{count}", String(Math.floor(elapsed / 60)));
  if (elapsed < 86400) return t.timeHoursAgo.replace("{count}", String(Math.floor(elapsed / 3600)));
  return t.timeDaysAgo.replace("{count}", String(Math.floor(elapsed / 86400)));
}
type SessionDetail = { id: string; failure?: RuntimeFailure | null; workspace: string; parent_session_id?: string | null; delegation_read_only?: boolean; delegation_detached?: boolean; messages: ConversationItem[]; response_style?: "short" | "detailed" | null; approval_mode?: WorkspaceAccess | null; workspace_access: WorkspaceAccess; model?: string | null; provider_id?: string | null; reasoning_level?: string | null };
type RewindPoint = { step: number; turn_id: string | null; snippet: string; completed_at: number | null; can_restore_workspace: boolean };
type RewindResult = { message_count: number; dropped_turn_ids: string[]; workspace: { restored: string[]; removed: string[]; skipped: string[]; recovery_path: string | null } | null };

/// 把还在跑的那些步骤收尾，并记下各自的耗时。
function settleActiveSteps(steps: RunStep[], now: number): RunStep[] {
  return steps.map((step) => step.status === "active"
    ? { ...step, status: "done" as const, elapsedMs: step.elapsedMs ?? now - step.startedAt }
    : step);
}
type ChatMessage = { id: string; role: ConversationItem["role"] | "activity" | "divider" | "status"; content: string; steps?: RunStep[]; thinking?: string; thought?: ThoughtRecord; attachmentReference?: string; plan?: Plan; details?: string[]; concise?: ConciseLine; workflow?: WorkflowRun; model?: string };
/// 记下了模型名的消息才带这一小行；没记下的什么都不渲染，不用会话当前模型补。
function messageModelLabel(model: string | undefined, t: Messages) {
  const name = model?.trim();
  if (!name) return null;
  return <span className="message-model">{t.messageModel.replace("{model}", name)}</span>;
}

function sessionChat(detail: SessionDetail, t: Messages): ChatMessage[] {
  const chat: ChatMessage[] = [];
  conciseHistory(detail.messages, detail.response_style).forEach((message, index) => {
    const id = `${detail.id}-${index}`;
    // Missing output may be a suspended live request, not an interrupted tool.
    if (message.tools?.length) {
      const now = Date.now();
      chat.push({ id: `${id}-tools`, role: "activity", content: "", steps: message.tools.map((tool, order) => ({
        id: `${id}-tool-${order}`,
        ...replayToolState(tool.completed, tool.name, t),
        detail: tool.detail,
        startedAt: now,
      })) });
    }
    // 只带工具调用、正文为空的助手消息不再单独占一个气泡。
    if (!message.content && !message.plan && !message.workflow && !message.details?.length) return;
    chat.push({
      id, role: message.role, attachmentReference: message.attachment_reference, plan: message.plan, workflow: message.workflow, details: message.details,
      model: message.model,
      content: `${message.content}${message.attachment_count ? `\n[${message.attachment_count} ${t.attachmentCount}]` : ""}`,
    });
  });
  return chat;
}
/// 长会话只渲染最近这么多轮；更早的留在 chat 里，用户滚到顶再放回来。
const CHAT_WINDOW_ROUNDS = 50;
const CHAT_WINDOW_STEP = 50;

/** 按「轮」切出渲染窗口：分隔线就是轮次的终点。 */
function windowChat(chat: ChatMessage[], rounds: number): { visible: ChatMessage[]; hiddenRounds: number } {
  const dividers: number[] = [];
  chat.forEach((message, index) => { if (message.role === "divider") dividers.push(index); });
  if (dividers.length <= rounds) return { visible: chat, hiddenRounds: 0 };
  const hiddenRounds = dividers.length - rounds;
  // 分隔线是上一轮的收尾统计，属于被归档的那一轮；窗口从它后面开始。
  return { visible: chat.slice(dividers[hiddenRounds - 1] + 1), hiddenRounds };
}

type Attachment = DraftAttachment;
type ComposerSkill = { identifier: string; name: string; description: string };
type ComposerData = { commands: string[]; skills: ComposerSkill[] };
type Health = { status: string; version: string };
type RuntimeStreamEvent = {
  type: string;
  kind?: string;
  workflow?: WorkflowRun;
  data?: { workflow?: WorkflowRun };
  cursor?: number;
  session_id?: string;
  turn_id?: string;
  text?: string;
  /// 产出这段文字的模型名，只有事件确实带了才有。
  model?: string;
  label?: string;
  /// 这一步具体在干什么：命令的前几个词，或 Worker 的职责与标签。服务端已经
  /// 收敛并打码过，前端只负责淡色显示。
  detail?: string;
  id?: string;
  name?: string;
  is_error?: boolean;
  plan_update?: { plan: Plan; replaced: boolean } | null;
  message?: string;
  task_id?: string | null;
  reason?: string | null;
  failure_domain?: string | null;
  failure_detail?: string | null;
};
type RuntimeFailureReference = { sessionId: string; taskId: string };
const defaultCommands = ["/help", "/goal", "/permissions", "/model", "/compress", "/skills", "/clear", "/short", "/verbose", "/name"];
const lastSessionPrefix = "willdeep.web.last-session.";
const runtimeCursorPrefix = "willdeep.web.runtime-cursor.";
const goalPreferencePrefix = "willdeep.web.goal.";

type GoalPreference = { text: string; enabled: boolean };
function savedGoalPreference(sessionId: string): GoalPreference {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(`${goalPreferencePrefix}${sessionId}`) || "null");
    if (value && typeof value === "object" && "text" in value && "enabled" in value
      && typeof value.text === "string" && typeof value.enabled === "boolean") {
      return { text: value.text, enabled: value.enabled };
    }
  } catch { /* Invalid local state falls back to the default. */ }
  return { text: "", enabled: false };
}

type SkillMention = { start: number; end: number; query: string };
function skillMentionAtCursor(value: string, cursor: number): SkillMention | null {
  const before = value.slice(0, cursor);
  const match = /(?:^|\s)\$([\w-]*)$/.exec(before);
  if (!match) return null;
  const start = match.index + match[0].lastIndexOf("$");
  return { start, end: cursor, query: match[1].toLowerCase() };
}

async function json<T>(url: string, init?: RequestInit): Promise<T> {
  const response = await fetch(url, init); const value = await response.json();
  if (!response.ok) throw new Error(value.error ?? response.statusText); return value as T;
}

async function mutate(url: string, init: RequestInit) {
  const response = await fetch(url, init);
  if (!response.ok) throw new Error((await response.text()) || response.statusText);
}

function nextId(prefix: string) { return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2)}`; }

// 一级入口写进 URL hash，刷新和分享链接都能回到同一个目的地。
function railFromHash(): RailSelection {
  const hash = window.location.hash.replace(/^#/, "");
  if (hash === "agents") return { kind: "agents" };
  if (hash === "agent-experiments") return { kind: "agent-experiments" };
  if (hash === "schedules") return { kind: "schedules" };
  if (hash === "federation") return { kind: "federation" };
  if (hash === "intake") return { kind: "intake" };
  if (hash === "mcp") return { kind: "mcp" };
  if (hash === "configuration") return { kind: "configuration" };
  if (hash === "plugins") return { kind: "center" };
  const plugin = hash.startsWith("plugin/") ? hash.slice("plugin/".length) : "";
  return plugin ? { kind: "plugin", qualifiedId: decodeURIComponent(plugin) } : { kind: "conversation" };
}

function hashForRail(selection: RailSelection): string {
  if (selection.kind === "agents") return "#agents";
  if (selection.kind === "agent-experiments") return "#agent-experiments";
  if (selection.kind === "schedules") return "#schedules";
  if (selection.kind === "federation") return "#federation";
  if (selection.kind === "intake") return "#intake";
  if (selection.kind === "mcp") return "#mcp";
  if (selection.kind === "configuration") return "#configuration";
  if (selection.kind === "center") return "#plugins";
  if (selection.kind === "plugin") return `#plugin/${encodeURIComponent(selection.qualifiedId)}`;
  return "";
}

function clipboardImageFile(clipboard: DataTransfer): File | null {
  const file = Array.from(clipboard.files).find((candidate) => candidate.type.startsWith("image/"));
  if (file) return file;
  for (const item of Array.from(clipboard.items)) {
    if (item.kind !== "file" || !item.type.startsWith("image/")) continue;
    const candidate = item.getAsFile();
    if (candidate) return candidate;
  }
  return null;
}

function runtimeCursorKey(sessionId: string, turnId: string) {
  return `${runtimeCursorPrefix}${sessionId}.${turnId}`;
}

function savedRuntimeCursor(sessionId: string, turnId: string) {
  const value = Number(localStorage.getItem(runtimeCursorKey(sessionId, turnId)) ?? "0");
  return Number.isSafeInteger(value) && value >= 0 ? value : 0;
}

function saveRuntimeCursor(sessionId: string, turnId: string, cursor: number | undefined) {
  if (cursor === undefined || !Number.isSafeInteger(cursor) || cursor < 0) return;
  localStorage.setItem(runtimeCursorKey(sessionId, turnId), String(cursor));
}

function nextSseFrame(buffer: string): { frame: string; rest: string } | null {
  const lf = buffer.indexOf("\n\n");
  const crlf = buffer.indexOf("\r\n\r\n");
  if (lf < 0 && crlf < 0) return null;
  const useCrlf = crlf >= 0 && (lf < 0 || crlf < lf);
  const index = useCrlf ? crlf : lf;
  const width = useCrlf ? 4 : 2;
  return { frame: buffer.slice(0, index), rest: buffer.slice(index + width) };
}

async function readSse(response: Response, onEvent: (event: RuntimeStreamEvent) => Promise<void>) {
  if (!response.ok || !response.body) throw new Error(await response.text());
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    for (;;) {
      const next = nextSseFrame(buffer);
      if (!next) break;
      buffer = next.rest;
      const data = next.frame
        .split(/\r?\n/)
        .filter((line) => line.startsWith("data:"))
        .map((line) => line.slice(5).trimStart())
        .join("\n");
      if (data) await onEvent(JSON.parse(data) as RuntimeStreamEvent);
    }
  }
}

function waitForReconnect(delay: number, signal: AbortSignal) {
  return new Promise<void>((resolve) => {
    if (signal.aborted) { resolve(); return; }
    const timer = window.setTimeout(resolve, delay);
    signal.addEventListener("abort", () => { window.clearTimeout(timer); resolve(); }, { once: true });
  });
}

export function App() {
  const workflowClient = useQueryClient();
  const [language, setLanguage] = useState<Language>(detectLanguage); const t = messages[language];
  const [theme, setTheme] = useState<ThemeMode>(storedThemeMode);
  // 主题写在根元素上，样式表按 data-theme 取变量；跟随系统那档不写属性，交给
  // prefers-color-scheme。用 layout effect：它先于所有子组件的 effect 落地，
  // 插件页面在自己的 effect 里读 token 推主题时，拿到的已经是新配色。
  useLayoutEffect(() => { applyThemeMode(theme); }, [theme]);
  const colorScheme = useResolvedColorScheme(theme);
  const [workspace, setWorkspace] = useState("");
  const workspaceRef = useRef("");
  const workspaceDefaultRef = useRef<WorkspaceAccess>("smart");
  const workspaceDraftApprovalRef = useRef<WorkspaceAccess | null>(null);
  const workspaceEntryRevision = useRef(0);
  const initialWorkspaceRequested = useRef(false);
  const skipWorkspaceRestore = useRef(false);
  const workspaceApproval = useWorkspaceApproval(enterWorkspace);
  const workspaces = workspaceApproval.workspaces;
  const [sessions, setSessions] = useState<Session[]>([]); const [sessionId, setSessionId] = useState("");
  const [sessionRelation, setSessionRelation] = useState<{ id: string; parent: string | null; readOnly: boolean; detached: boolean } | null>(null);
  const deepLinkSession = useRef(new URLSearchParams(window.location.search).get("session"));
  const startupSessions = useQuery({ queryKey: ["sessions", "workspace-entry"], queryFn: () => json<Session[]>("/api/sessions"), enabled: Boolean(deepLinkSession.current) && !initialWorkspaceRequested.current });
  // 项目树的展开与每个项目已加载的条数。默认只展开最近更新的项目，其余收起。
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(new Set());
  const [projectShown, setProjectShown] = useState<Record<string, number>>({});
  const treeInitialized = useRef(false);
  const [runtimeEvents, setRuntimeEvents] = useState<RuntimeEvent[]>([]);
  // 轮询闭包里读得到当前会话，而不必把 sessionId 放进依赖数组重开定时器。
  const sessionIdRef = useRef(sessionId);
  const pendingWorkspaceSession = useRef<{ id: string; workspace: string; turnId: string | null } | null>(null);
  const displayRevisionRef = useRef("");
  useEffect(() => { sessionIdRef.current = sessionId; }, [sessionId]);
  const [sessionSearch, setSessionSearch] = useState("");
  const [mobileSessionsOpen, setMobileSessionsOpen] = useState(false);
  const [skillSearch, setSkillSearch] = useState("");
  const [version, setVersion] = useState("");
  // 这台实例的名字与 UUID。只在回环 Web 上读得到；读到之前先不渲染，免得闪一下占位。
  const [identity, setIdentity] = useState<IdentityView | null>(null);
  const [chat, setChat] = useState<ChatMessage[]>([]); const [prompt, setPrompt] = useState("");
  // 轮次收尾时要从运行卡片里数工具步，而会话重载会把卡片整个换掉，先照一份镜像。
  const chatRef = useRef<ChatMessage[]>([]);
  useEffect(() => { chatRef.current = chat; }, [chat]);
  // 这条会话的回复风格。缺省即简洁，与提示词侧同一条判定；切它只从下一轮起生效。
  const replyStyleRef = useRef<ReplyStyle>(null);
  // 这条会话里选过的审批档位；`null` 表示跟随工作区策略。档位只存在 Runtime
  // 会话上，核心会话文件里没有，所以只能从会话详情带回来。
  const [approvalMode, setApprovalMode] = useState<WorkspaceAccess | null>(null);
  // 工作区策略，是会话档位的硬上限。以服务端会话详情里的那份为准——那正是
  // 后端在合并档位时用的上限，比从工作区列表自己推更可靠。
  const [workspacePolicy, setWorkspacePolicy] = useState<WorkspaceAccess>("smart");
  // 审批面板由 App 持有开合状态：`/permissions` 命令也要能把面板摊开。
  const [permissionsOpen, setPermissionsOpen] = useState(false);
  // 会话当前模型（`None` 表示跟随 Provider 默认）与 `/model` 选择器的开合。
  const [sessionModel, setSessionModel] = useState<string | null>(null);
  const [sessionModelProviderId, setSessionModelProviderId] = useState<string | null>(null);
  const [sessionReasoningLevel, setSessionReasoningLevel] = useState<string | null>(null);
  // 还没建会话时先选好的模型。它不是会话上的设置，只是第一条消息的草稿：
  // 发出去时随请求一起交给服务端，建会话时一次写进去。
  const [draftModel, setDraftModel] = useState<{ model: string; providerId: string } | null>(null);
  const [modelOpen, setModelOpen] = useState(false);
  // 分享面板同理：工具栏一个按钮，开合状态留在 App 里，换会话时收起来。
  const [shareOpen, setShareOpen] = useState(false);
  // 模型和审批档位选完就把弹窗收起来，成功反馈改由这里常驻几秒——
  // 提示若只画在面板里，面板一关就再也看不见，用户会以为没切换成功。
  const [selectionNotice, setSelectionNotice] = useState<string | null>(null);
  const selectionNoticeTimer = useRef<number | undefined>(undefined);
  const showSelectionNotice = useCallback((text: string) => {
    window.clearTimeout(selectionNoticeTimer.current);
    setSelectionNotice(text);
    selectionNoticeTimer.current = window.setTimeout(() => setSelectionNotice(null), 4000);
  }, []);
  useEffect(() => () => window.clearTimeout(selectionNoticeTimer.current), []);
  // 简洁模式还没落进聊天区的最近一段中途文字。轮次收尾时据此去重，中断时补写。
  const conciseLineRef = useRef<ConciseLine | null>(null);
  const [conciseNow, setConciseNow] = useState(() => Date.now());
  const [attachments, updateAttachments] = useState<Attachment[]>([]);
  const attachmentsRef = useRef<Attachment[]>([]);
  const setAttachments = (value: Attachment[] | ((current: Attachment[]) => Attachment[])) => {
    const next = typeof value === "function" ? value(attachmentsRef.current) : value;
    attachmentsRef.current = next; updateAttachments(next);
  };
  const appendAttachment = (payload: AttachmentPayload) => {
    const reason = attachmentError(payload);
    if (reason) { setError(attachmentText(reason, language)); return; }
    if (attachmentsRef.current.length >= MAX_ATTACHMENTS) { setError(attachmentText("limit", language)); return; }
    setAttachments([...attachmentsRef.current, { ...payload, id: crypto.randomUUID() }]);
  };
  const [goal, setGoal] = useState("");
  const [goalEnabled, setGoalEnabled] = useState(false);
  const pendingGoalPreference = useRef<GoalPreference | null>(null);
  function updateGoalPreference(text: string, enabled: boolean) {
    const preference = { text, enabled };
    setGoal(text);
    setGoalEnabled(enabled);
    if (sessionId) localStorage.setItem(`${goalPreferencePrefix}${sessionId}`, JSON.stringify(preference));
    else pendingGoalPreference.current = preference;
  }
  const [composerCursor, setComposerCursor] = useState(0);
  const [skillSelected, setSkillSelected] = useState(0);
  const [skillPickerDismissed, setSkillPickerDismissed] = useState(false);
  const [composer, setComposer] = useState<ComposerData>({ commands: defaultCommands, skills: [] });
  const [error, setError] = useState("");
  const [runtimeFailure, setRuntimeFailure] = useState<RuntimeFailureReference | null>(null);
  const references = useComposerReferences(workspace, prompt, t.loadFailed, setError);
  const promptRef = useRef<HTMLTextAreaElement>(null);
  function chooseReference(entry: Reference) {
    if (!references.query) return;
    const cursor = references.query.start + entry.value.length + 1;
    setPrompt(insertReference(prompt, references.query, entry.value));
    setComposerCursor(cursor);
    setSkillPickerDismissed(false);
    references.dismiss();
    requestAnimationFrame(() => { promptRef.current?.focus(); promptRef.current?.setSelectionRange(cursor, cursor); });
  }
  const [runtimeActivity, setRuntimeActivity] = useState<RuntimeActivity>({ tools: [], artifacts: [], agents: [], tasks: [], gates: [], attention_count: 0 });
  const visibleHumanGates = runtimeActivity.gates.filter((gate) => runtimeActivity.tasks.some((task) =>
    task.id === gate.task_id && (task.session_id === null || task.session_id === sessionId)));
  const [busy, setBusy] = useState(false); const [activity, setActivity] = useState("");
  const draftSendRequest = useRef<{ fingerprint: string; id: string } | null>(null);
  const queueSubmitting = useRef(false);
  const [submissionNotice, setSubmissionNotice] = useState<{ sessionId: string; text: string } | null>(null);
  useEffect(() => { setSubmissionNotice(null); }, [sessionId]);
  const inputSuggestion = useInputSuggestion();
  // 预测回包时要看「此刻」输入框空不空、有没有新一轮，闭包里的值是发起时的旧值。
  const composerEmptyRef = useRef(true);
  const composerIdleRef = useRef(true);
  useEffect(() => {
    composerEmptyRef.current = !prompt && !attachments.length;
    composerIdleRef.current = composerEmptyRef.current && !busy;
  }, [prompt, attachments.length, busy]);
  const abortRef = useRef<AbortController | null>(null); const endRef = useRef<HTMLDivElement | null>(null);
  const activeRunRef = useRef<string | null>(null);
  const activeTurnRef = useRef<string | null>(null);
  const activeSessionRef = useRef<string | null>(null);
  const [activeRuntimeSessionId, setActiveRuntimeSessionId] = useState("");
  async function enqueueDraft(content = prompt.trim() || t.attachmentPrompt) {
    const target = activeSessionRef.current || sessionId;
    // 没有会话就无处可入。不为了入队隐式建一个空会话：那会在历史列表里
    // 留下一条谁也没发过言的记录。
    if (!target) { setError(queueText("enqueueNeedsSession", language)); return; }
    if (queueSubmitting.current || (!content && !attachmentsRef.current.length)) return;
    const outgoingPrompt = prompt;
    const outgoingAttachments = attachmentsRef.current;
    // 空闲时入队要暂缓，否则服务端会在会话空闲的一瞬间把它领走，等于立即发送。
    // 忙时不暂缓：这一轮本来就占着会话，消息按原顺序等它结束。
    const payload = { prompt: content, session_id: target, workspace, language, attachments: outgoingAttachments.map(attachmentPayload), defer_until_released: !busy };
    const fingerprint = JSON.stringify(payload);
    if (draftSendRequest.current?.fingerprint !== fingerprint) draftSendRequest.current = { fingerprint, id: crypto.randomUUID() };
    const body = JSON.stringify({ ...payload, request_id: draftSendRequest.current.id });
    if (new TextEncoder().encode(body).length > MAX_CHAT_BODY_BYTES) { setError(attachmentText("batchTooLarge", language)); return; }
    queueSubmitting.current = true;
    const running = busy || selectedSession?.active || activeRuntimeSessionId === target;
    const progress = running || visibleHumanGates.length ? runningInputFeedback(runtimeActivity, target, language).text : "";
    setSubmissionNotice({ sessionId: target, text: [progress, queueText("sending", language)].filter(Boolean).join(" · ") });
    try {
      await queueRequest(`/api/sessions/${encodeURIComponent(target)}/queue`, { method: "POST", headers: { "content-type": "application/json" }, body });
      draftSendRequest.current = null;
      setPrompt((current) => current === outgoingPrompt ? "" : current);
      if (attachmentsRef.current === outgoingAttachments) setAttachments([]);
      setSubmissionNotice({ sessionId: target, text: [progress, queueText("saved", language)].filter(Boolean).join(" · ") });
      setError("");
    } catch (reason) {
      setSubmissionNotice({ sessionId: target, text: [progress, queueText("failed", language)].filter(Boolean).join(" · ") });
      setError(`${t.requestFailed}: ${reason instanceof Error ? reason.message : String(reason)}`);
    } finally { queueSubmitting.current = false; }
  }

  const pendingStopRef = useRef(false);
  const loadSessionRef = useRef<(id: string, activeTurnId?: string | null) => Promise<void>>(async () => undefined);
  const chatViewportRef = useRef<HTMLDivElement | null>(null); const followBottomRef = useRef(true);
  // 只影响渲染的窗口大小；会话数据仍是完整的一份。
  const [chatRounds, setChatRounds] = useState(CHAT_WINDOW_ROUNDS);
  const chatSessionRef = useRef<string | null>(null);
  const pendingChatScroll = useRef<number | null>(null);
  const loadEarlierRef = useRef(false);
  const dividerCountRef = useRef(0);
  const chatWindow = useMemo(() => windowChat(chat, chatRounds), [chat, chatRounds]);
  const visibleChat = chatWindow.visible;
  const hiddenRounds = chatWindow.hiddenRounds;
  const groupedChat = useMemo(() => groupActivityMessages(visibleChat), [visibleChat]);
  function loadEarlierChat() {
    if (loadEarlierRef.current) return;
    loadEarlierRef.current = true;
    const node = chatViewportRef.current;
    if (node) pendingChatScroll.current = node.scrollHeight - node.scrollTop;
    setChatRounds((rounds) => rounds + CHAT_WINDOW_STEP);
  }
  // 放回更早的轮次后把视野钉在原处，别把人甩到底部。
  useLayoutEffect(() => {
    if (pendingChatScroll.current === null) return;
    const node = chatViewportRef.current;
    if (node) node.scrollTop = node.scrollHeight - pendingChatScroll.current;
    pendingChatScroll.current = null;
    loadEarlierRef.current = false;
  }, [visibleChat]);
  const [addingWorkspace, setAddingWorkspace] = useState(false);
  const [newWorkspacePath, setNewWorkspacePath] = useState("");
  const [workspaceError, setWorkspaceError] = useState("");
  const [plugins, setPlugins] = useState<PluginView[]>([]);
  const [pluginFailures, setPluginFailures] = useState<PluginFailureView[]>([]);
  const [rail, setRail] = useState<RailSelection>(railFromHash);
  const scheduleDirtyRef = useRef(false);
  const agentDirtyRef = useRef(false);
  const trackAgentDirty = useCallback((dirty: boolean) => {agentDirtyRef.current = dirty;}, []);
  const [pluginSelectedItem, setPluginSelectedItem] = useState<string | null>(null);
  const [pluginReloadToken, setPluginReloadToken] = useState(0);
  const [setupLoadFailed, setSetupLoadFailed] = useState(false);
  const [showSetup, setShowSetup] = useState(new URLSearchParams(window.location.search).get("setup") === "1");
  const checkSetup = useCallback(async () => {
    const value = await json<{ ready: boolean }>("/api/setup");
    setSetupLoadFailed(false);
    return value;
  }, []);
  useEffect(() => {
    checkSetup().then((value) => { if (!value.ready) setShowSetup(true); }).catch(() => setSetupLoadFailed(true));
  }, [checkSetup]);
  const finishSetup = () => {
    setShowSetup(false);
    setRail({ kind: "conversation" });
    const url = new URL(window.location.href);
    url.searchParams.delete("setup");
    window.history.replaceState(null, "", `${url.pathname}${url.search}${url.hash}`);
  };

  const refreshPlugins = useCallback(() => {
    fetchPlugins(language)
      .then((value) => { setPlugins(value.plugins); setPluginFailures(value.failures); })
      // 插件宿主没起来不该让聊天界面报错——那是两件独立的事。
      .catch(() => { setPlugins([]); setPluginFailures([]); });
  }, [language]);
  useEffect(() => { refreshPlugins(); }, [refreshPlugins, pluginReloadToken]);

  const pluginEntries = useMemo(() => orderedDestinations(plugins), [plugins]);
  const activePlugin = useMemo(
    () => (rail.kind === "plugin" ? pluginEntries.find((entry) => entry.destination.qualified_id === rail.qualifiedId) : undefined),
    [rail, pluginEntries]
  );
  // 选中的插件被停用或卸载时回到对话，而不是停在一个已经不存在的目的地上。
  // 插件清单还没加载完时先别动——否则从一个 #plugin/... 链接进来会被立刻踢走。
  useEffect(() => {
    if (rail.kind === "plugin" && plugins.length > 0 && !activePlugin) setRail({ kind: showSetup ? "center" : "conversation" });
  }, [rail, activePlugin, plugins.length, showSetup]);

  useEffect(() => {
    const target = hashForRail(rail);
    if (window.location.hash !== target) {
      window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}${target}`);
    }
  }, [rail]);
  useEffect(() => {
    const onHashChange = () => {
      const next = railFromHash();
      if (hashForRail(next) !== hashForRail(rail) && !allowAgentNavigation(agentDirtyRef.current, language)) {
        window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}${hashForRail(rail)}`); return;
      }
      setRail(next);
    };
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, [rail, language]);

  const navigateToDestination = useCallback((qualifiedId: string) => {
    setPluginSelectedItem(null);
    setRail({ kind: "plugin", qualifiedId });
  }, []);

  // 插件的五个菜单贡献点。目的地是主入口，这些是顺手入口——收藏夹与待办
  // 真正的用法是「聊天里选中一句就记下」，而不是先切目的地再手打一遍。
  const [showPalette, setShowPalette] = useState(false);
  const [popup, setPopup] = useState<{ entries: MenuEntry[]; x: number; y: number; args: Record<string, string> } | null>(null);
  const [pluginMenuError, setPluginMenuError] = useState("");
  const runPluginCommand = usePluginCommandRunner(navigateToDestination);
  const paletteEntries = useMemo(() => menuEntries(plugins, "commandPalette"), [plugins]);
  const chatSelectionEntries = useMemo(() => menuEntries(plugins, "chat.selection"), [plugins]);
  const sessionContextEntries = useMemo(() => menuEntries(plugins, "session.context"), [plugins]);
  const composerEntries = useMemo(() => menuEntries(plugins, "composer.more"), [plugins]);
  const [chatSelection, setChatSelection] = useChatSelection(chatViewportRef, chatSelectionEntries.length > 0);
  useEffect(() => { setChatSelection(null); }, [sessionId, rail, setChatSelection]);

  const runMenuEntry = useCallback(async (entry: MenuEntry, args: Record<string, string> = {}) => {
    setPopup(null);
    setShowPalette(false);
    setChatSelection(null);
    setPluginMenuError("");
    try {
      await runPluginCommand(entry, args);
      // 命令可能改了插件自己的数据，让当前目的地的侧栏与页面重取一次。
      setPluginReloadToken((current) => current + 1);
    } catch (reason) {
      setPluginMenuError(reason instanceof Error ? reason.message : String(reason));
    }
  }, [runPluginCommand, setChatSelection]);

  useEffect(() => {
    if (!paletteEntries.length) return;
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setShowPalette((current) => !current);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [paletteEntries.length]);

  const pluginOverlays = <>
    {showPalette && <PluginCommandPalette entries={paletteEntries} messages={t} onRun={(entry) => void runMenuEntry(entry)} onClose={() => setShowPalette(false)} />}
    {popup && <PluginMenuPopup entries={popup.entries} x={popup.x} y={popup.y} onRun={(entry) => void runMenuEntry(entry, popup.args)} onClose={() => setPopup(null)} />}
    {pluginMenuError && <Text className="plugin-error" position="fixed" bottom="3" right="3" zIndex="50" borderRadius="8px" onClick={() => setPluginMenuError("")}>{t.pluginCommandFailed}: {pluginMenuError}</Text>}
  </>;

  useEffect(() => { document.title = t.documentTitle; document.documentElement.lang = language; localStorage.setItem("willdeep.language", language); }, [language, t.documentTitle]);
  useEffect(() => { json<Health>("/health").then((value) => setVersion(value.version)).catch(() => setVersion("")); }, []);
  useEffect(() => { loadIdentity().then(setIdentity).catch(() => setIdentity(null)); }, []);
  useEffect(() => {
    const reason = workspaceApproval.query.error ?? startupSessions.error;
    if (reason) setError(`${t.loadFailed}: ${reason.message}`);
  }, [workspaceApproval.query.error, startupSessions.error, t.loadFailed]);
  useEffect(() => {
    if (initialWorkspaceRequested.current || !workspaceApproval.query.isSuccess) return;
    const linkedId = deepLinkSession.current;
    if (linkedId && startupSessions.isPending) return;
    const linked = startupSessions.data?.find((item) => item.id === linkedId);
    const target = workspaces.find((item) => item.path === linked?.workspace) ?? workspaces[0];
    if (!target) return;
    initialWorkspaceRequested.current = true;
    deepLinkSession.current = null;
    workspaceApproval.requestEntry(target, linked ? { session: { id: linked.id, turnId: linked.active_turn_id } } : {});
  }, [workspaces, workspaceApproval, startupSessions.data, startupSessions.isPending]);
  useEffect(() => {
    if (!workspace) return;
    let active = true;
    const revision = workspaceEntryRevision.current;
    const skipRestore = skipWorkspaceRestore.current;
    skipWorkspaceRestore.current = false;
    abortRef.current?.abort();
    sessionIdRef.current = "";
    setSessionId("");
    setChat([]);
    // 新会话使用此目录已选的审批值；恢复后再取会话自身的覆盖值。
    setApprovalMode(workspaceDraftApprovalRef.current);
    setWorkspacePolicy(workspaceDefaultRef.current);
    setPermissionsOpen(false);
    setSessionModel(null); setSessionModelProviderId(null); setSessionReasoningLevel(null);
    setDraftModel(null);
    setModelOpen(false);
    setShareOpen(false);
    setGoal("");
    setGoalEnabled(false);
    pendingGoalPreference.current = null;
    setRuntimeFailure(null);
    setShowArchived(false);
    json<Session[]>("/api/sessions").then((values) => {
      if (!active || revision !== workspaceEntryRevision.current) return;
      setSessions(values);
      if (skipRestore) return;
      const available = values.filter((item) => item.workspace === workspace && !item.archived);
      const remembered = localStorage.getItem(`${lastSessionPrefix}${workspace}`);
      const pending = pendingWorkspaceSession.current;
      const candidate = pending?.workspace === workspace ? values.find((item) => item.id === pending.id) : available.find((item) => item.id === remembered) ?? available.find((item) => item.active);
      if (pending?.workspace === workspace) pendingWorkspaceSession.current = null;
      if (candidate) void loadSessionRef.current(candidate.id, pending?.workspace === workspace ? pending.turnId ?? candidate.active_turn_id : candidate.active_turn_id);
    }).catch((reason: Error) => { if (active) setError(`${t.loadFailed}: ${reason.message}`); });
    json<ComposerData>(`/api/composer?workspace=${encodeURIComponent(workspace)}`).then((value) => { if (active) setComposer(value); }).catch((reason: Error) => { if (active) setError(`${t.loadFailed}: ${reason.message}`); });
    return () => { active = false; };
  }, [workspace, t.loadFailed]);
  useEffect(() => {
    if (!workspace) { setRuntimeActivity({ tools: [], artifacts: [], agents: [], tasks: [], gates: [], attention_count: 0 }); return; }
    let active = true;
    // 上一轮没回来就跳过这一轮，避免慢响应时请求堆积、把服务端 CPU 顶满。
    let inFlight = false;
    const refresh = () => {
      if (inFlight) return Promise.resolve();
      inFlight = true;
      const requestedSession = sessionIdRef.current;
      // 事件按会话取，且只在选中会话时取：浏览器端没有应用层鉴权，跨会话
      // 列表没有归属可校验。
      const eventsRequest = sessionIdRef.current
        ? json<RuntimeEvent[]>(`/api/runtime/events?session=${encodeURIComponent(sessionIdRef.current)}`).catch(() => [] as RuntimeEvent[])
        : Promise.resolve([] as RuntimeEvent[]);
      return Promise.all([json<RuntimeActivity>(`/api/runtime/activity?workspace=${encodeURIComponent(workspace)}${sessionIdRef.current ? `&session=${encodeURIComponent(sessionIdRef.current)}` : ""}`), json<Session[]>("/api/sessions"), eventsRequest])
        .then(async ([runtime, currentSessions, events]) => {
          if (!active || requestedSession !== sessionIdRef.current) return;
          setRuntimeActivity(runtime); setSessions(currentSessions); setRuntimeEvents(events);
          const visible = currentSessions.find((item) => item.id === sessionIdRef.current);
          const revision = visible ? `${visible.id}:${visible.updated_at}:${t.attachmentCount}` : "";
          if (!visible || activeRunRef.current || displayRevisionRef.current === revision) return;
          const detail = await json<SessionDetail>(`/api/sessions/${encodeURIComponent(visible.id)}`);
          if (active && sessionIdRef.current === visible.id && !activeRunRef.current) {
            displayRevisionRef.current = revision;
            setChat(sessionChat(detail, t));
          }
        })
        .catch(() => undefined)
        .finally(() => { inFlight = false; });
    };
    void refresh(); const timer = window.setInterval(refresh, 2000);
    return () => { active = false; window.clearInterval(timer); };
  }, [workspace, t]);
  useEffect(() => { if (followBottomRef.current) endRef.current?.scrollIntoView({ behavior: "smooth", block: "end" }); }, [chat, activity]);
  // 状态行的已运行时长每秒跳一格。没有状态行时不转定时器。
  useEffect(() => {
    if (!chat.some((message) => message.role === "status")) return undefined;
    const timer = window.setInterval(() => setConciseNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [chat]);

  const [showArchived, setShowArchived] = useState(false);
  const sessionQuery = sessionSearch.trim().toLowerCase();
  const searching = sessionQuery.length > 0;
  // 搜索跨所有项目。不搜索时每个项目只露已加载的那几条，置顶的排在前面。
  const projectSessions = useMemo(() => {
    const grouped = new Map<string, Session[]>();
    for (const item of sessions) {
      if (item.archived) continue;
      const group = grouped.get(item.workspace) ?? [];
      group.push(item);
      grouped.set(item.workspace, group);
    }
    for (const group of grouped.values()) group.sort((a, b) => (b.pinned_at ?? 0) - (a.pinned_at ?? 0) || b.updated_at - a.updated_at);
    return grouped;
  }, [sessions]);
  const projectLatest = useMemo(() => {
    const latest = new Map<string, number>();
    for (const item of sessions) latest.set(item.workspace, Math.max(latest.get(item.workspace) ?? 0, item.updated_at));
    return latest;
  }, [sessions]);
  const orderedProjects = useMemo(
    () => orderWorkspaces(workspaces.map((item) => ({ ...item, latest: projectLatest.get(item.path) ?? 0 }))),
    [workspaces, projectLatest],
  );
  // 第一次拿到项目名单时只展开最近更新的那个。之后新加进来的项目保持收起。
  useEffect(() => {
    if (treeInitialized.current || !orderedProjects.length) return;
    treeInitialized.current = true;
    setExpandedProjects(new Set([orderedProjects[0].path]));
  }, [orderedProjects]);
  const archivedSessions = useMemo(() => {
    return sessions
      .filter((item) => item.archived && item.workspace === workspace && matchesSessionQuery(item, sessionQuery))
      .sort((a, b) => (b.pinned_at ?? 0) - (a.pinned_at ?? 0) || b.updated_at - a.updated_at);
  }, [sessions, workspace, sessionQuery]);
  const selectedSession = useMemo(() => sessions.find((item) => item.id === sessionId), [sessions, sessionId]);
  const childParentId = selectedSession?.parent_session_id ?? (sessionRelation?.id === sessionId ? sessionRelation.parent : null);
  const childReadOnly = selectedSession?.delegation_read_only ?? (sessionRelation?.id === sessionId && sessionRelation.readOnly);
  const childDetached = selectedSession?.delegation_detached ?? (sessionRelation?.id === sessionId && sessionRelation.detached);
  const ghostSuggestion = inputSuggestion.suggestion && inputSuggestion.suggestion.sessionId === sessionId && !prompt && !attachments.length && !busy ? inputSuggestion.suggestion.text : null;
  const enqueueButton = enqueueButtonState({ hasDraft: Boolean(prompt.trim() || attachments.length), hasSession: Boolean(sessionId) });
  const commandMatches = useMemo(() => prompt.startsWith("/") ? composer.commands.filter((item) => item.startsWith(prompt.split(/\s/)[0])).slice(0, 6) : [], [prompt, composer.commands]);
  const skillMention = skillMentionAtCursor(prompt, composerCursor);
  const skillQuery = skillMention?.query;
  const skillMatches = useMemo(() => {
    if (skillQuery === undefined) return [];
    const query = skillSearch.trim().toLowerCase() || skillQuery;
    return composer.skills.filter((skill) => `${skill.identifier} ${skill.name} ${skill.description}`.toLowerCase().includes(query)).slice(0, 8);
  }, [skillQuery, skillSearch, composer.skills]);
  useEffect(() => { setSkillSelected(0); setSkillSearch(""); setSkillPickerDismissed(false); }, [skillQuery]);
  useEffect(() => { setSkillSelected((current) => Math.min(current, Math.max(0, skillMatches.length - 1))); }, [skillMatches.length]);

  function chooseSkill(skill: ComposerSkill) {
    if (!skillMention) return;
    const insertion = `$${skill.identifier} `;
    const cursor = skillMention.start + insertion.length;
    setPrompt(`${prompt.slice(0, skillMention.start)}${insertion}${prompt.slice(skillMention.end)}`);
    setComposerCursor(cursor);
    setSkillPickerDismissed(true);
    requestAnimationFrame(() => { promptRef.current?.focus(); promptRef.current?.setSelectionRange(cursor, cursor); });
  }

  function updateRun(runId: string, updater: (steps: RunStep[]) => RunStep[]) {
    setChat((current) => current.map((message) => message.id === runId ? { ...message, steps: updater(message.steps ?? []) } : message));
  }

  /// 「── 本轮结束 · 总耗时 · 工具 N 次 ──」：与 TUI 同一条分隔线。会话重载
  /// 后运行卡片就没了，所以在重载前算好，重载后再追加。「轮到你」不写进内容，
  /// 只在它还是最后一条时由渲染补上，新输出一来就不再显示。
  function turnDivider(runId: string, startedAt: number | null): ChatMessage {
    const steps = chatRef.current.find((message) => message.id === runId)?.steps ?? [];
    const tools = steps.filter((step) => !step.id.startsWith("turn-"));
    const failed = tools.filter((step) => step.status === "failed").length;
    const parts = [t.turnFinished];
    if (startedAt !== null) parts.push(`${t.totalElapsed} ${formatStepElapsed(Date.now() - startedAt)}`);
    if (tools.length) parts.push(`${t.toolCount.replace("{count}", String(tools.length))}${failed ? t.foldedFailed.replace("{count}", String(failed)) : ""}`);
    return { id: nextId("divider"), role: "divider", content: `── ${parts.join(" · ")} ──` };
  }

  function applySessionDetail(detail: SessionDetail) {
    setError(detail.failure ? runtimeFailureMessage(detail.failure, t) : "");
    setRuntimeFailure(detail.failure ? runtimeFailureReference(detail.failure, detail.id) : null);
    setSessionRelation({ id: detail.id, parent: detail.parent_session_id ?? null, readOnly: detail.delegation_read_only ?? false, detached: detail.delegation_detached ?? false });
    replyStyleRef.current = detail.response_style ?? null;
    setApprovalMode(detail.approval_mode ?? null);
    setWorkspacePolicy(detail.workspace_access);
    setSessionModel(detail.model ?? null);
    setSessionModelProviderId(detail.provider_id ?? null);
    setSessionReasoningLevel(detail.reasoning_level ?? null);
    // 会话已经存在，草稿里的选择要么已经被采纳，要么不再相关。
    setDraftModel(null);
    conciseLineRef.current = null;
    sessionIdRef.current = detail.id;
    setSessionId(detail.id);
    pendingGoalPreference.current = null;
    const goalPreference = savedGoalPreference(detail.id);
    setGoal(goalPreference.text);
    setGoalEnabled(goalPreference.enabled);
    // 换会话才重置窗口；同一个会话的轮询刷新不能把用户展开的历史收回去。
    if (chatSessionRef.current !== detail.id) {
      chatSessionRef.current = detail.id;
      setChatRounds(CHAT_WINDOW_ROUNDS);
      pendingChatScroll.current = null;
      loadEarlierRef.current = false;
      dividerCountRef.current = 0;
    }
    const next = sessionChat(detail, t);
    // 正翻着旧轮次时又完成了一轮：把窗口同步撑大，别让顶上正在看的轮次被抽走。
    const dividers = next.reduce((count, message) => count + (message.role === "divider" ? 1 : 0), 0);
    const gained = Math.max(0, dividers - dividerCountRef.current);
    dividerCountRef.current = dividers;
    if (gained > 0 && hiddenRounds > 0) setChatRounds((rounds) => rounds + gained);
    localStorage.setItem(`${lastSessionPrefix}${workspace}`, detail.id);
    setChat(next);
    followBottomRef.current = true;
  }

  async function applyStreamEvent(event: RuntimeStreamEvent, runId: string) {
    const workflow = workflowFromEvent(event);
    if (workflow) {
      await cacheWorkflow(workflowClient, workflow);
      if (workflow.parent_session_id === sessionIdRef.current || workflow.parent_session_id === activeSessionRef.current) {
        setChat((current) => current.some((item) => item.workflow?.id === workflow.id)
          ? current.map((item) => item.workflow?.id === workflow.id ? { ...item, workflow } : item)
          : [...current, { id: `workflow-${workflow.id}`, role: "workflow", content: "", workflow }]);
      }
      const turnId = event.turn_id || activeTurnRef.current;
      if (turnId) saveRuntimeCursor(workflow.parent_session_id, turnId, event.cursor);
      return { terminal: false as const };
    }
    if (event.type === "submitted" || event.type === "resumed") {
      const nextSessionId = event.session_id || activeSessionRef.current || "";
      const nextTurnId = event.turn_id || activeTurnRef.current;
      setSessionId(nextSessionId);
      if (event.type === "submitted" && nextSessionId) {
        const preference = pendingGoalPreference.current ?? { text: goal, enabled: goalEnabled };
        localStorage.setItem(`${goalPreferencePrefix}${nextSessionId}`, JSON.stringify(preference));
        pendingGoalPreference.current = null;
      }
      setActiveRuntimeSessionId(nextSessionId);
      activeSessionRef.current = nextSessionId || null;
      activeTurnRef.current = nextTurnId || null;
      if (nextSessionId) localStorage.setItem(`${lastSessionPrefix}${workspace}`, nextSessionId);
      if (nextSessionId && nextTurnId) saveRuntimeCursor(nextSessionId, nextTurnId, event.cursor);
      if (event.type === "submitted" && pendingStopRef.current && activeTurnRef.current) await stopRemoteTurn(activeTurnRef.current);
      return { terminal: false as const };
    }
    const currentSessionId = event.session_id || activeSessionRef.current;
    const currentTurnId = event.turn_id || activeTurnRef.current;
    if (currentSessionId && currentTurnId) saveRuntimeCursor(currentSessionId, currentTurnId, event.cursor);
    if (event.type === "completed" || event.type === "partial") {
      setChat((current) => current.map((message) => {
        if (message.id !== runId) return message;
        const now = Date.now();
        const thought = message.thought && !message.thought.elapsedMs
          ? { ...message.thought, elapsedMs: now - message.thought.startedAt }
          : message.thought;
        return { ...message, content: "", thinking: "", thought };
      }));
      setActiveRuntimeSessionId("");
      activeSessionRef.current = null;
      activeTurnRef.current = null;
      return { terminal: true as const, text: event.text || t.emptyReply };
    }
    if (event.type === "error") {
      setRuntimeFailure(currentSessionId ? runtimeFailureReference(event, currentSessionId) : null);
      return { terminal: true as const, error: runtimeFailureMessage(event, t) };
    }
    if (event.type === "assistant_text_delta") {
      // 正文一开始，思维链尾巴就隐藏。模型名只在事件确实带来时才记下，
      // 不拿会话当前的模型填。
      setChat((current) => current.map((message) => {
        if (message.id !== runId) return message;
        const now = Date.now();
        const thought = message.thought && !message.thought.elapsedMs
          ? { ...message.thought, elapsedMs: now - message.thought.startedAt }
          : message.thought;
        const model = event.model?.trim() ? event.model.trim() : message.model;
        return { ...message, content: message.content + (event.text || ""), thinking: "", thought, model };
      }));
    }
    else if (event.type === "reasoning_delta") {
      setActivity(t.thinking);
      setChat((current) => current.map((message) => {
        if (message.id !== runId) return message;
        const now = Date.now();
        const thought = message.thought ?? { text: "", startedAt: now };
        return { ...message, thinking: "", thought: { ...thought, text: `${thought.text}${event.text || ""}` } };
      }));
    }
    else if (["provider_retry_wait", "subagent_retry_wait", "provider_retry_started", "subagent_retry_started", "runtime_handoff_wait"].includes(event.type)) setActivity(event.label || t.thinking);
    else if (event.type === "thought") {
      const text = (event.text || "").trim();
      // 计划围栏两种模式都原位更新卡片，不进状态行，也不落成一段正文。
      if (narrationDecision(replyStyleRef.current, text) === "hold") {
        const startedAt = conciseLineRef.current?.startedAt ?? Date.now();
        conciseLineRef.current = { text, startedAt };
        setChat((current) => {
          const without = current.filter((message) => message.role !== "status");
          return [...without, { id: "concise-status", role: "status", content: "", concise: { text, startedAt } }];
        });
      } else if (narrationDecision(replyStyleRef.current, text) === "append") {
        const model = event.model?.trim() || undefined;
        setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: text, model }]);
      }
      setActivity(event.text || t.thinking);
    }
    else if (event.type === "turn_started") {
      setChat((current) => current.map((message) => {
        if (message.id !== runId) return message;
        const now = Date.now();
        const thought = message.thought && !message.thought.elapsedMs
          ? { ...message.thought, elapsedMs: now - message.thought.startedAt }
          : message.thought;
        return { ...message, content: "", thinking: "", thought };
      }));
      setActivity(event.label || t.thinking);
      const stepId = event.id || `turn-${event.cursor ?? nextId("turn")}`;
      updateRun(runId, (steps) => {
        const now = Date.now();
        const updated = settleActiveSteps(steps, now);
        return updated.some((step) => step.id === stepId) ? updated : [...updated, { id: stepId, label: event.label || t.thinking, status: "active", startedAt: now }];
      });
    }
    else if (event.type === "tool_requested") {
      setActivity(event.label || t.toolRunning);
      setChat((current) => current.map((message) => {
        if (message.id !== runId) return message;
        const now = Date.now();
        const thought = message.thought && !message.thought.elapsedMs
          ? { ...message.thought, elapsedMs: now - message.thought.startedAt }
          : message.thought;
        return { ...message, thinking: "", thought };
      }));
      const stepId = event.id || `tool-${event.cursor ?? nextId("tool")}`;
      updateRun(runId, (steps) => steps.some((step) => step.id === stepId) ? steps : [...steps, { id: stepId, label: event.label || t.toolRunning, detail: event.detail, status: "active", startedAt: Date.now() }]);
    }
    else if (event.type === "plan_updated" && event.plan_update) {
      const receipt = event.plan_update;
      setChat((current) => {
        const index = current.map((message) => message.role).lastIndexOf("plan");
        if (!receipt.replaced && index >= 0) return current.map((message, position) =>
          position === index ? { ...message, plan: receipt.plan } : message);
        return [...current, { id: `plan-${event.id ?? nextId("plan")}`, role: "plan", content: "", plan: receipt.plan }];
      });
    }
    else if (event.type === "tool_completed") {
      if (!event.is_error && event.name === "update_plan" && event.plan_update) {
        const receipt = event.plan_update;
        setChat((current) => {
          const index = current.map((message) => message.role).lastIndexOf("plan");
          if (!receipt.replaced && index >= 0) return current.map((message, position) =>
            position === index ? { ...message, plan: receipt.plan } : message);
          return [...current, { id: `plan-${event.id ?? nextId("plan")}`, role: "plan", content: "", plan: receipt.plan }];
        });
      }
      setActivity(event.label || (event.is_error ? t.toolFailed : t.toolDone));
      const stepId = event.id || `tool-${event.cursor ?? nextId("tool")}`;
      updateRun(runId, (steps) => {
        const now = Date.now();
        return steps.some((step) => step.id === stepId)
          ? steps.map((step) => step.id === stepId ? { ...step, label: event.label || step.label, detail: event.detail ?? step.detail, status: event.is_error ? "failed" : "done", elapsedMs: now - step.startedAt } : step)
          // 没见过 requested 就直接 completed：这一步的耗时无从谈起，留空比编一个 0 好。
          : [...steps, { id: stepId, label: event.label || (event.is_error ? t.toolFailed : t.toolDone), detail: event.detail, status: event.is_error ? "failed" : "done", startedAt: now }];
      });
    }
    else setActivity(event.label || event.type);
    return { terminal: false as const };
  }

  /** 轮次没正常收尾：还挂在状态行上的最后一段补写进聊天区，不随状态行一起消失。 */
  function keepInterruptedLine() {
    const text = interruptedReply(conciseLineRef.current);
    conciseLineRef.current = null;
    if (!text) return;
    setChat((current) => [...current.filter((message) => message.role !== "status"), { id: nextId("assistant"), role: "assistant", content: text }]);
  }

  async function stopRemoteTurn(turnId: string) {
    const response = await fetch(`/api/turns/${encodeURIComponent(turnId)}/stop`, { method: "POST" });
    if (!response.ok) throw new Error(await response.text());
    keepInterruptedLine();
    abortRef.current?.abort(); setActivity(t.stopped); setBusy(false); abortRef.current = null; activeTurnRef.current = null; activeSessionRef.current = null;
    setActiveRuntimeSessionId("");
    if (activeRunRef.current) updateRun(activeRunRef.current, (steps) => steps.map((step) => step.status === "active" ? { ...step, label: t.stopped, status: "failed" as const, elapsedMs: Date.now() - step.startedAt } : step));
  }

  async function stop() {
    pendingStopRef.current = true;
    setActivity(t.stopping);
    const turnId = activeTurnRef.current;
    if (!turnId) return;
    try {
      await stopRemoteTurn(turnId);
    } catch (reason) {
      pendingStopRef.current = false;
      setError(`${t.requestFailed}: ${reason instanceof Error ? reason.message : String(reason)}`);
    }
  }

  // 轮次正常收尾（completed / partial）后发一次预测；输入框已有内容或附件就不发。
  function requestInputSuggestion(targetSessionId: string, turnId: string | null) {
    // 读 ref 而不是闭包：send() 的闭包里 prompt 还是刚发出去的那句。
    if (!targetSessionId || !composerEmptyRef.current) return;
    void inputSuggestion.request(targetSessionId, turnId, () => composerIdleRef.current && sessionIdRef.current === targetSessionId);
  }

  async function resumeTurn(id: string, turnId: string) {
    if (busy || abortRef.current) return;
    const controller = new AbortController();
    const runId = nextId("resume");
    let cursor = savedRuntimeCursor(id, turnId);
    let terminal = false;
    let terminalError = "";
    let retryDelay = 500;
    abortRef.current = controller;
    activeRunRef.current = runId;
    activeSessionRef.current = id;
    activeTurnRef.current = turnId;
    setActiveRuntimeSessionId(id);
    setBusy(true);
    setActivity(t.reconnecting);
    setChat((current) => [...current, { id: runId, role: "activity", content: "", steps: [] }]);
    try {
      while (!terminal && !controller.signal.aborted) {
        try {
          const response = await fetch(`/api/sessions/${encodeURIComponent(id)}/stream?after=${cursor}&language=${encodeURIComponent(language)}`, { signal: controller.signal, headers: { accept: "text/event-stream" } });
          if (response.status === 409) { terminal = true; break; }
          if (!response.ok && response.status >= 400 && response.status < 500) {
            terminalError = `${t.requestFailed}: ${(await response.text()) || t.requestFailed}`;
            terminal = true;
            break;
          }
          await readSse(response, async (event) => {
            if (event.cursor !== undefined) cursor = Math.max(cursor, event.cursor);
            const result = await applyStreamEvent(event, runId);
            if (result.terminal && result.error && event.turn_id) {
              terminal = true;
              terminalError = result.error;
            } else if (result.terminal && result.error) {
              throw new Error(result.error);
            } else if (result.terminal) {
              terminal = true;
            }
          });
          retryDelay = 500;
        } catch {
          if (controller.signal.aborted) break;
          setActivity(t.reconnecting);
          await waitForReconnect(retryDelay, controller.signal);
          retryDelay = Math.min(retryDelay * 2, 5000);
          continue;
        }
        if (!terminal && !controller.signal.aborted) {
          setActivity(t.reconnecting);
          await waitForReconnect(retryDelay, controller.signal);
          retryDelay = Math.min(retryDelay * 2, 5000);
        }
      }
      updateRun(runId, (steps) => {
        const now = Date.now();
        return steps.map((step) => step.status === "active"
          ? { ...step, status: terminalError ? "failed" as const : "done" as const, elapsedMs: now - step.startedAt }
          : step);
      });
      if (!controller.signal.aborted) {
        // 恢复的轮次不知道开始时间，分隔线只报工具数。
        const divider = turnDivider(runId, null);
        const detail = await json<SessionDetail>(`/api/sessions/${encodeURIComponent(id)}`);
        applySessionDetail(detail);
        setChat((current) => [...current, divider]);
        if (terminalError) setError(terminalError);
        else if (terminal) requestInputSuggestion(detail.id, turnId);
      }
      await refreshSessions();
    } finally {
      if (abortRef.current === controller) abortRef.current = null;
      if (activeRunRef.current === runId) activeRunRef.current = null;
      if (activeTurnRef.current === turnId) activeTurnRef.current = null;
      if (activeSessionRef.current === id) activeSessionRef.current = null;
      setActiveRuntimeSessionId("");
      pendingStopRef.current = false;
      setBusy(false);
      setActivity("");
    }
  }

  function requestWorkspaceEntry(target: Workspace, entry: WorkspaceEntry = {}) {
    ++workspaceEntryRevision.current;
    workspaceApproval.requestEntry(target, entry);
  }

  function cancelWorkspaceEntry() {
    ++workspaceEntryRevision.current;
    workspaceApproval.cancel();
  }

  function enterWorkspace(target: Workspace, entry: WorkspaceEntry) {
    ++workspaceEntryRevision.current;
    const changed = target.path !== workspaceRef.current;
    workspaceRef.current = target.path;
    workspaceDefaultRef.current = target.access;
    // 引导选择写入工作区默认；这里只恢复入口携带的会话覆盖值。
    workspaceDraftApprovalRef.current = target.access === "read_only" ? null : entry.approvalMode ?? null;
    pendingWorkspaceSession.current = entry.session ? { ...entry.session, workspace: target.path } : null;
    skipWorkspaceRestore.current = entry.fresh ?? false;
    if (entry.fresh) localStorage.removeItem(`${lastSessionPrefix}${target.path}`);
    if (entry.added) { setNewWorkspacePath(""); setAddingWorkspace(false); }
    setWorkspace(target.path);
    if (!changed && entry.session) {
      pendingWorkspaceSession.current = null;
      void loadSessionRef.current(entry.session.id, entry.session.turnId);
    } else if (!changed && entry.fresh) {
      sessionIdRef.current = "";
      setSessionId(""); setChat([]); setApprovalMode(workspaceDraftApprovalRef.current); setWorkspacePolicy(target.access);
      setPermissionsOpen(false); setSessionModel(null); setSessionModelProviderId(null); setSessionReasoningLevel(null);
      setDraftModel(null); setModelOpen(false); setShareOpen(false);
    }
  }

  async function loadSession(id: string, activeTurnId: string | null = null) {
    if (busy) return;
    workspaceApproval.cancel();
    const revision = ++workspaceEntryRevision.current;
    try {
      const detail = await json<SessionDetail>(`/api/sessions/${encodeURIComponent(id)}`);
      if (revision !== workspaceEntryRevision.current) return;
      const target = workspaces.find((item) => item.path === detail.workspace);
      if (target && (detail.workspace !== workspaceRef.current || needsWorkspaceApproval(target))) {
        requestWorkspaceEntry(target, { session: { id: detail.id, turnId: activeTurnId } });
        return;
      }
      if (detail.workspace !== workspaceRef.current) return;
      workspaceApproval.cancel();
      inputSuggestion.clear(); setError(""); setRuntimeFailure(null);
      applySessionDetail(detail);
      if (activeTurnId) await resumeTurn(detail.id, activeTurnId);
    } catch (reason) {
      if (revision === workspaceEntryRevision.current) setError(`${t.loadFailed}: ${reason instanceof Error ? reason.message : String(reason)}`);
    }
  }
  loadSessionRef.current = loadSession;

  function openRelatedSession(id: string, activeTurnId?: string | null) {
    setMobileSessionsOpen(false);
    if (id === sessionIdRef.current) return;
    if (busy || prompt.trim() || attachmentsRef.current.length > 0) {
      // The current stream remains attached here; the independent conversation
      // opens with its own stream and approval controls.
      const url = new URL(window.location.href);
      url.searchParams.set("session", id);
      url.hash = "";
      window.open(url.toString(), "_blank", "noopener");
      return;
    }
    void loadSession(id, activeTurnId ?? sessions.find((item) => item.id === id)?.active_turn_id);
  }

  async function refreshSessions() {
    setSessions(await json<Session[]>("/api/sessions"));
  }

  function childRelationshipChanged(link: ChildLink) {
    // Relationship changes refresh navigation without reloading chat or clearing drafts.
    if (link.control_state === "detached") {
      setSessions((current) => current.map((item) => item.id === link.child_session_id ? { ...item, delegation_detached: true } : item));
      setSessionRelation((current) => current?.id === link.child_session_id ? { ...current, detached: true } : current);
    }
    void refreshSessions().catch((reason: unknown) => setError(`${t.loadFailed}: ${reason instanceof Error ? reason.message : String(reason)}`));
  }

  /// 面板摊开时再读一次档位：TUI 那边也能切，页面上的旧值会骗人。
  async function refreshApprovalMode(id: string) {
    const requestedWorkspace = workspaceRef.current;
    try {
      const detail = await json<SessionDetail>(`/api/sessions/${encodeURIComponent(id)}`);
      if (id !== sessionIdRef.current || requestedWorkspace !== workspaceRef.current) return;
      setApprovalMode(detail.approval_mode ?? null);
      setWorkspacePolicy(detail.workspace_access);
    } catch { /* 读不到就保留现值；真正的切换失败会在面板里报出来 */ }
  }

  async function ignoreRuntimeEvent(id: string) {
    if (!sessionId) return;
    await fetch(`/api/runtime/events/${encodeURIComponent(id)}/ignore`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ session: sessionId }),
    });
    setRuntimeEvents((current) => current.filter((event) => event.id !== id));
  }

  async function refreshRuntimeActivity() {
    if (!workspace) return;
    setRuntimeActivity(await json<RuntimeActivity>(`/api/runtime/activity?workspace=${encodeURIComponent(workspace)}${sessionIdRef.current ? `&session=${encodeURIComponent(sessionIdRef.current)}` : ""}`));
  }

  async function resolveRuntimeApproval(id: string, decision: "allow_once" | "deny" | "always_allow") {
    try {
      await mutate(`/api/runtime/approvals/${encodeURIComponent(id)}/resolve`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ workspace, session: sessionId || null, decision }) });
      await refreshRuntimeActivity();
    } catch (reason) { setError(`${t.runtimeActionFailed}: ${reason instanceof Error ? reason.message : String(reason)}`); throw reason; }
  }

  async function answerRuntimeQuestion(id: string, answer: string | null) {
    try {
      await mutate(`/api/runtime/questions/${encodeURIComponent(id)}/answer`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ workspace, session: sessionId || null, answer }) });
      await refreshRuntimeActivity();
    } catch (reason) { setError(`${t.runtimeActionFailed}: ${reason instanceof Error ? reason.message : String(reason)}`); throw reason; }
  }

  function openRuntimeGate(id: string) {
    const card = document.getElementById(`runtime-gate-${id}`);
    if (!card) return;
    followBottomRef.current = false;
    setMobileSessionsOpen(false);
    card.scrollIntoView({ block: "center" });
    card.focus({ preventScroll: true });
  }

  async function runAgentAction(id: string, action: "stop" | "retry" | "retry_model" | "prompt" | "continue", currentModel?: string | null) {
    const needsMessage = action === "prompt" || action === "continue";
    const message = needsMessage ? window.prompt(action === "continue" ? t.agentContinuePrompt : t.agentPrompt)?.trim() : undefined;
    if (needsMessage && !message) return;
    const model = action === "retry_model" ? window.prompt(t.agentModelPrompt, currentModel ?? "")?.trim() : undefined;
    if (action === "retry_model" && !model) return;
    const endpointAction = action === "retry_model" ? "retry" : action;
    try {
      await mutate(`/api/runtime/agents/${encodeURIComponent(id)}/${endpointAction}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ workspace, ...(message ? { message } : {}), ...(model ? { model } : {}) }) });
      await refreshRuntimeActivity();
    } catch (reason) { setError(`${t.runtimeActionFailed}: ${reason instanceof Error ? reason.message : String(reason)}`); }
  }

  async function spawnRuntimeAgent(profile: AgentSpawnProfile, task: string): Promise<boolean> {
    const targetSessionId = sessionId;
    const isActive = Boolean(targetSessionId) && (selectedSession?.active === true || activeRuntimeSessionId === targetSessionId);
    if (!isActive) { setError(`${t.runtimeActionFailed}: ${t.activeSessionRequired}`); return false; }
    try {
      setError("");
      await mutate("/api/runtime/agents/spawn", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ workspace, session_id: targetSessionId, profile, prompt: task }) });
      await refreshRuntimeActivity();
      return true;
    } catch (reason) {
      setError(`${t.runtimeActionFailed}: ${reason instanceof Error ? reason.message : String(reason)}`);
      return false;
    }
  }

  const [deleteTarget, setDeleteTarget] = useState<Session | null>(null);
  // 「回到第 N 步」：步骤列表由服务端算（只有 Runtime 跑完的轮次才能回），
  // 前端只问两件事——回到哪一步、文件跟不跟着回。
  const [rewindPoints, setRewindPoints] = useState<RewindPoint[] | null>(null);
  const [rewindLoading, setRewindLoading] = useState(false);
  const [rewindSelection, setRewindSelection] = useState<number>(-1);
  const [rewindFiles, setRewindFiles] = useState(true);
  const [rewindNotice, setRewindNotice] = useState("");

  // 加进来的工作区只影响这个浏览器视图能看到什么，不改 Runtime 的全局默认。
  // 非回环监听时后端会拒绝并说明理由——那条边界留在启动命令里。
  async function addWorkspace() {
    const path = newWorkspacePath.trim();
    if (!path || busy) return;
    const revision = ++workspaceEntryRevision.current;
    setWorkspaceError("");
    try {
      const added = await workspaceApproval.addWorkspace(path);
      if (revision === workspaceEntryRevision.current) requestWorkspaceEntry(added, { fresh: true, added: true });
    } catch (reason) {
      if (revision === workspaceEntryRevision.current) setWorkspaceError(reason instanceof Error ? reason.message : String(reason));
    }
  }

  function sessionRow(item: Session, depth: number) {
    const selected = sessionId === item.id;
    return <Flex key={item.id} className={`session-row${depth ? " nested" : ""}${selected ? " selected" : ""}`} opacity={item.archived ? 0.68 : 1}
      onContextMenu={(event) => {
        // 没有插件贡献这个位置时不劫持右键——浏览器自带的菜单仍然该能用。
        if (!sessionContextEntries.length) return;
        event.preventDefault();
        setPopup({ entries: sessionContextEntries, x: event.clientX, y: event.clientY, args: { session: item.id } });
      }}>
      <button type="button" className="session-title" disabled={busy} onClick={() => void loadSession(item.id, item.active_turn_id)}>
        {item.parent_session_id && !item.delegation_detached && <span className="session-pin" title={childSessionsText(language).title} aria-label={childSessionsText(language).title}><GitFork size={12} /></span>}{item.active && <span className="session-active-dot" aria-hidden="true" />}{item.pinned_at != null && <span className="session-pin" title={t.pinned} aria-label={t.pinned}><Pin size={12} /></span>}
        {/* 标题没生成时后端会带回首条用户消息。显示它而不是一排
            一模一样的 New session——那些会话是有内容的。 */}
        {item.preview ? <span className="session-untitled">{item.preview}</span> : item.title}
      </button>
      <Flex className="session-actions">
        <button type="button" title={t.renameSession} aria-label={t.renameSession} disabled={busy || item.active} onClick={() => void renameSession(item)}><Pencil size={12} /></button>
        <button type="button" title={item.pinned_at != null ? t.unpinSession : t.pinSession} aria-label={item.pinned_at != null ? t.unpinSession : t.pinSession} disabled={busy} onClick={() => void togglePinSession(item)}>{item.pinned_at != null ? <PinOff size={12} /> : <Pin size={12} />}</button>
        <button type="button" title={item.archived ? t.unarchiveSession : t.archiveSession} aria-label={item.archived ? t.unarchiveSession : t.archiveSession} disabled={busy || item.active} onClick={() => void toggleArchiveSession(item)}>{item.archived ? <ArchiveRestore size={12} /> : <Archive size={12} />}</button>
        <button type="button" className="danger" title={t.deleteSession} aria-label={t.deleteSession} disabled={busy || item.active} onClick={() => setDeleteTarget(item)}><X size={12} /></button>
      </Flex>
    </Flex>;
  }

  async function renameInstance() {
    if (!identity) return;
    const name = window.prompt(t.instanceRenamePrompt, identity.name)?.trim();
    if (!name || name === identity.name) return;
    try {
      const next = await renameIdentity(name);
      setIdentity(next);
      setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: t.instanceRenamed.replace("{name}", next.name) }]);
    } catch (reason) {
      setError(`${t.requestFailed}: ${reason instanceof Error ? reason.message : String(reason)}`);
    }
  }

  async function renameSession(target: Session) {
    const title = window.prompt(t.renamePrompt, target.title)?.trim(); if (!title) return;
    try { await mutate(`/api/sessions/${encodeURIComponent(target.id)}/rename`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ title }) }); await refreshSessions(); }
    catch (reason) { setError(`${t.sessionActionFailed}: ${reason instanceof Error ? reason.message : String(reason)}`); }
  }

  async function togglePinSession(target: Session) {
    const action = target.pinned_at ? "unpin" : "pin";
    try { await mutate(`/api/sessions/${encodeURIComponent(target.id)}/${action}`, { method: "POST" }); await refreshSessions(); }
    catch (reason) { setError(`${t.sessionActionFailed}: ${reason instanceof Error ? reason.message : String(reason)}`); }
  }

  async function forkSelectedSession() {
    if (!selectedSession) return;
    const title = window.prompt(t.forkPrompt, `${selectedSession.title}`)?.trim(); if (!title) return;
    try { const fork = await json<{ id: string }>(`/api/sessions/${encodeURIComponent(selectedSession.id)}/fork`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ title }) }); await refreshSessions(); await loadSession(fork.id); }
    catch (reason) { setError(`${t.sessionActionFailed}: ${reason instanceof Error ? reason.message : String(reason)}`); }
  }

  async function openRewindDialog() {
    if (!selectedSession) return;
    setRewindNotice(""); setRewindLoading(true); setRewindPoints([]); setRewindSelection(-1); setRewindFiles(true);
    try {
      const points = await json<RewindPoint[]>(`/api/sessions/${encodeURIComponent(selectedSession.id)}/rewind-points`);
      setRewindPoints(points);
      // 默认停在最近的一步：最常见的回退是「上一步做错了」。
      setRewindSelection(points.length - 1);
      setRewindFiles(points.length > 0 && points[points.length - 1].can_restore_workspace);
    } catch (reason) { setRewindPoints(null); setError(`${t.sessionActionFailed}: ${reason instanceof Error ? reason.message : String(reason)}`); }
    finally { setRewindLoading(false); }
  }

  async function confirmRewind() {
    const target = selectedSession; const points = rewindPoints; const point = points?.[rewindSelection];
    if (!target || !point) return;
    const restore_workspace = rewindFiles && point.can_restore_workspace;
    try {
      const result = await json<RewindResult>(`/api/sessions/${encodeURIComponent(target.id)}/rewind`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ through_turn_id: point.turn_id, restore_workspace }) });
      setRewindPoints(null);
      const done = (point.step === 0 ? t.rewindDoneBeginning : t.rewindDone.replace("{step}", String(point.step))).replace("{dropped}", String(result.dropped_turn_ids.length));
      const files = result.workspace ? ` · ${t.rewindFilesSummary.replace("{restored}", String(result.workspace.restored.length)).replace("{removed}", String(result.workspace.removed.length))}` : "";
      setRewindNotice(`${done}${files}`);
      await refreshSessions(); await loadSession(target.id);
    } catch (reason) { setError(`${t.sessionActionFailed}: ${reason instanceof Error ? reason.message : String(reason)}`); }
  }

  async function toggleArchiveSession(target: Session) {
    const action = target.archived ? "unarchive" : "archive";
    try { await mutate(`/api/sessions/${encodeURIComponent(target.id)}/${action}`, { method: "POST" }); await refreshSessions(); }
    catch (reason) { setError(`${t.sessionActionFailed}: ${reason instanceof Error ? reason.message : String(reason)}`); }
  }

  async function confirmDeleteSession() {
    const target = deleteTarget;
    if (!target) return;
    try {
      await mutate(`/api/sessions/${encodeURIComponent(target.id)}`, { method: "DELETE", headers: { "content-type": "application/json" }, body: JSON.stringify({ confirmation: target.id }) });
      if (sessionId === target.id) { setSessionId(""); setChat([]); setGoal(""); setGoalEnabled(false); pendingGoalPreference.current = null; setRuntimeFailure(null); }
      await refreshSessions();
    } catch (reason) { setError(`${t.sessionActionFailed}: ${reason instanceof Error ? reason.message : String(reason)}`); }
    finally { setDeleteTarget(null); }
  }

  async function exportSelectedSession() {
    if (!selectedSession) return;
    try { const value = await json<unknown>(`/api/sessions/${encodeURIComponent(selectedSession.id)}/export`); const url = URL.createObjectURL(new Blob([JSON.stringify(value, null, 2)], { type: "application/json" })); const anchor = document.createElement("a"); anchor.href = url; anchor.download = `willdeep-session-${selectedSession.id}.json`; anchor.click(); URL.revokeObjectURL(url); }
    catch (reason) { setError(`${t.sessionActionFailed}: ${reason instanceof Error ? reason.message : String(reason)}`); }
  }

  function attachImage(file: File) {
    if (file.size > MAX_IMAGE_BASE64 * 3 / 4) { setError(attachmentText("imageTooLarge", language)); return; }
    const reader = new FileReader(); reader.onerror = () => setError(attachmentText("imageFailed", language)); reader.onload = () => {
      const url = String(reader.result); const image = new Image(); image.onload = () => {
        const data = url.slice(url.indexOf(",") + 1);
        appendAttachment({ kind: "image", name: file.name || `${t.pastedImage}.png`, media_type: file.type, data, width: image.naturalWidth, height: image.naturalHeight });
      }; image.onerror = () => setError(attachmentText("imageFailed", language)); image.src = url;
    }; reader.readAsDataURL(file);
  }

  function handlePaste(event: React.ClipboardEvent<HTMLTextAreaElement>) {
    const imageFile = clipboardImageFile(event.clipboardData);
    if (imageFile) { event.preventDefault(); attachImage(imageFile); return; }
    const text = event.clipboardData.getData("text/plain");
    if (text.includes("\n") || text.length > 200) { event.preventDefault(); appendAttachment({ kind: "text", name: `${t.pastedText}-${attachmentsRef.current.length + 1}.txt`, content: text }); }
  }

  // `override` 给插件的 `window.willdeep.chat.send` 用：它递进来的文本要
  // 立刻起一个回合，而 setPrompt 是异步的——读 state 会读到上一轮的值。
  // 走的是与用户敲回车完全相同的这条路：审批档位与沙箱一个都不绕。
  async function send(override?: string, source: "composer" | "plugin" = "composer") {
    if (workspaceApproval.pending || !workspaceRef.current) return;
    const sendAttachments = attachmentsRef.current;
    const typed = (override ?? prompt).trim();
    if (selectedSession?.archived) { setError(`${t.requestFailed}: ${t.archived}`); return; }
    if (typed === "/clear") { setChat([]); setPrompt(""); return; }
    if (typed === "/help") { setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: t.helpText }]); setPrompt(""); return; }
    if (typed === "/skills") { setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: composer.skills.length ? composer.skills.map((skill) => `$${skill.identifier} · ${skill.name}\n${skill.description}`).join("\n\n") : t.noSkills }]); setPrompt(""); return; }
    if (typed === "/goal off") { updateGoalPreference("", false); setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: t.goalOff }]); setPrompt(""); return; }
    if (typed === "/short" || typed === "/verbose") {
      setPrompt("");
      // 风格记在会话上。还没有会话时无处可记，先说清楚，不假装已经切过去了。
      if (!sessionId) { setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: t.shortNeedsSession }]); return; }
      const detailed = typed === "/verbose";
      try {
        await mutate(`/api/sessions/${encodeURIComponent(sessionId)}/response-style`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ response_style: detailed ? "detailed" : "short" }) });
        replyStyleRef.current = detailed ? "detailed" : "short";
        setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: detailed ? t.verboseOn : t.shortOn }]);
      } catch (reason) { setError(`${t.requestFailed}: ${reason instanceof Error ? reason.message : String(reason)}`); }
      return;
    }
    if (typed === "/name" || typed.startsWith("/name ")) {
      setPrompt("");
      // 改名是本机管理动作：插件页面不能替用户改这台机器的身份。
      if (source === "plugin") { setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: t.instancePluginBlocked }]); return; }
      const argument = typed.slice("/name".length).trim();
      if (!argument) {
        setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: identity ? `${t.instanceName}: ${identity.name} · ${identity.id}\n${t.instanceUsage}` : `${t.instanceUnavailable}\n${t.instanceUsage}` }]);
        return;
      }
      try {
        const next = await renameIdentity(argument);
        setIdentity(next);
        setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: t.instanceRenamed.replace("{name}", next.name) }]);
      } catch (reason) { setError(`${t.requestFailed}: ${reason instanceof Error ? reason.message : String(reason)}`); }
      return;
    }
    // 审批档位是本机管理动作：面板是主入口，这条命令只是同一条路的口令版。
    if (typed === "/permissions" || typed.startsWith("/permissions ")) {
      setPrompt("");
      const copy = permissionsText(language);
      // 插件页面也能调 send()。档位只认用户亲手敲的字——否则一个页面就能
      // 替用户把会话升到完全访问。
      if (source === "plugin") { setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: copy.pluginBlocked }]); return; }
      if (!sessionId) { setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: copy.needsSession }]); return; }
      const argument = typed.slice("/permissions".length).trim();
      if (!argument) { setPermissionsOpen(true); void refreshApprovalMode(sessionId); return; }
      const [word, ...flags] = argument.toLowerCase().split(/\s+/);
      const mode = (["strict", "smart", "workspace_write", "full_access"] as WorkspaceAccess[])
        .find((item) => item === word || item.replace(/_/g, "-") === word);
      if (!mode) { setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: copy.usage }]); return; }
      if (workspacePolicy === "read_only") { setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: copy.readOnlyWorkspace }]); return; }
      // 完全访问多一道确认，且确认必须是用户亲手敲的那个词。
      if (mode === "full_access" && !flags.includes("confirm")) { setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: copy.fullConfirmHint }]); return; }
      try {
        await changeApprovalMode(sessionId, mode);
        setApprovalMode(mode);
        setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: copy.saved.replace("{mode}", accessLabel(mode, t)) }]);
      } catch (reason) { setError(`${copy.failed}: ${reason instanceof Error ? reason.message : String(reason)}`); }
      return;
    }
    // 换模型同样是本机管理动作：面板是主入口，这条命令是同一件事的口令版。
    if (typed === "/model" || typed.startsWith("/model ")) {
      setPrompt("");
      const copy = modelText(language);
      if (source === "plugin") { setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: copy.pluginBlocked }]); return; }
      if (!sessionId) { setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: copy.noSession }]); return; }
      const argument = typed.slice("/model".length).trim();
      if (!argument) { setModelOpen(true); return; }
      try {
        await changeModel(sessionId, argument);
        setSessionModel(argument);
        setSessionModelProviderId(null);
        setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: copy.applied.replace("{model}", argument) }]);
      } catch (reason) {
        if (reason instanceof ModelSwitchFailure && reason.status === 403) setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: copy.notWritable }]);
        else setError(`${copy.failed}: ${reason instanceof Error ? reason.message : String(reason)}`);
      }
      return;
    }
    const goalToStart = typed.startsWith("/goal ") ? typed.slice(6).trim() : "";
    if (goalToStart) {
      updateGoalPreference(goalToStart, true);
      setChat((current) => [...current, { id: nextId("assistant"), role: "assistant", content: `${t.goalSet}: ${goalToStart}` }]);
      if (busy) { setPrompt(""); return; }
    }
    const content = goalToStart || typed || (sendAttachments.length ? t.attachmentPrompt : ""); if (!content || !workspace) return;
    // 普通输入不是审批决定或工具提问的答案。恢复现有入口，并保留草稿。
    if (!typed.startsWith("/") && visibleHumanGates.length) {
      const feedback = runningInputFeedback(runtimeActivity, sessionId, language);
      setSubmissionNotice({ sessionId, text: `${feedback.text} · ${queueText("inputRetained", language)}` });
      if (feedback.gateId) openRuntimeGate(feedback.gateId);
      return;
    }
    // 采用过的建议在真正发出去这一刻结算：原样 / 改过 / 重写由服务端判定。
    inputSuggestion.settleSent(typed);
    // 旧 steer 仅内存入箱且无幂等键；保留 Runtime 持久队列，不能把入队说成模型已读。
    if (busy || selectedSession?.active || activeRuntimeSessionId === sessionId && Boolean(sessionId)) { await enqueueDraft(content); return; }
    setSubmissionNotice(null);
    const activeGoal = goalToStart || (goalEnabled ? goal : "");
    const harnessPrompt = activeGoal && content !== "/compress" ? `<goal>\n${activeGoal}\n</goal>\nContinue until this goal is genuinely complete.\n\n${content}` : content;
    const outgoingAttachments = sendAttachments;
    const outgoingPrompt = prompt;
    // 新会话第一条消息顺便把开局选好的模型与档位带上：会话是这条消息建的，
    // 不带上它们就只能等建好之后再补一次，中间那一轮已经按默认跑掉了。
    const draftChoices = sessionId ? {} : {
      model: draftModel?.model ?? null,
      provider_id: draftModel?.providerId ?? null,
      approval_mode: approvalMode,
      // 升到完全访问要在请求里显式确认；面板上的勾选就是那次确认。
      approval_confirm: approvalMode ? isDangerousAccess(approvalMode) : false,
    };
    const payload = { prompt: harnessPrompt, session_id: sessionId || null, workspace, language, attachments: outgoingAttachments.map(attachmentPayload), ...draftChoices };
    const fingerprint = JSON.stringify(payload);
    if (draftSendRequest.current?.fingerprint !== fingerprint) draftSendRequest.current = { fingerprint, id: crypto.randomUUID() };
    const requestBody = JSON.stringify({ ...payload, request_id: draftSendRequest.current?.id });
    if (new TextEncoder().encode(requestBody).length > MAX_CHAT_BODY_BYTES) { setError(attachmentText("batchTooLarge", language)); return; }
    const runId = nextId("run"); const controller = new AbortController(); abortRef.current = controller;
    const startedAt = Date.now();
    activeRunRef.current = runId;
    followBottomRef.current = true;
    setError(""); setRuntimeFailure(null); setActivity(t.thinking); setBusy(true);
    inputSuggestion.clear();
    setChat((current) => [...current, { id: nextId("user"), role: "user", content: `${content}${outgoingAttachments.length ? `\n[${outgoingAttachments.length} ${t.attachmentCount}]` : ""}` }, { id: runId, role: "activity", content: "", steps: [] }]);
    let terminalFailure = false;
    let terminalFailureMessage = "";
    try {
      const response = await fetch("/api/chat/stream", { method: "POST", signal: controller.signal, headers: { "content-type": "application/json", accept: "text/event-stream" }, body: requestBody });
      let answer = "";
      let completedSessionId = sessionId;
      let completedTurnId: string | null = null;
      let terminal = false;
      await readSse(response, async (event) => {
        if (event.type === "submitted" && event.turn_id) {
          draftSendRequest.current = null;
          if (override === undefined) setPrompt((current) => current === outgoingPrompt ? "" : current);
          if (attachmentsRef.current === outgoingAttachments) setAttachments([]);
        }
        if (event.session_id) completedSessionId = event.session_id;
        if (event.turn_id) completedTurnId = event.turn_id;
        const result = await applyStreamEvent(event, runId);
        if (result.terminal && result.error) {
          terminalFailure = Boolean(event.turn_id);
          terminalFailureMessage = result.error;
          throw new Error(result.error);
        }
        if (result.terminal) {
          terminal = true;
          answer = result.text || t.emptyReply;
        }
      });
      if (!terminal) throw new Error(t.streamDisconnected);
      updateRun(runId, (steps) => settleActiveSteps(steps, Date.now()));
      // 简洁模式的中途文字只摆在状态行上。收尾是它进聊天区的时候：正文与它相同就落这一段，
      // 正文在它前面拼了提示就两段都落。状态行随之撤掉。
      const held = conciseLineRef.current?.text ?? null;
      conciseLineRef.current = null;
      const head = unshownReply(answer, held);
      setChat((current) => {
        const settled = current.filter((message) => message.role !== "status");
        const bubbles: ChatMessage[] = [];
        if (head) bubbles.push({ id: nextId("assistant"), role: "assistant", content: head });
        if (held && usesShortStyle(replyStyleRef.current)) bubbles.push({ id: nextId("assistant"), role: "assistant", content: held });
        if (!bubbles.length) bubbles.push({ id: nextId("assistant"), role: "assistant", content: answer || t.emptyReply });
        return [...settled, ...bubbles];
      });
      const divider = turnDivider(runId, startedAt);
      if (completedSessionId) {
        const detail = await json<SessionDetail>(`/api/sessions/${encodeURIComponent(completedSessionId)}`);
        applySessionDetail(detail);
      }
      setChat((current) => [...current, divider]);
      requestInputSuggestion(completedSessionId, completedTurnId);
      refreshSessions().catch(() => undefined);
    } catch (reason) {
      const recoverSessionId = activeSessionRef.current;
      const recoverTurnId = activeTurnRef.current;
      if (!(reason instanceof DOMException && reason.name === "AbortError") && recoverSessionId && recoverTurnId && !terminalFailure) {
        if (abortRef.current === controller) abortRef.current = null;
        if (activeRunRef.current === runId) activeRunRef.current = null;
        setBusy(false);
        setActivity(t.reconnecting);
        await resumeTurn(recoverSessionId, recoverTurnId);
      } else if (!(reason instanceof DOMException && reason.name === "AbortError")) {
        keepInterruptedLine();
        setError(terminalFailureMessage || `${t.requestFailed}: ${reason instanceof Error ? reason.message : String(reason)}`);
      }
    } finally { if (abortRef.current === controller) abortRef.current = null; if (activeRunRef.current === runId) activeRunRef.current = null; activeTurnRef.current = null; activeSessionRef.current = null; setActiveRuntimeSessionId(""); pendingStopRef.current = false; setBusy(false); setActivity(""); }
  }

  // 一级入口栏之后，右侧要么是对话（原有侧栏 + 聊天），要么是一个插件目的地
  // （它的配套侧栏 + 页面），要么是插件中心。入口、侧栏与中央页永远来自
  // 同一个 descriptor，不会出现入口属于 A、侧栏属于 B 的半切换状态。
  const railNav = <PluginRail entries={pluginEntries} selection={rail} messages={t} brandLabel={agentText("brand", language)} schedulesLabel={scheduleText("title", language)} agentsLabel={agentText("title", language)} experimentsLabel={agentText("experimentTitle", language)} agentsGroupLabel={agentText("navigationGroup", language)} onSelect={(next) => { if (hashForRail(next) === hashForRail(rail)) return; if (!allowAgentNavigation(agentDirtyRef.current, language)) return; if (rail.kind === "schedules" && scheduleDirtyRef.current && !window.confirm(scheduleText("discard", language))) return; scheduleDirtyRef.current = false; setPluginSelectedItem(null); setRail(next); }} />;
  const setupGuide = <>
    {workspaceApproval.pending && <WorkspaceApprovalGuide key={workspaceApproval.pending.revision} path={workspaceApproval.pending.workspace.path} language={language} messages={t} onSave={workspaceApproval.apply} onCancel={cancelWorkspaceEntry} />}
    {showSetup ? <ConfigurationSettings compact messages={t} language={language} onDone={finishSetup} /> : setupLoadFailed ? <Box role="alert" className="provider-setup">{t.setupCheckFailed}</Box> : null}
  </>;

  if (rail.kind === "configuration") {
    return <Box>{setupGuide}<Flex minH="100vh" bg="var(--bg-page)" color="var(--text)">{railNav}<Box as="main" flex="1" minW="0" overflow="hidden"><BuiltinConfigurationWorkspace messages={t} locale={language} colorScheme={colorScheme} onIdentityChange={setIdentity} /></Box>{pluginOverlays}</Flex></Box>;
  }

  if (rail.kind === "agents" || rail.kind === "agent-experiments") {
    return <Box>{setupGuide}<Flex minH="100vh" bg="var(--bg-page)" color="var(--text)">{railNav}<AgentLibrary mode={rail.kind === "agents" ? "library" : "experiments"} onDirty={trackAgentDirty} onNavigate={mode => setRail({kind:mode === "library" ? "agents" : "agent-experiments"})} language={language} onTrial={(text) => { setPrompt(text); setRail({ kind: "conversation" }); }} />{pluginOverlays}</Flex></Box>;
  }

  if (rail.kind === "schedules") {
    return <Box>{setupGuide}<Flex minH="100vh" bg="var(--bg-page)" color="var(--text)">{railNav}<ScheduleManager onDirty={(value) => { scheduleDirtyRef.current = value; }} workspaces={workspaces} workspace={workspace} language={language} messages={t} onOpenSession={(id, turnId) => { if (busy) { setError(scheduleText("sessionSwitchBusy", language)); setRail({ kind: "conversation" }); return; } setRail({ kind: "conversation" }); void loadSessionRef.current(id, turnId ?? null); }} />{pluginOverlays}</Flex></Box>;
  }

  if (rail.kind === "federation") {
    return <Box>{setupGuide}<Flex minH="100vh" bg="var(--bg-page)" color="var(--text)">{railNav}<FederationPanel language={language} onOpenSession={(id) => { setRail({ kind: "conversation" }); void loadSessionRef.current(id); }} />{pluginOverlays}</Flex></Box>;
  }

  if (rail.kind === "intake") {
    return <Box>{setupGuide}<Flex minH="100vh" bg="var(--bg-page)" color="var(--text)">{railNav}<IntakeManager language={language} onOpenSession={(id) => { setRail({ kind: "conversation" }); void loadSessionRef.current(id); }} />{pluginOverlays}</Flex></Box>;
  }

  if (rail.kind === "mcp") {
    return <Box>{setupGuide}<Flex minH="100vh" bg="var(--bg-page)" color="var(--text)">{railNav}<McpPage messages={t} />{pluginOverlays}</Flex></Box>;
  }

  if (rail.kind === "center") {
    return <Box>{setupGuide}<Flex minH="100vh" bg="var(--bg-page)" color="var(--text)">
      {railNav}
      <PluginCenter plugins={plugins} failures={pluginFailures} messages={t} onChanged={() => setPluginReloadToken((current) => current + 1)} />
      {pluginOverlays}
    </Flex></Box>;
  }

  if (rail.kind === "plugin" && activePlugin) {
    const { plugin, destination } = activePlugin;
    return <Box>{setupGuide}<Flex minH="100vh" bg="var(--bg-page)" color="var(--text)">
      {railNav}
      {destination.sidebar?.mode === "declarative" && <PluginSidebar plugin={plugin} destination={destination} locale={language} messages={t} reloadToken={pluginReloadToken} onNavigate={navigateToDestination} onSelectItem={setPluginSelectedItem} selectedItemId={pluginSelectedItem} onRowContextMenu={(event, componentId, commands) => {
        const entries = menuEntries(plugins, "plugin.sidebar.row.context").filter((entry) => entry.pluginId === plugin.id && (!commands.length || commands.includes(entry.commandId)));
        if (!entries.length) return;
        event.preventDefault();
        setPopup({ entries, x: event.clientX, y: event.clientY, args: { item: componentId } });
      }} />}
      <PluginPage plugin={plugin} destination={destination} messages={t} locale={language} colorScheme={colorScheme} workspace={workspace || null} sessionId={sessionId || null} selectedItemId={pluginSelectedItem} onSelectItem={setPluginSelectedItem} onNavigate={navigateToDestination} onOpenPluginCenter={() => setRail({ kind: "center" })} busy={busy} onChatText={(text, sendNow) => {
        setRail({ kind: "conversation" });
        if (sendNow) void send(text, "plugin"); else setPrompt(text);
      }} onOpenSession={(id) => { setRail({ kind: "conversation" }); void loadSessionRef.current(id); }} />
      {pluginOverlays}
    </Flex></Box>;
  }

  return <Box>{setupGuide}<Flex className="chat-workspace" minH="100vh" bg="var(--bg-page)" color="var(--text)">
    {railNav}
    {/* 侧栏本身不滚动：只有会话列表滚。这样列表能吃掉全部剩余高度，
        而不是被上面几块设置挤到下半屏。 */}
    {mobileSessionsOpen && <button className="session-sidebar-backdrop" aria-label={t.cancel} onClick={() => setMobileSessionsOpen(false)} />}
    <Box as="aside" id="chat-session-sidebar" aria-label={t.session} className={`chat-sidebar${mobileSessionsOpen ? " mobile-open" : ""}`} onKeyDown={(event) => { if (event.key === "Escape") setMobileSessionsOpen(false); }} onClick={(event) => { if (event.target instanceof Element && event.target.closest(".session-title")) setMobileSessionsOpen(false); }} w={{ base: "0", md: "260px" }} display={{ base: "none", md: "flex" }} flexDir="column" borderRight="1px solid" borderColor="var(--border-subtle)" p="5" bg="var(--bg-surface)" overflow="hidden">
      <Flex justify="space-between" align="flex-start" mb="5">
        <Box minW="0">
          <Flex align="center" gap="2">
            <Heading size="md" lineHeight="1.2">{t.appName}</Heading>
            {identity && <button type="button" className="instance-name" title={`${t.instanceRenameHint} · ${identity.id}`} onClick={() => void renameInstance()}>{identity.name}</button>}
          </Flex>
          <Text color="var(--text-faint)" fontSize="xs">{t.conversation}</Text>
        </Box>
        <SidebarSettings messages={t} language={language} onLanguageChange={setLanguage} />
      </Flex>
      {/* 项目是一棵树，不再用下拉框切换。加号贴在树的右上角。 */}
      <Flex gap="1" align="center" justify="flex-end">
        <button type="button" className={`workspace-add${addingWorkspace ? " active" : ""}`} title={addingWorkspace ? t.cancel : t.addWorkspace} aria-label={addingWorkspace ? t.cancel : t.addWorkspace} aria-expanded={addingWorkspace} onClick={() => { setAddingWorkspace((current) => !current); setWorkspaceError(""); }}>
          <SfIcon name={addingWorkspace ? "sf:x.mark" : "sf:plus.circle"} size={15} />
        </button>
      </Flex>
      {addingWorkspace && <Box mt="2">
        <Input size="sm" autoFocus value={newWorkspacePath} placeholder={t.addWorkspacePlaceholder} aria-label={t.addWorkspace} color="var(--text)" _placeholder={{ color: "var(--text-ghost)" }} bg="var(--bg-raised)" borderColor="var(--border-strong)"
          onChange={(event) => setNewWorkspacePath(event.target.value)}
          onKeyDown={(event) => { if (event.key === "Enter") void addWorkspace(); if (event.key === "Escape") { setAddingWorkspace(false); setWorkspaceError(""); } }} />
        <Text fontSize="2xs" color="var(--text-faint)" mt="1">{t.addWorkspaceHint}</Text>
        {workspaceError && <Text fontSize="xs" color="var(--danger-text)" mt="1">{workspaceError}</Text>}
      </Box>}
      <RuntimeSidebar language={language} onOpenSession={openRelatedSession} workflowPanel={<WorkflowSidebar key={sessionId} parentSessionId={sessionId} language={language} messages={t} canControl={identity !== null && workspacePolicy !== "read_only"} />} onLoadRuns={(id, after) => json(`/api/runtime/agents/${encodeURIComponent(id)}/runs?workspace=${encodeURIComponent(workspace)}${after === undefined ? "" : `&after=${after}`}`)} activity={{ ...runtimeActivity, gates: visibleHumanGates }} events={runtimeEvents} onIgnoreEvent={ignoreRuntimeEvent} messages={t} onOpenGate={openRuntimeGate} onAgentAction={runAgentAction} canSpawnAgent={Boolean(sessionId) && !childReadOnly && (selectedSession?.active === true || activeRuntimeSessionId === sessionId)} onSpawnAgent={spawnRuntimeAgent} />
      {/* 会话区吃掉剩余高度。minH=0 是必须的：没有它，flex 子项的最小高度是
          内容高度，列表撑破容器而不是内部滚动。 */}
      <Flex direction="column" flex="1" minH="0" mt="4">
        <div className="session-search"><Search size={15} /><Input size="sm" mb="2" flex="0 0 auto" value={sessionSearch} onChange={(event) => setSessionSearch(event.target.value)} placeholder={t.searchSessions} aria-label={t.searchSessions} color="var(--text)" _placeholder={{ color: "var(--text-ghost)" }} bg="var(--bg-raised)" borderColor="var(--border-strong)" /></div>
        <Box className="session-list" flex="1" minH="0">
          {!orderedProjects.length && <Text color="var(--text-faint)" fontSize="sm">{t.noSessions}</Text>}
          {searching && !orderedProjects.some((project) => (projectSessions.get(project.path) ?? []).some((item) => matchesSessionQuery(item, sessionQuery))) && <Text color="var(--text-faint)" fontSize="sm">{t.noSessionMatch}</Text>}
          {orderedProjects.map((project) => {
            const all = projectSessions.get(project.path) ?? [];
            // 搜索按标题，占位标题还比对侧栏上显示的首条消息。命中全部列出，不受 20 条上限。
            const matched = searching ? all.filter((item) => matchesSessionQuery(item, sessionQuery)) : all;
            if (searching && !matched.length) return null;
            const open = searching || expandedProjects.has(project.path);
            const shown = listedSessionCount(matched.length, projectShown[project.path] ?? PROJECT_PREVIEW, searching);
            const rows = sessionTree(matched.slice(0, shown));
            const canShowMore = !searching && shown < Math.min(matched.length, PROJECT_CAP);
            const canCollapse = !searching && shown > PROJECT_PREVIEW;
            const capped = shown >= PROJECT_CAP && matched.length > PROJECT_CAP;
            return <Box key={project.id} className="project-group">
              <div className={`project-head${project.path === workspace ? " current" : ""}`}>
                <button type="button" className="project-toggle" aria-expanded={open} title={project.path} onClick={() => setExpandedProjects((current) => { const next = new Set(current); if (next.has(project.path)) next.delete(project.path); else next.add(project.path); return next; })}>
                  <span className={`archived-caret${open ? " open" : ""}`}>▸</span>
                  <FolderGit2 size={13} />
                  <span className="project-name">{project.name}</span>
                  <span className="project-count">{matched.length}</span>
                </button>
                <button type="button" className="project-new" title={t.newSessionIn} aria-label={t.newSessionIn} disabled={busy} onClick={() => requestWorkspaceEntry(project, { fresh: true })}><SfIcon name="sf:plus.circle" size={13} /></button>
              </div>
              {open && <VStack align="stretch" gap="1" className="project-sessions">
                {rows.map((entry) => sessionRow(entry.session, entry.depth))}
                {!rows.length && <Text color="var(--text-faint)" fontSize="sm" pl="5">{t.noSessions}</Text>}
                {(canShowMore || canCollapse) && <div className="project-more">
                  {canShowMore && <button type="button" onClick={() => setProjectShown((current) => ({ ...current, [project.path]: afterShowMore(current[project.path] ?? PROJECT_PREVIEW) }))}>{t.showMoreSessions}</button>}
                  {canCollapse && <button type="button" onClick={() => setProjectShown((current) => ({ ...current, [project.path]: PROJECT_PREVIEW }))}>{t.collapseSessions}</button>}
                </div>}
                {capped && <Text color="var(--text-ghost)" fontSize="2xs" pl="5">{t.sessionLimit.replace("{count}", String(PROJECT_CAP))}</Text>}
              </VStack>}
            </Box>;
          })}
        </Box>
        {/* 已归档会话固定在列表下方，不跟着滚动。会话多的项目会把滚动区顶得很长，
            收在滚动区底部就等于藏起来了——用户看不到这个入口。展开后的归档列表
            自己限高滚动，仍然不挤压项目树。 */}
        {archivedSessions.length > 0 && <Box className="archived-section" flex="0 0 auto">
          <button type="button" className="archived-toggle" aria-expanded={showArchived} onClick={() => setShowArchived((current) => !current)}>
            <span className={`archived-caret${showArchived ? " open" : ""}`}>▸</span>{t.showArchived} ({archivedSessions.length})
          </button>
          {showArchived && <VStack align="stretch" gap="1" className="archived-sessions">{sessionTree(archivedSessions).map((entry) => sessionRow(entry.session, entry.depth))}</VStack>}
        </Box>}
        {selectedSession && <Flex mt="2" gap="1" wrap="wrap" flex="0 0 auto"><Button size="xs" variant="ghost" title={t.forkSession} aria-label={t.forkSession} disabled={busy || selectedSession.active} onClick={() => void forkSelectedSession()}><GitFork size={15} /></Button><Button size="xs" variant="ghost" title={t.rewindSession} aria-label={t.rewindSession} disabled={busy || selectedSession.active} onClick={() => void openRewindDialog()}><History size={15} /></Button><Button size="xs" variant="ghost" title={t.exportSession} aria-label={t.exportSession} disabled={busy} onClick={() => void exportSelectedSession()}><Download size={15} /></Button></Flex>}
        {rewindNotice && <Text mt="1" fontSize="xs" color="var(--text-dim)">{rewindNotice}</Text>}
      </Flex>
      <Text as="footer" mt="5" pt="3" borderTop="1px solid" borderColor="var(--bg-panel-hover)" color="var(--text-ghost)" fontSize="2xs" textAlign="right">{t.version}: {version ? `v${version}` : "—"}</Text>
      {/* closeOnInteractOutside 必须显式写：zag 对 role="alertdialog" 默认关掉外部点击，
          于是这个框只认 Esc，点旁边的输入框什么也不发生。删除仍要点「删除」才发生，
          点外面等同取消，没有误删风险。 */}
      <Dialog.Root role="alertdialog" closeOnInteractOutside open={deleteTarget !== null} onOpenChange={(details) => { if (!details.open) setDeleteTarget(null); }}>
        <Portal>
          <Dialog.Backdrop bg="var(--shadow)" />
          <Dialog.Positioner>
            <Dialog.Content bg="var(--bg-raised)" color="var(--text)" border="1px solid" borderColor="var(--border)" borderRadius="12px" maxW="360px">
              <Dialog.Header><Dialog.Title fontSize="md">{t.deleteDialogTitle}</Dialog.Title></Dialog.Header>
              <Dialog.Body>
                <Text fontWeight="600" mb="2" overflow="hidden" textOverflow="ellipsis" whiteSpace="nowrap">{deleteTarget?.title}</Text>
                <Text fontSize="sm" color="var(--text-dim)">{t.deleteConfirm}</Text>
              </Dialog.Body>
              <Dialog.Footer gap="2">
                <Button size="sm" variant="outline" borderColor="var(--border-strong)" color="var(--text)" onClick={() => setDeleteTarget(null)}>{t.cancel}</Button>
                <Button size="sm" colorPalette="red" onClick={() => void confirmDeleteSession()}>{t.confirmDelete}</Button>
              </Dialog.Footer>
            </Dialog.Content>
          </Dialog.Positioner>
        </Portal>
      </Dialog.Root>
      <Dialog.Root role="alertdialog" closeOnInteractOutside open={rewindPoints !== null} onOpenChange={(details) => { if (!details.open) setRewindPoints(null); }}>
        <Portal>
          <Dialog.Backdrop bg="var(--shadow)" />
          <Dialog.Positioner>
            <Dialog.Content bg="var(--bg-raised)" color="var(--text)" border="1px solid" borderColor="var(--border)" borderRadius="12px" maxW="480px">
              <Dialog.Header><Dialog.Title fontSize="md">{t.rewindDialogTitle}</Dialog.Title></Dialog.Header>
              <Dialog.Body>
                {rewindLoading && <Text fontSize="sm" color="var(--text-dim)">{t.rewindLoading}</Text>}
                {!rewindLoading && rewindPoints && rewindPoints.length === 0 && <Text fontSize="sm" color="var(--text-dim)">{t.rewindNoPoints}</Text>}
                {!rewindLoading && rewindPoints && rewindPoints.length > 0 && <VStack align="stretch" gap="1" maxH="40vh" overflowY="auto">
                  {rewindPoints.map((point, index) => <label key={point.turn_id ?? "beginning"} className="rewind-point" style={{ display: "flex", gap: "8px", alignItems: "baseline", cursor: "pointer" }}>
                    <input type="radio" name="rewind-point" checked={rewindSelection === index} onChange={() => { setRewindSelection(index); setRewindFiles(point.can_restore_workspace); }} />
                    <Text as="span" fontWeight="600" whiteSpace="nowrap">{point.step === 0 ? t.rewindBeginning : t.rewindStep.replace("{step}", String(point.step))}</Text>
                    <Text as="span" fontSize="xs" color="var(--text-dim)" whiteSpace="nowrap">{point.can_restore_workspace ? t.rewindFilesAvailable : t.rewindChatOnly}</Text>
                    <Text as="span" fontSize="sm" overflow="hidden" textOverflow="ellipsis" whiteSpace="nowrap">{point.snippet}</Text>
                  </label>)}
                </VStack>}
                {!rewindLoading && rewindPoints && rewindPoints[rewindSelection] && <Box mt="3">
                  <label style={{ display: "flex", gap: "8px", alignItems: "center", cursor: rewindPoints[rewindSelection].can_restore_workspace ? "pointer" : "not-allowed" }}>
                    <input type="checkbox" checked={rewindFiles && rewindPoints[rewindSelection].can_restore_workspace} disabled={!rewindPoints[rewindSelection].can_restore_workspace} onChange={(event) => setRewindFiles(event.target.checked)} />
                    <Text as="span" fontSize="sm">{rewindPoints[rewindSelection].can_restore_workspace ? t.rewindRestoreFiles : t.rewindRestoreUnavailable}</Text>
                  </label>
                  <Text mt="2" fontSize="xs" color="var(--text-dim)">{t.rewindDropHint.replace("{count}", String(rewindPoints.length - rewindSelection))}</Text>
                </Box>}
              </Dialog.Body>
              <Dialog.Footer gap="2">
                <Button size="sm" variant="outline" borderColor="var(--border-strong)" color="var(--text)" onClick={() => setRewindPoints(null)}>{t.cancel}</Button>
                <Button size="sm" colorPalette="orange" disabled={rewindLoading || !rewindPoints || !rewindPoints[rewindSelection]} onClick={() => void confirmRewind()}>{t.rewindConfirm}</Button>
              </Dialog.Footer>
            </Dialog.Content>
          </Dialog.Positioner>
        </Portal>
      </Dialog.Root>
    </Box>
    <Flex flex="1" minW="0" className="conversation-with-schedules">
    <header className="conversation-toolbar"><button className="mobile-session-toggle" aria-label={t.session} title={t.session} aria-expanded={mobileSessionsOpen} aria-controls="chat-session-sidebar" onClick={() => setMobileSessionsOpen((open) => !open)}><PanelLeft size={18} /></button><div className="conversation-location"><span className="conversation-project"><FolderGit2 size={14} />{workspaces.find((item) => item.path === workspace)?.name || t.appName}</span>{selectedSession?.started_by && <span className="session-lineage">{t.sessionLineage.replace("{parent}", sessions.find((item) => item.id === selectedSession.started_by)?.title || selectedSession.started_by.slice(0, 8)).replace("{time}", lineageTime(selectedSession.created_at, t))}</span>}<h1>{selectedSession?.title || t.newSession}</h1></div><div className="conversation-tools">
      {sessionId && <SessionCollaboration key={`peers-${sessionId}`} sessionId={sessionId} language={language} onOpenSession={(id) => void loadSession(id)} />}
      {sessionId && !childReadOnly && <SharePanel key={`share-${sessionId}`} sessionId={sessionId} language={language} open={shareOpen} onOpenChange={setShareOpen} />}
      {sessionId && <ConversationSchedules key={sessionId} sessionId={sessionId} workspace={selectedSession?.workspace ?? workspace} workspaces={workspaces} language={language} messages={t} onOpenSession={(id, turnId) => void loadSession(id, turnId ?? undefined)} />}
    </div></header>
    <Container className="conversation-stage" flex="1" minW="0" minH="0" maxW="860px" px={{ base: "4", md: "8" }} py="6" display="flex" flexDir="column">
      {sessionId && childParentId && <ChildSessionOrigin key={`origin-${sessionId}`} parentId={childParentId} parentTitle={sessions.find((item) => item.id === childParentId)?.title} childId={sessionId} readOnly={childReadOnly} detached={childDetached} language={language} onOpenSession={openRelatedSession} onRelationshipChanged={childRelationshipChanged} />}
      <Box ref={chatViewportRef} className="chat-viewport" flex="1" minH="0" overflowY="auto" pb="10" onScroll={() => { const node = chatViewportRef.current; if (node) { const atBottom = node.scrollHeight - node.scrollTop - node.clientHeight < 80; followBottomRef.current = atBottom; if (atBottom) { if (chatRounds !== CHAT_WINDOW_ROUNDS) setChatRounds(CHAT_WINDOW_ROUNDS); } else if (node.scrollTop < 24 && hiddenRounds > 0) loadEarlierChat(); } }}>{!chat.length && <Box className="chat-welcome" py="24"><span className="welcome-icon"><MessageSquare size={25} /></span><Heading size="2xl" mb="4">{t.welcomeTitle}</Heading><Text color="var(--text-dim)">{t.welcomeBody}</Text></Box>}
        <VStack className="chat-timeline" align="stretch" gap="3">{hiddenRounds > 0 && <button type="button" className="chat-archive" onClick={loadEarlierChat}>{t.chatArchive.replace("{count}", String(hiddenRounds))}</button>}{groupedChat.map((message, index, visible) => message.role === "workflow" && message.workflow ? <WorkflowCard key={message.id} workflow={message.workflow} language={language} messages={t} canControl={identity !== null && workspacePolicy !== "read_only"} /> : message.role === "plan" || message.role === "system" ? <ConversationCard key={message.id} plan={message.plan} details={message.details} messages={t} /> : message.role === "divider" ? <Text key={message.id} className="turn-divider">{turnDividerText(message.content, t.yourTurn, index === visible.length - 1)}</Text> : message.role === "status" && message.concise ? <Text key={message.id} className="concise-status" role="status"><span className="thinking-pulse" aria-hidden="true" />{message.concise.text} · {formatStepElapsed(Math.max(0, conciseNow - message.concise.startedAt))}</Text> : message.role === "activity" ? <Box key={message.id}><RunCard steps={message.steps ?? []} messages={t} />{message.thought?.text && <ThoughtCard thought={message.thought} messages={t} />}{message.content &&<Box className="message assistant">{messageModelLabel(message.model, t)}<Markdown content={message.content} copy={markdownCopy(t)} /></Box>}</Box>: <Box key={message.id} className={`message ${message.role}`}>{message.role === "assistant" ? <>{messageModelLabel(message.model, t)}<Markdown content={message.content} copy={markdownCopy(t)} /></> : <UserMessage key={sessionId} content={message.content} messages={t} onToggle={() => { followBottomRef.current = false; }} />}{message.attachmentReference && sessionId && <SentAttachments sessionId={sessionId} reference={message.attachmentReference} language={language} onCopy={(item) => appendAttachment(attachmentPayload(item))} />}</Box>)}</VStack>
        <RuntimeHumanGates gates={visibleHumanGates} messages={t} language={language} onApproval={resolveRuntimeApproval} onAnswer={answerRuntimeQuestion} />
        <IndependentTasks sessionId={sessionId || null} language={language} />
        {sessionId && !childParentId && !childReadOnly && <ChildSessions key={`children-${sessionId}`} sessionId={sessionId} language={language} onOpenSession={openRelatedSession} onRelationshipChanged={childRelationshipChanged} />}
        {error && <Box className="request-error"><Text role="alert">{error}</Text>{runtimeFailure && <RuntimeFailureDetails key={`${runtimeFailure.sessionId}:${runtimeFailure.taskId}`} sessionId={runtimeFailure.sessionId} taskId={runtimeFailure.taskId} messages={t} />}</Box>}<div ref={endRef} />
      </Box>
      {/* 聊天正文选中气泡。插件拿到的 `text` 是用户真正看到的那段字，
          `source` 固定 "chat.selection"，与 macOS 宿主传的两个参数一致。 */}
      {chatSelection && chatSelectionEntries.length > 0 && (
        <PluginMenuPopup
          entries={chatSelectionEntries}
          x={chatSelection.x}
          y={chatSelection.y}
          onRun={(entry) => void runMenuEntry(entry, { text: chatSelection.text, source: "chat.selection" })}
          onClose={() => setChatSelection(null)}
        />
      )}
      <RuntimeQueue key={sessionId ?? "new"} sessionId={sessionId || null} language={language} />
      {selectionNotice && <Text className="selection-notice" role="status">{selectionNotice}</Text>}
      {submissionNotice?.sessionId === sessionId && <Text className="selection-notice" role="status">{submissionNotice.text}</Text>}
      <Box className="composer-shell">
        {references.query && <ComposerReferences entries={references.entries} selected={references.selected} loading={references.loading} messages={t} choose={chooseReference} />}
        {busy && needsActivityStrip(activity || t.thinking, chat.find((message) => message.id === activeRunRef.current)?.steps ?? [], t.thinking) && <Flex className="thinking-strip"><Box className="thinking-pulse" /><Text title={activity}>{activity || t.thinking}</Text></Flex>}
        {commandMatches.length > 0 && <Box className="suggestions"><Text className="suggestion-title">{t.commands}</Text>{commandMatches.map((command) => <button key={command} type="button" onMouseDown={(event) => { event.preventDefault(); if (command === "/permissions" || command === "/model") void send(command); else setPrompt(command); }}>{command}</button>)}</Box>}
        {skillQuery !== undefined && !skillPickerDismissed && <Box className="suggestions"><Text className="suggestion-title">{t.skills}</Text><Input className="skill-search" size="sm" value={skillSearch} onChange={(event) => setSkillSearch(event.target.value)} placeholder={t.searchSkills} aria-label={t.searchSkills} />{skillMatches.length ? skillMatches.map((skill, index) => <button key={skill.identifier} type="button" aria-selected={index === skillSelected} onMouseDown={(event) => { event.preventDefault(); chooseSkill(skill); }}><strong>${skill.identifier}</strong><small>{skill.name} · {skill.description}</small></button>) : <Text className="suggestion-empty">{t.noSkills}</Text>}</Box>}
        {enqueueButton.visible && <button type="button" className="queue-enqueue" title={enqueueButton.enabled ? queueText("enqueue", language) : queueText("enqueueNeedsSession", language)} disabled={!enqueueButton.enabled} onClick={() => { void enqueueDraft(); }}><ListPlus size={15} />{queueText("enqueue", language)}</button>}
        <AttachmentManager items={attachments} onChange={setAttachments} language={language} />
        {ghostSuggestion && <Box className="input-suggestion" aria-hidden="true" pt={attachments.length ? "2" : "4"}><Text as="span">{ghostSuggestion}</Text><Text as="span" className="input-suggestion-hint">{t.inputSuggestionAccept}</Text></Box>}
        <Textarea ref={promptRef} value={prompt} onPaste={handlePaste} onChange={(event) => { setPrompt(event.target.value); setComposerCursor(event.target.selectionStart); setSkillPickerDismissed(false); references.update(event.target.value, event.target.selectionStart); }} onSelect={(event) => { const field = event.currentTarget; setComposerCursor(field.selectionStart); setSkillPickerDismissed(false); if (!references.query || field.selectionStart !== references.query.cursor) references.update(field.value, field.selectionStart); }} onKeyDown={(event) => {
          if (event.nativeEvent.isComposing) return;
          if (skillMention && !skillPickerDismissed) {
            if (event.key === "Escape") { event.preventDefault(); setSkillPickerDismissed(true); setSkillSearch(""); return; }
            if (skillMatches.length && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
              event.preventDefault();
              setSkillSelected((current) => (current + (event.key === "ArrowUp" ? -1 : 1) + skillMatches.length) % skillMatches.length);
              return;
            }
            if (skillMatches.length && ["Enter", "Tab"].includes(event.key) && !event.shiftKey) {
              event.preventDefault();
              chooseSkill(skillMatches[skillSelected]);
              return;
            }
          }
          if (references.query) {
            if (event.key === "Escape") { event.preventDefault(); references.dismiss(); return; }
            if (references.entries.length && ["ArrowUp", "ArrowDown", "Tab", "Enter"].includes(event.key) && !event.shiftKey) {
              event.preventDefault();
              if (event.key === "ArrowUp" || event.key === "ArrowDown") references.setSelected((references.selected + (event.key === "ArrowUp" ? -1 : 1) + references.entries.length) % references.entries.length);
              else chooseReference(references.entries[references.selected]);
              return;
            }
            if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); return; }
          }
          if (event.key === "Tab" && !event.shiftKey && ghostSuggestion) { event.preventDefault(); setPrompt(ghostSuggestion); inputSuggestion.accept(); return; }
          if (event.key === "Escape" && ghostSuggestion) { event.preventDefault(); inputSuggestion.clear("dismissed"); return; }
          // ⌘Enter（macOS）与 Ctrl+Enter 都是「先入队、以后再发」，普通 Enter 仍立即发送。
          if (event.key === "Enter" && !event.shiftKey && (event.metaKey || event.ctrlKey)) { event.preventDefault(); void enqueueDraft(); return; }
          if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); void send(); }
        }} aria-description={ghostSuggestion ? `${ghostSuggestion} · ${t.inputSuggestionAccept}` : undefined} placeholder={ghostSuggestion ? "" : busy ? t.steerPlaceholder : t.promptPlaceholder} minH="104px" maxH="240px" resize="vertical" border="0" outline="none" lineHeight="1.5" _focus={{ boxShadow: "none", outline: "none" }} _focusVisible={{ boxShadow: "none", outline: "none" }} px="4" pt={attachments.length ? "2" : "4"} pb="12" />
        {composerEntries.length > 0 && (
          <button
            type="button"
            className="composer-more"
            title={t.pluginComposerMore}
            aria-label={t.pluginComposerMore}
            onClick={(event) => {
              const rect = event.currentTarget.getBoundingClientRect();
              setPopup({ entries: composerEntries, x: rect.left, y: rect.top - 8, args: {} });
            }}
          >
            <SfIcon name="sf:plus.circle" size={16} />
          </button>
        )}
        <Text className={`send-hint ${busy ? "busy" : "idle"}`}>{busy ? attachments.length ? queueText("composerHint", language) : t.hintBusy : t.hintIdle}</Text>
        <Button aria-label={busy ? t.stop : t.send} title={busy ? t.stop : t.send} className={`send-button ${busy ? "stop" : ""}`} onClick={busy ? () => void stop() : () => void send()} disabled={!busy && ((!prompt.trim() && !attachments.length) || selectedSession?.archived)}>{busy ? <Box className="stop-icon" /> : <Text className="send-icon">↑</Text>}</Button>
      </Box>
      <div className="composer-control-row">
        <div className="composer-control-items">
          {/* 还没建会话也要能选：这两个面板在没有会话时只存草稿，跟着第一条消息一起交出去。 */}
          <ModelPicker key={`model-${sessionId || "draft"}`} sessionId={sessionId || null} language={language} current={sessionId ? sessionModel : draftModel?.model ?? null} currentProviderId={sessionId ? sessionModelProviderId : draftModel?.providerId ?? null} open={modelOpen} onOpenChange={setModelOpen} onPicked={(model, providerId) => { setSessionModel(model); if (sessionId) setSessionModelProviderId(providerId); else setDraftModel({ model, providerId }); }} onApplied={showSelectionNotice} />
          <ReasoningPicker key={`reasoning-${sessionId || "draft"}`} sessionId={sessionId || null} language={language} current={sessionId ? sessionReasoningLevel : null} currentProviderId={sessionId ? sessionModelProviderId : draftModel?.providerId ?? null} onChanged={setSessionReasoningLevel} onApplied={showSelectionNotice} />
          <PermissionsPanel key={`permissions-${sessionId || "draft"}`} sessionId={sessionId || null} language={language} messages={t} override={approvalMode} workspaceAccess={workspacePolicy} open={permissionsOpen} onOpenChange={(open) => { setPermissionsOpen(open); if (open && sessionId) void refreshApprovalMode(sessionId); }} onSaved={setApprovalMode} onApplied={showSelectionNotice} />
        </div>
        <div className="goal-mode-control">
          <label className="goal-mode-toggle">
            <input type="checkbox" checked={goalEnabled} onChange={(event) => updateGoalPreference(goal, event.target.checked)} />
            <span>{t.goalMode}</span>
          </label>
          <Input className="goal-mode-input" size="sm" value={goal} onChange={(event) => updateGoalPreference(event.target.value, goalEnabled)} placeholder={t.goalModePlaceholder} aria-label={t.goalMode} title={t.goalModeHint} disabled={!goalEnabled} />
        </div>
      </div>
      <QuickSettings messages={t} language={language} onLanguageChange={setLanguage} theme={theme} onThemeChange={setTheme} />
    </Container>
    </Flex>
    {pluginOverlays}
  </Flex></Box>;
}
