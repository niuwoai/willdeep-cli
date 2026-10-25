use super::*;

fn fixture() -> (tempfile_stub::Directory, ScreenplayWorkflow) {
    let root = tempfile_stub::Directory(
        std::env::temp_dir().join(format!("screenplay-unit-{}", Uuid::new_v4())),
    );
    let id = Uuid::new_v4();
    let workflow = ScreenplayWorkflow {
        id,
        request_id: id,
        input: ScreenplayStart {
            brief: "乡村创业".into(),
            workspace: root.0.clone(),
            max_calls: 32,
            max_tokens: 300_000,
        },
        status: "running".into(),
        created_at: 1,
        updated_at: 1,
        tasks: vec![],
        calls: vec![],
        domain: None,
        error: None,
        cancel_requested: false,
        budget_changes: Vec::new(),
    };
    let store = Store::new(&root.0);
    let _lock = store.lock(id, false).unwrap();
    store.put(&workflow).unwrap();
    (root, workflow)
}

mod tempfile_stub {
    pub(super) struct Directory(pub std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.0) {
                eprintln!("screenplay fixture cleanup: {error}");
            }
        }
    }
}

#[test]
fn interrupted_task_retains_its_identity_and_grant() {
    let (root, workflow) = fixture();
    let first = prepare_task(&root.0, workflow.id).unwrap();
    let recovered = prepare_task(&root.0, workflow.id).unwrap();
    assert_eq!(first.id, recovered.id);
    assert_eq!(first.arguments, recovered.arguments);
    assert_eq!(Store::new(&root.0).get(workflow.id).unwrap().tasks.len(), 1);
}

#[test]
fn domain_next_step_uses_saved_content_basis_and_new_task_identity() {
    let (root, workflow) = fixture();
    let first = prepare_task(&root.0, workflow.id).unwrap();
    Store::new(&root.0)
        .update(workflow.id, |value| {
            value.tasks[0].state = "completed".into();
            value.domain =
                Some(json!({"basis":"v1","next":{"step":"review_episode","episodeOrder":2}}));
            Ok(())
        })
        .unwrap();
    let next = prepare_task(&root.0, workflow.id).unwrap();
    assert_ne!(first.id, next.id);
    assert_eq!(next.arguments["expectedBasis"], "v1");
    assert_eq!(next.arguments["episodeOrder"], 2);
}

#[test]
fn cancellation_blocks_subsequent_dispatch() {
    let (root, workflow) = fixture();
    Store::new(&root.0)
        .update(workflow.id, |value| {
            value.cancel_requested = true;
            Ok(())
        })
        .unwrap();
    assert!(prepare_task(&root.0, workflow.id).is_err());
}

#[test]
fn only_one_execution_lease_can_exist() {
    let (root, workflow) = fixture();
    let lease = Store::new(&root.0).lock(workflow.id, true).unwrap();
    assert!(Store::new(&root.0).lock(workflow.id, true).is_err());
    drop(lease);
    assert!(Store::new(&root.0).lock(workflow.id, true).is_ok());
}

#[test]
fn public_projection_does_not_expose_task_grants() {
    let (root, workflow) = fixture();
    let task = prepare_task(&root.0, workflow.id).unwrap();
    let projected = public(Store::new(&root.0).get(workflow.id).unwrap()).unwrap();
    assert!(!projected.to_string().contains(&task.grant.to_string()));
}

#[test]
fn corrupt_archive_is_not_replaced_or_recovered_as_empty() {
    let (root, workflow) = fixture();
    std::fs::write(
        root.0
            .join("runtime/screenplays")
            .join(format!("{}.json", workflow.id)),
        b"{",
    )
    .unwrap();
    assert!(Store::new(&root.0).list().is_err());
    assert!(Store::new(&root.0).get(workflow.id).is_err());
}

