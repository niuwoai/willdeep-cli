use super::*;
use willdeep_core::agent_definition::store::{AgentLibrary, ModelBinding, private_write};
use willdeep_core::agent_definition::{AgentDefinition, AgentPackage};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("wd-subagent-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn config(&self, contents: &str) -> PathBuf {
        let path = self.0.join("config.toml");
        private_write(&path, contents.as_bytes()).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("clean isolated subagent fixture");
    }
}

#[test]
fn unknown_subagent_points_to_config_section_and_library_commands() {
    for (language, explanation) in [
        ("zh-CN", "仅覆盖内置工种参数"),
        ("en", "only overrides built-in worker settings"),
        ("ja", "組み込みロールの設定専用"),
    ] {
        let fixture = Fixture::new();
        let path = fixture.config(&format!(
            "version=1\n[agent]\nlanguage='{language}'\n[subagents.doc-multilingual]\ncontext_window=65536\n"
        ));
        let error = LoadedConfig::load(Some(&path)).err().unwrap().to_string();
        for expected in [
            "unknown subagent profile: doc-multilingual",
            path.to_str().unwrap(),
            "[subagents.doc-multilingual]",
            "willdeep agents create/import",
            "willdeep agents bind <definition_id>",
            explanation,
        ] {
            assert!(error.contains(expected), "missing {expected:?}: {error}");
        }
    }
}

#[test]
fn built_in_and_legacy_subagent_sections_remain_valid() {
    for role in [
        "generalist",
        "implementer",
        "tester",
        "reviewer",
        "ops_runner",
        "scout",
        "reader",
        "deep",
        "judge",
        "editor",
        "test_fixer",
        "build_fixer",
        "log_inspector",
        "git_detective",
    ] {
        let file: ConfigFile = toml::from_str(&format!(
            "version=1\n[subagents.{role}]\ncontext_window=65536\n"
        ))
        .unwrap();
        validate(&file, Path::new("config.toml")).unwrap();
    }
}

#[test]
fn custom_agent_binding_loads_without_a_toml_subagent_section() {
    let fixture = Fixture::new();
    let path = fixture.config(
        "version=1\n[providers.fixture]\nprovider='openai-compatible'\napi_base='http://127.0.0.1:1/v1'\napi_key='loopback-fixture'\nmodel='parent-model'\n",
    );
    let loaded = LoadedConfig::load(Some(&path)).unwrap();
    let definition = AgentDefinition::parse(
        &serde_json::to_vec(&serde_json::json!({
            "schema": "willdeep.agent-definition.v1",
            "definition_id": "user/doc-multilingual",
            "version": "1.0.0",
            "name": {"en": "Document specialist"},
            "purpose": "Translate a bounded document",
            "base_role": "generalist",
            "instructions": "Preserve the complete source structure and terminology.",
            "input_modalities": ["text"],
            "tools": ["workspace.read"],
            "skills": [],
            "forked_from": null
        }))
        .unwrap(),
    )
    .unwrap();
    let package = AgentPackage::local(definition.clone()).unwrap();
    AgentLibrary::new(&fixture.0)
        .create_bound(
            &package,
            ModelBinding {
                provider_profile: Some("fixture".to_owned()),
                model: Some("specialist-model".to_owned()),
                context_window: Some(65_536),
                max_turns: Some(12),
            },
        )
        .unwrap();
    let parent = crate::provider_config_from_profile(&loaded.file, "fixture").unwrap();
    let bases =
        willdeep_core::builtin_profiles(willdeep_core::build_provider(parent.clone()).unwrap());
    let profiles =
        crate::agent_registry::runtime_profiles(&fixture.0, &bases, &loaded.file, &parent).unwrap();
    let profile = profiles
        .iter()
        .find(|item| item.id == definition.definition_id)
        .unwrap();
    assert!(loaded.file.subagents.is_empty());
    assert_eq!(profile.context_window, 65_536);
    assert_eq!(profile.max_turns, 12);
    assert_eq!(profile.model.as_deref(), Some("specialist-model"));
    assert_eq!(
        profile.definition.as_ref().unwrap().instructions,
        definition.instructions
    );
    assert!(profile.capability_prompt.contains(&definition.instructions));
}
