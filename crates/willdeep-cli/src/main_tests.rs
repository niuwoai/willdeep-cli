use super::*;

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
