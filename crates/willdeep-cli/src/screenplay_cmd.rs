use super::*;

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum ScreenplayCommand {
    /// Start a budgeted three-episode screenplay in the persistent Runtime.
    Start {
        brief: String,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long, default_value_t = 64)]
        max_calls: u32,
        #[arg(long, default_value_t = 400_000)]
        max_tokens: u64,
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
    },
    Status {
        id: uuid::Uuid,
    },
    List,
    Cancel {
        id: uuid::Uuid,
    },
    Resume {
        id: uuid::Uuid,
    },
    /// Increase an explicitly authorized budget without resetting prior consumption.
    Budget {
        id: uuid::Uuid,
        #[arg(long)]
        expected_calls: u32,
        #[arg(long)]
        expected_tokens: u64,
        #[arg(long)]
        max_calls: u32,
        #[arg(long)]
        max_tokens: u64,
    },
}

pub(crate) async fn run(command: ScreenplayCommand, home: &std::path::Path) -> Result<()> {
    use serde_json::json;
    let (operation, params, request_id) = match command {
        ScreenplayCommand::Start {
            brief,
            workspace,
            max_calls,
            max_tokens,
            request_id,
        } => (
            "screenplay.start",
            json!({"brief":brief,"workspace":workspace,"max_calls":max_calls,"max_tokens":max_tokens}),
            request_id,
        ),
        ScreenplayCommand::Status { id } => ("screenplay.status", json!({"id":id}), None),
        ScreenplayCommand::List => ("screenplay.list", json!({}), None),
        ScreenplayCommand::Cancel { id } => ("screenplay.cancel", json!({"id":id}), None),
        ScreenplayCommand::Resume { id } => ("screenplay.resume", json!({"id":id}), None),
        ScreenplayCommand::Budget {
            id,
            expected_calls,
            expected_tokens,
            max_calls,
            max_tokens,
        } => (
            "screenplay.budget",
            json!({"id":id,"expected_calls":expected_calls,"expected_tokens":expected_tokens,"max_calls":max_calls,"max_tokens":max_tokens}),
            None,
        ),
    };
    let result = crate::daemon::schedule_operation(home, operation, params, request_id).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
