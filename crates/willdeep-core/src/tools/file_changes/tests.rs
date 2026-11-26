use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

use super::*;
use crate::tools::{ApprovalDecision, Approver, CreateArgs, ReadArgs};

#[path = "contract_tests.rs"]
mod contract_tests;
#[path = "edit_display_tests.rs"]
mod edit_display_tests;
#[path = "review_tests.rs"]
mod review_tests;
#[path = "typed_preview_tests.rs"]
mod typed_preview_tests;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("willdeep-file-changes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }

    fn write(&self, path: &str, content: &str) {
        std::fs::write(self.0.join(path), content).unwrap();
    }

    fn text(&self, path: &str) -> String {
        std::fs::read_to_string(self.0.join(path)).unwrap()
    }

    fn registry(&self) -> ToolRegistry {
        ToolRegistry::new(&self.0, ApprovalMode::WorkspaceAccess).unwrap()
    }

    fn entries(&self) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // The fixture owns only its unique, generated temporary directory.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn workspace_preview_reads_current_text_and_refuses_non_workspace_or_non_text_paths() {
    let workspace = Fixture::new();
    let outside = Fixture::new();
    workspace.write("report.md", "# Report\n");
    outside.write("outside.md", "outside");
    workspace.write("binary.txt", "\0");
    workspace.write(".env.txt", "hidden");
    std::fs::write(workspace.0.join("invalid.txt"), [0xff]).unwrap();
    std::fs::write(
        workspace.0.join("large.txt"),
        vec![b'a'; crate::tools::MAX_READ_BYTES + 1],
    )
    .unwrap();
    let absolute = workspace.0.join("report.md");
    for path in [Path::new("./report.md"), &absolute] {
        assert_eq!(
            read_workspace_preview(&workspace.0, path).unwrap(),
            "# Report\n"
        );
    }
    let outside_path = outside.0.join("outside.md");
    for path in [
        Path::new("../outside.md"),
        &outside_path,
        Path::new(".env.txt"),
        Path::new("binary.txt"),
        Path::new("invalid.txt"),
        Path::new("large.txt"),
        Path::new("missing.md"),
        Path::new("."),
    ] {
        assert!(
            read_workspace_preview(&workspace.0, path).is_err(),
            "{path:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn workspace_preview_does_not_follow_file_or_directory_symlinks() {
    let workspace = Fixture::new();
    let outside = Fixture::new();
    outside.write("report.md", "outside");
    std::os::unix::fs::symlink(outside.0.join("report.md"), workspace.0.join("escape.md")).unwrap();
    std::os::unix::fs::symlink(&outside.0, workspace.0.join("escape")).unwrap();
    assert!(read_workspace_preview(&workspace.0, Path::new("escape.md")).is_err());
    assert!(read_workspace_preview(&workspace.0, Path::new("escape/report.md")).is_err());
}

fn edit(old: &str, new: &str, expected: Option<String>) -> EditArgs {
    EditArgs {
        path: "a.txt".to_owned(),
        old_string: old.to_owned(),
        new_string: new.to_owned(),
        replace_all: None,
        expected_version: expected,
    }
}

fn replace(path: &str, before: &str, after: &str) -> Value {
    json!({"path":path,"operation":"replace","content":after,"expected_version":version(before.as_bytes())})
}

fn create(path: &str, content: &str) -> Value {
    json!({"path":path,"operation":"create","content":content})
}

async fn request(registry: &ToolRegistry, args: Value) -> Result<String, ToolError> {
    registry
        .file_changes(&ToolCall {
            id: "isolated-file-change-test".to_owned(),
            name: "file_changes".to_owned(),
            arguments: args.to_string(),
        })
        .await
}

async fn preview(registry: &ToolRegistry, changes: Vec<Value>) -> Value {
    let output = request(registry, json!({"operation":"preview","changes":changes}))
        .await
        .unwrap();
    let report: Value = serde_json::from_str(&output).unwrap();
    assert!(report.get("edit_display").is_none());
    assert!(crate::tools::edit_display::extract_edit_display(&output, false).is_none());
    report
}

async fn apply(registry: &ToolRegistry, preview: &Value) -> Result<String, ToolError> {
    request(
        registry,
        json!({"operation":"apply","preview_id":preview["preview_id"]}),
    )
    .await
}

fn failure(result: Result<String, ToolError>) -> Value {
    match result {
        Err(ToolError::FileChange(report)) => {
            let value: Value = serde_json::from_str(&report).expect("structured failure report");
            assert!(value.get("edit_display").is_none());
            assert!(crate::tools::edit_display::extract_edit_display(&report, false).is_none());
            value
        }
        other => panic!("expected structured FileChange failure, got {other:?}"),
    }
}

#[tokio::test]
async fn unread_edit_is_rejected_and_actual_read_establishes_full_file_baseline() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "one\ntwo\nthree\n");
    let registry = fixture.registry();
    assert!(matches!(
        registry
            .edit_file_versioned(edit("two", "changed", None))
            .await,
        Err(ToolError::ReadRequired(_))
    ));
    assert_eq!(fixture.text("a.txt"), "one\ntwo\nthree\n");
    // A paged read must observe all raw bytes, including lines outside the page.
    let output = registry
        .read_file(ReadArgs {
            path: "a.txt".into(),
            offset: Some(2),
            limit: Some(1),
            max_bytes: None,
        })
        .await
        .unwrap();
    assert!(output.contains(&version(b"one\ntwo\nthree\n")));
    fixture.write("a.txt", "one\ntwo\nexternal\n");
    assert!(matches!(
        registry
            .edit_file_versioned(edit("two", "changed", None))
            .await,
        Err(ToolError::EditConflict { .. })
    ));
    assert_eq!(fixture.text("a.txt"), "one\ntwo\nexternal\n");
}

