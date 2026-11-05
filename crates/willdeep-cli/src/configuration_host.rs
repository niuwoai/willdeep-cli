//! Session-scoped configuration capability. The model never receives a file writer,
//! an MCP connection, or a configuration path override.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::OnceCell;
use willdeep_core::tools::{ConfigurationHost, ToolError};
use willdeep_core::{McpRegistry, McpServerConfig};

const COMPRESSOR_KEYS: &[&str] = &[
    "compressor_model",
    "compressor_profile",
    "compressor_context_window",
];
const SERVER: &str = "willdeep-config";

/// The bundled config service is a local Ruby process. On Windows the first
/// spawn pays the on-access scan of `ruby.exe` and the freshly written package,
/// which can exceed the interactive 10 second budget; every later call reuses
/// the connected service. Keep the stricter budget on the other platforms.
#[cfg(windows)]
const CONFIG_SERVICE_STARTUP_TIMEOUT_SECONDS: u64 = 30;
#[cfg(not(windows))]
const CONFIG_SERVICE_STARTUP_TIMEOUT_SECONDS: u64 = 10;

pub(crate) struct ChatConfigurationHost {
    home: PathBuf,
    path: PathBuf,
    parent: willdeep_core::provider::ProviderConfig,
    follow_main: Arc<dyn willdeep_core::provider::Provider>,
    service: OnceCell<Arc<McpRegistry>>,
    // Only the latest preview is committable, and only by this host instance.
    plan: Mutex<Option<Plan>>,
}

struct Plan {
    id: String,
    revision: String,
    edits: Value,
    description: String,
    state: PlanState,
}

enum PlanState {
    Prepared,
    Submitted,
    Completed(Value),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    edits: Vec<Edit>,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    op: String,
    section: String,
    key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Apply {
    plan_id: String,
}

impl ChatConfigurationHost {
    pub(crate) fn new(
        home: PathBuf,
        path: PathBuf,
        parent: willdeep_core::provider::ProviderConfig,
        follow_main: Arc<dyn willdeep_core::provider::Provider>,
    ) -> anyhow::Result<Self> {
        // Pin existing symlinks to the very file that was loaded, and normalize
        // relative --config paths before an MCP child chooses its own cwd.
        let path = std::path::absolute(path)?;
        let path = if path.exists() {
            path.canonicalize()?
        } else {
            path
        };
        Ok(Self {
            home: std::path::absolute(home)?,
            path,
            parent,
            follow_main,
            service: OnceCell::new(),
            plan: Mutex::new(None),
        })
    }

    async fn service(&self) -> Result<&Arc<McpRegistry>, ToolError> {
        self.service
            .get_or_try_init(|| async {
                let root = crate::config_plugin::prepare_builtin_runtime(&self.home)
                    .map_err(|_| unavailable())?;
                let ruby = crate::config_plugin::find_ruby().ok_or_else(unavailable)?;
                let server = McpServerConfig {
                    command: Some(ruby.display().to_string()),
                    args: vec![root.join("server/config_server.rb").display().to_string()],
                    env: [
                        ("WD_CONFIG_FILE".into(), self.path.display().to_string()),
                        ("WD_HOME".into(), self.home.display().to_string()),
                        (
                            "WD_CONFIG_BACKUP_DIR".into(),
                            self.home
                                .join("plugin-data/willdeep-config/backups")
                                .display()
                                .to_string(),
                        ),
                        ("WD_CONFIG_CHAT".into(), "1".into()),
                    ]
                    .into_iter()
                    .collect(),
                    startup_timeout_seconds: CONFIG_SERVICE_STARTUP_TIMEOUT_SECONDS,
                    enabled: true,
                    ..McpServerConfig::default()
                };
                McpRegistry::connect_in(
                    Some(&self.home),
                    &[(SERVER.into(), server)].into_iter().collect(),
                )
                .await
                .map(Arc::new)
                .map_err(|_| unavailable())
            })
            .await
    }

    async fn call(&self, operation: &str, arguments: Value) -> Result<Value, ToolError> {
        let result = self
            .service()
            .await?
            .call_tool_on(SERVER, &format!("config.{operation}"), arguments)
            .await
            // Transport errors may contain subprocess output; never relay them.
            .map_err(|_| unavailable())?;
        let value = result["content"][0]["text"]
            .as_str()
            .ok_or_else(unavailable)?;
        serde_json::from_str(value).map_err(|_| unavailable())
    }

