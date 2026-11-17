use super::*;
use crate::hooks::{Hook, HookFailure, HookRegistry};
use crate::judge::SafetyJudge;
use crate::tools::Approver;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

type ApprovalRequests = Arc<Mutex<Vec<(String, bool)>>>;

struct FixedApprover {
    decision: ApprovalDecision,
    requests: ApprovalRequests,
}

#[async_trait::async_trait]
impl Approver for FixedApprover {
    async fn approve(&self, description: &str, always_allow_available: bool) -> ApprovalDecision {
        self.requests
            .lock()
            .unwrap()
            .push((description.to_owned(), always_allow_available));
        self.decision
    }
}

struct FixedJudge {
    verdict: JudgeVerdict,
    requests: Arc<Mutex<Vec<JudgeRequest>>>,
}

#[async_trait::async_trait]
impl SafetyJudge for FixedJudge {
    async fn judge(&self, request: JudgeRequest) -> JudgeVerdict {
        self.requests.lock().unwrap().push(request);
        self.verdict.clone()
    }
}

struct Fixture {
    root: PathBuf,
    captured: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("willdeep-approval-hook-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        Self {
            captured: root.join("events.jsonl"),
            root,
        }
    }

    fn audit_hook(&self) -> Hook {
        let path = self.captured.display().to_string();
        let command = if cfg!(windows) {
            format!(
                "[IO.File]::AppendAllText('{}', [Console]::In.ReadToEnd() + [Environment]::NewLine)",
                path.replace('\'', "''")
            )
        } else {
            let quoted = path.replace('\'', "'\\''");
            format!("cat >> '{quoted}'; printf '\\n' >> '{quoted}'")
        };
        Hook {
            name: "approval audit".to_owned(),
            event: HookEvent::ApprovalResolved,
            command,
            // ApprovalResolved must stay observational even with this setting.
            blocking: true,
            timeout: Duration::from_secs(5),
            on_error: HookFailure::Deny,
        }
    }

    fn registry(&self, decision: ApprovalDecision) -> (ToolRegistry, ApprovalRequests) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let registry = ToolRegistry::new(&self.root, ApprovalMode::Strict)
            .unwrap()
            .with_hooks(HookRegistry::new(vec![self.audit_hook()]))
            .with_session_id(Some("approval-session-fixture".to_owned()))
            .with_approver(Arc::new(FixedApprover {
                decision,
                requests: requests.clone(),
            }));
        (registry, requests)
    }

    fn events(&self) -> Vec<HookPayload> {
        let text = match std::fs::read_to_string(&self.captured) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => panic!("read approval hook stdin: {error}"),
        };
        text.lines()
            .map(|line| serde_json::from_str(line).expect("hook stdin is JSON"))
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn outcome(decision: ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::AllowOnce => "allow_once",
        ApprovalDecision::AlwaysAllow => "always_allow",
        ApprovalDecision::Deny => "deny",
    }
}

#[derive(Clone, Copy, Debug)]
enum Entry {
    Direct,
    UncertainReplay,
    Rememberable,
}

#[tokio::test]
async fn three_approval_entries_report_each_returned_decision() {
    for entry in [Entry::Direct, Entry::UncertainReplay, Entry::Rememberable] {
        for decision in [
            ApprovalDecision::AllowOnce,
            ApprovalDecision::Deny,
            ApprovalDecision::AlwaysAllow,
        ] {
            let fixture = Fixture::new();
            let (registry, requests) = fixture.registry(decision);
            let result = match entry {
                Entry::Direct => registry.require_approval("write fixture", false).await,
                Entry::UncertainReplay => {
                    registry
                        .approve_uncertain_replay(&ToolCall {
                            id: "interrupted".to_owned(),
                            name: "create_file".to_owned(),
                            arguments: r#"{"path":"fixture.txt","content":"fixture"}"#.to_owned(),
                        })
                        .await
                }
                Entry::Rememberable => {
                    registry
                        .require_rememberable_approval("run fixture", "fixture-rule".to_owned())
                        .await
                }
            };
            let allowed = match entry {
                Entry::UncertainReplay => decision == ApprovalDecision::AllowOnce,
                _ => decision != ApprovalDecision::Deny,
            };
            assert_eq!(result.is_ok(), allowed, "{entry:?}: {decision:?}");
            if !allowed {
                assert!(matches!(result, Err(ToolError::ApprovalDenied(_))));
            }
            // fire is awaited on the approval chain: no polling for the observer.
            let events = fixture.events();
            assert_eq!(events.len(), 1, "{entry:?}: {decision:?}");
            let event = &events[0];
            assert_eq!(event.event, "approval_resolved");
            assert_eq!(event.outcome.as_deref(), Some(outcome(decision)));
            assert_eq!(event.extra["source"], "approver");
            assert_eq!(event.extra["mode"], "strict");
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(event.tool.as_deref(), Some(requests[0].0.as_str()));
            assert_eq!(event.detail.as_deref(), Some(requests[0].0.as_str()));
            assert_eq!(requests[0].1, matches!(entry, Entry::Rememberable));
            assert_eq!(
                registry
                    .always_allowed
                    .lock()
                    .unwrap()
                    .contains("fixture-rule"),
                matches!(entry, Entry::Rememberable) && decision == ApprovalDecision::AlwaysAllow
            );
        }
    }
}

