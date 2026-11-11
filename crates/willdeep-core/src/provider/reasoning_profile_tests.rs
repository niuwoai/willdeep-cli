use super::{ApiDialect, ProviderConfig, ProviderError, ProviderKind, ReasoningProfile};
use serde_json::json;

fn configured() -> ProviderConfig {
    let mut config = ProviderConfig::new(
        ProviderKind::OpenAiCompatible,
        ApiDialect::ChatCompletions,
        "https://provider.example/v1",
        "",
        "demo",
    );
    config.allow_unauthenticated = true;
    config.reasoning = Some(ReasoningProfile {
        default_level: Some("balanced".into()),
        parameter: None,
        levels: [
            ("balanced".into(), json!("medium")),
            ("deep".into(), json!("high")),
        ]
        .into(),
    });
    config
}

#[test]
fn protocol_paths_apply_default_and_override_without_losing_sibling_fields() {
    for (path, pointer) in [
        ("reasoning_effort", "/reasoning_effort"),
        ("reasoning.effort", "/reasoning/effort"),
        ("output_config.effort", "/output_config/effort"),
    ] {
        let mut config = configured();
        let original = json!({
            "model": "demo",
            "reasoning": { "summary": "auto" },
            "output_config": { "format": { "type": "json_schema" } }
        });
        let mut body = original.clone();
        config.apply_reasoning(&mut body, path).unwrap();
        assert_eq!(body.pointer(pointer), Some(&json!("medium")), "{path}");
        config.reasoning_level = Some("deep".into());
        config.apply_reasoning(&mut body, path).unwrap();
        assert_eq!(body.pointer(pointer), Some(&json!("high")), "{path}");
        assert_eq!(body["model"], original["model"]);
        assert_eq!(
            body["reasoning"]["summary"],
            original["reasoning"]["summary"]
        );
        assert_eq!(
            body["output_config"]["format"],
            original["output_config"]["format"]
        );
    }
}

#[test]
fn custom_paths_preserve_numeric_and_boolean_upstream_values() {
    for value in [json!(2048), json!(true), json!(false), json!(0)] {
        let mut config = configured();
        let profile = config.reasoning.as_mut().unwrap();
        profile.parameter = Some("thinking.budget".into());
        profile.levels.insert("balanced".into(), value.clone());
        config.validate().unwrap();
        let mut body = json!({"thinking": {"type": "enabled"}});
        config
            .apply_reasoning(&mut body, "reasoning_effort")
            .unwrap();
        assert_eq!(body["thinking"]["budget"], value);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert!(body.get("reasoning_effort").is_none());
    }
}

#[test]
fn absent_mapping_and_non_thinking_requests_leave_the_body_unchanged() {
    let original = json!({"model": "demo"});
    let mut config = configured();
    config.non_thinking = true;
    let mut body = original.clone();
    config
        .apply_reasoning(&mut body, "reasoning.effort")
        .unwrap();
    assert_eq!(body, original);
    config.non_thinking = false;
    config.reasoning = None;
    config
        .apply_reasoning(&mut body, "reasoning.effort")
        .unwrap();
    assert_eq!(body, original);
}

#[test]
fn invalid_selection_and_conflicting_paths_fail_without_replacing_existing_data() {
    let mut config = configured();
    config.reasoning_level = Some("unknown".into());
    assert!(matches!(
        config.validate(),
        Err(ProviderError::InvalidReasoningLevel(_))
    ));
    let original = json!({"reasoning": "existing-value"});
    let mut body = original.clone();
    assert!(matches!(
        config.apply_reasoning(&mut body, "reasoning.effort"),
        Err(ProviderError::InvalidReasoningLevel(_))
    ));
    assert_eq!(body, original);
    config.reasoning_level = None;
    assert!(matches!(
        config.apply_reasoning(&mut body, "reasoning.effort"),
        Err(ProviderError::InvalidReasoningParameter(_))
    ));
    assert_eq!(body, original);
}

