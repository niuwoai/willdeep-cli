use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::background::BackgroundTaskRegistry;
use crate::goal::{
    ContinuationDecision, ContinuationRung, GoalContinuation, RoundObservation, SoftStopReason,
};
use crate::provider::{Provider, ProviderError};
use crate::routing::{RoutingGuard, RoutingTier};
use crate::subagent::{SpawnAgentArgs, SubagentCatalog};
use crate::tools::{ToolError, ToolRegistry};
use crate::types::{Message, ToolCall, Usage, sanitize_tool_history};

mod context;
mod dispatch;
mod guidance;
mod instance_identity;
mod messaging;
mod parallel;
mod planning;
mod progress;
mod recovery;
mod streaming;
mod uncertain;

const MAX_INCOMPLETE_RESPONSES: usize = 3;

#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub max_turns: usize,
    pub system_prompt: String,
    pub context_window: u64,
    pub token_budget: Option<u64>,
}

#[derive(Default)]
pub struct AgentInstructionInbox {
    pending: Mutex<VecDeque<AgentInstruction>>,
}

impl std::fmt::Debug for AgentInstructionInbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 只报条数：插话原文是用户的话，不该跟着请求结构一起进日志。
        let pending = self
            .pending
            .lock()
            .map(|pending| pending.len())
            .unwrap_or(0);
        f.debug_struct("AgentInstructionInbox")
            .field("pending", &pending)
            .finish()
    }
}

/// 送进正在跑的 Agent 的插话，主循环每一小轮开始前排空——也就是「当前工具跑完、
/// 下一次调模型之前」，不打断手上的活。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentInstruction {
    /// 父 Agent 给子 Agent 的补充指令，以宿主口吻合并成一条注入。
    Parent(String),
    /// 用户在本轮进行中说的话，以用户身份逐条注入，回放时显示为 `You:`。
    Operator(String),
}

impl AgentInstruction {
    pub fn text(&self) -> &str {
        match self {
            Self::Parent(text) | Self::Operator(text) => text,
        }
    }
}

/// 单条插话的上限；再长就不是插话而是新任务了。
const MAX_INSTRUCTION_BYTES: usize = 16 * 1024;

impl AgentInstructionInbox {
    /// 父 Agent 的补充指令。
    pub fn push(&self, instruction: String) -> bool {
        self.enqueue(AgentInstruction::Parent, instruction)
    }

    /// 用户在本轮进行中的插话。
    pub fn push_operator(&self, text: String) -> bool {
        self.enqueue(AgentInstruction::Operator, text)
    }

    /// Inspect without consuming: only the model loop may take instructions
    /// so cancellation can still return undelivered operator messages.
    pub fn has_pending(&self) -> bool {
        self.pending
            .lock()
            .map(|pending| !pending.is_empty())
            .unwrap_or(false)
    }

    fn enqueue(&self, wrap: fn(String) -> AgentInstruction, text: String) -> bool {
        let text = text.trim();
        if text.is_empty() || text.len() > MAX_INSTRUCTION_BYTES {
            return false;
        }
        let Ok(mut pending) = self.pending.lock() else {
            return false;
        };
        pending.push_back(wrap(text.to_owned()));
        true
    }

    /// 取走全部待送的插话。任务结束后还剩下的，就是没赶上这一轮的。
    pub fn drain(&self) -> Vec<AgentInstruction> {
        self.pending
            .lock()
            .map(|mut pending| pending.drain(..).collect())
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeaseOutcome {
    /// 事件已经进到会留下来的对话里。
    Delivered,
    /// 这一轮整个作废，事件没被任何人看过。
    Failed,
}

#[derive(Clone, Debug)]
pub enum AgentEvent {
    /// A durable runtime-derived plan update; no model/tool call is fabricated.
    PlanUpdated(crate::conversation::Plan),
    /// Host-persisted graph state, emitted after the workflow ledger commits.
    WorkflowUpdated(Box<willdeep_runtime_protocol::workflow::WorkflowRun>),
    /// Model loop returned while background work remains active.
    BackgroundWait {
        waiting: bool,
    },
    TurnStarted {
        turn: usize,
    },
    /// 宿主签发的抢占事件取消了这一轮的 provider 请求。转录与工具记录都还在，
    /// 下一轮从保留的上下文继续——用户看到的应该是「插了一件急事」，不是
    /// 「刚才那步白做了」。
    TurnPreempted {
        turn: usize,
    },
    AssistantText(String),
    ProviderProgress(crate::provider::ProviderEvent),
    ToolRequested(ToolCall),
    ToolCompleted {
        call: ToolCall,
        output: String,
        is_error: bool,
    },
    Usage(Usage),
    /// Deterministic routing decision made by the runtime, before provider
    /// prompting. Deep events are emitted only after admission succeeds.
    RouteDecided {
        tier: RoutingTier,
        profile: Option<String>,
        confidence: u8,
        auto_dispatched: bool,
        reason: String,
        /// 实际干这份活的模型；说不上来（测试替身）时为 `None`。
        model: Option<String>,
        /// `model` 是兜底而不是本来要的那个时，说明原因。
        fallback: Option<String>,
    },
    /// 某一路没用上首选模型，退到了别的模型（或只放宽了预算没换模型）。
    /// 以前这些都是静默的：分类器候选失败、压缩 / 标题换下一家、要了贵一档
    /// 却没有绑定。
    ModelFallback {
        /// `classifier` / `compressor` / `title` / `worker_tier`。
        purpose: String,
        /// 没用上的那个（失败的候选模型，或没绑定的档位）。
        skipped: Option<String>,
        /// 实际用上的模型；`None` 表示没有可用的或说不上来。
        used: Option<String>,
        reason: String,
    },
    CompressionStarted {
        estimated_tokens: u64,
    },
    CompressionCompleted {
        estimated_tokens: u64,
        /// 摘要之后仍超窗时，从保留区头部丢掉的消息条数。丢弃只影响本次
        /// 请求视图，会话存档不受影响，但用户有权知道模型少看了几条。
        dropped_messages: usize,
    },
    BackgroundShellStarted {
        id: String,
    },
    BackgroundShellCompleted {
        id: String,
        status: crate::BackgroundTaskStatus,
        exit_code: Option<i32>,
        elapsed_millis: u64,
        output_bytes: usize,
    },
    /// Associates every event from a reusable worker with its durable attempt.
    RunScoped {
        run_id: uuid::Uuid,
        sequence: u64,
        event: Box<AgentEvent>,
    },
    SubagentQueued {
        id: uuid::Uuid,
        profile: String,
        model: Option<String>,
        label: String,
    },
    SubagentStarted {
        id: uuid::Uuid,
        profile: String,
        model: Option<String>,
        label: String,
        background: bool,
        max_turns: usize,
        token_budget: Option<u64>,
        timeout_seconds: Option<u64>,
        workspace: std::path::PathBuf,
        root_workspace: std::path::PathBuf,
        worktree_branch: Option<String>,
        dedicated_worktree: bool,
    },
    SubagentCompleted {
        id: uuid::Uuid,
        status: SubagentLifecycleStatus,
        report: Option<String>,
    },
    SubagentTurnStarted {
        id: uuid::Uuid,
        turn: usize,
    },
    SubagentToolRequested {
        id: uuid::Uuid,
        name: String,
    },
    SubagentToolCompleted {
        id: uuid::Uuid,
        name: String,
        is_error: bool,
    },
    SubagentUsage {
        id: uuid::Uuid,
        usage: Usage,
    },
    SubagentRetryWait {
        id: uuid::Uuid,
        attempt: u32,
        delay: std::time::Duration,
    },
    SubagentRetryStarted {
        id: uuid::Uuid,
        attempt: u32,
    },
    /// What a delegated run actually proved, emitted once per run whether it
    /// passed, failed or had no verifier at all.
    ///
    /// This is the only place `verified` is a fact rather than a claim: the
    /// verdict comes from the verifier's exit code, so a report that reads
    /// like success but never passed a check cannot be counted as one. With
    /// `repo_commit` for the tree the run started from, one record is a
    /// complete replay case — initial state, task, verdict.
    SubagentVerdict {
        id: uuid::Uuid,
        repo_commit: Option<String>,
        verifier_command: Option<String>,
        /// `None` when the run had no verifier: unverified, not failed.
        verifier_passed: Option<bool>,
        attempts: usize,
        /// Citations the runtime could check in a report-only run: file paths,
        /// line numbers and commit hashes the worker named. Zero means the
        /// report cited nothing checkable, which is not the same as a report
        /// whose every citation held up.
        claims_checked: usize,
        /// Cited locations that do not exist. A read-only trade has no exit
        /// code to judge it, and this is the one thing a program can still
        /// verify about its answer.
        claims_unverifiable: usize,
    },
    /// 目标未达，宿主拒绝了一次隐式收口并注入续推引导。
    GoalContinuationInjected {
        rung: ContinuationRung,
    },
    /// 预算耗尽，转入有序收尾。
    GoalBudgetLimited {
        reason: SoftStopReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentLifecycleStatus {
    Partial,
    Completed,
    Blocked,
    Cancelled,
    Failed,
}

#[async_trait]
pub trait EventSink: Send + Sync {
    async fn emit(&self, event: AgentEvent);

    /// Emit an event and return the sequence number a host durably recorded
    /// it under, if the host keeps one. The Runtime daemon returns the
    /// `events.ndjson` sequence so the usage ledger can link a live record
    /// to the event a later backfill would read; everyone else keeps the
    /// default and returns `None`. Forwarding sinks must forward this too.
    async fn emit_sequenced(&self, event: AgentEvent) -> Option<u64> {
        self.emit(event).await;
        None
    }
}

impl AgentEvent {
    pub fn unscoped(&self) -> &Self {
        match self {
            Self::RunScoped { event, .. } => event.unscoped(),
            _ => self,
        }
    }

    pub fn into_unscoped(self) -> Self {
        match self {
            Self::RunScoped { event, .. } => event.into_unscoped(),
            _ => self,
        }
    }
}

struct NoopSink;

#[async_trait]
impl EventSink for NoopSink {
    async fn emit(&self, _event: AgentEvent) {}
}

/// 一次 run 为什么停下来。长程模式下「停下来」有多种含义，调用方需要能区分。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentStopReason {
    /// 模型给出终稿且没有激活的目标——原有语义。
    #[default]
    Finished,
    /// 目标激活期间，模型显式声明目标达成。
    GoalComplete,
    /// 目标未达但预算耗尽，已按收尾引导产出交接快照。
    BudgetLimited,
    /// 轮次上限用尽，模型没来得及给终稿。`final_text` 是它最后一段可见文字，
    /// 只是部分结果；改好的文件、跑过的命令都还在，只是没收敛。
    MaxTurns,
    /// The provider stopped mid-response or filtered output; never a verified final answer.
    Incomplete,
    /// Workspace changes have no passing verification for their current snapshot.
    Unverified,
}

impl AgentStopReason {
    pub fn is_complete(self) -> bool {
        matches!(self, Self::Finished | Self::GoalComplete)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Finished => "finished",
            Self::GoalComplete => "goal_complete",
            Self::BudgetLimited => "budget_limited",
            Self::MaxTurns => "max_turns",
            Self::Incomplete => "incomplete",
            Self::Unverified => "unverified",
        }
    }
}

#[derive(Debug)]
pub struct AgentOutcome {
    pub final_text: String,
    pub turns: usize,
    pub messages: Vec<Message>,
    pub stop_reason: AgentStopReason,
    /// 本次运行累计的 token 用量（Provider 报了多少算多少）。
    /// 调用方拿它做用量展示与遥测，不必自己去挂 `AgentEvent::Usage` 收集器。
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// 从运行开始到首次可用响应的毫秒数：流式文本取首个非空文本增量，
    /// 非流式响应或纯工具调用取完整响应。重试提示与用量事件不算响应。
    /// 后续轮次不会覆盖该值；它包含首次请求前的上下文准备时间。
    pub first_response_millis: Option<u64>,
    /// 本次运行产出的压缩摘要，调用方应随会话一起落盘，下一次运行再交回来。
    /// 没有压缩就没有它；调用方原样存回即可。
    pub context_summary: Option<crate::context_summary::ContextSummary>,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error("provider returned neither text nor tool calls")]
    EmptyResponse,
    #[error("agent reached the maximum of {0} turns before producing a final answer")]
    MaxTurns(usize),
    #[error("agent exhausted its token budget of {budget} tokens (used {used})")]
    TokenBudgetExceeded { budget: u64, used: u64 },
    #[error(
        "provider usage is unknown under a bounded token budget; no proposed tools were executed"
    )]
    TokenBudgetUsageUnknown,
    #[error("Agent team budget blocked the request: {0}")]
    TeamBudgetBlocked(String),
    #[error("subagent blocked: {0}")]
    SubagentBlocked(String),
    #[error("subagent failed: {0}")]
    Subagent(String),
    #[error("subagent stopped with {reason:?} after {turns} rounds; partial result:\n{report}")]
    SubagentPartial {
        reason: AgentStopReason,
        turns: usize,
        report: String,
    },
    #[error("cannot persist agent execution checkpoint: {0}")]
    Checkpoint(String),
    #[error("cannot capture verification snapshot: {0}")]
    VerificationSnapshot(String),
    #[error("cannot load current instance identity: {0}")]
    InstanceIdentity(String),
    #[error(
        "protected instructions and task context need {estimated} tokens, but only {capacity} remain after reserving tool schemas and output; select a larger context window or explicitly narrow the task"
    )]
    ContextCapacity { estimated: u64, capacity: u64 },
}

