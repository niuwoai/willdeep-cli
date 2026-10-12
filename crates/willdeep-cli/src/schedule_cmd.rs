//! Every maintenance entry point calls the same Runtime API.
use super::*;
use willdeep_runtime_protocol::schedule::*;

#[derive(clap::Parser)]
struct ScheduleCli {
    #[command(subcommand)]
    action: ScheduleCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum ScheduleCommand {
    /// List task configuration and recent outcomes.
    List,
    /// List metadata retained for deleted task histories.
    Deleted,
    /// Inspect one task.
    Get { id: uuid::Uuid },
    /// Create a task from a ScheduleDraft JSON file.
    Create {
        file: PathBuf,
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
    },
    /// Update future configuration, requiring the revision shown by get.
    Update {
        id: uuid::Uuid,
        revision: u64,
        file: PathBuf,
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
    },
    /// Pause, resume, archive, complete or permanently delete a task.
    Action {
        id: uuid::Uuid,
        revision: u64,
        #[arg(value_enum)]
        action: TaskAction,
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
    },
    /// Start a run, or retry with an original run snapshot.
    Run {
        id: uuid::Uuid,
        #[arg(long)]
        retry_of: Option<uuid::Uuid>,
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
    },
    /// Page through run history (including retained history of deleted tasks).
    History {
        id: uuid::Uuid,
        #[arg(long)]
        before: Option<String>,
        #[arg(long, default_value_t = 25)]
        limit: usize,
        #[arg(long)]
        status: Option<String>,
    },
    /// Inspect one run including its immutable configuration snapshot.
    RunGet { id: uuid::Uuid, run_id: uuid::Uuid },
    /// Stop a run without removing the recurring task.
    Stop {
        id: uuid::Uuid,
        run_id: uuid::Uuid,
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
    },
    /// Preview expired records; --apply deletes the previewed class of records.
    Cleanup {
        id: uuid::Uuid,
        revision: u64,
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
    },
}
#[derive(clap::ValueEnum, Clone, Debug)]
pub(crate) enum TaskAction {
    Pause,
    Resume,
    Archive,
    Complete,
    Delete,
}
impl TaskAction {
    fn wire(&self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Archive => "archive",
            Self::Complete => "complete",
            Self::Delete => "delete",
        }
    }
}

fn read_draft(path: &std::path::Path) -> Result<ScheduleDraft> {
    serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("read schedule draft {}", path.display()))?,
    )
    .with_context(|| format!("parse ScheduleDraft JSON {}", path.display()))
}
fn request(
    command: ScheduleCommand,
) -> Result<(&'static str, serde_json::Value, Option<uuid::Uuid>)> {
    use serde_json::json;
    Ok(match command {
        ScheduleCommand::List => ("schedule.list", json!({}), None),
        ScheduleCommand::Deleted => ("schedule.deleted", json!({}), None),
        ScheduleCommand::Get { id } => ("schedule.get", json!({"id":id}), None),
        ScheduleCommand::Create { file, request_id } => (
            "schedule.create",
            serde_json::to_value(read_draft(&file)?)?,
            Some(request_id.unwrap_or_else(uuid::Uuid::new_v4)),
        ),
        ScheduleCommand::Update {
            id,
            revision,
            file,
            request_id,
        } => (
            "schedule.update",
            json!({"id":id,"expected_revision":revision,"task":read_draft(&file)?}),
            Some(request_id.unwrap_or_else(uuid::Uuid::new_v4)),
        ),
        ScheduleCommand::Action {
            id,
            revision,
            action,
            request_id,
        } => (
            "schedule.action",
            json!({"id":id,"expected_revision":revision,"action":action.wire()}),
            Some(request_id.unwrap_or_else(uuid::Uuid::new_v4)),
        ),
        ScheduleCommand::Run {
            id,
            retry_of,
            request_id,
        } => (
            "schedule.run",
            json!({"id":id,"retry_of":retry_of}),
            Some(request_id.unwrap_or_else(uuid::Uuid::new_v4)),
        ),
        ScheduleCommand::History {
            id,
            before,
            limit,
            status,
        } => {
            let status = status
                .map(|value| serde_json::from_value::<ScheduleRunStatus>(json!(value)))
                .transpose()
                .context("invalid scheduled run status")?;
            (
                "schedule.history",
                json!({"task_id":id,"before":before,"limit":limit,"status":status}),
                None,
            )
        }
        ScheduleCommand::RunGet { id, run_id } => (
            "schedule.run.get",
            json!({"task_id":id,"run_id":run_id}),
            None,
        ),
        ScheduleCommand::Stop {
            id,
            run_id,
            request_id,
        } => (
            "schedule.run.stop",
            json!({"task_id":id,"run_id":run_id}),
            Some(request_id.unwrap_or_else(uuid::Uuid::new_v4)),
        ),
        ScheduleCommand::Cleanup {
            id,
            revision,
            apply,
            request_id,
        } => (
            "schedule.cleanup",
            json!({"task_id":id,"expected_revision":revision,"preview":!apply}),
            Some(request_id.unwrap_or_else(uuid::Uuid::new_v4)),
        ),
    })
}
pub(crate) async fn run(command: ScheduleCommand, home: &std::path::Path) -> Result<String> {
    let (operation, params, request_id) = request(command)?;
    let value = crate::daemon::schedule_operation(home, operation, params, request_id)
        .await
        .with_context(|| format!("Runtime operation {operation}, request_id={request_id:?}"))?;
    Ok(serde_json::to_string_pretty(&value)?)
}
pub(crate) async fn slash(text: &str, home: &std::path::Path) -> Result<String> {
    let words = text.split_whitespace().skip(1);
    let mut arguments = vec!["schedule".to_owned()];
    arguments.extend(words.map(str::to_owned));
    if arguments.len() == 1 {
        arguments.push("list".to_owned());
    }
    let cli = ScheduleCli::try_parse_from(arguments)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    run(cli.action, home).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mutations_keep_revision_and_explicit_retry_request_id() {
        let id = uuid::Uuid::new_v4();
        let request_id = uuid::Uuid::new_v4();
        let (operation, value, nonce) = request(ScheduleCommand::Action {
            id,
            revision: 7,
            action: TaskAction::Pause,
            request_id: Some(request_id),
        })
        .unwrap();
        assert_eq!(operation, "schedule.action");
        assert_eq!(value["expected_revision"], 7);
        assert_eq!(nonce, Some(request_id));
    }
    #[test]
    fn cleanup_defaults_to_preview_and_rejects_unknown_history_status() {
        let id = uuid::Uuid::new_v4();
        let (_, value, _) = request(ScheduleCommand::Cleanup {
            id,
            revision: 1,
            apply: false,
            request_id: None,
        })
        .unwrap();
        assert_eq!(value["preview"], true);
        assert!(
            request(ScheduleCommand::History {
                id,
                before: None,
                limit: 25,
                status: Some("success-ish".to_owned())
            })
            .is_err()
        );
    }
}
