use super::*;
use willdeep_runtime_protocol::{PendingApproval, PendingQuestion, RuntimeSession};

const NOW: u64 = 100_000;

fn digest(id: Uuid, workspace: &str) -> SessionDigest {
    SessionDigest {
        id,
        started_by: None,
        parent_session_id: None,
        delegation_read_only: false,
        delegation_detached: false,
        title: "修复登录".to_owned(),
        workspace: workspace.into(),
        created_at: 1,
        updated_at: NOW,
        pinned_at: None,
        has_user_input: true,
        message_count: 2,
        preview: None,
        bridged: false,
    }
}

fn session(id: Uuid) -> RuntimeSession {
    serde_json::from_value(serde_json::json!({
        "id": id, "root_agent_id": Uuid::new_v4(), "workspace": "/project",
        "profile": null, "model": null, "status": "idle", "active_turn_id": null,
        "created_at": 1, "updated_at": NOW,
    }))
    .unwrap()
}

fn task(session_id: Option<Uuid>, status: TaskStatus) -> RuntimeTask {
    RuntimeTask {
        id: Uuid::new_v4(),
        session_id,
        turn_id: Some(Uuid::new_v4()),
        agent_id: None,
        origin_client: Some("tui:other-client".to_owned()),
        event_start_sequence: 0,
        workspace: Some("/project".to_owned()),
        profile: None,
        prompt_excerpt: None,
        status,
        created_at: NOW - 10,
        started_at: Some(NOW - 10),
        completed_at: Some(NOW),
        exit_code: None,
        failure_domain: None,
    }
}

fn turn(task: &RuntimeTask, status: TurnStatus) -> RuntimeTurn {
    serde_json::from_value(serde_json::json!({
        "id": task.turn_id, "session_id": task.session_id, "request_id": Uuid::new_v4(),
        "queue_sequence": 1, "status": status, "active_task_id": task.id, "attempts": 1,
        "created_at": task.created_at, "started_at": task.started_at, "completed_at": NOW,
    }))
    .unwrap()
}

fn snapshot(tasks: Vec<RuntimeTask>) -> RemoteAttentionSnapshot {
    let ids = tasks
        .iter()
        .filter_map(|task| task.session_id)
        .collect::<HashSet<_>>();
    RemoteAttentionSnapshot {
        sessions: ids.into_iter().map(session).collect(),
        tasks,
        approvals: Vec::new(),
        questions: Vec::new(),
    }
}

fn project_visible(digests: &[SessionDigest]) -> HashMap<Uuid, &SessionDigest> {
    visible_sessions(digests, &["/project".into()], Path::new("/project"))
}

#[test]
fn deduplicates_multiple_tasks_and_gates_into_each_session() {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let digests = [digest(first, "/project"), digest(second, "/project")];
    let approval_task = task(Some(first), TaskStatus::WaitingApproval);
    let mut data = snapshot(vec![
        approval_task.clone(),
        task(Some(first), TaskStatus::WaitingAnswer),
        task(Some(second), TaskStatus::WaitingAnswer),
    ]);
    data.sessions
        .iter_mut()
        .find(|item| item.id == first)
        .unwrap()
        .active_turn_id = approval_task.turn_id;
    data.approvals.push(PendingApproval {
        id: Uuid::new_v4(),
        task_id: approval_task.id,
        parked: true,
        description: "private command is not projected".to_owned(),
        always_allow_available: false,
        created_at: NOW,
    });
    data.questions.push(PendingQuestion {
        id: Uuid::new_v4(),
        task_id: approval_task.id,
        parked: false,
        question: "private question is not projected".to_owned(),
        options: vec![],
        multi_select: false,
        created_at: NOW,
    });
    let result = aggregate_attention(&data, &project_visible(&digests), &HashMap::new(), NOW);
    assert_eq!(
        result.len(),
        2,
        "the count is the distinct navigable Session list length"
    );
    let first = result.iter().find(|item| item.session_id == first).unwrap();
    assert_eq!(first.title.as_deref(), Some("修复登录"));
    assert_eq!(first.active_turn_id, approval_task.turn_id);
    assert_eq!(
        first.reasons.len(),
        2,
        "task state and its gate must not duplicate a reason"
    );
    assert_eq!(first.reasons[0].status, AttentionStatus::WaitingApproval);
    let json = serde_json::to_string(&result).unwrap();
    assert!(!json.contains("private"));
    assert!(
        !json.contains("tui:other-client"),
        "origin does not limit navigation"
    );
}

#[test]
fn running_queued_completed_cancelled_and_unowned_tasks_are_not_pending_sessions() {
    let id = Uuid::new_v4();
    let digests = [digest(id, "/project")];
    let tasks = [
        TaskStatus::Running,
        TaskStatus::Queued,
        TaskStatus::Cancelling,
        TaskStatus::Completed,
        TaskStatus::Cancelled,
    ]
    .into_iter()
    .map(|status| task(Some(id), status))
    .chain([task(None, TaskStatus::Failed)])
    .collect();
    assert!(
        aggregate_attention(
            &snapshot(tasks),
            &project_visible(&digests),
            &HashMap::new(),
            NOW
        )
        .is_empty()
    );
}