pub struct Agent {
    /// 主模型。与「跟主模型一样」的那些兜底共用一格，`/model` 一换全跟着换。
    provider: crate::provider::MainModelHandle,
    tools: ToolRegistry,
    config: AgentConfig,
    instance_name_source: Option<Arc<instance_identity::InstanceNameSource>>,
    sink: Arc<dyn EventSink>,
    image_fallback: Option<(Arc<dyn Provider>, String)>,
    /// 上下文压缩的专用 Provider。bool 为真表示压缩指令由网关托管
    /// （some.im 的 someim-32b-compressor，服务端 replace 注入），
    /// 摘要请求只发裸转录；为假时仍携带行内压缩指令。
    compressors: Vec<(Arc<dyn Provider>, bool)>,
    /// 压缩器自己的上下文窗口。不绑就沿用会话窗口：两者通常是同一个 Provider，
    /// 分开算只会把原料切碎。绑了之后摘要请求按它封顶，本地压缩器窗口远小于
    /// 云端会话时不至于发出一个对方收不下的请求。
    compressor_window: Option<u64>,
    /// 会话标题摘要的专用 Provider。没绑就没有 L2 润色，列表停在 L1 派生标题。
    titlers: Vec<Arc<dyn Provider>>,
    /// 轮次结束后预测用户下一句的 Provider，与标题同一档。没绑就不预测。
    input_suggesters: Vec<Arc<dyn Provider>>,
    subagents: Option<Arc<SubagentCatalog>>,
    team_recovery: tokio::sync::OnceCell<()>,
    instruction_inbox: Option<Arc<AgentInstructionInbox>>,
    goal_continuation: Option<Arc<GoalContinuation>>,
    background_tasks: Option<Arc<BackgroundTaskRegistry>>,
    /// 小模型优先路由（`small_model_routing`）：只管关键词分类、自动下发与
    /// 引导。关掉它不影响下面的准入。
    routing: Option<Arc<RoutingGuard>>,
    /// 专家档的准入闸：票据校验、低档尝试的证据、每个 harness 的次数上限。
    /// 它**总是在**——以前它挂在 `routing` 上，路由开关一关，专家档就不要票
    /// 据了，贵模型的闸门跟一个与它无关的开关绑在了一起。
    admission: Arc<RoutingGuard>,
    /// 宿主事件内核。不挂等于本改动前的行为：没有边界投递，也没有抢占。
    kernel: Option<crate::kernel::EventKernel>,
    /// 本机用量账本。不挂就不记账，其余行为完全不变。
    usage_ledger: Option<crate::usage_ledger::UsageLedgerScope>,
    /// 本机反馈账本（RSI 强反馈）：工具失败、没收敛的运行。不挂就不记。
    feedback: Option<crate::feedback::FeedbackRecorder>,
    time_budget: Option<std::time::Duration>,
    inherited_deadline: Option<tokio::time::Instant>,
}

impl Agent {
    fn has_bounded_usage(&self) -> bool {
        self.config.token_budget.is_some()
            || self.goal_continuation.as_ref().is_some_and(|goal| {
                goal.default_budget().max_tokens.is_some() && goal.statement().is_some()
            })
    }
    pub fn new(provider: Arc<dyn Provider>, tools: ToolRegistry, config: AgentConfig) -> Self {
        Self {
            provider: crate::provider::MainModelHandle::new(provider),
            tools,
            config,
            instance_name_source: None,
            sink: Arc::new(NoopSink),
            image_fallback: None,
            compressors: Vec::new(),
            compressor_window: None,
            titlers: Vec::new(),
            input_suggesters: Vec::new(),
            subagents: None,
            team_recovery: tokio::sync::OnceCell::new(),
            // 根 Agent 也带收件箱：用户在本轮进行中说的话由此送达，不必等轮次结束。
            instruction_inbox: Some(Arc::new(AgentInstructionInbox::default())),
            goal_continuation: None,
            background_tasks: None,
            routing: None,
            admission: Arc::new(RoutingGuard::new(crate::RoutingPolicy::default())),
            kernel: None,
            usage_ledger: None,
            feedback: None,
            time_budget: None,
            inherited_deadline: None,
        }
    }

