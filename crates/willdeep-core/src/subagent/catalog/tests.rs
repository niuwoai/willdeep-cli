use std::time::Duration;

use async_trait::async_trait;

use super::*;
use crate::provider::ProviderError;
use crate::subagent::builtin_profiles;
use crate::subagent::test_support::{
    AllowingJudge, CaptureSink, ModelProvider, ReportProvider, fixture,
};
use crate::subagent::types::TaskPacket;
use crate::types::{Completion, Message, ToolDefinition};

struct EditingThenWaitingProvider(std::sync::atomic::AtomicUsize);
#[async_trait]
impl Provider for EditingThenWaitingProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
            std::future::pending::<()>().await;
        }
        Ok(Completion {
            reasoning: None,
            content: "editing".into(),
            tool_calls: vec![crate::types::ToolCall {
                id: "edit".into(),
                name: "edit_file".into(),
                arguments:
                    serde_json::json!({"path":"fix.rs","old_string":"before","new_string":"after"})
                        .to_string(),
            }],
            finish_reason: Some("tool_calls".into()),
            usage: None,
        })
    }
}

#[tokio::test]
async fn cancelling_dedicated_worker_retains_patch_and_cancelled_status() {
    let root = std::env::temp_dir().join(format!("cancel-handoff-{}", uuid::Uuid::new_v4()));
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("fix.rs"), "before").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "fix.rs"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@invalid",
            "commit",
            "-qm",
            "base",
        ],
    ] {
        assert!(
            tokio::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .await
                .unwrap()
                .success()
        );
    }
    let background = Arc::new(BackgroundTaskRegistry::default());
    let mut profiles = builtin_profiles(Arc::new(EditingThenWaitingProvider(0.into())));
    profiles
        .iter_mut()
        .find(|p| p.id == "generalist")
        .unwrap()
        .worktree = crate::subagent_worktree::SubagentWorktreePolicy::Dedicated;
    let catalog = SubagentCatalog::new(&repo, profiles, background.clone())
        .with_worktree_root(root.join("managed"))
        .with_parent_approval_mode(crate::tools::SharedApprovalMode::new(
            crate::ApprovalMode::FullAccess,
        ));
    catalog
        .run(
            SpawnAgentArgs {
                prompt: "change before to after".into(),
                profile: Some("generalist".into()),
                run_in_background: Some(true),
                task: Some(TaskPacket {
                    goal: "change before to after".into(),
                    write_files: vec!["fix.rs".into()],
                    ..TaskPacket::default()
                }),
                ..SpawnAgentArgs::default()
            },
            Some(BTreeSet::from([repo
                .join("fix.rs")
                .canonicalize()
                .unwrap()])),
        )
        .await
        .unwrap();
    let agent = *catalog
        .owned_background_agents
        .lock()
        .unwrap()
        .iter()
        .next()
        .unwrap();
    let child = root.join("managed").join(agent.to_string());
    tokio::time::timeout(Duration::from_secs(5), async {
        while std::fs::read_to_string(child.join("fix.rs"))
            .ok()
            .as_deref()
            != Some("after")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("worker made actual edit");
    let id = background.snapshots()[0].id.clone();
    assert!(background.kill(&id));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !background
            .full_output(&id)
            .is_some_and(|s| s.contains("Tracked patch:"))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled handoff delivered");
    assert_eq!(
        background.snapshots()[0].status,
        BackgroundTaskStatus::Killed
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("fix.rs")).unwrap(),
        "before"
    );
    assert_eq!(
        std::fs::read_to_string(child.join("fix.rs")).unwrap(),
        "after"
    );
    assert!(
        background
            .full_output(&id)
            .unwrap()
            .contains("Integration: pending")
    );
    std::fs::remove_dir_all(root).unwrap();
}