    async fn preview(&self, params: Value) -> Result<Value, ToolError> {
        let preview: Preview = serde_json::from_value(params).map_err(|_| invalid())?;
        if preview.edits.is_empty()
            || preview.edits.iter().any(|edit| {
                edit.section != "agent"
                    || !COMPRESSOR_KEYS.contains(&edit.key.as_str())
                    || !matches!(edit.op.as_str(), "set" | "unset")
                    || (edit.op == "set" && edit.value.is_none())
                    || (edit.op == "unset" && edit.value.is_some())
            })
        {
            return Err(invalid());
        }
        let edits = serde_json::to_value(preview.edits).map_err(|_| invalid())?;
        let mut result = self.call("plan", json!({"edits":edits})).await?;
        if result["ok"] != true {
            // The backend's parse errors may quote secret source lines.
            return Ok(rejected("invalid_configuration"));
        }
        let candidate = result["candidate"].as_str().ok_or_else(unavailable)?;
        let parsed: crate::config::ConfigFile = match toml::from_str(candidate) {
            Ok(file) => file,
            Err(_) => return Ok(rejected("runtime_validation_failed")),
        };
        if crate::config::validate(&parsed, &self.path).is_err() {
            return Ok(rejected("runtime_validation_failed"));
        }
        if crate::harness::build_compression_settings(
            &crate::config::LoadedConfig { file: parsed },
            &self.parent,
            self.follow_main.clone(),
        )
        .is_err()
        {
            return Ok(rejected("compressor_configuration_invalid"));
        }
        let revision = result["revision"]
            .as_str()
            .ok_or_else(unavailable)?
            .to_owned();
        let changes = result["changes"].take();
        let id = uuid::Uuid::new_v4().to_string();
        let description = format!("configuration.apply {}\n{}", self.path.display(), changes);
        *self.plan.lock().unwrap_or_else(|e| e.into_inner()) = Some(Plan {
            id: id.clone(),
            revision,
            edits,
            description,
            state: PlanState::Prepared,
        });
        Ok(
            json!({"ok":true,"plan_id":id,"path":self.path,"changes":changes,
            "effective":"next_runtime_turn","current_turn_unchanged":true}),
        )
    }

    async fn apply(&self, params: Value) -> Result<Value, ToolError> {
        let args: Apply = serde_json::from_value(params).map_err(|_| invalid())?;
        let (edits, revision) = {
            let mut guard = self.plan.lock().unwrap_or_else(|e| e.into_inner());
            let plan = guard
                .as_mut()
                .filter(|plan| plan.id == args.plan_id)
                .ok_or_else(invalid)?;
            match &plan.state {
                PlanState::Completed(receipt) => return Ok(receipt.clone()),
                PlanState::Submitted => return Ok(rejected("outcome_unknown_repreview_required")),
                PlanState::Prepared => {}
            }
            plan.state = PlanState::Submitted;
            (plan.edits.clone(), plan.revision.clone())
        };
        // Submitted remains consumed on transport failure. Never retry a write
        // whose response was lost; a fresh snapshot/preview is required.
        let result = self
            .call(
                "apply",
                json!({"edits":edits,"confirm":true,"expectedRevision":revision}),
            )
            .await?;
        let receipt = if result["ok"] == true {
            json!({"ok":true,"plan_id":args.plan_id,"path":self.path,
                "backupPath":result["backupPath"],"changes":result["changes"],
                "effective":"next_runtime_turn","current_turn_unchanged":true})
        } else if result["error"] == "conflict" {
            rejected("conflict_repreview_required")
        } else {
            rejected("save_failed")
        };
        let mut guard = self.plan.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(plan) = guard.as_mut().filter(|plan| plan.id == args.plan_id) {
            plan.state = PlanState::Completed(receipt.clone());
        }
        Ok(receipt)
    }
}

#[async_trait]
impl ConfigurationHost for ChatConfigurationHost {
    fn approval_description(&self, plan_id: &str) -> Result<String, ToolError> {
        self.plan
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|plan| plan.id == plan_id)
            .map(|plan| plan.description.clone())
            .ok_or_else(invalid)
    }

    async fn execute(&self, operation: &str, params: Value) -> Result<Value, ToolError> {
        match operation {
            "plan" => self.preview(params).await,
            "apply" => self.apply(params).await,
            "schema" | "snapshot" => {
                if !params.as_object().is_some_and(|object| object.is_empty()) {
                    return Err(invalid());
                }
                let result = self.call(operation, json!({})).await?;
                if result["ok"] != true || !result["parseError"].is_null() {
                    return Ok(rejected("invalid_configuration"));
                }
                Ok(safe_projection(result, &self.path))
            }
            _ => Err(invalid()),
        }
    }
}

fn safe_projection(result: Value, path: &Path) -> Value {
    let sections: Vec<Value> = result["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|section| {
            let name = section["name"].as_str()?;
            if name != "agent" && !name.starts_with("providers.") {
                return None;
            }
            let fields: Vec<Value> = section["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|field| {
                    field["key"].as_str().is_some_and(|key| {
                        if name == "agent" {
                            COMPRESSOR_KEYS.contains(&key)
                        } else {
                            matches!(key, "model" | "context_window")
                        }
                    })
                })
                .cloned()
                .collect();
            Some(json!({"name":name,"fields":fields}))
        })
        .collect();
    json!({"ok":true,"path":path,"exists":result["exists"],"sections":sections,
        "editable_section":"agent","editable_keys":COMPRESSOR_KEYS})
}

fn rejected(code: &str) -> Value {
    json!({"ok":false,"error":code})
}

fn invalid() -> ToolError {
    ToolError::InvalidArguments {
        tool: "configuration".into(),
        source: <serde_json::Error as serde::de::Error>::custom(
            "invalid configuration operation or expired plan",
        ),
    }
}

fn unavailable() -> ToolError {
    ToolError::Io(std::io::Error::other(
        "bundled configuration service unavailable",
    ))
}

#[cfg(test)]
mod tests;
