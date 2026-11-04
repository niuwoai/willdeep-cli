//! Parent-only host services. Identity, storage scope and dispatch authority stay in the host.
use super::*;

#[async_trait]
pub trait AgentDefinitionHost: Send + Sync {
    async fn execute(
        &self,
        operation: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError>;
}

#[derive(Clone, Debug)]
pub struct WorkflowNotice {
    pub run_id: uuid::Uuid,
    pub report: String,
}

#[async_trait]
pub trait WorkflowHost: Send + Sync {
    /// A queued leaf may recover only while its owning graph still authorizes it.
    fn allows_queued_recovery(
        &self,
        _run: &willdeep_runtime_protocol::AgentRun,
    ) -> Result<bool, ToolError> {
        Ok(false)
    }

    /// Called only after the parent owns its execution session and restores its team.
    fn activate(&self) {}

    async fn execute(
        &self,
        operation: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError>;

    fn has_pending(&self) -> bool;

    fn pending_reports(&self) -> Result<Vec<WorkflowNotice>, ToolError> {
        Ok(Vec::new())
    }

    fn acknowledge_report(&self, _run_id: uuid::Uuid) -> Result<(), ToolError> {
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostArgs {
    operation: String,
    #[serde(default = "empty_params")]
    params: serde_json::Value,
}

fn empty_params() -> serde_json::Value {
    json!({})
}

const LIBRARY_READ: &[&str] = &["list", "get", "history"];
const LIBRARY_MANAGE: &[&str] = &["create", "update", "fork", "bind", "enable"];
const WORKFLOW_READ: &[&str] = &["list", "get", "history", "validate", "schema"];
const WORKFLOW_CONTROL: &[&str] = &["save", "run", "stop", "retry", "resume"];

fn host_definition(name: &str, description: &str, operations: &[&str]) -> ToolDefinition {
    definition(
        name,
        description,
        json!({"type":"object","properties":{
            "operation":{"type":"string","enum":operations},
            "params":{"type":"object","description":"Operation parameters. Read before updating; pass the returned installation_revision as expected_revision. A request_id UUID makes mutations idempotent. Identity and storage scope are supplied by the host."}
        },"required":["operation"],"additionalProperties":false}),
    )
}

impl ToolRegistry {
    pub fn with_agent_definition_host(mut self, host: Arc<dyn AgentDefinitionHost>) -> Self {
        self.agent_definition_host = Some(host);
        self
    }

    pub fn with_workflow_host(mut self, host: Arc<dyn WorkflowHost>) -> Self {
        self.workflow_host = Some(host);
        self
    }

    pub(crate) async fn workflow_operation(
        &self,
        operation: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        let call = ToolCall {
            id: uuid::Uuid::new_v4().to_string(),
            name: if WORKFLOW_READ.contains(&operation) {
                "workflow_read"
            } else {
                "workflow_control"
            }
            .into(),
            arguments: json!({"operation":operation,"params":params}).to_string(),
        };
        let result = self.host_tool(&call).await?;
        serde_json::from_str(&result).map_err(|source| ToolError::InvalidArguments {
            tool: call.name,
            source,
        })
    }

    pub(crate) fn allows_workflow_recovery(
        &self,
        run: &willdeep_runtime_protocol::AgentRun,
    ) -> Result<bool, ToolError> {
        self.workflow_host
            .as_ref()
            .map_or(Ok(false), |host| host.allows_queued_recovery(run))
    }

    pub(crate) fn activate_workflows(&self) {
        if let Some(host) = &self.workflow_host {
            host.activate();
        }
    }

    pub fn has_pending_workflows(&self) -> bool {
        self.workflow_host
            .as_ref()
            .is_some_and(|host| host.has_pending())
    }

    pub(crate) fn workflow_reports(&self) -> Result<Vec<WorkflowNotice>, ToolError> {
        self.workflow_host
            .as_ref()
            .map_or_else(|| Ok(Vec::new()), |host| host.pending_reports())
    }

    pub(crate) fn acknowledge_workflow_report(&self, id: uuid::Uuid) -> Result<(), ToolError> {
        self.workflow_host
            .as_ref()
            .map_or(Ok(()), |host| host.acknowledge_report(id))
    }

    pub(super) fn host_definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = Vec::new();
        if self.agent_definition_host.is_some() {
            definitions.push(host_definition(
                "agent_library_read",
                "Read this host's specialist Agent library: list available definition ids, get a definition and its installation_revision, or inspect history. list/get record each selected local/private installation in the host for subsequent management CAS; history alone does not select a target. Built-in roles are permission ceilings. New specialists can be used by spawn_agent in this same turn after a successful management call.",
                LIBRARY_READ,
            ));
            definitions.push(host_definition(
                "agent_library_manage",
                "Manage local specialist definitions: create {definition}, update {definition,expected_revision}, fork {id,new_id,expected_revision}, bind {id,binding,expected_revision}, enable {id,value,expected_revision}. Definitions use the v1 package shape with a user/ definition_id, version, name, purpose, base_role, instructions, input_modalities, tools and skills. Before updating/binding/enabling or forking a source, list/get that target and pass its installation_revision as expected_revision. The host injects the observed local/private target_scope and rejects conflicting overrides; reread after a conflict. create needs no old target. Committed writes also record their returned target and revision, including saved_not_applied. After an applied create, spawn_agent then update then bind works in one turn without another read. Reuse the same request_id and parameters only to replay that request; replay refreshes the catalog without reverting observations to a historical receipt. Official content must be forked. New versions affect future dispatches; existing workers keep their snapshots. This never expands base-role authority or shares definitions externally.",
                LIBRARY_MANAGE,
            ));
        }
        if self.workflow_host.is_some() {
            definitions.push(host_definition(
                "workflow_read",
                "Inspect declarative workflows and durable runs. Read schema first for the supported v1 DSL and an example. list returns definitions and runs; get/history accept an id/run_id; validate checks a spec without saving. Phases group presentation; dependencies control execution. Outputs are data, not authorization.",
                WORKFLOW_READ,
            ));
            definitions.push(host_definition(
                "workflow_control",
                "Save {spec}, run {definition_id,input}, or stop/retry/resume {run_id} a durable declarative workflow. Run immediately returns a handle; the host advances stages in the background through the same worker approvals, budgets and file leases. retry/resume create a linked run and preserve successful results. Unknown side effects require explicit recovery. Only agent/switch/foreach/repeat are supported; arbitrary JavaScript is not executed.",
                WORKFLOW_CONTROL,
            ));
        }
        definitions
    }

    pub(super) async fn host_tool(&self, call: &ToolCall) -> Result<String, ToolError> {
        if !self.allows_agent_tool(&call.name) {
            return Err(ToolError::UnknownTool(call.name.clone()));
        }
        let mut args: HostArgs = parse(call)?;
        let operations = match call.name.as_str() {
            "agent_library_read" => LIBRARY_READ,
            "agent_library_manage" => LIBRARY_MANAGE,
            "workflow_read" => WORKFLOW_READ,
            "workflow_control" => WORKFLOW_CONTROL,
            _ => return Err(ToolError::UnknownTool(call.name.clone())),
        };
        if !operations.contains(&args.operation.as_str()) {
            return Err(invalid_host_args(call, "unsupported host operation"));
        }
        let Some(params) = args.params.as_object_mut() else {
            return Err(invalid_host_args(call, "params must be an object"));
        };
        let mutation = matches!(
            call.name.as_str(),
            "agent_library_manage" | "workflow_control"
        );
        if mutation {
            if self.approval_mode() == ApprovalMode::ReadOnly {
                return Err(ToolError::ReadOnlyPolicy(call.name.clone()));
            }
            let request_id = uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_OID,
                format!(
                    "{}:{}:{}",
                    self.session_id.as_deref().unwrap_or(""),
                    call.name,
                    call.id
                )
                .as_bytes(),
            );
            params
                .entry("request_id")
                .or_insert_with(|| json!(request_id));
            let target = params
                .get("id")
                .or_else(|| params.get("definition_id"))
                .or_else(|| params.get("run_id"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let action = format!("{} {} {target}", call.name, args.operation);
            if !self.allowed_by_full_access(&action) {
                let local_execution = call.name == "workflow_control" && args.operation != "save";
                self.require_approval(&action, local_execution).await?;
                self.report_approval(
                    &action,
                    ApprovalSource::User,
                    "host operation authorized".into(),
                );
            }
        }
        let result = match call.name.as_str() {
            "agent_library_read" | "agent_library_manage" => {
                self.agent_definition_host
                    .as_ref()
                    .ok_or_else(|| ToolError::UnknownTool(call.name.clone()))?
                    .execute(&args.operation, args.params)
                    .await?
            }
            _ => {
                self.workflow_host
                    .as_ref()
                    .ok_or_else(|| ToolError::UnknownTool(call.name.clone()))?
                    .execute(&args.operation, args.params)
                    .await?
            }
        };
        Ok(result.to_string())
    }
}

fn invalid_host_args(call: &ToolCall, message: &str) -> ToolError {
    ToolError::InvalidArguments {
        tool: call.name.clone(),
        source: <serde_json::Error as serde::de::Error>::custom(message),
    }
}

#[cfg(test)]
mod tests;