struct WaitingProvider;
#[async_trait]
impl Provider for WaitingProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn parent_deadline_bounds_worker_provider_wait() {
    let root = std::env::temp_dir().join(format!("worker-deadline-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let catalog = SubagentCatalog::new(
        &root,
        builtin_profiles(Arc::new(WaitingProvider)),
        Arc::new(BackgroundTaskRegistry::default()),
    );
    catalog.set_execution_deadline(Some(
        tokio::time::Instant::now() + Duration::from_millis(50),
    ));
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        catalog.run(
            SpawnAgentArgs {
                prompt: "inspect".into(),
                profile: Some("scout".into()),
                run_in_background: Some(false),
                ..SpawnAgentArgs::default()
            },
            None,
        ),
    )
    .await
    .expect("parent deadline bounds worker");
    match result.unwrap_err() {
        AgentError::Subagent(message) => {
            assert!(message.contains("Parent integration deadline"), "{message}")
        }
        AgentError::SubagentPartial { reason, .. } => {
            assert_eq!(reason, crate::agent::AgentStopReason::BudgetLimited)
        }
        other => panic!("worker stopped for a different reason: {other}"),
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn failed_workers_retain_workspace_handoff_without_becoming_success() {
    let root = std::env::temp_dir().join(format!("worker-handoff-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    assert!(
        tokio::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .await
            .unwrap()
            .success()
    );
    std::fs::write(root.join("fix.rs"), "pending fix").unwrap();
    let prepared = PreparedSubagentWorkspace {
        workspace: root.clone(),
        root_workspace: root.clone(),
        branch: Some("worker".into()),
        dedicated: true,
    };
    let timeout = attach_worktree_report(Err(AgentError::Subagent("timed out".into())), &prepared)
        .await
        .unwrap_err();
    assert!(timeout.to_string().contains("timed out"));
    assert!(timeout.to_string().contains("fix.rs"));
    let partial = attach_worktree_report(
        Err(AgentError::SubagentPartial {
            reason: crate::agent::AgentStopReason::MaxTurns,
            turns: 2,
            report: "partial implementation".into(),
        }),
        &prepared,
    )
    .await
    .unwrap_err();
    match partial {
        AgentError::SubagentPartial {
            reason,
            turns,
            report,
        } => {
            assert_eq!(reason, crate::agent::AgentStopReason::MaxTurns);
            assert_eq!(turns, 2);
            assert!(report.contains("partial implementation"));
            assert!(report.contains("fix.rs"));
        }
        other => panic!("lost partial result: {other}"),
    }
    std::fs::remove_dir_all(root).unwrap();
}

/// 文件写集合冲突由认领控制，独立于并发槽位。
#[test]
fn file_claims_guard_writes_independently_of_admission() {
    let claimed = Arc::new(Mutex::new(BTreeSet::new()));
    let files: BTreeSet<PathBuf> = [PathBuf::from("src/lib.rs")].into_iter().collect();
    let first =
        super::super::runner::acquire_file_claim_for_test(&claimed, &files).expect("first claim");
    assert!(first.is_some());
    assert!(
        super::super::runner::acquire_file_claim_for_test(&claimed, &files).is_err(),
        "同一个文件的第二个认领必须被拒，哪怕并发数还没到上限"
    );
}

/// 写集与正在跑的 Worker 冲突：派工当场被拒，点名冲突文件，不调模型、
/// 也不起一个注定失败的后台任务。
#[tokio::test]
async fn overlapping_write_sets_are_refused_at_dispatch() {
    let root = std::env::temp_dir().join(format!("willdeep-overlap-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(super::super::test_support::ModelProvider {
        model: "local-test".into(),
        seen: seen.clone(),
    });
    let background = Arc::new(BackgroundTaskRegistry::default());
    let mut profiles = crate::subagent::builtin_profiles(provider);
    profiles
        .iter_mut()
        .find(|profile| profile.id == "implementer")
        .unwrap()
        .worktree = crate::subagent_worktree::SubagentWorktreePolicy::Shared;
    let catalog = SubagentCatalog::new(&root, profiles, background.clone());
    let target = root.join("code.rs");
    catalog.claimed_files.lock().unwrap().insert(target.clone());
    let mut events = background.subscribe();
    catalog
        .run(
            SpawnAgentArgs {
                profile: Some("implementer".into()),
                prompt: "edit code.rs".into(),
                run_in_background: Some(true),
                task: Some(crate::subagent::TaskPacket {
                    goal: "edit code.rs".into(),
                    verifier: Some(crate::subagent::TaskVerifier {
                        command: "true".into(),
                        expected_exit_code: None,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Some(BTreeSet::from([target.clone()])),
        )
        .await
        .unwrap();
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.snapshot.status, BackgroundTaskStatus::Blocked);
    let message = event.notice;
    assert!(message.contains("already claimed"), "{message}");
    assert!(message.contains("code.rs"), "{message}");
    assert!(seen.lock().unwrap().is_empty(), "no model call");
    assert_eq!(background.snapshots().len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_open_circuit_allows_one_trial_after_the_cooldown() {
    let failures = Mutex::new(BTreeMap::new());
    let cooldown = std::time::Duration::from_secs(600);
    let failed: Result<String, AgentError> = Err(AgentError::Subagent("boom".into()));
    for _ in 0..3 {
        record_profile_result(&failures, "tester", &failed);
    }
    let opened = failures.lock().unwrap()["tester"].last_failure;
    let admit = |at: std::time::Instant| {
        circuit_admit(&mut failures.lock().unwrap(), "tester", 3, cooldown, at)
    };
    let error = admit(opened + std::time::Duration::from_secs(60)).unwrap_err();
    assert!(error.contains("circuit is open"), "{error}");
    assert!(error.contains("540s"), "{error}");
    assert!(
        admit(opened + cooldown).is_ok(),
        "one trial after the cooldown"
    );
    assert!(
        admit(opened + cooldown)
            .unwrap_err()
            .contains("already in flight"),
        "only one trial at a time"
    );
    // 试探失败：重新计时，仍然开着。
    record_profile_result(&failures, "tester", &failed);
    let reopened = failures.lock().unwrap()["tester"].last_failure;
    assert!(admit(reopened + std::time::Duration::from_secs(1)).is_err());
    // 试探成功：清零。
    assert!(admit(reopened + cooldown).is_ok());
    record_profile_result(&failures, "tester", &Ok("fixed".into()));
    assert!(admit(reopened).is_ok());
    assert!(failures.lock().unwrap().is_empty());
    // 别的工种不受影响。
    assert!(circuit_admit(&mut BTreeMap::new(), "scout", 3, cooldown, opened).is_ok());
}

#[test]
fn partial_child_remains_partial_in_background_and_lifecycle() {
    let error = AgentError::SubagentPartial {
        reason: crate::AgentStopReason::Incomplete,
        turns: 3,
        report: "saved partial work".into(),
    };
    let result = Err(error);
    assert_eq!(
        subagent_lifecycle_status(&result),
        SubagentLifecycleStatus::Partial
    );
    let task = subagent_task_result(result);
    assert_eq!(task.status, BackgroundTaskStatus::Partial);
    assert_eq!(task.exit_code, None);
    assert!(task.output.contains("saved partial work"));
    assert_eq!(
        background_lifecycle_status(&task.status),
        SubagentLifecycleStatus::Partial
    );
}

#[test]
fn approval_denial_marks_background_subagent_blocked() {
    let result = subagent_task_result(Err(AgentError::Tool(ToolError::ApprovalDenied(
        "write access needed".to_owned(),
    ))));
    assert_eq!(result.status, BackgroundTaskStatus::Blocked);
    assert_eq!(result.exit_code, None);
    assert_eq!(result.output, "write access needed");
}

/// 一次就交报告、每次报 700 token 的 Worker 模型。
struct MeteredProvider;

#[async_trait::async_trait]
impl crate::provider::Provider for MeteredProvider {
    async fn complete(
        &self,
        _messages: &[crate::types::Message],
        _tools: &[crate::types::ToolDefinition],
    ) -> Result<crate::types::Completion, crate::provider::ProviderError> {
        Ok(crate::types::Completion {
            content: "CONCLUSION: done".to_owned(),
            finish_reason: Some("stop".to_owned()),
            usage: Some(crate::types::Usage {
                input_tokens: Some(600),
                output_tokens: Some(100),
                total_tokens: None,
                cache_read_tokens: None,
            }),
            ..Default::default()
        })
    }
}

/// RA3：Worker 的用量计入父会话目标的 token 预算；没有激活目标时不记。
#[tokio::test]
async fn worker_usage_counts_toward_the_parent_goal_budget() {
    let root = std::env::temp_dir().join(format!("willdeep-goal-worker-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let goal = Arc::new(crate::goal::GoalContinuation::new());
    let catalog = SubagentCatalog::new(
        &root,
        builtin_profiles(Arc::new(MeteredProvider)),
        Arc::new(BackgroundTaskRegistry::default()),
    )
    .with_goal(goal.clone());
    let spawn = || SpawnAgentArgs {
        prompt: "inspect".to_owned(),
        profile: Some("scout".to_owned()),
        run_in_background: Some(false),
        ..SpawnAgentArgs::default()
    };
    catalog
        .run(spawn(), None)
        .await
        .expect("run without a goal");
    assert!(goal.snapshot().is_none(), "no goal, nothing recorded");

    goal.activate("audit the parser", goal.default_budget());
    catalog.run(spawn(), None).await.expect("run under a goal");
    assert_eq!(goal.snapshot().unwrap().tokens_used, 700);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn foreground_profile_runs_in_isolated_non_nested_agent() {
    let (catalog, root) = fixture();
    let sink = Arc::new(CaptureSink::default());
    let catalog = catalog.with_event_sink(sink.clone());
    let report = catalog
        .run(
            SpawnAgentArgs {
                prompt: "inspect".to_owned(),
                label: None,
                profile: Some("scout".to_owned()),
                run_in_background: Some(false),
                ..SpawnAgentArgs::default()
            },
            None,
        )
        .await
        .expect("run");
    assert!(report.starts_with("subagent report\n\n"), "{report}");
    assert!(
        report.ends_with("</worker-facts>"),
        "the runtime trailer closes every report: {report}"
    );
    assert!(report.contains(r#"profile="scout" verdict="unverified""#));
    let events = sink.0.lock().unwrap();
    let started = events.iter().find_map(|event| match event {
        AgentEvent::SubagentStarted {
            id,
            profile,
            background,
            ..
        } => Some((*id, profile.as_str(), *background)),
        _ => None,
    });
    let completed = events.iter().find_map(|event| match event {
        AgentEvent::SubagentCompleted { id, status, .. } => Some((*id, *status)),
        _ => None,
    });
    assert_eq!(
        started.map(|(_, profile, background)| (profile, background)),
        Some(("scout", false))
    );
    assert_eq!(
        completed,
        started.map(|(id, _, _)| (id, SubagentLifecycleStatus::Completed))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::SubagentTurnStarted { id, turn: 1 }
            if Some(*id) == started.map(|(id, _, _)| id)
    )));
    drop(events);
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[tokio::test]
async fn editor_requires_preapproved_target() {
    let (catalog, root) = fixture();
    let result = catalog
        .run(
            SpawnAgentArgs {
                prompt: "edit".to_owned(),
                label: None,
                profile: Some("editor".to_owned()),
                run_in_background: Some(false),
                target_file: Some("file.txt".to_owned()),
                ..SpawnAgentArgs::default()
            },
            None,
        )
        .await;
    assert!(matches!(result, Err(AgentError::Subagent(_))));
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[tokio::test]
async fn external_spawn_is_background_only_and_rejects_writing_profiles() {
    let (catalog, root) = fixture();
    let sink = Arc::new(CaptureSink::default());
    let catalog = catalog.with_event_sink(sink.clone());
    assert!(
        catalog
            .spawn_external_read_only(
                uuid::Uuid::new_v4(),
                "edit".to_owned(),
                None,
                Some("editor".to_owned()),
            )
            .await
            .is_err()
    );
    assert!(
        catalog
            .spawn_external_read_only(
                uuid::Uuid::new_v4(),
                "inspect everything".to_owned(),
                None,
                Some("deep".to_owned()),
            )
            .await
            .is_err()
    );
    assert!(
        catalog
            .spawn_external_read_only(
                uuid::Uuid::new_v4(),
                "inspect".to_owned(),
                None,
                Some("missing".to_owned()),
            )
            .await
            .is_err()
    );

    let id = uuid::Uuid::new_v4();
    catalog
        .spawn_external_read_only(
            id,
            "inspect".to_owned(),
            Some("external scout".to_owned()),
            Some("scout".to_owned()),
        )
        .await
        .expect("spawn external read-only agent");
    for _ in 0..50 {
        if sink.0.lock().unwrap().iter().any(|event| {
            matches!(event, AgentEvent::SubagentCompleted { id: event_id, .. } if *event_id == id)
        }) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let events = sink.0.lock().unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::SubagentStarted {
            id: event_id,
            profile,
            background: true,
            ..
        } if *event_id == id && profile == "scout"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::SubagentCompleted { id: event_id, .. } if *event_id == id
    )));
    drop(events);
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[tokio::test]
async fn terminal_background_agent_retries_with_a_reconfigured_provider_model() {
    let root =
        std::env::temp_dir().join(format!("willdeep-subagent-model-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("workspace");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn Provider> = Arc::new(ModelProvider {
        model: "old-model".to_owned(),
        seen: seen.clone(),
    });
    let background = Arc::new(BackgroundTaskRegistry::default());
    let sink = Arc::new(CaptureSink::default());
    let mut profiles = builtin_profiles(provider);
    for profile in &mut profiles {
        profile.model = Some("old-model".to_owned());
    }
    let catalog = SubagentCatalog::new(&root, profiles, background).with_event_sink(sink.clone());
    let id = uuid::Uuid::new_v4();
    catalog
        .spawn_external_read_only(
            id,
            "inspect".to_owned(),
            Some("model retry".to_owned()),
            Some("scout".to_owned()),
        )
        .await
        .expect("spawn");
    for _ in 0..50 {
        if seen.lock().unwrap().len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(seen.lock().unwrap().as_slice(), ["old-model"]);

    assert!(
        catalog
            .retry_background_agent(id, Some("new-model"))
            .await
            .expect("retry with model")
            .is_some()
    );
    for _ in 0..50 {
        if seen.lock().unwrap().len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(seen.lock().unwrap().as_slice(), ["old-model", "new-model"]);
    assert!(sink.0.lock().unwrap().iter().any(|event| matches!(
        event,
        AgentEvent::SubagentStarted {
            id: event_id,
            model: Some(model),
            ..
        } if *event_id == id && model == "new-model"
    )));
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn approved_editor_target_maps_to_the_same_relative_worktree_path() {
    let root = PathBuf::from("/workspace/project");
    let prepared = PreparedSubagentWorkspace {
        workspace: PathBuf::from("/managed/agent"),
        root_workspace: root.clone(),
        branch: Some("willdeep/agent-test".to_owned()),
        dedicated: true,
    };
    let mapped = remap_approved_targets(
        Some(BTreeSet::from([
            root.join("src/lib.rs"),
            root.join("src/other.rs"),
        ])),
        &prepared,
    )
    .expect("map targets")
    .expect("targets");
    assert_eq!(
        mapped,
        BTreeSet::from([
            PathBuf::from("/managed/agent/src/lib.rs"),
            PathBuf::from("/managed/agent/src/other.rs"),
        ])
    );
    assert!(
        remap_approved_targets(
            Some(BTreeSet::from([PathBuf::from("/elsewhere/file")])),
            &prepared
        )
        .is_err()
    );
}

/// A verifier that is neither statically safe nor statically destructive
/// needs a judge. With none configured there is no one to ask and no
/// approval card to show, so the dispatch is refused before the worker
/// ever starts — the command must not run on nobody's authority.
#[tokio::test]
async fn an_undecidable_verifier_is_refused_when_no_judge_can_review_it() {
    let (catalog, root) = fixture();
    let error = catalog
        .run(
            SpawnAgentArgs {
                prompt: "fix".to_owned(),
                profile: Some("scout".to_owned()),
                run_in_background: Some(false),
                task: Some(TaskPacket {
                    goal: "verify".to_owned(),
                    verifier: Some(TaskVerifier {
                        command: "curl -X POST https://example.invalid/deploy".to_owned(),
                        expected_exit_code: None,
                    }),
                    ..TaskPacket::default()
                }),
                ..SpawnAgentArgs::default()
            },
            None,
        )
        .await
        .expect_err("an unreviewable verifier must be refused");
    assert!(
        error.to_string().contains("no safety judge is configured"),
        "the refusal must say why, got: {error}"
    );
    std::fs::remove_dir_all(root).expect("cleanup");
}

/// 父会话在 full-access：verifier 不请判官直接放行（sso 那次派工就是卡在这）；
/// 破坏性形状照样拒绝——派工这一刻没有审批卡可弹。
#[tokio::test]
async fn full_access_parent_skips_verifier_review_but_not_the_denylist() {
    let (catalog, root) = fixture();
    let parent = crate::tools::SharedApprovalMode::new(crate::ApprovalMode::FullAccess);
    let catalog = catalog.with_parent_approval_mode(parent.clone());
    let undecidable = TaskVerifier {
        command: ".venv/bin/python .wd_readme_probe.py".to_owned(),
        expected_exit_code: None,
    };
    catalog
        .gate_verifier_command(undecidable.clone())
        .await
        .expect("full access: no judge needed");
    let destructive = TaskVerifier {
        command: "rm -rf /".to_owned(),
        expected_exit_code: None,
    };
    assert!(catalog.gate_verifier_command(destructive).await.is_err());
    parent.set(crate::ApprovalMode::Smart);
    assert!(
        catalog.gate_verifier_command(undecidable).await.is_err(),
        "back in smart with no judge, the verifier needs review again"
    );
    std::fs::remove_dir_all(root).expect("cleanup");
}

/// A destructive shape never reaches the judge, exactly as in the main
/// agent's shell gate: a model must not be able to talk its way into one
/// by calling it a verifier.
#[tokio::test]
async fn a_destructive_verifier_is_refused_outright() {
    let (catalog, root) = fixture();
    let error = catalog
        .run(
            SpawnAgentArgs {
                prompt: "fix".to_owned(),
                profile: Some("scout".to_owned()),
                run_in_background: Some(false),
                task: Some(TaskPacket {
                    goal: "verify".to_owned(),
                    verifier: Some(TaskVerifier {
                        command: "rm -rf / --no-preserve-root".to_owned(),
                        expected_exit_code: None,
                    }),
                    ..TaskPacket::default()
                }),
                ..SpawnAgentArgs::default()
            },
            None,
        )
        .await
        .expect_err("a destructive verifier must be refused");
    assert!(
        error.to_string().contains("destructive shape"),
        "the refusal must name the reason, got: {error}"
    );
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[tokio::test]
async fn a_sensitive_verifier_never_reaches_the_ai_judge() {
    let root = std::env::temp_dir().join(format!(
        "willdeep-sensitive-verifier-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).expect("workspace");
    let catalog = SubagentCatalog::new(
        &root,
        builtin_profiles(Arc::new(ReportProvider) as Arc<dyn Provider>),
        Arc::new(BackgroundTaskRegistry::default()),
    )
    .with_safety_judge(Arc::new(AllowingJudge));
    let error = catalog
        .run(
            SpawnAgentArgs {
                prompt: "inspect".to_owned(),
                profile: Some("reader".to_owned()),
                task: Some(TaskPacket {
                    goal: "verify".to_owned(),
                    verifier: Some(TaskVerifier {
                        command: "cat ~/.ssh/id_ed25519".to_owned(),
                        expected_exit_code: None,
                    }),
                    ..TaskPacket::default()
                }),
                ..SpawnAgentArgs::default()
            },
            None,
        )
        .await
        .expect_err("sensitive verifier must be refused before AI review");
    assert!(error.to_string().contains("credential-sensitive"));
    assert!(error.to_string().contains("cat ~/.ssh/id_ed25519"));
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn public_catalog_is_five_responsibilities_while_legacy_profiles_remain_executable() {
    let provider: Arc<dyn Provider> = Arc::new(ReportProvider);
    let catalog = SubagentCatalog::new(
        std::env::temp_dir(),
        builtin_profiles(provider),
        Arc::new(BackgroundTaskRegistry::default()),
    );
    let description = catalog.description();
    for profile in PUBLIC_SUBAGENT_IDS {
        assert!(description.contains(&format!("`{profile}`")));
        assert!(catalog.has_profile(profile));
    }
    for legacy in [
        "scout",
        "editor",
        "test_fixer",
        "build_fixer",
        "log_inspector",
        "git_detective",
    ] {
        assert!(catalog.has_profile(legacy));
        assert!(!description.contains(&format!("`{legacy}`")));
    }
    assert_eq!(public_profile_id("git_detective"), Some("generalist"));
    assert_eq!(public_profile_id("build_fixer"), Some("implementer"));
    assert_eq!(public_profile_id("terminal_operator"), Some("ops_runner"));
    assert_eq!(public_profile_id("security_guard"), Some("reviewer"));
    // 改名前的公开名继续路由，别人保存的流程不该在这次改动里断掉。
    assert_eq!(public_profile_id("reader"), Some("generalist"));
    assert_eq!(public_profile_id("judge"), Some("reviewer"));
    // `deep` 现在是档位而不是工种：它的职责部分落在 generalist 上。
    assert_eq!(public_profile_id("deep"), Some("generalist"));
}

/// 档位换模型，职责不变。这条盯着正交化本身：`deep` 曾经既是职责也是
/// 价格，把它拆开之后，一个便宜的调查工种不能因为换了个名字就白拿父模型。
#[tokio::test]
async fn the_expert_tier_swaps_the_model_without_changing_the_responsibility() {
    let cheap: Arc<dyn Provider> = Arc::new(ReportProvider);
    let expert: Arc<dyn Provider> = Arc::new(ReportProvider);
    let catalog = SubagentCatalog::new(
        std::env::temp_dir(),
        builtin_profiles(cheap),
        Arc::new(BackgroundTaskRegistry::default()),
    )
    .with_tier_binding(
        crate::WorkerTier::Expert,
        TierBinding {
            provider: expert,
            model: Some("gpt-5.6-sol".to_owned()),
            window: 200_000,
            hosted_job_prompt: false,
        },
    );

    let generalist = catalog.profile(Some("generalist")).expect("generalist");
    // 默认这一档是便宜的：贵模型要过 agent 层的票据才拿得到。
    assert!(generalist.context_window <= crate::WorkerTier::Standard.context_budget());

    // 旧名仍然解析得到，别人保存的流程不该断。
    assert!(catalog.has_profile("deep"));
    assert!(catalog.has_profile("reader"));
    assert!(catalog.has_profile("judge"));
    assert_eq!(
        catalog.profile(Some("deep")).map(|p| p.id.as_str()),
        Some("generalist")
    );
    assert_eq!(
        catalog.profile(Some("judge")).map(|p| p.id.as_str()),
        Some("reviewer")
    );
}

#[test]
fn dispatch_model_names_the_bound_model_and_flags_an_unbound_tier() {
    let mut profiles = builtin_profiles(Arc::new(ReportProvider));
    for profile in &mut profiles {
        profile.model = Some("trade-model".to_owned());
    }
    let catalog = SubagentCatalog::new(
        std::env::temp_dir(),
        profiles,
        Arc::new(BackgroundTaskRegistry::default()),
    )
    .with_tier_binding(
        crate::WorkerTier::Expert,
        TierBinding {
            provider: Arc::new(ReportProvider),
            model: Some("gpt-5.6-sol".to_owned()),
            window: 200_000,
            hosted_job_prompt: false,
        },
    );

    let plain = catalog.dispatch_model("generalist", None);
    assert_eq!(plain.model.as_deref(), Some("trade-model"));
    assert_eq!(plain.fallback, None);

    let expert = catalog.dispatch_model("deep", Some("expert"));
    assert_eq!(expert.model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(expert.fallback, None);

    // 进阶档在这个 provider 上没绑：模型没换，这件事必须说出来。
    let advanced = catalog.dispatch_model("tester", Some("advanced"));
    assert_eq!(advanced.model.as_deref(), Some("trade-model"));
    let reason = advanced.fallback.expect("unbound tier is reported");
    assert!(reason.contains("worker_tier=advanced"), "{reason}");
    assert!(reason.contains("[worker_tiers.advanced]"), "{reason}");

    // 基础档不绑定是常态，不算兜底。
    assert_eq!(
        catalog.dispatch_model("tester", Some("standard")).fallback,
        None
    );
    assert_eq!(
        catalog.dispatch_model("no_such_trade", None),
        DispatchModel::default()
    );
}

/// 三档都能各绑各的模型，而且真的走到那个 provider 上。
///
/// 只断言绑定存进去了是不够的——它得在派工路径上兑现出来，否则用户在
/// `[worker_tiers.*]` 里写的东西就是一句空话（这正是 0.51 的毛病）。
#[tokio::test]
async fn every_tier_cashes_its_own_binding_at_dispatch() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let base: Arc<dyn Provider> = Arc::new(ModelProvider {
        model: "someim-32b".to_owned(),
        seen: seen.clone(),
    });
    let root = std::env::temp_dir().join(format!("willdeep-tiers-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("workspace");
    let mut catalog = SubagentCatalog::new(
        &root,
        builtin_profiles(base),
        Arc::new(BackgroundTaskRegistry::default()),
    );
    for (tier, model) in [
        (crate::WorkerTier::Advanced, "deepseek-v4-flash"),
        (crate::WorkerTier::Expert, "opus-5"),
    ] {
        catalog = catalog.with_tier_binding(
            tier,
            TierBinding {
                provider: Arc::new(ModelProvider {
                    model: model.to_owned(),
                    seen: seen.clone(),
                }),
                model: Some(model.to_owned()),
                window: tier.context_budget(),
                hosted_job_prompt: false,
            },
        );
    }

    for tier in ["standard", "advanced", "expert"] {
        catalog
            .run(
                SpawnAgentArgs {
                    prompt: "look".to_owned(),
                    profile: Some("generalist".to_owned()),
                    worker_tier: Some(tier.to_owned()),
                    run_in_background: Some(false),
                    ..SpawnAgentArgs::default()
                },
                None,
            )
            .await
            .expect("run");
    }

    // 同一个职责，三档三个模型。基础档没绑定，留在工种自己的模型上。
    assert_eq!(
        seen.lock().expect("models").as_slice(),
        ["someim-32b", "deepseek-v4-flash", "opus-5"]
    );
    std::fs::remove_dir_all(root).expect("cleanup");
}

/// 档位换掉一个托管工种的模型时，客户端必须把职责提示词补回来。
///
/// 这是最容易漏的一条：工种绑在网关托管的别名上（提示词由服务端 prepend，
/// 客户端因此不发），档位又把模型换成了别处的 `opus-5`——两边都不发，
/// Worker 就只剩边界段落，完全不知道自己该干什么。
#[tokio::test]
async fn switching_tiers_hands_the_job_prompt_back_to_the_client() {
    struct PromptProbe(Arc<Mutex<Vec<String>>>);
    #[async_trait]
    impl Provider for PromptProbe {
        async fn complete(
            &self,
            messages: &[Message],
            _tools: &[ToolDefinition],
        ) -> Result<Completion, ProviderError> {
            self.0.lock().unwrap().push(messages[0].content.clone());
            Ok(Completion {
                reasoning: None,
                content: "report".to_owned(),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_owned()),
                usage: None,
            })
        }
    }

    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn Provider> = Arc::new(PromptProbe(seen.clone()));
    let root = std::env::temp_dir().join(format!("willdeep-tierprompt-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("workspace");
    let mut profiles = builtin_profiles(provider.clone());
    for profile in &mut profiles {
        // 模拟一个绑在 `someim-32b-scout` 上的托管工种：提示词由服务端发。
        profile.hosted_job_prompt = profile.id == "scout";
    }
    let catalog =
        SubagentCatalog::new(&root, profiles, Arc::new(BackgroundTaskRegistry::default()))
            .with_tier_binding(
                crate::WorkerTier::Expert,
                TierBinding {
                    provider,
                    model: Some("opus-5".to_owned()),
                    window: 200_000,
                    // opus-5 不是托管别名，服务端不会 prepend 任何东西。
                    hosted_job_prompt: false,
                },
            );

    catalog
        .run(
            SpawnAgentArgs {
                prompt: "look".to_owned(),
                profile: Some("scout".to_owned()),
                worker_tier: Some("expert".to_owned()),
                run_in_background: Some(false),
                ..SpawnAgentArgs::default()
            },
            None,
        )
        .await
        .expect("run");

    let prompts = seen.lock().expect("prompts").clone();
    assert!(
        prompts[0].contains("LOCATION"),
        "换掉托管模型之后职责提示词必须由客户端补上，实际收到：{}",
        prompts[0]
    );
    std::fs::remove_dir_all(root).expect("cleanup");
}
