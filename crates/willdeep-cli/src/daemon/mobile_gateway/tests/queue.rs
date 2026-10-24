use super::*;

async fn add(gateway: &mut Gateway, session: uuid::Uuid, text: &str) -> Value {
    let replies = phone(
        gateway,
        json!({"id": uuid::Uuid::new_v4().to_string(), "type":"queue.update",
        "session_id":session, "payload":{"action":"add","text":text}}),
    )
    .await;
    one(&replies, "ack");
    one(&replies, "state.snapshot")["payload"]["queued_messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["text"] == text)
        .unwrap()
        .clone()
}

async fn update(gateway: &mut Gateway, session: uuid::Uuid, payload: Value) -> Vec<Value> {
    phone(
        gateway,
        json!({"id":uuid::Uuid::new_v4().to_string(),"type":"queue.update",
        "session_id":session,"payload":payload}),
    )
    .await
}

#[tokio::test]
async fn queue_round_trip_edit_preserves_ids_attachments_and_survives_restart() {
    let h = Harness::new();
    let session = h.session().await;
    let mut gateway = h.gateway();
    let long_text = "中文\n日本語\nEnglish ".repeat(90).trim().to_owned();
    let replies = phone(&mut gateway, json!({"id":"attachment-add","type":"queue.update","session_id":session,
        "payload":{"action":"add","text":long_text,"images":[{"image_url":{"url":png_data_url(2,2)}}]}})).await;
    one(&replies, "ack");
    assert!(
        of_type(&replies, "message.append").is_empty(),
        "pending text must not appear as delivered"
    );
    let item = one(&replies, "state.snapshot")["payload"]["queued_messages"][0].clone();
    if let Ok(path) = std::env::var("WILLDEEP_QUEUE_RUST_FIXTURE") {
        std::fs::write(path, one(&replies, "state.snapshot").to_string()).unwrap();
    }
    assert_eq!(item["text"], long_text);
    assert_eq!(item["image_count"], 1);
    let id = item["id"].as_str().unwrap();
    let edited = update(
        &mut gateway,
        session,
        json!({"action":"edit","message_id":id,
        "expected_text":long_text,"text":"修改后的消息\nnext step"}),
    )
    .await;
    one(&edited, "ack");
    let pending = &one(&edited, "state.snapshot")["payload"]["queued_messages"][0];
    assert_eq!(pending["id"], id);
    assert_eq!(pending["image_count"], 1);
    assert_eq!(pending["text"], "修改后的消息\nnext step");
    let restarted = session_store::RuntimeSessionStore::open(
        h.state.home.join("runtime-sessions.json"),
        &h.state.home,
    )
    .unwrap();
    assert_eq!(
        restarted.queued_messages(session).unwrap()[0].text,
        "修改后的消息\nnext step"
    );
    let claimed = restarted.claim_next(session).unwrap().unwrap();
    assert_eq!(claimed.request.prompt, "修改后的消息\nnext step");
    assert_eq!(claimed.request.attachments.len(), 1);
}

#[tokio::test]
async fn queue_rejects_stale_edit_wrong_session_and_already_claimed_message() {
    let h = Harness::new();
    let session = h.session().await;
    let other = h.session().await;
    let mut gateway = h.gateway();
    let item = add(&mut gateway, session, "original").await;
    for (target, payload) in [
        (
            session,
            json!({"action":"edit","message_id":item["id"],"expected_text":"stale","text":"new"}),
        ),
        (other, json!({"action":"remove","message_id":item["id"]})),
        (
            session,
            json!({"action":"edit","message_id":item["id"],"text":"new"}),
        ),
        (
            session,
            json!({"action":"edit","message_id":item["id"],"expected_text":"original","text":"  "}),
        ),
    ] {
        one(&update(&mut gateway, target, payload).await, "error");
    }
    assert_eq!(
        h.state.sessions.queued_messages(session).unwrap()[0].text,
        "original"
    );
    h.state.sessions.claim_next(session).unwrap().unwrap();
    assert!(
        h.state
            .sessions
            .queued_messages(session)
            .unwrap()
            .is_empty()
    );
    one(
        &update(
            &mut gateway,
            session,
            json!({"action":"remove","message_id":item["id"]}),
        )
        .await,
        "error",
    );
}

#[tokio::test]
async fn queue_remove_clear_and_send_now_preserve_other_sessions_and_order() {
    let h = Harness::new();
    let session = h.session().await;
    let other = h.session().await;
    let mut gateway = h.gateway();
    add(&mut gateway, session, "active").await;
    let active = h.state.sessions.claim_next(session).unwrap().unwrap();
    let first = add(&mut gateway, session, "first").await;
    let second = add(&mut gateway, session, "second").await;
    let other_message = add(&mut gateway, other, "other").await;
    let urgent = update(
        &mut gateway,
        session,
        json!({"action":"send_now","message_id":second["id"]}),
    )
    .await;
    one(&urgent, "ack");
    let pending = h.state.sessions.queued_messages(session).unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>(),
        ["second", "first"]
    );
    assert_eq!(
        h.state
            .sessions
            .get_turn(active.metadata.id)
            .unwrap()
            .unwrap()
            .status,
        session_store::RuntimeTurnStatus::Cancelled
    );
    one(
        &update(
            &mut gateway,
            session,
            json!({"action":"remove","message_id":first["id"]}),
        )
        .await,
        "ack",
    );
    let claimed = h.state.sessions.claim_next(session).unwrap().unwrap();
    assert_eq!(claimed.request.prompt, "second");
    add(&mut gateway, session, "to clear").await;
    one(
        &update(&mut gateway, session, json!({"action":"clear"})).await,
        "ack",
    );
    assert!(
        h.state
            .sessions
            .queued_messages(session)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        h.state.sessions.queued_messages(other).unwrap()[0]
            .id
            .to_string(),
        other_message["id"].as_str().unwrap()
    );
    assert_eq!(
        h.state
            .sessions
            .get(session)
            .unwrap()
            .unwrap()
            .active_turn_id,
        Some(claimed.metadata.id)
    );
}

#[tokio::test]
async fn queue_write_failure_keeps_memory_and_request_retry_is_idempotent() {
    let h = Harness::new();
    let session = h.session().await;
    let mut gateway = h.gateway();
    let item = add(&mut gateway, session, "keep me").await;
    let path = h.state.home.join("turns.json");
    assert!(path.exists(), "{}", path.display());
    std::fs::rename(&path, path.with_extension("backup")).unwrap();
    std::fs::create_dir(&path).unwrap();
    let failed = update(
        &mut gateway,
        session,
        json!({"action":"remove","message_id":item["id"]}),
    )
    .await;
    one(&failed, "error");
    assert_eq!(h.state.sessions.queued_messages(session).unwrap().len(), 1);
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(path.with_extension("backup"), &path).unwrap();
    let command = json!({"id":"retry-remove","type":"queue.update","session_id":session,
        "payload":{"action":"remove","message_id":item["id"]}});
    one(&phone(&mut gateway, command.clone()).await, "ack");
    one(&phone(&mut gateway, command).await, "ack");
    assert!(
        h.state
            .sessions
            .queued_messages(session)
            .unwrap()
            .is_empty()
    );
}
