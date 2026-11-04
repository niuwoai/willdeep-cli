use super::{
    expression::{check_template, condition_values},
    schema::validate_schema,
    *,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn validate_limits(limits: &WorkflowLimits) -> WorkflowResult<()> {
    let max = WorkflowLimits::default();
    for (name, value, ceiling) in [
        (
            "definition bytes",
            limits.max_definition_bytes,
            max.max_definition_bytes,
        ),
        ("run bytes", limits.max_run_bytes, max.max_run_bytes),
        ("nodes", limits.max_nodes, max.max_nodes),
        ("depth", limits.max_depth, max.max_depth),
        ("instances", limits.max_instances, max.max_instances),
        ("attempts", limits.max_attempts, max.max_attempts),
        (
            "foreach items",
            limits.max_foreach_items,
            max.max_foreach_items,
        ),
        (
            "iterations",
            limits.max_iterations as usize,
            max.max_iterations as usize,
        ),
        ("concurrency", limits.max_concurrent, max.max_concurrent),
        ("value bytes", limits.max_value_bytes, max.max_value_bytes),
        (
            "report bytes",
            limits.max_report_bytes,
            max.max_report_bytes,
        ),
        ("name bytes", limits.max_name_bytes, max.max_name_bytes),
    ] {
        if value == 0 || value > ceiling {
            return Err(WorkflowError::Limit(format!(
                "{name}: {value}, allowed 1..={ceiling}"
            )));
        }
    }
    Ok(())
}

pub(crate) fn identifier(name: &str, limit: usize) -> WorkflowResult<()> {
    if name.is_empty()
        || name.len() > limit
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
        || name == "."
        || name == ".."
    {
        return Err(WorkflowError::Validation(format!(
            "invalid identifier {name:?}; use bounded ASCII letters, digits, _, - or ."
        )));
    }
    Ok(())
}

pub fn validate_spec(spec: &WorkflowSpecV1, limits: &WorkflowLimits) -> WorkflowResult<()> {
    validate_limits(limits)?;
    if spec.version != WORKFLOW_SPEC_VERSION {
        return Err(WorkflowError::Validation(format!(
            "unsupported workflow version {}",
            spec.version
        )));
    }
    identifier(&spec.id, limits.max_name_bytes)?;
    if spec.name.is_empty() || spec.name.len() > limits.max_name_bytes {
        return Err(WorkflowError::Limit(
            "workflow name is empty or too long".into(),
        ));
    }
    if let Some(schema) = &spec.input_schema {
        validate_schema(schema)?;
    }
    let mut phases = BTreeSet::new();
    for phase in &spec.phases {
        identifier(&phase.id, limits.max_name_bytes)?;
        if phase.name.is_empty()
            || phase.name.len() > limits.max_name_bytes
            || !phases.insert(phase.id.clone())
        {
            return Err(WorkflowError::Validation(
                "invalid or duplicate phase".into(),
            ));
        }
    }
    let mut count = 0;
    scope(
        &spec.nodes,
        &spec.output,
        &BTreeSet::new(),
        false,
        false,
        0,
        &mut count,
        limits,
        &phases,
    )?;
    // Bound recursive structures before serializing programmatically built specs.
    check_value(
        &serde_json::to_value(spec).map_err(|e| WorkflowError::Validation(e.to_string()))?,
        limits.max_definition_bytes,
    )
}

pub(crate) fn ancestors(nodes: &[WorkflowNode], id: &str) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let mut pending: Vec<_> = nodes
        .iter()
        .find(|n| n.id == id)
        .map(|n| n.depends_on.clone())
        .unwrap_or_default();
    while let Some(next) = pending.pop() {
        if result.insert(next.clone())
            && let Some(node) = nodes.iter().find(|n| n.id == next)
        {
            pending.extend(node.depends_on.iter().cloned());
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn scope(
    nodes: &[WorkflowNode],
    output: &Value,
    inherited: &BTreeSet<String>,
    item: bool,
    previous: bool,
    depth: usize,
    count: &mut usize,
    limits: &WorkflowLimits,
    phases: &BTreeSet<String>,
) -> WorkflowResult<()> {
    if depth > limits.max_depth {
        return Err(WorkflowError::Limit("scope depth exhausted".into()));
    }
    *count += nodes.len();
    if *count > limits.max_nodes {
        return Err(WorkflowError::Limit(
            "definition node count exhausted".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    for node in nodes {
        identifier(&node.id, limits.max_name_bytes)?;
        if !ids.insert(node.id.clone()) || inherited.contains(&node.id) {
            return Err(WorkflowError::Validation(format!(
                "duplicate or shadowed node {}",
                node.id
            )));
        }
        if node.phase.as_ref().is_some_and(|p| !phases.contains(p)) {
            return Err(WorkflowError::Validation(format!(
                "{}: unknown phase",
                node.id
            )));
        }
    }
    let mut degree = BTreeMap::new();
    for node in nodes {
        let unique: BTreeSet<_> = node.depends_on.iter().collect();
        if unique.len() != node.depends_on.len()
            || unique
                .iter()
                .any(|id| !ids.contains(*id) || **id == node.id)
        {
            return Err(WorkflowError::Validation(format!(
                "{}: invalid or duplicate dependency",
                node.id
            )));
        }
        degree.insert(node.id.clone(), node.depends_on.len());
    }
    let mut completed = BTreeSet::new();
    loop {
        let ready: Vec<_> = degree
            .iter()
            .filter(|(id, d)| **d == 0 && !completed.contains(*id))
            .map(|(id, _)| id.clone())
            .collect();
        if ready.is_empty() {
            break;
        }
        for id in ready {
            completed.insert(id.clone());
            for node in nodes.iter().filter(|n| n.depends_on.contains(&id)) {
                *degree.get_mut(&node.id).expect("all nodes indexed") -= 1;
            }
        }
    }
    if completed.len() != nodes.len() {
        return Err(WorkflowError::Validation(
            "dependency cycle in workflow scope".into(),
        ));
    }
    for node in nodes {
        let mut visible = inherited.clone();
        visible.extend(ancestors(nodes, &node.id));
        let template = |value: &Value| check_template(value, &visible, item, previous, 0);
        match &node.kind {
            WorkflowNodeKind::Agent(agent) => {
                if agent.profile.trim().is_empty()
                    || agent.profile.len() > limits.max_name_bytes
                    || agent.prompt.trim().is_empty()
                {
                    return Err(WorkflowError::Validation(format!(
                        "{}: profile and prompt required",
                        node.id
                    )));
                }
                for (name, value) in [
                    ("max_turns", agent.max_turns),
                    ("token_budget", agent.token_budget),
                    ("timeout_seconds", agent.timeout_seconds),
                ] {
                    if value == Some(0) {
                        return Err(WorkflowError::Validation(format!(
                            "{}: {name} must be positive",
                            node.id
                        )));
                    }
                }
                if agent.write_targets.len() > limits.max_nodes
                    || agent
                        .write_targets
                        .iter()
                        .any(|v| v.is_empty() || v.len() > WORKFLOW_MAX_WRITE_TARGET_BYTES)
                {
                    return Err(WorkflowError::Limit(format!(
                        "{}: write_targets exceeds bounds",
                        node.id
                    )));
                }
                template(&agent.input)?;
                if let Some(packet) = &agent.task_packet {
                    check_value(packet, limits.max_value_bytes)?;
                }
                if let Some(ticket) = &agent.escalation {
                    check_value(ticket, limits.max_value_bytes)?;
                    if !ticket.is_object() {
                        return Err(WorkflowError::Validation(format!(
                            "{}: escalation must be an object",
                            node.id
                        )));
                    }
                }
                if let Some(command) = &agent.target_command
                    && (command.trim().is_empty() || command.len() > limits.max_value_bytes)
                {
                    return Err(WorkflowError::Validation(format!(
                        "{}: target_command must be a bounded nonempty command",
                        node.id
                    )));
                }
                if let Some(schema) = &agent.output_schema {
                    validate_schema(schema)?;
                }
            }
            WorkflowNodeKind::Switch {
                condition,
                then_scope,
                else_scope,
            } => {
                condition_values(condition, &mut |v| template(v), 0)?;
                for body in [then_scope, else_scope] {
                    scope(
                        &body.nodes,
                        &body.output,
                        &visible,
                        item,
                        previous,
                        depth + 1,
                        count,
                        limits,
                        phases,
                    )?;
                }
            }
            WorkflowNodeKind::Foreach { items, body } => {
                template(items)?;
                scope(
                    &body.nodes,
                    &body.output,
                    &visible,
                    true,
                    previous,
                    depth + 1,
                    count,
                    limits,
                    phases,
                )?;
            }
            WorkflowNodeKind::Repeat {
                initial,
                body,
                until,
                carry,
                max_iterations,
            } => {
                if *max_iterations == 0 || *max_iterations > limits.max_iterations {
                    return Err(WorkflowError::Limit(format!(
                        "{}: max_iterations out of bounds",
                        node.id
                    )));
                }
                template(initial)?;
                scope(
                    &body.nodes,
                    &body.output,
                    &visible,
                    item,
                    true,
                    depth + 1,
                    count,
                    limits,
                    phases,
                )?;
                let mut iteration_visible = visible.clone();
                iteration_visible.extend(body.nodes.iter().map(|n| n.id.clone()));
                let mut iteration_template =
                    |v: &Value| check_template(v, &iteration_visible, item, true, 0);
                condition_values(until, &mut iteration_template, 0)?;
                iteration_template(carry)?;
            }
        }
    }
    let mut output_visible = inherited.clone();
    output_visible.extend(ids);
    check_template(output, &output_visible, item, previous, 0)
}

pub fn example_spec() -> WorkflowSpecV1 {
    serde_json::from_value(json!({
        "version":1,"id":"review-v1","name":"Review", "nodes":[
            {"id":"inspect","kind":"agent","profile":"generalist","prompt":"Review the resolved input and return structured output.",
             "input":{"$ref":"input#"},"output_schema":{"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}}
        ],"output":{"$ref":"node:inspect#"}
    })).expect("static workflow example")
}

/// Machine-readable DSL schema for model-facing documentation. This describes
/// the DSL itself; output schemas use the explicitly smaller subset in schema.rs.
pub fn spec_schema() -> Value {
    let mut schema = json!({
      "$schema":"https://json-schema.org/draft/2020-12/schema",
      "title":"WorkflowSpecV1", "type":"object", "additionalProperties":false,
      "required":["version","id","name","nodes"],
      "properties":{
        "version":{"const":1},"id":{"type":"string"},"name":{"type":"string"},
        "input_schema":{"description":"Finite JSON Schema: type, enum, const, properties, required, additionalProperties, items, min/maxItems, min/maxLength, minimum/maximum, exclusiveMinimum/Maximum. No $ref, format, pattern or combinators."},
        "phases":{"type":"array","items":{"type":"object","required":["id","name"],"properties":{"id":{"type":"string"},"name":{"type":"string"}},"additionalProperties":false}},
        "nodes":{"type":"array","items":{"$ref":"#/$defs/node"}},"output":{"$ref":"#/$defs/template"}
      },
      "$defs":{
        "template":{"description":"Any JSON literal or recursive template. A reference is exactly {\"$ref\":\"input#/path\"}; roots: input, item (foreach), previous (repeat), node:<id> (declared transitive upstream). RFC6901 pointers only."},
        "scope":{"type":"object","required":["nodes"],"properties":{"nodes":{"type":"array","items":{"$ref":"#/$defs/node"}},"output":{"$ref":"#/$defs/template"}},"additionalProperties":false},
        "condition":{"oneOf":[
          {"type":"object","required":["op","left","right"],"properties":{"op":{"enum":["eq","ne","lt","le","gt","ge"]},"left":{"$ref":"#/$defs/template"},"right":{"$ref":"#/$defs/template"}},"additionalProperties":false},
          {"type":"object","required":["op","conditions"],"properties":{"op":{"enum":["and","or"]},"conditions":{"type":"array","minItems":1,"items":{"$ref":"#/$defs/condition"}}},"additionalProperties":false},
          {"type":"object","required":["op","condition"],"properties":{"op":{"const":"not"},"condition":{"$ref":"#/$defs/condition"}},"additionalProperties":false}
        ]},
        "node":{"type":"object","required":["id","kind"],"properties":{
          "id":{"type":"string"},"depends_on":{"type":"array","uniqueItems":true,"items":{"type":"string"}},"phase":{"type":["string","null"]},
          "kind":{"enum":["agent","switch","foreach","repeat"]},"profile":{"type":"string"},"prompt":{"type":"string"},"input":{"$ref":"#/$defs/template"},
          "output_schema":{},"worker_tier":{"enum":["standard","advanced","expert",null]},"max_turns":{"type":["integer","null"],"minimum":1},
          "token_budget":{"type":["integer","null"],"minimum":1},"timeout_seconds":{"type":["integer","null"],"minimum":1},
          "write_targets":{"type":"array","items":{"type":"string"}},"task_packet":{},
          "condition":{"$ref":"#/$defs/condition"},"then_scope":{"$ref":"#/$defs/scope"},"else_scope":{"$ref":"#/$defs/scope"},
          "items":{"$ref":"#/$defs/template"},"body":{"$ref":"#/$defs/scope"},"initial":{"$ref":"#/$defs/template"},
          "until":{"$ref":"#/$defs/condition"},"carry":{"$ref":"#/$defs/template"},"max_iterations":{"type":"integer","minimum":1,"maximum":128}
        },"additionalProperties":false,
        "allOf":[
          {"if":{"properties":{"kind":{"const":"agent"}}},"then":{"required":["profile","prompt"]}},
          {"if":{"properties":{"kind":{"const":"switch"}}},"then":{"required":["condition","then_scope","else_scope"]}},
          {"if":{"properties":{"kind":{"const":"foreach"}}},"then":{"required":["items","body"]}},
          {"if":{"properties":{"kind":{"const":"repeat"}}},"then":{"required":["initial","body","until","carry","max_iterations"]}}
        ]}
      }
    });
    schema["$defs"]["node"]["properties"]["escalation"] = json!({"type":["object","null"],"description":"Existing expert escalation ticket; validated by the host routing policy."});
    schema["$defs"]["node"]["properties"]["target_command"] = json!({"type":["string","null"]});
    let properties = schema["$defs"]["node"]["properties"]
        .as_object()
        .expect("static node schema");
    let mut variants = Vec::new();
    for (kind, fields, required) in [
        (
            "agent",
            vec![
                "profile",
                "prompt",
                "input",
                "output_schema",
                "worker_tier",
                "max_turns",
                "token_budget",
                "timeout_seconds",
                "write_targets",
                "task_packet",
                "escalation",
                "target_command",
            ],
            vec!["profile", "prompt"],
        ),
        (
            "switch",
            vec!["condition", "then_scope", "else_scope"],
            vec!["condition", "then_scope", "else_scope"],
        ),
        ("foreach", vec!["items", "body"], vec!["items", "body"]),
        (
            "repeat",
            vec!["initial", "body", "until", "carry", "max_iterations"],
            vec!["initial", "body", "until", "carry", "max_iterations"],
        ),
    ] {
        let mut props = serde_json::Map::new();
        for name in ["id", "depends_on", "phase"].into_iter().chain(fields) {
            props.insert(name.into(), properties[name].clone());
        }
        props.insert("kind".into(), json!({"const":kind}));
        let required: Vec<_> = ["id", "kind"].into_iter().chain(required).collect();
        variants.push(json!({"type":"object","properties":props,"required":required,"additionalProperties":false}));
    }
    schema["$defs"]["node"] = json!({"oneOf":variants});
    schema
}
