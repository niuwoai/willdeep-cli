//! The chat MCP tools and plugin page share the Runtime-owned screenplay queue.
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
use willdeep_core::mcp::HostRequestError;

pub(crate) const CONTROL: &str = "willdeep/screenplay/control";
const DEFAULT_CALLS: u32 = 64;
const DEFAULT_TOKENS: u64 = 500_000;

/// Make the installed domain capability visible before a coding agent searches local source.
/// This is guidance, never permission: tool approval, workspace and Runtime budget gates remain.
pub(crate) async fn routing_guidance(home: &Path, plugins_allowed: bool) -> Option<String> {
    if !plugins_allowed {
        return None;
    }
    let host = willdeep_core::plugin::PluginHost::discover(home).ok()?;
    let id = "willdeep-video-studio";
    let package = host.package(id).ok()?;
    if !package
        .manifest
        .as_ref()?
        .commands
        .iter()
        .any(|command| command.id == "screenplay.start")
        || !host.is_enabled(id).await
        || host.approval_gap(id).await.ok()?.is_some()
    {
        return None;
    }
    Some(format!(
        "\n\n# Automatic screenplay capability\n\
         The approved Video Studio plugin can autonomously finish a three-episode short drama from a rough topic. \
         When the user asks for this (including Chinese requests such as 三集短剧、自动完成、给我定稿), \
         use list_mcp_tools with query screenplay, then call_mcp_tool with the exact returned start tool name. \
         Do not inspect repository source, create helper programs, write screenplay files yourself, or substitute \
         a single-episode/manual planning tool for the durable workflow. A rough topic is sufficient: keep it as brief, \
         generate one UUID requestID and reuse it unchanged on retries. Tell the user the default cumulative \
         limit is {DEFAULT_TOKENS} tokens / {DEFAULT_CALLS} model calls before starting; never increase it without \
         explicit human authorization. Runtime owns writing, independent reviews, revisions and final export. \
         Use screenplay.inspect for progress and screenplay.download for accepted final files. Return the actual \
         workflow ID and status after start, explain that it continues in the background, and direct the user to \
         工坊 → 自动剧本. Do not claim a completed screenplay from a running, failed or unverified task.\n"
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation: String,
    #[serde(default)]
    arguments: Value,
}

fn roots(workspaces: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots: Vec<_> = workspaces
        .iter()
        .filter_map(|path| path.canonicalize().ok())
        .filter(|path| path.is_dir())
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

fn workspace(value: Option<&str>, roots: &[PathBuf]) -> anyhow::Result<PathBuf> {
    let path = match value {
        Some(value) => PathBuf::from(value).canonicalize()?,
        None if roots.len() == 1 => roots[0].clone(),
        _ => anyhow::bail!("screenplay_workspace_required"),
    };
    anyhow::ensure!(roots.contains(&path), "screenplay_workspace_denied");
    Ok(path)
}

fn allowed(value: &Value, roots: &[PathBuf]) -> bool {
    value["input"]["workspace"]
        .as_str()
        .and_then(|path| workspace(Some(path), roots).ok())
        .is_some()
}

fn progress(value: Value) -> Value {
    let calls = value["calls"].as_array().cloned().unwrap_or_default();
    let tokens: u64 = calls
        .iter()
        .filter_map(|call| call["tokens"].as_u64())
        .sum();
    let reserved: u64 = calls
        .iter()
        .filter(|call| call["state"] != "completed")
        .filter_map(|call| call["reservation"].as_u64())
        .sum();
    let tasks: Vec<_> = value["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|task| {
            json!({
                "id": task["id"], "step": task["arguments"]["step"],
                "episodeOrder": task["arguments"]["episodeOrder"], "state": task["state"]
            })
        })
        .collect();
    json!({"id": value["id"], "brief": value["input"]["brief"],
        "workspace": value["input"]["workspace"], "status": value["status"],
        "maxCalls": value["input"]["max_calls"], "maxTokens": value["input"]["max_tokens"],
        "calls": calls.len(), "tokens": tokens, "reservedTokens": reserved,
        "tasks": tasks, "domain": value["domain"], "error": value["error"],
        "createdAt": value["created_at"], "updatedAt": value["updated_at"],
        "budgetChanges": value["budget_changes"]})
}

pub(crate) async fn control(
    home: &Path,
    workspaces: &[PathBuf],
    params: Value,
) -> Result<Value, HostRequestError> {
    let request: Request = serde_json::from_value(params)
        .map_err(|error| HostRequestError::invalid_params(error.to_string()))?;
    dispatch(home, &roots(workspaces), request)
        .await
        .map_err(|error| {
            HostRequestError::failed(willdeep_core::judge::redact_credentials(&format!(
                "{error:#}"
            )))
        })
}

async fn dispatch(home: &Path, roots: &[PathBuf], request: Request) -> anyhow::Result<Value> {
    let mut args = request.arguments;
    let operation = request.operation.as_str();
    if operation == "list" {
        let values =
            crate::daemon::schedule_operation(home, "screenplay.list", json!({}), None).await?;
        let mut workflows: Vec<_> = values
            .as_array()
            .into_iter()
            .flatten()
            .filter(|value| allowed(value, roots))
            .cloned()
            .map(progress)
            .collect();
        workflows.sort_by_key(|value| std::cmp::Reverse(value["createdAt"].as_u64().unwrap_or(0)));
        return Ok(
            json!({"ok": true, "workflows": workflows, "workspaces": roots,
            "defaults": {"maxCalls": DEFAULT_CALLS, "maxTokens": DEFAULT_TOKENS}}),
        );
    }
    anyhow::ensure!(
        ["start", "inspect", "cancel", "resume", "budget"].contains(&operation),
        "screenplay_operation_denied"
    );
    let request_id = if operation == "start" {
        let id = serde_json::from_value::<Uuid>(args["requestID"].clone())?;
        let root = workspace(args["workspace"].as_str(), roots)?;
        args = json!({"brief": args["brief"], "workspace": root,
            "max_calls": args.get("maxCalls").cloned().unwrap_or(json!(DEFAULT_CALLS)),
            "max_tokens": args.get("maxTokens").cloned().unwrap_or(json!(DEFAULT_TOKENS))});
        Some(id)
    } else {
        let id: Uuid = serde_json::from_value(args["id"].clone())?;
        let current =
            crate::daemon::schedule_operation(home, "screenplay.status", json!({"id":id}), None)
                .await?;
        anyhow::ensure!(allowed(&current, roots), "screenplay_workspace_denied");
        if operation == "budget" {
            args = json!({"id":id, "expected_calls":args["expectedCalls"],
                "expected_tokens":args["expectedTokens"], "max_calls":args["maxCalls"], "max_tokens":args["maxTokens"]});
        } else {
            args = json!({"id": id});
        }
        None
    };
    let name = if operation == "inspect" {
        "status"
    } else {
        operation
    };
    let value =
        crate::daemon::schedule_operation(home, &format!("screenplay.{name}"), args, request_id)
            .await?;
    Ok(json!({"ok":true, "workflow":progress(value)}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn routing_requires_enabled_approved_plugin_and_respects_mcp_allowlist() {
        let home = std::env::temp_dir().join(format!("screenplay-routing-{}", Uuid::new_v4()));
        crate::video_studio_plugin::ensure_installed(&home)
            .await
            .unwrap();
        assert!(routing_guidance(&home, true).await.is_none());
        let host = willdeep_core::plugin::PluginHost::discover(&home).unwrap();
        host.approve("willdeep-video-studio", 1).await.unwrap();
        host.set_enabled("willdeep-video-studio", true)
            .await
            .unwrap()
            .unwrap();
        let guidance = routing_guidance(&home, true).await.unwrap();
        assert!(guidance.contains("list_mcp_tools with query screenplay,"));
        assert!(guidance.contains("500000 tokens / 64 model calls"));
        assert!(routing_guidance(&home, false).await.is_none());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn workspace_must_be_an_existing_host_root() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        assert_eq!(workspace(None, std::slice::from_ref(&root)).unwrap(), root);
        assert!(workspace(None, &[]).is_err());
        assert!(workspace(Some("/"), &[root]).is_err());
    }

    #[test]
    fn progress_omits_grants_and_counts_unsettled_reservations() {
        let value = progress(json!({"calls":[{"state":"completed","tokens":12},
            {"state":"reserved","reservation":20}], "tasks":[{"grant":"secret",
            "arguments":{"grant":"secret","step":"write_episode","episodeOrder":2},"state":"running"}]}));
        assert_eq!(value["tokens"], 12);
        assert_eq!(value["reservedTokens"], 20);
        assert_eq!(value["tasks"][0]["episodeOrder"], 2);
        assert!(!value.to_string().contains("secret"));
    }
}
