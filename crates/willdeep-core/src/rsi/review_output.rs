//! The offline-only review format. This does not change the legacy RSI contract.
use anyhow::{Result, ensure};
use serde_json::Value;
use std::sync::OnceLock;

pub const CONTRACT: &[u8] =
    include_bytes!("../../../../bench/agent-rsi/review-output-v2/contract.json");
pub const MAXIMUM_REPORT_BYTES: usize = 16_384;
const MAXIMUM_DEPTH: usize = 8;
const MAXIMUM_ITEMS: usize = 128;
const MAXIMUM_STRING_BYTES: usize = 4096;
const FORBIDDEN_KEYS: &[&str] = &["__proto__", "prototype", "constructor"];

pub fn instructions() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| {
        serde_json::from_slice::<Value>(CONTRACT).expect("bundled review contract")["instructions"]
            .as_str()
            .expect("bundled instructions")
            .to_owned()
    })
}
pub fn instructions_sha256() -> String {
    crate::agent_definition::digest(instructions().as_bytes())
}
pub fn validate(bytes: &[u8]) -> Result<Value> {
    ensure!(
        bytes.len() <= MAXIMUM_REPORT_BYTES,
        "review report exceeds byte limit"
    );
    let value = super::contract::parse_json(bytes)?;
    bounded(&value, 1, &mut 0)?;
    exact_keys(&value, &["has_defect", "line", "reason", "reproduction"])?;
    let defect = value["has_defect"]
        .as_bool()
        .ok_or_else(|| anyhow::anyhow!("has_defect must be boolean"))?;
    ensure!(
        value["reason"]
            .as_str()
            .is_some_and(|text| !text.trim().is_empty()),
        "reason must be nonempty text"
    );
    if defect {
        ensure!(
            value["line"].as_u64().is_some_and(|line| line > 0),
            "defect line must be positive"
        );
        exact_keys(&value["reproduction"], &["input", "expected", "actual"])?;
    } else {
        ensure!(
            value["line"].is_null() && value["reproduction"].is_null(),
            "clean review requires null line and reproduction"
        );
    }
    Ok(value)
}
fn exact_keys(value: &Value, names: &[&str]) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("review field must be an object"))?;
    ensure!(
        object.len() == names.len() && names.iter().all(|name| object.contains_key(*name)),
        "review object has missing or extra fields"
    );
    Ok(())
}
fn bounded(value: &Value, depth: usize, count: &mut usize) -> Result<()> {
    ensure!(depth <= MAXIMUM_DEPTH, "review JSON exceeds depth limit");
    match value {
        Value::Array(values) => {
            *count += values.len();
            ensure!(*count <= MAXIMUM_ITEMS, "review exceeds total item limit");
            for child in values {
                bounded(child, depth + 1, count)?;
            }
        }
        Value::Object(values) => {
            *count += values.len();
            ensure!(*count <= MAXIMUM_ITEMS, "review exceeds total item limit");
            ensure!(
                values.keys().all(|key| key.len() <= MAXIMUM_STRING_BYTES),
                "review object key exceeds byte limit"
            );
            ensure!(
                !values
                    .keys()
                    .any(|key| FORBIDDEN_KEYS.contains(&key.as_str())),
                "reserved review data key"
            );
            for child in values.values() {
                bounded(child, depth + 1, count)?;
            }
        }
        Value::String(text) => ensure!(
            text.len() <= MAXIMUM_STRING_BYTES,
            "review string exceeds byte limit"
        ),
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_golden_and_strict_json() {
        let golden: Value = serde_json::from_slice(include_bytes!(
            "../../../../bench/agent-rsi/review-output-v2/golden.json"
        ))
        .unwrap();
        for value in golden["valid"].as_array().unwrap() {
            validate(&serde_json::to_vec(value).unwrap()).unwrap();
        }
        for value in golden["invalid"].as_array().unwrap() {
            assert!(validate(&serde_json::to_vec(value).unwrap()).is_err());
        }
        for case in golden["raw_cases"].as_array().unwrap() {
            assert_eq!(
                validate(case["raw"].as_str().unwrap().as_bytes()).is_ok(),
                case["valid"].as_bool().unwrap(),
                "{}",
                case["id"]
            );
        }
        for bytes in [
            r#"{"has_defect":false,"has_defect":true,"line":null,"reason":"ok","reproduction":null}"#,
            r#"{"has_defect":false,"line":null,"reason":"ok","reproduction":null} {}"#,
            "```json\n{}\n```",
            "process.exit()",
            "[]",
            r#"{"has_defect":true,"line":1,"reason":"ok","reproduction":{"input":{},"expected":-1,"actual":0}}"#,
        ] {
            assert!(validate(bytes.as_bytes()).is_err(), "accepted {bytes}");
        }
        for literal in ["-0", "1.0", "1e0", "9007199254740992"] {
            let raw = format!(
                r#"{{"has_defect":true,"line":1,"reason":"ok","reproduction":{{"input":{{}},"expected":{literal},"actual":0}}}}"#
            );
            assert!(
                validate(raw.as_bytes()).is_err(),
                "accepted numeric lexeme {literal}"
            );
        }
        let parsed: Value = serde_json::from_slice(CONTRACT).unwrap();
        assert_eq!(parsed["maximum_report_bytes"], MAXIMUM_REPORT_BYTES);
        assert_eq!(parsed["maximum_json_depth"], MAXIMUM_DEPTH);
        assert_eq!(parsed["maximum_collection_items"], MAXIMUM_ITEMS);
        assert_eq!(parsed["root_json_depth"], 1);
        assert_eq!(parsed["maximum_string_utf8_bytes"], MAXIMUM_STRING_BYTES);
        assert_eq!(
            parsed["forbidden_object_keys"],
            serde_json::json!(FORBIDDEN_KEYS)
        );
        assert_eq!(parsed["instructions"], instructions());
    }
    #[test]
    fn bounded_data_rejects_nested_duplicates_reserved_keys_size_and_depth() {
        assert!(validate(br#"{"has_defect":true,"line":1,"reason":"ok","reproduction":{"input":{"a":0,"a":1},"expected":0,"actual":1}}"#).is_err());
        for input in [
            serde_json::json!({"__proto__":{}}),
            serde_json::json!(vec![0; 129]),
            serde_json::json!({"a":{"b":{"c":{"d":{"e":{"f":0}}}}}}),
        ] {
            let report = serde_json::json!({"has_defect":true,"line":1,"reason":"ok","reproduction":{"input":input,"expected":0,"actual":1}});
            assert!(validate(&serde_json::to_vec(&report).unwrap()).is_err());
        }
        let report = serde_json::json!({"has_defect":false,"line":null,"reason":"中".repeat(1366),"reproduction":null});
        assert!(validate(&serde_json::to_vec(&report).unwrap()).is_err());
        assert!(validate(&vec![b' '; MAXIMUM_REPORT_BYTES + 1]).is_err());
    }
}
