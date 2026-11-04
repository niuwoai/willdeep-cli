use super::*;

fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("willdeep-magpie-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    root
}
fn connection(model: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema_version":1, "client_id":CLIENT_ID, "enabled":true,
        "base_url":"http://localhost:4567/v1", "client_marker":magpie::CLI_MARKER, "model":model
    }))
    .unwrap()
}

#[test]
fn shared_contract_fixtures_preserve_raw_contents() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../docs/fixtures/magpie-agent-v1/cases.json"
    ))
    .unwrap();
    assert_eq!(fixture["cli_profile_id"], PROFILE_ID);
    for case in fixture["cases"].as_array().unwrap() {
        let result = match case["contents"].as_str() {
            Some(raw) => parse_for(raw.as_bytes(), case["client_id"].as_str().unwrap()),
            None => Ok(None),
        };
        let state = match result {
            Ok(Some(_)) => "connected",
            Ok(None) => "disconnected",
            Err(_) => "invalid",
        };
        assert_eq!(state, case["expected"], "case {}", case["id"]);
    }
}

#[test]
fn disabled_does_not_bypass_field_validation_and_integers_are_tokens() {
    for raw in [
        r#"{"enabled":false,"model":null}"#,
        r#"{"enabled":false,"schema_version":1.0}"#,
        r#"{"enabled":false,"schema_version":null}"#,
        r#"{"enabled":false,"context_window":4000.0}"#,
        r#"{"enabled":false,"max_output_tokens":1e2}"#,
        r#"{"enabled":false,"client_id":"willdeep"}"#,
        r#"{"enabled":false,"model":"x\u0085y"}"#,
        r#"{"enabled":false,"base_url":"https://remote/v1"}"#,
        r#"{"enabled":false,"enabled":false}"#,
    ] {
        assert!(parse_for(raw.as_bytes(), CLIENT_ID).is_err(), "{raw}");
    }
    let mut value: Value = serde_json::from_slice(&connection("provider/model")).unwrap();
    value["context_window"] = Value::Null;
    value["max_output_tokens"] = Value::Null;
    let profile = parse_for(&serde_json::to_vec(&value).unwrap(), CLIENT_ID)
        .unwrap()
        .unwrap();
    assert_eq!(profile.context_window, None);
    assert_eq!(profile.max_output_tokens, None);
    assert_eq!(profile.model.as_deref(), Some("provider/model"));
    assert!(parse_for(&connection(&"界".repeat(342)), CLIENT_ID).is_err());
}

