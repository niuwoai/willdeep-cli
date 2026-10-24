use super::*;

#[tokio::test]
async fn readonly_bridges_never_create_runtime_home() {
    let home = std::env::temp_dir().join(format!("willdeep-query-{}", uuid::Uuid::new_v4()));
    let id = uuid::Uuid::new_v4();
    macro_rules! unavailable {
        ($query:expr) => {
            assert!(
                $query.await.is_err(),
                "{} should require an existing Runtime",
                stringify!($query)
            );
            assert!(!home.exists(), "query created the Runtime home");
        };
    }
    unavailable!(tui_bridge::remote_session_states(&home));
    unavailable!(tui_bridge::remote_rewind_points(&home, id));
    unavailable!(tui_bridge::export_remote_session(&home, id));
    unavailable!(tui_bridge::search_remote_session_results(&home, &[]));
    unavailable!(tui_bridge::remote_turn_session(&home, id));
    unavailable!(tui_bridge::remote_active_turn(&home, id));
    unavailable!(tui_bridge::remote_latest_turn(&home, id));
    unavailable!(tui_bridge::remote_agent_detail(&home, id));
    unavailable!(tui_bridge::remote_task_diagnostics(&home, id));
    unavailable!(workspace_store::remote_workspaces(&home));
    unavailable!(diff_review::remote_snapshot(&home, &home));
    unavailable!(diff_review::remote_reviews(&home, &home, "snapshot"));
    unavailable!(diff_review::remote_verifications(&home, &home, "snapshot"));
    unavailable!(diff_review::remote_attributions(&home, &home, "snapshot"));
    unavailable!(schedule_client::read_session_attachments(
        &home,
        id,
        "attachment"
    ));
    unavailable!(peer_client::peer_operation(
        &home,
        "session.peers",
        serde_json::json!({}),
        None
    ));
    unavailable!(peer_client::peer_operation(
        &home,
        "session.peer.inbox",
        serde_json::json!({}),
        None
    ));
}

#[tokio::test]
async fn public_read_operations_require_existing_runtime() {
    let home = std::env::temp_dir().join(format!("willdeep-api-query-{}", uuid::Uuid::new_v4()));
    let params = serde_json::json!({});
    for operation in willdeep_runtime_protocol::SUPPORTED_OPERATIONS {
        if control_api::is_mutating_operation(operation)
            || matches!(
                *operation,
                "mobile.enable" | "mobile.disable" | "webapp.start" | "webapp.stop"
            )
        {
            continue;
        }
        assert!(
            state_for_operation(&home, operation, &params)
                .await
                .is_err()
        );
        assert!(!home.exists(), "{operation} started a Runtime");
    }
    let preview = serde_json::json!({"preview": true});
    assert!(
        state_for_operation(&home, "schedule.cleanup", &preview)
            .await
            .is_err()
    );
    assert!(!home.exists());
}
