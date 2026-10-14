use super::*;

fn fixture() -> AgentDefinition {
    AgentDefinition::parse(include_bytes!(
        "../../../../docs/examples/agent-definition.v1.json"
    ))
    .unwrap()
}

#[test]
fn canonical_identity_is_independent_of_json_layout() {
    let definition = fixture();
    let reordered = serde_json::to_vec_pretty(&definition).unwrap();
    assert_eq!(
        definition.revision().unwrap(),
        AgentDefinition::parse(&reordered)
            .unwrap()
            .revision()
            .unwrap()
    );
    assert_eq!(
        canonical::encode(&serde_json::json!({"z":"中😀\n", "a":true})).unwrap(),
        "{\"a\":true,\"z\":\"\\u4e2d\\ud83d\\ude00\\u000a\"}"
    );
}

#[test]
fn duplicate_unknown_and_numeric_fields_are_rejected() {
    assert!(canonical::parse_unique(br#"{"a":null,"a":false}"#).is_err());
    assert!(canonical::parse_unique(br#"{"a":{"b":true,"b":false}}"#).is_err());
    assert!(canonical::parse_unique(b"12").is_err());
    let mut value = serde_json::to_value(fixture()).unwrap();
    value["permission"] = "full-access".into();
    assert!(AgentDefinition::parse(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn tamper_and_untrusted_signature_are_rejected() {
    let package = AgentPackage::local(fixture()).unwrap();
    let mut value = serde_json::to_value(&package).unwrap();
    value["definition"]["instructions"] = "tampered".into();
    assert!(AgentPackage::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    assert!(package.verify_official(&BTreeMap::new()).is_err());
}

#[test]
fn pinned_install_and_version_collision_preserve_previous_revision() {
    let home = std::env::temp_dir().join(format!("agent-library-{}", uuid::Uuid::new_v4()));
    let library = store::AgentLibrary::new(&home);
    let original = AgentPackage::local(fixture()).unwrap();
    library
        .install(&original, store::Origin::Local, &BTreeMap::new())
        .unwrap();
    let mut next = fixture();
    next.instructions.push_str(" Changed.");
    assert!(
        library
            .install(
                &AgentPackage::local(next.clone()).unwrap(),
                store::Origin::Local,
                &BTreeMap::new()
            )
            .is_err()
    );
    next.version = "1.0.0-rc2".into();
    library.set_pinned(&next.definition_id, true).unwrap();
    assert!(
        library
            .install(
                &AgentPackage::local(next).unwrap(),
                store::Origin::Local,
                &BTreeMap::new()
            )
            .is_err()
    );
    assert_eq!(
        library.installations().unwrap()[&original.definition.definition_id].sha256,
        original.sha256
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn skill_content_must_match_its_lock() {
    let mut definition = fixture();
    definition.skills.push(DefinitionSkill {
        id: "user/test".into(),
        content: "Read evidence".into(),
        sha256: "0".repeat(64),
        required: true,
    });
    assert!(definition.validate().is_err());
    definition.skills[0].sha256 = digest(definition.skills[0].content.as_bytes());
    assert!(definition.validate().is_ok());
}

#[test]
fn cross_platform_golden_hashes_and_signature_match() {
    let golden: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../../docs/examples/agent-contract-golden.json"
    ))
    .unwrap();
    let keys = serde_json::from_value(golden["keys"].clone()).unwrap();
    for value in golden["packages"].as_array().unwrap() {
        let package = AgentPackage::parse(&serde_json::to_vec(value).unwrap()).unwrap();
        assert_eq!(package.definition.revision().unwrap(), package.sha256);
        if package.signature.is_some() {
            package.verify_official(&keys).unwrap();
        }
    }
}

#[test]
fn nullable_fields_are_still_required_by_contract() {
    let mut value = serde_json::to_value(fixture()).unwrap();
    value.as_object_mut().unwrap().remove("forked_from");
    assert!(AgentDefinition::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value = serde_json::to_value(AgentPackage::local(fixture()).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("signature");
    assert!(AgentPackage::parse(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[tokio::test]
async fn dispatch_injects_locked_definition_without_expanding_role_permissions() {
    use crate::provider::{Provider, ProviderError};
    use crate::types::{Completion, Message, ToolDefinition};
    use std::sync::{Arc, Mutex};
    struct InspectProvider(Arc<Mutex<String>>);
    #[async_trait::async_trait]
    impl Provider for InspectProvider {
        async fn complete(
            &self,
            messages: &[Message],
            tools: &[ToolDefinition],
        ) -> std::result::Result<Completion, ProviderError> {
            *self.0.lock().unwrap() = messages[0].content.clone();
            assert!(!tools.iter().any(|tool| matches!(
                tool.name.as_str(),
                "edit_file" | "create_file" | "run_command" | "spawn_agent"
            )));
            Ok(Completion {
                reasoning: None,
                content: "independent report".into(),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".into()),
                usage: None,
            })
        }
    }
    let captured = Arc::new(Mutex::new(String::new()));
    let bases = crate::builtin_profiles(Arc::new(InspectProvider(captured.clone())));
    let mut definition = fixture();
    definition
        .tools
        .extend(["workspace.edit".into(), "process.verify".into()]);
    let base = bases.iter().find(|p| p.id == "reviewer").unwrap();
    let profile = definition.project(base).unwrap();
    assert_eq!(profile.write_scope, crate::SubagentWriteScope::None);
    let home = std::env::temp_dir().join(format!("agent-dispatch-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();
    let parent = uuid::Uuid::new_v4();
    let mut profiles = bases;
    profiles.push(profile);
    let catalog = crate::SubagentCatalog::new(
        &home,
        profiles,
        Arc::new(crate::BackgroundTaskRegistry::default()),
    )
    .with_state_home(&home)
    .with_parent_session(parent);
    assert!(
        catalog
            .public_profile_ids()
            .contains(&definition.definition_id)
    );
    assert!(
        catalog
            .description()
            .contains(&definition.revision().unwrap())
    );
    let report = catalog
        .run(
            crate::subagent::SpawnAgentArgs {
                prompt: "Inspect evidence".into(),
                profile: Some(definition.definition_id.clone()),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    assert!(report.contains("independent report"));
    let actual = captured.lock().unwrap();
    assert!(actual.contains(&definition.instructions));
    assert!(actual.contains(&definition.revision().unwrap()));
    drop(actual);
    let files: Vec<_> = std::fs::read_dir(home.join("dispatches"))
        .unwrap()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            path.is_file().then_some(path)
        })
        .collect();
    let snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
    assert_eq!(
        snapshot["definition"],
        serde_json::to_value(definition).unwrap()
    );
    std::fs::remove_dir_all(home).unwrap();
}
