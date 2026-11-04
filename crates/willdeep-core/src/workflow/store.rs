//! Parent-scoped durable state. One advisory file lock covers each transaction;
//! a run and its intents are committed together in one atomic JSON replacement.
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use willdeep_runtime_protocol::{AgentRun, AgentStatus, AgentUsageSummary};

use super::{engine, schema, validation, *};
mod io;
#[cfg(test)]
mod snapshot_tests;

/// Host metadata shares the run transaction, but has its own absolute ceiling.
const MAX_HOST_SNAPSHOT_BYTES: usize = WORKFLOW_MAX_RUN_BYTES / 4;

#[derive(Clone, Debug)]
pub struct WorkflowStore {
    root: PathBuf,
    parent_session_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Operation {
    Retry(Uuid),
    Resume(Uuid),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StoredRun {
    pub version: u32,
    pub run: WorkflowRun,
    pub intents: BTreeMap<Uuid, WorkflowReadyTask>,
    pub expansions: BTreeMap<String, engine::Expansion>,
    /// Fingerprints retain exact replay/conflict semantics even for rejected,
    /// oversized values which must not be stored in the run itself.
    #[serde(default)]
    pub settlements: BTreeMap<Uuid, String>,
    pub operation: Option<Operation>,
    /// Host-owned catalog state, never serialized through public WorkflowRun/tool parameters.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "decode_host_snapshot"
    )]
    pub host_snapshot: Option<Value>,
}

// A present JSON null is still a pinned value; only an absent field means unpinned.
fn decode_host_snapshot<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

