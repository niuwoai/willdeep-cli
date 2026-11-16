use super::*;

#[test]
fn ndjson_usage_keeps_the_model_that_was_billed() {
    let event = AgentEvent::Usage {
        usage: willdeep_core::types::Usage {
            input_tokens: Some(30),
            output_tokens: Some(4),
            cache_read_tokens: Some(10),
            ..Default::default()
        },
        model: Some("actual-provider-model".to_owned()),
    };
    let value = agent_event_json(event);
    assert_eq!(value["model"], "actual-provider-model");
    assert_eq!(value["input_tokens"], 30);
    assert_eq!(value["cache_read_tokens"], 10);
    let child = agent_event_json(AgentEvent::SubagentUsage {
        id: uuid::Uuid::nil(),
        usage: willdeep_core::types::Usage {
            input_tokens: Some(8),
            cache_read_tokens: Some(3),
            ..Default::default()
        },
        model: Some("worker-model".to_owned()),
    });
    assert_eq!(child["model"], "worker-model");
    assert_eq!(child["cache_read_tokens"], 3);
}

#[test]
fn headless_run_defaults_to_runtime_and_keeps_cli_secret_overrides_local() {
    let runtime = Cli::try_parse_from(["willdeep", "run", "inspect"]).unwrap();
    let Some(CliCommand::Run(runtime_args)) = runtime.command.as_ref() else {
        panic!("run command");
    };
    assert!(should_use_headless_runtime(
        &runtime,
        Some(runtime_args),
        false,
        false,
    ));

    let local = Cli::try_parse_from(["willdeep", "run", "--local", "inspect"]).unwrap();
    let Some(CliCommand::Run(local_args)) = local.command.as_ref() else {
        panic!("run command");
    };
    assert!(!should_use_headless_runtime(
        &local,
        Some(local_args),
        false,
        false,
    ));

    let secret =
        Cli::try_parse_from(["willdeep", "run", "--api-key", "not-serialized", "inspect"]).unwrap();
    let Some(CliCommand::Run(secret_args)) = secret.command.as_ref() else {
        panic!("run command");
    };
    assert!(!should_use_headless_runtime(
        &secret,
        Some(secret_args),
        false,
        false,
    ));
}

#[test]
fn bare_cli_matches_clap_defaults() {
    let parsed = Cli::try_parse_from(["willdeep"]).expect("bare invocation");
    assert_eq!(format!("{:?}", Cli::bare()), format!("{parsed:?}"));
}
