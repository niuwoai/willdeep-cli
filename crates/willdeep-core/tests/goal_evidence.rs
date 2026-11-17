use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use willdeep_core::goal::{
    ContinuationDecision, GOAL_COMPLETE_MARKER, GoalBudget, GoalContinuation, GoalState,
    GoalStatus, RoundObservation, SoftStopReason,
};
use willdeep_core::{ApprovalMode, ToolCall, ToolRegistry};

struct Fixture {
    root: PathBuf,
    tools: ToolRegistry,
    goal: Arc<GoalContinuation>,
}

impl Fixture {
    fn new(script: &str) -> Self {
        let root = std::env::temp_dir().join(format!("goal-native-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("checks.rb"), script).unwrap();
        std::fs::write(root.join("source.rs"), "fn shipped() {}\n").unwrap();
        std::fs::write(root.join("other.rs"), "fn unrelated() {}\n").unwrap();
        let captured = root.clone();
        let tools = ToolRegistry::new(&root, ApprovalMode::FullAccess)
            .unwrap()
            .with_session_id(Some(uuid::Uuid::new_v4().to_string()))
            .with_fallible_verification_snapshot(move || {
                let mut hash = Sha256::new();
                // These are all files in this isolated synthetic workspace.
                for name in ["checks.rb", "source.rs", "other.rs"] {
                    hash.update(name.as_bytes());
                    let bytes =
                        std::fs::read(captured.join(name)).map_err(|error| error.to_string())?;
                    hash.update((bytes.len() as u64).to_le_bytes());
                    hash.update(bytes);
                }
                Ok(Some(format!("{:x}", hash.finalize())))
            });
        tools
            .require_verifications(&["ruby checks.rb".into()])
            .unwrap();
        let goal = Arc::new(GoalContinuation::new());
        goal.set_evidence_resolver(tools.goal_evidence_handle());
        goal.activate("ship the parser", GoalBudget::default());
        apply(&goal, json!({"criteria":["parser checks pass"]})).unwrap();
        Self { root, tools, goal }
    }

    async fn run(&self) -> (String, Value) {
        let output = self
            .tools
            .execute(&ToolCall {
                id: uuid::Uuid::new_v4().to_string(),
                name: "run_command".into(),
                arguments: json!({"command":"ruby checks.rb"}).to_string(),
            })
            .await
            .expect("even a nonzero process exit is an Ok tool result");
        let receipt = output
            .rsplit_once("\n[goal-evidence] ")
            .expect("runtime evidence receipt")
            .1;
        let receipt = serde_json::from_str(receipt).expect("structured evidence receipt");
        (output, receipt)
    }

    fn check(&self, receipt: &Value) -> Result<String, String> {
        apply(
            &self.goal,
            json!({"checklist":[{
                "index":1,"done":true,"evidence_ref":receipt["evidence_ref"],
                "evidence":"The runtime record contains the parser check result."
            }]}),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).expect("remove this test's isolated workspace");
    }
}

fn apply(goal: &GoalContinuation, update: Value) -> Result<String, String> {
    goal.update_plan(serde_json::from_value(update).unwrap())
}

#[tokio::test]
async fn real_foreground_execution_returns_a_scoped_record_and_can_complete() {
    let fixture = Fixture::new("puts 'parser passed'\n");
    let (output, record) = fixture.run().await;
    assert!(output.contains("exit_code: 0"));
    assert_eq!(record["source"], "command");
    assert_eq!(record["status"], "passed");
    assert_eq!(record["exit_code"], 0);
    assert_eq!(record["start_snapshot"], record["end_snapshot"]);
    assert_eq!(record["goal_id"], fixture.goal.snapshot().unwrap().goal_id);
    assert!(uuid::Uuid::parse_str(record["record_id"].as_str().unwrap()).is_ok());
    fixture.check(&record).unwrap();
    assert_eq!(fixture.goal.snapshot().unwrap().open_criteria().len(), 0);
    assert_eq!(
        fixture
            .goal
            .evaluate(GOAL_COMPLETE_MARKER, &RoundObservation::default()),
        Some(ContinuationDecision::Complete)
    );
    assert_eq!(
        fixture.goal.snapshot().unwrap().status,
        GoalStatus::Complete
    );
}

#[tokio::test]
async fn nonzero_exit_in_an_ok_tool_result_cannot_be_marked_done() {
    let fixture = Fixture::new("warn 'parser failed'\nexit 7\n");
    let (output, record) = fixture.run().await;
    assert!(output.contains("exit_code: 7"));
    assert_eq!(record["status"], "failed");
    let before = fixture.goal.snapshot().unwrap();
    assert!(fixture.check(&record).is_err());
    assert_eq!(fixture.goal.snapshot().unwrap(), before);
}

#[tokio::test]
async fn editing_after_checking_a_criterion_rejects_completion() {
    let fixture = Fixture::new("puts 'parser passed'\n");
    let (_, record) = fixture.run().await;
    fixture.check(&record).unwrap();
    std::fs::write(
        fixture.root.join("other.rs"),
        "fn changed_after_check() {}\n",
    )
    .unwrap();
    assert_eq!(fixture.goal.snapshot().unwrap().open_criteria().len(), 1);
    assert!(matches!(
        fixture
            .goal
            .evaluate(GOAL_COMPLETE_MARKER, &RoundObservation::default()),
        Some(ContinuationDecision::CompletionRejected {
            open_criteria: 1,
            ..
        })
    ));
    assert!(fixture.goal.is_active());
    assert!(fixture.check(&record).is_err());
}

#[tokio::test]
async fn code_changes_during_a_successful_command_make_its_record_invalid() {
    let fixture =
        Fixture::new("File.write('other.rs', 'changed during execution')\nputs 'passed'\n");
    let (_, record) = fixture.run().await;
    assert_eq!(record["exit_code"], 0);
    assert_eq!(record["status"], "invalidated");
    assert_ne!(record["start_snapshot"], record["end_snapshot"]);
    assert!(fixture.check(&record).is_err());
}

#[tokio::test]
async fn later_failure_revokes_a_previously_checked_success_on_the_same_snapshot() {
    let fixture = Fixture::new("puts 'parser passed'\n");
    let (_, success) = fixture.run().await;
    fixture.check(&success).unwrap();
    // Replace the script to fail, then restore all original source bytes. The
    // latest attempt for this exact command must still revoke the old success.
    let original = std::fs::read(fixture.root.join("checks.rb")).unwrap();
    std::fs::write(fixture.root.join("checks.rb"), "exit 7\n").unwrap();
    let (_, failure) = fixture.run().await;
    assert_eq!(failure["status"], "failed");
    std::fs::write(fixture.root.join("checks.rb"), original).unwrap();
    assert!(fixture.check(&success).is_err());
    assert!(matches!(
        fixture
            .goal
            .evaluate(GOAL_COMPLETE_MARKER, &RoundObservation::default()),
        Some(ContinuationDecision::CompletionRejected { .. })
    ));
}

#[tokio::test]
async fn timeout_returns_a_failed_receipt_and_revokes_old_success() {
    let fixture = Fixture::new("puts 'parser passed'\n");
    let (_, success) = fixture.run().await;
    fixture.check(&success).unwrap();
    let original = std::fs::read(fixture.root.join("checks.rb")).unwrap();
    std::fs::write(fixture.root.join("checks.rb"), "sleep 10\n").unwrap();
    let error = fixture
        .tools
        .execute(&ToolCall {
            id: "goal-timeout".into(),
            name: "run_command".into(),
            arguments: json!({"command":"ruby checks.rb","timeout_seconds":1}).to_string(),
        })
        .await
        .unwrap_err();
    let willdeep_core::tools::ToolError::CommandTimeout { output, .. } = error else {
        panic!("expected process timeout");
    };
    let record: Value =
        serde_json::from_str(output.rsplit_once("\n[goal-evidence] ").unwrap().1).unwrap();
    assert_eq!(record["status"], "timed_out");
    assert!(record["exit_code"].is_null());
    std::fs::write(fixture.root.join("checks.rb"), original).unwrap();
    assert!(fixture.check(&success).is_err());
    assert!(fixture.check(&record).is_err());
}

#[tokio::test]
async fn cancellation_of_a_real_attempt_revokes_old_success_without_a_receipt() {
    let fixture = Fixture::new("puts 'parser passed'\n");
    let (_, success) = fixture.run().await;
    fixture.check(&success).unwrap();
    let original = std::fs::read(fixture.root.join("checks.rb")).unwrap();
    std::fs::write(fixture.root.join("checks.rb"), "sleep 10\n").unwrap();
    let call = ToolCall {
        id: "goal-cancel".into(),
        name: "run_command".into(),
        arguments: json!({"command":"ruby checks.rb"}).to_string(),
    };
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(150),
            fixture.tools.execute(&call)
        )
        .await
        .is_err()
    );
    std::fs::write(fixture.root.join("checks.rb"), original).unwrap();
    assert!(fixture.check(&success).is_err());
    assert_eq!(fixture.goal.snapshot().unwrap().open_criteria().len(), 1);
}