#[tokio::test]
async fn explicit_version_works_and_success_updates_only_this_registrys_baseline() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "alpha beta");
    let registry = fixture.registry();
    let output: Value = serde_json::from_str(
        &registry
            .edit_file_versioned(edit("beta", "gamma", Some(version(b"alpha beta"))))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(output["versions"]["a.txt"], version(b"alpha gamma"));
    assert_eq!(output["files_touched"], json!(["a.txt"]));
    registry
        .edit_file_versioned(edit("gamma", "delta", None))
        .await
        .unwrap();
    assert_eq!(fixture.text("a.txt"), "alpha delta");
    assert!(matches!(
        fixture
            .registry()
            .edit_file_versioned(edit("delta", "other", None))
            .await,
        Err(ToolError::ReadRequired(_))
    ));
}

#[tokio::test]
async fn explicit_stale_version_wins_over_fresh_implicit_baseline() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "fresh");
    let registry = fixture.registry();
    registry
        .file_changes
        .observe(&fixture.0.join("a.txt"), b"fresh");
    let result = registry
        .edit_file_versioned(edit("fresh", "other", Some(version(b"stale"))))
        .await;
    let Err(ToolError::EditConflict {
        expected, actual, ..
    }) = result
    else {
        panic!("{result:?}");
    };
    assert_eq!(expected, version(b"stale"));
    assert_eq!(actual, version(b"fresh"));
    assert_eq!(fixture.text("a.txt"), "fresh");
}

#[tokio::test]
async fn duplicate_exact_spans_and_copied_line_gutters_keep_matcher_semantics() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "same same");
    let registry = fixture.registry();
    let result = registry
        .edit_file_versioned(edit("same", "new", Some(version(b"same same"))))
        .await;
    assert!(matches!(
        result,
        Err(ToolError::EditTextNotUnique { count: 2, .. })
    ));
    assert_eq!(fixture.text("a.txt"), "same same");
    let mut replace_all = edit("same", "new", Some(version(b"same same")));
    replace_all.replace_all = Some(true);
    registry.edit_file_versioned(replace_all).await.unwrap();
    assert_eq!(fixture.text("a.txt"), "new new");
    fixture.write("a.txt", "one\n  two\nthree\n");
    registry
        .edit_file_versioned(edit(
            "     1  one\n     2    two",
            "first\n  second",
            Some(version(b"one\n  two\nthree\n")),
        ))
        .await
        .unwrap();
    assert_eq!(fixture.text("a.txt"), "first\n  second\nthree\n");
}

