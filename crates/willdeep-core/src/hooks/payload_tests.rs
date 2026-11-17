use super::*;

fn assert_bounded(payload: &HookPayload) -> HookPayload {
    let body = serialize_payload(payload);
    assert!(
        body.len() <= MAX_PAYLOAD_BYTES,
        "frame is {} bytes",
        body.len()
    );
    let parsed: HookPayload = serde_json::from_slice(body.as_bytes()).expect("complete UTF-8 JSON");
    assert_eq!(parsed.extra[TRUNCATED_KEY], "true");
    parsed
}

fn oversized_text() -> String {
    // Multi-byte code points and JSON escapes have different encoded costs.
    "审计🦀\"\\\n\t\u{0000}".repeat(MAX_PAYLOAD_BYTES)
}

#[test]
fn ordinary_payloads_keep_their_exact_serialization() {
    let mut payload = HookPayload::new(HookEvent::ApprovalResolved)
        .with_tool("run_command", "cargo test")
        .with_session(Some("session".to_owned()), Some("workspace".to_owned()))
        .with_outcome("allow");
    payload
        .extra
        .insert("source".to_owned(), "judge".to_owned());
    assert_eq!(
        serialize_payload(&payload),
        serde_json::to_string(&payload).unwrap()
    );
}

#[test]
fn every_public_string_field_and_extra_keys_are_bounded() {
    let huge = oversized_text();
    for field in 0..9 {
        let mut payload = HookPayload::new(HookEvent::ApprovalResolved);
        match field {
            0 => payload.event = huge.clone(),
            1 => payload.session_id = Some(huge.clone()),
            2 => payload.workspace = Some(huge.clone()),
            3 => payload.tool = Some(huge.clone()),
            4 => payload.detail = Some(huge.clone()),
            5 => payload.outcome = Some(huge.clone()),
            6 => {
                payload.extra.insert(huge.clone(), "value".to_owned());
            }
            7 => {
                payload.extra.insert("reason".to_owned(), huge.clone());
            }
            _ => {
                for index in 0..MAX_PAYLOAD_BYTES {
                    payload
                        .extra
                        .insert(format!("field-{index}"), "value".to_owned());
                }
            }
        }
        assert_bounded(&payload);
    }
}

#[test]
fn combined_oversized_fields_account_for_escaping_and_structure() {
    let huge = oversized_text();
    let payload = HookPayload {
        event: huge.clone(),
        session_id: Some(huge.clone()),
        workspace: Some(huge.clone()),
        tool: Some(huge.clone()),
        detail: Some(huge.clone()),
        outcome: Some(huge.clone()),
        exit_code: Some(i32::MIN),
        extra: BTreeMap::from([
            ("reason\"\n".to_owned(), huge.clone()),
            (
                huge,
                "a huge key must not displace the truncation marker".to_owned(),
            ),
            (TRUNCATED_KEY.to_owned(), "untrusted old value".to_owned()),
        ]),
    };
    let parsed = assert_bounded(&payload);
    assert_eq!(parsed.exit_code, Some(i32::MIN));
    assert!(!parsed.extra.contains_key(&payload.event));
    for (original, bounded) in [
        (&payload.event, &parsed.event),
        (
            payload.session_id.as_ref().unwrap(),
            parsed.session_id.as_ref().unwrap(),
        ),
        (
            payload.workspace.as_ref().unwrap(),
            parsed.workspace.as_ref().unwrap(),
        ),
        (
            payload.tool.as_ref().unwrap(),
            parsed.tool.as_ref().unwrap(),
        ),
        (
            payload.detail.as_ref().unwrap(),
            parsed.detail.as_ref().unwrap(),
        ),
        (
            payload.outcome.as_ref().unwrap(),
            parsed.outcome.as_ref().unwrap(),
        ),
    ] {
        assert!(original.starts_with(bounded.as_str()));
        assert!(!bounded.is_empty());
    }
}

#[test]
fn string_budgets_match_serde_json_for_all_control_escapes() {
    let text = (0_u8..=31).map(char::from).collect::<String>() + "\"\\审计🦀";
    for available in 0..256 {
        for field_limit in [0, 1, 7, 32, 255] {
            let mut remaining = available;
            let bounded = budget_string(&text, &mut remaining, field_limit);
            let encoded = serde_json::to_string(&bounded).unwrap().len() - 2;
            assert!(encoded <= available.min(field_limit));
            assert_eq!(remaining + encoded, available);
            assert!(text.starts_with(bounded.as_str()));
        }
    }
}

#[tokio::test]
async fn a_long_multifile_diff_reaches_real_stdin_as_parseable_json() {
    let root = std::env::temp_dir().join(format!("willdeep-hook-frame-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("stdin.json");
    let path_text = path.display().to_string();
    let command = if cfg!(windows) {
        format!(
            "[IO.File]::WriteAllText('{}', [Console]::In.ReadToEnd())",
            path_text.replace('\'', "''")
        )
    } else {
        format!("cat > '{}'", path_text.replace('\'', "'\\''"))
    };
    let hook = Hook {
        name: "capture oversized stdin".to_owned(),
        event: HookEvent::ApprovalResolved,
        command,
        blocking: true,
        timeout: Duration::from_secs(5),
        on_error: HookFailure::Deny,
    };
    let diff = "diff --git a/审计.rs b/审计.rs\n+let text = \"新内容\";\n".repeat(2048);
    assert!(diff.len() >= 64 * 1024);
    let payload = HookPayload::new(HookEvent::ApprovalResolved)
        .with_tool("apply diff", &diff)
        .with_session(
            Some("session-fixture".to_owned()),
            Some("workspace-fixture".to_owned()),
        )
        .with_outcome("allow_once");
    assert_eq!(
        HookRegistry::new(vec![hook])
            .fire(HookEvent::ApprovalResolved, &payload)
            .await,
        HookOutcome::Allow
    );
    let bytes = std::fs::read(&path).expect("hook must capture actual stdin");
    assert!(bytes.len() <= MAX_PAYLOAD_BYTES);
    let parsed: HookPayload = serde_json::from_slice(&bytes).expect("stdin must be complete JSON");
    assert_eq!(parsed.event, "approval_resolved");
    assert_eq!(parsed.session_id.as_deref(), Some("session-fixture"));
    assert_eq!(parsed.workspace.as_deref(), Some("workspace-fixture"));
    assert_eq!(parsed.tool.as_deref(), Some("apply diff"));
    assert_eq!(parsed.outcome.as_deref(), Some("allow_once"));
    assert_eq!(parsed.extra[TRUNCATED_KEY], "true");
    assert!(
        payload
            .detail
            .as_deref()
            .unwrap()
            .starts_with(parsed.detail.as_deref().unwrap())
    );
    assert!(!parsed.detail.as_deref().unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn event_presence_is_specific_to_the_registered_event() {
    let registry = HookRegistry::new(vec![Hook {
        name: "pre gate".to_owned(),
        event: HookEvent::PreTool,
        command: "exit 0".to_owned(),
        blocking: true,
        timeout: Duration::from_secs(1),
        on_error: HookFailure::Deny,
    }]);
    assert!(registry.has_event(HookEvent::PreTool));
    assert!(!registry.has_event(HookEvent::ApprovalResolved));
    assert!(!HookRegistry::default().has_event(HookEvent::ApprovalResolved));
}