#[tokio::test]
async fn stale_checklist_entries_can_be_reverified_one_at_a_time() {
    let fixture = Fixture::new("puts 'parser passed'\n");
    apply(
        &fixture.goal,
        json!({"criteria":["parser checks pass","second check"]}),
    )
    .unwrap();
    let (_, old) = fixture.run().await;
    apply(
        &fixture.goal,
        json!({"checklist":[
            {"index":1,"done":true,"evidence_ref":old["evidence_ref"]},
            {"index":2,"done":true,"evidence_ref":old["evidence_ref"]},
        ]}),
    )
    .unwrap();
    std::fs::write(fixture.root.join("source.rs"), "fn reverified() {}\n").unwrap();
    let (_, fresh) = fixture.run().await;
    fixture.check(&fresh).unwrap();
    let state = fixture.goal.snapshot().unwrap();
    assert!(state.criteria[0].done && state.criteria[0].evidence_verified);
    assert_eq!(state.open_criteria()[0].0, 2);
    assert_eq!(state.open_criteria().len(), 1);
    assert!(matches!(
        fixture
            .goal
            .evaluate(GOAL_COMPLETE_MARKER, &RoundObservation::default()),
        Some(ContinuationDecision::CompletionRejected {
            open_criteria: 1,
            ..
        })
    ));
    apply(
        &fixture.goal,
        json!({"checklist":[
            {"index":2,"done":true,"evidence_ref":fresh["evidence_ref"]},
        ]}),
    )
    .unwrap();
    assert_eq!(
        fixture
            .goal
            .evaluate(GOAL_COMPLETE_MARKER, &RoundObservation::default()),
        Some(ContinuationDecision::Complete)
    );
}

