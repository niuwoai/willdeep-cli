//! Durable three-episode workflow. No Web process, model-driven loop or media generation is required.
mod host;
mod store;
#[cfg(test)]
mod tests;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::Store;
use uuid::Uuid;
use willdeep_core::plugin::PluginHost;
use willdeep_runtime_protocol::screenplay::*;

const PLUGIN: &str = "willdeep-video-studio";
const MAX_TASKS: usize = 64;

pub(super) async fn operation(
    home: &Path,
    request_id: Uuid,
    operation: &str,
    params: Value,
) -> Result<Value> {
    match operation {
        "screenplay.budget" => {
            let update: ScreenplayBudgetUpdate = serde_json::from_value(params)?;
            anyhow::ensure!(
                (12..=128).contains(&update.max_calls)
                    && (50_000..=2_000_000).contains(&update.max_tokens),
                "invalid screenplay budget"
            );
            Store::new(home).update(update.id, |workflow| {
                anyhow::ensure!(
                    !matches!(workflow.status.as_str(), "completed" | "cancelled"),
                    "terminal workflow budget cannot change"
                );
                anyhow::ensure!(
                    workflow.input.max_calls == update.expected_calls
                        && workflow.input.max_tokens == update.expected_tokens,
                    "budget revision conflict"
                );
                anyhow::ensure!(
                    update.max_calls >= update.expected_calls
                        && update.max_tokens >= update.expected_tokens
                        && (update.max_calls > update.expected_calls
                            || update.max_tokens > update.expected_tokens),
                    "explicit budget increase required"
                );
                workflow.budget_changes.push(ScreenplayBudgetChange {
                    request_id,
                    at: super::now(),
                    previous_calls: update.expected_calls,
                    previous_tokens: update.expected_tokens,
                    max_calls: update.max_calls,
                    max_tokens: update.max_tokens,
                });
                workflow.input.max_calls = update.max_calls;
                workflow.input.max_tokens = update.max_tokens;
                if workflow.status == "budget_limited" {
                    workflow.status = "waiting_user".into();
                }
                Ok(())
            })?;
            public(Store::new(home).get(update.id)?)
        }
        "screenplay.start" => {
            let mut input: ScreenplayStart = serde_json::from_value(params)?;
            anyhow::ensure!(
                !input.brief.trim().is_empty() && input.brief.chars().count() <= 4_000,
                "brief must contain 1-4000 characters"
            );
            anyhow::ensure!(
                (12..=128).contains(&input.max_calls)
                    && (50_000..=2_000_000).contains(&input.max_tokens),
                "invalid screenplay budget"
            );
            input.workspace = std::fs::canonicalize(input.workspace)?;
            anyhow::ensure!(input.workspace.is_dir(), "workspace must be a directory");
            let store = Store::new(home);
            let _lock = store.lock(request_id, false)?;
            let mut workflow = match store.get(request_id) {
                Ok(existing) => {
                    anyhow::ensure!(
                        serde_json::to_value(&existing.input)? == serde_json::to_value(&input)?,
                        "request ID belongs to different input"
                    );
                    existing
                }
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
                {
                    let timestamp = super::now();
                    ScreenplayWorkflow {
                        id: request_id,
                        request_id,
                        input,
                        status: "running".into(),
                        created_at: timestamp,
                        updated_at: timestamp,
                        tasks: Vec::new(),
                        calls: Vec::new(),
                        domain: None,
                        error: None,
                        cancel_requested: false,
                        budget_changes: Vec::new(),
                    }
                }
                Err(error) => return Err(error),
            };
            if workflow.tasks.is_empty() {
                workflow.status = "running".into();
            }
            store.put(&workflow)?;
            launch(home.to_path_buf(), workflow.id);
            public(workflow)
        }
        "screenplay.list" => Store::new(home)
            .list()?
            .into_iter()
            .map(public)
            .collect::<Result<Vec<_>>>()
            .map(|items| json!(items)),
        "screenplay.status" | "screenplay.cancel" | "screenplay.resume" => {
            let id: Uuid = serde_json::from_value(params["id"].clone())?;
            if operation == "screenplay.cancel" {
                Store::new(home).update(id, |workflow| {
                    anyhow::ensure!(
                        workflow.status != "completed",
                        "completed screenplay cannot be cancelled"
                    );
                    workflow.cancel_requested = true;
                    workflow.status = "cancelled".into();
                    Ok(())
                })?;
            } else if operation == "screenplay.resume" {
                let lease = Store::new(home).lock(id, true)?;
                Store::new(home).reconcile(id)?;
                Store::new(home).update(id, |workflow| {
                    anyhow::ensure!(
                        !matches!(
                            workflow.status.as_str(),
                            "completed" | "cancelled" | "running"
                        ),
                        "workflow cannot resume from this state"
                    );
                    anyhow::ensure!(
                        !workflow.calls.iter().any(|call| call.state != "completed"),
                        "uncertain billed call needs manual reconciliation"
                    );
                    anyhow::ensure!(workflow.tasks.len() < MAX_TASKS, "task limit exhausted");
                    workflow.status = "running".into();
                    workflow.error = None;
                    Ok(())
                })?;
                drop(lease);
                launch(home.to_path_buf(), id);
            }
            public(Store::new(home).get(id)?)
        }
        _ => anyhow::bail!("unknown screenplay operation"),
    }
}

fn public(mut workflow: ScreenplayWorkflow) -> Result<Value> {
    for task in &mut workflow.tasks {
        task.grant = Uuid::nil();
        if let Some(arguments) = task.arguments.as_object_mut() {
            arguments.remove("grant");
        }
    }
    serde_json::to_value(workflow).map_err(Into::into)
}

