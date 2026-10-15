//! Single approved native trial. Batch admission and money are owned by the loopback broker.
use super::native_provider::{BrokerProvider, PROPOSAL};
use super::*;
use std::sync::Arc;

const SUITE_SHA: &str = "281f7dadf99f4cf86c4de5a922cdac7531c933654f878bb6b8f532d502a96575";
const BASELINE: &str = "51b79a923ceceeae80a7cc6e20f2f0dbdfdb136f1bc516b895450ea633273bdd";
const CANDIDATE: &str = "9554ab2e7fb5449750c903716efdbb640215af6fe34975609b17fd3d39a79306";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    schema: String,
    proposal_sha256: String,
    experiment_id: String,
    trial_id: String,
    arm: String,
    repetition: u64,
    attempt: u64,
    resume_of: Option<String>,
    agent_package: PathBuf,
    agent_revision: String,
    suite: PathBuf,
    suite_sha256: String,
    task_id: String,
    broker_origin: String,
    capability: String,
    output_directory: PathBuf,
}
fn read_control(path: &Path) -> Result<Control> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "native control must be a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o777 == 0o600,
            "native control requires permissions 0600"
        );
    }
    ensure!(
        metadata.len() <= 64 * 1024,
        "native control exceeds size limit"
    );
    let value = contract::parse_json(&std::fs::read(path)?)?;
    ensure!(
        value.get("resume_of").is_some(),
        "native control requires explicit resume_of"
    );
    let control: Control = serde_json::from_value(value)?;
    ensure!(
        control.schema == "willdeep.agent-rsi.native-trial-control.v1"
            && control.proposal_sha256 == PROPOSAL,
        "native control does not identify the separately approved proposal"
    );
    ensure!(
        !control.experiment_id.is_empty()
            && !control.trial_id.is_empty()
            && (1..=3).contains(&control.repetition)
            && (1..=2).contains(&control.attempt)
            && ((control.attempt == 1 && control.resume_of.is_none())
                || (control.attempt == 2
                    && control.resume_of.as_ref().is_some_and(|id| !id.is_empty()))),
        "invalid native trial lineage"
    );
    let expected = match control.arm.as_str() {
        "baseline" => BASELINE,
        "candidate" => CANDIDATE,
        _ => anyhow::bail!("unknown native trial arm"),
    };
    ensure!(
        control.agent_revision == expected && control.suite_sha256 == SUITE_SHA,
        "native revision or suite differs from approved proposal"
    );
    Ok(control)
}

