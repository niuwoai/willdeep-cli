//! Chat and page controls share the Runtime-owned full-film production queue.
use anyhow::Context;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use uuid::Uuid;
use willdeep_core::mcp::HostRequestError;
use willdeep_runtime_protocol::production::{ProductionStart, SeriesSpec};

pub(crate) const CONTROL: &str = "willdeep/production/control";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation: String,
    #[serde(default)]
    arguments: Value,
}

pub(crate) async fn routing_guidance(home: &Path, allowed: bool) -> Option<String> {
    if !allowed {
        return None;
    }
    let host = willdeep_core::plugin::PluginHost::discover(home).ok()?;
    let package = host.package("willdeep-video-studio").ok()?;
    if !host.is_enabled("willdeep-video-studio").await
        || host
            .approval_gap("willdeep-video-studio")
            .await
            .ok()?
            .is_some()
        || !package
            .manifest
            .as_ref()?
            .commands
            .iter()
            .any(|command| command.id == "production.start")
    {
        return None;
    }
    Some("\n\n# Complete-film automatic production\nFor a rough-topic request to produce a complete series including 图片、配音、音乐、视频、审核、修订、成片, use list_mcp_tools query production and the exact discovered production.start tool. Preserve the requested episode count (commonly 20-80); the visible default is 40, never silently substitute the three-episode screenplay pilot. Keep the rough topic as brief; the workflow decides missing creative details. Require a human-approved cumulative media budget or an existing approved preset; an old screenplay token allowance does not authorize new media purchases. Ask only for missing run conditions, not an outline. Generate one UUID requestID and reuse it unchanged on transport retries. Use production.inspect/list for progress, not another start. Report real IDs, status, pauses and usage honestly. A script or one clip is not a completed film. Do not inspect repository source, write helper loops or substitute manual per-episode tools for the durable workflow. Screenplay-only requests must not purchase media.\n".into())
}

fn roots(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots: Vec<_> = paths
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
        _ => anyhow::bail!("production_workspace_required"),
    };
    anyhow::ensure!(roots.contains(&path), "production_workspace_denied");
    Ok(path)
}
fn permitted(value: &Value, roots: &[PathBuf]) -> bool {
    value["input"]["workspace"]
        .as_str()
        .is_some_and(|value| workspace(Some(value), roots).is_ok())
}
pub(crate) async fn control(
    home: &Path,
    workspaces: &[PathBuf],
    origin: Option<Uuid>,
    params: Value,
) -> Result<Value, HostRequestError> {
    let request: Request = serde_json::from_value(params)
        .map_err(|error| HostRequestError::invalid_params(error.to_string()))?;
    dispatch(home, &roots(workspaces), origin, request)
        .await
        .map_err(|error| {
            HostRequestError::failed(willdeep_core::judge::redact_credentials(&format!(
                "{error:#}"
            )))
        })
}
fn resume_parameters(id: Uuid, args: &Value) -> Value {
    let mut params = json!({"id":id,"detail":false});
    for key in [
        "retryUnknownTextRequests",
        "retryUnknownImageRequests",
        "historicalUsageHold",
        "imageRetryPolicy",
    ] {
        if let Some(value) = args.get(key) {
            params[key] = value.clone();
        }
    }
    params
}