#[tokio::test]
async fn concurrent_registries_cannot_both_apply_the_same_content_version() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "original");
    let first = fixture.registry();
    let second = fixture.registry();
    let expected = version(b"original");
    let (left, right) = tokio::join!(
        first.edit_file_versioned(edit("original", "left", Some(expected.clone()))),
        second.edit_file_versioned(edit("original", "right", Some(expected))),
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert!(
        matches!(left, Err(ToolError::EditConflict { .. }))
            || matches!(right, Err(ToolError::EditConflict { .. }))
    );
    assert!(["left", "right"].contains(&fixture.text("a.txt").as_str()));
}

#[tokio::test]
async fn preview_is_memory_only_binding_bounded_diff_and_apply_payload_is_immutable() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let registry = fixture.registry();
    let before = fixture.entries();
    let plan = preview(
        &registry,
        vec![replace("a.txt", "old", "new"), create("b.txt", "created")],
    )
    .await;
    assert_eq!(fixture.entries(), before);
    assert_eq!(fixture.text("a.txt"), "old");
    assert_eq!(plan["files_touched"], json!([]));
    assert_eq!(
        uuid::Uuid::parse_str(plan["preview_id"].as_str().unwrap())
            .unwrap()
            .get_version_num(),
        4
    );
    assert_eq!(plan["changes"][0]["expected_version"], version(b"old"));
    assert_eq!(plan["changes"][0]["new_version"], version(b"new"));
    assert!(
        plan["changes"][0]["diff"]
            .as_str()
            .unwrap()
            .contains("-old")
    );
    let tampered = request(&registry, json!({"operation":"apply","preview_id":plan["preview_id"],"changes":[create("evil.txt","evil")]})).await;
    assert!(matches!(tampered, Err(ToolError::InvalidArguments { .. })));
    assert!(
        apply(&fixture.registry(), &plan).await.is_err(),
        "preview must belong to its issuing registry"
    );
    let applied: Value = serde_json::from_str(&apply(&registry, &plan).await.unwrap()).unwrap();
    assert_eq!(applied["files_touched"], json!(["a.txt", "b.txt"]));
    assert_eq!(fixture.text("a.txt"), "new");
    assert_eq!(fixture.text("b.txt"), "created");
    assert!(
        apply(&registry, &plan).await.is_err(),
        "preview cannot be replayed"
    );
    assert_eq!(fixture.entries(), vec!["a.txt", "b.txt"]);
}

