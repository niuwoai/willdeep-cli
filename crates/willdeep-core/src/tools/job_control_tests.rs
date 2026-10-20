use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct RejectApproval(Arc<AtomicUsize>);

#[async_trait]
impl Approver for RejectApproval {
    async fn approve(&self, _: &str, _: bool) -> ApprovalDecision {
        self.0.fetch_add(1, Ordering::SeqCst);
        ApprovalDecision::Deny
    }
}

#[tokio::test]
async fn full_access_cancels_its_in_process_job_without_approval() {
    let root = std::env::temp_dir().join(format!("job-cancel-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let background = Arc::new(BackgroundTaskRegistry::default());
    let mut events = background.subscribe();
    let id = background.start_retriable(BackgroundTaskKind::Shell, "own job".into(), || async {
        std::future::pending::<crate::background::TaskResult>().await
    });
    let tools = ToolRegistry::new(&root, ApprovalMode::FullAccess)
        .unwrap()
        .with_background_tasks(background)
        .with_approver(Arc::new(RejectApproval(calls.clone())));
    assert!(
        tools
            .kill_job(JobIDArgs { job_id: id })
            .await
            .unwrap()
            .contains("kill requested")
    );
    let event = tokio::time::timeout(std::time::Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.snapshot.status, BackgroundTaskStatus::Killed);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn cancellation_exemption_requires_full_access_and_explicit_detached_ownership() {
    for (mode, owner, expected_approvals) in [
        (ApprovalMode::FullAccess, Some("current"), 0),
        (ApprovalMode::FullAccess, Some("another-session"), 1),
        (ApprovalMode::FullAccess, None, 1),
        (ApprovalMode::Smart, Some("current"), 1),
        (ApprovalMode::WorkspaceAccess, Some("current"), 1),
        (ApprovalMode::Strict, Some("current"), 1),
    ] {
        let root = std::env::temp_dir().join(format!("detached-owner-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let jobs = Arc::new(crate::DetachedJobStore::new(&root).with_owner("current"));
        let job = crate::DetachedJob {
            id: "job_fixture".into(),
            command: "completed fixture".into(),
            label: "fixture".into(),
            workspace: root.clone(),
            pid: 0,
            started_marker: None,
            created_at: 1,
            owner: owner.map(str::to_owned),
        };
        let directory = jobs.directory().join(&job.id);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("meta.json"),
            serde_json::to_vec(&job).unwrap(),
        )
        .unwrap();
        std::fs::write(directory.join("exit"), "0").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let tools = ToolRegistry::new(&root, mode)
            .unwrap()
            .with_detached_jobs(jobs.clone())
            .with_approver(Arc::new(RejectApproval(calls.clone())));
        let result = tools
            .kill_job(JobIDArgs {
                job_id: job.id.clone(),
            })
            .await;
        if expected_approvals == 0 {
            assert!(result.unwrap_err().to_string().contains("already finished"));
        } else {
            assert!(matches!(result, Err(ToolError::ApprovalDenied(_))));
        }
        assert_eq!(calls.load(Ordering::SeqCst), expected_approvals);
        assert!(jobs.get("../job_fixture").is_none());
        assert!(jobs.get("..\\job_fixture").is_none());
        let mut mismatched = job;
        mismatched.id = "job_other".into();
        std::fs::write(
            directory.join("meta.json"),
            serde_json::to_vec(&mismatched).unwrap(),
        )
        .unwrap();
        assert!(jobs.get("job_fixture").is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
