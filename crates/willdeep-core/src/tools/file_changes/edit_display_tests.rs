use super::*;
use crate::tools::edit_display::{MAX_FILE_DIFF_BYTES, extract_edit_display};

#[tokio::test]
async fn create_receipt_uses_written_content_preserves_summary_and_handles_empty_and_long_files() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    for (path, content) in [
        ("nested/created.txt", "一\ntwo\n".to_owned()),
        ("empty.txt", String::new()),
        ("long.txt", "长\n".repeat(MAX_FILE_DIFF_BYTES)),
    ] {
        let requested = format!("./{path}");
        let output = registry
            .execute(&ToolCall {
                id: "create-display".into(),
                name: "create_file".into(),
                arguments: json!({"path":requested,"content":content}).to_string(),
            })
            .await
            .unwrap();
        assert_eq!(fixture.text(path), content);
        let receipt: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(receipt["status"], "created");
        assert_eq!(receipt["files_touched"], json!([path]));
        assert_eq!(
            receipt["message"],
            format!("created {requested} ({} bytes)", content.len())
        );
        let display = extract_edit_display(&output, false).unwrap();
        let file = &display.files[0];
        assert_eq!(file.path, path);
        assert_eq!(file.added, content.split_inclusive('\n').count());
        assert_eq!(file.removed, 0);
        assert!(file.diff.len() <= MAX_FILE_DIFF_BYTES);
        match path {
            "nested/created.txt" => {
                assert_eq!(file.diff, "@@ -0,0 +1,2 @@\n+一\n+two\n");
                assert!(!file.truncated);
            }
            "empty.txt" => {
                assert!(file.diff.is_empty());
                assert!(!file.truncated);
            }
            _ => assert!(file.truncated),
        }
    }
}

#[tokio::test]
async fn edit_receipt_uses_whole_snapshots_after_normalized_matcher_and_preserves_evidence() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "one\n  two\nthree\n");
    let registry = fixture.registry();
    let output = registry
        .edit_file_versioned(edit(
            "     1  one\n     2    two",
            "first\n  second",
            Some(version(b"one\n  two\nthree\n")),
        ))
        .await
        .unwrap();
    assert_eq!(fixture.text("a.txt"), "first\n  second\nthree\n");
    let receipt: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(receipt["files_touched"], json!(["a.txt"]));
    assert_eq!(
        receipt["versions"]["a.txt"],
        version(b"first\n  second\nthree\n")
    );
    let display = extract_edit_display(&output, false).unwrap();
    let file = &display.files[0];
    assert_eq!(
        file.diff,
        "@@ -1,3 +1,3 @@\n-one\n-  two\n+first\n+  second\n three\n"
    );
    assert_eq!((file.added, file.removed, file.truncated), (2, 2, false));
    assert!(!file.diff.contains("     1"));
}

#[tokio::test]
async fn multifile_receipt_preserves_shared_context_between_adjacent_edits() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "first\nold\nshared\nold\nlast\n");
    let registry = fixture.registry();
    let plan = preview(
        &registry,
        vec![
            replace(
                "a.txt",
                "first\nold\nshared\nold\nlast\n",
                "first\nnew\nshared\nnew\nlast\n",
            ),
            create("b.txt", "created\n"),
            create("empty.txt", ""),
        ],
    )
    .await;
    let output = apply(&registry, &plan).await.unwrap();
    let display = extract_edit_display(&output, false).unwrap();
    assert_eq!(
        display
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["a.txt", "b.txt", "empty.txt"]
    );
    assert_eq!(
        display.files[0].diff,
        "@@ -1,5 +1,5 @@\n first\n-old\n+new\n shared\n-old\n+new\n last\n"
    );
    assert_eq!((display.files[0].added, display.files[0].removed), (2, 2));
    assert_eq!(display.files[1].diff, "@@ -0,0 +1,1 @@\n+created\n");
    assert!(display.files[2].diff.is_empty());
    assert!(display.files.iter().all(|file| !file.truncated));
    assert_eq!(fixture.text("a.txt"), "first\nnew\nshared\nnew\nlast\n");
    assert_eq!(fixture.text("b.txt"), "created\n");
    assert_eq!(fixture.text("empty.txt"), "");
}

