use super::*;
use willdeep_core::checkpoint::{
    CheckpointMetadata, CheckpointSink, RunCheckpoint, SessionCheckpointSink,
};
use willdeep_core::{Message, MessageAttachment, Role, SessionStore};

struct Fixture {
    root: PathBuf,
    runtime: RuntimeSessionStore,
    core: SessionStore,
    session: uuid::Uuid,
}

impl Fixture {
    fn new(history: Vec<Message>) -> Self {
        let root = std::env::temp_dir().join(format!("request-recovery-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let runtime = RuntimeSessionStore::open(root.join("runtime-sessions.json"), &root).unwrap();
        let session = runtime
            .create(CreateRuntimeSession {
                id: None,
                workspace,
                profile: None,
                model: None,
                config: None,
                title: None,
            })
            .unwrap()
            .id;
        let core = SessionStore::new(&root);
        let mut stored = core.load_baselined(session).unwrap();
        stored.messages = history;
        stored.goal_state = Some(willdeep_core::GoalState::new("finish the recovery fixture"));
        core.save(&mut stored).unwrap();
        Self {
            root,
            runtime,
            core,
            session,
        }
    }

    fn claim(
        &self,
        prompt: &str,
        attachments: Vec<MessageAttachment>,
    ) -> (RuntimeTurn, uuid::Uuid) {
        let (turn, _) = self
            .runtime
            .enqueue_turn(
                self.session,
                CreateRuntimeTurn {
                    team_control: None,
                    request_id: uuid::Uuid::new_v4(),
                    prompt: prompt.into(),
                    attachments,
                    origin_client: None,
                    defer_until_released: false,
                },
            )
            .unwrap();
        self.runtime.claim_next(self.session).unwrap().unwrap();
        let task = uuid::Uuid::new_v4();
        assert!(self.runtime.bind_task(turn.id, task).unwrap());
        let _ownership = SessionCheckpointSink {
            store: self.core.clone(),
            session_id: self.session,
        }
        .claim()
        .unwrap();
        let core = self.core.load_baselined(self.session).unwrap();
        self.runtime.prepare_execution(task, &core).unwrap();
        (self.runtime.get_turn(turn.id).unwrap().unwrap(), task)
    }

    fn raw(
        &self,
        turn: &RuntimeTurn,
        prompt: &str,
        attachments: Vec<MessageAttachment>,
    ) -> Message {
        let mut message = Message::user_with_attachments(prompt, attachments);
        message.runtime_turn_id = Some(turn.id);
        let mut core = self.core.load_baselined(self.session).unwrap();
        core.messages.push(message.clone());
        self.core.save(&mut core).unwrap();
        message
    }

    fn checkpoint(&self, messages: Vec<Message>) {
        let sink = SessionCheckpointSink {
            store: self.core.clone(),
            session_id: self.session,
        }
        .claim()
        .unwrap();
        sink.save(&RunCheckpoint {
            messages,
            metadata: CheckpointMetadata::default(),
            plan: None,
            goal: None,
            context_summary: None,
        })
        .unwrap();
    }

    fn restart(&mut self) {
        self.runtime =
            RuntimeSessionStore::open(self.root.join("runtime-sessions.json"), &self.root).unwrap();
    }

    fn candidate(&self) -> GoalResumeCandidate {
        let candidates = self.runtime.goal_resume_candidates("goal-resume:").unwrap();
        assert_eq!(candidates.len(), 1);
        candidates.into_iter().next().unwrap()
    }

    fn assert_recorded(&self) {
        let candidate = self.candidate();
        assert!(candidate.unrecorded_request.is_none());
        assert!(candidate.unrecorded_attachments.is_empty());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn image() -> MessageAttachment {
    MessageAttachment::Image {
        name: "figure.png".into(),
        media_type: "image/png".into(),
        data: "AQID".into(),
        width: 1,
        height: 1,
    }
}

fn steering() -> String {
    willdeep_core::routing::RouteDecision {
        tier: willdeep_core::routing::RoutingTier::Worker,
        profile: Some("reader"),
        confidence: 94,
        auto_dispatch_read_only: false,
        reason: "fixture",
        classifier_model: None,
        classifier_failures: Vec::new(),
    }
    .steering()
    .unwrap()
}

#[test]
fn system_pin_and_route_rewrite_do_not_replay_the_durable_request() {
    for legacy in [false, true] {
        let mut fixture = Fixture::new(Vec::new());
        let (turn, _) = fixture.claim("inspect parser", Vec::new());
        assert_eq!(turn.message_start, Some(0));
        let mut user = fixture.raw(&turn, "inspect parser", Vec::new());
        if legacy {
            user.runtime_turn_id = None;
        }
        user.content.push_str(&format!("\n\n{}", steering()));
        fixture.checkpoint(vec![Message::system("pin"), user]);
        assert_eq!(
            fixture
                .core
                .load(fixture.session)
                .unwrap()
                .compression_generation,
            0
        );
        fixture.restart();
        fixture.assert_recorded();
    }
}

#[test]
fn durable_identity_survives_recovery_user_rows_and_arbitrary_request_rewriting() {
    let mut fixture = Fixture::new(vec![Message::user("previous request")]);
    let (turn, _) = fixture.claim("inspect parser", Vec::new());
    let mut user = fixture.raw(&turn, "inspect parser", Vec::new());
    // The real Harness inserts this as another role=user row before the input.
    let notice = CheckpointMetadata::default().recovery_notice().unwrap();
    user.content = "rewritten request with preflight evidence".into();
    fixture.checkpoint(vec![
        Message::system("pin"),
        Message::user("previous request"),
        notice,
        user,
    ]);
    fixture.restart();
    fixture.assert_recorded();
}

#[test]
fn image_description_and_worker_report_never_resend_the_original_image() {
    for legacy in [false, true] {
        let mut fixture = Fixture::new(Vec::new());
        let text = MessageAttachment::Text {
            name: "context.txt".into(),
            content: "context".into(),
        };
        let (turn, _) = fixture.claim("inspect image", vec![image(), text.clone()]);
        let mut user = fixture.raw(&turn, "inspect image", vec![image(), text]);
        if legacy {
            user.runtime_turn_id = None;
        }
        user.content.push_str(&format!(
            "{}vision for 1 attached image(s)]\na square\n\n{}",
            willdeep_core::types::IMAGE_DESCRIPTION_MARKER,
            steering()
        ));
        user.attachments
            .retain(|value| !matches!(value, MessageAttachment::Image { .. }));
        fixture.checkpoint(vec![Message::system("pin"), user]);
        fixture.restart();
        fixture.assert_recorded();
    }
}

#[test]
fn repeated_prompt_in_a_different_turn_cannot_hide_an_unrecorded_image_request() {
    let mut fixture = Fixture::new(Vec::new());
    let (first, first_task) = fixture.claim("same prompt", vec![image()]);
    fixture.raw(&first, "same prompt", vec![image()]);
    fixture
        .runtime
        .complete_task(first_task, RuntimeTaskStatus::Completed, None)
        .unwrap();
    let (second, _) = fixture.claim("same prompt", vec![image()]);
    assert_ne!(first.id, second.id);
    assert_eq!(second.user_message_ordinal, Some(1));
    fixture.restart();
    let candidate = fixture.candidate();
    assert_eq!(candidate.unrecorded_request.as_deref(), Some("same prompt"));
    assert_eq!(candidate.unrecorded_attachments, vec![image()]);

    let mut core = fixture.core.load_baselined(fixture.session).unwrap();
    // Even a misplaced equal request at the expected ordinal belongs to first.
    core.messages.push(core.messages[0].clone());
    fixture.core.save(&mut core).unwrap();
    assert_eq!(
        fixture.candidate().unrecorded_request.as_deref(),
        Some("same prompt")
    );
}

#[test]
fn compression_preserves_user_order_and_identity_with_duplicate_prompts() {
    for legacy in [false, true] {
        let mut fixture = Fixture::new(vec![
            Message::user("same prompt"),
            Message::assistant("old reply", Vec::new()),
        ]);
        let (turn, _) = fixture.claim("same prompt", Vec::new());
        let mut user = fixture.raw(&turn, "same prompt", Vec::new());
        if legacy {
            user.runtime_turn_id = None;
        }
        fixture.checkpoint(vec![
            Message::system("pin"),
            Message::user("same prompt"),
            Message::assistant("old reply", Vec::new()),
            user,
        ]);
        let mut core = fixture.core.load_baselined(fixture.session).unwrap();
        let compact = core
            .messages
            .iter()
            .filter(|message| message.role == Role::User)
            .cloned()
            .collect();
        assert!(core.replace_with_compressed_messages(compact));
        fixture.core.save(&mut core).unwrap();
        fixture.restart();
        fixture.assert_recorded();
        core.messages.pop();
        fixture.core.save(&mut core).unwrap();
        assert_eq!(
            fixture.candidate().unrecorded_request.as_deref(),
            Some("same prompt")
        );
    }
}

#[test]
fn legacy_records_without_identity_or_ordinal_keep_the_original_boundary() {
    let mut fixture = Fixture::new(Vec::new());
    let (turn, _) = fixture.claim("legacy request", Vec::new());
    fixture.checkpoint(vec![
        Message::system("pin"),
        Message::user("legacy request"),
    ]);
    // Exercise actual serde defaults in both historical JSON record formats.
    let bytes = std::fs::read(&fixture.runtime.turns_path).unwrap();
    let mut turns: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    turns[0]["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("user_message_ordinal");
    let old: Vec<StoredRuntimeTurn> = serde_json::from_value(serde_json::json!(turns)).unwrap();
    assert_eq!(old[0].metadata.user_message_ordinal, None);
    assert_eq!(old[0].metadata.id, turn.id);
    let legacy_user: Message = serde_json::from_value(serde_json::json!({
        "role": "user", "content": "legacy request", "tool_call_id": null,
    }))
    .unwrap();
    assert_eq!(legacy_user.runtime_turn_id, None);
    let turns = old
        .into_iter()
        .map(|turn| (turn.metadata.id, turn))
        .collect();
    persist_turns(&fixture.runtime.turns_path, &turns).unwrap();
    fixture.restart();
    fixture.assert_recorded();
    let mut core = fixture.core.load_baselined(fixture.session).unwrap();
    core.compression_generation += 1;
    fixture.core.save(&mut core).unwrap();
    assert_eq!(
        fixture.candidate().unrecorded_request.as_deref(),
        Some("legacy request")
    );
}

#[test]
fn legacy_matching_rejects_arbitrary_prompt_suffixes_and_malformed_routes() {
    for suffix in [
        " and do something else",
        "\n\n<runtime-route tier=\"worker\" profile=\"reader\" confidence=\"94\">\nuser-written text</runtime-route>",
    ] {
        let mut fixture = Fixture::new(Vec::new());
        fixture.claim("original request", Vec::new());
        fixture.checkpoint(vec![
            Message::system("pin"),
            Message::user(format!("original request{suffix}")),
        ]);
        fixture.restart();
        assert_eq!(
            fixture.candidate().unrecorded_request.as_deref(),
            Some("original request")
        );
    }
}

#[test]
fn legacy_worker_report_requires_the_complete_generated_wrapper() {
    let suffix = "\n\n<runtime-route tier=\"worker\" profile=\"reader\" confidence=\"94\">\nThe runtime dispatched this bounded read-only preflight before the standard model. Use the report as evidence, verify citations when needed, and answer without re-reading the same material unless it is incomplete.\n<worker-report>\nreport evidence\n</worker-report>\n</runtime-route>";
    let mut fixture = Fixture::new(Vec::new());
    fixture.claim("inspect parser", Vec::new());
    fixture.checkpoint(vec![
        Message::system("pin"),
        Message::user(format!("inspect parser{suffix}")),
    ]);
    fixture.restart();
    fixture.assert_recorded();
}
