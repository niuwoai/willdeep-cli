//! Additional protocol regressions, separated from the original fixture tests.

use super::*;
use crate::tools::SharedApprovalMode;

#[tokio::test]
async fn rejected_file_change_errors_are_json_with_empty_touch_sets() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let registry = fixture.registry();
    for changes in [
        vec![],
        vec![
            replace("a.txt", "old", "one"),
            replace("./a.txt", "old", "two"),
        ],
        vec![
            json!({"path":"a.txt","operation":"replace","content":"new","expected_version":"invalid"}),
        ],
    ] {
        let report =
            failure(request(&registry, json!({"operation":"preview","changes":changes})).await);
        assert_eq!(report["status"], "rejected");
        assert_eq!(report["files_touched"], json!([]));
        assert!(
            report["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty())
        );
    }
    let unknown = failure(
        request(
            &registry,
            json!({"operation":"apply","preview_id":uuid::Uuid::new_v4().to_string()}),
        )
        .await,
    );
    assert_eq!(unknown["files_touched"], json!([]));
    assert_eq!(fixture.text("a.txt"), "old");
    assert_eq!(fixture.entries(), vec!["a.txt"]);
}

#[tokio::test]
async fn approval_downgrade_rejects_edits_and_entire_batches_before_staging() {
    for batch in [false, true] {
        let fixture = Fixture::new();
        fixture.write("a.txt", "old");
        let approver = Arc::new(RecordingApprover::new(true, None));
        let registry = ToolRegistry::new(&fixture.0, ApprovalMode::Strict)
            .unwrap()
            .with_approver(approver.clone());
        let mode = registry.approval_mode_handle();
        *approver.action.lock().unwrap() = Some(Box::new(move || {
            mode.set(ApprovalMode::ReadOnly);
        }));
        let result = if batch {
            let plan = preview(
                &registry,
                vec![replace("a.txt", "old", "new"), create("b.txt", "new-b")],
            )
            .await;
            apply(&registry, &plan).await
        } else {
            registry
                .edit_file_versioned(edit("old", "new", Some(version(b"old"))))
                .await
        };
        assert!(matches!(result, Err(ToolError::ReadOnlyPolicy(_))));
        assert_eq!(approver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.text("a.txt"), "old");
        assert_eq!(fixture.entries(), vec!["a.txt"]);
    }
}

#[tokio::test]
async fn strict_mode_switched_while_waiting_for_coordinator_requires_approval() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let approver = Arc::new(RecordingApprover::new(true, None));
    let registry = fixture.registry().with_approver(approver.clone());
    let bound = registry
        .prepare_changes(vec![ChangeInput {
            path: "a.txt".into(),
            operation: Operation::Replace,
            content: "new".into(),
            expected_version: Some(version(b"old")),
        }])
        .unwrap();
    let lock = write_lock().await;
    let waiting = registry.approve_and_lock(&bound, "file_changes");
    tokio::pin!(waiting);
    // Poll exactly through the automatic approval and into the contended lock.
    let pending = std::future::poll_fn(|cx| {
        std::task::Poll::Ready(std::future::Future::poll(waiting.as_mut(), cx).is_pending())
    })
    .await;
    assert!(pending);
    assert_eq!(approver.calls.load(Ordering::SeqCst), 0);
    registry.approval_mode_handle().set(ApprovalMode::Strict);
    drop(lock);
    let _lock = waiting.await.unwrap();
    assert_eq!(approver.calls.load(Ordering::SeqCst), 1);
    let output: Value = serde_json::from_str(&registry.commit_changes(&bound).unwrap()).unwrap();
    assert_eq!(output["files_touched"], json!(["a.txt"]));
    assert_eq!(fixture.text("a.txt"), "new");
}

#[tokio::test]
async fn delegation_read_only_remains_a_ceiling_under_full_access() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let registry = fixture
        .registry()
        .with_parent_approval_mode(SharedApprovalMode::new(ApprovalMode::FullAccess))
        .with_delegation_read_only();
    registry
        .approval_mode_handle()
        .set(ApprovalMode::FullAccess);
    assert!(matches!(
        request(
            &registry,
            json!({"operation":"preview","changes":[create("b.txt","new")]})
        )
        .await,
        Err(ToolError::ReadOnlyPolicy(_))
    ));
    assert!(matches!(
        registry
            .edit_file_versioned(edit("old", "new", Some(version(b"old"))))
            .await,
        Err(ToolError::ReadOnlyPolicy(_))
    ));
    assert_eq!(fixture.text("a.txt"), "old");
    assert_eq!(fixture.entries(), vec!["a.txt"]);
}

