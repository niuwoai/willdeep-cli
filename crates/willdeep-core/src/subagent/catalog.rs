//! Dispatch: which trade runs, whether it is allowed to run at all, and what
//! it is handed. The circuit breaker, the write-set approval, the verifier
//! safety gate, the worktree policy and the background lifecycle all live
//! here; the actual execution is [`super::runner`]'s job.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::runs::RunRequest;
use crate::agent::{
    AgentError, AgentEvent, AgentInstructionInbox, EventSink, SubagentLifecycleStatus,
};
use crate::background::{
    BackgroundTaskKind, BackgroundTaskRegistry, BackgroundTaskStatus, TaskResult,
};
use crate::judge::{JudgeRequest, JudgeVerdict, SafetyJudge};
use crate::provider::Provider;
use crate::safety::CommandSafety;
use crate::subagent_worktree::{
    PreparedSubagentWorkspace, SubagentWorktreeManager, SubagentWorktreePolicy,
    worktree_result_note,
};
use crate::tools::ToolError;
use willdeep_runtime_protocol::AgentRunReason;

use super::runner::{SubagentRun, run_subagent};
use super::text::bounded_report;
use super::types::{
    PUBLIC_SUBAGENT_IDS, SpawnAgentArgs, SubagentProfile, SubagentWriteScope, TaskVerifier,
    public_profile_id,
};

mod control;
mod run_control;

/// Hard ceiling on the attempt budget a task packet may ask for.
const MAX_ATTEMPTS_CEILING: usize = 6;

#[derive(Clone)]
pub struct SubagentCatalog {
    workspace: PathBuf,
    profiles: BTreeMap<String, SubagentProfile>,
    background: Arc<BackgroundTaskRegistry>,
    sink: Arc<dyn EventSink>,
    failures: Arc<Mutex<BTreeMap<String, CircuitState>>>,
    model_overrides: Arc<Mutex<BTreeMap<uuid::Uuid, String>>>,
    worktrees: Option<SubagentWorktreeManager>,
    /// Files currently claimed by a running writing worker. Two workers whose
    /// declared sets intersect would race on the same lines; the second one
    /// is refused with the conflict named, and the parent decides whether to
    /// wait or to re-cut the work.
    claimed_files: Arc<Mutex<BTreeSet<PathBuf>>>,
    /// Judge consulted for verifier commands the static classifier cannot
    /// decide. A subagent has no approval UI, so an undecidable command with
    /// no judge is refused rather than silently run.
    safety_judge: Option<Arc<dyn SafetyJudge>>,
    mcp: Option<Arc<crate::mcp::McpRegistry>>,
    /// 父会话的 always-allow 存储。Worker 与父会话共享**已经明确批准过的精确
    /// 动作**，仅此而已：没批准过的照旧被拒，Worker 把精确动作写进报告，由父
    /// 会话去请人批。
    always_allow_store: Option<PathBuf>,
    state_home: Option<PathBuf>,
    parent_session: Option<uuid::Uuid>,
    /// Skill library used to resolve `task.skill` at dispatch time.
    skills: Option<Arc<crate::skills::SkillCatalog>>,
    /// 每一档兑现成哪个 provider。准入在上一层（[`crate::WorkerTier::requires_admission`]），
    /// 这里只负责兑现：档位说「这活儿值得贵一次」，绑定说「贵成什么样」。
    ///
    /// 没配的档不换模型，只放宽预算——票据白拿不到东西，好过静默降级到一个
    /// 谁也没指定的模型。
    tier_bindings: BTreeMap<crate::WorkerTier, TierBinding>,
    sandbox: crate::sandbox::SandboxSpec,
    /// 父会话的档位句柄；父会话在 full-access 时 Worker 跟着免审。
    parent_approval_mode: Option<crate::tools::SharedApprovalMode>,
    /// 这个目录（即这个父会话）起过的后台子 Agent。`send_agent_message` /
    /// `stop_agent` 只认这份名单：注册表可能被别的入口共用，名单外的 id 一律
    /// 当作找不到。
    pub(super) owned_background_agents: Arc<Mutex<BTreeSet<uuid::Uuid>>>,
    /// 父会话的用量账本；Worker 派生出自己的句柄记 `subagent` 行。
    usage_ledger: Option<crate::usage_ledger::UsageLedgerScope>,
    feedback: Option<crate::feedback::FeedbackRecorder>,
    board: Option<Arc<crate::board::Board>>,
    goal: Option<Arc<crate::goal::GoalContinuation>>,
    execution_deadline: Arc<Mutex<Option<tokio::time::Instant>>>,
}

/// 见 [`SubagentCatalog::dispatch_model`]。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DispatchModel {
    /// 实际会用的模型；说不上来（测试替身）时为 `None`。
    pub model: Option<String>,
    /// 这是兜底而不是所要的那个时，说明原因。
    pub fallback: Option<String>,
}

/// 一个档位兑现出来的模型。
#[derive(Clone)]
pub struct TierBinding {
    pub provider: Arc<dyn Provider>,
    /// 展示与遥测用的模型名；`None` 表示沿用 provider 自己配的那个。
    pub model: Option<String>,
    /// 这一档的上下文预算下限。
    pub window: u64,
    /// 这个模型是否由网关托管职责提示词（见 [`crate::worker_tier::hosts_job_prompt`]）。
    pub hosted_job_prompt: bool,
}

struct SubagentNoopSink;

#[async_trait::async_trait]
impl EventSink for SubagentNoopSink {
    async fn emit(&self, _event: AgentEvent) {}
}

impl SubagentCatalog {
    pub fn new(
        workspace: impl AsRef<Path>,
        profiles: Vec<SubagentProfile>,
        background: Arc<BackgroundTaskRegistry>,
    ) -> Self {
        Self {
            workspace: workspace.as_ref().to_path_buf(),
            profiles: profiles
                .into_iter()
                .map(|profile| (profile.id.clone(), profile))
                .collect(),
            background,
            sink: Arc::new(SubagentNoopSink),
            failures: Arc::new(Mutex::new(BTreeMap::new())),
            model_overrides: Arc::new(Mutex::new(BTreeMap::new())),
            worktrees: None,
            claimed_files: Arc::new(Mutex::new(BTreeSet::new())),
            safety_judge: None,
            skills: None,
            mcp: None,
            always_allow_store: None,
            state_home: None,
            parent_session: None,
            tier_bindings: BTreeMap::new(),
            sandbox: crate::sandbox::SandboxSpec::new(crate::sandbox::SandboxPolicy::Off, []),
            parent_approval_mode: None,
            owned_background_agents: Arc::new(Mutex::new(BTreeSet::new())),
            usage_ledger: None,
            feedback: None,
            board: None,
            goal: None,
            execution_deadline: Arc::new(Mutex::new(None)),
        }
    }