#[tokio::test]
async fn invalid_whole_checklist_cannot_change_plan_or_budget() {
    let fixture = Fixture::new("puts 'parser passed'\n");
    let (_, record) = fixture.run().await;
    let before = fixture.goal.snapshot().unwrap();
    let error = apply(
        &fixture.goal,
        json!({
            "criteria":["parser checks pass","another criterion"],
            "checklist":[
                {"index":1,"done":true,"evidence_ref":record["evidence_ref"]},
                {"index":2,"done":true,"evidence_ref":"goal-evidence:forged","evidence":"trust me"}
            ],
            "steps":[{"title":"must not be installed","status":"done"}],
            "blocked_reason":"must not be installed"
        }),
    )
    .unwrap_err();
    assert!(error.contains("unknown evidence"));
    let after = fixture.goal.snapshot().unwrap();
    assert_eq!(after, before);
    assert_eq!(after.continuations, 0);
    assert_eq!(after.tokens_used, 0);
}

#[tokio::test]
async fn new_goal_with_the_same_statement_and_other_session_reject_old_refs() {
    let fixture = Fixture::new("puts 'parser passed'\n");
    let (_, record) = fixture.run().await;
    let old_id = fixture.goal.snapshot().unwrap().goal_id;
    fixture.goal.clear();
    fixture
        .goal
        .activate("ship the parser", GoalBudget::default());
    assert_ne!(old_id, fixture.goal.snapshot().unwrap().goal_id);
    apply(&fixture.goal, json!({"criteria":["parser checks pass"]})).unwrap();
    assert!(fixture.check(&record).is_err());
    let another = Fixture::new("puts 'parser passed'\n");
    assert!(another.check(&record).is_err());
}