#[test]
fn failures_expire_and_keep_localizable_failure_context() {
    for status in [
        TaskStatus::Failed,
        TaskStatus::Interrupted,
        TaskStatus::Partial,
    ] {
        let id = Uuid::new_v4();
        let digests = [digest(id, "/project")];
        let mut failed = task(Some(id), status);
        failed.exit_code = Some(23);
        failed.failure_domain = Some(willdeep_runtime_protocol::FailureDomain::Provider);
        let result = aggregate_attention(
            &snapshot(vec![failed.clone()]),
            &project_visible(&digests),
            &HashMap::new(),
            NOW,
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].reasons[0].failure_domain, Some("provider"));
        assert_eq!(result[0].reasons[0].exit_code, Some(23));
        failed.completed_at = Some(NOW - 24 * 60 * 60 - 1);
        assert!(
            aggregate_attention(
                &snapshot(vec![failed]),
                &project_visible(&digests),
                &HashMap::new(),
                NOW
            )
            .is_empty()
        );
    }
}

#[test]
fn later_tasks_and_same_second_queued_turns_supersede_old_failures() {
    let id = Uuid::new_v4();
    let digests = [digest(id, "/project")];
    let failed = task(Some(id), TaskStatus::Failed);
    let mut newer = task(Some(id), TaskStatus::Running);
    newer.created_at = failed.created_at + 1;
    assert!(
        aggregate_attention(
            &snapshot(vec![failed.clone(), newer]),
            &project_visible(&digests),
            &HashMap::new(),
            NOW
        )
        .is_empty()
    );

    let mut queued = turn(&failed, TurnStatus::Queued);
    queued.id = Uuid::new_v4();
    queued.queue_sequence = 2;
    queued.active_task_id = None;
    assert_eq!(queued.created_at, failed.created_at);
    let latest = HashMap::from([(id, Some(queued))]);
    assert!(
        aggregate_attention(
            &snapshot(vec![failed.clone()]),
            &project_visible(&digests),
            &latest,
            NOW
        )
        .is_empty()
    );

    let mut retry = turn(&failed, TurnStatus::Running);
    retry.active_task_id = Some(Uuid::new_v4());
    let latest = HashMap::from([(id, Some(retry))]);
    assert!(
        aggregate_attention(
            &snapshot(vec![failed]),
            &project_visible(&digests),
            &latest,
            NOW
        )
        .is_empty()
    );
}

#[test]
fn old_waiting_gates_remain_actionable_when_an_independent_turn_is_newer() {
    let id = Uuid::new_v4();
    let digests = [digest(id, "/project")];
    let waiting = task(Some(id), TaskStatus::WaitingAnswer);
    let mut newer = task(Some(id), TaskStatus::Completed);
    newer.created_at += 1;
    let latest = HashMap::from([(id, Some(turn(&newer, TurnStatus::Completed)))]);
    let result = aggregate_attention(
        &snapshot(vec![waiting, newer]),
        &project_visible(&digests),
        &latest,
        NOW,
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].reasons[0].status, AttentionStatus::WaitingAnswer);
}

#[test]
fn navigation_respects_project_allowlist_parent_worktree_and_archival() {
    let parent = digest(Uuid::new_v4(), "/project");
    let mut child = digest(Uuid::new_v4(), "/runtime/worktrees/child");
    child.parent_session_id = Some(parent.id);
    let outsider = digest(Uuid::new_v4(), "/private");
    let mut foreign_child = digest(Uuid::new_v4(), "/runtime/worktrees/foreign");
    foreign_child.parent_session_id = Some(outsider.id);
    let archived = digest(Uuid::new_v4(), "/project");
    let other_project = digest(Uuid::new_v4(), "/other-project");
    let digests = [
        parent.clone(),
        child.clone(),
        outsider,
        foreign_child,
        archived.clone(),
        other_project,
    ];
    let visible = visible_sessions(
        &digests,
        &["/project".into(), "/other-project".into()],
        Path::new("/project"),
    );
    assert_eq!(visible.len(), 3);
    assert!(visible.contains_key(&child.id));
    let mut data = snapshot(
        digests
            .iter()
            .map(|item| task(Some(item.id), TaskStatus::Failed))
            .collect(),
    );
    data.sessions
        .iter_mut()
        .find(|item| item.id == archived.id)
        .unwrap()
        .status = SessionStatus::Archived;
    let result = aggregate_attention(&data, &visible, &HashMap::new(), NOW);
    assert_eq!(
        result
            .iter()
            .map(|item| item.session_id)
            .collect::<HashSet<_>>(),
        HashSet::from([parent.id, child.id])
    );
    assert!(visible_sessions(&digests, &[], Path::new("/project")).is_empty());
}

#[test]
fn safe_titles_use_preview_and_redact_before_truncation() {
    let mut item = digest(Uuid::new_v4(), "/project");
    item.title = "New session".to_owned();
    assert_eq!(safe_title(&item), None);
    item.preview = Some("  检查问题\n  然后修复  ".to_owned());
    assert_eq!(safe_title(&item).as_deref(), Some("检查问题 然后修复"));
    let secret = format!("sk-{}", "x".repeat(48));
    item.title = format!("{} {secret}", "字".repeat(150));
    let title = safe_title(&item).unwrap();
    assert!(!title.contains("sk-"));
    assert!(title.chars().count() <= ATTENTION_TITLE_CHARS);
}

#[test]
fn complete_list_has_no_preview_limit() {
    let digests = (0..8)
        .map(|_| digest(Uuid::new_v4(), "/project"))
        .collect::<Vec<_>>();
    let data = snapshot(
        digests
            .iter()
            .map(|item| task(Some(item.id), TaskStatus::WaitingAnswer))
            .collect(),
    );
    assert_eq!(
        aggregate_attention(&data, &project_visible(&digests), &HashMap::new(), NOW).len(),
        8
    );
}
