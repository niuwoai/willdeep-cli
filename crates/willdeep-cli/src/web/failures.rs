//! Safe failure summaries for live streams, reconnects and historical Sessions.
use super::*;
use willdeep_runtime_protocol::{RuntimeTaskDiagnostics, RuntimeTurn, TaskStatus, TurnStatus};

#[derive(Debug, Serialize)]
pub(super) struct WebRuntimeFailure {
    message: String,
    session_id: uuid::Uuid,
    turn_id: uuid::Uuid,
    task_id: Option<uuid::Uuid>,
    reason: Option<String>,
    failure_domain: Option<String>,
    /// Only this server-projected field may be used as explanatory UI text.
    failure_detail: Option<String>,
}

impl WebRuntimeFailure {
    pub(super) fn with_classification(
        mut self,
        reason: Option<&str>,
        domain: Option<&str>,
    ) -> Self {
        self.reason = reason.map(str::to_owned);
        self.failure_domain = self.failure_domain.or_else(|| domain.map(str::to_owned));
        self
    }

    pub(super) fn into_event(self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("failure summary is JSON serializable");
        value["type"] = serde_json::json!("error");
        value
    }

    fn apply_diagnostics(&mut self, diagnostics: &RuntimeTaskDiagnostics) {
        if Some(diagnostics.task.id) != self.task_id
            || diagnostics.task.session_id != Some(self.session_id)
            || diagnostics.task.turn_id != Some(self.turn_id)
            || !matches!(
                diagnostics.task.status,
                TaskStatus::Failed | TaskStatus::Interrupted
            )
        {
            return;
        }
        self.failure_domain = diagnostics
            .task
            .failure_domain
            .map(|domain| failure_domain_name(domain).to_owned())
            .or(self.failure_domain.take());
        self.failure_detail = diagnostics
            .failure
            .as_deref()
            .map(diagnostic_failure_text)
            .filter(|text| !text.is_empty());
    }
}

/// Call only after the enclosing Session endpoint has authorized its workspace.
/// Diagnostic fetches never enlarge the public listener's disclosure boundary.
pub(super) async fn terminal_failure(
    state: &WebState,
    message: &str,
    session_id: uuid::Uuid,
    turn_id: uuid::Uuid,
    task_id: Option<uuid::Uuid>,
) -> WebRuntimeFailure {
    let mut failure = WebRuntimeFailure {
        message: message.to_owned(),
        session_id,
        turn_id,
        task_id,
        reason: None,
        failure_domain: None,
        failure_detail: None,
    };
    if !state.settings_writable || message == "turn.cancelled" {
        return failure;
    }
    if let Some(task_id) = task_id {
        match crate::daemon::remote_task_diagnostics(&state.home, task_id).await {
            Ok(diagnostics) => failure.apply_diagnostics(&diagnostics),
            Err(error) => log_unavailable("task_diagnostics", session_id, Some(task_id), &error),
        }
    }
    failure
}

pub(super) async fn for_turn(state: &WebState, turn: &RuntimeTurn) -> Option<WebRuntimeFailure> {
    let message = match turn.status {
        TurnStatus::Failed => "turn.failed",
        TurnStatus::Interrupted => "turn.interrupted",
        TurnStatus::Cancelled => "turn.cancelled",
        _ => return None,
    };
    Some(
        terminal_failure(
            state,
            message,
            turn.session_id,
            turn.id,
            turn.active_task_id,
        )
        .await,
    )
}

pub(super) async fn latest(state: &WebState, session_id: uuid::Uuid) -> Option<WebRuntimeFailure> {
    match crate::daemon::remote_latest_turn(&state.home, session_id).await {
        Ok(Some(turn)) if turn.session_id == session_id => for_turn(state, &turn).await,
        Ok(_) => None,
        Err(error) => {
            log_unavailable("latest_turn", session_id, None, &error);
            None
        }
    }
}

