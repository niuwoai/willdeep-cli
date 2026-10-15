//! Strict, cross-platform RSI wire validation. Registration never changes installations.
use crate::agent_definition::{AgentPackage, digest, valid_hash};
use anyhow::{Result, bail, ensure};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

pub const MAX_BYTES: usize = 2 * 1024 * 1024;
pub const SAFE_INTEGER: u64 = 9_007_199_254_740_991;

struct Strict;
impl<'de> DeserializeSeed<'de> for Strict {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Value, D::Error> {
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Strict {
    type Value = Value;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("unique JSON with nonnegative safe integers")
    }
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_string<E: serde::de::Error>(self, v: String) -> std::result::Result<Value, E> {
        Ok(v.into())
    }
    fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Value, E> {
        if v <= SAFE_INTEGER {
            Ok(v.into())
        } else {
            Err(E::custom("integer exceeds safe range"))
        }
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> std::result::Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(v) = a.next_element_seed(Strict)? {
            values.push(v);
        }
        Ok(values.into())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> std::result::Result<Value, A::Error> {
        let mut values = serde_json::Map::new();
        while let Some(k) = a.next_key::<String>()? {
            if values.contains_key(&k) {
                return Err(serde::de::Error::custom("duplicate JSON key"));
            }
            values.insert(k, a.next_value_seed(Strict)?);
        }
        Ok(Value::Object(values))
    }
}

pub fn parse_json(bytes: &[u8]) -> Result<Value> {
    ensure!(bytes.len() <= MAX_BYTES, "RSI object exceeds size limit");
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    let value = Strict.deserialize(&mut parser)?;
    parser.end()?;
    Ok(value)
}
pub fn canonical(value: &Value) -> Result<String> {
    Ok(match value {
        Value::Null => "null".into(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => {
            ensure!(
                v.as_u64().is_some_and(|n| n <= SAFE_INTEGER),
                "invalid integer"
            );
            v.to_string()
        }
        Value::String(v) => crate::agent_definition::canonical::string(v),
        Value::Array(a) => format!(
            "[{}]",
            a.iter()
                .map(canonical)
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ),
        Value::Object(m) => {
            let m: std::collections::BTreeMap<_, _> = m.iter().collect();
            format!(
                "{{{}}}",
                m.iter()
                    .map(|(k, v)| Ok(format!(
                        "{}:{}",
                        crate::agent_definition::canonical::string(k),
                        canonical(v)?
                    )))
                    .collect::<Result<Vec<_>>>()?
                    .join(",")
            )
        }
    })
}
pub fn hash(value: &Value) -> Result<String> {
    Ok(digest(canonical(value)?.as_bytes()))
}
pub fn golden() -> Value {
    serde_json::from_str(include_str!(
        "../../../../docs/examples/agent-rsi-golden.json"
    ))
    .expect("bundled RSI golden")
}

fn shape(value: &Value, template: &Value, path: &str) -> Result<()> {
    match template {
        Value::Object(t) => {
            let v = value
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("expected object at {path}"))?;
            if path.ends_with(".name") {
                ensure!(
                    !v.is_empty() && v.len() <= 16 && v.values().all(Value::is_string),
                    "invalid display-name dictionary"
                );
                return Ok(());
            }
            ensure!(
                v.len() == t.len() && t.keys().all(|k| v.contains_key(k)),
                "unknown or missing fields at {path}"
            );
            for (k, t) in t {
                shape(&v[k], t, &format!("{path}.{k}"))?;
            }
        }
        Value::Array(t) => {
            let a = value
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("expected array at {path}"))?;
            if let Some(t) = t.first() {
                for v in a {
                    shape(v, t, path)?;
                }
            }
        }
        Value::String(_) => ensure!(value.is_string(), "expected string at {path}"),
        Value::Number(_) => ensure!(
            value.as_u64().is_some_and(|n| n <= SAFE_INTEGER),
            "expected integer at {path}"
        ),
        Value::Bool(_) => ensure!(value.is_boolean(), "expected bool at {path}"),
        Value::Null => {
            let leaf = path.rsplit('.').next().unwrap_or("");
            if leaf == "signature" {
                ensure!(value.is_null() || value.is_object(), "invalid signature");
            } else if [
                "exit_code",
                "input_tokens",
                "output_tokens",
                "auxiliary_tokens",
                "elapsed_ms",
                "cost_microusd",
            ]
            .contains(&leaf)
            {
                ensure!(
                    value.is_null() || value.as_u64().is_some_and(|n| n <= SAFE_INTEGER),
                    "invalid nullable integer at {path}"
                );
            } else {
                ensure!(
                    value.is_null() || value.is_string(),
                    "invalid nullable string at {path}"
                );
            }
        }
    }
    Ok(())
}
pub fn validate(value: &Value) -> Result<()> {
    let schema = value["schema"].as_str().unwrap_or("");
    let entity = match schema {
        "willdeep.agent-rsi.experiment.v1" => "experiment",
        "willdeep.agent-rsi.evidence.v1" => "evidence",
        "willdeep.agent-rsi.candidate.v1" => "candidate",
        "willdeep.agent-rsi.grant.v1" => "grant",
        "willdeep.agent-rsi.admission.v1" => "admission",
        _ => bail!("unsupported RSI schema"),
    };
    let mut template = golden()["values"][entity].clone();
    // Golden concrete values may be nonnull where the contract permits null.
    for pointer in [
        "/grant_id",
        "/finished_at",
        "/worker_run_id",
        "/events_sha256",
        "/verifier/exit_code",
        "/usage/input_tokens",
        "/usage/output_tokens",
        "/usage/auxiliary_tokens",
        "/usage/elapsed_ms",
        "/usage/cost_microusd",
    ] {
        if pointer == "/grant_id" && entity != "experiment" {
            continue;
        }
        if let Some(v) = template.pointer_mut(pointer) {
            *v = Value::Null;
        }
    }
    shape(value, &template, entity)?;
    canonical(value)?;
    validate_hashes(value, "")?;
    validate_identifiers(value, "")?;
    if entity == "candidate" {
        validate_candidate(value)?;
    }
    if entity == "experiment" {
        validate_experiment(value)?;
    }
    if entity == "evidence" {
        validate_evidence(value)?;
    }
    if entity == "grant" {
        ensure!(
            ["private_evaluation", "official_contribution"]
                .contains(&value["purpose"].as_str().unwrap_or(""))
                && value["material_scope"] == serde_json::json!(["execution_summary"]),
            "unsupported evidence grant scope"
        );
        ensure!(
            valid_timestamp(value["expires_at"].as_str().unwrap_or("")),
            "invalid grant expiry"
        );
    }
    if entity == "admission" {
        ensure!(
            ["baseline", "candidate"].contains(&value["arm"].as_str().unwrap_or(""))
                && value["estimated_cost_microusd"] == 0,
            "paid_not_approved"
        );
        bounded(value, "attempt", 1, 10)?;
        bounded(value, "repetition", 1, 10)?;
        ensure!(
            (value["attempt"] == 1) == value["resume_of"].is_null(),
            "invalid resume lineage"
        );
    }
    Ok(())
}
fn validate_hashes(v: &Value, key: &str) -> Result<()> {
    if (key.ends_with("sha256") || key.ends_with("revision") && key != "model_revision")
        && !v.is_null()
    {
        ensure!(v.as_str().is_some_and(valid_hash), "invalid hash {key}");
    }
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                validate_hashes(v, k)?;
            }
        }
        Value::Array(a) => {
            for v in a {
                validate_hashes(v, "")?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn validate_identifiers(v: &Value, key: &str) -> Result<()> {
    if (key.ends_with("_id") || key == "resume_of") && !v.is_null() {
        let id = v.as_str().unwrap_or("");
        ensure!(
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._:/-".contains(&b)),
            "invalid identifier {key}"
        );
    }
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                validate_identifiers(v, k)?;
            }
        }
        Value::Array(a) => {
            for v in a {
                validate_identifiers(v, "")?;
            }
        }
        _ => {}
    }
    Ok(())
}
pub fn validate_candidate(value: &Value) -> Result<()> {
    let baseline = AgentPackage::parse(&serde_json::to_vec(&value["baseline_package"])?)?;
    let candidate = AgentPackage::parse(&serde_json::to_vec(&value["candidate_package"])?)?;
    ensure!(baseline.sha256 != candidate.sha256, "candidate must differ");
    ensure!(
        baseline.definition.version != candidate.definition.version
            && baseline.definition.instructions != candidate.definition.instructions,
        "candidate requires a new version and instruction change"
    );
    ensure!(
        candidate.definition.forked_from.as_deref() == Some(&baseline.sha256),
        "candidate parent mismatch"
    );
    let mut expected = baseline.definition.clone();
    expected.instructions = candidate.definition.instructions.clone();
    expected.version = candidate.definition.version.clone();
    expected.forked_from = candidate.definition.forked_from.clone();
    ensure!(
        expected == candidate.definition,
        "candidate may only change instructions, version and parent"
    );
    Ok(())
}
fn bounded(v: &Value, key: &str, min: u64, max: u64) -> Result<()> {
    ensure!(
        v[key].as_u64().is_some_and(|n| (min..=max).contains(&n)),
        "invalid {key}"
    );
    Ok(())
}
fn validate_experiment(v: &Value) -> Result<()> {
    ensure!(
        v["baseline_revision"] != v["candidate_revision"],
        "candidate revision must differ from baseline"
    );
    ensure!(v["execution_policy"] == "offline_only", "paid_not_approved");
    ensure!(
        v["model"]["provider"] == "mock" && v["budget"]["max_cost_microusd"] == 0,
        "paid_not_approved"
    );
    ensure!(
        ["direct", "team"].contains(&v["execution_mode"].as_str().unwrap_or("")),
        "invalid execution mode"
    );
    bounded(v, "repetitions", 1, 10)?;
    for (k, max) in [
        ("max_turns", 1000),
        ("max_tokens", 10_000_000),
        ("timeout_seconds", 86400),
        ("max_attempts", 10),
        ("max_parallel_trials", 4),
        ("max_candidates", 1),
        ("max_holdout_accesses", 128),
    ] {
        bounded(&v["budget"], k, 1, max)?;
    }
    let tasks = v["dataset"]["tasks"].as_array().unwrap();
    ensure!(
        !tasks.is_empty() && tasks.len() <= 128,
        "invalid task count"
    );
    let mut ids = std::collections::BTreeSet::new();
    let mut splits = std::collections::BTreeSet::new();
    for task in tasks {
        ensure!(
            ids.insert(task["task_id"].as_str().unwrap()),
            "duplicate task"
        );
        let split = task["split"].as_str().unwrap();
        ensure!(
            ["validation", "regression", "holdout"].contains(&split),
            "invalid split"
        );
        splits.insert(split);
    }
    ensure!(splits.len() == 3, "all three splits are required");
    Ok(())
}
fn validate_evidence(v: &Value) -> Result<()> {
    ensure!(
        ["baseline", "candidate"].contains(&v["arm"].as_str().unwrap_or("")),
        "invalid arm"
    );
    ensure!(
        [
            "passed",
            "failed",
            "unverified",
            "infra_error",
            "interrupted",
            "budget_exhausted",
            "invalid"
        ]
        .contains(&v["status"].as_str().unwrap_or("")),
        "invalid status"
    );
    ensure!(
        ["local_mock", "client_reported"].contains(&v["trust"].as_str().unwrap_or("")),
        "invalid trust"
    );
    bounded(v, "repetition", 1, 10)?;
    bounded(v, "attempt", 1, 10)?;
    ensure!(
        ["validation", "regression", "holdout"].contains(&v["split"].as_str().unwrap_or("")),
        "invalid split"
    );
    ensure!(
        ["passed", "failed", "unverified"]
            .contains(&v["verifier"]["result"].as_str().unwrap_or("")),
        "invalid verifier result"
    );
    ensure!(
        valid_timestamp(v["started_at"].as_str().unwrap_or("")),
        "invalid start timestamp"
    );
    if !v["finished_at"].is_null() {
        ensure!(
            valid_timestamp(v["finished_at"].as_str().unwrap_or("")),
            "invalid finish timestamp"
        );
    }
    if v["status"] == "passed" {
        ensure!(
            v["target_executed"] == true
                && !v["worker_run_id"].is_null()
                && !v["finished_at"].is_null()
                && !v["events_sha256"].is_null()
                && v["verifier"]["result"] == "passed"
                && v["verifier"]["exit_code"] == 0,
            "missing target or verifier evidence"
        );
    }
    Ok(())
}
fn valid_timestamp(value: &str) -> bool {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    if !PATTERN
        .get_or_init(|| {
            regex::Regex::new(
                r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]{1,9})?Z$",
            )
            .unwrap()
        })
        .is_match(value)
    {
        return false;
    }
    crate::session::parse_iso8601_utc(&format!("{}Z", &value[..19])).is_some()
}