pub(super) async fn execute(control_path: &Path, preview: bool) -> Result<Value> {
    let control = read_control(control_path)?;
    let package = read_package(&control.agent_package)?;
    ensure!(
        package.sha256 == control.agent_revision,
        "native package revision differs from target"
    );
    let suite_value = contract::parse_json(&std::fs::read(&control.suite)?)?;
    ensure!(
        contract::hash(&suite_value)? == control.suite_sha256,
        "native suite bytes changed"
    );
    let suite: Suite = serde_json::from_value(suite_value)?;
    suite.validate()?;
    let task = suite
        .tasks
        .iter()
        .find(|task| task.task_id == control.task_id)
        .context("native task not in frozen suite")?;
    let provider = Arc::new(BrokerProvider::new(
        &control.broker_origin,
        control.capability,
        control.trial_id.clone(),
        preview,
    )?);
    // Never put a task label, host oracle, control or credentials into this random workspace.
    let workspace = std::env::temp_dir().join(format!("willdeep-native-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&control.output_directory)
        .context("native output directory must be new")?;
    std::fs::create_dir(&workspace)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for directory in [&control.output_directory, &workspace] {
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let fixture = workspace.join("fixture.txt");
    private_write(&fixture, task.fixture.as_bytes())?;
    let fixture_before = digest(task.fixture.as_bytes());
    let budget = json!({"max_turns":4,"max_tokens":16000,"timeout_seconds":120});
    let home = control.output_directory.join("worker-home");
    let started = std::time::Instant::now();
    let observed = willdeep_core::subagent::evaluation::run_isolated(
        &package,
        &home,
        &workspace,
        &task.prompt,
        &budget,
        provider.clone(),
    )
    .await;
    let fixture_after = std::fs::read(&fixture).map(|bytes| digest(&bytes)).ok();
    let mut observation = json!({"schema":"willdeep.agent-rsi.native-observation.v1","proposal_sha256":PROPOSAL,
        "experiment_id":control.experiment_id,"trial_id":control.trial_id,"task_id":task.task_id,"split":task.split,
        "arm":control.arm,"repetition":control.repetition,"attempt":control.attempt,"resume_of":control.resume_of,
        "definition_revision":package.sha256,"suite_sha256":control.suite_sha256,"fixture_sha256":fixture_before,
        "fixture_after_sha256":fixture_after,"fixture_unchanged":fixture_after.as_deref()==Some(fixture_before.as_str()),
        "budget":budget,"preview":preview,"context":null,"events":[],"worker_report_sha256":null,
        "worker_status":"not_started","read_observed":false,"host_quality_verifier":"pending",
        "runtime":runtime()?,"request_receipts":[],"raw_model_content":null,"raw_model_content_sha256":null});
    observation["runtime"]["adapter"] = "rust-worker-native-broker-v1".into();
    match observed {
        Ok(observed) => {
            observation["context"] = serde_json::to_value(&observed.context)?;
            observation["events"] = observed.events.clone().into();
            observation["read_observed"] = observed
                .events
                .iter()
                .any(|event| {
                    event["kind"] == "tool_completed"
                        && event["name"] == "read_file"
                        && event["is_error"] == false
                })
                .into();
            match observed.result {
                Ok(report) => {
                    private_write(
                        &control.output_directory.join("worker_report.txt"),
                        report.as_bytes(),
                    )?;
                    observation["worker_report_sha256"] = digest(report.as_bytes()).into();
                    observation["worker_status"] = "completed".into();
                }
                Err(error) => {
                    observation["worker_status"] = classify(&error).into();
                }
            }
        }
        Err(_) => {
            observation["worker_status"] = "infrastructure_error".into();
        }
    }
    let captured = provider.captured.lock().unwrap();
    observation["request_receipts"] = captured.receipts.clone().into();
    let reads = validated_fixture_reads(&captured.receipts, &fixture, &task.fixture);
    observation["fixture_read_proofs"] = reads.clone().into();
    observation["read_observed"] =
        (observation["read_observed"] == true && !reads.is_empty()).into();
    let usage_complete = !captured.receipts.is_empty()
        && captured.receipts.iter().all(|receipt| {
            ["prompt_tokens", "completion_tokens", "total_tokens"]
                .iter()
                .all(|field| receipt["usage"][field].as_u64().is_some())
        });
    observation["usage_complete"] = usage_complete.into();
    for (target, source) in [
        ("input_tokens", "prompt_tokens"),
        ("output_tokens", "completion_tokens"),
        ("total_tokens", "total_tokens"),
    ] {
        observation[target] = if usage_complete {
            captured
                .receipts
                .iter()
                .map(|r| r["usage"][source].as_u64().unwrap())
                .sum::<u64>()
                .into()
        } else {
            Value::Null
        };
    }
    if let Some(content) = &captured.raw_model_content {
        private_write(
            &control.output_directory.join("raw_model_content.txt"),
            content.as_bytes(),
        )?;
        observation["raw_model_content"] = content.clone().into();
        observation["raw_model_content_sha256"] = digest(content.as_bytes()).into();
    }
    if let Some(body) = &captured.first_request {
        let bytes = serde_json::to_vec(body)?;
        observation["first_request_utf8_bytes"] = bytes.len().into();
        observation["first_request_sha256"] = digest(&bytes).into();
        if preview {
            private_write(&control.output_directory.join("first_request.json"), &bytes)?;
        }
    }
    if preview {
        observation["worker_status"] = "preview_stopped_before_network".into();
    }
    observation["platform"] = "rust-native-worker".into();
    observation["task_sha256"] = contract::hash(&serde_json::to_value(task)?)?.into();
    observation["loaded_revision"] = observation["context"]["definition_revision"].clone();
    observation["effective_prompt_sha256"] =
        observation["context"]["effective_prompt_sha256"].clone();
    observation["events_sha256"] = contract::hash(&observation["events"])?.into();
    observation["did_truncate"] = captured
        .receipts
        .iter()
        .any(|receipt| receipt["completion_truncated"] == true)
        .into();
    observation["model"] = json!({"provider":"some-im","model":super::native_provider::MODEL});
    observation["status"] = if preview {
        "preview"
    } else if observation["worker_status"] == "completed" {
        "unverified"
    } else {
        "infra_error"
    }
    .into();
    observation["grading"] = "host_only_pending".into();
    observation["duration_ms"] =
        (started.elapsed().as_millis().min(u64::MAX as u128) as u64).into();
    let tools = observation["events"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let verifier_input = json!({"schema":"willdeep.native-mechanism-verifier-input.v1","raw_model_content_sha256":observation["raw_model_content_sha256"],
        "worker_report_sha256":observation["worker_report_sha256"],"fixture_before":fixture_before,"fixture_after":fixture_after,"events":tools,"fixture_reads":reads});
    observation["mechanism_verifier"] = json!({"input_sha256":contract::hash(&verifier_input)?,"input":verifier_input,
        "passed":observation["read_observed"]==true && observation["fixture_unchanged"]==true && observation["worker_status"]=="completed"});
    private_write(
        &control.output_directory.join("observation.json"),
        &serde_json::to_vec_pretty(&observation)?,
    )?;
    Ok(observation)
}
fn validated_fixture_reads(receipts: &[Value], fixture: &Path, content: &str) -> Vec<Value> {
    let Some(canonical) = fixture.canonicalize().ok() else {
        return Vec::new();
    };
    let workspace = fixture.parent().expect("fixture parent");
    let mut proofs = Vec::new();
    for receipt in receipts
        .iter()
        .filter(|receipt| receipt["transport_valid"] == true)
    {
        for read in receipt["read_results_in_request"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let args = &read["arguments"];
            let Some(path) = args["path"].as_str() else {
                continue;
            };
            let path = if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                workspace.join(path)
            };
            if path.canonicalize().ok().as_ref() != Some(&canonical) {
                continue;
            }
            let offset = args["offset"]
                .as_u64()
                .unwrap_or(1)
                .max(1)
                .min(usize::MAX as u64) as usize;
            let limit = args["limit"]
                .as_u64()
                .unwrap_or(u64::MAX)
                .min(usize::MAX as u64) as usize;
            let max_bytes = args["max_bytes"]
                .as_u64()
                .unwrap_or(u64::MAX)
                .max(1)
                .min(usize::MAX as u64) as usize;
            let mut expected = String::new();
            for (index, line) in content.lines().enumerate().skip(offset - 1).take(limit) {
                let row = format!("{:>6}  {}\n", index + 1, line);
                if expected.len() + row.len() > max_bytes {
                    expected.push_str(&format!(
                        "[truncated; continue with offset {}]\n",
                        index + 1
                    ));
                    break;
                }
                expected.push_str(&row);
            }
            if expected.is_empty() || read["tool_result_sha256"] != digest(expected.as_bytes()) {
                continue;
            }
            proofs.push(json!({"request_sequence":receipt["sequence"],"request_id":receipt["request_id"],"receipt_id":receipt["receipt_id"],
                "tool_call_id":read["tool_call_id"],"tool_result_sha256":read["tool_result_sha256"],"fixture_sha256":digest(content.as_bytes())}));
        }
    }
    proofs
}
fn classify(error: &willdeep_core::AgentError) -> &'static str {
    use willdeep_core::{AgentError, AgentStopReason};
    match error {
        AgentError::MaxTurns(_)
        | AgentError::TokenBudgetExceeded { .. }
        | AgentError::SubagentPartial {
            reason: AgentStopReason::MaxTurns | AgentStopReason::BudgetLimited,
            ..
        } => "budget_exhausted",
        AgentError::TokenBudgetUsageUnknown => "usage_unknown",
        _ => "infrastructure_error",
    }
}

#[cfg(test)]
mod tests;