async fn dispatch(
    home: &Path,
    roots: &[PathBuf],
    origin: Option<Uuid>,
    request: Request,
) -> anyhow::Result<Value> {
    let args = request.arguments;
    let operation = request.operation.as_str();
    if operation == "list" {
        let value =
            crate::daemon::schedule_operation(home, "production.list", json!({}), None).await?;
        let mut items: Vec<_> = value
            .as_array()
            .context("production_list_invalid")?
            .iter()
            .filter(|item| permitted(item, roots))
            .cloned()
            .collect();
        items.sort_by_key(|item| std::cmp::Reverse(item["createdAt"].as_u64().unwrap_or(0)));
        return Ok(json!({"ok":true,"workflows":items,"workspaces":roots,
            "defaults":{"episodeCount":40,"episodeSeconds":90,"aspect":"portrait","requiresMediaBudget":true}}));
    }
    anyhow::ensure!(
        [
            "start", "inspect", "cancel", "resume", "budget", "pause", "music"
        ]
        .contains(&operation),
        "production_operation_denied"
    );
    let (params, request_id) = if operation == "start" {
        let id: Uuid = serde_json::from_value(args["requestID"].clone())?;
        let default = SeriesSpec::default();
        let input = ProductionStart {
            origin_session_id: origin,
            brief: args["brief"].as_str().map(str::to_owned),
            source_screenplay_id: serde_json::from_value(
                args.get("sourceScreenplayID")
                    .cloned()
                    .unwrap_or(Value::Null),
            )?,
            workspace: workspace(args["workspace"].as_str(), roots)?,
            spec: serde_json::from_value(
                json!({"episode_count":args.get("episodeCount").cloned().unwrap_or(json!(default.episode_count)),
                "episode_seconds":args.get("episodeSeconds").cloned().unwrap_or(json!(default.episode_seconds)),
                "aspect":args.get("aspect").cloned().unwrap_or(json!(default.aspect))}),
            )?,
            limits: serde_json::from_value(args["limits"].clone())?,
            max_model_calls_per_episode: serde_json::from_value(
                args.get("maxModelCallsPerEpisode")
                    .cloned()
                    .unwrap_or(Value::Null),
            )?,
        };
        input.validate().map_err(anyhow::Error::msg)?;
        (serde_json::to_value(input)?, Some(id))
    } else {
        let id: Uuid = serde_json::from_value(args["id"].clone())?;
        let current = crate::daemon::schedule_operation(
            home,
            "production.status",
            json!({"id":id,"detail":false}),
            None,
        )
        .await?;
        anyhow::ensure!(permitted(&current, roots), "production_workspace_denied");
        let params = if operation == "budget" {
            json!({"id":id,"expected":args["expected"],"limits":args["limits"],"detail":false})
        } else if operation == "music" {
            json!({"id":id,"deferred":args["deferred"],"detail":false})
        } else if operation == "resume" {
            resume_parameters(id, &args)
        } else {
            json!({"id":id,"detail":args.get("detail").cloned().unwrap_or(json!(false))})
        };
        (params, None)
    };
    let operation = if operation == "inspect" {
        "status"
    } else {
        operation
    };
    let mut workflow = crate::daemon::schedule_operation(
        home,
        &format!("production.{operation}"),
        params,
        request_id,
    )
    .await?;
    workflow["conversationUsage"] =
        crate::daemon::production::accounting::conversation(home, &workflow);
    Ok(json!({"ok":true,"workflow":workflow}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_forwards_one_exact_image_recovery_without_an_implicit_override() {
        let args = json!({"retryUnknownImageRequests":[{"requestID":Uuid::new_v4(),"subject":"d".repeat(64)}],"overrideUnknown":true});
        let forwarded = resume_parameters(Uuid::new_v4(), &args);
        assert_eq!(
            forwarded["retryUnknownImageRequests"],
            args["retryUnknownImageRequests"]
        );
        assert!(forwarded.get("overrideUnknown").is_none());
    }
    #[test]
    fn requests_are_scoped_to_registered_host_roots() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        assert_eq!(workspace(None, std::slice::from_ref(&root)).unwrap(), root);
        assert!(workspace(Some("/"), &[root]).is_err());
        assert!(workspace(None, &[]).is_err());
    }
    #[test]
    fn resume_forwards_the_exact_authorized_image_policy_with_existing_recovery_fields() {
        let id = Uuid::new_v4();
        let args = json!({"imageRetryPolicy":{"mode":"per_material_v1","legacyBindings":[{"requestID":Uuid::new_v4(),"subject":"a".repeat(64),"fileName":"old.png"}]},
            "historicalUsageHold":{"tokens":12},"retryUnknownTextRequests":[Uuid::new_v4()],"unrelated":"must not forward"});
        let forwarded = resume_parameters(id, &args);
        assert_eq!(forwarded["id"], json!(id));
        for key in [
            "imageRetryPolicy",
            "historicalUsageHold",
            "retryUnknownTextRequests",
        ] {
            assert_eq!(forwarded[key], args[key]);
        }
        assert_eq!(forwarded["detail"], false);
        assert!(forwarded.get("unrelated").is_none());
    }
}
