use super::{
    WORKFLOW_MAX_EXPRESSION_DEPTH, WORKFLOW_MAX_JSON_DEPTH, WorkflowCondition, WorkflowError,
    WorkflowResult,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Default)]
pub(crate) struct Context {
    pub input: Value,
    pub item: Option<Value>,
    pub previous: Option<Value>,
    pub nodes: BTreeMap<String, Value>,
}

pub(crate) fn parse_reference(reference: &str) -> WorkflowResult<(&str, &str)> {
    let (root, pointer) = reference.split_once('#').ok_or_else(|| {
        WorkflowError::Reference(format!("{reference}: expected root#JSON-Pointer"))
    })?;
    if !(matches!(root, "input" | "item" | "previous")
        || root
            .strip_prefix("node:")
            .is_some_and(|name| !name.is_empty()))
    {
        return Err(WorkflowError::Reference(format!(
            "{reference}: unsupported root"
        )));
    }
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err(WorkflowError::Reference(format!(
            "{reference}: pointer must be empty or start with /"
        )));
    }
    let mut chars = pointer.chars();
    while let Some(c) = chars.next() {
        if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return Err(WorkflowError::Reference(format!(
                "{reference}: invalid JSON Pointer escape"
            )));
        }
    }
    Ok((root, pointer))
}

pub(crate) fn resolve(value: &Value, context: &Context) -> WorkflowResult<Value> {
    match value {
        Value::Object(map) if map.contains_key("$ref") => {
            if map.len() != 1 {
                return Err(WorkflowError::Reference(
                    "$ref template cannot have sibling keys".into(),
                ));
            }
            let reference = map["$ref"]
                .as_str()
                .ok_or_else(|| WorkflowError::Reference("$ref must be a string".into()))?;
            let (root, pointer) = parse_reference(reference)?;
            let source = match root {
                "input" => Some(&context.input),
                "item" => context.item.as_ref(),
                "previous" => context.previous.as_ref(),
                _ => context.nodes.get(&root[5..]),
            }
            .ok_or_else(|| WorkflowError::Reference(format!("{reference}: root is not visible")))?;
            source.pointer(pointer).cloned().ok_or_else(|| {
                WorkflowError::Reference(format!("{reference}: pointer does not exist"))
            })
        }
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| Ok((key.clone(), resolve(value, context)?)))
            .collect::<WorkflowResult<serde_json::Map<_, _>>>()
            .map(Value::Object),
        Value::Array(items) => items
            .iter()
            .map(|v| resolve(v, context))
            .collect::<WorkflowResult<Vec<_>>>()
            .map(Value::Array),
        _ => Ok(value.clone()),
    }
}