#[test]
fn builtin_ladder_applies_only_when_a_level_is_explicitly_selected() {
    for (path, pointer) in [
        ("reasoning_effort", "/reasoning_effort"),
        ("reasoning.effort", "/reasoning/effort"),
        ("output_config.effort", "/output_config/effort"),
    ] {
        let mut config = configured();
        config.reasoning = None;
        let mut body = json!({"model": "demo"});
        // 没选等级：和以前一样，什么都不发。
        config.apply_reasoning(&mut body, path).unwrap();
        assert_eq!(body, json!({"model": "demo"}), "{path}");
        assert!(!config.will_apply_reasoning(), "{path}");
        // 显式选中内置档位：按方言默认参数路径原样发送。
        config.reasoning_level = Some("xhigh".into());
        config.validate().unwrap();
        assert!(config.will_apply_reasoning(), "{path}");
        assert!(config.accepts_reasoning_level("high"), "{path}");
        assert!(!config.accepts_reasoning_level("unknown"), "{path}");
        let effective = config.effective_reasoning().unwrap();
        assert!(effective.builtin, "{path}");
        assert_eq!(effective.level, "xhigh");
        config.apply_reasoning(&mut body, path).unwrap();
        assert_eq!(body.pointer(pointer), Some(&json!("xhigh")), "{path}");
        assert_eq!(body["model"], json!("demo"), "{path}");
    }
}

#[test]
fn configured_mapping_rejects_builtin_names_it_does_not_define() {
    let mut config = configured();
    assert!(config.accepts_reasoning_level("deep"));
    assert!(!config.accepts_reasoning_level("high"));
    config.reasoning_level = Some("high".into());
    assert!(matches!(
        config.validate(),
        Err(ProviderError::InvalidReasoningLevel(_))
    ));
}

#[test]
fn non_thinking_wins_over_a_selected_builtin_level() {
    let mut config = configured();
    config.reasoning = None;
    config.reasoning_level = Some("high".into());
    config.non_thinking = true;
    let mut body = json!({"model": "demo"});
    config
        .apply_reasoning(&mut body, "reasoning_effort")
        .unwrap();
    assert_eq!(body, json!({"model": "demo"}));
    assert!(!config.will_apply_reasoning());
}

#[test]
fn an_empty_profile_behaves_like_an_unmapped_provider() {
    let mut config = configured();
    config.reasoning = Some(ReasoningProfile::default());
    config.reasoning_level = Some("low".into());
    config.validate().unwrap();
    assert!(config.accepts_reasoning_level("low"));
    let mut body = json!({});
    config
        .apply_reasoning(&mut body, "reasoning.effort")
        .unwrap();
    assert_eq!(body["reasoning"]["effort"], "low");
}

#[test]
fn profile_validation_accepts_custom_names_and_rejects_invalid_maps() {
    for name in [
        "auto",
        "default",
        "reset",
        "__proto__",
        "x-high",
        "budget_4k",
    ] {
        let profile = ReasoningProfile {
            default_level: Some(name.into()),
            parameter: Some("output_config.effort".into()),
            levels: [(name.into(), json!("high"))].into(),
        };
        profile.validate().unwrap();
    }
    for invalid in [json!(null), json!([]), json!({"effort": "high"})] {
        let mut profile = configured().reasoning.unwrap();
        profile.levels.insert("deep".into(), invalid);
        assert!(profile.validate().is_err());
    }
    for path in [
        "",
        ".effort",
        "reasoning..effort",
        "reasoning.",
        "a.b.c.d.e.f.g.h.i",
    ] {
        let mut profile = configured().reasoning.unwrap();
        profile.parameter = Some(path.into());
        assert!(profile.validate().is_err(), "{path}");
    }
    for name in ["", "deep level", "deep.level", "深度"] {
        let mut profile = configured().reasoning.unwrap();
        profile.levels.insert(name.into(), json!("high"));
        assert!(profile.validate().is_err(), "{name}");
    }
    let mut profile = configured().reasoning.unwrap();
    profile.default_level = None;
    assert!(profile.validate().is_err());
    profile.default_level = Some("not-configured".into());
    assert!(profile.validate().is_err());
    assert!(ReasoningProfile::default().validate().is_ok());
}