    /// 挂上父会话的目标：Worker 的用量计入目标的 token 预算。
    pub fn with_goal(mut self, goal: Arc<crate::goal::GoalContinuation>) -> Self {
        self.goal = Some(goal);
        self
    }

    pub(crate) fn set_execution_deadline(&self, deadline: Option<tokio::time::Instant>) {
        *self
            .execution_deadline
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = deadline;
    }

    /// 挂上父会话的共享黑板：派出的每个 Worker 都能读写它。
    pub fn with_board(mut self, board: Arc<crate::board::Board>) -> Self {
        self.board = Some(board);
        self
    }

    pub(crate) fn board(&self) -> Option<&Arc<crate::board::Board>> {
        self.board.as_ref()
    }

    /// 让 Worker 的工具失败、没收敛与没结果的收尾进父会话的反馈账本。
    pub fn with_feedback(mut self, recorder: crate::feedback::FeedbackRecorder) -> Self {
        self.feedback = recorder.is_enabled().then_some(recorder);
        self
    }

    /// 让 Worker 的模型调用进父会话的用量账本。
    pub fn with_usage_ledger(mut self, scope: crate::usage_ledger::UsageLedgerScope) -> Self {
        self.usage_ledger = Some(scope);
        self
    }

    /// Workers and unattended verifiers inherit the parent's OS boundary.
    /// Worker 跟随父会话的 full-access（实时）。见 [`crate::ToolRegistry::with_parent_approval_mode`]。
    pub fn with_parent_approval_mode(mut self, parent: crate::tools::SharedApprovalMode) -> Self {
        self.parent_approval_mode = Some(parent);
        self
    }

    pub fn with_sandbox(mut self, sandbox: crate::sandbox::SandboxSpec) -> Self {
        self.sandbox = sandbox;
        self
    }

    /// 把已连接的 MCP 服务交给兜底工种。
    ///
    /// 只有 `generalist` 会拿到 MCP 工具：窄工种的价值在于范围窄，给它们开一
    /// 条通往任意外部服务的门等于把这个价值抵消掉。**动态网关不等于免审批**
    /// ——`call_mcp_tool` 的外部副作用仍走各自的审批路径。
    pub fn with_mcp(mut self, mcp: Arc<crate::mcp::McpRegistry>) -> Self {
        self.mcp = Some(mcp);
        self
    }

    /// 让 Worker 共享父会话已批准的精确动作。
    ///
    /// 共享的**是精确项，不是命令族**：父会话批过 `cargo test` 才等于 Worker
    /// 也能跑 `cargo test`，不等于它能跑任意 `cargo`。没有这条，一个后台
    /// Worker 会在人已经批准过的同一条命令上再次卡住，而它自己没有审批 UI，
    /// 只能失败回来。
    pub fn with_state_home(mut self, path: impl Into<PathBuf>) -> Self {
        self.state_home = Some(path.into());
        self
    }

    pub fn with_parent_session(mut self, id: uuid::Uuid) -> Self {
        self.parent_session = Some(id);
        self
    }

    pub fn with_always_allow_store(mut self, path: impl Into<PathBuf>) -> Self {
        self.always_allow_store = Some(path.into());
        self
    }

    /// Attach the skill library so a task packet can name a skill and have
    /// its body inlined into the worker's opening message.
    pub fn with_skills(mut self, skills: Arc<crate::skills::SkillCatalog>) -> Self {
        self.skills = Some(skills);
        self
    }

    /// 给某一档绑定兑现用的 provider。
    ///
    /// 三档都可绑：基础档默认沿用工种自己的模型，进阶与专家档由 harness 按
    /// 网关默认表或用户的 `[worker_tiers.*]` 配置填进来。
    pub fn with_tier_binding(mut self, tier: crate::WorkerTier, binding: TierBinding) -> Self {
        self.tier_bindings.insert(tier, binding);
        self
    }

    /// 这一档兑现成什么。没绑就是 `None`。
    pub fn tier_binding(&self, tier: crate::WorkerTier) -> Option<&TierBinding> {
        self.tier_bindings.get(&tier)
    }

    /// 一次派工会跑在哪个模型上，与 `run_with_id` 的兑现规则同一条：有档位
    /// 绑定就用绑定，否则是工种自己的模型。给事件与提示用，不改变派工。
    ///
    /// 要了贵一档（非 standard）却没有绑定时，`fallback` 说明「只放宽了预算，
    /// 模型没换」——以前这件事是静默的。
    pub fn dispatch_model(&self, profile: &str, worker_tier: Option<&str>) -> DispatchModel {
        let Some(resolved) = self.profile(Some(profile)) else {
            return DispatchModel::default();
        };
        let own = resolved
            .model
            .clone()
            .or_else(|| crate::provider::provider_model(resolved.provider.as_ref()));
        if resolved.definition_model_bound {
            return DispatchModel {
                model: own,
                fallback: None,
            };
        }
        let Some(tier) = worker_tier.and_then(crate::WorkerTier::parse) else {
            return DispatchModel {
                model: own,
                fallback: None,
            };
        };
        match self.tier_bindings.get(&tier) {
            Some(binding) => DispatchModel {
                model: binding
                    .model
                    .clone()
                    .or_else(|| crate::provider::provider_model(binding.provider.as_ref())),
                fallback: None,
            },
            None if tier != crate::WorkerTier::Standard => DispatchModel {
                fallback: Some(format!(
                    "worker_tier={} has no model bound on this provider (configure [worker_tiers.{}]); `{}` keeps its own model{} with a {}-token context budget",
                    tier.as_str(),
                    tier.as_str(),
                    resolved.id,
                    own.as_deref()
                        .map(|model| format!(" {model}"))
                        .unwrap_or_default(),
                    resolved
                        .definition_binding
                        .as_ref()
                        .and_then(|binding| binding.context_window)
                        .unwrap_or_else(|| resolved.context_window.max(tier.context_budget())),
                )),
                model: own,
            },
            None => DispatchModel {
                model: own,
                fallback: None,
            },
        }
    }

    pub fn with_event_sink(mut self, sink: Arc<dyn EventSink>) -> Self {
        self.sink = sink;
        self
    }

    /// Attach the judge that reviews verifier commands the static classifier
    /// cannot decide. Without one, those commands are refused.
    pub fn with_safety_judge(mut self, judge: Arc<dyn SafetyJudge>) -> Self {
        self.safety_judge = Some(judge);
        self
    }

    pub fn with_worktree_root(mut self, root: impl AsRef<Path>) -> Self {
        self.worktrees = Some(SubagentWorktreeManager::new(root.as_ref().to_path_buf()));
        self
    }

