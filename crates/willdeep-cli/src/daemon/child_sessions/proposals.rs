//! Durable Web suggestions. Only a separate operator action admits child work.
use super::*;
use willdeep_runtime_protocol::ApiRequest;

const PENDING_LIMIT: usize = 32;
const TITLE_LIMIT: usize = 200;

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::daemon) struct Proposal {
    pub id: Uuid,
    pub params: StartChildSessionParams,
    pub created_at: u64,
    #[serde(default)]
    pub dismissed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalRef {
    parent_session_id: Uuid,
    proposal_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParentRef {
    parent_session_id: Uuid,
}

pub(super) fn requires_confirmation(state: &ServerState, request: &ApiRequest) -> Result<bool> {
    if request.operation != "session.child.start" {
        return Ok(false);
    }
    let params: StartChildSessionParams = serde_json::from_value(request.params.clone())?;
    let Some(turn_id) = params.created_from_turn_id else {
        return Ok(false);
    };
    state
        .sessions
        .child_start_requires_confirmation(params.parent_session_id, turn_id)
}

fn assignment<'a>(ledger: &'a Ledger, proposal: &Proposal) -> Option<&'a Assignment> {
    let id = start_request_id(proposal.id);
    ledger.assignments.values().find(|entry| {
        entry.task.request_id == id
            && entry.task.parent_session_id == proposal.params.parent_session_id
    })
}

fn started(ledger: &Ledger, proposal: &Proposal) -> bool {
    assignment(ledger, proposal)
        .is_some_and(|entry| entry.task.accepted_at.is_some() || entry.task.child_turn_id.is_some())
}

/// Called with the existing ledger lock; the originating Turn may already be gone.
pub(in crate::daemon) fn confirmed_request_requires_child_confirmation(
    ledger: &Ledger,
    parent: Uuid,
    request: Uuid,
) -> bool {
    ledger.proposals.values().any(|proposal| {
        proposal.params.parent_session_id == parent && start_request_id(proposal.id) == request
    })
}

fn is_proposal_task(ledger: &Ledger, task: &ChildSessionTask) -> bool {
    confirmed_request_requires_child_confirmation(ledger, task.parent_session_id, task.request_id)
}

/// Failed preparation stays visible until the operator retries or dismisses it.
/// Ordinary child starts retain their automatic recovery behavior.
pub(super) fn awaiting_retry(ledger: &Ledger, task: &ChildSessionTask) -> bool {
    task.accepted_at.is_none()
        && task.child_turn_id.is_none()
        && task.error.is_some()
        && is_proposal_task(ledger, task)
}

pub(super) fn preparation_failed(
    sessions: &RuntimeSessionStore,
    ledger: &mut Ledger,
    id: Uuid,
    error: &anyhow::Error,
) -> Result<()> {
    let entry = ledger
        .assignments
        .get(&id)
        .context("child assignment missing")?;
    if !is_proposal_task(ledger, &entry.task) {
        return Ok(());
    }
    let task = &mut ledger.assignments.get_mut(&id).unwrap().task;
    task.error = Some(willdeep_core::provider::safe_error_detail(&format!(
        "Child preparation failed: {error:#}"
    )));
    task.updated_at = now();
    // The API returns an error, so the transaction's final commit will not run.
    sessions.children.persist(ledger)
}

fn start_request_id(proposal: Uuid) -> Uuid {
    Uuid::new_v5(&proposal, b"operator-confirmed-child")
}

pub(super) fn create(
    state: &ServerState,
    ledger: &mut Ledger,
    request: &ApiRequest,
) -> Result<serde_json::Value> {
    let params: StartChildSessionParams = serde_json::from_value(request.params.clone())?;
    params.validate().map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        !params.title.trim().is_empty() && params.title.chars().count() <= TITLE_LIMIT,
        "child title must contain 1 to {TITLE_LIMIT} characters"
    );
    anyhow::ensure!(
        !params.prompt.trim().is_empty()
            && params.prompt.chars().count() <= CHILD_SESSION_BRIEF_MAX_CHARS,
        "child brief must contain 1 to {CHILD_SESSION_BRIEF_MAX_CHARS} characters"
    );
    // Repeated tool events recover the original proposal, including after the Turn ended.
    if let Some(existing) = ledger.proposals.get(&request.request_id) {
        anyhow::ensure!(
            existing.params == params,
            "proposal request_id conflicts with different parameters"
        );
        return Ok(reply(ledger, existing));
    }
    parent(
        &state.sessions,
        ledger,
        params.parent_session_id,
        params.created_from_turn_id,
    )?;
    anyhow::ensure!(
        ledger
            .proposals
            .values()
            .filter(
                |proposal| proposal.params.parent_session_id == params.parent_session_id
                    && !proposal.dismissed
                    && !started(ledger, proposal)
            )
            .count()
            < PENDING_LIMIT,
        "too many pending child proposals; start or dismiss existing proposals first"
    );
    let proposal = Proposal {
        id: request.request_id,
        params,
        created_at: now(),
        dismissed: false,
    };
    let result = reply(ledger, &proposal);
    ledger.proposals.insert(proposal.id, proposal);
    Ok(result)
}

