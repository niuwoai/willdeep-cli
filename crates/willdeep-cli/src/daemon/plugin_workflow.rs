//! Native desktop clients use the Runtime-owned plugin process and data for
//! durable workflows. Public tools only; step grants and arbitrary MCP are excluded.
use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
};

use serde::Deserialize;
use serde_json::Value;
use willdeep_core::plugin::PluginHost;

const PLUGIN: &str = "willdeep-video-studio";
const TOOLS: &[&str] = &[
    "screenplay.start",
    "screenplay.list",
    "screenplay.inspect",
    "screenplay.cancel",
    "screenplay.resume",
    "screenplay.budget",
    "screenplay.download",
    "production.start",
    "production.list",
    "production.inspect",
    "production.cancel",
    "production.resume",
    "production.budget",
    "production.pause",
    "production.music",
    "production.delivery",
    "production.file",
    "production.configuration",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    tool: String,
    arguments: Value,
    workspaces: Vec<PathBuf>,
}

pub(super) async fn call(home: &std::path::Path, params: Value) -> anyhow::Result<Value> {
    let request: Request = serde_json::from_value(params)?;
    anyhow::ensure!(
        TOOLS.contains(&request.tool.as_str()),
        "plugin_workflow_tool_denied"
    );
    anyhow::ensure!(
        request.arguments.is_object(),
        "plugin_workflow_arguments_invalid"
    );
    anyhow::ensure!(
        !request.workspaces.is_empty() && request.workspaces.len() <= 32,
        "plugin_workflow_workspaces_required"
    );
    let mut roots = Vec::new();
    for path in request.workspaces {
        let path = path.canonicalize()?;
        anyhow::ensure!(path.is_dir(), "plugin_workflow_workspace_invalid");
        roots.push(path);
    }
    roots.sort();
    roots.dedup();
    let plugin = PluginHost::discover(home)?;
    anyhow::ensure!(
        plugin.approval_gap(PLUGIN).await?.is_none(),
        "plugin_workflow_approval_required"
    );
    plugin.set_host_requests(Arc::new(crate::plugin_host_requests::HostRequests::new(
        home.to_path_buf(),
        home.join("config.toml"),
        Arc::new(RwLock::new(roots)),
    )));
    // mcp() requires the installed plugin to be enabled and approved. The
    // reverse control validates workspace ownership, budgets and request UUIDs.
    let mcp = plugin.mcp(PLUGIN).await?;
    mcp.call_tool_on("video-studio", &request.tool, request.arguments)
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn private_steps_and_arbitrary_tools_cannot_reach_plugin() {
        for tool in [
            "production.step",
            "screenplay.step",
            "fs.read",
            "production.unknown",
        ] {
            let error = call(
                std::path::Path::new("/nonexistent"),
                json!({
                    "tool": tool, "arguments": {}, "workspaces": []
                }),
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains("plugin_workflow_tool_denied"));
        }
    }

    #[tokio::test]
    async fn public_workflows_require_explicit_host_workspaces() {
        let error = call(
            std::path::Path::new("/nonexistent"),
            json!({
                "tool": "production.list", "arguments": {}, "workspaces": []
            }),
        )
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("plugin_workflow_workspaces_required")
        );
    }
}
