//! Deliberately finite JSON Schema subset. Unknown keywords fail at definition
//! time. All $ref forms (including remote refs) are explicitly unsupported.
use super::{WORKFLOW_MAX_EXPRESSION_DEPTH, WorkflowError, WorkflowResult};
use serde_json::Value;

const TYPES: &[&str] = &[
    "null", "boolean", "object", "array", "number", "integer", "string",
];
const KEYWORDS: &[&str] = &[
    "$schema",
    "title",
    "description",
    "type",
    "enum",
    "const",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "minItems",
    "maxItems",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
];

pub(crate) fn validate_schema(schema: &Value) -> WorkflowResult<()> {
    super::check_value(schema, super::WORKFLOW_MAX_VALUE_BYTES)?;
    definition(schema, 0)
}

fn error(message: impl Into<String>) -> WorkflowError {
    WorkflowError::Schema(message.into())
}

fn definition(schema: &Value, depth: usize) -> WorkflowResult<()> {
    if depth > WORKFLOW_MAX_EXPRESSION_DEPTH {
        return Err(error("schema nesting exceeds 32"));
    }
    if schema.is_boolean() {
        return Ok(());
    }
    let map = schema
        .as_object()
        .ok_or_else(|| error("schema must be an object or boolean"))?;
    for (key, value) in map {
        if !KEYWORDS.contains(&key.as_str()) {
            return Err(error(format!(
                "unsupported keyword {key}; references and remote loading are forbidden"
            )));
        }
        match key.as_str() {
            "$schema" => {
                if !matches!(
                    value.as_str(),
                    Some(
                        "https://json-schema.org/draft/2020-12/schema"
                            | "http://json-schema.org/draft-07/schema#"
                    )
                ) {
                    return Err(error("unsupported $schema dialect"));
                }
            }
            "title" | "description" => {
                if !value.is_string() {
                    return Err(error(format!("{key} must be a string")));
                }
            }
            "type" => {
                if !value.as_str().is_some_and(|s| TYPES.contains(&s)) {
                    return Err(error(
                        "type must name one supported type (type unions are unsupported)",
                    ));
                }
            }
            "properties" => {
                let props = value
                    .as_object()
                    .ok_or_else(|| error("properties must be an object"))?;
                for sub in props.values() {
                    definition(sub, depth + 1)?;
                }
            }
            "items" | "additionalProperties" => definition(value, depth + 1)?,
            "required" => {
                let list = value
                    .as_array()
                    .ok_or_else(|| error("required must be a string array"))?;
                let mut names = std::collections::BTreeSet::new();
                for name in list {
                    let name = name
                        .as_str()
                        .ok_or_else(|| error("required must contain strings"))?;
                    if !names.insert(name) {
                        return Err(error("duplicate required property"));
                    }
                }
            }
            "enum" => {
                let values = value
                    .as_array()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| error("enum must be a nonempty array"))?;
                for (i, value) in values.iter().enumerate() {
                    if values[..i].iter().any(|other| equal(other, value)) {
                        return Err(error("duplicate enum value"));
                    }
                }
            }
            "minItems" | "maxItems" | "minLength" | "maxLength" => {
                if value.as_u64().is_none() {
                    return Err(error(format!("{key} must be a nonnegative integer")));
                }
            }
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" => {
                if !value.is_number() {
                    return Err(error(format!("{key} must be numeric")));
                }
            }
            "const" => {}
            _ => unreachable!(),
        }
    }
    for (min, max) in [
        ("minItems", "maxItems"),
        ("minLength", "maxLength"),
        ("minimum", "maximum"),
    ] {
        if let (Some(Value::Number(a)), Some(Value::Number(b))) = (map.get(min), map.get(max))
            && super::expression::compare_numbers(a, b).is_some_and(|o| o.is_gt())
        {
            return Err(error(format!("{min} exceeds {max}")));
        }
    }
    Ok(())
}

pub(crate) fn validate_output(schema: &Value, value: &Value) -> WorkflowResult<()> {
    validate_schema(schema)?;
    instance(schema, value, "#")
}

fn instance(schema: &Value, value: &Value, path: &str) -> WorkflowResult<()> {
    if let Some(allowed) = schema.as_bool() {
        return if allowed {
            Ok(())
        } else {
            Err(error(format!("{path}: false schema rejects value")))
        };
    }
    let map = schema.as_object().ok_or_else(|| error("invalid schema"))?;
    if let Some(kind) = map.get("type").and_then(Value::as_str) {
        let valid = match kind {
            "null" => value.is_null(),
            "boolean" => value.is_boolean(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => {
                value.as_i64().is_some()
                    || value.as_u64().is_some()
                    || value.as_f64().is_some_and(|n| n.fract() == 0.0)
            }
            _ => false,
        };
        if !valid {
            return Err(error(format!("{path}: expected {kind}")));
        }
    }
    if let Some(expected) = map.get("const")
        && !equal(expected, value)
    {
        return Err(error(format!("{path}: const mismatch")));
    }
    if let Some(values) = map.get("enum").and_then(Value::as_array)
        && !values.iter().any(|expected| equal(expected, value))
    {
        return Err(error(format!("{path}: value not in enum")));
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = map.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    return Err(error(format!("{path}: missing required property {name}")));
                }
            }
        }
        let props = map.get("properties").and_then(Value::as_object);
        for (key, val) in object {
            let sub = props
                .and_then(|p| p.get(key))
                .or_else(|| map.get("additionalProperties"));
            if let Some(sub) = sub {
                instance(
                    sub,
                    val,
                    &format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")),
                )?;
            }
        }
    }
    if let Some(array) = value.as_array() {
        length(map, "minItems", "maxItems", array.len(), path)?;
        if let Some(sub) = map.get("items") {
            for (index, val) in array.iter().enumerate() {
                instance(sub, val, &format!("{path}/{index}"))?;
            }
        }
    }
    if let Some(string) = value.as_str() {
        length(map, "minLength", "maxLength", string.chars().count(), path)?;
    }
    if let Value::Number(number) = value {
        for key in ["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"] {
            if let Some(Value::Number(bound)) = map.get(key) {
                let ordering = super::expression::compare_numbers(number, bound)
                    .ok_or_else(|| error("invalid numeric constraint"))?;
                let valid = match key {
                    "minimum" => !ordering.is_lt(),
                    "maximum" => !ordering.is_gt(),
                    "exclusiveMinimum" => ordering.is_gt(),
                    _ => ordering.is_lt(),
                };
                if !valid {
                    return Err(error(format!("{path}: {key} constraint failed")));
                }
            }
        }
    }
    Ok(())
}

fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            super::expression::compare_numbers(a, b).is_some_and(|o| o.is_eq())
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(k, a)| b.get(k).is_some_and(|b| equal(a, b)))
        }
        _ => a == b,
    }
}

fn length(
    map: &serde_json::Map<String, Value>,
    min: &str,
    max: &str,
    actual: usize,
    path: &str,
) -> WorkflowResult<()> {
    if map
        .get(min)
        .and_then(Value::as_u64)
        .is_some_and(|n| (actual as u64) < n)
        || map
            .get(max)
            .and_then(Value::as_u64)
            .is_some_and(|n| (actual as u64) > n)
    {
        return Err(error(format!("{path}: {min}/{max} constraint failed")));
    }
    Ok(())
}