#[tokio::test]
async fn forged_task_grant_is_rejected_without_model_dispatch() {
    use willdeep_core::plugin::{PluginHostRequests, PluginPermission, PluginRequestContext};
    let (root, workflow) = fixture();
    let task = prepare_task(&root.0, workflow.id).unwrap();
    let handler = host::Host {
        home: root.0.clone(),
        workflow_id: workflow.id,
        inner: crate::plugin_host_requests::HostRequests::new(
            root.0.clone(),
            root.0.join("config.toml"),
            Arc::new(std::sync::RwLock::new(vec![])),
        ),
    };
    let context = PluginRequestContext {
        plugin_id: PLUGIN.into(),
        permissions: vec![PluginPermission::AiChat],
    };
    let mut forged = task.arguments.clone();
    forged["grant"] = json!(Uuid::new_v4());
    assert!(
        handler
            .handle(&context, host::AUTHORIZE, forged)
            .await
            .is_err()
    );
    assert!(
        handler
            .handle(&context, host::AUTHORIZE, task.arguments)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn exhausted_budget_blocks_before_missing_provider_configuration() {
    use willdeep_core::plugin::{PluginHostRequests, PluginPermission, PluginRequestContext};
    let (root, workflow) = fixture();
    let task = prepare_task(&root.0, workflow.id).unwrap();
    Store::new(&root.0)
        .update(workflow.id, |value| {
            value.calls = (0..32)
                .map(|_| ScreenplayCall {
                    id: Uuid::new_v4(),
                    task_id: task.id,
                    state: "completed".into(),
                    reservation: 100,
                    tokens: Some(100),
                    model: None,
                    request_digest: String::new(),
                    response_digest: None,
                })
                .collect();
            Ok(())
        })
        .unwrap();
    let handler = host::Host {
        home: root.0.clone(),
        workflow_id: workflow.id,
        inner: crate::plugin_host_requests::HostRequests::new(
            root.0.clone(),
            root.0.join("config.toml"),
            Arc::new(std::sync::RwLock::new(vec![])),
        ),
    };
    let context = PluginRequestContext {
        plugin_id: PLUGIN.into(),
        permissions: vec![PluginPermission::AiChat],
    };
    let result = handler
        .handle(
            &context,
            willdeep_core::plugin::host_requests::AI_COMPLETE,
            json!({"system":"test"}),
        )
        .await;
    assert!(format!("{:?}", result.unwrap_err()).contains("budget_limited"));
    assert_eq!(
        Store::new(&root.0).get(workflow.id).unwrap().calls.len(),
        32
    );
}

#[tokio::test]
async fn saved_model_response_recovers_without_a_second_provider_call() {
    use sha2::{Digest, Sha256};
    use willdeep_core::plugin::{PluginHostRequests, PluginPermission, PluginRequestContext};
    let (root, workflow) = fixture();
    let task = prepare_task(&root.0, workflow.id).unwrap();
    let params = json!({"system":"test", "max_output_tokens":4000});
    let call_id = Uuid::new_v4();
    let response = json!({"text":"saved response", "model":"fixture", "usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}});
    Store::new(&root.0)
        .save_response(workflow.id, call_id, &response)
        .unwrap();
    Store::new(&root.0)
        .update(workflow.id, |value| {
            value.calls.push(ScreenplayCall {
                id: call_id,
                task_id: task.id,
                state: "started".into(),
                reservation: 5000,
                tokens: None,
                model: None,
                request_digest: format!("{:x}", Sha256::digest(serde_json::to_vec(&params)?)),
                response_digest: None,
            });
            Ok(())
        })
        .unwrap();
    Store::new(&root.0).reconcile(workflow.id).unwrap();
    let handler = host::Host {
        home: root.0.clone(),
        workflow_id: workflow.id,
        inner: crate::plugin_host_requests::HostRequests::new(
            root.0.clone(),
            root.0.join("missing-config.toml"),
            Arc::new(std::sync::RwLock::new(vec![])),
        ),
    };
    let context = PluginRequestContext {
        plugin_id: PLUGIN.into(),
        permissions: vec![PluginPermission::AiChat],
    };
    assert_eq!(
        handler
            .handle(
                &context,
                willdeep_core::plugin::host_requests::AI_COMPLETE,
                params
            )
            .await
            .unwrap(),
        response
    );
    let state = Store::new(&root.0).get(workflow.id).unwrap();
    assert_eq!(state.calls.len(), 1);
    assert_eq!(state.calls[0].tokens, Some(15));
    assert_eq!(state.calls[0].state, "completed");
}

#[tokio::test]
async fn retrying_a_budget_stop_preserves_the_same_limits_and_does_not_erase_calls() {
    let (root, workflow) = fixture();
    let task = prepare_task(&root.0, workflow.id).unwrap();
    Store::new(&root.0)
        .update(workflow.id, |value| {
            value.status = "budget_limited".into();
            value.calls.push(ScreenplayCall {
                id: Uuid::new_v4(),
                task_id: task.id,
                state: "completed".into(),
                reservation: 100,
                tokens: Some(80),
                model: None,
                request_digest: "original-request".into(),
                response_digest: None,
            });
            Ok(())
        })
        .unwrap();
    let result = operation(
        &root.0,
        Uuid::new_v4(),
        "screenplay.resume",
        json!({"id":workflow.id}),
    )
    .await
    .unwrap();
    assert_eq!(result["input"]["max_tokens"], 300_000);
    assert_eq!(result["input"]["max_calls"], 32);
    assert_eq!(result["tasks"][0]["id"], task.id.to_string());
    assert_eq!(result["budget_changes"], json!([]));
    assert_eq!(result["calls"][0]["tokens"], 80);
    assert_eq!(result["calls"][0]["request_digest"], "original-request");
}

#[tokio::test]
async fn explicit_budget_increase_preserves_usage_and_rejects_stale_updates() {
    let (root, workflow) = fixture();
    let task = prepare_task(&root.0, workflow.id).unwrap();
    Store::new(&root.0)
        .update(workflow.id, |value| {
            value.status = "budget_limited".into();
            value.calls.push(ScreenplayCall {
                id: Uuid::new_v4(),
                task_id: task.id,
                state: "completed".into(),
                reservation: 100,
                tokens: Some(80),
                model: None,
                request_digest: String::new(),
                response_digest: None,
            });
            Ok(())
        })
        .unwrap();
    let params = json!({"id":workflow.id,"expected_calls":32,"expected_tokens":300_000,"max_calls":64,"max_tokens":500_000});
    operation(&root.0, Uuid::new_v4(), "screenplay.budget", params.clone())
        .await
        .unwrap();
    let saved = Store::new(&root.0).get(workflow.id).unwrap();
    assert_eq!(saved.calls[0].tokens, Some(80));
    assert_eq!(saved.tasks[0].id, task.id);
    assert_eq!(saved.budget_changes.len(), 1);
    assert_eq!(saved.status, "waiting_user");
    assert!(
        operation(&root.0, Uuid::new_v4(), "screenplay.budget", params)
            .await
            .is_err()
    );
}
