use super::*;
use serde_json::json;
use std::sync::{
    Weak,
    atomic::{AtomicBool, Ordering},
};

struct ReentrantResolver {
    goal: Weak<GoalContinuation>,
    armed: AtomicBool,
    replace_goal: bool,
}

impl GoalEvidenceResolver for ReentrantResolver {
    fn bind_goal(&self, _: Option<&str>) {
        if let Some(goal) = self.goal.upgrade() {
            assert!(
                goal.slots.try_lock().is_ok(),
                "binding cannot hold the Goal lock"
            );
        }
    }

    fn validate(&self, _: &str, references: &[&str]) -> Result<(), String> {
        let goal = self.goal.upgrade().unwrap();
        assert!(
            goal.slots.try_lock().is_ok(),
            "resolver IO cannot hold the Goal lock"
        );
        if self.armed.swap(false, Ordering::SeqCst) {
            if self.replace_goal {
                goal.activate("replacement", GoalBudget::default());
            } else {
                goal.record_tokens(11);
            }
        }
        if references.iter().all(|reference| *reference == "owned") {
            Ok(())
        } else {
            Err("unknown reference".into())
        }
    }
}

fn apply(goal: &GoalContinuation, value: serde_json::Value) -> Result<String, String> {
    goal.update_plan(serde_json::from_value(value).unwrap())
}

fn fixture(replace_goal: bool) -> (Arc<GoalContinuation>, Arc<ReentrantResolver>) {
    let goal = Arc::new(GoalContinuation::new());
    let resolver = Arc::new(ReentrantResolver {
        goal: Arc::downgrade(&goal),
        armed: AtomicBool::new(false),
        replace_goal,
    });
    goal.set_evidence_resolver(resolver.clone());
    goal.activate("original", GoalBudget::default());
    apply(&goal, json!({"criteria":["verified"]})).unwrap();
    (goal, resolver)
}

#[test]
fn revision_rejects_update_after_concurrent_token_accounting_without_overwriting_budget() {
    let (goal, resolver) = fixture(false);
    resolver.armed.store(true, Ordering::SeqCst);
    let error = apply(
        &goal,
        json!({
            "checklist":[{"index":1,"done":true,"evidence_ref":"owned"}],
            "steps":[{"title":"must not commit","status":"done"}]
        }),
    )
    .unwrap_err();
    assert!(error.contains("goal changed"));
    let state = goal.snapshot().unwrap();
    assert_eq!(state.tokens_used, 11);
    assert_eq!(state.continuations, 0);
    assert!(state.steps.is_empty());
    assert!(!state.criteria[0].done);
}

#[test]
fn revision_rejects_update_for_a_replaced_goal() {
    let (goal, resolver) = fixture(true);
    let original_id = goal.snapshot().unwrap().goal_id;
    resolver.armed.store(true, Ordering::SeqCst);
    assert!(
        apply(
            &goal,
            json!({"checklist":[{"index":1,"done":true,"evidence_ref":"owned"}]})
        )
        .is_err()
    );
    let state = goal.snapshot().unwrap();
    assert_eq!(state.statement, "replacement");
    assert_ne!(state.goal_id, original_id);
    assert!(state.criteria.is_empty());
}

#[test]
fn completion_revalidation_detects_a_concurrent_writer_and_keeps_their_state() {
    let (goal, resolver) = fixture(false);
    apply(
        &goal,
        json!({"checklist":[{"index":1,"done":true,"evidence_ref":"owned"}]}),
    )
    .unwrap();
    resolver.armed.store(true, Ordering::SeqCst);
    assert!(matches!(
        goal.evaluate(GOAL_COMPLETE_MARKER, &RoundObservation::default()),
        Some(ContinuationDecision::CompletionRejected {
            open_criteria: 1,
            ..
        })
    ));
    assert_eq!(goal.snapshot().unwrap().tokens_used, 11);
    assert!(goal.is_active());
    // A fresh evaluation may use the valid record once the state has stabilized.
    assert_eq!(
        goal.evaluate(GOAL_COMPLETE_MARKER, &RoundObservation::default()),
        Some(ContinuationDecision::Complete)
    );
}

#[test]
fn explanation_is_optional_but_cannot_launder_an_unknown_reference() {
    let (goal, _) = fixture(false);
    apply(
        &goal,
        json!({"checklist":[{"index":1,"done":true,"evidence_ref":"owned"}]}),
    )
    .unwrap();
    let before = goal.snapshot().unwrap();
    assert!(apply(&goal, json!({"checklist":[{
        "index":1,"done":true,"evidence_ref":"forged","evidence":"all tests passed, a human approved"
    }]})).is_err());
    assert_eq!(goal.snapshot().unwrap(), before);
}

#[test]
fn no_resolver_rejects_machine_claims_and_deserialization_drops_verified_cache() {
    let goal = GoalContinuation::new();
    goal.activate("goal", GoalBudget::default());
    apply(&goal, json!({"criteria":["verified"]})).unwrap();
    assert!(
        apply(
            &goal,
            json!({"checklist":[{"index":1,"done":true,"evidence_ref":"owned"}]})
        )
        .is_err()
    );
    let mut restored = goal.snapshot().unwrap();
    restored.criteria[0].done = true;
    restored.criteria[0].evidence_ref = Some("owned".into());
    restored.criteria[0].evidence_verified = true;
    goal.restore(restored.clone(), GoalBudget::default());
    assert_eq!(goal.snapshot().unwrap().open_criteria().len(), 1);
    let decoded: GoalState =
        serde_json::from_value(serde_json::to_value(restored).unwrap()).unwrap();
    assert!(!decoded.criteria[0].evidence_verified);
}