#[tokio::test]
async fn preview_replace_uses_actual_baseline_but_never_establishes_one_itself() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let registry = fixture.registry();
    let changes = vec![json!({"path":"a.txt","operation":"replace","content":"new"})];
    assert!(matches!(
        request(&registry, json!({"operation":"preview","changes":changes})).await,
        Err(ToolError::ReadRequired(_))
    ));
    let plan = preview(&registry, vec![replace("a.txt", "old", "new")]).await;
    assert!(matches!(
        registry
            .edit_file_versioned(edit("old", "other", None))
            .await,
        Err(ToolError::ReadRequired(_))
    ));
    registry
        .read_file(ReadArgs {
            path: "a.txt".into(),
            offset: None,
            limit: None,
            max_bytes: None,
        })
        .await
        .unwrap();
    preview(
        &registry,
        vec![json!({"path":"a.txt","operation":"replace","content":"other"})],
    )
    .await;
    assert!(!plan["preview_id"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn preflight_conflict_writes_nothing_and_consumes_stale_preview() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old-a");
    fixture.write("b.txt", "old-b");
    let registry = fixture.registry();
    let plan = preview(
        &registry,
        vec![
            replace("a.txt", "old-a", "new-a"),
            replace("b.txt", "old-b", "new-b"),
        ],
    )
    .await;
    fixture.write("b.txt", "external-b");
    let report = failure(apply(&registry, &plan).await);
    assert_eq!(report["applied"], json!([]));
    assert_eq!(report["files_touched"], json!([]));
    assert_eq!(report["status"], "preflight_failed");
    assert_eq!(fixture.text("a.txt"), "old-a");
    assert_eq!(fixture.text("b.txt"), "external-b");
    assert_eq!(fixture.entries(), vec!["a.txt", "b.txt"]);
    assert!(apply(&registry, &plan).await.is_err());
}

#[tokio::test]
async fn create_requires_absence_and_an_existing_parent_and_never_truncates() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "keep");
    let registry = fixture.registry();
    assert!(
        request(
            &registry,
            json!({"operation":"preview","changes":[create("a.txt","overwrite")]})
        )
        .await
        .is_err()
    );
    assert!(
        request(
            &registry,
            json!({"operation":"preview","changes":[create("missing/b.txt","new")]})
        )
        .await
        .is_err()
    );
    assert!(!fixture.0.join("missing").exists());
    let plan = preview(&registry, vec![create("b.txt", "ours")]).await;
    fixture.write("b.txt", "external");
    assert_eq!(
        failure(apply(&registry, &plan).await)["files_touched"],
        json!([])
    );
    assert_eq!(fixture.text("a.txt"), "keep");
    assert_eq!(fixture.text("b.txt"), "external");
}

#[tokio::test]
async fn invalid_paths_duplicate_canonical_paths_and_write_scope_fail_before_writing() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    fixture.write("b.txt", "old");
    let registry = fixture.registry();
    for path in [
        "../escape.txt".to_owned(),
        fixture.0.join("absolute.txt").display().to_string(),
        String::new(),
    ] {
        assert!(
            request(
                &registry,
                json!({"operation":"preview","changes":[create(&path,"new")]})
            )
            .await
            .is_err()
        );
    }
    let duplicate = request(&registry, json!({"operation":"preview","changes":[replace("a.txt","old","one"),replace("./a.txt","old","two")]})).await;
    assert!(matches!(duplicate, Err(ToolError::FileChange(_))));
    let scoped = fixture
        .registry()
        .with_write_targets(Some(BTreeSet::from([fixture.0.join("a.txt")])));
    assert!(request(&scoped, json!({"operation":"preview","changes":[replace("a.txt","old","one"),replace("b.txt","old","two")]})).await.is_err());
    assert_eq!(fixture.text("a.txt"), "old");
    assert_eq!(fixture.text("b.txt"), "old");
    let readonly = ToolRegistry::new(&fixture.0, ApprovalMode::ReadOnly).unwrap();
    assert!(matches!(
        request(
            &readonly,
            json!({"operation":"preview","changes":[create("new.txt","x")]})
        )
        .await,
        Err(ToolError::ReadOnlyPolicy(_))
    ));
    assert!(matches!(
        readonly
            .edit_file_versioned(edit("old", "new", Some(version(b"old"))))
            .await,
        Err(ToolError::ReadOnlyPolicy(_))
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_aliases_and_retargeting_are_rejected() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let outside = Fixture::new();
    fixture.write("a.txt", "old");
    fixture.write("b.txt", "old");
    outside.write("outside.txt", "external");
    symlink(&outside.0, fixture.0.join("escape")).unwrap();
    symlink("a.txt", fixture.0.join("alias.txt")).unwrap();
    let registry = fixture.registry();
    assert!(request(&registry, json!({"operation":"preview","changes":[replace("escape/outside.txt","external","bad")]})).await.is_err());
    assert!(
        request(
            &registry,
            json!({"operation":"preview","changes":[create("escape/new.txt","bad")]})
        )
        .await
        .is_err()
    );
    assert!(request(&registry, json!({"operation":"preview","changes":[replace("a.txt","old","one"),replace("alias.txt","old","two")]})).await.is_err());
    let plan = preview(&registry, vec![replace("alias.txt", "old", "changed")]).await;
    std::fs::remove_file(fixture.0.join("alias.txt")).unwrap();
    symlink("b.txt", fixture.0.join("alias.txt")).unwrap();
    assert_eq!(
        failure(apply(&registry, &plan).await)["files_touched"],
        json!([])
    );
    assert_eq!(fixture.text("a.txt"), "old");
    assert_eq!(fixture.text("b.txt"), "old");
    assert_eq!(outside.text("outside.txt"), "external");
}

#[cfg(unix)]
#[tokio::test]
async fn parent_symlink_swap_and_dangling_create_symlink_do_not_escape() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let outside = Fixture::new();
    std::fs::create_dir(fixture.0.join("dir")).unwrap();
    fixture.write("dir/a.txt", "old");
    outside.write("a.txt", "old");
    let registry = fixture.registry();
    let plan = preview(&registry, vec![replace("dir/a.txt", "old", "new")]).await;
    std::fs::rename(fixture.0.join("dir"), fixture.0.join("moved")).unwrap();
    symlink(&outside.0, fixture.0.join("dir")).unwrap();
    assert_eq!(
        failure(apply(&registry, &plan).await)["files_touched"],
        json!([])
    );
    assert_eq!(outside.text("a.txt"), "old");
    symlink(
        outside.0.join("missing.txt"),
        fixture.0.join("dangling.txt"),
    )
    .unwrap();
    assert!(
        request(
            &registry,
            json!({"operation":"preview","changes":[create("dangling.txt","bad")]})
        )
        .await
        .is_err()
    );
    assert!(!outside.0.join("missing.txt").exists());
}

