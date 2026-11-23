use super::*;

#[tokio::test]
async fn previous_plan_is_context_only_and_is_not_erased_by_a_query() {
    let fixture = Fixture::new();
    let previous: crate::conversation::Plan = serde_json::from_value(serde_json::json!({
        "summary":"old release",
        "steps":[{"id":"old-check", "text":"run the old project suite", "status":"pending"}]
    }))
    .unwrap();
    fixture
        .sink
        .store
        .update(fixture.sink.session_id, |session| {
            session.current_plan = Some(previous.clone());
        })
        .unwrap();
    let provider = Arc::new(SyntheticProvider::default());
    let agent = fixture.agent(provider.clone(), fixture.tools());
    let ownership = fixture.new_input();
    let sink = ownership.for_new_input().unwrap();
    let outcome = agent
        .run_checkpointed(
            Vec::new(),
            Message::user("answer this independent question"),
            Some(&sink),
        )
        .await
        .unwrap();
    assert_answer_only(&outcome, 0);
    let requests = provider.requests.lock().unwrap();
    assert!(requests[0].iter().any(|message| {
        message.content.contains("<historical-session-plan>")
            && message.content.contains("old-check")
    }));
    assert!(
        requests[0]
            .iter()
            .all(|message| !message.content.contains("<session-plan>"))
    );
    assert!(requests[0].iter().any(|message| {
        message.content.contains("[current-task-boundary]")
            && message.content.contains("wait for explicit agreement")
    }));
    assert_eq!(fixture.saved().current_plan, Some(previous));
}

#[cfg(unix)]
struct ChangeDuringApproval(std::path::PathBuf);

#[cfg(unix)]
#[async_trait]
impl crate::Approver for ChangeDuringApproval {
    async fn approve(&self, _: &str, _: bool) -> crate::ApprovalDecision {
        std::fs::write(&self.0, "another writer").unwrap();
        crate::ApprovalDecision::AllowOnce
    }
}

#[tokio::test]
async fn host_continuation_keeps_unattributed_drift_from_requesting_tests() {
    let fixture = Fixture::new();
    fixture
        .sink
        .save(&RunCheckpoint {
            plan: None,
            metadata: CheckpointMetadata {
                status: CheckpointStatus::Partial,
                // The original run observed this snapshot; another writer changed
                // the file after the run paused and before the host continuation.
                verification_baseline: Some("before-drift".into()),
                completion_scope_uncertain: true,
                ..Default::default()
            },
            messages: vec![Message::user("earlier independent query")],
            goal: None,
            context_summary: None,
        })
        .unwrap();
    let provider = Arc::new(SyntheticProvider::default());
    let outcome = fixture
        .agent(provider.clone(), fixture.tools())
        .run_checkpointed(
            fixture.saved().messages,
            Message::host_instruction("background result"),
            Some(&fixture.sink),
        )
        .await
        .unwrap();
    assert_eq!(outcome.stop_reason, AgentStopReason::Unverified);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert!(outcome.messages.iter().any(|message| {
        message
            .content
            .contains("[completion-verification-scope-unknown]")
    }));
    assert!(outcome.messages.iter().all(|message| {
        !message
            .content
            .contains("[completion-verification-required]")
    }));
    assert!(fixture.saved().current_plan.is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn opaque_window_drift_stops_without_starting_tests() {
    let fixture = Fixture::new();
    let provider = Arc::new(SyntheticProvider::default());
    provider.tools.lock().unwrap().push_back(vec![ToolCall {
        id: "query".into(),
        name: "run_command".into(),
        arguments: serde_json::json!({"command":"printf query"}).to_string(),
    }]);
    let tools = fixture.tools().with_approver(Arc::new(ChangeDuringApproval(
        fixture.saved().workspace.join("result.txt"),
    )));
    // Strict mode guarantees a deterministic approval window for this query.
    tools.approval_mode_handle().set(ApprovalMode::Strict);
    let ownership = fixture.new_input();
    let sink = ownership.for_new_input().unwrap();
    let outcome = fixture
        .agent(provider.clone(), tools)
        .run_checkpointed(
            Vec::new(),
            Message::user("return the query result"),
            Some(&sink),
        )
        .await
        .unwrap();
    assert_eq!(outcome.stop_reason, AgentStopReason::Unverified);
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    assert!(outcome.messages.iter().any(|message| {
        message
            .content
            .contains("[completion-verification-scope-unknown]")
    }));
    assert!(outcome.messages.iter().all(|message| {
        !message
            .content
            .contains("[completion-verification-required]")
    }));
}
