//! Read-only, content-free originating conversation usage, separate from job quota.
use super::*;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Read},
};
use willdeep_core::usage_ledger::{MAX_LINE_BYTES, SCHEMA, UsageLedgerRecord, ledger_dir};

const MAX_LEDGER_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) fn conversation(home: &Path, workflow: &Value) -> Value {
    let Some(session) = workflow["input"]["origin_session_id"]
        .as_str()
        .and_then(|id| id.parse::<Uuid>().ok())
    else {
        return json!({"state":"unlinked","scope":"entire_origin_session","tokens":null,"calls":null});
    };
    match records(home, session, &workflow["input"]["workspace"]) {
        Ok((calls, tokens, unknown)) if calls > 0 => {
            let job = &workflow["usage"];
            let settled = job["unknown_requests"] == json!([])
                && job["reserved"]
                    .as_object()
                    .is_some_and(|values| values.values().all(|value| value.as_u64() == Some(0)));
            let combined = if unknown == 0 && settled {
                tokens.checked_add(job["used"]["tokens"].as_u64().unwrap_or(0))
            } else {
                None
            };
            json!({"state":if unknown == 0 {"recorded"} else {"incomplete"},"scope":"entire_origin_session",
                "calls":calls,"tokens":tokens,"unknownRecords":unknown,"combinedSettledTokens":combined})
        }
        _ => {
            json!({"state":"unavailable","scope":"entire_origin_session","calls":null,"tokens":null})
        }
    }
}

fn records(home: &Path, session: Uuid, workspace: &Value) -> Result<(u64, u64, u64)> {
    let workspace = Path::new(
        workspace
            .as_str()
            .context("production_origin_workspace_missing")?,
    );
    let mut ids = HashMap::<Uuid, UsageLedgerRecord>::new();
    let (mut calls, mut tokens, mut unknown, mut bytes) = (0u64, 0u64, 0u64, 0u64);
    for entry in std::fs::read_dir(ledger_dir(home))? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
            continue;
        }
        let file = File::open(path)?;
        bytes = bytes
            .checked_add(file.metadata()?.len())
            .context("production_ledger_overflow")?;
        anyhow::ensure!(bytes <= MAX_LEDGER_BYTES, "production_ledger_scan_limit");
        let mut reader = BufReader::new(file);
        loop {
            let mut line = Vec::new();
            let length = reader
                .by_ref()
                .take(MAX_LINE_BYTES as u64 + 1)
                .read_until(b'\n', &mut line)?;
            if length == 0 {
                break;
            }
            anyhow::ensure!(
                length <= MAX_LINE_BYTES && line.last() == Some(&b'\n'),
                "production_ledger_incomplete"
            );
            let record: UsageLedgerRecord = serde_json::from_slice(&line)?;
            anyhow::ensure!(record.schema == SCHEMA, "production_ledger_schema_unknown");
            if record.session_id != Some(session) {
                continue;
            }
            if let Some(previous) = ids.get(&record.id) {
                anyhow::ensure!(previous == &record, "production_ledger_identity_conflict");
                continue;
            }
            calls = calls.checked_add(1).context("production_ledger_overflow")?;
            if record.workspace.as_deref() != Some(workspace)
                || !(record.usage.total_tokens.is_some()
                    || (record.usage.input_tokens.is_some()
                        && record.usage.output_tokens.is_some()))
            {
                unknown += 1;
            } else {
                tokens = tokens
                    .checked_add(record.usage.billable_tokens())
                    .context("production_ledger_overflow")?;
            }
            ids.insert(record.id, record);
        }
    }
    Ok((calls, tokens, unknown))
}

#[cfg(test)]
mod tests {
    use super::*;
    use willdeep_core::usage_ledger::{Execution, UsageKind};

    #[test]
    fn conversation_is_scoped_deduplicated_and_never_claims_missing_usage_is_free() {
        let root = std::env::temp_dir().join(format!("production-accounting-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let session = Uuid::new_v4();
        let mut public = json!({"input":{"workspace":root}});
        assert_eq!(conversation(&root, &public)["state"], "unlinked");
        public["input"]["origin_session_id"] = json!(session);
        public["usage"] = json!({"used":{"tokens":20},"reserved":{},"unknown_requests":[]});
        assert_eq!(conversation(&root, &public)["state"], "unavailable");
        std::fs::create_dir_all(ledger_dir(&root)).unwrap();
        let mut record = UsageLedgerRecord::new(UsageKind::Main, Execution::InProcess);
        record.session_id = Some(session);
        record.workspace = Some(root.clone());
        record.usage.total_tokens = Some(120);
        let line = serde_json::to_string(&record).unwrap() + "\n";
        let path = ledger_dir(&root).join("2026-10.jsonl");
        std::fs::write(&path, line.repeat(2)).unwrap();
        let result = conversation(&root, &public);
        assert_eq!(result["calls"], 1);
        assert_eq!(result["tokens"], 120);
        assert_eq!(result["combinedSettledTokens"], 140);
        record.id = Uuid::new_v4();
        record.usage.total_tokens = None;
        std::fs::write(
            &path,
            line.clone() + &serde_json::to_string(&record).unwrap() + "\n",
        )
        .unwrap();
        let result = conversation(&root, &public);
        assert_eq!(result["state"], "incomplete");
        assert!(result["combinedSettledTokens"].is_null());
        std::fs::write(path, line + "{partial").unwrap();
        assert_eq!(conversation(&root, &public)["state"], "unavailable");
        std::fs::remove_dir_all(root).unwrap();
    }
}