#[tokio::test]
async fn bounded_requests_cache_expiration_and_unsupported_operations_are_explicit() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    let too_many: Vec<_> = (0..=MAX_FILES)
        .map(|index| create(&format!("file-{index}"), "x"))
        .collect();
    assert!(
        request(&registry, json!({"operation":"preview","changes":too_many}))
            .await
            .is_err()
    );
    assert!(request(&registry,json!({"operation":"preview","changes":[create("huge.txt",&"x".repeat(MAX_TOTAL_BYTES+1))]})).await.is_err());
    for operation in ["delete", "rename"] {
        assert!(matches!(request(&registry,json!({"operation":"preview","changes":[{"path":"x","operation":operation,"content":""}]})).await,Err(ToolError::InvalidArguments{..})));
    }
    let plan = preview(&registry, vec![create("expired.txt", "x")]).await;
    {
        let mut state = registry.file_changes.inner.lock().unwrap();
        state
            .previews
            .get_mut(plan["preview_id"].as_str().unwrap())
            .unwrap()
            .created = Instant::now() - PREVIEW_TTL;
    }
    assert!(apply(&registry, &plan).await.is_err());
    for index in 0..MAX_PREVIEWS {
        preview(&registry, vec![create(&format!("cached-{index}"), "x")]).await;
    }
    assert!(
        request(
            &registry,
            json!({"operation":"preview","changes":[create("full.txt","x")]})
        )
        .await
        .is_err()
    );
    assert!(fixture.entries().is_empty());
}

#[test]
fn diff_is_byte_bounded_utf8_safe_and_marks_truncation() {
    let (diff, truncated) = bounded_diff(
        b"old\n",
        "新".repeat(MAX_DIFF_BYTES).as_bytes(),
        MAX_DIFF_BYTES,
    );
    assert!(truncated);
    assert!(diff.len() <= MAX_DIFF_BYTES);
    assert!(diff.starts_with("@@"));
    assert_eq!(bounded_diff(b"same", b"same", 10), (String::new(), false));
    assert_eq!(bounded_diff(b"a", b"b", 0), (String::new(), true));
}

