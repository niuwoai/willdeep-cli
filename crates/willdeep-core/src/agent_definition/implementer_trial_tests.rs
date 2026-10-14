use super::*;
use crate::provider::{Provider, ProviderError};
use crate::types::{Completion, Message, ToolDefinition};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn implementer_trial_delivers_locked_skills_and_keeps_undeclared_writes_disabled() {
    struct TrialProvider(Arc<Mutex<String>>);
    #[async_trait::async_trait]
    impl Provider for TrialProvider {
        async fn complete(
            &self,
            messages: &[Message],
            tools: &[ToolDefinition],
        ) -> std::result::Result<Completion, ProviderError> {
            *self.0.lock().unwrap() = messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(tools.iter().any(|tool| tool.name == "read_file"));
            for denied in [
                "create_file",
                "edit_file",
                "run_command",
                "spawn_agent",
                "send_agent_message",
                "stop_agent",
            ] {
                assert!(
                    !tools.iter().any(|tool| tool.name == denied),
                    "undeclared or nested tool was exposed: {denied}"
                );
            }
            Ok(Completion {
                reasoning: None,
                content: "fixture implementer trial report; no files changed".into(),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".into()),
                usage: None,
            })
        }
    }
    let captured = Arc::new(Mutex::new(String::new()));
    let mut bases = crate::builtin_profiles(Arc::new(TrialProvider(captured.clone())));
    let mut definition = AgentDefinition::parse(include_bytes!(
        "../../../../docs/examples/agent-definition.v1.json"
    ))
    .unwrap();
    definition.definition_id = "user/implementer-trial".into();
    definition.base_role = "implementer".into();
    definition.instructions="Implement only the declared task. For this fixture trial, inspect supplied context and return a bounded report.".into();
    definition.tools = vec!["workspace.read".into()];
    let content =
        "Use supplied fixture evidence. Preserve files when no write set is declared.".to_owned();
    let skill_hash = digest(content.as_bytes());
    definition.skills = vec![DefinitionSkill {
        id: "user/fixture-skill".into(),
        content: content.clone(),
        sha256: skill_hash.clone(),
        required: true,
    }];
    let revision = definition.revision().unwrap();
    let base = bases
        .iter()
        .find(|profile| profile.id == "implementer")
        .unwrap();
    let projected = definition.project(base).unwrap();
    assert_eq!(projected.write_scope, crate::SubagentWriteScope::None);
    bases.push(projected);
    let home = std::env::temp_dir().join(format!("implementer-trial-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("fixture.md"),
        "Fixture input for a read-only implementer trial.\n",
    )
    .unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec!["add", "fixture.md"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    ] {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&home)
            .output()
            .unwrap();
        assert!(output.status.success(), "fixture Git initialization failed");
    }
    let catalog = crate::SubagentCatalog::new(
        &home,
        bases,
        Arc::new(crate::BackgroundTaskRegistry::default()),
    )
    .with_state_home(&home)
    .with_worktree_root(home.join("worker-worktrees"))
    .with_parent_session(uuid::Uuid::new_v4());
    let result = catalog
        .run(
            crate::subagent::SpawnAgentArgs {
                profile: Some(definition.definition_id.clone()),
                prompt: "Inspect supplied fixture context and report the bounded outcome.".into(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    assert!(result.contains("fixture implementer trial report"));
    let actual = captured.lock().unwrap();
    for expected in [&definition.instructions, &revision, &content, &skill_hash] {
        assert!(
            actual.contains(expected),
            "trial provider did not receive the locked definition evidence"
        );
    }
    drop(actual);
    let snapshot_path = std::fs::read_dir(home.join("dispatches"))
        .unwrap()
        .filter_map(|item| item.ok().map(|item| item.path()))
        .find(|path| path.is_file())
        .unwrap();
    let snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(snapshot_path).unwrap()).unwrap();
    assert_eq!(
        snapshot["definition"],
        serde_json::to_value(definition).unwrap()
    );
    assert!(snapshot["approved_targets"].is_null());
    std::fs::remove_dir_all(home).unwrap();
}
