use super::*;

#[derive(Clone, Debug, Subcommand)]
pub enum DaemonAction {
    /// Start the local Runtime Daemon in the background.
    Start,
    /// Show Runtime Daemon health and endpoint information.
    Status,
    /// Show negotiated Runtime protocol objects, operations, transports and limits.
    Capabilities,
    /// Gracefully stop the local Runtime Daemon.
    Stop,
    /// Drain active work and hand the Runtime lease to this executable.
    Upgrade {
        /// Maximum seconds to wait for active work to finish.
        #[arg(
            long,
            default_value_t = 300,
            value_parser = clap::value_parser!(u64).range(1..=86_400)
        )]
        timeout: u64,
        /// Restart even when the running Runtime already has this version.
        #[arg(long)]
        force: bool,
    },
    /// Print Runtime Daemon logs.
    Logs {
        #[arg(long, default_value_t = 100)]
        lines: usize,
        #[arg(short, long)]
        follow: bool,
    },
    /// Submit a non-interactive Harness task to the persistent Runtime.
    Submit {
        /// Workspace root available to the task.
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// Provider profile from config.toml.
        #[arg(long)]
        profile: Option<String>,
        /// Optional model override for the selected Provider profile.
        #[arg(long)]
        model: Option<String>,
        /// TOML configuration path inherited by the task.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Prompt sent through private stdin rather than process arguments.
        #[arg(value_name = "PROMPT", num_args = 1.., trailing_var_arg = true)]
        prompt: Vec<String>,
    },
    /// List tasks owned by the Runtime.
    Tasks,
    /// Show one Runtime-owned task.
    Task { id: uuid::Uuid },
    /// List structured agents owned by the Runtime.
    Agents,
    /// Show one Runtime-owned agent.
    Agent { id: uuid::Uuid },
    /// Show queue positions, active Run and cumulative usage for a parent team.
    AgentTeam { id: uuid::Uuid },
    /// Read one page of durable worker runs; use the returned cursor for more.
    AgentRuns {
        id: uuid::Uuid,
        #[arg(long)]
        after: Option<u64>,
    },
    /// Report delegation metrics over the agents the Runtime still holds.
    AgentMetrics {
        /// Print one JSON object instead of the tab-separated report.
        #[arg(long)]
        json: bool,
        /// Only count child runs created inside this window: `7d`, `24h`,
        /// `2w`, a UTC date (`2026-09-14`) or a UTC timestamp.
        #[arg(long, value_name = "WINDOW")]
        since: Option<String>,
    },
    /// Preview an exact Diff and conflict check for a child Agent worktree.
    AgentWorktreeReview { id: uuid::Uuid },
    /// Apply an exact reviewed child Agent patch to its root Workspace.
    MergeAgentWorktree {
        id: uuid::Uuid,
        #[arg(long)]
        review: String,
        #[arg(long)]
        yes: bool,
    },
    /// Audit managed, missing, reviewable, merged and unknown child Worktrees.
    WorktreesAudit,
    /// Move an exact clean or merged Worktree into recoverable quarantine.
    QuarantineAgentWorktree {
        id: uuid::Uuid,
        #[arg(long)]
        snapshot: String,
        #[arg(long)]
        yes: bool,
    },
    /// Create a persistent interactive Runtime Session.
    CreateSession {
        #[arg(long)]
        workspace: Option<PathBuf>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        title: Option<String>,
    },
    /// List persistent interactive Runtime Sessions.
    Sessions,
    /// List registered Runtime Workspaces.
    Workspaces,
    /// Register or update a Runtime Workspace and its independent settings.
    RegisterWorkspace {
        #[arg(value_name = "PATH")]
        root: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_enum, default_value_t = workspace_store::WorkspaceAccess::WorkspaceWrite)]
        access: workspace_store::WorkspaceAccess,
        #[arg(long)]
        provider_profile: Option<String>,
        #[arg(long = "skill")]
        skills: Vec<String>,
        #[arg(long = "mcp-server")]
        mcp_servers: Vec<String>,
    },
    /// Make one registered Workspace the default for new clients.
    ActivateWorkspace { id: uuid::Uuid },
    /// Remove a Workspace registration without deleting files or Sessions.
    RemoveWorkspace {
        id: uuid::Uuid,
        #[arg(long)]
        yes: bool,
    },
    /// Show one persistent Runtime Session.
    Session { id: uuid::Uuid },
    /// Search persistent Runtime Sessions by title or message text.
    SearchSessions {
        #[arg(value_name = "QUERY", num_args = 0.., trailing_var_arg = true)]
        query: Vec<String>,
        #[arg(long)]
        workspace: Option<PathBuf>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        updated_after: Option<u64>,
        #[arg(long)]
        updated_before: Option<u64>,
    },
    /// Rename a persistent Runtime Session.
    RenameSession {
        id: uuid::Uuid,
        #[arg(value_name = "TITLE", num_args = 1.., trailing_var_arg = true)]
        title: Vec<String>,
    },
    /// Fork a stable Session snapshot without copying its Turn history.
    ForkSession {
        id: uuid::Uuid,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, value_name = "TURN_ID")]
        through_turn: Option<uuid::Uuid>,
        #[arg(long, value_name = "PROFILE")]
        provider_profile: Option<String>,
        #[arg(long)]
        model: Option<String>,
    },
    /// Rewind an inactive Runtime Session to the end of a completed Turn.
    RewindSession {
        id: uuid::Uuid,
        /// Keep history through this Turn; omit to rewind to the beginning.
        #[arg(long, value_name = "TURN_ID")]
        through_turn: Option<uuid::Uuid>,
        /// Also restore workspace files to the checkpoint taken before the first dropped Turn.
        #[arg(long)]
        restore_workspace: bool,
    },
    /// Archive an inactive Runtime Session.
    ArchiveSession { id: uuid::Uuid },
    /// Restore an archived Runtime Session to Idle.
    UnarchiveSession { id: uuid::Uuid },
    /// Export a Runtime Session as a credential-free JSON snapshot.
    ExportSession {
        id: uuid::Uuid,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Permanently delete an inactive Runtime Session and its local history.
    DeleteSession {
        id: uuid::Uuid,
        #[arg(long)]
        yes: bool,
    },
    /// Queue a user Turn in a persistent Runtime Session.
    SubmitTurn {
        session_id: uuid::Uuid,
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
        #[arg(value_name = "PROMPT", num_args = 1.., trailing_var_arg = true)]
        prompt: Vec<String>,
    },
    /// List Turns in a Runtime Session.
    Turns { session_id: uuid::Uuid },
    /// Show one Runtime Turn.
    Turn { id: uuid::Uuid },
    /// Stop a queued or running Runtime Turn.
    StopTurn { id: uuid::Uuid },
    /// Request cancellation of a running background child Agent.
    StopAgent { id: uuid::Uuid },
    /// Retry a terminal background child Agent.
    RetryAgent { id: uuid::Uuid },
    /// Continue a child Agent in its original conversation and workspace.
    ContinueAgent {
        id: uuid::Uuid,
        #[arg(value_name = "TASK", num_args = 1.., trailing_var_arg = true)]
        task: Vec<String>,
    },
    /// Add instructions to a running background child Agent.
    InstructAgent {
        id: uuid::Uuid,
        #[arg(value_name = "INSTRUCTION", num_args = 1.., trailing_var_arg = true)]
        instruction: Vec<String>,
    },
    /// Capture a structured Diff snapshot for a Runtime Workspace.
    DiffSnapshot {
        #[arg(long)]
        workspace: PathBuf,
    },
    /// Show one file from an exact Diff snapshot.
    DiffFile {
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        snapshot: String,
        #[arg(long)]
        path: String,
        #[arg(long, value_enum, default_value_t = diff_review::DiffArea::Combined)]
        area: diff_review::DiffArea,
    },
    /// Save a decision for one file in an exact Diff snapshot.
    DiffReview {
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        snapshot: String,
        #[arg(long)]
        path: String,
        #[arg(long, value_enum)]
        decision: diff_review::ReviewDecision,
        #[arg(long)]
        note: Option<String>,
    },
    /// List test verifications bound to an exact Diff snapshot.
    DiffVerifications {
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        snapshot: String,
    },
    /// List Turn, Task, Agent, and Tool attribution for an exact Diff snapshot.
    DiffAttributions {
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        snapshot: String,
    },
    /// Preview an exact commit, tag, and push target without mutating Git.
    DiffCommitPreview {
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        snapshot: String,
        #[arg(long)]
        message: String,
        #[arg(long, default_value = "origin")]
        remote: String,
        #[arg(long)]
        tag: Option<String>,
    },
    /// Safely revert one file from an exact Diff snapshot.
    DiffRevert {
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        snapshot: String,
        #[arg(long)]
        path: String,
        #[arg(long, value_enum, default_value_t = diff_review::DiffArea::Combined)]
        area: diff_review::DiffArea,
    },
    /// Request cancellation of a Runtime-owned task.
    Cancel { id: uuid::Uuid },
    /// List approvals and questions currently blocking Runtime tasks.
    Pending,
    /// Resolve a pending Runtime approval.
    Resolve {
        id: uuid::Uuid,
        #[arg(value_enum)]
        decision: ApprovalArg,
    },
    /// Answer a pending Runtime question.
    Answer {
        id: uuid::Uuid,
        #[arg(value_name = "ANSWER", num_args = 1.., trailing_var_arg = true)]
        answer: Vec<String>,
    },
    /// Control the mobile relay that lets WillDeep Mobile follow this Runtime.
    Mobile {
        #[command(subcommand)]
        action: MobileAction,
    },
    /// Internal foreground server entry used by `daemon start`.
    #[command(hide = true)]
    Run,
    /// Internal background Shell supervisor connected to its parent by stdin.
    #[command(hide = true)]
    BackgroundSupervisor,
}