struct RecordingApprover {
    calls: AtomicUsize,
    descriptions: Mutex<Vec<String>>,
    action: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    allow: bool,
}

impl RecordingApprover {
    fn new(allow: bool, action: Option<Box<dyn FnOnce() + Send>>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            descriptions: Mutex::new(Vec::new()),
            action: Mutex::new(action),
            allow,
        }
    }
}

#[async_trait::async_trait]
impl Approver for RecordingApprover {
    async fn approve(&self, description: &str, always_allow_available: bool) -> ApprovalDecision {
        assert!(!always_allow_available);
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.descriptions
            .lock()
            .unwrap()
            .push(description.to_owned());
        if let Some(action) = self.action.lock().unwrap().take() {
            action();
        }
        if self.allow {
            ApprovalDecision::AllowOnce
        } else {
            ApprovalDecision::Deny
        }
    }
}

#[tokio::test]
async fn exactly_one_approval_binds_the_rendered_diff_and_denial_is_memory_only() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let approver = Arc::new(RecordingApprover::new(true, None));
    let registry = ToolRegistry::new(&fixture.0, ApprovalMode::Strict)
        .unwrap()
        .with_approver(approver.clone());
    let plan = preview(
        &registry,
        vec![replace("a.txt", "old", "new"), create("b.txt", "new-b")],
    )
    .await;
    assert_eq!(approver.calls.load(Ordering::SeqCst), 0);
    apply(&registry, &plan).await.unwrap();
    assert_eq!(approver.calls.load(Ordering::SeqCst), 1);
    {
        let descriptions = approver.descriptions.lock().unwrap();
        assert!(descriptions[0].contains(plan["binding"].as_str().unwrap()));
        assert!(
            descriptions[0].contains(plan["changes"][0]["diff"].as_str().unwrap().trim_end())
                || descriptions[0].contains("-old")
        );
    }
    let denied = Arc::new(RecordingApprover::new(false, None));
    let registry = ToolRegistry::new(&fixture.0, ApprovalMode::Strict)
        .unwrap()
        .with_approver(denied);
    let plan = preview(&registry, vec![create("denied.txt", "x")]).await;
    assert!(matches!(
        apply(&registry, &plan).await,
        Err(ToolError::ApprovalDenied(_))
    ));
    assert_eq!(fixture.entries(), vec!["a.txt", "b.txt"]);
    assert!(apply(&registry, &plan).await.is_err());
}

#[tokio::test]
async fn edits_and_batches_revalidate_changes_that_happen_during_approval() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old");
    let path = fixture.0.join("a.txt");
    let approver = Arc::new(RecordingApprover::new(
        true,
        Some(Box::new(move || std::fs::write(path, "external").unwrap())),
    ));
    let registry = ToolRegistry::new(&fixture.0, ApprovalMode::Strict)
        .unwrap()
        .with_approver(approver);
    let result = registry
        .edit_file_versioned(edit("old", "new", Some(version(b"old"))))
        .await;
    assert!(matches!(result, Err(ToolError::EditConflict { .. })));
    assert_eq!(fixture.text("a.txt"), "external");
    fixture.write("a.txt", "old");
    let path = fixture.0.join("a.txt");
    let approver = Arc::new(RecordingApprover::new(
        true,
        Some(Box::new(move || std::fs::write(path, "external").unwrap())),
    ));
    let registry = ToolRegistry::new(&fixture.0, ApprovalMode::Strict)
        .unwrap()
        .with_approver(approver);
    let plan = preview(
        &registry,
        vec![replace("a.txt", "old", "new"), create("b.txt", "new-b")],
    )
    .await;
    assert_eq!(
        failure(apply(&registry, &plan).await)["files_touched"],
        json!([])
    );
    assert_eq!(fixture.text("a.txt"), "external");
    assert!(!fixture.0.join("b.txt").exists());
}