pub fn bind_evidence(spec: &Value, evidence: &Value) -> Result<()> {
    validate(spec)?;
    validate(evidence)?;
    for key in [
        "experiment_id",
        "definition_id",
        "runtime",
        "model",
        "conditions",
        "budget",
    ] {
        ensure!(spec[key] == evidence[key], "evidence {key} mismatch");
    }
    let revision = if evidence["arm"] == "baseline" {
        &spec["baseline_revision"]
    } else {
        &spec["candidate_revision"]
    };
    ensure!(
        evidence["model_route"]
            .as_array()
            .unwrap()
            .iter()
            .all(|route| route["provider"] == spec["model"]["provider"]
                && route["model"] == spec["model"]["model"]
                && route["model_revision"] == spec["model"]["model_revision"]),
        "paid or unregistered route denied"
    );
    if evidence["target_executed"] == true {
        ensure!(
            !evidence["model_route"].as_array().unwrap().is_empty(),
            "missing actual model route"
        );
    }
    ensure!(
        revision == &evidence["definition_revision"],
        "effective revision mismatch"
    );
    let task = spec["dataset"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["task_id"] == evidence["task_id"])
        .ok_or_else(|| anyhow::anyhow!("unknown task"))?;
    for key in ["split", "task_sha256"] {
        ensure!(task[key] == evidence[key], "task {key} mismatch");
    }
    ensure!(
        evidence["dataset_sha256"] == spec["dataset"]["suite_sha256"]
            && evidence["fixture_sha256"] == spec["dataset"]["fixture_sha256"]
            && evidence["verifier"]["verifier_sha256"] == spec["dataset"]["verifier_sha256"]
            && evidence["skills_sha256"] == spec["conditions"]["skills_sha256"],
        "evidence dependency mismatch"
    );
    ensure!(
        evidence["repetition"].as_u64().unwrap() <= spec["repetitions"].as_u64().unwrap()
            && evidence["attempt"].as_u64().unwrap()
                <= spec["budget"]["max_attempts"].as_u64().unwrap(),
        "trial exceeds limits"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_golden_and_strict_wire() {
        let g = golden();
        for (key, value) in g["values"].as_object().unwrap() {
            assert_eq!(
                canonical(value).unwrap(),
                g["canonical"][key].as_str().unwrap(),
                "{key}"
            );
            assert_eq!(
                hash(value).unwrap(),
                g["sha256"][key].as_str().unwrap(),
                "{key}"
            );
            if key != "canonical_edge" {
                validate(value).unwrap();
            }
        }
        for bad in [
            r#"{"a":1,"a":2}"#,
            r#"{"a":1.0}"#,
            r#"{"a":1e2}"#,
            r#"{"a":-1}"#,
            r#"{"a":9007199254740992}"#,
        ] {
            assert!(parse_json(bad.as_bytes()).is_err(), "{bad}");
        }
        bind_evidence(&g["values"]["experiment"], &g["values"]["evidence"]).unwrap();
    }
    #[test]
    fn tampering_and_missing_evidence_fail_closed() {
        let g = golden();
        let spec = &g["values"]["experiment"];
        let mut v = g["values"]["evidence"].clone();
        v["target_executed"] = false.into();
        assert!(bind_evidence(spec, &v).is_err());
        v = g["values"]["evidence"].clone();
        v["unexpected"] = true.into();
        assert!(validate(&v).is_err());
        v = g["values"]["evidence"].clone();
        v["definition_revision"] = "0".repeat(64).into();
        assert!(bind_evidence(spec, &v).is_err());
    }
    #[test]
    fn shared_negative_corpus_is_rejected() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../../docs/examples/agent-rsi-negative.json"
        ))
        .unwrap();
        let golden = golden();
        for case in corpus["cases"].as_array().unwrap() {
            let result = parse_json(case["raw"].as_str().unwrap().as_bytes()).and_then(|value| {
                if case["kind"] == "binding" {
                    bind_evidence(&golden["values"]["experiment"], &value)
                } else {
                    validate(&value)
                }
            });
            assert!(result.is_err(), "accepted invalid case {}", case["id"]);
        }
    }
    #[test]
    fn dates_and_grant_nullability_are_strict() {
        let g = golden();
        let mut grant = g["values"]["grant"].clone();
        grant["grant_id"] = Value::Null;
        assert!(validate(&grant).is_err());
        let mut candidate = g["values"]["candidate"].clone();
        candidate["grant_id"] = Value::Null;
        assert!(validate(&candidate).is_err());
        let mut spec = g["values"]["experiment"].clone();
        spec["grant_id"] = Value::Null;
        validate(&spec).unwrap();
        for date in [
            "2026-10-15",
            " 2026-10-15T00:00:00Z",
            "2026-02-30T00:00:00Z",
            "2026-10-15T24:00:00Z",
        ] {
            assert!(!valid_timestamp(date));
        }
        assert!(valid_timestamp("2026-10-15T00:00:00.123Z"));
    }
    #[test]
    fn shared_skill_fingerprint_binds_full_contents_flags_and_order() {
        let sample: Value = serde_json::from_str(include_str!(
            "../../../../docs/examples/agent-rsi-skills-golden.json"
        ))
        .unwrap();
        assert_eq!(canonical(&sample["skills"]).unwrap(), sample["canonical"]);
        assert_eq!(hash(&sample["skills"]).unwrap(), sample["sha256"]);
        let mut changed = sample["skills"].clone();
        changed[0]["required"] = false.into();
        assert_ne!(hash(&changed).unwrap(), sample["sha256"]);
        changed = sample["skills"].clone();
        changed.as_array_mut().unwrap().reverse();
        assert_ne!(hash(&changed).unwrap(), sample["sha256"]);
    }
}
