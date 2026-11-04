use super::{
    expression::{Context, evaluate, resolve},
    store::StoredRun,
    validation::ancestors,
    *,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Expansion {
    Switch { selected: bool },
    Foreach { items: Vec<Value> },
    Repeat { iteration: u32, previous: Value },
}

enum ScopeOutcome {
    Waiting,
    Complete(Value, Context),
    Failed,
}

pub(crate) fn tick(record: &mut StoredRun) -> WorkflowResult<()> {
    if record.run.status.is_terminal() {
        return Ok(());
    }
    if record.run.status == WorkflowRunStatus::Stopping {
        settle_stop(record);
        return Ok(());
    }
    let spec = record.run.spec.clone();
    let context = Context {
        input: record.run.input.clone(),
        ..Default::default()
    };
    let mut engine = Engine {
        record,
        changed: false,
    };
    if spec.nodes.len() > engine.record.run.limits.max_instances {
        engine.record.run.status = WorkflowRunStatus::Failed;
        engine.record.run.error = Some("root node instance count exhausted".into());
        engine.record.run.finished_at_ms = Some(now_ms());
        return Ok(());
    }
    // A pass may free concurrency or expose a later-declared dependency. The
    // hard instance ceiling also bounds the fixed-point scheduler's work.
    for _ in 0..=engine.record.run.limits.max_instances {
        engine.changed = false;
        let outcome = engine.scope(&spec.nodes, &spec.output, "", &context)?;
        engine.block_orphaned();
        let all_settled = engine
            .record
            .run
            .nodes
            .iter()
            .all(|n| n.status.is_terminal());
        if all_settled {
            match outcome {
                ScopeOutcome::Complete(output, _) => {
                    engine.record.run.output = Some(output);
                    engine.record.run.status = WorkflowRunStatus::Completed;
                }
                ScopeOutcome::Failed => {
                    engine.record.run.status = if engine
                        .record
                        .run
                        .nodes
                        .iter()
                        .any(|n| n.status == WorkflowNodeStatus::Interrupted)
                    {
                        WorkflowRunStatus::Interrupted
                    } else {
                        WorkflowRunStatus::Failed
                    };
                    if engine.record.run.error.is_none() {
                        engine.record.run.error = engine
                            .record
                            .run
                            .nodes
                            .iter()
                            .find_map(|n| n.error.clone())
                            .or_else(|| {
                                Some("one or more workflow nodes failed or were blocked".into())
                            });
                    }
                }
                ScopeOutcome::Waiting => break,
            }
            engine.record.run.finished_at_ms = Some(now_ms());
            break;
        }
        if !engine.changed {
            break;
        }
    }
    Ok(())
}

fn settle_stop(record: &mut StoredRun) {
    let active: Vec<_> = record
        .run
        .nodes
        .iter()
        .filter(|n| n.request_id.is_some() && !n.status.is_terminal())
        .map(|n| n.path.clone())
        .collect();
    for node in &mut record.run.nodes {
        if node.status.is_terminal() {
            continue;
        }
        if node.request_id.is_some()
            || active
                .iter()
                .any(|p| p.starts_with(&format!("{}/", node.path)))
        {
            node.status = WorkflowNodeStatus::Stopping;
        } else {
            node.status = WorkflowNodeStatus::Cancelled;
            node.finished_at_ms = Some(now_ms());
        }
    }
    if record.run.nodes.iter().all(|n| n.status.is_terminal()) {
        record.run.status = WorkflowRunStatus::Cancelled;
        record.run.finished_at_ms = Some(now_ms());
    }
}

struct Engine<'a> {
    record: &'a mut StoredRun,
    changed: bool,
}