#[derive(Clone, Debug, Subcommand)]
pub enum SessionAction {
    /// List persistent Runtime Sessions.
    List,
    /// Show one Session and its active Turn.
    Get { id: uuid::Uuid },
    /// List Turns owned by one Session.
    Turns { id: uuid::Uuid },
    /// Stop the Session's active or queued Turn.
    Stop { id: uuid::Uuid },
    /// Delegate a read-only investigation to an independent conversation.
    Child {
        #[command(subcommand)]
        action: ChildSessionAction,
    },
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum ChildContextArg {
    Fresh,
    Fork,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum ChildWorkspaceArg {
    SharedReadOnly,
    Worktree,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum ChildWorktreeBaseArg {
    Head,
    Snapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ChildSubscriptionArg {
    None,
    Inbox,
    #[value(name = "auto_wake", alias = "auto-wake")]
    AutoWake,
}

impl ChildSubscriptionArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Inbox => "inbox",
            Self::AutoWake => "auto_wake",
        }
    }
}

#[derive(Clone, Debug, Subcommand)]
pub enum ChildSubscriptionAction {
    Set {
        #[arg(long)]
        parent: uuid::Uuid,
        assignment: uuid::Uuid,
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        expected_subscription_revision: u64,
        #[arg(long, value_enum)]
        subscription: ChildSubscriptionArg,
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
    },
}

#[derive(Clone, Debug, Subcommand)]
pub enum ChildSessionAction {
    /// Start a fresh or forked child conversation, optionally in an isolated worktree.
    Start {
        #[arg(long)]
        parent: uuid::Uuid,
        #[arg(long)]
        title: String,
        #[arg(long, value_enum, default_value = "fresh")]
        context: ChildContextArg,
        /// Completed parent Turn to inherit when --context fork is used.
        #[arg(long)]
        through_turn: Option<uuid::Uuid>,
        #[arg(
            long = "workspace-mode",
            id = "child_workspace_mode",
            value_name = "WORKSPACE_MODE",
            value_enum,
            default_value = "shared-read-only"
        )]
        workspace: ChildWorkspaceArg,
        #[arg(long, value_enum, default_value = "head")]
        worktree_base: ChildWorktreeBaseArg,
        /// Optional model override from the parent Session's Provider profile.
        #[arg(long = "child-model", id = "child_model", value_name = "MODEL")]
        model: Option<String>,
        #[arg(long, value_enum)]
        subscription: Option<ChildSubscriptionArg>,
        #[arg(long = "depends-on", value_name = "ASSIGNMENT_UUID", action = clap::ArgAction::Append)]
        depends_on: Vec<uuid::Uuid>,
        /// Reuse this key after a lost response to avoid duplicate work.
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
        #[arg(value_name = "PROMPT", num_args = 1.., trailing_var_arg = true)]
        prompt: Vec<String>,
    },
    /// List assignments, including waiting work and terminal results.
    List {
        #[arg(long)]
        parent: uuid::Uuid,
        #[arg(long)]
        after: Option<uuid::Uuid>,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
    },
    /// Read an assignment and its durable result.
    Get {
        #[arg(long)]
        parent: uuid::Uuid,
        assignment: uuid::Uuid,
    },
    Link {
        #[arg(long)]
        parent: uuid::Uuid,
        child: uuid::Uuid,
    },
    Takeover {
        #[arg(long)]
        parent: uuid::Uuid,
        child: uuid::Uuid,
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        expected_control_revision: u64,
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
        #[arg(long)]
        yes: bool,
    },
    Detach {
        #[arg(long)]
        parent: uuid::Uuid,
        child: uuid::Uuid,
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        expected_control_revision: u64,
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
        #[arg(long)]
        yes: bool,
    },
    Subscription {
        #[command(subcommand)]
        action: ChildSubscriptionAction,
    },
    /// Continue the same child conversation with a new assignment.
    Continue {
        #[arg(long)]
        parent: uuid::Uuid,
        child: uuid::Uuid,
        #[arg(long, value_enum)]
        subscription: Option<ChildSubscriptionArg>,
        #[arg(long = "depends-on", value_name = "ASSIGNMENT_UUID", action = clap::ArgAction::Append)]
        depends_on: Vec<uuid::Uuid>,
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
        #[arg(value_name = "PROMPT", num_args = 1.., trailing_var_arg = true)]
        prompt: Vec<String>,
    },
    /// Retry a failed or interrupted assignment, preserving its old result.
    Retry {
        #[arg(long)]
        parent: uuid::Uuid,
        assignment: uuid::Uuid,
        #[arg(long)]
        expected_result_revision: u64,
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
    },
    /// Stop only the Turn belonging to this assignment.
    Stop {
        #[arg(long)]
        parent: uuid::Uuid,
        assignment: uuid::Uuid,
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
    },
}