pub(super) fn recover(home: &Path) {
    match Store::new(home).list() {
        Ok(workflows) => {
            for workflow in workflows {
                if workflow.status == "running" {
                    launch(home.to_path_buf(), workflow.id);
                }
            }
        }
        Err(error) => eprintln!(
            "screenplay recovery refused: {}",
            willdeep_core::judge::redact_credentials(&error.to_string())
        ),
    }
}

fn launch(home: PathBuf, id: Uuid) {
    tokio::spawn(async move {
        let store = Store::new(&home);
        let Ok(_execution) = store.lock(id, true) else {
            return;
        };
        if let Err(error) = run(&home, id).await {
            let message = willdeep_core::judge::redact_credentials(&format!("{error:#}"));
            let _ = store.update(id, |workflow| {
                if workflow.cancel_requested {
                    workflow.status = "cancelled".into();
                    for task in workflow
                        .tasks
                        .iter_mut()
                        .filter(|task| task.state == "running")
                    {
                        task.state = "cancelled".into();
                    }
                } else if message.contains("budget_limited") {
                    workflow.status = "budget_limited".into();
                } else {
                    workflow.status = "waiting_user".into();
                }
                workflow.error = Some(message);
                Ok(())
            });
        }
    });
}

async fn run(home: &Path, id: Uuid) -> Result<()> {
    Store::new(home).reconcile(id)?;
    let workflow = Store::new(home).get(id)?;
    let plugin = PluginHost::discover(home)?;
    anyhow::ensure!(
        plugin.approval_gap(PLUGIN).await?.is_none(),
        "screenplay plugin requires permission approval"
    );
    plugin.set_host_requests(Arc::new(host::Host {
        home: home.to_path_buf(),
        workflow_id: id,
        inner: crate::plugin_host_requests::HostRequests::new(
            home.to_path_buf(),
            home.join("config.toml"),
            Arc::new(std::sync::RwLock::new(vec![
                workflow.input.workspace.clone(),
            ])),
        )
        .with_usage_context(id, workflow.input.workspace.clone()),
    }));
    let mcp = plugin.mcp(PLUGIN).await?; // Existing enabled/approved plugin permissions are required.
    let tools = mcp.request("video-studio", "tools/list", json!({})).await?;
    anyhow::ensure!(
        tools["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == "screenplay.step")),
        "installed plugin lacks screenplay.step; install the tested candidate locally"
    );
    loop {
        let workflow = Store::new(home).get(id)?;
        if workflow.cancel_requested || workflow.status != "running" {
            return Ok(());
        }
        // A saved plugin receipt is safe to query/replay with the original task identity.
        // In-flight model usage is reconciled by the host before it permits another billed call.
        let task = prepare_task(home, id)?;
        let response = mcp
            .request(
                "video-studio",
                "tools/call",
                json!({"name":"screenplay.step", "arguments":task.arguments}),
            )
            .await?;
        let result = tool_result(response)?;
        anyhow::ensure!(
            result["ok"] == true,
            "plugin step failed: {}",
            result["error"]
        );
        Store::new(home).update(id, |current| {
            let saved = current.tasks.last_mut().context("missing task")?;
            anyhow::ensure!(saved.id == task.id, "stale execution lease");
            saved.state = "completed".into();
            saved.result = Some(result.clone());
            current.domain = Some(result.clone());
            if result["completed"] == true {
                anyhow::ensure!(
                    result["scripts"] == 3
                        && result["mustFix"] == 0
                        && result["export"].is_object(),
                    "invalid completion evidence"
                );
                anyhow::ensure!(
                    current.calls.iter().all(|call| call.state == "completed"),
                    "uncertain usage prevents completion"
                );
                if !current.cancel_requested {
                    current.status = "completed".into();
                }
            }
            Ok(())
        })?;
    }
}

fn prepare_task(home: &Path, id: Uuid) -> Result<ScreenplayTask> {
    Store::new(home).update(id, |workflow| {
        anyhow::ensure!(!workflow.cancel_requested, "screenplay cancelled");
        if let Some(task) = workflow
            .tasks
            .last_mut()
            .filter(|task| task.state == "running")
        {
            return Ok(task.clone());
        }
        anyhow::ensure!(
            workflow.tasks.len() < MAX_TASKS,
            "budget_limited: task limit"
        );
        let task_id = Uuid::new_v4();
        let grant = Uuid::new_v4();
        let mut arguments = workflow
            .domain
            .as_ref()
            .map(|domain| domain["next"].clone())
            .unwrap_or_else(|| json!({"step":"plan"}));
        anyhow::ensure!(arguments.is_object(), "plugin supplied no next step");
        arguments["workflowID"] = json!(id);
        arguments["taskID"] = json!(task_id);
        arguments["grant"] = json!(grant);
        if let Some(domain) = &workflow.domain {
            arguments["expectedBasis"] = domain["basis"].clone();
        } else {
            arguments["brief"] = json!(workflow.input.brief);
        }
        let task = ScreenplayTask {
            id: task_id,
            grant,
            arguments,
            state: "running".into(),
            result: None,
        };
        workflow.tasks.push(task.clone());
        Ok(task)
    })
}

fn tool_result(response: Value) -> Result<Value> {
    if let Some(value) = response.get("structuredContent") {
        return Ok(value.clone());
    }
    let text = response["content"]
        .as_array()
        .and_then(|items| items.iter().find_map(|item| item["text"].as_str()))
        .context("missing plugin result")?;
    serde_json::from_str(text).context("invalid plugin JSON result")
}
