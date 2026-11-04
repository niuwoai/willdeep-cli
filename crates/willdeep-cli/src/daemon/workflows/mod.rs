//! External control only admits durable state. The session control Turn owns execution.
use super::control_api::{ApiFailure, ApiResult};
use super::*;
use serde_json::{Value, json};
use willdeep_core::workflow::{WorkflowError, WorkflowLimits, WorkflowSpecV1, WorkflowStore};
use willdeep_runtime_protocol::ApiRequest;
use willdeep_runtime_protocol::agent_team::AgentTeamControl;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowParams {
    parent_session_id: uuid::Uuid,
    #[serde(default)]
    run_id: Option<uuid::Uuid>,
    #[serde(default)]
    request_id: Option<uuid::Uuid>,
    #[serde(default)]
    definition_id: Option<String>,
    #[serde(default)]
    spec: Option<WorkflowSpecV1>,
    #[serde(default)]
    input: Value,
    #[serde(default)]
    limits: WorkflowLimits,
}

pub(super) fn dispatch(state: &ServerState, request: &ApiRequest) -> ApiResult {
    let params: WorkflowParams = serde_json::from_value(request.params.clone())
        .map_err(|error| ApiFailure::invalid(format!("invalid workflow params: {error}")))?;
    let session = state
        .sessions
        .get(params.parent_session_id)
        .map_err(ApiFailure::internal)?
        .ok_or_else(|| ApiFailure::not_found("parent Runtime Session not found"))?;
    let operation = request.operation.trim_start_matches("workflow.");
    if session.status == session_store::RuntimeSessionStatus::Archived
        && matches!(operation, "save" | "run" | "stop" | "retry" | "resume")
    {
        return Err(ApiFailure::conflict(
            "restore the parent Session before controlling workflows",
        ));
    }
    if matches!(operation, "save" | "run" | "retry" | "resume") {
        let workspace = state
            .workspaces
            .list()
            .map_err(ApiFailure::internal)?
            .into_iter()
            .find(|workspace| workspace.root == session.workspace)
            .ok_or_else(|| ApiFailure::not_found("Runtime Workspace not found"))?;
        if workspace
            .access
            .with_session_override(session.approval_mode)
            == WorkspaceAccess::ReadOnly
        {
            return Err(ApiFailure::from_status(StatusCode::FORBIDDEN));
        }
    }
    let store = WorkflowStore::new(&state.home, session.id);
    let run_id = || {
        params
            .run_id
            .ok_or_else(|| ApiFailure::invalid("run_id is required"))
    };
    let request_id = || {
        let identity = params
            .request_id
            .ok_or_else(|| ApiFailure::invalid("request_id is required"))?;
        if identity != request.request_id {
            return Err(ApiFailure::invalid(
                "params.request_id must match the request envelope",
            ));
        }
        Ok(identity)
    };
    let workflow = match operation {
        "schema" => {
            return Ok(
                json!({"schema":willdeep_core::workflow::spec_schema(),"example":willdeep_core::workflow::example_spec()}),
            );
        }
        "list" => {
            return Ok(
                json!({"definitions":store.list_definitions().map_err(failure)?,"runs":store.list_runs().map_err(failure)?}),
            );
        }
        "history" => return Ok(json!({"runs":store.list_runs().map_err(failure)?})),
        "get" => {
            return Ok(
                json!({"workflow":store.get_run(run_id()?).map_err(failure)?.ok_or_else(|| ApiFailure::not_found("workflow run not found"))?}),
            );
        }
        "validate" | "save" => {
            let spec = params
                .spec
                .as_ref()
                .ok_or_else(|| ApiFailure::invalid("spec is required"))?;
            willdeep_core::workflow::validate_spec(spec, &WorkflowLimits::default())
                .map_err(failure)?;
            if operation == "save" {
                store.save_definition(spec).map_err(failure)?;
            }
            return Ok(json!({"valid":true,"definition_id":spec.id}));
        }
        "run" => store
            .start_with_request(
                params
                    .definition_id
                    .as_deref()
                    .ok_or_else(|| ApiFailure::invalid("definition_id is required"))?,
                params.input.clone(),
                params.limits.clone(),
                request_id()?,
            )
            .map_err(failure)?,
        "stop" => {
            request_id()?;
            store.stop(run_id()?).map_err(failure)?
        }
        "retry" => store
            .retry_with_request(run_id()?, request_id()?)
            .map_err(failure)?,
        "resume" => store
            .resume_with_request(run_id()?, request_id()?)
            .map_err(failure)?,
        _ => return Err(ApiFailure::invalid("unsupported workflow operation")),
    };
    // The fallback is durable even while the parent is busy. It runs without a
    // parent model and shares the existing session executor and file lock.
    wake(state, session.id, request_id()?).map_err(ApiFailure::internal)?;
    state
        .events
        .append(
            "workflow.updated",
            format!(
                "session_id={} workflow_run_id={} status={:?}",
                session.id, workflow.id, workflow.status
            ),
        )
        .map_err(ApiFailure::internal)?;
    Ok(json!({"workflow":workflow}))
}

fn failure(error: WorkflowError) -> ApiFailure {
    match error {
        WorkflowError::Storage(_) => ApiFailure::internal(error),
        WorkflowError::State(_) => ApiFailure::conflict(error.to_string()),
        _ => ApiFailure::invalid(error.to_string()),
    }
}

fn wake(state: &ServerState, session_id: uuid::Uuid, identity: uuid::Uuid) -> Result<()> {
    let request_id = uuid::Uuid::new_v5(&identity, b"workflow-control-turn");
    state.sessions.enqueue_turn_observed(
        session_id,
        session_store::CreateRuntimeTurn {
            team_control: Some(AgentTeamControl::RecoverQueue),
            request_id,
            prompt: String::new(),
            attachments: Vec::new(),
            origin_client: Some("workflow-control".into()),
            defer_until_released: false,
        },
    )?;
    state.tasks.schedule_session(session_id)?;
    Ok(())
}

/// Reconstruct the admission-to-wake crash window, including graphs which have
/// no Worker yet. Completed aggregate receipts also need the session executor.
pub(super) fn recover(state: &ServerState) -> Result<usize> {
    let mut queued = 0;
    for session in state.sessions.list()? {
        if session.status == session_store::RuntimeSessionStatus::Archived {
            continue;
        }
        let runs = WorkflowStore::new(&state.home, session.id).list_runs()?;
        let pending: Vec<_> = runs
            .iter()
            .filter(|run| !run.status.is_terminal() || !run.acknowledged)
            .collect();
        if pending.is_empty() {
            continue;
        }
        let turns = state.sessions.list_turns(session.id)?;
        if turns
            .iter()
            .any(|turn| turn.status == session_store::RuntimeTurnStatus::Queued)
        {
            state.tasks.schedule_session(session.id)?;
            continue;
        }
        if session.active_turn_id.is_some() {
            continue;
        }
        let latest = turns
            .iter()
            .map(|turn| turn.queue_sequence)
            .max()
            .unwrap_or(0);
        let identities: Vec<_> = pending
            .iter()
            .map(|run| (run.id, run.status, run.acknowledged))
            .collect();
        let identity = uuid::Uuid::new_v5(
            &session.id,
            format!(
                "workflow-recovery:{latest}:{}",
                serde_json::to_string(&identities)?
            )
            .as_bytes(),
        );
        wake(state, session.id, identity)?;
        queued += 1;
    }
    Ok(queued)
}

#[cfg(test)]
mod tests;
