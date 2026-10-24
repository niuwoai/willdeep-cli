use super::*;

#[tokio::test]
async fn editing_preserves_attached_context_and_claimed_prompt() {
    let h = Harness::new();
    let session = h.session().await;
    let prefix = format!(
        "{}\n\ntrusted host context\n\n{}\n\n",
        willdeep_runtime_protocol::ATTACHED_CONTEXT_HEADER,
        willdeep_runtime_protocol::USER_MESSAGE_BOUNDARY
    );
    h.execute(
        "turn.submit",
        json!({"session_id":session,"turn_request_id":uuid::Uuid::new_v4(),
        "prompt":format!("{prefix}用户原文"),"attachments":[]}),
    )
    .await;
    let pending = h.state.sessions.queued_messages(session).unwrap();
    assert_eq!(pending[0].text, "用户原文");
    let mut gateway = h.gateway();
    let replies = phone(&mut gateway,json!({"id":uuid::Uuid::new_v4(),"type":"queue.update","session_id":session,
        "payload":{"action":"edit","message_id":pending[0].id,"text":"新内容","expected_text":"用户原文"}})).await;
    one(&replies, "ack");
    assert_eq!(
        h.state
            .sessions
            .claim_next(session)
            .unwrap()
            .unwrap()
            .request
            .prompt,
        format!("{prefix}新内容")
    );
}

#[tokio::test]
async fn queue_mutation_respects_drain_and_excludes_internal_wakes() {
    let h = Harness::new();
    let session = h.session().await;
    let (internal, _) = h
        .state
        .sessions
        .enqueue_turn(
            session,
            session_store::CreateRuntimeTurn {
                team_control: None,
                request_id: uuid::Uuid::new_v4(),
                prompt: "internal wake".into(),
                attachments: vec![],
                origin_client: Some("peer-wake:fixture".into()),
            },
        )
        .unwrap();
    h.execute("turn.submit",json!({"session_id":session,"turn_request_id":uuid::Uuid::new_v4(),"prompt":"user","attachments":[]})).await;
    assert_eq!(h.state.sessions.queued_messages(session).unwrap().len(), 1);
    let mut gateway = h.gateway();
    *h.state.work_gate.write().await = true;
    one(
        &phone(
            &mut gateway,
            json!({"id":uuid::Uuid::new_v4(),"type":"queue.update","session_id":session,
        "payload":{"action":"clear"}}),
        )
        .await,
        "error",
    );
    assert_eq!(h.state.sessions.queued_messages(session).unwrap().len(), 1);
    *h.state.work_gate.write().await = false;
    one(
        &phone(
            &mut gateway,
            json!({"id":uuid::Uuid::new_v4(),"type":"queue.update","session_id":session,
        "payload":{"action":"clear"}}),
        )
        .await,
        "ack",
    );
    assert_eq!(
        h.state
            .sessions
            .get_turn(internal.id)
            .unwrap()
            .unwrap()
            .status,
        session_store::RuntimeTurnStatus::Queued
    );
    assert!(
        h.state
            .sessions
            .queued_messages(session)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn explicit_send_now_precedes_hidden_internal_wakeups() {
    let h = Harness::new();
    let session = h.session().await;
    h.state
        .sessions
        .enqueue_turn(
            session,
            session_store::CreateRuntimeTurn {
                team_control: None,
                request_id: uuid::Uuid::new_v4(),
                prompt: "internal".into(),
                attachments: vec![],
                origin_client: Some("peer-wake:fixture".into()),
            },
        )
        .unwrap();
    h.execute("turn.submit",json!({"session_id":session,"turn_request_id":uuid::Uuid::new_v4(),"prompt":"urgent user","attachments":[]})).await;
    let id = h.state.sessions.queued_messages(session).unwrap()[0].id;
    let mut gateway = h.gateway();
    one(
        &phone(
            &mut gateway,
            json!({"id":uuid::Uuid::new_v4(),"type":"queue.update","session_id":session,
        "payload":{"action":"send_now","message_id":id}}),
        )
        .await,
        "ack",
    );
    assert_eq!(
        h.state
            .sessions
            .claim_next(session)
            .unwrap()
            .unwrap()
            .request
            .prompt,
        "urgent user"
    );
}

#[tokio::test]
async fn racing_claim_and_remove_cannot_remove_a_started_turn() {
    let h = Harness::new();
    let session = h.session().await;
    h.execute("turn.submit",json!({"session_id":session,"turn_request_id":uuid::Uuid::new_v4(),"prompt":"user","attachments":[]})).await;
    let id = h.state.sessions.queued_messages(session).unwrap()[0].id;
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let store = h.state.sessions.clone();
    let start = barrier.clone();
    let claim = std::thread::spawn(move || {
        start.wait();
        store.claim_next(session).unwrap()
    });
    barrier.wait();
    let removed = h
        .state
        .sessions
        .update_turn_queue(&willdeep_runtime_protocol::UpdateTurnQueueParams {
            session_id: session,
            action: willdeep_runtime_protocol::QueueAction::Remove,
            message_id: Some(id),
            text: None,
            expected_text: None,
        })
        .is_ok();
    let claimed = claim.join().unwrap().is_some();
    assert_ne!(removed, claimed, "exactly one of remove or claim may win");
}