impl WorkflowStore {
    pub fn new(home: impl AsRef<Path>, parent_session_id: Uuid) -> Self {
        Self {
            root: home
                .as_ref()
                .join("workflows")
                .join(parent_session_id.to_string()),
            parent_session_id,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn save_definition(&self, spec: &WorkflowSpecV1) -> WorkflowResult<WorkflowSpecV1> {
        validate_spec(spec, &WorkflowLimits::default())?;
        let _lock = self.lock()?;
        let path = self.definition_path(&spec.id);
        if let Some(existing) =
            io::read::<WorkflowSpecV1>(&path, WorkflowLimits::default().max_definition_bytes)?
        {
            if existing != *spec {
                return Err(WorkflowError::State(format!(
                    "definition {} is immutable; use a new id for different content",
                    spec.id
                )));
            }
            return Ok(existing);
        }
        io::write(&path, spec, WorkflowLimits::default().max_definition_bytes)?;
        Ok(spec.clone())
    }

    pub fn save(&self, spec: &WorkflowSpecV1) -> WorkflowResult<WorkflowSpecV1> {
        self.save_definition(spec)
    }

    pub fn get_definition(&self, id: &str) -> WorkflowResult<Option<WorkflowSpecV1>> {
        validation::identifier(id, WorkflowLimits::default().max_name_bytes)?;
        let _lock = self.lock()?;
        self.read_definition(id)
    }

    pub fn list_definitions(&self) -> WorkflowResult<Vec<WorkflowSpecV1>> {
        let _lock = self.lock()?;
        let mut definitions = Vec::new();
        for path in io::json_files(&self.root.join("definitions"))? {
            let spec: WorkflowSpecV1 =
                io::read(&path, WorkflowLimits::default().max_definition_bytes)?.ok_or_else(
                    || WorkflowError::Storage("definition disappeared under lock".into()),
                )?;
            validate_spec(&spec, &WorkflowLimits::default())?;
            if path != self.definition_path(&spec.id) {
                return Err(WorkflowError::Storage(
                    "definition filename mismatch".into(),
                ));
            }
            definitions.push(spec);
        }
        definitions.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(definitions)
    }

    pub fn list_definition(&self) -> WorkflowResult<Vec<WorkflowSpecV1>> {
        self.list_definitions()
    }

    pub fn start(
        &self,
        definition_id: &str,
        input: Value,
        limits: WorkflowLimits,
    ) -> WorkflowResult<WorkflowRun> {
        self.start_with_request(definition_id, input, limits, Uuid::new_v4())
    }

    /// The request UUID is the run UUID. Replays compare the complete admitted
    /// payload, including the lowered limits, before returning the original run.
    pub fn start_with_request(
        &self,
        definition_id: &str,
        input: Value,
        limits: WorkflowLimits,
        request_id: Uuid,
    ) -> WorkflowResult<WorkflowRun> {
        validation::identifier(definition_id, WorkflowLimits::default().max_name_bytes)?;
        validation::validate_limits(&limits)?;
        check_value(&input, limits.max_value_bytes)?;
        let _lock = self.lock()?;
        if let Some(record) = self.read_run(request_id)? {
            if record.operation.is_some()
                || record.run.definition_id != definition_id
                || record.run.input != input
                || record.run.limits != limits
            {
                return Err(request_conflict(request_id));
            }
            return Ok(record.run);
        }
        let spec = self
            .read_definition(definition_id)?
            .ok_or_else(|| WorkflowError::State(format!("definition {definition_id} not found")))?;
        validate_spec(&spec, &limits)?;
        if let Some(schema) = &spec.input_schema {
            schema::validate_output(schema, &input)?;
        }
        let run = WorkflowRun {
            id: request_id,
            revision: 1,
            parent_session_id: self.parent_session_id,
            definition_id: definition_id.into(),
            spec,
            input,
            limits,
            status: WorkflowRunStatus::Running,
            nodes: Vec::new(),
            output: None,
            report: None,
            error: None,
            created_at_ms: now_ms(),
            finished_at_ms: None,
            source_run_id: None,
            acknowledged: false,
        };
        let record = engine::fresh_record(run);
        self.write_run(&record)?;
        Ok(record.run)
    }

    pub fn get(&self, id: Uuid) -> WorkflowResult<Option<WorkflowRun>> {
        let _lock = self.lock()?;
        Ok(self.read_run(id)?.map(|r| r.run))
    }

    pub fn get_run(&self, id: Uuid) -> WorkflowResult<Option<WorkflowRun>> {
        self.get(id)
    }

    /// Host-only metadata. A missing run is an error; a legacy unpinned run returns None.
    pub fn host_snapshot(&self, run_id: Uuid) -> WorkflowResult<Option<Value>> {
        let _lock = self.lock()?;
        Ok(self.required_run(run_id)?.host_snapshot)
    }

    /// Pin exactly once under the run transaction lock. Equal retries are harmless;
    /// a different value cannot replace the original, including across retry/resume.
    /// Host metadata does not change the public run revision.
    pub fn pin_host_snapshot(&self, run_id: Uuid, snapshot: Value) -> WorkflowResult<()> {
        let _lock = self.lock()?;
        let mut record = self.required_run(run_id)?;
        if let Some(existing) = &record.host_snapshot {
            return if existing == &snapshot {
                Ok(())
            } else {
                Err(WorkflowError::State(format!(
                    "run {run_id} host snapshot is already pinned to different content"
                )))
            };
        }
        check_value(&snapshot, MAX_HOST_SNAPSHOT_BYTES)?;
        record.host_snapshot = Some(snapshot);
        self.write_run(&record)
    }

    pub fn list_runs(&self) -> WorkflowResult<Vec<WorkflowRun>> {
        let _lock = self.lock()?;
        let mut runs = Vec::new();
        for path in io::json_files(&self.root.join("runs"))? {
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| WorkflowError::Storage("invalid run filename".into()))?;
            runs.push(self.required_run(id)?.run);
        }
        runs.sort_by_key(|r| (r.created_at_ms, r.id));
        Ok(runs)
    }

    pub fn poll(&self, id: Uuid) -> WorkflowResult<WorkflowPoll> {
        let _lock = self.lock()?;
        let mut record = self.required_run(id)?;
        let previous = record.run.clone();
        engine::tick(&mut record)?;
        if record.run != previous {
            record.run.revision = previous
                .revision
                .checked_add(1)
                .ok_or_else(|| WorkflowError::State("workflow revision exhausted".into()))?;
        }
        self.write_run(&record)?;
        // Never expose an intent before the transaction above has committed.
        let ready = if record.run.status == WorkflowRunStatus::Running {
            unbound(&record)
        } else {
            Vec::new()
        };
        let cancel_agent_run_ids = if record.run.status == WorkflowRunStatus::Stopping {
            record
                .run
                .nodes
                .iter()
                .filter(|n| !n.status.is_terminal())
                .filter_map(|n| n.agent_run.as_ref().map(|r| r.id))
                .collect()
        } else {
            Vec::new()
        };
        Ok(WorkflowPoll {
            run: record.run,
            ready,
            cancel_agent_run_ids,
        })
    }

    /// Read-only: this does not tick the graph or allocate request identities.
    /// After restart the driver must reconcile these requests with its dispatch
    /// ledger before deciding whether any side effect is safe to execute.
    pub fn pending(&self, id: Uuid) -> WorkflowResult<Vec<WorkflowReadyTask>> {
        let _lock = self.lock()?;
        Ok(unbound(&self.required_run(id)?))
    }

    /// Accepts queued/running updates and idempotently binds a ledger record.
    /// Terminal observations go through complete_task/fail_task so structured
    /// output cannot be accidentally bypassed by a status refresh.
    pub fn record_dispatch(
        &self,
        id: Uuid,
        request: Uuid,
        agent_run: AgentRun,
    ) -> WorkflowResult<WorkflowRun> {
        self.update(id, |record| {
            let index = validate_agent(record, request, &agent_run)?;
            let node = &mut record.run.nodes[index];
            if node.status.is_terminal() {
                if node
                    .agent_run
                    .as_ref()
                    .is_some_and(|r| r.id == agent_run.id)
                {
                    return Ok(());
                }
                return Err(WorkflowError::State(
                    "terminal task cannot be rebound".into(),
                ));
            }
            if let Some(previous) = &node.agent_run {
                if terminal_agent(previous.status) {
                    if previous.status == agent_run.status || !terminal_agent(agent_run.status) {
                        return Ok(());
                    }
                    return Err(WorkflowError::State(
                        "conflicting terminal AgentRun observations".into(),
                    ));
                }
                if previous.status != AgentStatus::Queued && agent_run.status == AgentStatus::Queued
                {
                    return Ok(());
                }
            }
            let mut agent_run = agent_run;
            // Reports are admitted by the terminal result methods. Binding a
            // ledger entry must remain possible even when its report is invalid.
            if check_report(&agent_run, &record.run.limits).is_err() {
                agent_run.report = None;
            }
            node.started_at_ms = agent_run.started_at_ms.or(node.started_at_ms);
            node.usage = agent_run.usage.clone();
            if record.run.status != WorkflowRunStatus::Stopping {
                node.status = if agent_run.status == AgentStatus::Queued {
                    WorkflowNodeStatus::Dispatched
                } else {
                    WorkflowNodeStatus::Running
                };
            }
            node.agent_run = Some(agent_run);
            Ok(())
        })
    }

    /// Invalid output becomes a durable task failure, blocking its downstreams.
    /// The host still receives Ok(run), because the terminal observation was
    /// recorded successfully; inspect node.error for schema/size diagnostics.
    pub fn complete_task(
        &self,
        id: Uuid,
        request: Uuid,
        result: WorkflowTaskResult,
    ) -> WorkflowResult<WorkflowRun> {
        self.update(id, |record| {
            let index = validate_agent(record, request, &result.agent_run)?;
            if result.agent_run.status != AgentStatus::Completed {
                return Err(WorkflowError::State(
                    "complete_task requires a completed AgentRun".into(),
                ));
            }
            if settle_replay(record, request, "complete", &result)? {
                return Ok(());
            }
            if record.run.nodes[index].status.is_terminal() {
                return Err(WorkflowError::State(
                    "terminal task result conflicts with recorded result".into(),
                ));
            }
            let intent = &record.intents[&request];
            let validation = check_value(&result.output, record.run.limits.max_value_bytes)
                .and_then(|()| {
                    if let Some(schema) = &intent.output_schema {
                        schema::validate_output(schema, &result.output)
                    } else {
                        Ok(())
                    }
                })
                .and_then(|()| check_report(&result.agent_run, &record.run.limits));
            let node = &mut record.run.nodes[index];
            let mut agent = result.agent_run;
            if agent
                .report
                .as_ref()
                .is_some_and(|s| s.len() > record.run.limits.max_report_bytes)
            {
                agent.report = None;
            }
            apply_agent(node, agent);
            match validation {
                Ok(()) => {
                    node.status = WorkflowNodeStatus::Completed;
                    node.output = Some(result.output);
                }
                Err(error) => {
                    node.status = WorkflowNodeStatus::Failed;
                    node.error = Some(format!("{}: {error}", node.path));
                }
            }
            Ok(())
        })
    }

    pub fn fail_task(
        &self,
        id: Uuid,
        request: Uuid,
        agent_run: AgentRun,
    ) -> WorkflowResult<WorkflowRun> {
        self.update(id, |record| {
            let index = validate_agent(record, request, &agent_run)?;
            if !terminal_agent(agent_run.status) || agent_run.status == AgentStatus::Completed {
                return Err(WorkflowError::State(
                    "fail_task requires a non-success terminal AgentRun".into(),
                ));
            }
            if settle_replay(record, request, "fail", &agent_run)? {
                return Ok(());
            }
            if record.run.nodes[index].status.is_terminal() {
                return Err(WorkflowError::State(
                    "terminal task failure conflicts with recorded result".into(),
                ));
            }
            let report_error = check_report(&agent_run, &record.run.limits).err();
            let mut agent_run = agent_run;
            if report_error.is_some() {
                agent_run.report = None;
            }
            let node = &mut record.run.nodes[index];
            node.status = match agent_run.status {
                AgentStatus::Interrupted => WorkflowNodeStatus::Interrupted,
                AgentStatus::Cancelled => WorkflowNodeStatus::Cancelled,
                _ => WorkflowNodeStatus::Failed,
            };
            node.error = Some(format!(
                "{}: AgentRun {} ended with {:?}{}",
                node.path,
                agent_run.id,
                agent_run.status,
                report_error.map(|e| format!("; {e}")).unwrap_or_default()
            ));
            apply_agent(node, agent_run);
            Ok(())
        })
    }

    /// Use only after reconciliation proves no AgentRun was accepted. For an
    /// uncertain accepted side effect, bind an Interrupted AgentRun instead.
    pub fn reject_task(
        &self,
        id: Uuid,
        request: Uuid,
        error: impl Into<String>,
    ) -> WorkflowResult<WorkflowRun> {
        let error = error.into();
        self.update(id, |record| {
            if error.len() > record.run.limits.max_report_bytes {
                return Err(WorkflowError::Limit(
                    "task rejection error exceeds report bytes".into(),
                ));
            }
            let index = request_index(record, request)?;
            if settle_replay(record, request, "reject", &error)? {
                return Ok(());
            }
            let node = &mut record.run.nodes[index];
            if node.agent_run.is_some() {
                return Err(WorkflowError::State(
                    "accepted dispatch must settle using AgentRun".into(),
                ));
            }
            if node.status.is_terminal() {
                if node.error.as_ref() == Some(&error) {
                    return Ok(());
                }
                return Err(WorkflowError::State(
                    "terminal task rejection conflicts".into(),
                ));
            }
            node.status = if record.run.status == WorkflowRunStatus::Stopping {
                WorkflowNodeStatus::Cancelled
            } else {
                WorkflowNodeStatus::Failed
            };
            node.error = Some(error);
            node.finished_at_ms = Some(now_ms());
            Ok(())
        })
    }

    pub fn stop(&self, id: Uuid) -> WorkflowResult<WorkflowRun> {
        self.update(id, |record| {
            if !record.run.status.is_terminal() {
                record.run.status = WorkflowRunStatus::Stopping;
                engine::tick(record)?;
            }
            Ok(())
        })
    }

    pub fn retry(&self, source: Uuid) -> WorkflowResult<WorkflowRun> {
        self.retry_with_request(source, Uuid::new_v4())
    }
    pub fn resume(&self, source: Uuid) -> WorkflowResult<WorkflowRun> {
        self.resume_with_request(source, Uuid::new_v4())
    }
    pub fn retry_with_request(&self, source: Uuid, request: Uuid) -> WorkflowResult<WorkflowRun> {
        self.restart(source, request, Operation::Retry(source))
    }
    pub fn resume_with_request(&self, source: Uuid, request: Uuid) -> WorkflowResult<WorkflowRun> {
        self.restart(source, request, Operation::Resume(source))
    }

    fn restart(
        &self,
        source: Uuid,
        request: Uuid,
        operation: Operation,
    ) -> WorkflowResult<WorkflowRun> {
        let _lock = self.lock()?;
        if let Some(existing) = self.read_run(request)? {
            if existing.operation.as_ref() != Some(&operation) {
                return Err(request_conflict(request));
            }
            return Ok(existing.run);
        }
        let old = self.required_run(source)?;
        if !old.run.status.is_terminal() {
            return Err(WorkflowError::State(
                "retry/resume requires a terminal source run".into(),
            ));
        }
        let mut record = old.clone();
        record.operation = Some(operation);
        record.intents.clear();
        record.settlements.clear();
        record.run.id = request;
        record.run.revision = 1;
        record.run.source_run_id = Some(source);
        record.run.status = WorkflowRunStatus::Running;
        record.run.output = None;
        record.run.report = None;
        record.run.error = None;
        record.run.acknowledged = false;
        record.run.finished_at_ms = None;
        record.run.created_at_ms = now_ms();
        for node in &mut record.run.nodes {
            let old_id = node.id;
            node.id = Uuid::new_v4();
            node.attempt_id = Uuid::new_v4();
            node.request_id = None;
            node.usage = AgentUsageSummary::default();
            if node.status == WorkflowNodeStatus::Completed {
                node.reused_from = Some(WorkflowReuseSource {
                    run_id: source,
                    node_instance_id: old_id,
                });
            } else {
                node.status = WorkflowNodeStatus::Pending;
                node.agent_run = None;
                node.output = None;
                node.report = None;
                node.error = None;
                node.created_at_ms = now_ms();
                node.started_at_ms = None;
                node.finished_at_ms = None;
                node.reused_from = None;
            }
        }
        self.write_run(&record)?;
        Ok(record.run)
    }

    pub fn aggregate_report(&self, id: Uuid) -> WorkflowResult<Option<WorkflowReport>> {
        let _lock = self.lock()?;
        let record = self.required_run(id)?;
        if !record.run.status.is_terminal() {
            return Ok(None);
        }
        let mut report = format!(
            "Workflow {}: {:?}",
            record.run.definition_id, record.run.status
        );
        if let Some(error) = &record.run.error {
            report.push_str(&format!("\n{error}"));
        }
        let mut usage = AgentUsageSummary::default();
        for node in &record.run.nodes {
            if let Some(text) = &node.report {
                report.push_str(&format!("\n\n{}\n{text}", node.path));
            }
            if let Some(error) = &node.error {
                report.push_str(&format!("\n{}: {error}", node.path));
            }
            if node.reused_from.is_none() {
                add_usage(&mut usage, &node.usage);
            }
        }
        if report.len() > record.run.limits.max_run_bytes {
            return Err(WorkflowError::Limit(
                "aggregate report exceeds run bytes".into(),
            ));
        }
        Ok(Some(WorkflowReport {
            run_id: id,
            status: record.run.status,
            output: record.run.output,
            report,
            usage,
            acknowledged: record.run.acknowledged,
        }))
    }

    /// The driver calls this only after durably saving the parent's report.
    pub fn acknowledge_report(&self, id: Uuid) -> WorkflowResult<WorkflowRun> {
        self.update(id, |record| {
            if !record.run.status.is_terminal() {
                return Err(WorkflowError::State(
                    "only terminal reports can be acknowledged".into(),
                ));
            }
            record.run.acknowledged = true;
            Ok(())
        })
    }

    fn update(
        &self,
        id: Uuid,
        change: impl FnOnce(&mut StoredRun) -> WorkflowResult<()>,
    ) -> WorkflowResult<WorkflowRun> {
        let _lock = self.lock()?;
        let mut record = self.required_run(id)?;
        let previous = record.run.clone();
        change(&mut record)?;
        if record.run != previous {
            record.run.revision = previous
                .revision
                .checked_add(1)
                .ok_or_else(|| WorkflowError::State("workflow revision exhausted".into()))?;
        }
        self.write_run(&record)?;
        Ok(record.run)
    }
    fn lock(&self) -> WorkflowResult<File> {
        io::lock(&self.root)
    }
    fn definition_path(&self, id: &str) -> PathBuf {
        self.root.join("definitions").join(format!("{id}.json"))
    }
    fn read_definition(&self, id: &str) -> WorkflowResult<Option<WorkflowSpecV1>> {
        let spec: Option<WorkflowSpecV1> = io::read(
            &self.definition_path(id),
            WorkflowLimits::default().max_definition_bytes,
        )?;
        if let Some(spec) = &spec {
            validate_spec(spec, &WorkflowLimits::default())?;
            if spec.id != id {
                return Err(WorkflowError::Storage(
                    "definition identity mismatch".into(),
                ));
            }
        }
        Ok(spec)
    }
    fn run_path(&self, id: Uuid) -> PathBuf {
        self.root.join("runs").join(format!("{id}.json"))
    }
    fn required_run(&self, id: Uuid) -> WorkflowResult<StoredRun> {
        self.read_run(id)?
            .ok_or_else(|| WorkflowError::State(format!("run {id} not found")))
    }
    fn read_run(&self, id: Uuid) -> WorkflowResult<Option<StoredRun>> {
        let record: Option<StoredRun> =
            io::read(&self.run_path(id), WorkflowLimits::default().max_run_bytes)?;
        if let Some(record) = &record {
            if record.version != 1
                || record.run.id != id
                || record.run.parent_session_id != self.parent_session_id
                || record.run.definition_id != record.run.spec.id
            {
                return Err(WorkflowError::Storage(format!(
                    "run {id} identity/version mismatch"
                )));
            }
            validate_spec(&record.run.spec, &record.run.limits)?;
            check_value(&record.run.input, record.run.limits.max_value_bytes)?;
            if let Some(snapshot) = &record.host_snapshot {
                check_value(snapshot, MAX_HOST_SNAPSHOT_BYTES)?;
            }
            if record.run.nodes.len() > record.run.limits.max_instances
                || record.intents.len() > record.run.limits.max_attempts
            {
                return Err(WorkflowError::Storage(
                    "persisted instances/attempts exceed admitted limits".into(),
                ));
            }
            let mut ids = std::collections::BTreeSet::new();
            let mut paths = std::collections::BTreeSet::new();
            for node in &record.run.nodes {
                if !ids.insert(node.id) || !paths.insert(&node.path) {
                    return Err(WorkflowError::Storage(
                        "duplicate persisted node identity/path".into(),
                    ));
                }
                if let Some(request) = node.request_id {
                    let task = record.intents.get(&request).ok_or_else(|| {
                        WorkflowError::Storage("node missing durable dispatch intent".into())
                    })?;
                    if task.request_id != request
                        || task.run_id != id
                        || task.parent_session_id != self.parent_session_id
                        || task.node_instance_id != node.id
                        || task.attempt_id != node.attempt_id
                        || task.node_path != node.path
                    {
                        return Err(WorkflowError::Storage(
                            "persisted dispatch intent identity mismatch".into(),
                        ));
                    }
                }
            }
        }
        Ok(record)
    }
    fn write_run(&self, record: &StoredRun) -> WorkflowResult<()> {
        io::write(
            &self.run_path(record.run.id),
            record,
            record.run.limits.max_run_bytes,
        )
    }
}

fn settle_replay(
    record: &mut StoredRun,
    request: Uuid,
    kind: &str,
    payload: &impl Serialize,
) -> WorkflowResult<bool> {
    use sha2::{Digest, Sha256};
    let mut value = serde_json::to_value(payload)
        .map_err(|e| WorkflowError::State(format!("encode task receipt: {e}")))?;
    let delivery = if kind == "complete" {
        value.pointer_mut("/agent_run/delivered")
    } else if kind == "fail" {
        value.get_mut("delivered")
    } else {
        None
    };
    if let Some(delivered) = delivery {
        *delivered = Value::Bool(false);
    }
    let bytes = serde_json::to_vec(&(kind, value))
        .map_err(|e| WorkflowError::State(format!("encode task receipt: {e}")))?;
    let fingerprint = format!("{:x}", Sha256::digest(bytes));
    if let Some(existing) = record.settlements.get(&request) {
        if existing != &fingerprint {
            return Err(request_conflict(request));
        }
        return Ok(true);
    }
    record.settlements.insert(request, fingerprint);
    Ok(false)
}

fn request_conflict(id: Uuid) -> WorkflowError {
    WorkflowError::State(format!(
        "request {id} already belongs to a different payload"
    ))
}

fn unbound(record: &StoredRun) -> Vec<WorkflowReadyTask> {
    record
        .run
        .nodes
        .iter()
        .filter(|n| n.agent_run.is_none() && !n.status.is_terminal())
        .filter_map(|n| n.request_id.and_then(|id| record.intents.get(&id)).cloned())
        .collect()
}

fn request_index(record: &StoredRun, request: Uuid) -> WorkflowResult<usize> {
    let intent = record
        .intents
        .get(&request)
        .ok_or_else(|| WorkflowError::State(format!("unknown dispatch request {request}")))?;
    record
        .run
        .nodes
        .iter()
        .position(|n| n.id == intent.node_instance_id && n.request_id == Some(request))
        .ok_or_else(|| WorkflowError::State("dispatch intent node mismatch".into()))
}

fn validate_agent(record: &StoredRun, request: Uuid, agent: &AgentRun) -> WorkflowResult<usize> {
    let index = request_index(record, request)?;
    let node = &record.run.nodes[index];
    if agent.request_id != request || agent.parent_session_id != record.run.parent_session_id {
        return Err(WorkflowError::State(
            "AgentRun request/parent mismatch".into(),
        ));
    }
    if let Some(owner) = &agent.workflow_owner
        && (owner.workflow_run_id != record.run.id || owner.node_instance_id != node.id.to_string())
    {
        return Err(WorkflowError::State(
            "AgentRun workflow ownership mismatch".into(),
        ));
    }
    if node.agent_run.as_ref().is_some_and(|old| {
        terminal_agent(old.status) && terminal_agent(agent.status) && old.status != agent.status
    }) {
        return Err(WorkflowError::State(
            "conflicting terminal AgentRun observations".into(),
        ));
    }
    if node
        .agent_run
        .as_ref()
        .is_some_and(|old| old.id != agent.id || old.agent_id != agent.agent_id)
    {
        return Err(WorkflowError::State(
            "dispatch request already bound to a different AgentRun".into(),
        ));
    }
    Ok(index)
}

fn terminal_agent(status: AgentStatus) -> bool {
    !matches!(
        status,
        AgentStatus::Queued
            | AgentStatus::Running
            | AgentStatus::WaitingApproval
            | AgentStatus::WaitingAnswer
    )
}
fn check_report(agent: &AgentRun, limits: &WorkflowLimits) -> WorkflowResult<()> {
    if agent
        .report
        .as_ref()
        .is_some_and(|s| s.len() > limits.max_report_bytes)
    {
        return Err(WorkflowError::Limit(
            "AgentRun report exceeds report bytes".into(),
        ));
    }
    Ok(())
}
fn apply_agent(node: &mut WorkflowNodeInstance, agent: AgentRun) {
    node.started_at_ms = agent.started_at_ms.or(node.started_at_ms);
    node.finished_at_ms = agent.finished_at_ms.or(Some(now_ms()));
    node.usage = agent.usage.clone();
    node.report = agent.report.clone();
    node.agent_run = Some(agent);
}
fn add_usage(total: &mut AgentUsageSummary, value: &AgentUsageSummary) {
    total.uncertain_execution |= value.uncertain_execution;
    total.model_calls = total.model_calls.saturating_add(value.model_calls);
    total.unknown_usage_calls = total
        .unknown_usage_calls
        .saturating_add(value.unknown_usage_calls);
    total.accounted_tokens = total
        .accounted_tokens
        .saturating_add(value.accounted_tokens);
    for (a, b) in [
        (&mut total.input_tokens, value.input_tokens),
        (&mut total.output_tokens, value.output_tokens),
        (&mut total.total_tokens, value.total_tokens),
        (&mut total.cache_read_tokens, value.cache_read_tokens),
    ] {
        if let Some(value) = b {
            *a = Some(a.unwrap_or(0).saturating_add(value));
        }
    }
}