#[tokio::test]
async fn restored_references_without_the_live_store_are_open_and_budget_is_preserved() {
    let fixture = Fixture::new("puts 'parser passed'\n");
    let (_, record) = fixture.run().await;
    fixture.check(&record).unwrap();
    fixture.goal.record_tokens(19);
    let saved = fixture.goal.snapshot().unwrap();
    let decoded: GoalState = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
    assert!(!decoded.criteria[0].evidence_verified);
    let restored = GoalContinuation::new();
    restored.restore(decoded, GoalBudget::default());
    let state = restored.snapshot().unwrap();
    assert_eq!(state.goal_id, saved.goal_id);
    assert_eq!(state.tokens_used, 19);
    assert_eq!(state.continuations, saved.continuations);
    assert_eq!(state.open_criteria().len(), 1);
    assert!(matches!(
        restored.evaluate(GOAL_COMPLETE_MARKER, &RoundObservation::default()),
        Some(ContinuationDecision::CompletionRejected { .. })
    ));
}

#[test]
fn legacy_text_and_absent_resolver_fail_closed_without_spending_budget() {
    let goal = GoalContinuation::new();
    let state: GoalState = serde_json::from_value(json!({
        "statement":"ship parser","criteria":[{"text":"passed","done":true,"evidence":"cargo test: passed"}],
        "continuations":3,"tokens_used":7
    })).unwrap();
    goal.restore(
        state,
        GoalBudget {
            wall_clock: None,
            max_continuations: 3,
            max_tokens: None,
        },
    );
    let before = goal.snapshot().unwrap();
    assert!(!before.goal_id.is_empty());
    assert_eq!(before.open_criteria().len(), 1);
    assert!(
        apply(
            &goal,
            json!({"checklist":[{"index":1,"done":true,"evidence":"human checked"}]})
        )
        .is_err()
    );
    assert!(
        apply(
            &goal,
            json!({"checklist":[{"index":1,"done":true,"evidence_ref":"unknown"}]})
        )
        .is_err()
    );
    assert_eq!(goal.snapshot().unwrap(), before);
    assert!(matches!(
        goal.evaluate(GOAL_COMPLETE_MARKER, &RoundObservation::default()),
        Some(ContinuationDecision::SoftStop {
            reason: SoftStopReason::Continuations,
            ..
        })
    ));
    assert_eq!(
        goal.evaluate("handover", &RoundObservation::default()),
        Some(ContinuationDecision::Complete)
    );
    assert_eq!(goal.snapshot().unwrap().status, GoalStatus::BudgetLimited);
}

#[test]
fn human_observations_require_the_host_api_and_cannot_be_declared_in_model_input() {
    let fixture = Fixture::new("puts 'passed'\n");
    let id = fixture.goal.snapshot().unwrap().goal_id;
    let record = fixture
        .tools
        .goal_evidence_handle()
        .record_human_observation(&id, "host inspected parser UI")
        .unwrap();
    apply(
        &fixture.goal,
        json!({"checklist":[{"index":1,"done":true,"evidence_ref":record.reference}]}),
    )
    .unwrap();
    assert!(
        serde_json::from_value::<willdeep_core::goal::PlanUpdate>(json!({
            "checklist":[{"index":1,"done":true,"source":"human","evidence":"I am human"}]
        }))
        .is_err()
    );
}