fn reply(ledger: &Ledger, proposal: &Proposal) -> serde_json::Value {
    serde_json::json!({"proposal_id": proposal.id,
        "status": if proposal.dismissed { "dismissed" } else if started(ledger, proposal) { "started" } else { "awaiting_user_start" },
        "title": proposal.params.title,
        "error": assignment(ledger, proposal).and_then(|entry| entry.task.error.as_deref()),
        "instruction": "A filled proposal is available in the conversation. The user must select Start before any child Session or Turn is created. Do not call another tool to start it, poll it, or say it is already running."})
}

pub(super) fn list(ledger: &Ledger, request: &ApiRequest) -> Result<serde_json::Value> {
    let params: ParentRef = serde_json::from_value(request.params.clone())?;
    let mut proposals = ledger
        .proposals
        .values()
        .filter(|proposal| {
            proposal.params.parent_session_id == params.parent_session_id
                && !proposal.dismissed
                && !started(ledger, proposal)
        })
        .collect::<Vec<_>>();
    proposals.sort_by_key(|proposal| (proposal.created_at, proposal.id));
    let proposals = proposals
        .into_iter()
        .map(|proposal| {
            let mut value = serde_json::to_value(proposal)?;
            value["error"] = serde_json::to_value(
                assignment(ledger, proposal).and_then(|entry| entry.task.error.as_deref()),
            )?;
            Ok(value)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(serde_json::json!({"proposals": proposals}))
}

fn owned<'a>(ledger: &'a Ledger, request: &ApiRequest) -> Result<&'a Proposal> {
    let params: ProposalRef = serde_json::from_value(request.params.clone())?;
    let proposal = ledger
        .proposals
        .get(&params.proposal_id)
        .context("child proposal not found")?;
    anyhow::ensure!(
        proposal.params.parent_session_id == params.parent_session_id,
        "child proposal belongs to another parent"
    );
    Ok(proposal)
}

pub(super) fn confirmed_params(
    ledger: &Ledger,
    request: &ApiRequest,
) -> Result<StartChildSessionParams> {
    let proposal = owned(ledger, request)?;
    anyhow::ensure!(!proposal.dismissed, "child proposal was dismissed");
    let mut params = proposal.params.clone();
    // The user may confirm after the originating Turn has ended. Current parent
    // permissions and all dependency/worktree/model checks still run at admission.
    params.created_from_turn_id = None;
    Ok(params)
}

pub(super) fn start(
    state: &ServerState,
    ledger: &mut Ledger,
    request: &ApiRequest,
    preflight: Option<&ChildModelPreflight>,
) -> Result<serde_json::Value> {
    let proposal = owned(ledger, request)?;
    let existing = assignment(ledger, proposal).map(|entry| entry.task.assignment_id);
    let mut start = ApiRequest::new(
        "session.child.start",
        serde_json::to_value(confirmed_params(ledger, request)?)?,
    );
    start.request_id = start_request_id(proposal.id);
    if let Some(id) = existing {
        dependencies::restore_turn(&state.sessions, ledger, id)?;
        let task = &mut ledger.assignments.get_mut(&id).unwrap().task;
        if task.status == ChildSessionTaskStatus::Preparing
            && task.accepted_at.is_none()
            && task.child_turn_id.is_none()
        {
            task.error = None;
        }
    }
    // Preserve assignment and Turn request identities across explicit retries.
    start_assignment(state, ledger, &start, preflight)
}

pub(super) fn dismiss(
    sessions: &RuntimeSessionStore,
    ledger: &mut Ledger,
    request: &ApiRequest,
) -> Result<serde_json::Value> {
    let proposal = owned(ledger, request)?;
    let id = proposal.id;
    let existing = assignment(ledger, proposal).map(|entry| entry.task.assignment_id);
    if let Some(assignment_id) = existing {
        // Enqueue may have committed before its ledger pointer. Never dismiss
        // work that already has a durable Turn, even after a lost acceptance write.
        dependencies::restore_turn(sessions, ledger, assignment_id)?;
    }
    anyhow::ensure!(
        !started(ledger, ledger.proposals.get(&id).unwrap()),
        "child proposal has already started"
    );
    if let Some(assignment_id) = existing {
        let task = &mut ledger.assignments.get_mut(&assignment_id).unwrap().task;
        if !task.status.is_terminal() {
            task.status = ChildSessionTaskStatus::Cancelled;
            task.waiting_on_dependencies = false;
            task.updated_at = now();
        }
        // Dismissing work before acceptance must not wake the parent to handle it.
        if task.subscription != ChildSessionSubscription::None {
            task.subscription = ChildSessionSubscription::None;
            task.notify = ChildSessionNotify::None;
            task.subscription_revision += 1;
        }
        reconcile::record_result(sessions, ledger, assignment_id, None)?;
    }
    ledger
        .proposals
        .get_mut(&id)
        .context("child proposal not found")?
        .dismissed = true;
    Ok(serde_json::json!({"proposal_id":id, "status":"dismissed"}))
}
