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
}

pub async fn handle_session(action: SessionAction) -> Result<()> {
    let home = crate::config::willdeep_home()?;
    match action {
        SessionAction::List => session_store::list_sessions_cli(&home).await,
        SessionAction::Get { id } => session_store::show_session_cli(&home, id).await,
        SessionAction::Turns { id } => session_store::list_turns_cli(&home, id).await,
        SessionAction::Stop { id } => session_store::stop_session_cli(&home, id).await,
    }
}