#[tokio::test]
async fn approval_wait_releases_coordinator_for_other_registries_and_create_file() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let approver = Arc::new(WaitingApprover {
        entered: tokio::sync::Notify::new(),
    });
    let registry = Arc::new(
        ToolRegistry::new(&fixture.0, ApprovalMode::Strict)
            .unwrap()
            .with_approver(approver.clone()),
    );
    let plan = preview(
        &registry,
        vec![replace("a.txt", "old", "new"), create("b.txt", "ours")],
    )
    .await;
    let running_registry = registry.clone();
    let running_plan = plan.clone();
    let task = tokio::spawn(async move { apply(&running_registry, &running_plan).await });
    approver.entered.notified().await;
    let other = fixture.registry();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        other.create_file(CreateArgs {
            path: "b.txt".into(),
            content: "other-registry".into(),
        }),
    )
    .await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    result
        .expect("approval must not monopolize the coordinator")
        .unwrap();
    assert_eq!(fixture.text("a.txt"), "old");
    assert_eq!(fixture.text("b.txt"), "other-registry");
    assert_eq!(fixture.entries(), vec!["a.txt", "b.txt"]);
    assert!(apply(&registry, &plan).await.is_err());
}

#[tokio::test]
async fn cancellation_waiting_for_coordinator_consumes_preview_without_staging() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let registry = fixture.registry();
    let plan = preview(
        &registry,
        vec![replace("a.txt", "old", "new"), create("b.txt", "new-b")],
    )
    .await;
    let lock = write_lock().await;
    {
        let waiting = apply(&registry, &plan);
        tokio::pin!(waiting);
        let pending = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(std::future::Future::poll(waiting.as_mut(), cx).is_pending())
        })
        .await;
        assert!(pending);
        assert!(
            !registry
                .file_changes
                .inner
                .lock()
                .unwrap()
                .previews
                .contains_key(plan["preview_id"].as_str().unwrap())
        );
    }
    drop(lock);
    assert_eq!(
        failure(apply(&registry, &plan).await)["files_touched"],
        json!([])
    );
    assert_eq!(fixture.text("a.txt"), "old");
    assert_eq!(fixture.entries(), vec!["a.txt"]);
}

#[cfg(unix)]
#[tokio::test]
async fn unix_permissions_are_checked_before_approval_and_bound_to_preview() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let path = fixture.0.join("a.txt");
    let approver = Arc::new(RecordingApprover::new(true, None));
    let registry = ToolRegistry::new(&fixture.0, ApprovalMode::Strict)
        .unwrap()
        .with_approver(approver.clone());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    assert!(
        registry
            .edit_file_versioned(edit("old", "new", Some(version(b"old"))))
            .await
            .is_err()
    );
    assert!(request(&registry, json!({"operation":"preview","changes":[create("b.txt","new-b"),replace("a.txt","old","new")]})).await.is_err());
    assert_eq!(approver.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.entries(), vec!["a.txt"]);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
    let plan = preview(
        &registry,
        vec![replace("a.txt", "old", "new"), create("b.txt", "new-b")],
    )
    .await;
    let changed_path = path.clone();
    *approver.action.lock().unwrap() = Some(Box::new(move || {
        std::fs::set_permissions(changed_path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }));
    assert_eq!(
        failure(apply(&registry, &plan).await)["files_touched"],
        json!([])
    );
    assert_eq!(fixture.text("a.txt"), "old");
    assert_eq!(fixture.entries(), vec!["a.txt"]);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    registry
        .edit_file_versioned(edit("old", "new", Some(version(b"old"))))
        .await
        .unwrap();
    assert_eq!(fixture.text("a.txt"), "new");
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn staging_create_never_clobbers_a_destination_created_after_preflight() {
    let fixture = Fixture::new();
    let path = fixture.0.join("a.txt");
    let parent = disk::DirectoryIdentity::capture(&path).unwrap();
    let mut staged = disk::StagedFile::new(&path, &parent, b"ours", None).unwrap();
    assert!(!path.exists());
    assert!(staged.temporary_path().exists());
    fixture.write("a.txt", "external");
    assert!(staged.install(true).is_err());
    staged.cleanup().unwrap();
    assert_eq!(fixture.text("a.txt"), "external");
    assert_eq!(fixture.entries(), vec!["a.txt"]);
}

#[test]
fn cleanup_failure_reports_a_temporary_path_without_claiming_a_target_write() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let bound = registry
        .prepare_changes(vec![ChangeInput {
            path: "a.txt".into(),
            operation: Operation::Create,
            content: "ours".into(),
            expected_version: None,
        }])
        .unwrap();
    let change = &bound.changes[0];
    let staged = disk::StagedFile::new(&change.path, &change.parent, &change.after, None).unwrap();
    let temporary = staged.temporary_path();
    std::fs::remove_file(&temporary).unwrap();
    std::fs::create_dir(&temporary).unwrap();
    let mut files = [staged];
    let mut report = CommitReport::new(&bound);
    cleanup(&mut files, &mut report);
    assert!(report.files_touched.is_empty());
    assert!(report.recovery_failed.is_empty());
    assert_eq!(report.cleanup_failed.len(), 1);
    assert_eq!(
        report.cleanup_failed[0].path,
        temporary.display().to_string()
    );
    assert!(!change.path.exists());
    // The test owns this replacement directory; remove it so the explicit retry
    // can also verify NotFound cleanup without leaving an artificial orphan.
    std::fs::remove_dir(&temporary).unwrap();
    files[0].cleanup().unwrap();
    assert!(fixture.entries().is_empty());
}