    /// 挂上反馈账本：每次工具失败、每次没收敛就停下的运行各记一行。
    pub fn with_feedback(mut self, recorder: crate::feedback::FeedbackRecorder) -> Self {
        self.feedback = recorder.is_enabled().then_some(recorder);
        self
    }

    pub fn feedback(&self) -> Option<&crate::feedback::FeedbackRecorder> {
        self.feedback.as_ref()
    }

    /// 挂上本机用量账本：主循环与上下文压缩的每次模型调用各记一行。
    ///
    /// 辅助用途的 Provider（标题、预测、看图兜底……）不在这里记，由宿主用
    /// [`crate::usage_ledger::UsageLedgerScope::auxiliary`] 包好再交进来。
    pub fn with_usage_ledger(mut self, scope: crate::usage_ledger::UsageLedgerScope) -> Self {
        self.usage_ledger = Some(scope);
        self
    }

    pub(crate) fn begin_model_call(
        &self,
        kind: crate::usage_ledger::UsageKind,
        provider: &dyn Provider,
    ) -> crate::usage_ledger::ModelCall {
        match &self.usage_ledger {
            Some(scope) => scope.begin(kind, provider.ledger_identity()),
            None => crate::usage_ledger::ModelCall::inert(),
        }
    }

    /// 主循环请求在账本上记哪一类：根 Agent 是 `main`，子 Agent 是 `subagent`。
    pub(crate) fn loop_usage_kind(&self) -> crate::usage_ledger::UsageKind {
        self.usage_ledger
            .as_ref()
            .map_or(crate::usage_ledger::UsageKind::Main, |scope| {
                scope.loop_kind()
            })
    }

    /// 一次模型调用收尾：有 usage 就发 `AgentEvent::Usage`（行为与以前相同），
    /// 再用事件宿主给的序号落账。没有 usage 的调用照样记一行，只是不发事件。
    pub(crate) async fn settle_model_call(
        &self,
        mut call: crate::usage_ledger::ModelCall,
        usage: Option<&Usage>,
        outcome: crate::usage_ledger::Outcome,
    ) {
        call.settle(usage, outcome);
        let sequence = match usage {
            Some(usage) => {
                self.sink
                    .emit_sequenced(AgentEvent::Usage(usage.clone()))
                    .await
            }
            None => None,
        };
        call.finish(sequence);
    }

    /// 主 Agent 工具注册表的审批档位句柄。前端改它，下一次工具调用即生效。
    pub fn approval_mode_handle(&self) -> crate::tools::SharedApprovalMode {
        self.tools.approval_mode_handle()
    }

    /// Switch future completions to another model without rebuilding the
    /// Agent's tools, approvals, subagents, or event sinks. Every provider
    /// obtained from [`Self::main_model`]'s follower switches with it.
    pub fn set_model(&self, model: &str) -> Result<(), ProviderError> {
        self.provider.set_model(model)
    }

    /// 主模型的共享句柄。宿主在组装 Agent 之前就要拿到它（Worker、压缩兜底
    /// 等在 Agent 之前建好），所以也可以先建句柄再用 [`Self::with_main_model`]
    /// 交进来。
    pub fn main_model(&self) -> crate::provider::MainModelHandle {
        self.provider.clone()
    }

    /// 用宿主先建好的主模型句柄替换 [`Self::new`] 里那一份。
    pub fn with_main_model(mut self, handle: crate::provider::MainModelHandle) -> Self {
        self.provider = handle;
        self
    }

    fn provider(&self) -> Result<Arc<dyn Provider>, ProviderError> {
        self.provider.current()
    }

    pub fn with_image_fallback(
        mut self,
        provider: Arc<dyn Provider>,
        label: impl Into<String>,
    ) -> Self {
        self.image_fallback = Some((provider, label.into()));
        self
    }

    /// Bind a dedicated provider for context-compression summaries. With
    /// `hosted_prompt` the fixed compression instruction lives on the some.im
    /// gateway (`someim-32b-compressor`, injected in replace mode) and the
    /// client sends only the bare transcript; without it the inline
    /// instruction is kept so a custom model never loses its task description.
    pub fn with_compressor(mut self, provider: Arc<dyn Provider>, hosted_prompt: bool) -> Self {
        self.compressors.push((provider, hosted_prompt));
        self
    }

    /// Bind context-compression candidates in preference order. A failed or
    /// empty local summary falls through to the hosted/session candidate.
    pub fn with_compressors(mut self, providers: Vec<(Arc<dyn Provider>, bool)>) -> Self {
        self.compressors = providers;
        self
    }

    /// Declare the context window the compressor accepts. Summaries are chunked
    /// and truncated against it instead of the session window, so a small local
    /// compressor never receives a prompt sized for the cloud model. Without
    /// it the session window is used, which is correct when both are the same
    /// endpoint.
    pub fn with_compressor_window(mut self, window: u64) -> Self {
        self.compressor_window = Some(window);
        self
    }

    /// Bind the provider used for the one-shot session-title summary. Left
    /// unbound the session keeps its deterministic first-prompt title — a
    /// missing title model degrades the label, never the conversation.
    pub fn with_titler(mut self, provider: Arc<dyn Provider>) -> Self {
        self.titlers.push(provider);
        self
    }

    /// Bind title candidates in preference order. Titling is decorative, so
    /// every failed candidate is skipped without affecting the conversation.
    pub fn with_titlers(mut self, providers: Vec<Arc<dyn Provider>>) -> Self {
        self.titlers = providers;
        self
    }

    /// Bind the candidates that predict the user's next message after a turn,
    /// in preference order. Unbound means the feature is off for this agent.
    pub fn with_input_suggesters(mut self, providers: Vec<Arc<dyn Provider>>) -> Self {
        self.input_suggesters = providers;
        self
    }

    pub fn input_suggestions_enabled(&self) -> bool {
        !self.input_suggesters.is_empty()
    }

    /// 预测用户的下一句。`payload` 来自 [`crate::input_suggestion::payload`]。
    /// 按候选顺序试，**第一个请求成功的就定案**——哪怕它说没有明显的下一步，
    /// 也不再换下一家重问：预测是装饰，不值得为它多花一次钱。
    pub async fn suggest_next_input(&self, payload: &str) -> Option<String> {
        crate::input_suggestion::predict_first(&self.input_suggesters, payload).await
    }

    /// 把第一轮问答压成一行短标题。没绑标题 Provider、调用失败或模型返回
    /// 垃圾时一律 `None`——标题是装饰，不值得为它中断任何东西。
    pub async fn summarize_title(&self, first_user: &str, first_assistant: &str) -> Option<String> {
        let mut skipped = Vec::new();
        for (index, provider) in self.titlers.iter().enumerate() {
            let model = crate::provider::provider_model(provider.as_ref())
                .unwrap_or_else(|| format!("candidate #{}", index + 1));
            if let Some(title) =
                crate::session_title::summarize(provider.clone(), first_user, first_assistant).await
            {
                self.report_model_fallback("title", &skipped, Some(model))
                    .await;
                return Some(title);
            }
            skipped.push((
                model,
                "request failed or the answer was not a usable title".to_owned(),
            ));
        }
        self.report_model_fallback("title", &skipped, None).await;
        None
    }

