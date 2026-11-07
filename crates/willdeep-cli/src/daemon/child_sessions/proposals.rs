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

fn started(ledger: &Ledger, proposal: &Proposal) -> bool {
    let id = start_request_id(proposal.id);
    ledger.assignments.values().any(|entry| {
        entry.task.request_id == id
            && entry.task.parent_session_id == proposal.params.parent_session_id
    })
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
    let mut start = ApiRequest::new(
        "session.child.start",
        serde_json::to_value(confirmed_params(ledger, request)?)?,
    );
    start.request_id = start_request_id(proposal.id);
    // Assignment receipts are the source of truth, including crash recovery and
    // concurrent clicks. No second mutable accepted flag can drift from admission.
    start_assignment(state, ledger, &start, preflight)
}

pub(super) fn dismiss(ledger: &mut Ledger, request: &ApiRequest) -> Result<serde_json::Value> {
    let proposal = owned(ledger, request)?;
    anyhow::ensure!(
        !started(ledger, proposal),
        "child proposal has already started"
    );
    let id = proposal.id;
    ledger
        .proposals
        .get_mut(&id)
        .context("child proposal not found")?
        .dismissed = true;
    Ok(serde_json::json!({"proposal_id":id, "status":"dismissed"}))
}