/// Client surfaces share the coordinator and its persisted request identity.
pub(crate) async fn child_operation(
    home: &Path,
    operation: &str,
    params: serde_json::Value,
    request_id: Option<uuid::Uuid>,
) -> Result<serde_json::Value> {
    child_operation_in_language(
        home,
        operation,
        params,
        request_id,
        crate::i18n::Language::default(),
    )
    .await
}

pub(crate) async fn child_operation_in_language(
    home: &Path,
    operation: &str,
    params: serde_json::Value,
    request_id: Option<uuid::Uuid>,
    language: crate::i18n::Language,
) -> Result<serde_json::Value> {
    anyhow::ensure!(
        matches!(
            operation,
            "session.child.start"
                | "session.child.proposal.list"
                | "session.child.proposal.start"
                | "session.child.proposal.dismiss"
                | "session.child.continue"
                | "session.child.retry"
                | "session.child.task.get"
                | "session.child.task.list"
                | "session.child.task.stop"
                | "session.child.link.get"
                | "session.child.takeover"
                | "session.child.detach"
                | "session.child.task.subscription.set"
        ),
        "unsupported child session operation"
    );
    let request_id = request_id.unwrap_or_else(uuid::Uuid::new_v4);
    let context = || format!("request_id={request_id}");
    let state = state_for_operation(home, operation, &params)
        .await
        .with_context(context)?;
    let client = runtime_client(&state).with_context(context)?;
    let capabilities = client
        .capabilities(None)
        .await
        .with_context(context)?
        .into_result()
        .with_context(context)?;
    validate_child_capabilities(&capabilities, operation, &params, language)
        .with_context(context)?;
    // Retrying a transport failure retains the outer ApiRequest key. Application
    // refusals are returned immediately, never replayed as a new assignment.
    for attempt in 0..2 {
        match client.call::<_, serde_json::Value>(operation, &params, Some(request_id)).await {
            Ok(response) => return response.into_result().map_err(anyhow::Error::from).with_context(context),
            Err(_) if attempt == 0 => continue,
            Err(error) => return Err(error).with_context(|| format!(
                "child session response unconfirmed (request_id={request_id}); retry with the same key"
            )),
        }
    }
    unreachable!()
}