#[tokio::test]
async fn execute_create_file_writes_only_when_approved_and_reports_once() {
    for decision in [ApprovalDecision::AllowOnce, ApprovalDecision::Deny] {
        let fixture = Fixture::new();
        let (registry, requests) = fixture.registry(decision);
        let target = fixture.root.join("created-by-tool.txt");
        let result = registry
            .execute(&ToolCall {
                id: "create-fixture".to_owned(),
                name: "create_file".to_owned(),
                arguments: serde_json::json!({
                    "path": "created-by-tool.txt",
                    "content": "approved tool side effect",
                })
                .to_string(),
            })
            .await;
        match decision {
            ApprovalDecision::AllowOnce => {
                result.expect("approved create_file must execute");
                assert_eq!(
                    std::fs::read_to_string(&target).unwrap(),
                    "approved tool side effect"
                );
            }
            ApprovalDecision::Deny => {
                assert!(matches!(result, Err(ToolError::ApprovalDenied(_))));
                assert!(
                    !target.exists(),
                    "denied create_file must not create a file"
                );
            }
            ApprovalDecision::AlwaysAllow => unreachable!("fixture covers one-time decisions"),
        }
        assert_eq!(requests.lock().unwrap().len(), 1);
        let events = fixture.events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].tool.as_deref(),
            Some("create file: created-by-tool.txt")
        );
        assert_eq!(events[0].outcome.as_deref(), Some(outcome(decision)));
        assert_eq!(events[0].extra["source"], "approver");
    }
}

#[tokio::test]
async fn remembered_allowlist_hits_do_not_report_another_approval() {
    let fixture = Fixture::new();
    let (registry, requests) = fixture.registry(ApprovalDecision::AlwaysAllow);
    let store = fixture.root.join("always-allow.json");
    let registry = registry.with_always_allow_store(store.clone()).unwrap();
    for _ in 0..2 {
        registry
            .require_rememberable_approval("run fixture", "fixture-rule".to_owned())
            .await
            .unwrap();
    }
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(fixture.events().len(), 1);
    let rules: Vec<String> = serde_json::from_slice(&std::fs::read(store).unwrap()).unwrap();
    assert_eq!(rules, ["fixture-rule"]);
}

#[tokio::test]
async fn approval_is_reported_even_when_remembering_it_fails() {
    let fixture = Fixture::new();
    let (registry, requests) = fixture.registry(ApprovalDecision::AlwaysAllow);
    let parent = fixture.root.join("store-directory");
    let registry = registry
        .with_always_allow_store(parent.join("always-allow.json"))
        .unwrap();
    // Break the parent after loading, so the failure belongs to persistence.
    std::fs::rename(&parent, fixture.root.join("moved-store-directory")).unwrap();
    std::fs::write(&parent, "fixture").unwrap();
    for _ in 0..2 {
        let result = registry
            .require_rememberable_approval("run fixture", "fixture-rule".to_owned())
            .await;
        assert!(matches!(result, Err(ToolError::Io(_))));
        assert!(
            !registry
                .always_allowed
                .lock()
                .unwrap()
                .contains("fixture-rule")
        );
    }
    assert_eq!(requests.lock().unwrap().len(), 2);
    let events = fixture.events();
    assert_eq!(events.len(), 2);
    for event in events {
        assert_eq!(event.outcome.as_deref(), Some("always_allow"));
        assert_eq!(event.extra["source"], "approver");
    }
}

