//! Regressions found by the independent protocol review.
use super::*;

#[cfg(unix)]
#[tokio::test]
async fn approved_directory_alias_creates_commit_and_roll_back_with_the_same_write_grant() {
    for fail_second in [false, true] {
        let fixture = Fixture::new();
        std::fs::create_dir(fixture.0.join("real")).unwrap();
        std::os::unix::fs::symlink("real", fixture.0.join("alias")).unwrap();
        fixture.write("b.txt", "old-b");
        let registry = fixture.registry().with_write_targets(Some(BTreeSet::from([
            fixture.0.join("alias/new.txt"),
            fixture.0.join("b.txt"),
        ])));
        let plan = preview(
            &registry,
            vec![
                create("alias/new.txt", "new"),
                replace("b.txt", "old-b", "new-b"),
            ],
        )
        .await;
        if fail_second {
            *registry.file_changes.fault.lock().unwrap() = Some(TestFault {
                fail_before: Some(1),
                ..Default::default()
            });
            let report = failure(apply(&registry, &plan).await);
            assert_eq!(report["status"], "rolled_back");
            assert_eq!(report["rolled_back"], json!(["alias/new.txt"]));
            assert_eq!(report["files_touched"], json!([]));
            assert_eq!(report["recovery_failed"], json!([]));
            assert!(!fixture.0.join("real/new.txt").exists());
            assert_eq!(fixture.text("b.txt"), "old-b");
        } else {
            let report: Value =
                serde_json::from_str(&apply(&registry, &plan).await.unwrap()).unwrap();
            assert_eq!(report["status"], "applied");
            assert_eq!(report["files_touched"], json!(["real/new.txt", "b.txt"]));
            assert_eq!(fixture.text("real/new.txt"), "new");
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn retargeted_alias_receipt_names_the_bound_destination_that_retains_the_write() {
    let fixture = Fixture::new();
    fixture.write("real-a.txt", "old-a");
    fixture.write("real-b.txt", "untouched");
    fixture.write("next.txt", "old-next");
    std::os::unix::fs::symlink("real-a.txt", fixture.0.join("alias.txt")).unwrap();
    let registry = fixture.registry();
    let plan = preview(
        &registry,
        vec![
            replace("alias.txt", "old-a", "new-a"),
            replace("next.txt", "old-next", "new-next"),
        ],
    )
    .await;
    assert_eq!(plan["changes"][0]["target_path"], "real-a.txt");
    *registry.file_changes.fault.lock().unwrap() = Some(TestFault {
        fail_before: Some(1),
        external_link: Some((fixture.0.join("alias.txt"), PathBuf::from("real-b.txt"))),
        ..Default::default()
    });
    let report = failure(apply(&registry, &plan).await);
    assert_eq!(report["status"], "partial_failure");
    assert_eq!(report["files_touched"], json!(["real-a.txt"]));
    assert_eq!(report["recovery_failed"][0]["path"], "alias.txt");
    assert_eq!(fixture.text("real-a.txt"), "new-a");
    assert_eq!(fixture.text("alias.txt"), "untouched");
    assert_eq!(fixture.text("next.txt"), "old-next");
}

#[cfg(unix)]
#[test]
fn non_utf8_absolute_paths_render_without_panicking_or_replacing_the_relative_identity() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let registry = fixture.registry();
    let mut plan = registry
        .prepare_changes(vec![ChangeInput {
            path: "a.txt".into(),
            operation: Operation::Replace,
            content: "new".into(),
            expected_version: Some(version(b"old")),
        }])
        .unwrap();
    // macOS APFS rejects invalid filename bytes before any tool runs. Exercise
    // serialization directly here; Linux also runs the real filesystem case.
    plan.changes[0].path = fixture
        .0
        .join(OsString::from_vec(vec![b'w', 0xff]))
        .join("a.txt");
    let rendered = plan.render();
    assert!(rendered["changes"][0]["canonical_path"].is_null());
    assert!(rendered["changes"][0]["canonical_path_display"].is_string());
    assert_eq!(rendered["changes"][0]["target_path"], "a.txt");
}

#[cfg(all(unix, not(target_os = "macos")))]
#[tokio::test]
async fn non_utf8_workspace_roots_do_not_panic_in_preview_or_edit_approval() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    let root = fixture.0.join(OsString::from_vec(vec![b'w', 0xff]));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("a.txt"), "old").unwrap();
    let approver = Arc::new(RecordingApprover::new(true, None));
    let registry = ToolRegistry::new(&root, ApprovalMode::Strict)
        .unwrap()
        .with_approver(approver.clone());
    let plan = preview(&registry, vec![replace("a.txt", "old", "new")]).await;
    assert!(plan["changes"][0]["canonical_path"].is_null());
    assert_eq!(plan["changes"][0]["target_path"], "a.txt");
    let report: Value = serde_json::from_str(&apply(&registry, &plan).await.unwrap()).unwrap();
    assert_eq!(report["files_touched"], json!(["a.txt"]));
    registry
        .edit_file_versioned(edit("new", "final", Some(version(b"new"))))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "final"
    );
    assert_eq!(approver.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn binary_replacement_is_rejected_before_a_lossy_preview_can_hide_the_change() {
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("a.txt"), [0xff]).unwrap();
    let registry = fixture.registry();
    let report = failure(
        request(
            &registry,
            json!({"operation":"preview","changes":[{
                "path":"a.txt","operation":"replace","content":"\u{fffd}",
                "expected_version":version(&[0xff])
            }]}),
        )
        .await,
    );
    assert_eq!(report["status"], "rejected");
    assert_eq!(report["files_touched"], json!([]));
    assert!(report["reason"].as_str().unwrap().contains("UTF-8"));
    assert_eq!(std::fs::read(fixture.0.join("a.txt")).unwrap(), [0xff]);
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_public_execution_in_post_tool_keeps_the_complete_commit_and_consumed_preview() {
    use crate::hooks::{Hook, HookEvent, HookFailure, HookRegistry};
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let registry = fixture.registry();
    let plan = preview(
        &registry,
        vec![replace("a.txt", "old", "new"), create("b.txt", "new-b")],
    )
    .await;
    let registry = Arc::new(registry.with_hooks(HookRegistry::new(vec![Hook {
        name: "waiting-post-tool".into(),
        event: HookEvent::PostTool,
        command: "exec sleep 30".into(),
        blocking: false,
        timeout: Duration::from_secs(30),
        on_error: HookFailure::Ignore,
    }])));
    let running = registry.clone();
    let id = plan["preview_id"].clone();
    let task = tokio::spawn(async move {
        running
            .execute(&ToolCall {
                id: "post-tool-cancelled-commit".into(),
                name: "file_changes".into(),
                arguments: json!({"operation":"apply","preview_id":id}).to_string(),
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.text("a.txt") != "new" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("public execute reaches the post-commit hook");
    assert_eq!(fixture.text("b.txt"), "new-b");
    assert!(!task.is_finished(), "the PostTool hook is still pending");
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(fixture.text("a.txt"), "new");
    assert_eq!(fixture.text("b.txt"), "new-b");
    assert_eq!(
        failure(apply(&registry, &plan).await)["files_touched"],
        json!([])
    );
    // No returned result means unknown effect, rather than success or no writes.
    // agent::uncertain tests independently enforce fresh approval for this call.
}