#[tokio::test]
async fn denied_and_failed_creates_and_edits_have_no_success_receipt() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old\n");
    let registry = ToolRegistry::new(&fixture.0, ApprovalMode::Strict)
        .unwrap()
        .with_approver(Arc::new(RecordingApprover::new(false, None)));
    let denied = registry
        .create_file(CreateArgs {
            path: "denied.txt".into(),
            content: "new\n".into(),
        })
        .await;
    assert!(matches!(&denied, Err(ToolError::ApprovalDenied(_))));
    assert!(extract_edit_display(&denied.unwrap_err().to_string(), false).is_none());
    let denied = registry
        .edit_file_versioned(edit("old", "new", Some(version(b"old\n"))))
        .await;
    assert!(matches!(&denied, Err(ToolError::ApprovalDenied(_))));
    assert!(extract_edit_display(&denied.unwrap_err().to_string(), false).is_none());
    assert_eq!(fixture.text("a.txt"), "old\n");
    assert!(!fixture.0.join("denied.txt").exists());
    let registry = fixture.registry();
    let existing = registry
        .create_file(CreateArgs {
            path: "a.txt".into(),
            content: "overwrite".into(),
        })
        .await;
    assert!(matches!(&existing, Err(ToolError::FileAlreadyExists(_))));
    assert!(extract_edit_display(&existing.unwrap_err().to_string(), false).is_none());
    fixture.write("not-directory", "file");
    let failed = registry
        .create_file(CreateArgs {
            path: "not-directory/new.txt".into(),
            content: "new".into(),
        })
        .await;
    assert!(matches!(&failed, Err(ToolError::Io(_))));
    assert!(extract_edit_display(&failed.unwrap_err().to_string(), false).is_none());
}

#[tokio::test]
async fn successful_install_with_real_cleanup_failure_has_no_display_card() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let plan = preview(&registry, vec![create("a.txt", "written\n")]).await;
    *registry.file_changes.fault.lock().unwrap() = Some(TestFault {
        fail_cleanup: true,
        ..Default::default()
    });
    let report = failure(apply(&registry, &plan).await);
    assert_eq!(report["status"], "applied_with_cleanup_failure");
    assert_eq!(report["files_touched"], json!(["a.txt"]));
    assert_eq!(report["cleanup_failed"].as_array().unwrap().len(), 1);
    assert!(report.get("edit_display").is_none());
    assert_eq!(fixture.text("a.txt"), "written\n");
    // Remove only the artificial directory owned by this fixture.
    std::fs::remove_dir(report["cleanup_failed"][0]["path"].as_str().unwrap()).unwrap();
    assert_eq!(fixture.entries(), ["a.txt"]);
}

#[cfg(unix)]
#[tokio::test]
async fn receipts_report_the_resolved_destination_for_create_and_batch_aliases() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.0.join("real")).unwrap();
    std::os::unix::fs::symlink("real", fixture.0.join("alias")).unwrap();
    let registry = fixture.registry();
    let created = registry
        .create_file(CreateArgs {
            path: "alias/created.txt".into(),
            content: "first\n".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        extract_edit_display(&created, false).unwrap().files[0].path,
        "real/created.txt"
    );
    let plan = preview(
        &registry,
        vec![
            replace("alias/created.txt", "first\n", "second\n"),
            create("alias/b.txt", "other\n"),
        ],
    )
    .await;
    let output = apply(&registry, &plan).await.unwrap();
    let display = extract_edit_display(&output, false).unwrap();
    assert_eq!(display.files[0].path, "real/created.txt");
    assert_eq!(display.files[1].path, "real/b.txt");
    assert_eq!(fixture.text("real/created.txt"), "second\n");
    assert_eq!(fixture.text("real/b.txt"), "other\n");
}