#[tokio::test]
async fn judge_allow_reports_its_final_decision_without_asking_the_approver() {
    let fixture = Fixture::new();
    let (registry, approvals) = fixture.registry(ApprovalDecision::Deny);
    registry.approval_mode_handle().set(ApprovalMode::Smart);
    let judged = Arc::new(Mutex::new(Vec::new()));
    let registry = registry.with_safety_judge(Arc::new(FixedJudge {
        verdict: JudgeVerdict::Allow,
        requests: judged.clone(),
    }));
    let command = "opaque_review_command API_KEY=judge-hook-fixture-value";
    registry
        .gate_main_command(command, "review fixture")
        .await
        .unwrap();
    assert_eq!(approvals.lock().unwrap().len(), 0);
    assert_eq!(judged.lock().unwrap().len(), 1);
    let events = fixture.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].tool.as_deref(), Some("run_command"));
    assert_eq!(events[0].outcome.as_deref(), Some("allow"));
    assert_eq!(events[0].extra["source"], "judge");
    assert_eq!(events[0].extra["mode"], "smart");
    assert_eq!(
        events[0].detail.as_deref(),
        Some("opaque_review_command API_KEY=[REDACTED]")
    );
}

#[tokio::test]
async fn judge_decline_unavailability_and_absence_report_only_the_final_approver() {
    for verdict in [
        Some(JudgeVerdict::Deny),
        Some(JudgeVerdict::Unavailable("fixture unavailable".to_owned())),
        None,
    ] {
        for decision in [
            ApprovalDecision::AllowOnce,
            ApprovalDecision::Deny,
            ApprovalDecision::AlwaysAllow,
        ] {
            let fixture = Fixture::new();
            let (mut registry, approvals) = fixture.registry(decision);
            registry.approval_mode_handle().set(ApprovalMode::Smart);
            let judged = Arc::new(Mutex::new(Vec::new()));
            if let Some(verdict) = &verdict {
                registry = registry.with_safety_judge(Arc::new(FixedJudge {
                    verdict: verdict.clone(),
                    requests: judged.clone(),
                }));
            }
            let result = registry
                .gate_main_command("opaque_review_command --fixture", "review fixture")
                .await;
            assert_eq!(result.is_ok(), decision != ApprovalDecision::Deny);
            assert_eq!(judged.lock().unwrap().len(), usize::from(verdict.is_some()));
            assert_eq!(approvals.lock().unwrap().len(), 1);
            let events = fixture.events();
            assert_eq!(events.len(), 1);
            assert_eq!(
                events[0].tool.as_deref(),
                Some("run command: review fixture")
            );
            assert_eq!(events[0].outcome.as_deref(), Some(outcome(decision)));
            assert_eq!(events[0].extra["source"], "approver");
        }
    }
}

#[tokio::test]
async fn approval_payload_has_session_workspace_and_redacted_action_details() {
    let fixture = Fixture::new();
    let (registry, _) = fixture.registry(ApprovalDecision::AllowOnce);
    registry
        .require_approval(
            "run command: ACCESS_TOKEN=approval-hook-fixture-value",
            false,
        )
        .await
        .unwrap();
    let events = fixture.events();
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(
        event.session_id.as_deref(),
        Some("approval-session-fixture")
    );
    assert_eq!(
        event.workspace.as_deref(),
        Some(registry.workspace.to_str().unwrap())
    );
    assert_eq!(
        event.tool.as_deref(),
        Some("run command: ACCESS_TOKEN=[REDACTED]")
    );
    assert_eq!(
        event.detail.as_deref(),
        Some("run command: ACCESS_TOKEN=[REDACTED]")
    );
    let raw = std::fs::read_to_string(&fixture.captured).unwrap();
    assert!(!raw.contains("approval-hook-fixture-value"));
}

#[tokio::test]
async fn noninteractive_denial_is_an_approver_result() {
    let fixture = Fixture::new();
    let registry = ToolRegistry::new(&fixture.root, ApprovalMode::Strict)
        .unwrap()
        .with_hooks(HookRegistry::new(vec![fixture.audit_hook()]));
    assert!(matches!(
        registry
            .require_approval("noninteractive fixture", false)
            .await,
        Err(ToolError::ApprovalDenied(_))
    ));
    let events = fixture.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome.as_deref(), Some("deny"));
    assert_eq!(events[0].extra["source"], "approver");
}