#[tokio::test]
async fn mid_commit_failure_rolls_back_created_and_replaced_files_and_reports_history() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old-a");
    fixture.write("b.txt", "old-b");
    let registry = fixture.registry();
    let plan = preview(
        &registry,
        vec![
            replace("a.txt", "old-a", "new-a"),
            create("created.txt", "new-c"),
            replace("b.txt", "old-b", "new-b"),
        ],
    )
    .await;
    *registry.file_changes.fault.lock().unwrap() = Some(TestFault {
        fail_before: Some(2),
        ..Default::default()
    });
    let report = failure(apply(&registry, &plan).await);
    assert_eq!(report["status"], "rolled_back");
    assert_eq!(report["applied"], json!(["a.txt", "created.txt"]));
    assert_eq!(report["rolled_back"], json!(["created.txt", "a.txt"]));
    assert_eq!(report["files_touched"], json!([]));
    assert_eq!(report["recovery_failed"], json!([]));
    assert_eq!(fixture.text("a.txt"), "old-a");
    assert_eq!(fixture.text("b.txt"), "old-b");
    assert_eq!(fixture.entries(), vec!["a.txt", "b.txt"]);
}

#[tokio::test]
async fn recovery_preserves_external_new_content_and_exposes_retained_touches() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old-a");
    fixture.write("b.txt", "old-b");
    let registry = fixture.registry();
    let plan = preview(
        &registry,
        vec![
            replace("a.txt", "old-a", "new-a"),
            replace("b.txt", "old-b", "new-b"),
        ],
    )
    .await;
    *registry.file_changes.fault.lock().unwrap() = Some(TestFault {
        fail_before: Some(1),
        external_write: Some((fixture.0.join("a.txt"), b"external-after-install".to_vec())),
        ..Default::default()
    });
    let report = failure(apply(&registry, &plan).await);
    assert_eq!(report["status"], "partial_failure");
    assert_eq!(report["applied"], json!(["a.txt"]));
    assert_eq!(report["rolled_back"], json!([]));
    assert_eq!(report["files_touched"], json!(["a.txt"]));
    assert_eq!(report["recovery_failed"][0]["path"], "a.txt");
    assert_eq!(fixture.text("a.txt"), "external-after-install");
    assert_eq!(fixture.text("b.txt"), "old-b");
    assert_eq!(fixture.entries(), vec!["a.txt", "b.txt"]);
}

#[tokio::test]
async fn recovery_io_failure_never_claims_transaction_success() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "old-a");
    fixture.write("b.txt", "old-b");
    let registry = fixture.registry();
    let plan = preview(
        &registry,
        vec![
            replace("a.txt", "old-a", "new-a"),
            replace("b.txt", "old-b", "new-b"),
        ],
    )
    .await;
    *registry.file_changes.fault.lock().unwrap() = Some(TestFault {
        fail_before: Some(1),
        fail_recovery: true,
        ..Default::default()
    });
    let report = failure(apply(&registry, &plan).await);
    assert_eq!(report["status"], "partial_failure");
    assert_eq!(report["files_touched"], json!(["a.txt"]));
    assert_eq!(report["recovery_failed"].as_array().unwrap().len(), 1);
    assert_eq!(fixture.text("a.txt"), "new-a");
    assert_eq!(fixture.text("b.txt"), "old-b");
    assert_eq!(fixture.entries(), vec!["a.txt", "b.txt"]);
}

struct WaitingApprover {
    entered: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl Approver for WaitingApprover {
    async fn approve(&self, _description: &str, _always_allow_available: bool) -> ApprovalDecision {
        self.entered.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn cancellation_while_waiting_for_approval_has_no_files_or_backups() {
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
        vec![replace("a.txt", "old", "new"), create("b.txt", "new-b")],
    )
    .await;
    let running_registry = registry.clone();
    let running_plan = plan.clone();
    let task = tokio::spawn(async move { apply(&running_registry, &running_plan).await });
    approver.entered.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(fixture.text("a.txt"), "old");
    assert_eq!(fixture.entries(), vec!["a.txt"]);
    assert!(apply(&registry, &plan).await.is_err());
}
