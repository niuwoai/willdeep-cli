//! Pure durable workflow orchestration. No Provider, catalog, Agent or events.
//! The host reconciles durable requests, dispatches through its existing safety
//! boundary, and publishes events only AFTER these methods have saved state.
mod engine;
mod expression;
mod schema;
mod store;
mod validation;

pub use store::WorkflowStore;
pub use validation::{example_spec, spec_schema, validate_spec};
pub use willdeep_runtime_protocol::workflow::*;

pub type WorkflowResult<T> = Result<T, WorkflowError>;

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error("workflow validation: {0}")]
    Validation(String),
    #[error("workflow hard limit: {0}")]
    Limit(String),
    #[error("workflow storage: {0}")]
    Storage(String),
    #[error("workflow state: {0}")]
    State(String),
    #[error("workflow reference: {0}")]
    Reference(String),
    #[error("workflow schema: {0}")]
    Schema(String),
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub(crate) fn check_value(value: &serde_json::Value, max: usize) -> WorkflowResult<()> {
    fn depth(value: &serde_json::Value, level: usize) -> bool {
        level <= WORKFLOW_MAX_JSON_DEPTH
            && match value {
                serde_json::Value::Array(items) => items.iter().all(|v| depth(v, level + 1)),
                serde_json::Value::Object(map) => map.values().all(|v| depth(v, level + 1)),
                _ => true,
            }
    }
    if !depth(value, 0) {
        return Err(WorkflowError::Limit("JSON nesting exceeds 64".into()));
    }
    let bytes = serde_json::to_vec(value).map_err(|e| WorkflowError::Validation(e.to_string()))?;
    if bytes.len() > max {
        return Err(WorkflowError::Limit(format!(
            "JSON value {} bytes exceeds {max}",
            bytes.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