#[test]
fn bounded_reader_rejects_symlinks_directories_and_oversize() {
    let root = root();
    let path = root.join("connection.json");
    assert!(read_at(&path).unwrap().is_none());
    assert!(read_at(&root).is_err());
    std::fs::write(&path, vec![b' '; MAX_MAGPIE_CONNECTION_BYTES as usize + 1]).unwrap();
    assert!(!read_at(&path).unwrap_err().retryable);
    std::fs::write(&path, connection("mock/a")).unwrap();
    assert_eq!(
        read_at(&path).unwrap().unwrap().model.as_deref(),
        Some("mock/a")
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&path, root.join("link")).unwrap();
        assert!(!read_at(&root.join("link")).unwrap_err().retryable);
        use std::os::unix::ffi::OsStrExt;
        let fifo = std::ffi::CString::new(root.join("fifo").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(!read_at(&root.join("fifo")).unwrap_err().retryable);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn truncated_snapshot_retries_once_and_reads_replacement() {
    let root = root();
    let path = root.join("connection.json");
    std::fs::write(&path, b"{\"enabled\":").unwrap();
    let pending = read_at_async(&path);
    let replace = async {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let replacement = root.join("replacement");
        std::fs::write(&replacement, connection("mock/replaced")).unwrap();
        std::fs::rename(replacement, &path).unwrap();
    };
    let (result, _) = tokio::join!(pending, replace);
    assert_eq!(
        result.unwrap().unwrap().model.as_deref(),
        Some("mock/replaced")
    );
    std::fs::write(&path, b"{\"enabled\":true,\"unknown\":1}").unwrap();
    assert!(!read_at(&path).unwrap_err().retryable);
    std::fs::remove_dir_all(root).unwrap();
}

fn configured() -> LoadedConfig {
    LoadedConfig {
        file: toml::from_str(
            r#"
        default_provider = "ordinary"
        [providers.ordinary]
        provider = "openai-compatible"
        api_base = "http://127.0.0.1:9999/v1"
        api_key = "fixture-public"
        model = "ordinary/model"
        [providers.worker]
        provider = "openai-compatible"
        api_base = "http://127.0.0.1:9998/v1"
        api_key = "fixture-worker"
        model = "worker/model"
    "#,
        )
        .unwrap(),
    }
}
fn foreground() -> crate::Cli {
    let mut cli = <crate::Cli as clap::Parser>::try_parse_from(["willdeep"]).unwrap();
    cli.model = None;
    cli.api_base = None;
    cli.api_key = None;
    cli.provider = None;
    cli.api = None;
    cli
}
fn environment(pairs: &[(&str, &str)]) -> crate::provider_settings::ProviderEnvironment {
    crate::provider_settings::ProviderEnvironment::from_values(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}
fn state(model: &str) -> std::result::Result<Option<ProviderProfile>, ConnectionError> {
    parse_for(&connection(model), CLIENT_ID)
}

#[test]
fn default_selection_and_restore_keep_provider_and_model_fixed() {
    let loaded = configured();
    let cli = foreground();
    let env = environment(&[]);
    let first = resolve_from_state(&cli, &loaded, None, &env, state("mock/a")).unwrap();
    assert_eq!(first.name.as_deref(), Some(PROFILE_ID));
    assert!(first.config.magpie_integration);
    let mut saved = willdeep_core::Session::new(PathBuf::from("/unused"), first.name, "");
    saved.model = Some(first.config.model);
    saved.magpie_integration = true;
    let second = resolve_from_state(&cli, &loaded, Some(&saved), &env, state("mock/b")).unwrap();
    assert_eq!(second.config.model, "mock/a");
    let new = resolve_from_state(&cli, &loaded, None, &env, state("mock/b")).unwrap();
    assert_eq!(new.config.model, "mock/b");
    assert!(resolve_from_state(&cli, &loaded, Some(&saved), &env, Ok(None)).is_err());
    let legacy = willdeep_core::Session::new(PathBuf::from("/unused"), None, "old");
    let restored = resolve_from_state(
        &cli,
        &loaded,
        Some(&legacy),
        &env,
        Err(invalid("fixture broken")),
    )
    .unwrap();
    assert_eq!(restored.name.as_deref(), Some("ordinary"));
    assert_eq!(restored.config.model, "ordinary/model");
    assert!(resolve_from_state(&cli, &loaded, None, &env, Err(invalid("fixture broken"))).is_err());
    let disconnected = resolve_from_state(&cli, &loaded, None, &env, Ok(None)).unwrap();
    assert_eq!(disconnected.config.model, "ordinary/model");
}

#[test]
fn explicit_profile_config_model_and_environment_bypass_unrelated_invalid() {
    let loaded = configured();
    let env = environment(&[]);
    for option in [
        "profile", "config", "model", "provider", "api", "base", "key",
    ] {
        let mut cli = foreground();
        match option {
            "profile" => cli.profile = Some("worker".into()),
            "config" => cli.config = Some(PathBuf::from("/custom/config.toml")),
            "model" => cli.model = Some("explicit/model".into()),
            "provider" => cli.provider = Some(crate::ProviderArg::OpenAiCompatible),
            "api" => cli.api = Some(crate::ApiArg::ChatCompletions),
            "base" => cli.api_base = Some("http://127.0.0.1:7777/v1".into()),
            "key" => cli.api_key = Some("fixture-key".into()),
            _ => unreachable!(),
        }
        let selection =
            resolve_from_state(&cli, &loaded, None, &env, Err(invalid("unrelated"))).unwrap();
        assert!(!selection.config.magpie_integration, "{option}");
        if option == "profile" {
            assert_eq!(selection.config.model, "worker/model");
        }
    }
    for pair in [
        ("WILLDEEP_MODEL", "env/model"),
        ("WILLDEEP_API_BASE", "http://127.0.0.1:8888/v1"),
        ("WILLDEEP_API_KEY", "fixture-key"),
        ("OPENAI_BASE_URL", "http://127.0.0.1:8888/v1"),
        ("ANTHROPIC_BASE_URL", "http://127.0.0.1:8888/v1"),
        ("OPENAI_API_KEY", "fixture-key"),
        ("ANTHROPIC_API_KEY", "fixture-key"),
        ("SOMEIM_API_KEY", "fixture-key"),
    ] {
        let selection = resolve_from_state(
            &foreground(),
            &loaded,
            None,
            &environment(&[pair]),
            Err(invalid("unrelated")),
        )
        .unwrap();
        assert!(!selection.config.magpie_integration, "{}", pair.0);
    }
}

#[test]
fn explicit_integration_allows_only_model_overrides_and_fails_closed() {
    let loaded = configured();
    let mut cli = foreground();
    cli.profile = Some(PROFILE_ID.into());
    cli.config = Some(PathBuf::from("/custom/config.toml"));
    cli.model = Some("chosen/full-model".into());
    let selected =
        resolve_from_state(&cli, &loaded, None, &environment(&[]), state("mock/a")).unwrap();
    assert_eq!(selected.config.model, "chosen/full-model");
    assert!(selected.config.magpie_integration);
    cli.model = None;
    let selected = resolve_from_state(
        &cli,
        &loaded,
        None,
        &environment(&[("WILLDEEP_MODEL", "env/model")]),
        state("mock/a"),
    )
    .unwrap();
    assert_eq!(selected.config.model, "env/model");
    for pair in [
        ("WILLDEEP_API_BASE", "http://127.0.0.1:1/v1"),
        ("WILLDEEP_API_KEY", "fixture-key"),
        ("OPENAI_BASE_URL", "http://127.0.0.1:1/v1"),
        ("ANTHROPIC_BASE_URL", "http://127.0.0.1:1/v1"),
        ("OPENAI_API_KEY", "fixture-key"),
        ("ANTHROPIC_API_KEY", "fixture-key"),
        ("SOMEIM_API_KEY", "fixture-key"),
    ] {
        assert!(
            resolve_from_state(&cli, &loaded, None, &environment(&[pair]), state("mock/a"))
                .is_err()
        );
    }
    cli.api = Some(crate::ApiArg::ChatCompletions);
    assert!(resolve_from_state(&cli, &loaded, None, &environment(&[]), state("mock/a")).is_err());
    cli.api = None;
    assert!(resolve_from_state(&cli, &loaded, None, &environment(&[]), Ok(None)).is_err());
    assert!(
        resolve_from_state(
            &cli,
            &loaded,
            None,
            &environment(&[]),
            Err(invalid("unrelated"))
        )
        .is_err()
    );
}

#[test]
fn effective_directory_keeps_disk_document_and_rejects_collisions() {
    let file = ConfigFile::default();
    let profile = parse_for(&connection("mock/a"), CLIENT_ID).unwrap();
    let view = EffectiveProviders::from_state(&file, Ok(profile.clone()));
    assert_eq!(
        view.default_name(true).unwrap().as_deref(),
        Some(PROFILE_ID)
    );
    assert!(
        view.config_with_environment(PROFILE_ID, &environment(&[]))
            .unwrap()
            .magpie_integration
    );
    assert!(file.providers.is_empty());
    assert_eq!(file.default_provider, None);
    let mut conflict = file;
    conflict
        .providers
        .insert(PROFILE_ID.into(), ProviderProfile::default());
    conflict
        .providers
        .insert("other".into(), ProviderProfile::default());
    let view = EffectiveProviders::from_state(&conflict, Ok(profile));
    assert!(view.default_name(true).is_err());
    assert!(view.config(PROFILE_ID).is_err());
    assert!(view.profile("other").is_ok());
    let disconnected = EffectiveProviders::from_state(&ConfigFile::default(), Ok(None));
    assert!(disconnected.config(PROFILE_ID).is_err());
}

#[test]
fn legacy_same_name_profile_works_but_integration_provenance_blocks_fallback() {
    let mut loaded = configured();
    loaded
        .file
        .providers
        .insert(PROFILE_ID.into(), loaded.file.providers["ordinary"].clone());
    let mut saved =
        willdeep_core::Session::new(PathBuf::from("/unused"), Some(PROFILE_ID.into()), "old");
    saved.model = Some("saved/model".into());
    let cli = foreground();
    let env = environment(&[]);
    let legacy = resolve_from_state(&cli, &loaded, Some(&saved), &env, Ok(None)).unwrap();
    assert!(!legacy.config.magpie_integration);
    assert_eq!(legacy.config.model, "saved/model");
    saved.magpie_integration = true;
    assert!(resolve_from_state(&cli, &loaded, Some(&saved), &env, Ok(None)).is_err());
    assert!(resolve_from_state(&cli, &loaded, Some(&saved), &env, state("mock/new")).is_err());
    let mut explicit = cli;
    explicit.profile = Some("worker".into());
    let other = resolve_from_state(
        &explicit,
        &loaded,
        Some(&saved),
        &env,
        Err(invalid("unrelated")),
    )
    .unwrap();
    assert!(!other.config.magpie_integration);
    assert_eq!(other.config.base_url, "http://127.0.0.1:9998/v1");
    assert_eq!(other.config.model, "worker/model");
}