fn child_message(zh: &'static str, en: &'static str, ja: &'static str) -> &'static str {
    crate::i18n::Language::default().text(zh, en, ja)
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct UnsupportedChildOperation(pub(crate) &'static str);

fn validate_child_capabilities(
    capabilities: &willdeep_runtime_protocol::RuntimeCapabilities,
    operation: &str,
    params: &serde_json::Value,
    language: crate::i18n::Language,
) -> Result<()> {
    let child_message = |zh, en, ja| language.text(zh, en, ja);
    use willdeep_runtime_protocol::Capability;
    let supports = |capability| capabilities.capabilities.contains(&capability);
    anyhow::ensure!(
        supports(Capability::SessionChildren)
            && capabilities
                .operations
                .iter()
                .any(|candidate| candidate == operation),
        UnsupportedChildOperation(child_message(
            "当前 Runtime 不支持此子会话操作，请更新 Runtime",
            "The Runtime does not support this child operation; update it",
            "Runtime がこの子セッション操作をサポートしていません。更新してください"
        ))
    );
    if matches!(
        operation,
        "session.child.link.get" | "session.child.takeover" | "session.child.detach"
    ) {
        anyhow::ensure!(
            supports(Capability::SessionChildrenControl),
            "{}",
            child_message(
                "当前 Runtime 不支持接管或脱离，请更新 Runtime",
                "The Runtime does not support child control; update it",
                "Runtime が子セッションの制御をサポートしていません。更新してください"
            )
        );
    }
    if operation == "session.child.task.subscription.set" || params.get("subscription").is_some() {
        anyhow::ensure!(
            supports(Capability::SessionChildrenSubscriptions),
            "{}",
            child_message(
                "当前 Runtime 不支持通知策略，请更新 Runtime",
                "The Runtime does not support result subscriptions; update it",
                "Runtime が通知設定をサポートしていません。更新してください"
            )
        );
    }
    if params.get("depends_on").is_some() {
        anyhow::ensure!(
            supports(Capability::SessionChildrenDependencies),
            "{}",
            child_message(
                "当前 Runtime 不支持前置任务，请更新 Runtime",
                "The Runtime does not support task dependencies; update it",
                "Runtime が前提タスクをサポートしていません。更新してください"
            )
        );
    }
    Ok(())
}

fn add_assignment_options(
    params: &mut serde_json::Value,
    subscription: Option<ChildSubscriptionArg>,
    depends_on: Vec<uuid::Uuid>,
) -> Result<()> {
    let limit = willdeep_runtime_protocol::child_session::CHILD_SESSION_DEPENDENCY_LIMIT;
    anyhow::ensure!(
        depends_on.len() <= limit,
        "{} ({limit})",
        child_message(
            "前置任务数量超过上限",
            "Too many prerequisites",
            "前提タスクが上限を超えています"
        )
    );
    let unique: HashSet<_> = depends_on.iter().collect();
    anyhow::ensure!(
        unique.len() == depends_on.len(),
        "{}",
        child_message(
            "前置任务不能重复",
            "Prerequisites must be unique",
            "前提タスクを重複指定できません"
        )
    );
    if let Some(subscription) = subscription {
        params["subscription"] = subscription.as_str().into();
    }
    if !depends_on.is_empty() {
        params["depends_on"] = serde_json::to_value(depends_on)?;
    }
    Ok(())
}

/// Shared Runtime call for surfaces that need a read-only capability query
/// (for example the Web fork picker) without duplicating transport setup.
pub(crate) async fn runtime_operation(
    home: &Path,
    operation: &str,
    params: serde_json::Value,
    request_id: Option<uuid::Uuid>,
) -> Result<serde_json::Value> {
    let state = state_for_operation(home, operation, &params).await?;
    let client = runtime_client(&state)?;
    let request_id = request_id.unwrap_or_else(uuid::Uuid::new_v4);
    client
        .call::<_, serde_json::Value>(operation, &params, Some(request_id))
        .await?
        .into_result()
        .map_err(Into::into)
}

fn child_request(
    action: ChildSessionAction,
) -> Result<(&'static str, serde_json::Value, Option<uuid::Uuid>)> {
    Ok(match action {
        ChildSessionAction::Start {
            parent,
            title,
            context,
            through_turn,
            workspace,
            worktree_base,
            model,
            subscription,
            depends_on,
            request_id,
            prompt,
        } => {
            let mut params = serde_json::json!({"parent_session_id":parent, "title":title,
                "prompt":prompt.join(" "),
                "context": match context { ChildContextArg::Fresh => "fresh", ChildContextArg::Fork => "fork" },
                "through_turn_id": through_turn,
                "workspace_mode": match workspace { ChildWorkspaceArg::SharedReadOnly => "shared_read_only", ChildWorkspaceArg::Worktree => "worktree" },
                "worktree_base": match worktree_base { ChildWorktreeBaseArg::Head => "head", ChildWorktreeBaseArg::Snapshot => "snapshot" },
                "model": model});
            add_assignment_options(&mut params, subscription, depends_on)?;
            ("session.child.start", params, request_id)
        }
        ChildSessionAction::List {
            parent,
            after,
            limit,
        } => (
            "session.child.task.list",
            serde_json::json!({"parent_session_id":parent,"after":after,"limit":limit}),
            None,
        ),
        ChildSessionAction::Get { parent, assignment } => (
            "session.child.task.get",
            serde_json::json!({"parent_session_id":parent,"assignment_id":assignment}),
            None,
        ),
        ChildSessionAction::Link { parent, child } => (
            "session.child.link.get",
            serde_json::json!({"parent_session_id":parent,"child_session_id":child}),
            None,
        ),
        ChildSessionAction::Takeover {
            parent,
            child,
            expected_control_revision,
            request_id,
            yes,
        }
        | ChildSessionAction::Detach {
            parent,
            child,
            expected_control_revision,
            request_id,
            yes,
        } => {
            let operation = if matches!(action, ChildSessionAction::Takeover { .. }) {
                "session.child.takeover"
            } else {
                "session.child.detach"
            };
            anyhow::ensure!(
                yes,
                "{}",
                child_message(
                    "请确认：父会话将失去控制权和自动通知，当前任务继续运行；接管后可继续脱离，但不能恢复委派控制。重试时添加 --yes",
                    "Confirm: the parent loses control and automatic results; current work keeps running. Takeover can proceed to detach, but delegation cannot be restored. Repeat with --yes",
                    "確認：親は制御権と自動通知を失い、現在のタスクは続行します。引継ぎ後の切り離しは可能ですが、委任制御には戻せません。--yes を追加してください"
                )
            );
            anyhow::ensure!(
                expected_control_revision > 0,
                "{}",
                child_message(
                    "控制版本必须大于零",
                    "Control revision must be positive",
                    "制御バージョンは正の値が必要です"
                )
            );
            (
                operation,
                serde_json::json!({"parent_session_id":parent,"child_session_id":child,"expected_control_revision":expected_control_revision}),
                request_id,
            )
        }
        ChildSessionAction::Subscription {
            action:
                ChildSubscriptionAction::Set {
                    parent,
                    assignment,
                    expected_subscription_revision,
                    subscription,
                    request_id,
                },
        } => (
            "session.child.task.subscription.set",
            serde_json::json!({"parent_session_id":parent,"assignment_id":assignment,"expected_subscription_revision":expected_subscription_revision,"subscription":subscription.as_str()}),
            request_id,
        ),
        ChildSessionAction::Continue {
            parent,
            child,
            subscription,
            depends_on,
            request_id,
            prompt,
        } => {
            let mut params = serde_json::json!({"parent_session_id":parent,"child_session_id":child,"prompt":prompt.join(" ")});
            add_assignment_options(&mut params, subscription, depends_on)?;
            ("session.child.continue", params, request_id)
        }
        ChildSessionAction::Retry {
            parent,
            assignment,
            expected_result_revision,
            request_id,
        } => (
            "session.child.retry",
            serde_json::json!({"parent_session_id":parent,"assignment_id":assignment,"expected_result_revision":expected_result_revision}),
            request_id,
        ),
        ChildSessionAction::Stop {
            parent,
            assignment,
            request_id,
        } => (
            "session.child.task.stop",
            serde_json::json!({"parent_session_id":parent,"assignment_id":assignment}),
            request_id,
        ),
    })
}

async fn handle_child_session(home: &Path, action: ChildSessionAction) -> Result<()> {
    let (operation, params, request_id) = child_request(action)?;
    let value = child_operation(home, operation, params, request_id).await?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

#[cfg(test)]
#[path = "commands_child_tests.rs"]
mod child_tests;

pub async fn handle_session(action: SessionAction) -> Result<()> {
    let home = crate::config::willdeep_home()?;
    match action {
        SessionAction::List => session_store::list_sessions_cli(&home).await,
        SessionAction::Get { id } => session_store::show_session_cli(&home, id).await,
        SessionAction::Turns { id } => session_store::list_turns_cli(&home, id).await,
        SessionAction::Stop { id } => session_store::stop_session_cli(&home, id).await,
        SessionAction::Child { action } => handle_child_session(&home, action).await,
    }
}