    pub fn description(&self) -> String {
        self.profiles
            .values()
            .filter(|profile| {
                PUBLIC_SUBAGENT_IDS.contains(&profile.id.as_str()) || profile.definition.is_some()
            })
            .map(|profile| format!("`{}` — {}", profile.id, profile.purpose))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn public_profile_ids(&self) -> Vec<String> {
        self.profiles
            .values()
            .filter(|profile| {
                PUBLIC_SUBAGENT_IDS.contains(&profile.id.as_str()) || profile.definition.is_some()
            })
            .map(|profile| profile.id.clone())
            .collect()
    }

    pub(crate) fn supports_target_command(&self, id: &str) -> bool {
        self.profile(Some(id)).is_some_and(|profile| {
            profile
                .definition
                .as_ref()
                .map(|definition| definition.base_role.as_str())
                .unwrap_or(&profile.id)
                == "ops_runner"
                && profile.tool_names.iter().any(|name| name == "run_command")
        })
    }

    pub async fn retry_background_agent(
        &self,
        agent_id: uuid::Uuid,
        model: Option<&str>,
    ) -> Result<Option<String>, AgentError> {
        if let Some(id) = self.retry_in_memory(agent_id, model)? {
            return Ok(Some(id));
        }
        if self
            .background
            .snapshots()
            .iter()
            .any(|task| task.agent_id == Some(agent_id))
        {
            return Ok(None);
        }
        let Some(home) = &self.state_home else {
            return Ok(None);
        };
        let latest = self
            .background
            .agent_run_store()
            .map(|store| store.latest(agent_id))
            .transpose()
            .map_err(AgentError::Checkpoint)?
            .flatten();
        let record = match latest {
            Some(entry) => Some(
                serde_json::from_value(entry.dispatch)
                    .map_err(|error| AgentError::Checkpoint(error.to_string()))?,
            ),
            None => super::dispatch_store::load(home, agent_id)?,
        };
        let Some(record) = record else {
            return Ok(None);
        };
        if record.parent_session != self.parent_session
            || record.args.run_in_background != Some(true)
        {
            return Err(AgentError::Subagent(
                "worker dispatch does not belong to this parent background session".into(),
            ));
        }
        if record
            .args
            .profile
            .as_deref()
            .is_none_or(|id| !self.profiles.contains_key(id))
        {
            return Err(AgentError::Subagent(
                "original worker profile is no longer configured".into(),
            ));
        }
        super::dispatch_store::validate_workspace(&record.prepared, &self.workspace).await?;
        let selected_model = model
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or(record.model);
        if let Some(model) = selected_model {
            self.model_overrides
                .lock()
                .expect("subagent model overrides")
                .insert(agent_id, model);
        }
        let result = self
            .run_with_id(
                agent_id,
                record.args,
                record.approved_targets,
                record.approved_command,
                Some(record.prepared),
                Some((uuid::Uuid::new_v4(), AgentRunReason::Retry)),
            )
            .await;
        if let Err(error) = result {
            self.model_overrides
                .lock()
                .expect("subagent model overrides")
                .remove(&agent_id);
            return Err(error);
        }
        Ok(self
            .background
            .snapshots()
            .into_iter()
            .find(|task| task.agent_id == Some(agent_id))
            .map(|task| task.id))
    }

    pub(crate) fn list_agent_recoveries(
        &self,
        after: Option<uuid::Uuid>,
    ) -> Result<String, AgentError> {
        let home = self
            .state_home
            .as_deref()
            .ok_or_else(|| AgentError::Checkpoint("worker state home is unavailable".into()))?;
        let parent = self.parent_session.ok_or_else(|| {
            AgentError::Checkpoint("parent session identity is unavailable".into())
        })?;
        Ok(super::dispatch_store::list_foreground(home, parent, after)?.to_string())
    }

    pub(crate) async fn resume_foreground_agent(
        &self,
        id: uuid::Uuid,
    ) -> Result<String, AgentError> {
        let home = self
            .state_home
            .as_deref()
            .ok_or_else(|| AgentError::Checkpoint("worker state home is unavailable".into()))?;
        let record = super::dispatch_store::load(home, id)?
            .ok_or_else(|| AgentError::Checkpoint("worker dispatch was not found".into()))?;
        if record.parent_session != self.parent_session
            || self.parent_session.is_none()
            || record.args.run_in_background == Some(true)
        {
            return Err(AgentError::Subagent(
                "dispatch is not a foreground child of this parent session".into(),
            ));
        }
        if let Some(report) = super::dispatch_store::load_report(home, id)? {
            self.sink
                .emit(AgentEvent::SubagentCompleted {
                    id,
                    status: SubagentLifecycleStatus::Completed,
                    report: Some(bounded_report(report.clone())),
                })
                .await;
            return Ok(format!(
                "[Recovered completed child report; tools and verifier were not rerun.]\n{report}"
            ));
        }
        if record
            .args
            .profile
            .as_deref()
            .is_none_or(|profile| !self.profiles.contains_key(profile))
        {
            return Err(AgentError::Subagent(
                "original worker profile is no longer configured".into(),
            ));
        }
        super::dispatch_store::validate_workspace(&record.prepared, &self.workspace).await?;
        if let Some(model) = record.model {
            self.model_overrides
                .lock()
                .expect("subagent model overrides")
                .insert(id, model);
        }
        self.run_with_id(
            id,
            record.args,
            record.approved_targets,
            record.approved_command,
            Some(record.prepared),
            None,
        )
        .await
    }

    fn retry_in_memory(
        &self,
        agent_id: uuid::Uuid,
        model: Option<&str>,
    ) -> Result<Option<String>, AgentError> {
        let model = model.map(str::trim).filter(|model| !model.is_empty());
        if model.is_some_and(|model| model.len() > 256) {
            return Err(AgentError::Subagent(
                "model override must contain 1 to 256 bytes".to_owned(),
            ));
        }
        let previous = if let Some(model) = model {
            self.model_overrides
                .lock()
                .expect("subagent model overrides")
                .insert(agent_id, model.to_owned())
        } else {
            None
        };
        let retried = self.background.retry_agent_with_model(agent_id, model);
        if retried.is_none() && model.is_some() {
            let mut overrides = self
                .model_overrides
                .lock()
                .expect("subagent model overrides");
            if let Some(previous) = previous {
                overrides.insert(agent_id, previous);
            } else {
                overrides.remove(&agent_id);
            }
        }
        Ok(retried)
    }

    pub(crate) fn write_scope(&self, id: Option<&str>) -> SubagentWriteScope {
        self.profile(id)
            .map_or(SubagentWriteScope::None, |profile| profile.write_scope)
    }

    pub(crate) fn has_profile(&self, id: &str) -> bool {
        self.resolve_profile_id(id).is_some()
    }

    /// 把一个可能是旧名的工种解析成目录里真实存在的那个。
    ///
    /// 直接命中优先——内部工种（`scout`、`editor` 等）仍然可以精确点名；
    /// 命不中再走公开名映射，让 `reader`、`judge`、`deep` 这些改名前的写法
    /// 继续可用。别人保存的流程不该因为我们换了个称呼就断掉。
    fn resolve_profile_id(&self, id: &str) -> Option<String> {
        let id = id.trim().to_ascii_lowercase();
        if self.profiles.contains_key(&id) {
            return Some(id);
        }
        public_profile_id(&id)
            .map(str::to_owned)
            .filter(|resolved| self.profiles.contains_key(resolved))
    }

    fn profile(&self, id: Option<&str>) -> Option<&SubagentProfile> {
        let resolved = self.resolve_profile_id(id.unwrap_or("generalist"))?;
        self.profiles.get(&resolved)
    }

    pub(crate) async fn run(
        &self,
        args: SpawnAgentArgs,
        approved_targets: Option<BTreeSet<PathBuf>>,
    ) -> Result<String, AgentError> {
        if args.target_command.is_some() {
            return Err(AgentError::Subagent(
                "target_command requires exact parent authorization".to_owned(),
            ));
        }
        self.run_with_id(
            uuid::Uuid::new_v4(),
            args,
            approved_targets,
            None,
            None,
            None,
        )
        .await
    }

    pub(crate) async fn run_authorized(
        &self,
        args: SpawnAgentArgs,
        approved_targets: Option<BTreeSet<PathBuf>>,
        approved_command: Option<String>,
    ) -> Result<String, AgentError> {
        self.run_with_id(
            uuid::Uuid::new_v4(),
            args,
            approved_targets,
            approved_command,
            None,
            None,
        )
        .await
    }

    pub async fn spawn_external_read_only(
        &self,
        id: uuid::Uuid,
        prompt: String,
        label: Option<String>,
        profile: Option<String>,
    ) -> Result<(), AgentError> {
        let profile_id = profile
            .as_deref()
            .unwrap_or("deep")
            .trim()
            .to_ascii_lowercase();
        if crate::WorkerTier::parse(&profile_id).is_some_and(|tier| tier.requires_admission()) {
            return Err(AgentError::Subagent(
                "external expert-tier spawn is disabled; the parent Agent must provide a runtime-validated escalation ticket"
                    .to_owned(),
            ));
        }
        let selected = self.profiles.get(&profile_id).ok_or_else(|| {
            AgentError::Subagent(format!("subagent profile not found: {profile_id}"))
        })?;
        const READ_ONLY_TOOLS: &[&str] = &[
            "search_files",
            "grep_files",
            "list_directory",
            "read_file",
            "git_status",
            "git_diff",
            "git_log",
            "git_blame",
        ];
        if selected.write_scope.writes()
            || selected
                .tool_names
                .iter()
                .any(|name| !READ_ONLY_TOOLS.contains(&name.as_str()))
        {
            return Err(AgentError::Subagent(format!(
                "profile {profile_id} is not eligible for external read-only spawn"
            )));
        }
        self.run_with_id(
            id,
            SpawnAgentArgs {
                prompt,
                label,
                profile: Some(profile_id),
                run_in_background: Some(true),
                ..SpawnAgentArgs::default()
            },
            None,
            None,
            None,
            Some((id, AgentRunReason::Spawn)),
        )
        .await?;
        Ok(())
    }

    async fn run_with_id(
        &self,
        agent_id: uuid::Uuid,
        args: SpawnAgentArgs,
        approved_targets: Option<BTreeSet<PathBuf>>,
        approved_command: Option<String>,
        restored_workspace: Option<PreparedSubagentWorkspace>,
        requested_run: Option<(uuid::Uuid, AgentRunReason)>,
    ) -> Result<String, AgentError> {
        let mut dispatch_args = args.clone();
        if let Some(store) = self.background.agent_run_store() {
            store
                .ensure_agent_available(agent_id)
                .map_err(AgentError::SubagentBlocked)?;
        }
        let mut profile = self
            .profile(args.profile.as_deref())
            .ok_or_else(|| AgentError::Subagent("no subagent profiles configured".to_owned()))?
            .clone();
        if restored_workspace.is_some()
            && let Some(home) = &self.state_home
            && let Some(record) = super::dispatch_store::load(home, agent_id)?
            && let Some(definition) = record.definition
        {
            let base = self.profiles.get(&definition.base_role).ok_or_else(|| {
                AgentError::Subagent("original definition base role unavailable".into())
            })?;
            let local_binding = profile
                .definition_model_bound
                .then(|| (profile.provider.clone(), profile.model.clone()));
            let execution_binding = profile.definition_binding.clone();
            profile = definition
                .project(base)
                .map_err(|error| AgentError::Subagent(error.to_string()))?;
            profile.definition_binding = execution_binding;
            if let Some((provider, model)) = local_binding {
                profile.provider = provider;
                profile.model = model;
                profile.definition_model_bound = true;
            }
        }
        dispatch_args.profile = Some(profile.id.clone());
        // 职责给提示词、工具和写入边界，档位给模型和上下文预算。
        //
        // 预算只放宽不收窄：工种自己声明的窗口是它完成职责的下限，implementer
        // 的 256K 不该因为跑在基础档上被砍掉。换模型则走档位绑定——专家档已经
        // 在 agent 层过了票据与预算，到这里只负责兑现。
        if let Some(tier) = args
            .worker_tier
            .as_deref()
            .and_then(crate::WorkerTier::parse)
        {
            profile.context_window = profile.context_window.max(tier.context_budget());
            if !profile.definition_model_bound
                && let Some(binding) = self.tier_bindings.get(&tier)
            {
                profile.provider = binding.provider.clone();
                profile.model = binding.model.clone();
                profile.context_window = profile.context_window.max(binding.window);
                // 提示词的归属跟着模型走，不跟着工种走。工种默认绑在托管别名上、
                // 档位又把它换成别的模型时，这份提示词必须重新由客户端发送，
                // 否则 Worker 只剩边界段落、不知道自己是干什么的。
                profile.hosted_job_prompt =
                    profile.definition.is_none() && binding.hosted_job_prompt;
            }
        }
        if let Some(model) = self
            .model_overrides
            .lock()
            .expect("subagent model overrides")
            .get(&agent_id)
            .cloned()
        {
            if profile.model.as_deref() != Some(model.as_str()) {
                profile.provider = profile.provider.with_model(&model)?;
            }
            profile.hosted_job_prompt =
                profile.definition.is_none() && crate::hosts_job_prompt(&model);
            profile.model = Some(model);
        }
        // 跟随主模型的工种没有固定模型名：派工这一刻主模型是谁，就记谁。
        // 落盘的派工记录与事件因此记的是实际跑的模型，不是启动时的快照。
        if profile.model.is_none()
            && let Some(model) = crate::provider::provider_model(profile.provider.as_ref())
        {
            profile.hosted_job_prompt = crate::hosts_job_prompt(&model);
            profile.model = Some(model);
        }
        if let (Some(store), Some((request_id, _))) =
            (self.background.agent_run_store(), requested_run)
            && let Some(saved) = store.get(request_id).map_err(AgentError::Checkpoint)?
            && let Some(turns) = saved.max_turns
        {
            profile.max_turns = turns as usize;
            profile.token_budget = saved.token_budget;
            profile.timeout_seconds = saved.timeout_seconds;
        }
        // Explicit local limits win over tier defaults, including resumed runs.
        if let Some(binding) = profile.definition_binding.clone() {
            binding.apply_limits(&mut profile);
        }
        let requested_command = args
            .target_command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty());
        if requested_command.is_some() && !self.supports_target_command(&profile.id) {
            return Err(AgentError::Subagent(
                "target_command is only supported by the ops_runner profile".to_owned(),
            ));
        }
        if requested_command != approved_command.as_deref() {
            return Err(AgentError::Subagent(
                "target_command did not receive matching parent authorization".to_owned(),
            ));
        }
        {
            let mut failures = self.failures.lock().map_err(|_| {
                AgentError::Subagent("subagent failure tracker is unavailable".to_owned())
            })?;
            circuit_admit(
                &mut failures,
                &profile.id,
                profile.max_consecutive_failures,
                CIRCUIT_COOLDOWN,
                std::time::Instant::now(),
            )
            .map_err(AgentError::Subagent)?;
        }
        if profile.write_scope.requires_declared_targets()
            && approved_targets.as_ref().is_none_or(BTreeSet::is_empty)
        {
            return Err(AgentError::Subagent(format!(
                "profile {} may write, so it requires an approved file set: pass target_file, or task.write_files for a file-set profile",
                profile.id
            )));
        }
        // 写集与正在跑的 Worker 冲突就在派工这一刻说清楚，而不是派出一个注定
        // 在认领时失败的后台任务。判定与 runner 的认领同一份名单、同一条规则。
        if args.run_in_background != Some(true)
            && let Some(targets) = approved_targets
                .as_ref()
                .filter(|targets| !targets.is_empty())
        {
            let held = self.claimed_files.lock().map_err(|_| {
                AgentError::Subagent("subagent file claims are unavailable".to_owned())
            })?;
            let conflicts = targets.intersection(&held).collect::<Vec<_>>();
            if !conflicts.is_empty() {
                return Err(AgentError::Subagent(format!(
                    "another running subagent is already writing {}; wait for it (await_agents) or dispatch this work over different files",
                    conflicts
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
        let task = args.task;
        let verifier = match task.as_ref().and_then(|task| task.verifier.clone()) {
            Some(verifier) => Some(self.gate_verifier_command(verifier).await?),
            None => None,
        };
        // A worker that can run commands is a worker whose whole design is
        // "fix, then prove it". Without a verifier it has nothing to run and
        // nothing to prove, and its report would be exactly the unverified
        // self-assessment this profile exists to replace.
        if verifier.is_none() && profile.shell.requires_verifier() {
            return Err(AgentError::Subagent(format!(
                "profile {} is a verified worker and requires task.verifier.command",
                profile.id
            )));
        }
        let max_attempts = task
            .as_ref()
            .and_then(|task| task.max_attempts)
            .unwrap_or(profile.max_attempts)
            .clamp(1, MAX_ATTEMPTS_CEILING);
        let label = args
            .label
            .filter(|value| !value.trim().is_empty())
            .or_else(|| task.as_ref().map(|task| task.goal.clone()))
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| args.prompt.chars().take(48).collect());
        let label = match &profile.definition {
            Some(definition) => format!(
                "[revision={}] {label}",
                definition
                    .revision()
                    .map_err(|error| AgentError::Subagent(error.to_string()))?
            ),
            None => label,
        };
        let prompt = args.prompt;
        let profile_id = profile.id.clone();
        let background = args.run_in_background.unwrap_or(false);
        let recovering = restored_workspace.is_some();
        let prepared = if let Some(prepared) = restored_workspace {
            if prepared.dedicated != (profile.worktree == SubagentWorktreePolicy::Dedicated) {
                return Err(AgentError::Subagent(
                    "worker worktree policy changed since dispatch".into(),
                ));
            }
            prepared
        } else if background {
            self.plan_workspace(agent_id, profile.worktree).await?
        } else {
            self.prepare_workspace(agent_id, profile.worktree).await?
        };
        let dispatch_record = super::dispatch_store::DispatchRecord {
            version: 1,
            id: agent_id,
            parent_session: self.parent_session,
            args: dispatch_args,
            approved_targets: approved_targets.clone(),
            approved_command: approved_command.clone(),
            model: profile.model.clone(),
            definition: profile.definition.clone(),
            prepared: prepared.clone(),
        };
        if !recovering && let Some(home) = &self.state_home {
            super::dispatch_store::save(home, &dispatch_record)?;
        }
        let approved_targets = remap_approved_targets(approved_targets, &prepared)?;
        let run = SubagentRun {
            isolated_evaluation: false,
            offline_review_json: false,
            execution_deadline: *self
                .execution_deadline
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            workspace: prepared.workspace.clone(),
            sandbox: self
                .sandbox
                .for_workspace(&self.workspace, &prepared.workspace),
            profile: profile.clone(),
            prompt: prompt.clone(),
            task,
            approved_targets,
            verifier,
            max_attempts,
            claimed_files: self.claimed_files.clone(),
            skills: self.skills.clone(),
            safety_judge: self.safety_judge.clone(),
            approved_command,
            // 只给兜底工种。窄工种声明里也没有那两个工具名，双重保险。
            mcp: (profile.id == "generalist")
                .then(|| self.mcp.clone())
                .flatten(),
            always_allow_store: self.always_allow_store.clone(),
            state_home: self.state_home.clone(),
            parent_approval_mode: self.parent_approval_mode.clone(),
            usage_ledger: self.usage_ledger.clone(),
            feedback: self.feedback.clone(),
            board: self.board.clone(),
            goal: self.goal.clone(),
        };
        if background {
            let (request_id, reason) =
                requested_run.unwrap_or_else(|| (uuid::Uuid::new_v4(), AgentRunReason::Spawn));
            let run_request = RunRequest {
                request_id,
                reason,
                agent_id,
                label: label.clone(),
                model: profile.model.clone(),
                max_turns: Some(profile.max_turns as u64),
                token_budget: profile.token_budget,
                timeout_seconds: profile.timeout_seconds,
                dispatch: serde_json::to_value(&dispatch_record)
                    .map_err(|error| AgentError::Checkpoint(error.to_string()))?,
            };
            let runner_sink = self.sink.clone();
            let failures = self.failures.clone();
            let lifecycle_sink = self.sink.clone();
            let lifecycle_background = Arc::downgrade(&self.background);
            let lifecycle_profile = profile_id.clone();
            let lifecycle_model = profile.model.clone();
            let lifecycle_label = label.clone();
            let lifecycle_max_turns = profile.max_turns;
            let lifecycle_token_budget = profile.token_budget;
            let lifecycle_timeout_seconds = profile.timeout_seconds;
            let lifecycle_workspace = prepared.workspace.clone();
            let lifecycle_root_workspace = prepared.root_workspace.clone();
            let lifecycle_worktree_branch = prepared.branch.clone();
            let lifecycle_dedicated_worktree = prepared.dedicated;
            let runner_model_overrides = self.model_overrides.clone();
            let lifecycle_model_overrides = self.model_overrides.clone();
            let instruction_inbox = Arc::new(AgentInstructionInbox::default());
            let runner_instruction_inbox = instruction_inbox.clone();
            let runner_prepared = prepared.clone();
            let runner_background = Arc::downgrade(&self.background);
            let runner_worktrees = self.worktrees.clone();
            let runner_root = self.workspace.clone();
            self.owned_background_agents
                .lock()
                .expect("owned background agents")
                .insert(agent_id);
            let id = self
                .background
                .start_agent_run(
                    crate::background::AgentRunSpec {
                        request: Some(run_request), agent_id, kind: BackgroundTaskKind::Subagent,
                        label: format!("{label} · {profile_id}"), instruction_inbox,
                    },
                    move || {
                        let mut run = run.clone();
                        let profile_id = run.profile.id.clone();
                        let sink = runner_sink.clone();
                        let failures = failures.clone();
                        let instruction_inbox = runner_instruction_inbox.clone();
                        let prepared = runner_prepared.clone();
                        let background = runner_background.clone();
                        let worktrees = runner_worktrees.clone();
                        let root_workspace = runner_root.clone();
                        let model_overrides = runner_model_overrides.clone();
                        async move {
                            let Some(background) = background.upgrade() else {
                                return subagent_task_result(Err(AgentError::Subagent(
                                    "parent execution host is unavailable".into(),
                                )));
                            };
                            let run_id = background
                                .task_for_agent(agent_id)
                                .and_then(|snapshot| snapshot.run_id);
                            let sink: Arc<dyn EventSink> = if let (Some(store), Some(run_id)) =
                                (background.agent_run_store(), run_id)
                            {
                                Arc::new(super::runs::RunProgressSink {
                                    store: store.clone(),
                                    run_id,
                                    parent: sink,
                                })
                            } else {
                                sink
                            };
                            if let (Some(scope), Some(store), Some(run_id)) =
                                (&run.usage_ledger, background.agent_run_store(), run_id)
                            {
                                run.usage_ledger =
                                    Some(scope.with_agent_run(store.clone(), run_id));
                            }
                            if let Some(scope) = &run.usage_ledger
                                && let Err(error) = scope.check_budget()
                            {
                                return subagent_task_result(Err(AgentError::TeamBudgetBlocked(
                                    error,
                                )));
                            }
                            if prepared.dedicated && prepared.workspace.exists()
                                && let Err(error) = super::dispatch_store::validate_workspace(&prepared, &root_workspace).await {
                                    return subagent_task_result(Err(AgentError::SubagentBlocked(error.to_string())));
                            }
                            let prepared = if prepared.dedicated && !prepared.workspace.exists() {
                                if let (Some(store), Some(run_id)) = (background.agent_run_store(), run_id) {
                                    match store.workspace_was_used(agent_id, run_id) {
                                        Ok(true) => return subagent_task_result(Err(AgentError::SubagentBlocked("The original worker worktree is missing; inspect or restore it before continuing.".into()))),
                                        Err(error) => return subagent_task_result(Err(AgentError::Checkpoint(error))),
                                        Ok(false) => {},
                                    }
                                }
                                let Some(manager) = &worktrees else {
                                    return subagent_task_result(Err(AgentError::Subagent(
                                        "worker worktree manager is unavailable".into(),
                                    )));
                                };
                                match manager
                                    .prepare(&root_workspace, agent_id, run.profile.worktree)
                                    .await
                                {
                                    Ok(prepared) => prepared,
                                    Err(error) => return subagent_task_result(Err(error)),
                                }
                            } else {
                                prepared
                            };
                            if let Some(model) = background.run_model(agent_id).or_else(|| {
                                model_overrides
                                    .lock()
                                    .expect("subagent model overrides")
                                    .get(&agent_id)
                                    .cloned()
                            }) {
                                if run.profile.model.as_deref() != Some(model.as_str()) {
                                    run.profile.provider =
                                        match run.profile.provider.with_model(&model) {
                                            Ok(provider) => provider,
                                            Err(error) => {
                                                return subagent_task_result(Err(error.into()));
                                            }
                                        };
                                }
                                run.profile.model = Some(model);
                                run.profile.hosted_job_prompt = run
                                    .profile
                                    .model
                                    .as_deref()
                                    .is_some_and(crate::hosts_job_prompt);
                            }
                            let budget_scope = run.usage_ledger.clone();
                            let mut result =
                                run_subagent(run, sink.clone(), agent_id, Some(instruction_inbox))
                                    .await;
                            if result.is_err()
                                && let Some(scope) = budget_scope
                                && let Err(error) = scope.check_budget()
                            {
                                result = Err(AgentError::TeamBudgetBlocked(error));
                            }
                            let result = attach_worktree_report(result, &prepared).await;
                            record_profile_result(&failures, &profile_id, &result);
                            subagent_task_result(result)
                        }
                    },
                    move |snapshot| {
                        let sink: Arc<dyn EventSink> = if let (Some(background), Some(run_id)) =
                            (lifecycle_background.upgrade(), snapshot.run_id)
                            && let Some(store) = background.agent_run_store()
                        {
                            Arc::new(super::runs::RunProgressSink {
                                store: store.clone(),
                                run_id,
                                parent: lifecycle_sink.clone(),
                            })
                        } else {
                            lifecycle_sink.clone()
                        };
                        let background = lifecycle_background.clone();
                        let profile = lifecycle_profile.clone();
                        let model = lifecycle_model_overrides
                            .lock()
                            .expect("subagent model overrides")
                            .get(&agent_id)
                            .cloned()
                            .or_else(|| lifecycle_model.clone());
                        let label = lifecycle_label.clone();
                        let workspace = lifecycle_workspace.clone();
                        let root_workspace = lifecycle_root_workspace.clone();
                        let worktree_branch = lifecycle_worktree_branch.clone();
                        async move {
                            if snapshot.status == BackgroundTaskStatus::Queued {
                                sink.emit(AgentEvent::SubagentQueued {
                                    id: agent_id,
                                    profile,
                                    model,
                                    label,
                                })
                                .await;
                            } else if snapshot.status == BackgroundTaskStatus::Running {
                                sink.emit(AgentEvent::SubagentStarted {
                                    id: agent_id,
                                    profile,
                                    model,
                                    label,
                                    background: true,
                                    max_turns: lifecycle_max_turns,
                                    token_budget: lifecycle_token_budget,
                                    timeout_seconds: lifecycle_timeout_seconds,
                                    workspace,
                                    root_workspace,
                                    worktree_branch,
                                    dedicated_worktree: lifecycle_dedicated_worktree,
                                })
                                .await;
                            } else {
                                if lifecycle_dedicated_worktree && workspace.exists() && matches!(snapshot.status, BackgroundTaskStatus::Killed | BackgroundTaskStatus::TimedOut) {
                                    let prepared = PreparedSubagentWorkspace { workspace, root_workspace, branch: worktree_branch, dedicated: true };
                                    let note = match worktree_result_note(&prepared).await {
                                        Ok(Some(note)) => note,
                                        Ok(None) => String::new(),
                                        Err(error) => format!("Cancelled worker workspace retained; handoff capture failed: {error}"),
                                    };
                                    if let Some(registry) = background.upgrade() && let Err(error) = registry.append_terminal_note(&snapshot.id, &note) {
                                        eprintln!("cannot persist cancelled worker handoff: {error}");
                                    }
                                }
                                sink.emit(AgentEvent::SubagentCompleted {
                                    id: agent_id,
                                    status: background_lifecycle_status(&snapshot.status),
                                    report: background.upgrade().and_then(|background| {
                                        background.output(&snapshot.id, 200)
                                    }),
                                })
                                .await;
                            }
                        }
                    },
                )
                .map_err(AgentError::Subagent)?;
            Ok(format!(
                "Subagent accepted: agent_id={agent_id}, background_task={id}. It may be queued; its report will be delivered automatically to the main harness."
            ))
        } else {
            let cached_report = if recovering {
                self.state_home
                    .as_deref()
                    .map(|home| super::dispatch_store::load_report(home, agent_id))
                    .transpose()?
                    .flatten()
            } else {
                None
            };
            let recovered_report = cached_report.is_some();
            self.sink
                .emit(AgentEvent::SubagentStarted {
                    id: agent_id,
                    profile: profile_id.clone(),
                    model: profile.model.clone(),
                    label,
                    background: false,
                    max_turns: profile.max_turns,
                    token_budget: profile.token_budget,
                    timeout_seconds: profile.timeout_seconds,
                    workspace: prepared.workspace.clone(),
                    root_workspace: prepared.root_workspace.clone(),
                    worktree_branch: prepared.branch.clone(),
                    dedicated_worktree: prepared.dedicated,
                })
                .await;
            let mut result = if let Some(report) = cached_report {
                Ok(format!(
                    "[Recovered completed child report; tools and verifier were not rerun.]\n{report}"
                ))
            } else {
                let result = run_subagent(run, self.sink.clone(), agent_id, None).await;
                attach_worktree_report(result, &prepared).await
            };
            if !recovered_report
                && let Ok(report) = &result
                && let Some(home) = &self.state_home
                && let Err(error) = super::dispatch_store::save_report(home, agent_id, report)
            {
                result = Err(error);
            }
            if !recovered_report {
                record_profile_result(&self.failures, &profile_id, &result);
            }
            self.sink
                .emit(AgentEvent::SubagentCompleted {
                    id: agent_id,
                    status: subagent_lifecycle_status(&result),
                    report: result
                        .as_ref()
                        .ok()
                        .cloned()
                        .or_else(|| result.as_ref().err().map(ToString::to_string))
                        .map(bounded_report),
                })
                .await;
            result
        }
    }

    /// Decide whether a verifier command may run unattended.
    ///
    /// An honest verifier writes — a build produces artifacts, a test writes
    /// fixtures — so "read-only only" is the wrong gate: it would push every
    /// real verifier back to "trust the model's own claim", which is worse
    /// than running the command. What cannot be given up is that *something*
    /// judges it. Statically read-only commands pass; a destructive shape is
    /// refused outright; everything in between goes to the same judge the
    /// main agent's `run_command` uses. With no judge configured there is no
    /// one to ask and no approval card to show, so the answer is no.
    async fn gate_verifier_command(
        &self,
        verifier: TaskVerifier,
    ) -> Result<TaskVerifier, AgentError> {
        let command = verifier.command.trim();
        if command.is_empty() {
            return Err(AgentError::Subagent(
                "verifier.command cannot be empty".to_owned(),
            ));
        }
        // 父会话在 full-access：不请判官，照跑；只有破坏性形状仍然拒绝——Worker 没有
        // 审批卡，这类命令在主 Agent 那里也不会被 full-access 放过去。
        if self
            .parent_approval_mode
            .as_ref()
            .is_some_and(|parent| parent.get() == crate::ApprovalMode::FullAccess)
        {
            if crate::safety::classify(command) == CommandSafety::AlwaysDangerous {
                return Err(AgentError::Subagent(format!(
                    "verifier command has a destructive shape and will not be run unattended: {command}"
                )));
            }
            return Ok(verifier);
        }
        if crate::tools::child_command_is_sensitive(command) {
            return Err(AgentError::Subagent(format!(
                "verifier command is credential-sensitive and will not be sent to the AI judge: {command}. The parent may request exact human authorization through ops_runner target_command instead"
            )));
        }
        match crate::safety::classify(command) {
            CommandSafety::AlwaysSafe => return Ok(verifier),
            CommandSafety::AlwaysDangerous => {
                return Err(AgentError::Subagent(format!(
                    "verifier command has a destructive shape and will not be run unattended: {command}"
                )));
            }
            CommandSafety::NeedsJudgment => {}
        }
        let Some(judge) = &self.safety_judge else {
            return Err(AgentError::Subagent(format!(
                "verifier command needs review but no safety judge is configured, and a subagent has no approval card to show: {command}"
            )));
        };
        match judge
            .judge(JudgeRequest {
                tool: "subagent_verifier".to_owned(),
                command: command.to_owned(),
                task_context: "run an unattended verifier for a delegated coding task".to_owned(),
            })
            .await
        {
            JudgeVerdict::Allow => Ok(verifier),
            JudgeVerdict::Deny => Err(AgentError::Subagent(format!(
                "the safety judge ({}) refused this verifier command: {command}. The parent may request exact human authorization through ops_runner target_command",
                judge.model()
            ))),
            JudgeVerdict::Unavailable(reason) => Err(AgentError::Subagent(format!(
                "the safety judge ({}) could not review this verifier command ({reason}): {command}. The parent may request exact human authorization through ops_runner target_command",
                judge.model()
            ))),
        }
    }

    async fn plan_workspace(
        &self,
        agent_id: uuid::Uuid,
        policy: SubagentWorktreePolicy,
    ) -> Result<PreparedSubagentWorkspace, AgentError> {
        if let Some(manager) = &self.worktrees {
            return manager.plan(&self.workspace, agent_id, policy).await;
        }
        self.prepare_workspace(agent_id, policy).await
    }

    async fn prepare_workspace(
        &self,
        agent_id: uuid::Uuid,
        policy: SubagentWorktreePolicy,
    ) -> Result<PreparedSubagentWorkspace, AgentError> {
        if policy == SubagentWorktreePolicy::Shared {
            return Ok(PreparedSubagentWorkspace {
                workspace: self.workspace.clone(),
                root_workspace: self.workspace.clone(),
                branch: None,
                dedicated: false,
            });
        }
        let manager = self.worktrees.as_ref().ok_or_else(|| {
            AgentError::Subagent("dedicated subagent worktree root is not configured".to_owned())
        })?;
        manager.prepare(&self.workspace, agent_id, policy).await
    }
}

async fn attach_worktree_report(
    result: Result<String, AgentError>,
    prepared: &PreparedSubagentWorkspace,
) -> Result<String, AgentError> {
    let note = match worktree_result_note(prepared).await {
        Ok(Some(note)) => note,
        Ok(None) => return result,
        Err(error) => format!("[WillDeep Worktree] Unable to inspect retained workspace: {error}"),
    };
    match result {
        Ok(report) => Ok(format!("{report}\n\n{note}")),
        Err(AgentError::SubagentPartial {
            reason,
            turns,
            report,
        }) => Err(AgentError::SubagentPartial {
            reason,
            turns,
            report: format!("{report}\n\n{note}"),
        }),
        Err(AgentError::Subagent(error)) => Err(AgentError::Subagent(format!("{error}\n\n{note}"))),
        Err(error) => Err(error),
    }
}

fn remap_approved_targets(
    targets: Option<BTreeSet<PathBuf>>,
    prepared: &PreparedSubagentWorkspace,
) -> Result<Option<BTreeSet<PathBuf>>, AgentError> {
    let Some(targets) = targets else {
        return Ok(None);
    };
    if !prepared.dedicated {
        return Ok(Some(targets));
    }
    targets
        .into_iter()
        .map(|target| {
            let relative = target.strip_prefix(&prepared.root_workspace).map_err(|_| {
                AgentError::Subagent(format!(
                    "approved write target {} is outside root workspace {}",
                    target.display(),
                    prepared.root_workspace.display()
                ))
            })?;
            Ok(prepared.workspace.join(relative))
        })
        .collect::<Result<BTreeSet<_>, AgentError>>()
        .map(Some)
}

fn background_lifecycle_status(status: &BackgroundTaskStatus) -> SubagentLifecycleStatus {
    match status {
        BackgroundTaskStatus::Completed => SubagentLifecycleStatus::Completed,
        BackgroundTaskStatus::Partial => SubagentLifecycleStatus::Partial,
        BackgroundTaskStatus::Blocked => SubagentLifecycleStatus::Blocked,
        BackgroundTaskStatus::Killed => SubagentLifecycleStatus::Cancelled,
        BackgroundTaskStatus::Queued
        | BackgroundTaskStatus::Running
        | BackgroundTaskStatus::Failed
        | BackgroundTaskStatus::TimedOut
        | BackgroundTaskStatus::LaunchFailed => SubagentLifecycleStatus::Failed,
    }
}

fn subagent_lifecycle_status(result: &Result<String, AgentError>) -> SubagentLifecycleStatus {
    match result {
        Ok(_) => SubagentLifecycleStatus::Completed,
        Err(AgentError::SubagentPartial { .. }) => SubagentLifecycleStatus::Partial,
        Err(AgentError::TeamBudgetBlocked(_) | AgentError::SubagentBlocked(_)) => {
            SubagentLifecycleStatus::Blocked
        }
        Err(AgentError::Tool(ToolError::ApprovalDenied(_))) => SubagentLifecycleStatus::Blocked,
        Err(_) => SubagentLifecycleStatus::Failed,
    }
}

fn subagent_task_result(result: Result<String, AgentError>) -> TaskResult {
    match result {
        Err(AgentError::TeamBudgetBlocked(message) | AgentError::SubagentBlocked(message)) => {
            TaskResult {
                status: BackgroundTaskStatus::Blocked,
                exit_code: None,
                output: message,
            }
        }
        Err(error @ AgentError::SubagentPartial { .. }) => TaskResult {
            status: BackgroundTaskStatus::Partial,
            exit_code: None,
            output: error.to_string(),
        },
        Ok(report) => TaskResult {
            status: BackgroundTaskStatus::Completed,
            exit_code: Some(0),
            output: report,
        },
        Err(AgentError::Tool(ToolError::ApprovalDenied(message))) => TaskResult {
            status: BackgroundTaskStatus::Blocked,
            exit_code: None,
            output: message,
        },
        Err(error) => TaskResult {
            status: BackgroundTaskStatus::Failed,
            exit_code: Some(1),
            output: error.to_string(),
        },
    }
}

/// 熔断打开后多久放行一次试探。没有这一档，熔断一旦打开就永远关不上：
/// 只有成功才能清零，而打开期间根本不会有成功。
const CIRCUIT_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// 一个工种的熔断状态：连续失败次数、最后一次失败的时刻、是否有试探在跑。
#[derive(Clone, Copy, Debug)]
pub(super) struct CircuitState {
    count: usize,
    last_failure: std::time::Instant,
    probing: bool,
}

/// 派工前过熔断：关着就放行；开着且冷却未到就拒；冷却已到放行**一次**试探
/// （半开），试探在跑时其余派工照旧被拒，免得并行派工一起冲过去。
fn circuit_admit(
    failures: &mut BTreeMap<String, CircuitState>,
    profile: &str,
    max_consecutive_failures: usize,
    cooldown: std::time::Duration,
    now: std::time::Instant,
) -> Result<(), String> {
    let Some(state) = failures.get_mut(profile) else {
        return Ok(());
    };
    if state.count < max_consecutive_failures {
        return Ok(());
    }
    if state.probing {
        return Err(format!(
            "profile {profile} circuit is open after {} consecutive failures; a trial run is already in flight",
            state.count
        ));
    }
    let waited = now.saturating_duration_since(state.last_failure);
    if waited < cooldown {
        return Err(format!(
            "profile {profile} circuit is open after {} consecutive failures; one trial run is allowed in {}s. Use another profile or handle the task directly meanwhile",
            state.count,
            (cooldown - waited).as_secs().max(1)
        ));
    }
    state.probing = true;
    Ok(())
}

fn record_profile_result(
    failures: &Mutex<BTreeMap<String, CircuitState>>,
    profile: &str,
    result: &Result<String, AgentError>,
) {
    let Ok(mut failures) = failures.lock() else {
        return;
    };
    if result.is_ok() {
        failures.remove(profile);
    } else {
        let now = std::time::Instant::now();
        let state = failures.entry(profile.to_owned()).or_insert(CircuitState {
            count: 0,
            last_failure: now,
            probing: false,
        });
        state.count = state.count.saturating_add(1);
        state.last_failure = now;
        state.probing = false;
    }
}

#[cfg(test)]
mod tests;
