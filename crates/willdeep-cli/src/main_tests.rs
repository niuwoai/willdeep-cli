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
fn continue_task_is_explicit_and_requires_a_session_selector() {
    for local in [false, true] {
        for selector in [None, Some("--session"), Some("--resume")] {
            let mut argv = vec!["willdeep"];
            if selector == Some("--resume") {
                argv.extend(["--resume", "latest"]);
            }
            argv.extend(["run", "--continue-task"]);
            if local {
                argv.push("--local");
            }
            if selector == Some("--session") {
                argv.extend(["--session", "latest"]);
            }
            argv.push("inspect");
            let cli = Cli::try_parse_from(argv).unwrap();
            let Some(CliCommand::Run(args)) = cli.command.as_ref() else {
                panic!("run command");
            };
            assert!(args.continue_task);
            let validation = validate_run_session(args, cli.resume.as_deref());
            if selector.is_some() {
                validation.unwrap();
            } else {
                let error = validation.unwrap_err();
                assert_eq!(stable_exit_code(&error), 2);
                assert!(error.to_string().contains("--continue-task requires"));
            }
        }
    }
    let cli = Cli::try_parse_from(["willdeep", "run", "--session", "latest", "inspect"]).unwrap();
    let Some(CliCommand::Run(args)) = cli.command.as_ref() else {
        panic!("run command");
    };
    assert!(!args.continue_task);
    validate_run_session(args, cli.resume.as_deref()).unwrap();
}

#[test]
fn bare_cli_matches_clap_defaults() {
    let parsed = Cli::try_parse_from(["willdeep"]).expect("bare invocation");
    assert_eq!(format!("{:?}", Cli::bare()), format!("{parsed:?}"));
}