    /// 报一次「首选没用上、退到了下一家」。`skipped` 为空就是首选成功，不报。
    pub(crate) async fn report_model_fallback(
        &self,
        purpose: &str,
        skipped: &[(String, String)],
        used: Option<String>,
    ) {
        if skipped.is_empty() {
            return;
        }
        self.sink
            .emit(AgentEvent::ModelFallback {
                purpose: purpose.to_owned(),
                skipped: Some(
                    skipped
                        .iter()
                        .map(|(model, _)| model.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
                used,
                reason: skipped
                    .iter()
                    .map(|(model, reason)| format!("{model}: {reason}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            })
            .await;
    }

    pub fn with_subagents(mut self, catalog: Arc<SubagentCatalog>) -> Self {
        if !self.tools.delegation_read_only() {
            self.subagents = Some(catalog);
        }
        self
    }

    /// Per-run wall clock limit. Existing checkpoints retain interrupted work.
    pub fn with_time_budget(mut self, budget: std::time::Duration) -> Self {
        self.time_budget = Some(budget);
        self
    }

    pub(crate) fn with_execution_deadline(
        mut self,
        deadline: Option<tokio::time::Instant>,
    ) -> Self {
        self.inherited_deadline = deadline;
        self
    }

    pub fn with_event_sink(mut self, sink: Arc<dyn EventSink>) -> Self {
        self.sink = sink;
        self
    }

    pub fn with_instruction_inbox(mut self, inbox: Arc<AgentInstructionInbox>) -> Self {
        self.instruction_inbox = Some(inbox);
        self
    }

    /// 正在跑的这一轮的收件箱；进程内的界面直接往里塞用户插话。
    pub fn instruction_inbox(&self) -> Option<Arc<AgentInstructionInbox>> {
        self.instruction_inbox.clone()
    }

    /// 挂上宿主事件内核。
    ///
    /// 挂上之后，待投递事件会在**模型与工具的步骤边界**进入对话，宿主签发的
    /// critical 事件还能取消正在进行的 provider 步骤。不挂就完全不影响原有
    /// 行为——内核是加在主循环旁边的一层，不是替换。
    pub fn with_event_kernel(mut self, kernel: crate::kernel::EventKernel) -> Self {
        self.kernel = Some(kernel);
        self
    }

    /// 挂上跨 turn 共享的 Goal 续推句柄（long-horizon.v1 RA1）。
    ///
    /// 不挂等于关闭长程续推，`run_*` 的行为与本改动前完全一致。
    pub fn with_goal_continuation(mut self, continuation: Arc<GoalContinuation>) -> Self {
        if !self.tools.delegation_read_only() {
            self.goal_continuation = Some(continuation);
        }
        self
    }

    /// 前端据此在 `/goal` 变更时同步激活状态，无需额外穿参。
    pub fn goal_continuation(&self) -> Option<&Arc<GoalContinuation>> {
        self.goal_continuation.as_ref()
    }

    /// 让续推判定能看见后台任务：仍有后台任务在跑时，「本轮没调工具」不算卡死。
    pub fn with_background_tasks(mut self, tasks: Arc<BackgroundTaskRegistry>) -> Self {
        self.background_tasks = Some(tasks);
        self
    }

    /// Attach deterministic small-model routing and deep-tier admission.
    pub fn with_routing_guard(mut self, routing: Arc<RoutingGuard>) -> Self {
        self.admission = routing.clone();
        self.routing = Some(routing);
        self
    }

    /// 只换准入闸（例如按配置的专家档次数上限），不开小模型路由。
    /// `small_model_routing = false` 时宿主走这条：路由关了，票据照要。
    pub fn with_admission_guard(mut self, admission: Arc<RoutingGuard>) -> Self {
        self.admission = admission;
        self
    }

    fn record_goal_feedback(
        &self,
        continuation: &GoalContinuation,
        signal: crate::feedback::Signal,
        open: usize,
    ) {
        let (Some(feedback), Some(state)) = (&self.feedback, continuation.snapshot()) else {
            return;
        };
        let open = if signal == crate::feedback::Signal::GoalCompletionRejected {
            open
        } else {
            state.open_criteria().len()
        };
        feedback.record_goal(
            signal,
            open,
            u32::try_from(state.continuations).unwrap_or(u32::MAX),
            state.elapsed(),
        );
    }

    /// 还在跑的委派 Worker 数，目标完成门禁用。
    fn workers_in_flight(&self) -> usize {
        self.background_tasks
            .as_ref()
            .map(|tasks| tasks.running_subagents())
            .unwrap_or(0)
            .saturating_add(usize::from(self.workflows_pending()))
    }

    fn background_active(&self) -> bool {
        self.background_tasks
            .as_ref()
            .map(|tasks| tasks.has_pending_tasks())
            .unwrap_or(false)
            || self.workflows_pending()
    }

    pub async fn run(&self, prompt: impl Into<String>) -> Result<AgentOutcome, AgentError> {
        self.run_with_history(Vec::new(), prompt).await
    }

    pub async fn run_with_history(
        &self,
        messages: Vec<Message>,
        prompt: impl Into<String>,
    ) -> Result<AgentOutcome, AgentError> {
        self.run_with_history_message(messages, Message::user(prompt))
            .await
    }

    pub async fn run_with_history_message(
        &self,
        messages: Vec<Message>,
        user_message: Message,
    ) -> Result<AgentOutcome, AgentError> {
        self.run_checkpointed(messages, user_message, None).await
    }

    pub async fn run_checkpointed(
        &self,
        messages: Vec<Message>,
        user_message: Message,
        sink: Option<&dyn crate::checkpoint::CheckpointSink>,
    ) -> Result<AgentOutcome, AgentError> {
        self.run_checkpointed_with_summary(messages, user_message, sink, None)
            .await
    }

    /// 同 [`Self::run_checkpointed`]，但先把上一次运行留下的压缩摘要交回来。
    ///
    /// 自动压缩不改写 durable 历史，所以没有这份摘要，每一轮都要把同一段前缀
    /// 从头再摘一遍；长会话里那是一次七八分钟的串行模型调用。摘要带着它覆盖的
    /// 前缀指纹，历史对不上就会被丢弃，最多浪费一次压缩。
    pub async fn run_checkpointed_with_summary(
        &self,
        messages: Vec<Message>,
        user_message: Message,
        sink: Option<&dyn crate::checkpoint::CheckpointSink>,
        summary: Option<crate::context_summary::ContextSummary>,
    ) -> Result<AgentOutcome, AgentError> {
        let mut preparation = crate::preparation::PreparationTrace::new("agent_preflight");
        let _execution_guard = sink
            .map(|sink| sink.acquire_run())
            .transpose()
            .map_err(AgentError::Checkpoint)?;
        if sink.is_some() {
            self.restore_team_for_execution().await?;
        }
        preparation.mark("restore_team");
        let mut recorder = crate::checkpoint::CheckpointRecorder::new(sink);
        let deadline = self
            .time_budget
            .map(|budget| {
                tokio::time::Instant::now()
                    .checked_add(budget)
                    .ok_or_else(|| {
                        AgentError::Checkpoint(
                            "Configured run time budget exceeds the monotonic clock range".into(),
                        )
                    })
            })
            .transpose()?
            .or(self.inherited_deadline);
        self.tools.set_execution_deadline(deadline);
        if let Some(catalog) = &self.subagents {
            catalog.set_execution_deadline(
                self.time_budget
                    .zip(deadline)
                    .map(|(budget, deadline)| deadline - budget / 5),
            );
        }
        if sink.is_some() {
            self.tools.activate_workflows();
        }
        if deadline.is_some() {
            recorder.retain_for_deadline();
        }
        if let Some(routing) = &self.routing {
            routing.begin_run();
        }
        recorder.initialize_evidence(&self.tools)?;
        preparation.mark("evidence");
        recorder.initialize_patch_contract(&self.tools)?;
        preparation.mark("patch_contract");
        recorder.initialize_verification(
            self.tools
                .try_verification_baseline()
                .map_err(AgentError::VerificationSnapshot)?,
        )?;
        preparation.mark("verification_baseline");
        recorder.watch_goal(self.goal_continuation.as_deref());
        recorder.watch_background(self.background_tasks.as_deref());
        // 目标的计时只在跑的时候走：轮次之间、进程不在的时候都不算。
        if let Some(goal) = &self.goal_continuation {
            goal.resume();
        }
        let verifications_before = self.tools.verification_count();
        let feedback = self.feedback.as_ref().map(|feedback| {
            feedback
                .for_run(
                    self.provider
                        .current()
                        .ok()
                        .and_then(|provider| provider.ledger_identity()),
                )
                .with_execution_limits(self.config.max_turns, self.config.token_budget)
        });
        if let Some(feedback) = &feedback {
            feedback.record_run_started();
        }
        recorder.watch_runtime_parameters(
            feedback
                .as_ref()
                .and_then(|feedback| feedback.runtime_parameters())
                .cloned(),
        );
        // 摘要缓存跨 run 复用：轮内命中省一次调用，从会话文件读回来才省掉整轮
        // 重摘。它只在 run_inner 里被替换成更新的一份。
        let previous_summary = summary.clone();
        let mut summary_cache = summary.map(context::SummaryCache::restore);
        preparation.mark("feedback_and_summary");
        drop(preparation);
        let mut result = if let Some(deadline) = deadline {
            match tokio::time::timeout_at(
                deadline,
                self.run_inner(
                    messages,
                    user_message,
                    &mut recorder,
                    feedback.as_ref(),
                    &mut summary_cache,
                ),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Ok(recorder.deadline_outcome()),
            }
        } else {
            self.run_inner(
                messages,
                user_message,
                &mut recorder,
                feedback.as_ref(),
                &mut summary_cache,
            )
            .await
        };
        // 失败也照常上报：压缩钱已经花过，下次运行不该再买一遍。调用方只在
        // `Ok` 时落盘，所以这里只管把结果挂到 outcome 上。
        //
        // 超时/取消会把正在重摘的槽位掏空（`request_messages_accounted` 先
        // `take()` 再 await）；这时手上那份旧摘要仍然有效，不能拿 None 把会话
        // 里已经付过钱的摘要抹掉——预算型会话会因此每一轮都从头重摘，永远走不出
        // BudgetLimited。
        if let Ok(outcome) = &mut result {
            outcome.context_summary = summary_cache
                .as_ref()
                .map(context::SummaryCache::persist)
                .or(previous_summary);
        }
        if let Some(goal) = &self.goal_continuation {
            goal.pause();
        }
        if let (Some(feedback), Ok(outcome)) = (&feedback, &result)
            && !outcome.stop_reason.is_complete()
        {
            feedback.record_incomplete(
                outcome.stop_reason.as_str(),
                outcome.turns,
                &outcome.final_text,
            );
        }
        // 这次运行的验证结论进账本：失败链据此判结局，不只看用户怎么说。
        if let (Some(feedback), Ok(_)) = (&feedback, &result) {
            let (verification, ran) = self
                .tools
                .run_verification(recorder.verification_baseline(), verifications_before);
            feedback.record_run_verification(verification, ran);
        }
        if let Some(feedback) = &feedback {
            feedback.record_run_finished(match &result {
                Ok(outcome) => outcome.stop_reason.as_str(),
                Err(_) => "error",
            });
        }
        recorder.finish(&result)?;
        result
    }

    async fn run_inner(
        &self,
        mut messages: Vec<Message>,
        mut user_message: Message,
        checkpoint: &mut crate::checkpoint::CheckpointRecorder<'_>,
        feedback: Option<&crate::feedback::FeedbackRecorder>,
        compressed: &mut Option<context::SummaryCache>,
    ) -> Result<AgentOutcome, AgentError> {
        let mut preparation = crate::preparation::PreparationTrace::new("agent_context");
        let patch_baseline = checkpoint.patch_baseline().cloned();
        // Persisted history can come from older desktop bridges that retained
        // `role=tool` while losing the protocol IDs. Never let one malformed
        // historical item make every future turn fail at the Provider boundary.
        let uncertain_calls = uncertain::UncertainCalls::recover(&mut messages);
        sanitize_tool_history(&mut messages);
        if let Some((provider, label)) = &self.image_fallback {
            let image_count = user_message
                .attachments
                .iter()
                .filter(|value| matches!(value, crate::types::MessageAttachment::Image { .. }))
                .count();
            if image_count > 0 {
                let vision_prompt = Message::user_with_attachments(
                    "Describe each attached image for a coding agent that cannot see images directly. Include visible UI text, errors, file names, code, controls, and layout.",
                    user_message.attachments.clone(),
                );
                let description = provider.complete(&[vision_prompt], &[]).await?.content;
                user_message.content.push_str(&format!("\n\n[Image description generated by {label} for {image_count} attached image(s)]\n{description}"));
                user_message.attachments.retain(|value| {
                    !matches!(value, crate::types::MessageAttachment::Image { .. })
                });
            }
        }
        if let Some(goal) = goal_from_message(&user_message.content)
            && let Some(continuation) = &self.goal_continuation
        {
            continuation.activate(goal, continuation.default_budget());
        }
        preparation.mark("history_and_images");
        self.apply_runtime_route(&mut user_message).await;
        preparation.mark("routing");
        messages.retain(|message| message.role != crate::types::Role::System);
        messages.insert(0, Message::system(&self.config.system_prompt));
        let mut rules = crate::project_rules::ProjectRules::new(self.tools.workspace())
            .map_err(ToolError::Io)?;
        // The approval judge reads this as inert context: it decides whether
        // a bounded action is relevant to the current goal, never whether a
        // destructive one is permitted.
        self.tools.set_task_context(&user_message.content);
        messages.push(user_message);
        let mut definitions = self.tools.definitions();
        if self.tools.allows_agent_tool("update_plan") {
            definitions.push(crate::tools::update_plan_definition());
        }
        let mut plan = checkpoint.restore_plan()?.or_else(|| {
            crate::conversation::project(&messages, None)
                .into_iter()
                .rev()
                .find_map(|item| item.plan)
        });
        checkpoint.plan = plan.clone();
        let mut used_tokens = 0_u64;
        // 分别累计输入/输出，供 AgentOutcome 上报——`used_tokens` 是预算判定用的
        // 合计值，两者语义不同，不能互相顶替。
        let mut input_tokens = 0_u64;
        let mut output_tokens = 0_u64;
        // 自上次续推判定以来成功发起的工具调用数——续推判定的「进展证据」。
        let mut tools_since_check = 0_usize;
        let mut progress = progress::ProgressTracker::default();
        let mut blockers = progress::BlockedToolTracker::default();
        let mut exploration = guidance::ExplorationGuidance::default();
        let mut incomplete_responses = 0_usize;
        let verification_baseline = checkpoint.verification_baseline().map(str::to_owned);
        let mut unverified_stops = 0_usize;
        // 流式文本在增量到达时计时；无文本增量的响应在完整返回时计时。
        let mut first_response_millis: Option<u64> = None;
        let run_started = std::time::Instant::now();
        preparation.mark("rules_and_definitions");
        let mut preparation = Some(preparation);
        for turn in 1..=self.config.max_turns {
            let plan_changed = self.tools.reconcile_plan_jobs(&mut plan);
            checkpoint.plan = plan.clone();
            rules.refresh().map_err(ToolError::Io)?;
            messages[0].content = format!(
                "{}\n\n{}\n\n{}\n\n{}",
                self.config.system_prompt,
                rules.render(),
                self.tools.required_verification_prompt(),
                self.tools.completion_contract_prompt()
            );
            self.append_instance_identity(&mut messages[0].content)?;
            if self.tools.allows_agent_tool("update_plan") {
                messages[0].content.push_str("\n\n");
                messages[0]
                    .content
                    .push_str(crate::planning::SCHEDULING_CONTRACT);
                if let Some(plan) = &plan {
                    let state = crate::planning::Receipt::new(plan.clone(), false);
                    messages[0].content.push_str(&format!(
                        "\n<session-plan>{}</session-plan>",
                        serde_json::to_string(&state).expect("plan serializes")
                    ));
                }
            }
            // 目标状态钉在 system 消息里：每轮重建、不进压缩摘要，所以目标、
            // 验收清单与在飞 Worker 永远不会被摘要掉。
            if let Some(pin) = self
                .goal_continuation
                .as_ref()
                .and_then(|goal| goal.pin(self.workers_in_flight()))
            {
                messages[0].content.push_str("\n\n");
                messages[0].content.push_str(&pin);
            }
            messages[0].content.push_str("\n\n");
            messages[0].content.push_str(&guidance::budget_context(
                used_tokens,
                self.config.token_budget,
                turn,
                self.config.max_turns,
            ));
            if let Some(remaining) = self.tools.remaining_execution_time() {
                messages[0].content.push_str(&format!("\n[run-time-budget] Remaining wall clock: {} ms. Model waits and tools consume this same budget; worker deadlines reserve the final 20% for parent integration.", remaining.as_millis()));
                if self
                    .time_budget
                    .is_some_and(|budget| remaining <= budget / 5)
                {
                    messages[0].content.push_str(" Begin bounded wrap-up now: inspect retained worker reports and actual parent diff, integrate explicitly, preserve partial work, and run short target/regression checks. Do not launch broad suites or new delegation; do not weaken tests or claim unverified completion.");
                }
            }
            self.append_pending_instructions(&mut messages);
            let workflow_reports = self.append_workflow_reports(&mut messages)?;
            if let Some(preparation) = preparation.as_mut() {
                preparation.mark("rules_and_reports");
            }
            let (peer_message_ids, _) = self.tools.append_peer_context(&mut messages)?;
            if let Some(preparation) = preparation.as_mut() {
                preparation.mark("peer_context");
            }
            checkpoint.record(&messages, turn, input_tokens, output_tokens)?;
            for run_id in workflow_reports {
                self.tools.acknowledge_workflow_report(run_id)?;
            }
            // 事件在这里进对话，而不是在工具与工具之间：一次 assistant 的
            // tool_calls 必须紧跟它那批 tool 结果，中间插一条用户消息会把这个
            // 配对拆散。turn 顶部就是「当前模型或工具步骤已经结束」那个边界。
            if plan_changed && let Some(plan) = &plan {
                self.sink.emit(AgentEvent::PlanUpdated(plan.clone())).await;
            }
            let leases = self.append_kernel_events(&mut messages);
            if !leases.is_empty()
                && let Err(error) = checkpoint.record(&messages, turn, input_tokens, output_tokens)
            {
                self.settle_leases(&leases, LeaseOutcome::Failed);
                return Err(error);
            }
            if let Some(mut preparation) = preparation.take() {
                preparation.mark("first_checkpoint");
            }
            self.sink.emit(AgentEvent::TurnStarted { turn }).await;
            if let Some(scope) = &self.usage_ledger {
                scope
                    .check_budget()
                    .map_err(AgentError::TeamBudgetBlocked)?;
            }
            let prepared = self
                .request_messages_accounted(&messages, compressed, &mut |usage| {
                    input_tokens = input_tokens.saturating_add(usage.input_tokens.unwrap_or(0));
                    output_tokens = output_tokens.saturating_add(usage.output_tokens.unwrap_or(0));
                    let call_tokens = usage.billable_tokens();
                    used_tokens = used_tokens.saturating_add(call_tokens);
                    if let Some(goal) = &self.goal_continuation {
                        goal.record_tokens(call_tokens);
                    }
                    checkpoint.record(&messages, turn, input_tokens, output_tokens)?;
                    if self.has_bounded_usage()
                        && usage.total_tokens.is_none()
                        && (usage.input_tokens.is_none() || usage.output_tokens.is_none())
                    {
                        return Err(AgentError::TokenBudgetUsageUnknown);
                    }
                    if let Some(budget) = self.config.token_budget
                        && used_tokens >= budget
                    {
                        return Err(AgentError::TokenBudgetExceeded {
                            budget,
                            used: used_tokens,
                        });
                    }
                    Ok(())
                })
                .await;
            let request_messages = match prepared {
                Ok(messages) => messages,
                Err(error) => {
                    self.settle_leases(&leases, LeaseOutcome::Failed);
                    return Err(error);
                }
            };
            if let Some(scope) = &self.usage_ledger {
                scope
                    .check_budget()
                    .map_err(AgentError::TeamBudgetBlocked)?;
            }
            // Record only admitted requests, never a call refused by the budget.
            let mut model_call = Some(match self.provider() {
                Ok(provider) => self.begin_model_call(self.loop_usage_kind(), provider.as_ref()),
                Err(_) => crate::usage_ledger::ModelCall::inert(),
            });
            let stream = streaming::StreamEvents::new(
                checkpoint,
                self.sink.as_ref(),
                turn,
                input_tokens,
                output_tokens,
            )
            .with_plan_tools(&self.tools);
            let response = match stream
                .wait(self.complete_or_preempt(&request_messages, &definitions, &stream))
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    drop(stream);
                    self.settle_leases(&leases, LeaseOutcome::Failed);
                    return Err(error);
                }
            };
            if first_response_millis.is_none() {
                first_response_millis = stream
                    .first_text_at()
                    .map(|at| at.saturating_duration_since(run_started).as_millis() as u64);
            }
            let streamed_partial = match stream.partial() {
                Ok(partial) => partial,
                Err(error) => {
                    drop(stream);
                    self.settle_leases(&leases, LeaseOutcome::Failed);
                    return Err(error);
                }
            };
            drop(stream);
            plan = checkpoint.plan.clone();
            let partial = match &response {
                Err(ProviderError::StreamInterrupted { partial, .. }) => partial.as_ref(),
                _ => &streamed_partial,
            };
            if !matches!(&response, Ok(Some(_))) {
                if !partial.content.is_empty() {
                    messages.push(Message::assistant(&partial.content, Vec::new()));
                }
                if let Some(usage) = &partial.usage {
                    input_tokens = input_tokens.saturating_add(usage.input_tokens.unwrap_or(0));
                    output_tokens = output_tokens.saturating_add(usage.output_tokens.unwrap_or(0));
                    used_tokens =
                        used_tokens.saturating_add(usage.total_tokens.unwrap_or_else(|| {
                            usage
                                .input_tokens
                                .unwrap_or(0)
                                .saturating_add(usage.output_tokens.unwrap_or(0))
                        }));
                }
                let outcome = if matches!(&response, Ok(None)) {
                    crate::usage_ledger::Outcome::Cancelled
                } else {
                    crate::usage_ledger::Outcome::Error
                };
                let call = model_call
                    .take()
                    .unwrap_or_else(crate::usage_ledger::ModelCall::inert);
                self.settle_model_call(call, partial.usage.as_ref(), outcome)
                    .await;
                checkpoint.record(&messages, turn, input_tokens, output_tokens)?;
            }
            let completion = match response {
                Ok(Some(completion)) => {
                    self.tools
                        .peer_context_consumed(&peer_message_ids, &request_messages)?;
                    self.settle_leases(&leases, LeaseOutcome::Delivered);
                    first_response_millis
                        .get_or_insert_with(|| run_started.elapsed().as_millis() as u64);
                    completion
                }
                Ok(None) => {
                    if let Some(budget) = self.config.token_budget
                        && used_tokens >= budget
                    {
                        self.settle_leases(&leases, LeaseOutcome::Delivered);
                        return Err(AgentError::TokenBudgetExceeded {
                            budget,
                            used: used_tokens,
                        });
                    }
                    // 被抢占：请求丢掉了，但事件文本已经在 transcript 里，
                    // 下一轮模型照样看得到，所以算投递成功。放回 pending 只会
                    // 让同一批事件再讲一遍。
                    self.settle_leases(&leases, LeaseOutcome::Delivered);
                    self.sink.emit(AgentEvent::TurnPreempted { turn }).await;
                    continue;
                }
                Err(error) => {
                    // 这一轮的 messages 会随着错误一起丢掉，事件也就没人看过。
                    self.settle_leases(&leases, LeaseOutcome::Failed);
                    return Err(error.into());
                }
            };
            let response_incomplete = completion.is_incomplete();
            // 思考型模型要求把这一轮的思维链随 assistant 消息回传，否则下一轮
            // 带着工具调用的历史会被上游整条拒掉。
            let reasoning = completion.reasoning.clone();
            let usage_known = completion.usage.as_ref().is_some_and(|usage| {
                usage.total_tokens.is_some()
                    || (usage.input_tokens.is_some() && usage.output_tokens.is_some())
            });
            if let Some(usage) = completion.usage {
                input_tokens = input_tokens.saturating_add(usage.input_tokens.unwrap_or(0));
                output_tokens = output_tokens.saturating_add(usage.output_tokens.unwrap_or(0));
                let call_tokens = usage.billable_tokens();
                used_tokens = used_tokens.saturating_add(call_tokens);
                // 目标的 token 预算跨轮次累计（RA3），与本轮的 token_budget 分开算。
                if let Some(goal) = &self.goal_continuation {
                    goal.record_tokens(call_tokens);
                }
                let call = model_call
                    .take()
                    .unwrap_or_else(crate::usage_ledger::ModelCall::inert);
                self.settle_model_call(call, Some(&usage), crate::usage_ledger::Outcome::Ok)
                    .await;
                let mut partial_history = messages.clone();
                if !completion.content.is_empty() {
                    partial_history.push(Message::assistant(&completion.content, Vec::new()));
                }
                checkpoint.record(&partial_history, turn, input_tokens, output_tokens)?;
                if let Some(budget) = self.config.token_budget
                    && used_tokens >= budget
                {
                    messages.push(Message::assistant(&completion.content, Vec::new()));
                    messages.push(Message::host_instruction("[budget-limited] The token budget was exhausted after this provider response. None of the tool calls proposed in that response were executed. Resume from the saved results and outstanding work when more budget is available."));
                    checkpoint.record(&messages, turn, input_tokens, output_tokens)?;
                    return Err(AgentError::TokenBudgetExceeded {
                        budget,
                        used: used_tokens,
                    });
                }
            } else if let Some(call) = model_call.take() {
                // 协议没报 usage：照样记一次调用，Token 留空，不估算。
                self.settle_model_call(call, None, crate::usage_ledger::Outcome::Ok)
                    .await;
            }
            if self.has_bounded_usage() && !usage_known {
                return Err(AgentError::TokenBudgetUsageUnknown);
            }
            // A completed answer may consume the last tokens. Refuse new side
            // effects here; the next loop/request still checks the same budget.
            if !completion.tool_calls.is_empty()
                && let Some(scope) = &self.usage_ledger
            {
                scope
                    .check_budget()
                    .map_err(AgentError::TeamBudgetBlocked)?;
            }
            let content = completion.content.trim().to_owned();
            if !content.is_empty() {
                self.sink
                    .emit(AgentEvent::AssistantText(content.clone()))
                    .await;
            }
            if response_incomplete {
                incomplete_responses += 1;
                // Truncated tool calls are not executable, even if their JSON happens to parse.
                messages.push(Message::assistant(&content, Vec::new()));
                if incomplete_responses >= MAX_INCOMPLETE_RESPONSES
                    || completion.finish_reason.as_deref() == Some("content_filter")
                {
                    return Ok(AgentOutcome {
                        final_text: content,
                        turns: turn,
                        messages,
                        stop_reason: AgentStopReason::Incomplete,
                        input_tokens,
                        output_tokens,
                        first_response_millis,
                        context_summary: None,
                    });
                }
                messages.push(Message::host_instruction("[response-incomplete] The provider ended the previous response before completion. No tool calls from that incomplete response were executed. Continue the outstanding work, using smaller complete steps. Do not treat the partial output as task completion."));
                continue;
            }
            incomplete_responses = 0;
            if completion.tool_calls.is_empty() {
                if content.is_empty() {
                    return Err(AgentError::EmptyResponse);
                }
                messages.push(Message::assistant(&content, Vec::new()));
                if self.append_pending_instructions(&mut messages)
                    || self.tools.append_peer_context(&mut messages)?.1
                {
                    continue;
                }
                let wrapping_up = self
                    .goal_continuation
                    .as_ref()
                    .is_some_and(|goal| goal.wrap_up_pending());
                if !wrapping_up
                    && let Some(feedback) = self
                        .tools
                        .completion_patch_feedback(patch_baseline.as_ref())
                        .or_else(|| {
                            self.tools
                                .completion_verification_feedback(verification_baseline.as_deref())
                        })
                {
                    unverified_stops += 1;
                    if unverified_stops >= MAX_INCOMPLETE_RESPONSES {
                        return Ok(AgentOutcome {
                            final_text: content,
                            turns: turn,
                            messages,
                            stop_reason: AgentStopReason::Unverified,
                            input_tokens,
                            output_tokens,
                            first_response_millis,
                            context_summary: None,
                        });
                    }
                    messages.push(Message::host_instruction(format!("[completion-verification-required] {feedback} Continue the outstanding task. Do not weaken checks or claim completion. If verification is unavailable, explain the limitation; the runtime will retain a partial result.")));
                    continue;
                }
                // 长程续推：目标未达且预算未尽时，这里不是终点。
                if let Some(continuation) = self.goal_continuation.clone() {
                    let observation = RoundObservation {
                        tools_executed: tools_since_check,
                        background_active: self.background_active(),
                        workers_in_flight: self.workers_in_flight(),
                    };
                    let was_wrapping_up = continuation.wrap_up_pending();
                    match continuation.evaluate(&content, observation) {
                        Some(ContinuationDecision::Continue { steering, rung }) => {
                            self.sink
                                .emit(AgentEvent::GoalContinuationInjected { rung })
                                .await;
                            messages.push(Message::host_instruction(steering));
                            tools_since_check = 0;
                            continue;
                        }
                        Some(ContinuationDecision::SoftStop { steering, reason }) => {
                            self.sink
                                .emit(AgentEvent::GoalBudgetLimited { reason })
                                .await;
                            messages.push(Message::host_instruction(steering));
                            tools_since_check = 0;
                            continue;
                        }
                        Some(ContinuationDecision::CompletionRejected {
                            steering,
                            open_criteria,
                            workers_in_flight,
                        }) => {
                            // 虚报完成：RSI 最想要的负样本之一。
                            self.record_goal_feedback(
                                &continuation,
                                crate::feedback::Signal::GoalCompletionRejected,
                                open_criteria.max(workers_in_flight),
                            );
                            messages.push(Message::host_instruction(steering));
                            tools_since_check = 0;
                            continue;
                        }
                        Some(ContinuationDecision::Complete) => {
                            self.record_goal_feedback(
                                &continuation,
                                if was_wrapping_up {
                                    crate::feedback::Signal::GoalBudgetLimited
                                } else {
                                    crate::feedback::Signal::GoalCompleted
                                },
                                0,
                            );
                            return Ok(AgentOutcome {
                                final_text: content,
                                turns: turn,
                                messages,
                                stop_reason: if was_wrapping_up {
                                    AgentStopReason::BudgetLimited
                                } else {
                                    AgentStopReason::GoalComplete
                                },
                                input_tokens,
                                output_tokens,
                                first_response_millis,
                                context_summary: None,
                            });
                        }
                        None => {}
                    }
                }
                return Ok(AgentOutcome {
                    final_text: content,
                    turns: turn,
                    messages,
                    stop_reason: AgentStopReason::Finished,
                    input_tokens,
                    output_tokens,
                    first_response_millis,
                    context_summary: None,
                });
            }
            messages.push(
                Message::assistant(content, completion.tool_calls.clone())
                    .with_reasoning(reasoning),
            );
            checkpoint.record(&messages, turn, input_tokens, output_tokens)?;
            let all_reads = completion
                .tool_calls
                .iter()
                .all(guidance::is_exploration_read);
            let mut novel_read = false;
            let mut rules_changed = false;
            let mut parallel_results = self
                .parallel_reads(&completion.tool_calls, &mut rules)
                .await;
            for call in completion.tool_calls {
                let previous_plan = plan.clone();
                let mut tool_plan = plan.clone();
                if parallel_results.is_none() {
                    self.sink
                        .emit(AgentEvent::ToolRequested(call.clone()))
                        .await;
                }
                let execution = async {
                    if blockers.refuses(&call) && parallel_results.is_none() {
                        Err(ToolError::HookDenied("This unchanged action failed three times without new successful evidence. It was not executed again. Diagnose the cause with a different bounded check, address the cause, or report the blocker; do not bypass approval or weaken verification.".into()))
                    } else if let Some(results) = parallel_results.as_mut() {
                        results.pop_front().expect("one result per parallel read")
                    } else {
                        match rules.before_call(&call) {
                            Ok(changed) => {
                                rules_changed |= changed;
                                if rules_changed {
                                    Err(ToolError::HookDenied("Applicable project instructions changed or a new directory scope was discovered. No action was executed. Read the refreshed system instructions and reissue the appropriate tool call on the next round.".to_owned()))
                                } else {
                                    if uncertain_calls.needs_approval(&call) {
                                        match self.tools.approve_uncertain_replay(&call).await {
                                            Ok(()) => {
                                                self.execute_planned_tool(&call, &mut tool_plan)
                                                    .await
                                            }
                                            Err(error) => Err(error),
                                        }
                                    } else {
                                        self.execute_planned_tool(&call, &mut tool_plan).await
                                    }
                                }
                            }
                            Err(error) => Err(ToolError::Io(error)),
                        }
                    }
                };
                let result = streaming::StreamEvents::new(
                    checkpoint,
                    self.sink.as_ref(),
                    turn,
                    input_tokens,
                    output_tokens,
                )
                .with_plan_tools(&self.tools)
                .wait(execution)
                .await?;
                self.tools.reconcile_plan_jobs(&mut tool_plan);
                plan = tool_plan;
                let (output, is_error) = match result {
                    Ok(output) => {
                        if let Some(feedback) = feedback {
                            feedback.record_tool_success(&call.name);
                        }
                        (output, false)
                    }
                    Err(error) => {
                        if let Some(feedback) = feedback {
                            feedback.record_tool_failure(&call.name, error.class());
                        }
                        (format!("tool error: {error}"), true)
                    }
                };
                let novel_progress = progress.observe(&call, &output, is_error);
                blockers.observe(&call, &output, is_error, novel_progress);
                if novel_progress {
                    tools_since_check = tools_since_check.saturating_add(1);
                    novel_read |= guidance::is_exploration_read(&call);
                }
                self.sink
                    .emit(AgentEvent::ToolCompleted {
                        call: call.clone(),
                        output: output.clone(),
                        is_error,
                    })
                    .await;
                checkpoint.plan = plan.clone();
                messages.push(Message::tool(&call, output));
                checkpoint.record(&messages, turn, input_tokens, output_tokens)?;
                if call.name != "update_plan"
                    && plan != previous_plan
                    && let Some(plan) = &plan
                {
                    self.sink.emit(AgentEvent::PlanUpdated(plan.clone())).await;
                }
            }
            if let Some(steering) = exploration.finish_round(all_reads, novel_read) {
                messages.push(Message::host_instruction(steering));
                checkpoint.record(&messages, turn, input_tokens, output_tokens)?;
            }
        }
        // 触顶不再判失败。此前这里返回 `AgentError::MaxTurns`，整轮标成失败，
        // 模型改好的十个文件、写到一半的结论一句都不给看，用户只看到一行
        // 「reached the maximum of N turns」。现在把最后一段可见文字当部分结果
        // 交出去，`stop_reason` 标成 `MaxTurns`，由展示层说明它没收敛。
        let final_text = messages
            .iter()
            .rev()
            .find(|message| {
                message.role == crate::types::Role::Assistant && !message.content.trim().is_empty()
            })
            .map(|message| message.content.clone())
            .unwrap_or_default();
        Ok(AgentOutcome {
            final_text,
            turns: self.config.max_turns,
            messages,
            stop_reason: AgentStopReason::MaxTurns,
            input_tokens,
            output_tokens,
            first_response_millis,
            context_summary: None,
        })
    }

    /// 取一批待投递事件，渲染进对话，返回它们的 lease。
    ///
    /// 一次最多取这么多条：事件是通知不是正文，一口气灌几十条只会把这一轮的
    /// 注意力全占掉，剩下的下一轮再来。
    fn append_kernel_events(&self, messages: &mut Vec<Message>) -> Vec<uuid::Uuid> {
        const BATCH: usize = 8;
        let Some(kernel) = &self.kernel else {
            return Vec::new();
        };
        let mut batch = kernel.take_for_model(BATCH);
        // `await_agents` 已经把这些 Worker 的报告直接交给了模型：它们的完成
        // 通知按「已投递」结账，不再讲第二遍。
        if let Some(background) = &self.background_tasks {
            let (already, fresh): (Vec<_>, Vec<_>) = batch.into_iter().partition(|leased| {
                leased
                    .event
                    .dedup_key
                    .as_deref()
                    .and_then(|key| key.strip_prefix("task:"))
                    .is_some_and(|id| background.is_delivered(id))
            });
            if !already.is_empty() {
                kernel.ack(
                    &already
                        .iter()
                        .map(|leased| leased.lease_id)
                        .collect::<Vec<_>>(),
                );
            }
            batch = fresh;
        }
        if batch.is_empty() {
            return Vec::new();
        }
        let events: Vec<_> = batch.iter().map(|leased| leased.event.clone()).collect();
        let Some(text) = crate::kernel::render_for_model(&events) else {
            // 渲染不出内容就别占着 lease。
            let leases: Vec<uuid::Uuid> = batch.iter().map(|leased| leased.lease_id).collect();
            kernel.release(&leases);
            return Vec::new();
        };
        messages.push(Message::host_instruction(text));
        if let Some(background) = &self.background_tasks {
            for event in &events {
                if let Some(id) = event
                    .dedup_key
                    .as_deref()
                    .and_then(|key| key.strip_prefix("task:"))
                {
                    background.mark_delivered(id);
                }
            }
        }
        batch.iter().map(|leased| leased.lease_id).collect()
    }

    /// 一批 lease 该按投递成功还是投递失败结账。
    ///
    /// 分界不是「请求成不成功」而是「事件有没有进到留得下来的 transcript」：
    /// 被抢占的那一轮请求虽然废了，事件文本却留在对话里，模型下一轮照样看得
    /// 到；而请求报错时整轮 messages 一起丢，事件就等于没人看过。
    fn settle_leases(&self, leases: &[uuid::Uuid], outcome: LeaseOutcome) {
        if leases.is_empty() {
            return;
        }
        let Some(kernel) = &self.kernel else {
            return;
        };
        match outcome {
            LeaseOutcome::Delivered => kernel.ack(leases),
            LeaseOutcome::Failed => kernel.release(leases),
        };
    }

    /// 发一次 provider 请求，除非中途被宿主签发的抢占事件打断。
    ///
    /// 返回 `Ok(None)` 表示被抢占。取消的只有这一次请求：transcript、工具
    /// 记录和事件队列都原样留着，下一轮从保留的上下文继续。
    async fn complete_or_preempt(
        &self,
        messages: &[Message],
        definitions: &[crate::types::ToolDefinition],
        events: &dyn crate::provider::ProviderEventSink,
    ) -> Result<Option<crate::types::Completion>, ProviderError> {
        let provider = self.provider()?;
        let Some(kernel) = self.kernel.clone() else {
            return provider
                .complete_with_events(messages, definitions, events)
                .await
                .map(Some);
        };
        tokio::select! {
            completion = provider.complete_with_events(messages, definitions, events) => completion.map(Some),
            () = kernel.preempted() => Ok(None),
        }
    }

    fn append_pending_instructions(&self, messages: &mut Vec<Message>) -> bool {
        let instructions = self
            .instruction_inbox
            .as_ref()
            .map(|inbox| inbox.drain())
            .unwrap_or_default();
        if instructions.is_empty() {
            return false;
        }
        let mut parent = Vec::new();
        for instruction in instructions {
            match instruction {
                AgentInstruction::Parent(text) => parent.push(text),
                // 用户插话以用户身份进对话：模型按用户说的办，回放时也是 `You:`。
                AgentInstruction::Operator(text) => messages.push(Message::user(text)),
            }
        }
        if !parent.is_empty() {
            messages.push(Message::host_instruction(format!(
                "Additional instructions from the parent Agent:\n\n{}",
                parent.join("\n\n")
            )));
        }
        true
    }

    fn execute_tool<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, ToolError>> + Send + 'a>>
    {
        Box::pin(async move {
            self.tools.check_tool_policy(&call.name)?;
            if let Some(result) = self.execute_recovery_tool(call).await {
                return result;
            }
            if let Some(result) = self.execute_agent_control_tool(call) {
                return result;
            }
            if call.name == "await_agents" {
                return self.execute_await_agents(call).await;
            }
            if call.name != "spawn_agent" {
                let result = self.tools.execute(call).await;
                if result.is_ok() {
                    self.admission.record_tool_success(&call.name);
                }
                return result;
            }
            let args: SpawnAgentArgs =
                serde_json::from_value(call.parsed_arguments().map_err(|source| {
                    ToolError::InvalidArguments {
                        tool: call.name.clone(),
                        source,
                    }
                })?)
                .map_err(|source| ToolError::InvalidArguments {
                    tool: call.name.clone(),
                    source,
                })?;
            self.execute_spawn_agent(args).await
        })
    }

    fn apply_runtime_route<'a>(
        &'a self,
        user_message: &'a mut Message,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        // A delegated run owns another Agent. Type-erasing this edge keeps
        // the parent's async state from recursively embedding the child's.
        Box::pin(async move {
            let (Some(routing), Some(catalog)) = (&self.routing, &self.subagents) else {
                return;
            };
            let decision = routing
                .route(routing_request_from_message(&user_message.content))
                .await;
            if !decision.classifier_failures.is_empty() {
                // 分类器候选失败以前是静默 continue：本地模型挂了一个月，
                // 路由悄悄退回关键词或会话模型，没人知道。
                self.sink
                    .emit(AgentEvent::ModelFallback {
                        purpose: "classifier".to_owned(),
                        skipped: Some(
                            decision
                                .classifier_failures
                                .iter()
                                .map(|failure| failure.model.as_str())
                                .collect::<Vec<_>>()
                                .join(", "),
                        ),
                        used: decision.classifier_model.clone(),
                        reason: decision
                            .classifier_failures
                            .iter()
                            .map(|failure| format!("{}: {}", failure.model, failure.reason))
                            .collect::<Vec<_>>()
                            .join("; "),
                    })
                    .await;
            }
            // 这一轮实际由谁干活：自动下发给 Worker 的是那个工种的模型，其余
            // 仍是主模型（引导只是提示，派不派工由主模型决定）。
            let dispatch = decision
                .profile
                .filter(|_| decision.auto_dispatch_read_only)
                .map(|profile| catalog.dispatch_model(profile, None));
            let model = match &dispatch {
                Some(dispatch) => dispatch.model.clone(),
                None => self.provider.model(),
            };
            self.sink
                .emit(AgentEvent::RouteDecided {
                    tier: decision.tier,
                    profile: decision.profile.map(str::to_owned),
                    confidence: decision.confidence,
                    auto_dispatched: decision.auto_dispatch_read_only,
                    reason: decision.reason.to_owned(),
                    model,
                    fallback: dispatch.and_then(|dispatch| dispatch.fallback),
                })
                .await;
            let Some(profile) = decision.profile else {
                return;
            };
            if !decision.auto_dispatch_read_only {
                if let Some(steering) = decision.steering() {
                    user_message.content.push_str("\n\n");
                    user_message.content.push_str(&steering);
                }
                return;
            }

            self.admission.record_profile_attempt(profile);
            let prompt = user_message.content.clone();
            let report = catalog
                .run(
                    SpawnAgentArgs {
                        prompt,
                        label: Some(format!("runtime preflight: {profile}")),
                        profile: Some(profile.to_owned()),
                        run_in_background: Some(false),
                        ..SpawnAgentArgs::default()
                    },
                    None,
                )
                .await;
            let report = match report {
                Ok(report) => report,
                Err(error) => format!(
                    "The `{profile}` preflight failed. Continue on the standard tier and do not treat this as evidence that deep is required. Error: {error}"
                ),
            };
            user_message.content.push_str(&format!(
                "\n\n<runtime-route tier=\"worker\" profile=\"{profile}\" confidence=\"{}\">\n\
The runtime dispatched this bounded read-only preflight before the standard model. Use the report as evidence, verify citations when needed, and answer without re-reading the same material unless it is incomplete.\n\
<worker-report>\n{report}\n</worker-report>\n\
</runtime-route>",
                decision.confidence
            ));
        })
    }
}

/// Frontends share the same explicit goal envelope. Parsing it in the core
/// keeps Web, TUI and headless Runtime behavior identical.
fn goal_from_message(content: &str) -> Option<String> {
    const OPEN: &str = "<goal>";
    const CLOSE: &str = "</goal>";
    let start = content.find(OPEN)? + OPEN.len();
    let rest = &content[start..];
    let end = rest.find(CLOSE)?;
    let goal = rest[..end].trim();
    if goal.is_empty() || goal.len() > 16 * 1024 {
        return None;
    }
    Some(goal.to_owned())
}

/// Goal frontends repeat a broad objective on every turn. Route the current
/// request, not that repeated envelope, or one word such as "implement" in
/// the goal would pin every later read-only question to the standard tier.
fn routing_request_from_message(content: &str) -> &str {
    let Some((_, tail)) = content.split_once("</goal>") else {
        return content;
    };
    let tail = tail.trim_start();
    let tail = tail
        .strip_prefix("Continue until this goal is genuinely complete.")
        .unwrap_or(tail)
        .trim_start();
    if tail.is_empty() { content } else { tail }
}

#[cfg(test)]
mod delegation_tests;
#[cfg(test)]
mod peer_session_tests;
#[cfg(test)]
mod tests;
