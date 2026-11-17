//! The semantic-edit entry point retains all existing preview gates.
use super::*;

fn replacement(path: &str, before: &str, after: &str) -> VersionedReplacement {
    VersionedReplacement {
        path: path.into(),
        expected_version: version(before.as_bytes()),
        content: after.into(),
    }
}

fn assert_no_preview(registry: &ToolRegistry) {
    assert!(
        registry
            .file_changes
            .inner
            .lock()
            .unwrap()
            .previews
            .is_empty()
    );
}

#[tokio::test]
async fn typed_preview_requires_exact_pre_request_versions_and_never_uses_observed_baselines() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "external");
    fixture.write("b.txt", "old-b");
    let registry = fixture.registry();
    registry
        .file_changes
        .observe(&fixture.0.join("a.txt"), b"external");
    assert!(matches!(
        registry
            .preview_versioned_replacements(vec![
                replacement("b.txt", "old-b", "new-b"),
                replacement("a.txt", "old-a", "new-a"),
            ])
            .await,
        Err(ToolError::EditConflict { .. })
    ));
    assert_no_preview(&registry);
    assert_eq!(fixture.text("a.txt"), "external");
    assert_eq!(fixture.text("b.txt"), "old-b");
    assert_eq!(fixture.entries(), vec!["a.txt", "b.txt"]);
}

#[tokio::test]
async fn typed_preview_rechecks_worker_write_sets_and_read_only_ceilings() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old-a");
    fixture.write("b.txt", "old-b");
    let restricted = fixture
        .registry()
        .with_write_targets(Some(BTreeSet::from([fixture.0.join("a.txt")])));
    assert!(
        restricted
            .preview_versioned_replacements(vec![
                replacement("a.txt", "old-a", "new-a"),
                replacement("b.txt", "old-b", "new-b"),
            ])
            .await
            .is_err()
    );
    assert_no_preview(&restricted);
    for registry in [
        ToolRegistry::new(&fixture.0, ApprovalMode::ReadOnly).unwrap(),
        fixture.registry().with_delegation_read_only(),
    ] {
        assert!(matches!(
            registry
                .preview_versioned_replacements(vec![replacement("a.txt", "old-a", "new-a"),])
                .await,
            Err(ToolError::ReadOnlyPolicy(_))
        ));
        assert_no_preview(&registry);
    }
    assert_eq!(fixture.text("a.txt"), "old-a");
    assert_eq!(fixture.text("b.txt"), "old-b");
}

#[tokio::test]
async fn typed_preview_waits_for_write_lock_then_rechecks_policy_and_versions() {
    for change_policy in [false, true] {
        let fixture = Fixture::new();
        fixture.write("a.txt", "old");
        let registry = fixture.registry();
        let lock = write_lock().await;
        let waiting =
            registry.preview_versioned_replacements(vec![replacement("a.txt", "old", "new")]);
        tokio::pin!(waiting);
        let pending = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(std::future::Future::poll(waiting.as_mut(), cx).is_pending())
        })
        .await;
        assert!(pending);
        if change_policy {
            registry.approval_mode_handle().set(ApprovalMode::ReadOnly);
        } else {
            fixture.write("a.txt", "external");
        }
        drop(lock);
        let result = waiting.await;
        if change_policy {
            assert!(matches!(result, Err(ToolError::ReadOnlyPolicy(_))));
        } else {
            assert!(matches!(result, Err(ToolError::EditConflict { .. })));
        }
        assert_no_preview(&registry);
        assert_eq!(
            fixture.text("a.txt"),
            if change_policy { "old" } else { "external" }
        );
    }
}

#[tokio::test]
async fn typed_preview_rejects_duplicate_paths_and_the_same_32_file_4_mib_budgets() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let registry = fixture.registry();
    assert!(
        registry
            .preview_versioned_replacements(vec![
                replacement("a.txt", "old", "new"),
                replacement("./a.txt", "old", "other"),
            ])
            .await
            .is_err()
    );
    assert_no_preview(&registry);
    assert!(
        registry
            .preview_versioned_replacements(
                (0..=MAX_FILES)
                    .map(|_| replacement("a.txt", "old", "new"))
                    .collect()
            )
            .await
            .is_err()
    );
    assert_no_preview(&registry);
    assert!(
        registry
            .preview_versioned_replacements(vec![replacement(
                "a.txt",
                "old",
                &"x".repeat(MAX_TOTAL_BYTES)
            ),])
            .await
            .is_err()
    );
    assert_no_preview(&registry);
    assert_eq!(fixture.text("a.txt"), "old");
}

