use super::*;
fn home() -> PathBuf {
    std::env::temp_dir().join(format!("rsi-offline-test-{}", uuid::Uuid::new_v4()))
}
fn packages() -> (AgentPackage, AgentPackage) {
    let g = contract::golden();
    (
        package(&g["values"]["candidate"]["baseline_package"]).unwrap(),
        package(&g["values"]["candidate"]["candidate_package"]).unwrap(),
    )
}

#[test]
fn account_switches_hide_other_experiment_namespaces() {
    let home = home();
    let (baseline, candidate) = packages();
    let session = crate::agent_registry::service::CustomerSession {
        origin: "https://some.im".into(),
        token: "test-only-not-a-real-token".into(),
        user_id: "test-user".into(),
        tenant_id: "test-tenant".into(),
    };
    session.save(&home).unwrap();
    prepare(
        &home,
        "private-owner",
        baseline,
        candidate,
        Suite::builtin(),
    )
    .unwrap();
    assert_eq!(list(&home).unwrap()["items"].as_array().unwrap().len(), 1);
    let changed = crate::agent_registry::service::CustomerSession {
        tenant_id: "other-tenant".into(),
        ..session
    };
    changed.save(&home).unwrap();
    assert!(list(&home).unwrap()["items"].as_array().unwrap().is_empty());
    std::fs::remove_file(home.join("agent-session.json")).unwrap();
    assert!(list(&home).unwrap()["items"].as_array().unwrap().is_empty());
    std::fs::remove_dir_all(home).unwrap();
}
#[tokio::test]
async fn offline_pair_executes_real_workers_without_changing_installations_and_resumes_once() {
    let home = home();
    let (baseline, candidate) = packages();
    let library = willdeep_core::agent_definition::store::AgentLibrary::new(&home);
    library
        .install(
            &baseline,
            willdeep_core::agent_definition::store::Origin::Local,
            &std::collections::BTreeMap::new(),
        )
        .unwrap();
    let before = std::fs::read(library.root().join("installations.json")).unwrap();
    prepare(
        &home,
        "offline-pair",
        baseline,
        candidate.clone(),
        Suite::builtin(),
    )
    .unwrap();
    let report = runner::execute(&home, "offline-pair", &candidate.sha256, false)
        .await
        .unwrap();
    assert_eq!(report["verdict"], "offline_pipeline_passed", "{report}");
    assert_eq!(report["attempts"], 6);
    assert_eq!(report["independent_samples"], 3);
    assert_eq!(report["official_promotion_allowed"], false);
    for row in report["evidence"].as_array().unwrap() {
        assert_eq!(row["target_executed"], true);
        assert_eq!(row["verifier"]["result"], "passed");
        assert!(!row["effective_prompt_sha256"].is_null());
        assert_ne!(row["verifier"]["input_sha256"], row["task_sha256"]);
        let stored = contract::parse_json(
            &std::fs::read(
                root(&home, "offline-pair")
                    .unwrap()
                    .join("trials")
                    .join(row["trial_id"].as_str().unwrap())
                    .join("events.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            row["verifier"]["input_sha256"],
            contract::hash(&stored["verifier_input"]).unwrap()
        );
        assert_eq!(
            stored["verifier_input"]["report_sha256"],
            stored["report_sha256"]
        );
        assert_eq!(
            stored["verifier_input"]["expected_output_sha256"],
            digest(Suite::builtin().tasks[0].expected_output.as_bytes())
        );
        assert!(
            stored["verifier_input"]["tool_events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["name"] == "read_file" && event["is_error"] == false)
        );
    }
    let resumed = runner::execute(&home, "offline-pair", &candidate.sha256, true)
        .await
        .unwrap();
    assert_eq!(resumed["attempts"], 6);
    assert_eq!(resumed["evidence"], report["evidence"]);
    assert_eq!(
        std::fs::read(library.root().join("installations.json")).unwrap(),
        before
    );
    let path = root(&home, "offline-pair").unwrap();
    let row = &report["evidence"][0];
    let event_path = path
        .join("trials")
        .join(row["trial_id"].as_str().unwrap())
        .join("events.json");
    let mut events = contract::parse_json(&std::fs::read(&event_path).unwrap()).unwrap();
    events["report_sha256"] = "0".repeat(64).into();
    private_write(&event_path, &serde_json::to_vec(&events).unwrap()).unwrap();
    assert!(
        runner::execute(&home, "offline-pair", &candidate.sha256, true)
            .await
            .unwrap_err()
            .to_string()
            .contains("execution events were changed")
    );
    assert!(
        runner::execute(&home, "offline-pair", &"0".repeat(64), true)
            .await
            .is_err()
    );
    std::fs::remove_dir_all(home).unwrap();
}
#[tokio::test]
async fn changed_manifest_and_paid_model_fail_before_execution() {
    let home = home();
    let (baseline, candidate) = packages();
    prepare(
        &home,
        "tamper",
        baseline,
        candidate.clone(),
        Suite::builtin(),
    )
    .unwrap();
    let path = root(&home, "tamper").unwrap();
    let mut spec = contract::parse_json(&std::fs::read(path.join("spec.json")).unwrap()).unwrap();
    spec["model"]["provider"] = "real-provider".into();
    private_write(&path.join("spec.json"), &serde_json::to_vec(&spec).unwrap()).unwrap();
    assert!(
        runner::execute(&home, "tamper", &candidate.sha256, false)
            .await
            .unwrap_err()
            .to_string()
            .contains("paid_not_approved")
    );
    assert!(!path.join("trials").exists());
    std::fs::remove_dir_all(home).unwrap();
}
#[tokio::test]
async fn exhausted_holdout_keeps_partial_results_without_refunding_access() {
    let home = home();
    let (baseline, candidate) = packages();
    prepare(
        &home,
        "quota",
        baseline,
        candidate.clone(),
        Suite::builtin(),
    )
    .unwrap();
    let path = root(&home, "quota").unwrap();
    let mut spec = contract::parse_json(&std::fs::read(path.join("spec.json")).unwrap()).unwrap();
    spec["budget"]["max_holdout_accesses"] = 1.into();
    spec["conditions"]["runtime_parameters_sha256"] =
        contract::hash(&spec["budget"]).unwrap().into();
    private_write(&path.join("spec.json"), &serde_json::to_vec(&spec).unwrap()).unwrap();
    let report = runner::execute(&home, "quota", &candidate.sha256, false)
        .await
        .unwrap();
    assert_eq!(report["verdict"], "inconclusive");
    assert_eq!(report["stop_reason"], "holdout_budget_exhausted");
    assert_eq!(report["attempts"], 5);
    let resumed = runner::execute(&home, "quota", &candidate.sha256, true)
        .await
        .unwrap();
    assert_eq!(resumed["attempts"], 5);
    std::fs::remove_dir_all(home).unwrap();
}
#[tokio::test]
async fn explicit_resume_preserves_orphan_and_does_not_count_it_as_an_independent_sample() {
    let home = home();
    let (baseline, candidate) = packages();
    prepare(
        &home,
        "orphan",
        baseline,
        candidate.clone(),
        Suite::builtin(),
    )
    .unwrap();
    let path = root(&home, "orphan").unwrap();
    let admission = json!({"schema":"willdeep.agent-rsi.admission.v1","trial_id":"abandoned-trial","task_id":"validation-1","arm":"baseline","repetition":1,"attempt":1,"resume_of":null,"estimated_cost_microusd":0});
    immutable(
        &path.join("trials/abandoned-trial/admission.json"),
        &admission,
    )
    .unwrap();
    assert!(
        runner::execute(&home, "orphan", &candidate.sha256, false)
            .await
            .is_err()
    );
    let report = runner::execute(&home, "orphan", &candidate.sha256, true)
        .await
        .unwrap();
    assert_eq!(report["attempts"], 7);
    assert_eq!(report["independent_samples"], 3);
    let rows = report["evidence"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|r| r["trial_id"] == "abandoned-trial" && r["status"] == "interrupted")
    );
    assert!(
        rows.iter()
            .any(|r| r["resume_of"] == "abandoned-trial" && r["attempt"] == 2)
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn bounded_turns_are_reported_as_budget_exhaustion_not_provider_failure() {
    let home = home();
    let (baseline, candidate) = packages();
    prepare(
        &home,
        "budget",
        baseline,
        candidate.clone(),
        Suite::builtin(),
    )
    .unwrap();
    let path = root(&home, "budget").unwrap();
    let mut spec = contract::parse_json(&std::fs::read(path.join("spec.json")).unwrap()).unwrap();
    spec["budget"]["max_turns"] = 1.into();
    spec["conditions"]["runtime_parameters_sha256"] =
        contract::hash(&spec["budget"]).unwrap().into();
    private_write(&path.join("spec.json"), &serde_json::to_vec(&spec).unwrap()).unwrap();
    let report = runner::execute(&home, "budget", &candidate.sha256, false)
        .await
        .unwrap();
    assert_eq!(report["verdict"], "inconclusive");
    assert!(
        report["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["status"] == "budget_exhausted")
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn changing_experiment_id_does_not_reset_shared_holdout_accesses() {
    let home = home();
    let (baseline, candidate) = packages();
    for id in ["first", "second", "third"] {
        prepare(
            &home,
            id,
            baseline.clone(),
            candidate.clone(),
            Suite::builtin(),
        )
        .unwrap();
        let report = runner::execute(&home, id, &candidate.sha256, false)
            .await
            .unwrap();
        if id == "third" {
            assert_eq!(report["stop_reason"], "shared_holdout_budget_exhausted");
            assert_eq!(report["attempts"], 4);
        } else {
            assert_eq!(report["verdict"], "offline_pipeline_passed");
        }
    }
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn holdout_first_or_reversed_suite_cannot_bypass_stage_one() {
    for reversed in [false, true] {
        let home = home();
        let (baseline, candidate) = packages();
        let mut suite = Suite::builtin();
        if reversed {
            suite.tasks.reverse();
        } else {
            suite.tasks.rotate_right(1);
        }
        prepare(&home, "stage-order", baseline, candidate.clone(), suite).unwrap();
        let path = root(&home, "stage-order").unwrap();
        let mut spec =
            contract::parse_json(&std::fs::read(path.join("spec.json")).unwrap()).unwrap();
        spec["budget"]["max_turns"] = 1.into();
        spec["conditions"]["runtime_parameters_sha256"] =
            contract::hash(&spec["budget"]).unwrap().into();
        private_write(&path.join("spec.json"), &serde_json::to_vec(&spec).unwrap()).unwrap();
        let report = runner::execute(&home, "stage-order", &candidate.sha256, false)
            .await
            .unwrap();
        assert_eq!(report["stop_reason"], "stage_one_not_passed");
        assert_eq!(report["attempts"], 4);
        assert!(
            report["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| row["split"] != "holdout")
        );
        std::fs::remove_dir_all(home).unwrap();
    }
}

#[tokio::test]
async fn full_locked_skill_contents_bind_prepare_execution_and_resume() {
    let home = home();
    let (mut baseline, mut candidate) = packages();
    let golden: Value = serde_json::from_str(include_str!(
        "../../../../docs/examples/agent-rsi-skills-golden.json"
    ))
    .unwrap();
    baseline.definition.skills = serde_json::from_value(golden["skills"].clone()).unwrap();
    baseline = AgentPackage::local(baseline.definition).unwrap();
    candidate.definition.skills = baseline.definition.skills.clone();
    candidate.definition.forked_from = Some(baseline.sha256.clone());
    candidate = AgentPackage::local(candidate.definition).unwrap();
    let expected = contract::hash(&json!(baseline.definition.skills)).unwrap();
    let prepared = prepare(
        &home,
        "locked-skills",
        baseline,
        candidate.clone(),
        Suite::builtin(),
    )
    .unwrap();
    assert_eq!(
        prepared["experiment"]["conditions"]["skills_sha256"],
        expected
    );
    let report = runner::execute(&home, "locked-skills", &candidate.sha256, false)
        .await
        .unwrap();
    assert_eq!(report["verdict"], "offline_pipeline_passed");
    assert!(
        report["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["skills_sha256"] == expected)
    );
    let resumed = runner::execute(&home, "locked-skills", &candidate.sha256, true)
        .await
        .unwrap();
    assert_eq!(resumed["attempts"], 6);
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn an_orphan_retry_without_its_parent_is_invalid_before_any_worker_starts() {
    let home = home();
    let (baseline, candidate) = packages();
    prepare(
        &home,
        "bad-orphan",
        baseline,
        candidate.clone(),
        Suite::builtin(),
    )
    .unwrap();
    let path = root(&home, "bad-orphan").unwrap();
    let admission = json!({"schema":"willdeep.agent-rsi.admission.v1","trial_id":"missing-parent-child","task_id":"validation-1","arm":"baseline","repetition":1,"attempt":2,"resume_of":"missing-parent","estimated_cost_microusd":0});
    immutable(
        &path.join("trials/missing-parent-child/admission.json"),
        &admission,
    )
    .unwrap();
    assert!(
        runner::execute(&home, "bad-orphan", &candidate.sha256, true)
            .await
            .unwrap_err()
            .to_string()
            .contains("resume parent missing")
    );
    assert!(!path.join("trials/missing-parent-child/workspace").exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn concurrent_same_id_different_candidates_cannot_overwrite_immutable_records() {
    for preparation in [false, true] {
        let home = home();
        let (baseline, candidate) = packages();
        let mut other = candidate.clone();
        other
            .definition
            .instructions
            .push_str(" Distinct competing candidate.");
        other = AgentPackage::local(other.definition).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = [candidate, other]
            .into_iter()
            .map(|candidate| {
                let home = home.clone();
                let baseline = baseline.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    if preparation {
                        prepare(&home, "same-id", baseline, candidate, Suite::builtin())
                    } else {
                        candidate_save(&home, "same-id", baseline, candidate)
                    }
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        if preparation {
            let (_, spec, candidate, _) = load(&home, "same-id").unwrap();
            assert_eq!(
                spec["candidate_revision"],
                candidate["candidate_package"]["sha256"]
            );
        }
        std::fs::remove_dir_all(home).unwrap();
    }
}