pub(crate) fn check_template(
    value: &Value,
    visible: &BTreeSet<String>,
    item: bool,
    previous: bool,
    depth: usize,
) -> WorkflowResult<()> {
    if depth > WORKFLOW_MAX_JSON_DEPTH {
        return Err(WorkflowError::Limit("template nesting exceeds 64".into()));
    }
    match value {
        Value::Object(map) if map.contains_key("$ref") => {
            if map.len() != 1 {
                return Err(WorkflowError::Reference(
                    "$ref template cannot have sibling keys".into(),
                ));
            }
            let reference = map["$ref"]
                .as_str()
                .ok_or_else(|| WorkflowError::Reference("$ref must be string".into()))?;
            let (root, _) = parse_reference(reference)?;
            let valid = match root {
                "input" => true,
                "item" => item,
                "previous" => previous,
                _ => visible.contains(&root[5..]),
            };
            if !valid {
                return Err(WorkflowError::Reference(format!(
                    "{reference}: not a visible declared upstream"
                )));
            }
        }
        Value::Object(map) => {
            for val in map.values() {
                check_template(val, visible, item, previous, depth + 1)?;
            }
        }
        Value::Array(items) => {
            for val in items {
                check_template(val, visible, item, previous, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn condition_values(
    condition: &WorkflowCondition,
    visit: &mut impl FnMut(&Value) -> WorkflowResult<()>,
    depth: usize,
) -> WorkflowResult<()> {
    if depth > WORKFLOW_MAX_EXPRESSION_DEPTH {
        return Err(WorkflowError::Limit("condition nesting exceeds 32".into()));
    }
    match condition {
        WorkflowCondition::And { conditions } | WorkflowCondition::Or { conditions } => {
            if conditions.is_empty() {
                return Err(WorkflowError::Validation(
                    "boolean combinations require at least one condition".into(),
                ));
            }
            for condition in conditions {
                condition_values(condition, visit, depth + 1)?;
            }
        }
        WorkflowCondition::Not { condition } => condition_values(condition, visit, depth + 1)?,
        WorkflowCondition::Eq { left, right }
        | WorkflowCondition::Ne { left, right }
        | WorkflowCondition::Lt { left, right }
        | WorkflowCondition::Le { left, right }
        | WorkflowCondition::Gt { left, right }
        | WorkflowCondition::Ge { left, right } => {
            visit(left)?;
            visit(right)?;
        }
    }
    Ok(())
}

/// Compare JSON numbers without rounding 64-bit integers through f64. A mixed
/// integer/float comparison first compares integral parts, then the fraction.
pub(crate) fn compare_numbers(
    a: &serde_json::Number,
    b: &serde_json::Number,
) -> Option<std::cmp::Ordering> {
    fn integer(n: &serde_json::Number) -> Option<i128> {
        n.as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from))
    }
    fn mixed(integer: i128, float: f64) -> std::cmp::Ordering {
        if float >= i128::MAX as f64 {
            return std::cmp::Ordering::Less;
        }
        if float <= i128::MIN as f64 {
            return std::cmp::Ordering::Greater;
        }
        let whole = float.trunc() as i128;
        integer.cmp(&whole).then_with(|| {
            if float.fract() > 0.0 {
                std::cmp::Ordering::Less
            } else if float.fract() < 0.0 {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
    }
    match (integer(a), integer(b)) {
        (Some(a), Some(b)) => Some(a.cmp(&b)),
        (Some(a), None) => b.as_f64().map(|b| mixed(a, b)),
        (None, Some(b)) => a.as_f64().map(|a| mixed(b, a).reverse()),
        (None, None) => a
            .as_f64()
            .zip(b.as_f64())
            .and_then(|(a, b)| a.partial_cmp(&b)),
    }
}

pub(crate) fn evaluate(condition: &WorkflowCondition, context: &Context) -> WorkflowResult<bool> {
    use WorkflowCondition::*;
    match condition {
        And { conditions } => {
            // Evaluate all operands: an invalid reference/type must never be hidden
            // by short-circuiting, including in nested boolean combinations.
            let values = conditions
                .iter()
                .map(|c| evaluate(c, context))
                .collect::<WorkflowResult<Vec<_>>>()?;
            Ok(values.iter().all(|v| *v))
        }
        Or { conditions } => {
            let values = conditions
                .iter()
                .map(|c| evaluate(c, context))
                .collect::<WorkflowResult<Vec<_>>>()?;
            Ok(values.iter().any(|v| *v))
        }
        Not { condition } => Ok(!evaluate(condition, context)?),
        Eq { left, right }
        | Ne { left, right }
        | Lt { left, right }
        | Le { left, right }
        | Gt { left, right }
        | Ge { left, right } => {
            let left = resolve(left, context)?;
            let right = resolve(right, context)?;
            let ordering = match (&left, &right) {
                (Value::Null, Value::Null) => Some(std::cmp::Ordering::Equal),
                (Value::Bool(a), Value::Bool(b)) if matches!(condition, Eq {..} | Ne {..}) => Some(a.cmp(b)),
                (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
                (Value::Number(a), Value::Number(b)) => compare_numbers(a, b),
                _ => None,
            }.ok_or_else(|| WorkflowError::Validation("condition operands require matching scalar types; ordering supports numbers/strings only".into()))?;
            if matches!(left, Value::Null) && !matches!(condition, Eq { .. } | Ne { .. }) {
                return Err(WorkflowError::Validation("null is not ordered".into()));
            }
            Ok(match condition {
                Eq { .. } => ordering.is_eq(),
                Ne { .. } => !ordering.is_eq(),
                Lt { .. } => ordering.is_lt(),
                Le { .. } => !ordering.is_gt(),
                Gt { .. } => ordering.is_gt(),
                Ge { .. } => !ordering.is_lt(),
                _ => unreachable!(),
            })
        }
    }
}