#[tokio::test]
async fn typed_preview_lease_revokes_failed_postchecks_without_exhausting_cache() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let registry = fixture.registry();
    for _ in 0..=MAX_PREVIEWS {
        let result = async {
            let _lease = registry
                .preview_versioned_replacements(vec![replacement("a.txt", "old", "new")])
                .await?;
            tokio::task::yield_now().await;
            Err::<String, ToolError>(change_error(
                "simulated asynchronous configuration check failure",
            ))
        }
        .await;
        assert!(result.is_err());
        assert_no_preview(&registry);
    }
    // Every slot is still available after more failed attempts than the limit.
    for _ in 0..MAX_PREVIEWS {
        registry
            .preview_versioned_replacements(vec![replacement("a.txt", "old", "new")])
            .await
            .unwrap()
            .publish();
    }
    assert!(
        registry
            .preview_versioned_replacements(vec![replacement("a.txt", "old", "new"),])
            .await
            .is_err(),
        "published previews still enforce the ordinary cache limit"
    );
    assert_eq!(fixture.text("a.txt"), "old");
    assert_eq!(fixture.entries(), vec!["a.txt"]);
}

#[tokio::test]
async fn typed_preview_lease_revokes_on_cancellation_and_deadline_during_postcheck() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let registry = fixture.registry();
    for deadline in [false, true] {
        for _ in 0..=MAX_PREVIEWS {
            let (ready, received) = tokio::sync::oneshot::channel();
            let mut running = Box::pin(async {
                let _lease = registry
                    .preview_versioned_replacements(vec![replacement("a.txt", "old", "new")])
                    .await?;
                ready.send(()).unwrap();
                std::future::pending::<()>().await;
                Ok::<(), ToolError>(())
            });
            tokio::select! {
                result = &mut running => panic!("preview completed before postcheck: {result:?}"),
                result = tokio::time::timeout(Duration::from_secs(5), received) => result.unwrap().unwrap(),
            }
            assert_eq!(
                registry.file_changes.inner.lock().unwrap().previews.len(),
                1
            );
            if deadline {
                assert!(
                    tokio::time::timeout(Duration::from_millis(1), running)
                        .await
                        .is_err()
                );
            } else {
                drop(running);
            }
            assert_no_preview(&registry);
        }
    }
    assert_eq!(fixture.text("a.txt"), "old");
    assert_eq!(fixture.entries(), vec!["a.txt"]);
}

#[tokio::test]
async fn typed_preview_lease_revokes_only_its_own_id_and_publish_keeps_one_shot_apply() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let registry = fixture.registry();
    let published: Value = serde_json::from_str(
        &registry
            .preview_versioned_replacements(vec![replacement("a.txt", "old", "published")])
            .await
            .unwrap()
            .publish(),
    )
    .unwrap();
    let abandoned = registry
        .preview_versioned_replacements(vec![replacement("a.txt", "old", "abandoned")])
        .await
        .unwrap();
    assert_eq!(
        registry.file_changes.inner.lock().unwrap().previews.len(),
        2
    );
    drop(abandoned);
    assert_eq!(
        registry.file_changes.inner.lock().unwrap().previews.len(),
        1
    );
    let receipt: Value =
        serde_json::from_str(&apply(&registry, &published).await.unwrap()).unwrap();
    assert_eq!(receipt["status"], "applied");
    assert_eq!(fixture.text("a.txt"), "published");
    assert_no_preview(&registry);
    assert!(apply(&registry, &published).await.is_err());
}

#[tokio::test]
async fn typed_preview_uses_existing_one_shot_apply_approval_and_stale_preflight() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let approver = Arc::new(RecordingApprover::new(false, None));
    let registry = ToolRegistry::new(&fixture.0, ApprovalMode::Strict)
        .unwrap()
        .with_approver(approver.clone());
    let plan: Value = serde_json::from_str(
        &registry
            .preview_versioned_replacements(vec![replacement("a.txt", "old", "new")])
            .await
            .unwrap()
            .publish(),
    )
    .unwrap();
    assert_eq!(approver.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.text("a.txt"), "old");
    assert!(matches!(
        apply(&registry, &plan).await,
        Err(ToolError::ApprovalDenied(_))
    ));
    assert_eq!(approver.calls.load(Ordering::SeqCst), 1);
    assert!(apply(&registry, &plan).await.is_err());
    let plan: Value = serde_json::from_str(
        &registry
            .preview_versioned_replacements(vec![replacement("a.txt", "old", "new")])
            .await
            .unwrap()
            .publish(),
    )
    .unwrap();
    fixture.write("a.txt", "external");
    assert_eq!(
        failure(apply(&registry, &plan).await)["files_touched"],
        json!([])
    );
    assert_eq!(
        approver.calls.load(Ordering::SeqCst),
        1,
        "stale batches fail before approval"
    );
    assert_eq!(fixture.text("a.txt"), "external");
}
