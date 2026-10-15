use super::*;
use clap::Parser;
use std::ffi::OsString;
use willdeep_core::subagent::free_review::{Fixture, Options, run as run_mock_review};

#[derive(Clone, Debug, clap::ValueEnum)]
pub enum FixtureArg {
    Bug,
    Clean,
}

#[derive(Clone, Debug, clap::Args)]
pub struct Arguments {
    #[arg(long, required = true)]
    offline_mock_only: bool,
    #[arg(long)]
    output_directory: PathBuf,
    #[arg(long, value_enum, default_value = "bug")]
    fixture: FixtureArg,
    #[arg(long, default_value_t = 2)]
    max_requests: usize,
    #[arg(long, default_value_t = 1000)]
    max_tokens: u64,
    #[arg(long, default_value_t = 1)]
    read_rounds: usize,
    #[arg(long, default_value_t = 5000)]
    timeout_ms: u64,
    #[arg(long)]
    cancel_after_ms: Option<u64>,
    #[arg(long, default_value_t = 0)]
    mock_delay_ms: u64,
}
#[derive(Parser)]
#[command(
    name = "willdeep agents rsi free-review",
    about = "Offline Mock Worker acceptance; no provider configuration or credentials"
)]
struct OfflineCommand {
    #[command(flatten)]
    arguments: Arguments,
}

fn standalone_arguments(args: &[OsString]) -> Option<Result<Vec<OsString>>> {
    // Recognize the first command, never a run task that happens to name it.
    const VALUE_FLAGS: &[&str] = &[
        "-c",
        "--config",
        "-p",
        "--profile",
        "--api-base",
        "--api-key",
        "-m",
        "--model",
        "--provider",
        "--api",
        "-w",
        "--workspace",
        "--max-turns",
        "--max-output-tokens",
        "--language",
        "--web-workspace",
    ];
    let prefix = ["agents", "rsi", "free-review"];
    let mut position = 1;
    let mut global_arguments = false;
    for expected in prefix {
        while let Some(argument) = args.get(position).and_then(|value| value.to_str()) {
            if argument == "--" {
                return None;
            }
            if !argument.starts_with('-') {
                break;
            }
            global_arguments = true;
            position += if VALUE_FLAGS.contains(&argument) {
                2
            } else {
                1
            };
        }
        if args.get(position).is_none_or(|value| value != expected) {
            return None;
        }
        position += 1;
    }
    if global_arguments {
        return Some(Err(anyhow::anyhow!(
            "free-review must be invoked directly as `willdeep agents rsi free-review`; global/provider arguments are not accepted"
        )));
    }
    Some(Ok(std::iter::once(OsString::from(
        "willdeep agents rsi free-review",
    ))
    .chain(args[position..].iter().cloned())
    .collect()))
}

pub(super) async fn run_standalone(args: Vec<OsString>) -> Option<Result<()>> {
    let arguments = match standalone_arguments(&args)? {
        Ok(arguments) => arguments,
        Err(error) => return Some(Err(error)),
    };
    let command = OfflineCommand::parse_from(arguments);
    Some(execute(command.arguments).await.and_then(|value| {
        println!("{}", serde_json::to_string_pretty(&value)?);
        Ok(())
    }))
}

pub(super) async fn execute(arguments: Arguments) -> Result<Value> {
    ensure!(
        arguments.offline_mock_only,
        "free-review requires --offline-mock-only"
    );
    run_mock_review(
        &arguments.output_directory,
        Options {
            fixture: match arguments.fixture {
                FixtureArg::Bug => Fixture::Bug,
                FixtureArg::Clean => Fixture::Clean,
            },
            max_requests: arguments.max_requests,
            max_tokens: arguments.max_tokens,
            read_rounds: arguments.read_rounds,
            timeout_ms: arguments.timeout_ms,
            cancel_after_ms: arguments.cancel_after_ms,
            mock_delay_ms: arguments.mock_delay_ms,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn standalone_mock_parser_requires_opt_in_and_rejects_global_provider_arguments() {
        use clap::CommandFactory;
        assert!(
            OfflineCommand::command()
                .get_arguments()
                .all(|argument| argument.get_env().is_none())
        );
        let command = standalone_arguments(&args(&[
            "willdeep",
            "agents",
            "rsi",
            "free-review",
            "--offline-mock-only",
            "--output-directory",
            "new",
        ]))
        .unwrap()
        .unwrap();
        let parsed = OfflineCommand::try_parse_from(command).unwrap();
        assert!(parsed.arguments.offline_mock_only);
        for extra in [
            "--api-key",
            "--api-base",
            "--model",
            "--config",
            "--provider",
        ] {
            let command = standalone_arguments(&args(&[
                "willdeep",
                "agents",
                "rsi",
                "free-review",
                "--offline-mock-only",
                "--output-directory",
                "new",
                extra,
                "forbidden",
            ]))
            .unwrap()
            .unwrap();
            assert!(OfflineCommand::try_parse_from(command).is_err());
        }
        assert!(
            standalone_arguments(&args(&[
                "willdeep",
                "--model",
                "forbidden",
                "agents",
                "rsi",
                "free-review"
            ]))
            .unwrap()
            .is_err()
        );
        for command in [
            args(&[
                "willdeep",
                "agents",
                "--api-key",
                "forbidden",
                "rsi",
                "free-review",
            ]),
            args(&[
                "willdeep",
                "agents",
                "rsi",
                "--model=forbidden",
                "free-review",
            ]),
        ] {
            assert!(standalone_arguments(&command).unwrap().is_err());
        }
        assert!(
            OfflineCommand::try_parse_from(["free-review", "--output-directory", "new"]).is_err()
        );
    }
    #[test]
    fn standalone_detection_does_not_intercept_old_commands() {
        for command in [
            args(&["willdeep", "--version"]),
            args(&[
                "willdeep",
                "agents",
                "rsi",
                "native-trial",
                "--control",
                "fixture",
            ]),
            args(&["willdeep", "agents", "rsi", "check", "fixture"]),
            args(&["willdeep", "run", "--local", "agents", "rsi", "free-review"]),
            args(&["willdeep", "--", "agents", "rsi", "free-review"]),
        ] {
            assert!(standalone_arguments(&command).is_none());
        }
    }
}
