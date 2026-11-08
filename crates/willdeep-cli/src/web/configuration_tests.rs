use super::*;

struct Fixture {
    root: std::path::PathBuf,
    path: std::path::PathBuf,
}

impl Fixture {
    fn new(contents: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("reasoning-settings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        std::fs::write(&path, contents).unwrap();
        Self { root, path }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn update(contents: &str, reasoning: serde_json::Value) -> ProviderUpdate {
    serde_json::from_value(serde_json::json!({
        "revision": crate::model_routing::revision(contents),
        "id": "preview",
        "provider": "openai-compatible",
        "api": "chat-completions",
        "api_base": "",
        "api_key": "",
        "model": "summary-model",
        "set_default": true,
        "reasoning_only": true,
        "reasoning": reasoning,
    }))
    .unwrap()
}

fn without_reasoning(contents: &str) -> toml::Value {
    let mut value: toml::Value = toml::from_str(contents).unwrap();
    value["providers"]["preview"]
        .as_table_mut()
        .unwrap()
        .remove("reasoning");
    value
}

#[test]
fn reasoning_only_preserves_hidden_connection_fields_and_default_provider() {
    for connection in [
        "api_base_env = 'TEST_PROVIDER_BASE'\napi_key_env = 'TEST_PROVIDER_KEY'\n",
        "api_base = 'https://api.anthropic.com'\napi_key_env = 'TEST_PROVIDER_KEY'\n",
        "provider = 'anthropic'\napi_base = 'https://api.anthropic.com'\napi_key_env = 'TEST_PROVIDER_KEY'\n",
    ] {
        let source = format!(
            "version = 1\ndefault_provider = 'other'\n[providers.other]\nmodel = 'other-model'\n[providers.preview]\n{connection}model = 'existing-model'\nmax_output_tokens = 8192\n"
        );
        let fixture = Fixture::new(&source);
        let profile = serde_json::json!({
            "default_level": "balanced", "parameter": "reasoning_effort",
            "levels": {"balanced": "medium", "deep": "high"}
        });
        save_provider(&fixture.path, update(&source, profile.clone())).unwrap();
        let saved = std::fs::read_to_string(&fixture.path).unwrap();
        assert_eq!(without_reasoning(&saved), without_reasoning(&source));
        let settings = read_settings(&fixture.path).unwrap();
        let reasoning = settings
            .providers
            .iter()
            .find(|p| p.id == "preview")
            .unwrap()
            .reasoning
            .as_ref()
            .unwrap();
        assert_eq!(serde_json::to_value(reasoning).unwrap(), profile);
    }
}

#[test]
fn reasoning_only_replaces_scalar_mappings_and_clears_the_profile() {
    let source = "version = 1\n[providers.preview]\napi_base_env = 'TEST_PROVIDER_BASE'\napi_key_env = 'TEST_PROVIDER_KEY'\nmodel = 'existing-model'\n[providers.preview.reasoning]\ndefault_level = 'old'\nparameter = 'reasoning_effort'\n[providers.preview.reasoning.levels]\nold = 'low'\n";
    let fixture = Fixture::new(source);
    let profile = serde_json::json!({
        "default_level": "__proto__", "parameter": "reasoning.effort",
        "levels": {"__proto__": "high", "budget": 4096, "disabled": false}
    });
    save_provider(&fixture.path, update(source, profile.clone())).unwrap();
    let saved = std::fs::read_to_string(&fixture.path).unwrap();
    let settings = read_settings(&fixture.path).unwrap();
    assert_eq!(
        serde_json::to_value(settings.providers[0].reasoning.as_ref().unwrap()).unwrap(),
        profile
    );
    assert_eq!(without_reasoning(&saved), without_reasoning(source));

    save_provider(&fixture.path, update(&saved, serde_json::Value::Null)).unwrap();
    let cleared = read_settings(&fixture.path).unwrap();
    let reasoning = cleared.providers[0].reasoning.as_ref().unwrap();
    assert!(reasoning.levels.is_empty());
    assert_eq!(reasoning.default_level, None);
    assert_eq!(reasoning.parameter, None);
    assert_eq!(
        without_reasoning(&std::fs::read_to_string(&fixture.path).unwrap()),
        without_reasoning(source)
    );
}

#[test]
fn reasoning_only_rejects_stale_missing_or_invalid_updates_without_writing() {
    let source = "version = 1\n[providers.preview]\nmodel = 'existing-model'\n";
    let fixture = Fixture::new(source);
    let profile = serde_json::json!({"default_level": "deep", "levels": {"deep": "high"}});
    for case in [
        "stale",
        "missing_provider",
        "omitted_reasoning",
        "invalid_default",
    ] {
        let mut request = update(source, profile.clone());
        match case {
            "stale" => request.revision = "stale".into(),
            "missing_provider" => request.id = "missing".into(),
            "omitted_reasoning" => request.reasoning = None,
            "invalid_default" => {
                request
                    .reasoning
                    .as_mut()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .default_level = Some("unknown".into())
            }
            _ => unreachable!(),
        }
        assert!(save_provider(&fixture.path, request).is_err(), "{case}");
        assert_eq!(
            std::fs::read_to_string(&fixture.path).unwrap(),
            source,
            "{case}"
        );
    }
}

#[test]
fn inline_reasoning_tables_can_be_updated_and_cleared_without_touching_connections() {
    for mapping in [
        "[providers.preview.reasoning]\ndefault_level = 'balanced'\nparameter = 'reasoning_effort'\nlevels = { balanced = 'medium', deep = 'high' } # keep this comment\n",
        "reasoning = { default_level = 'balanced', parameter = 'reasoning_effort', levels = { balanced = 'medium', deep = 'high' } } # keep this comment\n",
    ] {
        let source = format!(
            "version = 1\n[providers.preview]\napi_base_env = 'TEST_PROVIDER_BASE'\napi_key_env = 'TEST_PROVIDER_KEY'\nmodel = 'existing-model'\n{mapping}"
        );
        let fixture = Fixture::new(&source);
        let profile = serde_json::json!({
            "default_level": "budget", "parameter": "thinking.budget_tokens",
            "levels": {"budget": 8192, "disabled": false, "__proto__": "high"}
        });
        save_provider(&fixture.path, update(&source, profile.clone())).unwrap();
        let saved = std::fs::read_to_string(&fixture.path).unwrap();
        assert!(saved.contains("# keep this comment"));
        assert_eq!(without_reasoning(&saved), without_reasoning(&source));
        let settings = read_settings(&fixture.path).unwrap();
        assert_eq!(
            serde_json::to_value(settings.providers[0].reasoning.as_ref().unwrap()).unwrap(),
            profile
        );

        save_provider(&fixture.path, update(&saved, serde_json::Value::Null)).unwrap();
        let cleared = std::fs::read_to_string(&fixture.path).unwrap();
        let settings = read_settings(&fixture.path).unwrap();
        let reasoning = settings.providers[0].reasoning.as_ref().unwrap();
        assert!(reasoning.levels.is_empty());
        assert_eq!(reasoning.default_level, None);
        assert_eq!(reasoning.parameter, None);
        assert!(cleared.contains("# keep this comment"));
        assert_eq!(without_reasoning(&cleared), without_reasoning(&source));
    }
}

#[test]
fn omitted_reasoning_preserves_the_existing_mapping() {
    let source = "[providers.preview.reasoning]\ndefault_level = 'deep'\n[providers.preview.reasoning.levels]\ndeep = 'high'\n";
    let old: crate::config::ConfigFile = toml::from_str(source).unwrap();
    let existing = old.providers["preview"].reasoning.as_ref();
    assert_eq!(
        patch_reasoning(source, "providers.preview", existing, &None).unwrap(),
        source
    );
}
