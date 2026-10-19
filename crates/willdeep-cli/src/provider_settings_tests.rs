use super::*;

fn environment(pairs: &[(&str, &str)]) -> ProviderEnvironment {
    ProviderEnvironment::from_values(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}

fn foreground() -> Cli {
    let mut cli = Cli::try_parse_from(["willdeep"]).unwrap();
    cli.api_base = None;
    cli.api_key = None;
    cli.model = None;
    cli.provider = None;
    cli.api = None;
    cli
}

fn profile(source: &str) -> ProviderProfile {
    toml::from_str(source).unwrap()
}

#[test]
fn openai_alias_selects_gateway_instead_of_unrelated_someim_key() {
    let env = environment(&[
        ("OPENAI_BASE_URL", "http://127.0.0.1:3425/v1"),
        ("OPENAI_API_KEY", "magpie-willdeep"),
        ("SOMEIM_API_KEY", "unrelated"),
        ("WILLDEEP_MODEL", "mock/chat"),
    ]);
    let config = env.resolve(None, Some(&foreground())).unwrap();
    assert_eq!(config.kind, ProviderKind::OpenAiCompatible);
    assert_eq!(config.base_url, "http://127.0.0.1:3425/v1");
    assert_eq!(config.api_key, "magpie-willdeep");
}

#[test]
fn anthropic_alias_keeps_messages_protocol_on_loopback() {
    let env = environment(&[
        ("ANTHROPIC_BASE_URL", "http://127.0.0.1:3425"),
        ("ANTHROPIC_API_KEY", "magpie-willdeep"),
        ("WILLDEEP_MODEL", "mock/messages"),
    ]);
    let config = env.resolve(None, Some(&foreground())).unwrap();
    assert_eq!(config.kind, ProviderKind::Anthropic);
    assert_eq!(config.dialect, ApiDialect::AnthropicMessages);
    assert_eq!(config.base_url, "http://127.0.0.1:3425");
}

#[test]
fn profile_environment_is_identical_for_frontend_worker_and_runtime() {
    let env = environment(&[
        ("MAGPIE_BASE", "http://127.0.0.1:4321/v1"),
        ("MAGPIE_KEY", "magpie-willdeep"),
    ]);
    let profile = profile(
        "provider='openai-compatible'\napi='chat-completions'\napi_base_env='MAGPIE_BASE'\napi_key_env='MAGPIE_KEY'\nmodel='mock/chat'",
    );
    let parent = env.resolve(Some(&profile), Some(&foreground())).unwrap();
    let child = env.resolve(Some(&profile), None).unwrap();
    let runtime = env.resolve(Some(&profile), Some(&foreground())).unwrap();
    for config in [child, runtime] {
        assert_eq!(
            (
                config.base_url,
                config.api_key,
                config.model,
                config.dialect
            ),
            (
                parent.base_url.clone(),
                parent.api_key.clone(),
                parent.model.clone(),
                parent.dialect
            )
        );
    }
}

#[test]
fn runtime_constructed_arguments_honor_generic_process_environment() {
    let env = environment(&[
        ("WILLDEEP_API_BASE", "http://127.0.0.1:4321/v1"),
        ("WILLDEEP_API_KEY", "magpie-willdeep"),
        ("WILLDEEP_MODEL", "mock/chat"),
    ]);
    let config = env.resolve(None, Some(&foreground())).unwrap();
    assert_eq!(config.base_url, "http://127.0.0.1:4321/v1");
    assert_eq!(config.api_key, "magpie-willdeep");
    assert_eq!(config.model, "mock/chat");
}

#[test]
fn explicit_child_profile_is_not_overwritten_by_parent_process_overrides() {
    let env = environment(&[
        ("WILLDEEP_API_BASE", "http://127.0.0.1:4321/v1"),
        ("WILLDEEP_API_KEY", "parent-key"),
        ("WILLDEEP_MODEL", "parent-model"),
        ("OPENAI_BASE_URL", "http://127.0.0.1:4321/v1"),
    ]);
    let profile = profile(
        "provider='openai-compatible'\napi_base='https://worker.example/v1'\napi_key='worker-key'\nmodel='worker-model'",
    );
    let config = env.resolve(Some(&profile), None).unwrap();
    assert_eq!(config.base_url, "https://worker.example/v1");
    assert_eq!(config.api_key, "worker-key");
    assert_eq!(config.model, "worker-model");
}

#[test]
fn explicit_cli_overrides_take_priority_even_if_profile_variables_are_missing() {
    let env = environment(&[("OPENAI_BASE_URL", "http://127.0.0.1:1/v1")]);
    let profile = profile(
        "provider='openai-compatible'\napi_base_env='MISSING_BASE'\napi_key_env='MISSING_KEY'\nmodel='mock/chat'",
    );
    let mut cli = foreground();
    cli.api_base = Some("http://127.0.0.1:4321/v1".into());
    cli.api_key = Some("explicit-key".into());
    let config = env.resolve(Some(&profile), Some(&cli)).unwrap();
    assert_eq!(config.base_url, "http://127.0.0.1:4321/v1");
    assert_eq!(config.api_key, "explicit-key");
}

#[test]
fn missing_explicit_environment_never_falls_back_to_another_provider() {
    let env = environment(&[
        ("OPENAI_BASE_URL", "https://fallback.example/v1"),
        ("OPENAI_API_KEY", "unrelated-key"),
    ]);
    let profile = profile(
        "provider='openai-compatible'\napi_base_env='MISSING_BASE'\napi_key_env='MISSING_KEY'\nmodel='mock/chat'",
    );
    let error = env.resolve(Some(&profile), None).unwrap_err().to_string();
    assert!(error.contains("MISSING_BASE"));
    assert!(!error.contains("unrelated-key"));
}

#[test]
fn blank_aliases_are_absent_and_invalid_schemes_fail() {
    let env = environment(&[("OPENAI_BASE_URL", "  "), ("ANTHROPIC_BASE_URL", "\n")]);
    assert_eq!(env.inferred_provider(), None);
    let env = environment(&[
        ("OPENAI_BASE_URL", "file:///tmp/gateway"),
        ("OPENAI_API_KEY", "test"),
        ("WILLDEEP_MODEL", "mock/chat"),
    ]);
    assert!(env.resolve(None, Some(&foreground())).is_err());
}

#[test]
fn endpoint_variable_schema_rejects_ambiguous_configuration() {
    let file: config::ConfigFile = toml::from_str("version=1\n[providers.magpie]\napi_base='http://127.0.0.1:3425/v1'\napi_base_env='MAGPIE_BASE'\n").unwrap();
    let error = config::validate(&file, std::path::Path::new("unused.toml")).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot define both api_base and api_base_env")
    );
}

#[test]
fn missing_worker_endpoint_does_not_borrow_unrelated_someim_environment() {
    let env = environment(&[
        ("SOMEIM_API_KEY", "unrelated"),
        ("WILLDEEP_API_BASE", "http://127.0.0.1:3425/v1"),
    ]);
    let profile = profile("api_key='worker-key'\nmodel='worker-model'");
    assert!(env.resolve(Some(&profile), None).is_err());
}

#[test]
fn generic_base_preserves_existing_anthropic_key_inference() {
    let env = environment(&[
        ("WILLDEEP_API_BASE", "http://127.0.0.1:3425"),
        ("ANTHROPIC_API_KEY", "magpie-willdeep"),
        ("WILLDEEP_MODEL", "mock/messages"),
    ]);
    let config = env.resolve(None, Some(&foreground())).unwrap();
    assert_eq!(config.dialect, ApiDialect::AnthropicMessages);
}