impl Engine<'_> {
    fn scope(
        &mut self,
        nodes: &[WorkflowNode],
        output: &Value,
        prefix: &str,
        context: &Context,
    ) -> WorkflowResult<ScopeOutcome> {
        let missing = nodes
            .iter()
            .filter(|n| self.index(&path(prefix, &n.id)).is_none())
            .count();
        if self.record.run.nodes.len() + missing > self.record.run.limits.max_instances {
            return Err(WorkflowError::Limit("node instance count exhausted".into()));
        }
        for node in nodes {
            self.ensure(node, &path(prefix, &node.id));
        }
        for node in nodes {
            let node_path = path(prefix, &node.id);
            let index = self.index(&node_path).expect("scope instances initialized");
            let status = self.record.run.nodes[index].status;
            if status.is_terminal()
                || matches!(
                    status,
                    WorkflowNodeStatus::DispatchPending
                        | WorkflowNodeStatus::Dispatched
                        | WorkflowNodeStatus::Stopping
                )
            {
                continue;
            }
            if matches!(node.kind, WorkflowNodeKind::Agent(_))
                && status == WorkflowNodeStatus::Running
            {
                continue;
            }
            let deps: Vec<_> = node
                .depends_on
                .iter()
                .map(|id| {
                    self.record.run.nodes[self.index(&path(prefix, id)).expect("valid dependency")]
                        .status
                })
                .collect();
            if deps
                .iter()
                .any(|s| s.is_terminal() && *s != WorkflowNodeStatus::Completed)
            {
                self.finish(
                    index,
                    WorkflowNodeStatus::Blocked,
                    None,
                    Some("declared dependency did not succeed".into()),
                );
                continue;
            }
            if deps.iter().any(|s| *s != WorkflowNodeStatus::Completed) {
                continue;
            }
            let mut visible = context.clone();
            for id in ancestors(nodes, &node.id) {
                if let Some(upstream) = self
                    .index(&path(prefix, &id))
                    .map(|i| &self.record.run.nodes[i])
                    && let Some(output) = &upstream.output
                {
                    visible.nodes.insert(id, output.clone());
                }
            }
            if let Err(error) = self.node(node, index, &node_path, &visible) {
                self.finish(
                    index,
                    WorkflowNodeStatus::Failed,
                    None,
                    Some(error.to_string()),
                );
            }
        }
        let statuses: Vec<_> = nodes
            .iter()
            .map(|n| {
                self.record.run.nodes[self.index(&path(prefix, &n.id)).expect("initialized")].status
            })
            .collect();
        if statuses.iter().any(|s| !s.is_terminal()) {
            return Ok(ScopeOutcome::Waiting);
        }
        if statuses.iter().any(|s| *s != WorkflowNodeStatus::Completed) {
            return Ok(ScopeOutcome::Failed);
        }
        let mut complete = context.clone();
        for node in nodes {
            let output = self.record.run.nodes
                [self.index(&path(prefix, &node.id)).expect("initialized")]
            .output
            .clone()
            .unwrap_or(Value::Null);
            complete.nodes.insert(node.id.clone(), output);
        }
        match resolve(output, &complete).and_then(|v| {
            check_value(&v, self.record.run.limits.max_value_bytes)?;
            Ok(v)
        }) {
            Ok(output) => Ok(ScopeOutcome::Complete(output, complete)),
            Err(error) => {
                // Scope output failures must be durable even when there are no nodes.
                self.record.run.error = Some(error.to_string());
                Ok(ScopeOutcome::Failed)
            }
        }
    }

    fn node(
        &mut self,
        node: &WorkflowNode,
        index: usize,
        node_path: &str,
        context: &Context,
    ) -> WorkflowResult<()> {
        if !matches!(node.kind, WorkflowNodeKind::Agent(_))
            && self.record.run.nodes[index].status == WorkflowNodeStatus::Pending
        {
            self.begin(index);
        }
        match &node.kind {
            WorkflowNodeKind::Agent(agent) => {
                let active = self
                    .record
                    .run
                    .nodes
                    .iter()
                    .filter(|n| n.request_id.is_some() && !n.status.is_terminal())
                    .count();
                if active >= self.record.run.limits.max_concurrent {
                    return Ok(());
                }
                if self.record.intents.len() >= self.record.run.limits.max_attempts {
                    return Err(WorkflowError::Limit("leaf attempt count exhausted".into()));
                }
                let input = resolve(&agent.input, context)?;
                check_value(&input, self.record.run.limits.max_value_bytes)?;
                let request_id = Uuid::new_v4();
                let instance = &mut self.record.run.nodes[index];
                instance.request_id = Some(request_id);
                instance.status = WorkflowNodeStatus::DispatchPending;
                let task = WorkflowReadyTask {
                    run_id: self.record.run.id,
                    parent_session_id: self.record.run.parent_session_id,
                    node_instance_id: instance.id,
                    attempt_id: instance.attempt_id,
                    request_id,
                    node_path: node_path.into(),
                    profile: agent.profile.clone(),
                    prompt: agent.prompt.clone(),
                    input,
                    output_schema: agent.output_schema.clone(),
                    worker_tier: agent.worker_tier,
                    max_turns: agent.max_turns,
                    token_budget: agent.token_budget,
                    timeout_seconds: agent.timeout_seconds,
                    write_targets: agent.write_targets.clone(),
                    task_packet: agent.task_packet.clone(),
                    escalation: agent.escalation.clone(),
                    target_command: agent.target_command.clone(),
                };
                self.record.intents.insert(request_id, task);
                self.changed = true;
            }
            WorkflowNodeKind::Switch {
                condition,
                then_scope,
                else_scope,
            } => {
                let selected = match self.record.expansions.get(node_path) {
                    Some(Expansion::Switch { selected }) => *selected,
                    None => {
                        let selected = evaluate(condition, context)?;
                        self.record
                            .expansions
                            .insert(node_path.into(), Expansion::Switch { selected });
                        self.begin(index);
                        selected
                    }
                    _ => return Err(WorkflowError::State("switch expansion mismatch".into())),
                };
                let body = if selected { then_scope } else { else_scope };
                let child_path = format!("{node_path}/{}", if selected { "then" } else { "else" });
                let result = self.scope(&body.nodes, &body.output, &child_path, context)?;
                self.accept_scope(index, result);
            }
            WorkflowNodeKind::Foreach { items, body } => {
                let frozen = match self.record.expansions.get(node_path) {
                    Some(Expansion::Foreach { items }) => items.clone(),
                    None => {
                        let value = resolve(items, context)?;
                        let frozen = value
                            .as_array()
                            .ok_or_else(|| {
                                WorkflowError::Validation(
                                    "foreach items must resolve to an array".into(),
                                )
                            })?
                            .clone();
                        if frozen.len() > self.record.run.limits.max_foreach_items {
                            return Err(WorkflowError::Limit(
                                "foreach item count exhausted".into(),
                            ));
                        }
                        check_value(&value, self.record.run.limits.max_value_bytes)?;
                        self.record.expansions.insert(
                            node_path.into(),
                            Expansion::Foreach {
                                items: frozen.clone(),
                            },
                        );
                        self.begin(index);
                        frozen
                    }
                    _ => return Err(WorkflowError::State("foreach expansion mismatch".into())),
                };
                let mut outputs = Vec::with_capacity(frozen.len());
                let mut waiting = false;
                let mut failed = false;
                // Each item advances its own DAG immediately. There is no
                // barrier between corresponding nodes of different items.
                for (item_index, item) in frozen.into_iter().enumerate() {
                    let mut local = context.clone();
                    local.item = Some(item);
                    let child_path = format!("{node_path}/{item_index}");
                    match self.scope(&body.nodes, &body.output, &child_path, &local)? {
                        ScopeOutcome::Waiting => waiting = true,
                        ScopeOutcome::Failed => failed = true,
                        ScopeOutcome::Complete(output, _) => outputs.push(output),
                    }
                }
                if !waiting {
                    if failed {
                        self.finish(
                            index,
                            WorkflowNodeStatus::Failed,
                            None,
                            Some("foreach item failed".into()),
                        );
                    } else {
                        let output = Value::Array(outputs);
                        check_value(&output, self.record.run.limits.max_value_bytes)?;
                        self.finish(index, WorkflowNodeStatus::Completed, Some(output), None);
                    }
                }
            }
            WorkflowNodeKind::Repeat {
                initial,
                body,
                until,
                carry,
                max_iterations,
            } => {
                let (iteration, previous) = match self.record.expansions.get(node_path) {
                    Some(Expansion::Repeat {
                        iteration,
                        previous,
                    }) => (*iteration, previous.clone()),
                    None => {
                        let initial = resolve(initial, context)?;
                        check_value(&initial, self.record.run.limits.max_value_bytes)?;
                        self.record.expansions.insert(
                            node_path.into(),
                            Expansion::Repeat {
                                iteration: 0,
                                previous: initial.clone(),
                            },
                        );
                        self.begin(index);
                        (0, initial)
                    }
                    _ => return Err(WorkflowError::State("repeat expansion mismatch".into())),
                };
                let mut local = context.clone();
                local.previous = Some(previous);
                let child_path = format!("{node_path}/{iteration}");
                match self.scope(&body.nodes, &body.output, &child_path, &local)? {
                    ScopeOutcome::Waiting => {}
                    ScopeOutcome::Failed => self.finish(
                        index,
                        WorkflowNodeStatus::Failed,
                        None,
                        Some("repeat body failed".into()),
                    ),
                    ScopeOutcome::Complete(_, complete) => {
                        let done = evaluate(until, &complete)?;
                        let next = resolve(carry, &complete)?;
                        check_value(&next, self.record.run.limits.max_value_bytes)?;
                        if done {
                            self.finish(index, WorkflowNodeStatus::Completed, Some(next), None);
                        } else if iteration + 1 >= *max_iterations {
                            self.finish(
                                index,
                                WorkflowNodeStatus::Failed,
                                None,
                                Some(format!(
                                    "repeat max_iterations {max_iterations} exhausted without until"
                                )),
                            );
                        } else {
                            self.record.expansions.insert(
                                node_path.into(),
                                Expansion::Repeat {
                                    iteration: iteration + 1,
                                    previous: next,
                                },
                            );
                            self.changed = true;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn accept_scope(&mut self, index: usize, result: ScopeOutcome) {
        match result {
            ScopeOutcome::Waiting => {}
            ScopeOutcome::Complete(output, _) => {
                self.finish(index, WorkflowNodeStatus::Completed, Some(output), None)
            }
            ScopeOutcome::Failed => self.finish(
                index,
                WorkflowNodeStatus::Failed,
                None,
                Some("selected scope failed".into()),
            ),
        }
    }

    fn ensure(&mut self, node: &WorkflowNode, path: &str) {
        if self.index(path).is_some() {
            return;
        }
        self.record.run.nodes.push(WorkflowNodeInstance {
            id: Uuid::new_v4(),
            node_id: node.id.clone(),
            path: path.into(),
            attempt_id: Uuid::new_v4(),
            status: WorkflowNodeStatus::Pending,
            request_id: None,
            agent_run: None,
            output: None,
            report: None,
            error: None,
            usage: Default::default(),
            created_at_ms: now_ms(),
            started_at_ms: None,
            finished_at_ms: None,
            reused_from: None,
        });
        self.changed = true;
    }

    fn index(&self, path: &str) -> Option<usize> {
        self.record.run.nodes.iter().position(|n| n.path == path)
    }
    fn begin(&mut self, index: usize) {
        let instance = &mut self.record.run.nodes[index];
        instance.status = WorkflowNodeStatus::Running;
        instance.started_at_ms = Some(now_ms());
        self.changed = true;
    }
    fn finish(
        &mut self,
        index: usize,
        status: WorkflowNodeStatus,
        output: Option<Value>,
        error: Option<String>,
    ) {
        let instance = &mut self.record.run.nodes[index];
        instance.status = status;
        instance.output = output;
        instance.error = error;
        instance.finished_at_ms = Some(now_ms());
        self.changed = true;
    }
    fn block_orphaned(&mut self) {
        let failed: Vec<_> = self
            .record
            .run
            .nodes
            .iter()
            .filter(|n| {
                matches!(
                    n.status,
                    WorkflowNodeStatus::Failed
                        | WorkflowNodeStatus::Blocked
                        | WorkflowNodeStatus::Interrupted
                )
            })
            .map(|n| format!("{}/", n.path))
            .collect();
        for instance in &mut self.record.run.nodes {
            if !instance.status.is_terminal()
                && instance.request_id.is_none()
                && failed.iter().any(|p| instance.path.starts_with(p))
            {
                instance.status = WorkflowNodeStatus::Blocked;
                instance.error = Some("enclosing scope failed".into());
                instance.finished_at_ms = Some(now_ms());
                self.changed = true;
            }
        }
    }
}

fn path(prefix: &str, id: &str) -> String {
    if prefix.is_empty() {
        id.into()
    } else {
        format!("{prefix}/{id}")
    }
}

pub(crate) fn fresh_record(run: WorkflowRun) -> StoredRun {
    StoredRun {
        version: 1,
        run,
        intents: BTreeMap::new(),
        expansions: BTreeMap::new(),
        settlements: BTreeMap::new(),
        operation: None,
        host_snapshot: None,
    }
}