fn log_unavailable(
    operation: &str,
    session_id: uuid::Uuid,
    task_id: Option<uuid::Uuid>,
    error: &anyhow::Error,
) {
    eprintln!(
        "{}",
        serde_json::json!({
            "level": "debug", "event": "web.failure_summary_unavailable",
            "operation": operation, "session_id": session_id, "task_id": task_id,
            "error": willdeep_core::provider::safe_error_detail(&format!("{error:#}")),
        })
    );
}

/// Bridge/setup failures have no Task yet; still carry safe explanatory text.
pub(super) fn bridge_error(message: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "error", "message": "runtime.failed",
        "failure_detail": willdeep_core::provider::safe_error_detail(message),
    })
}

pub(super) fn diagnostic_failure_text(message: &str) -> String {
    let detail = message
        .split_once(" error=")
        .map_or(message, |(_, detail)| detail);
    willdeep_core::provider::safe_error_detail(detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (WebRuntimeFailure, RuntimeTaskDiagnostics) {
        let session_id = uuid::Uuid::new_v4();
        let turn_id = uuid::Uuid::new_v4();
        let task_id = uuid::Uuid::new_v4();
        let failure = WebRuntimeFailure {
            message: "turn.failed".into(),
            session_id,
            turn_id,
            task_id: Some(task_id),
            reason: None,
            failure_domain: None,
            failure_detail: None,
        };
        let diagnostics = serde_json::from_value(json!({
            "task": {
                "id": task_id, "session_id": session_id, "turn_id": turn_id,
                "agent_id": null, "event_start_sequence": 1, "status": "failed",
                "workspace": null, "profile": null, "created_at": 1,
                "started_at": 1, "completed_at": 2, "exit_code": 1,
                "failure_domain": "provider"
            },
            "failure": "task_id=private error=provider rejected model: maximum context length is 8192 tokens",
            "failed_tools": [{"sequence": 2, "name": "run_command", "arguments": "private command", "output": "private output"}]
        })).unwrap();
        (failure, diagnostics)
    }

    #[test]
    fn terminal_summary_preserves_detail_and_identity_without_raw_diagnostics() {
        let (mut failure, diagnostics) = fixture();
        failure.apply_diagnostics(&diagnostics);
        let event = failure.into_event();
        assert_eq!(event["type"], "error");
        assert_eq!(event["task_id"], diagnostics.task.id.to_string());
        assert_eq!(event["failure_domain"], "provider");
        assert!(
            event["failure_detail"]
                .as_str()
                .unwrap()
                .contains("maximum context length is 8192")
        );
        assert!(!event.to_string().contains("private"));
        assert!(event.get("failed_tools").is_none());
    }

    #[test]
    fn diagnostic_projection_rejects_other_sessions_turns_tasks_and_nonfailures() {
        for mismatch in 0..4 {
            let (mut failure, mut diagnostics) = fixture();
            match mismatch {
                0 => diagnostics.task.id = uuid::Uuid::new_v4(),
                1 => diagnostics.task.session_id = Some(uuid::Uuid::new_v4()),
                2 => diagnostics.task.turn_id = Some(uuid::Uuid::new_v4()),
                _ => diagnostics.task.status = TaskStatus::Completed,
            }
            failure.apply_diagnostics(&diagnostics);
            assert!(failure.failure_detail.is_none());
            assert!(failure.failure_domain.is_none());
        }
    }

    #[test]
    fn bridge_errors_keep_context_but_do_not_expose_paths_or_internal_objects() {
        let event = bridge_error(
            "cannot capture verification snapshot at /Users/fixture/private: Git snapshot failed",
        );
        let text = event["failure_detail"].as_str().unwrap();
        assert!(text.contains("cannot capture verification snapshot"));
        assert!(text.contains("Git snapshot failed"));
        assert!(!text.contains("/Users/fixture"));
        let text = diagnostic_failure_text(
            "task_id=private error=HTTP 429: {\"error\":{\"message\":\"Rate limit exceeded\"},\"request\":\"private prompt\"}",
        );
        assert_eq!(text, "HTTP 429: Rate limit exceeded");
    }
}