#[tokio::test]
async fn static_full_access_and_workspace_write_bypasses_have_no_approval_event() {
    let fixture = Fixture::new();
    let (registry, requests) = fixture.registry(ApprovalDecision::Deny);
    registry.approval_mode_handle().set(ApprovalMode::Smart);
    registry
        .gate_main_command("git status", "status")
        .await
        .unwrap();
    registry
        .require_approval("write fixture", true)
        .await
        .unwrap();
    registry
        .approval_mode_handle()
        .set(ApprovalMode::FullAccess);
    registry
        .gate_main_command("opaque_review_command --fixture", "fixture")
        .await
        .unwrap();
    assert!(requests.lock().unwrap().is_empty());
    assert!(fixture.events().is_empty());
}

#[tokio::test]
async fn observer_failure_and_timeout_do_not_change_any_approver_decision() {
    let timeout_command = if cfg!(windows) {
        "Start-Sleep -Milliseconds 1000"
    } else {
        "sleep 1"
    };
    for (command, timeout) in [
        ("exit 7", Duration::from_secs(5)),
        (timeout_command, Duration::from_millis(50)),
    ] {
        for decision in [
            ApprovalDecision::AllowOnce,
            ApprovalDecision::Deny,
            ApprovalDecision::AlwaysAllow,
        ] {
            let fixture = Fixture::new();
            let (registry, _) = fixture.registry(decision);
            let observer = Hook {
                name: "broken observer".to_owned(),
                command: command.to_owned(),
                timeout,
                ..fixture.audit_hook()
            };
            let registry =
                registry.with_hooks(HookRegistry::new(vec![fixture.audit_hook(), observer]));
            let result = registry.require_approval("fixture", false).await;
            assert_eq!(result.is_ok(), decision != ApprovalDecision::Deny);
            let events = fixture.events();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].outcome.as_deref(), Some(outcome(decision)));
        }
        let fixture = Fixture::new();
        let (registry, approvals) = fixture.registry(ApprovalDecision::Deny);
        registry.approval_mode_handle().set(ApprovalMode::Smart);
        let registry = registry
            .with_hooks(HookRegistry::new(vec![
                fixture.audit_hook(),
                Hook {
                    command: command.to_owned(),
                    timeout,
                    ..fixture.audit_hook()
                },
            ]))
            .with_safety_judge(Arc::new(FixedJudge {
                verdict: JudgeVerdict::Allow,
                requests: Arc::new(Mutex::new(Vec::new())),
            }));
        registry
            .gate_main_command("opaque_review_command --fixture", "fixture")
            .await
            .unwrap();
        assert!(approvals.lock().unwrap().is_empty());
        let events = fixture.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].outcome.as_deref(), Some("allow"));
        assert_eq!(events[0].extra["source"], "judge");
    }
}

#[tokio::test]
async fn questions_and_unsubscribed_approvals_do_not_emit_approval_events() {
    let fixture = Fixture::new();
    let (registry, requests) = fixture.registry(ApprovalDecision::AllowOnce);
    let question = ToolCall {
        id: "question".to_owned(),
        name: "ask_user".to_owned(),
        arguments: r#"{"question":"Choose a fixture?"}"#.to_owned(),
    };
    assert!(matches!(
        registry.execute(&question).await,
        Err(ToolError::ApprovalDenied(_))
    ));
    assert!(requests.lock().unwrap().is_empty());
    assert!(fixture.events().is_empty());

    let registry = registry.with_hooks(HookRegistry::new(vec![Hook {
        event: HookEvent::PreTool,
        ..fixture.audit_hook()
    }]));
    registry.require_approval("fixture", false).await.unwrap();
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert!(fixture.events().is_empty());
}

#[tokio::test]
async fn read_only_delegation_never_runs_the_host_approval_hook() {
    let fixture = Fixture::new();
    let (registry, _) = fixture.registry(ApprovalDecision::AllowOnce);
    let registry = registry
        .with_delegation_read_only()
        .with_hooks(HookRegistry::new(vec![fixture.audit_hook()]));
    assert!(registry.hooks.is_empty());
    registry.require_approval("fixture", false).await.unwrap();
    assert!(fixture.events().is_empty());
}
