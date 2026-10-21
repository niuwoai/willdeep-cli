use super::*;

#[test]
fn peer_request_excerpt_omits_host_routing_and_keeps_plain_operator_text() {
    let content = "Inspect the diff\n\n<runtime-route tier=\"standard\" profile=\"implementer\" confidence=\"78\">\nThe runtime classified this as bounded `implementer` work. Compile a task packet, keep read_files separate from write_files, and delegate it before doing the implementation in the parent. Give a verifier whenever completion is command-decidable.\n</runtime-route>";
    assert_eq!(request_excerpt(content), "Inspect the diff");
    let quoted = "Explain <runtime-route> in this example";
    assert_eq!(request_excerpt(quoted), quoted);
    assert_eq!(request_excerpt("中文\n请求"), "中文 请求");
}

struct Fixture {
    root: PathBuf,
    state: ServerState,
    source: uuid::Uuid,
    target: uuid::Uuid,
    foreign: uuid::Uuid,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("peer-api-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        let foreign = root.join("foreign");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&foreign).unwrap();
        let sessions = crate::daemon::tests::test_runtime_session_store(&root);
        let create = |workspace| {
            sessions
                .create(session_store::CreateRuntimeSession {
                    id: None,
                    workspace,
                    profile: None,
                    model: None,
                    config: None,
                    title: None,
                })
                .unwrap()
                .id
        };
        let source = create(workspace.clone());
        let target = create(workspace);
        let foreign = create(foreign);
        let state = crate::daemon::tests::test_server_state(&root, sessions);
        Self {
            root,
            state,
            source,
            target,
            foreign,
        }
    }
    fn request(&self, to: uuid::Uuid) -> ApiRequest {
        ApiRequest::new(
            "session.peer.send",
            serde_json::json!({"source_session_id": self.source,
            "session_id": to, "message": "Please avoid duplicate edits."}),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn discovers_project_peers_and_queues_without_claiming_consumption() {
    let fixture = Fixture::new();
    let peers = list(
        &fixture.state,
        &ApiRequest::new("session.peers", serde_json::json!({"id": fixture.source})),
    )
    .map_err(|error| error.message)
    .unwrap();
    assert_eq!(peers.as_array().unwrap().len(), 2);
    let request = fixture.request(fixture.target);
    let first = send(&fixture.state, &request)
        .map_err(|error| error.message)
        .unwrap();
    let second = send(&fixture.state, &request)
        .map_err(|error| error.message)
        .unwrap();
    assert_eq!(first["id"], second["id"]);
    assert!(first["consumed_at"].is_null());
    assert_eq!(first["origin"], "model");
    let records = inbox(
        &fixture.state,
        &ApiRequest::new(
            "session.peer.inbox",
            serde_json::json!({"session_id": fixture.target}),
        ),
    )
    .map_err(|error| error.message)
    .unwrap();
    assert_eq!(records.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn refuses_self_foreign_and_archived_targets() {
    let fixture = Fixture::new();
    assert!(send(&fixture.state, &fixture.request(fixture.source)).is_err());
    assert!(send(&fixture.state, &fixture.request(fixture.foreign)).is_err());
    fixture.state.sessions.archive(fixture.target).unwrap();
    assert!(send(&fixture.state, &fixture.request(fixture.target)).is_err());
}

fn operator_turn(fixture: &Fixture, cancelled: bool) {
    let core_store = willdeep_core::SessionStore::new(&fixture.root);
    let mut core = core_store.load(fixture.target).unwrap();
    core.messages
        .push(willdeep_core::Message::user("continue authorized work"));
    core_store.save(&mut core).unwrap();
    let (turn, _) = fixture
        .state
        .sessions
        .enqueue_turn(
            fixture.target,
            session_store::CreateRuntimeTurn {
                team_control: None,
                request_id: uuid::Uuid::new_v4(),
                prompt: "continue authorized work".into(),
                attachments: Vec::new(),
                origin_client: Some("test-operator".into()),
            },
        )
        .unwrap();
    if cancelled {
        fixture.state.sessions.request_cancel(turn.id).unwrap();
    } else {
        fixture
            .state
            .sessions
            .claim_next(fixture.target)
            .unwrap()
            .unwrap();
        let task = uuid::Uuid::new_v4();
        fixture.state.sessions.bind_task(turn.id, task).unwrap();
        fixture
            .state
            .sessions
            .complete_task(task, RuntimeTaskStatus::Completed, None)
            .unwrap();
    }
}

#[tokio::test]
async fn idle_peer_wakes_once_for_a_durable_note() {
    let fixture = Fixture::new();
    operator_turn(&fixture, false);
    let request = fixture.request(fixture.target);
    send(&fixture.state, &request)
        .map_err(|error| error.message)
        .unwrap();
    send(&fixture.state, &request)
        .map_err(|error| error.message)
        .unwrap();
    let turns = fixture.state.sessions.list_turns(fixture.target).unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(
        turns.last().unwrap().status,
        session_store::RuntimeTurnStatus::Queued
    );
}

#[tokio::test]
async fn user_stop_keeps_notes_queued_until_new_operator_work_completes() {
    let fixture = Fixture::new();
    operator_turn(&fixture, true);
    send(&fixture.state, &fixture.request(fixture.target))
        .map_err(|error| error.message)
        .unwrap();
    recover(&fixture.state).unwrap();
    assert_eq!(
        fixture
            .state
            .sessions
            .list_turns(fixture.target)
            .unwrap()
            .len(),
        1
    );
    operator_turn(&fixture, false);
    // The test scheduler intentionally has no receiver; the turn must still be durable.
    let _ = recover(&fixture.state);
    assert_eq!(
        fixture
            .state
            .sessions
            .list_turns(fixture.target)
            .unwrap()
            .len(),
        3
    );
}
