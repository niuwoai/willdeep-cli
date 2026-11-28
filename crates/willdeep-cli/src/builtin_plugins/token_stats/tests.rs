use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use serde_json::{Value, json};
use uuid::Uuid;
use willdeep_core::usage_ledger::{
    ClientKind, Execution, MAX_LINE_BYTES, Outcome, UsageKind, UsageLedgerRecord, append_record,
    parse_ts,
};

use super::{Query, TokenStats, read_report};
use crate::builtin_plugins::stdio_server::serve;

#[tokio::test]
async fn mcp_query_reads_only_the_configured_home_ledger() {
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, duplex, split};
    use willdeep_core::usage_ledger::ledger_dir;

    let home = Fixture::new();
    let record = row("2026-09-15T12:00:00Z", UsageKind::Main);
    append_record(&ledger_dir(&home.home), &record).unwrap();
    let (client, server) = duplex(64 * 1024);
    let (server_read, server_write) = split(server);
    let task = tokio::spawn(serve(
        Arc::new(TokenStats::new(home.home.clone())),
        server_read,
        server_write,
    ));
    let mut client = BufReader::new(client);
    let arguments = args("2026-09-15", "2026-09-15", "UTC", "all", "day");
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "query", "arguments": arguments},
    });
    client
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let mut reply = String::new();
    client.read_line(&mut reply).await.unwrap();
    let reply: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(reply["result"]["isError"], false);
    let report: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(report["totals"]["calls"], 1);
    assert_eq!(report["models"][0]["model"], "model-a");
    drop(client);
    task.await.unwrap();
}

struct Fixture {
    home: PathBuf,
    ledger: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let home =
            std::env::temp_dir().join(format!("willdeep-token-stats-test-{}", Uuid::new_v4()));
        let ledger = home.join("usage");
        fs::create_dir_all(&ledger).unwrap();
        Self { home, ledger }
    }

    fn add(&self, record: &UsageLedgerRecord) {
        append_record(&self.ledger, record).unwrap();
    }

    fn raw(&self, shard: &str, contents: &[u8]) {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.ledger.join(shard))
            .unwrap()
            .write_all(contents)
            .unwrap();
    }

    fn session(&self, updated_at: u64, messages: Value) {
        let sessions = self.home.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let body = json!({"version": 1, "updated_at": updated_at, "messages": messages});
        fs::write(
            sessions.join(format!("{}.json", Uuid::new_v4())),
            serde_json::to_vec(&body).unwrap(),
        )
        .unwrap();
    }

    fn report(&self, start: &str, end: &str, timezone: &str, source: &str, detail: &str) -> Value {
        let query = Query::parse(args(start, end, timezone, source, detail)).unwrap();
        let report = read_report(&self.ledger, &self.home, query).unwrap();
        serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.home) {
            eprintln!("Could not clean up the token statistics test fixture: {error}");
        }
    }
}

fn args(start: &str, end: &str, timezone: &str, source: &str, detail: &str) -> Value {
    json!({"start": start, "end": end, "timezone": timezone, "source": source, "detail": detail})
}

fn row(ts: &str, kind: UsageKind) -> UsageLedgerRecord {
    let mut record = UsageLedgerRecord::new(kind, Execution::InProcess);
    record.ts = parse_ts(ts).unwrap();
    record.model = Some("model-a".into());
    record.provider = Some("provider-a".into());
    record
}

#[test]
fn rejects_invalid_queries_before_reading() {
    for (start, end, timezone, source, detail) in [
        ("2024-01-01", "2025-01-01", "UTC", "all", "summary"),
        ("2024-01-02", "2024-01-01", "UTC", "all", "summary"),
        ("2024-01-01", "2024-01-02", "UTC", "all", "day"),
        ("2024-1-01", "2024-01-01", "UTC", "all", "summary"),
        ("2024-02-30", "2024-03-03", "UTC", "all", "summary"),
        ("2024-01-01", "2024-01-01", "+05:00", "all", "summary"),
        ("2024-01-01", "2024-01-01", "UTC", "web", "summary"),
        ("2024-01-01", "2024-01-01", "UTC", "all", "raw"),
    ] {
        assert!(Query::parse(args(start, end, timezone, source, detail)).is_err());
    }
    let valid = args(
        "2024-01-01",
        "2024-12-31",
        "Asia/Shanghai",
        "all",
        "summary",
    );
    assert!(Query::parse(valid.clone()).is_ok());
    let mut extra = valid;
    extra["path"] = json!("usage.jsonl");
    assert!(Query::parse(extra).is_err());
}

#[test]
fn uses_local_calendar_dates_across_utc_shards_and_historical_transitions() {
    for (day, timezone, timestamps) in [
        (
            "2026-10-01",
            "Asia/Shanghai",
            [
                "2026-09-30T15:59:59Z",
                "2026-09-30T16:00:00Z",
                "2026-10-01T15:59:59Z",
                "2026-10-01T16:00:00Z",
            ],
        ),
        (
            "2026-09-30",
            "America/New_York",
            [
                "2026-09-30T03:59:59Z",
                "2026-09-30T04:00:00Z",
                "2026-10-01T03:59:59Z",
                "2026-10-01T04:00:00Z",
            ],
        ),
        (
            "2026-03-08",
            "America/New_York",
            [
                "2026-03-08T04:59:59Z",
                "2026-03-08T05:00:00Z",
                "2026-03-09T03:59:59Z",
                "2026-03-09T04:00:00Z",
            ],
        ),
        (
            "2026-11-01",
            "America/New_York",
            [
                "2026-11-01T03:59:59Z",
                "2026-11-01T05:30:00Z",
                "2026-11-01T06:30:00Z",
                "2026-11-02T05:00:00Z",
            ],
        ),
    ] {
        let fixture = Fixture::new();
        for ts in timestamps {
            fixture.add(&row(ts, UsageKind::Main));
        }
        let result = fixture.report(day, day, timezone, "all", "day");
        assert_eq!(result["totals"]["calls"], 2, "{day} {timezone}");
        assert_eq!(result["days"][0]["date"], day);
        assert_eq!(result["models"][0]["totals"]["calls"], 2);
    }
    let fixture = Fixture::new();
    fixture.add(&row("2011-12-30T09:59:59Z", UsageKind::Main));
    fixture.add(&row("2011-12-30T10:00:00Z", UsageKind::Main));
    let result = fixture.report("2011-12-30", "2011-12-30", "Pacific/Apia", "all", "day");
    assert_eq!(result["totals"]["calls"], 0);
}

#[test]
fn deduplicates_calls_and_counts_complete_bad_rows() {
    let fixture = Fixture::new();
    let record = row("2026-09-30T18:00:00Z", UsageKind::Main);
    fixture.add(&record);
    fixture.raw("2026-10.jsonl", &record.to_line().unwrap());
    fixture.raw("2026-09.jsonl", &[0xff, b'\n']);
    let mut long = vec![b'x'; MAX_LINE_BYTES * 2];
    long.push(b'\n');
    fixture.raw("2026-09.jsonl", &long);
    let mut wrong_schema = record.clone();
    wrong_schema.schema = "unknown-schema".into();
    fixture.add(&wrong_schema);
    let invalid_ts = record.to_line().unwrap();
    let invalid_ts = String::from_utf8(invalid_ts)
        .unwrap()
        .replace("2026-09-30", "2026-09-31");
    fixture.raw("2026-09.jsonl", invalid_ts.as_bytes());
    fixture.add(&row("2026-09-30T18:00:01Z", UsageKind::Main));
    fixture.raw("2026-10.jsonl", b"{\"incomplete\":");
    fixture.raw("2026-04.jsonl", b"{malformed unselected shard}\n");
    let result = fixture.report(
        "2026-10-01",
        "2026-10-01",
        "Asia/Shanghai",
        "all",
        "summary",
    );
    assert_eq!(result["totals"]["calls"], 2);
    assert_eq!(result["skipped_rows"], 4);
}

#[test]
fn rolls_up_usage_independently_and_filters_source() {
    let fixture = Fixture::new();
    let mut first = row("2026-09-02T12:00:00Z", UsageKind::Main);
    first.session_id = Some(Uuid::new_v4());
    first.client = ClientKind::Cli;
    first.usage.input_tokens = Some(100);
    first.usage.cache_read_tokens = Some(25);
    first.usage.output_tokens = Some(0);
    first.workspace = Some(PathBuf::from("synthetic-private-workspace"));
    fixture.add(&first);
    let mut second = row("2026-09-03T12:00:00Z", UsageKind::Main);
    second.session_id = first.session_id;
    second.client = ClientKind::Cli;
    second.usage.input_tokens = Some(300);
    second.usage.total_tokens = Some(310);
    second.outcome = Outcome::Error;
    fixture.add(&second);
    let mut third = row("2026-09-03T13:00:00Z", UsageKind::Main);
    third.client = ClientKind::Tui;
    third.outcome = Outcome::Cancelled;
    fixture.add(&third);
    let result = fixture.report("2026-09-01", "2026-09-03", "UTC", "cli", "summary");
    assert_eq!(
        result["totals"],
        json!({
            "tokens": 310, "input": 400, "output": 0, "cache_read": 25, "cache_rate": 0.25,
            "calls": 2, "sessions": 1, "active_days": 2, "known_usage_calls": 2,
            "unpriced_calls": 2, "worker_runs": 0, "unknown_run_calls": 0,
            "errors": 1, "cancelled": 0
        })
    );
    assert_eq!(result["days"][0]["totals"]["calls"], 0);
    assert!(result["days"][1]["totals"]["tokens"].is_null());
    assert_eq!(result["days"][1]["totals"]["sessions"], 1);
    assert_eq!(result["days"][2]["totals"]["sessions"], 1);
    assert_eq!(result["days"][0]["source_calls"], json!({}));
    assert_eq!(result["days"][2]["source_calls"], json!({"cli": 1}));
    let all = fixture.report("2026-09-03", "2026-09-03", "UTC", "all", "summary");
    assert_eq!(all["days"][0]["source_calls"], json!({"cli": 1, "tui": 1}));
    assert!(!result.to_string().contains("synthetic-private-workspace"));
    assert!(
        !result
            .to_string()
            .contains(&first.session_id.unwrap().to_string())
    );
    let tui = fixture.report("2026-09-03", "2026-09-03", "UTC", "tui", "day");
    assert_eq!(tui["totals"]["calls"], 1);
    assert_eq!(tui["totals"]["cancelled"], 1);
    assert_eq!(tui["totals"]["known_usage_calls"], 0);
    assert!(tui["totals"]["cache_rate"].is_null());
}

#[test]
fn attributes_worker_auxiliary_and_compression_without_inventing_runs() {
    let fixture = Fixture::new();
    let mut worker = row("2026-09-01T12:00:00Z", UsageKind::Subagent);
    worker.run_id = Some(Uuid::new_v4());
    fixture.add(&worker);
    let mut helper = row("2026-09-02T12:00:00Z", UsageKind::Auxiliary);
    helper.run_id = worker.run_id;
    helper.provider = Some("provider-b".into());
    fixture.add(&helper);
    let mut compression = row("2026-09-01T14:00:00Z", UsageKind::Compression);
    compression.agent_id = Some(Uuid::new_v4().to_string());
    fixture.add(&compression);
    let legacy = row("2026-09-01T15:00:00Z", UsageKind::Subagent);
    let mut legacy_json = serde_json::to_value(legacy).unwrap();
    legacy_json.as_object_mut().unwrap().remove("run_id");
    fixture.raw("2026-09.jsonl", format!("{legacy_json}\n").as_bytes());
    fixture.add(&row("2026-09-01T16:00:00Z", UsageKind::Auxiliary));
    let result = fixture.report("2026-09-01", "2026-09-02", "UTC", "unknown", "agents");
    assert_eq!(result["totals"]["calls"], 5);
    assert_eq!(result["totals"]["worker_runs"], 1);
    assert_eq!(result["totals"]["unknown_run_calls"], 2);
    assert_eq!(result["agents"].as_array().unwrap().len(), 2);
    assert_eq!(result["agents"][0]["totals"]["calls"], 3);
    assert_eq!(result["agents"][0]["totals"]["unknown_run_calls"], 2);
    assert_eq!(result["agents"][1]["totals"]["worker_runs"], 1);
    assert_eq!(result["agents"][0]["agent"], "model-a");
    assert_eq!(result["models"], json!([]));
    assert_eq!(result["skills"], json!([]));
}

#[test]
fn day_detail_lists_subagents_and_skill_reads() {
    let fixture = Fixture::new();
    let run = Uuid::new_v4();
    let mut reader = row("2026-09-02T00:30:00Z", UsageKind::Subagent);
    reader.run_id = Some(run);
    reader.model = Some("model-reader".into());
    fixture.add(&reader);
    let mut repeated = row("2026-09-02T01:00:00Z", UsageKind::Subagent);
    repeated.run_id = Some(run);
    repeated.model = Some("model-reader".into());
    fixture.add(&repeated);
    let mut unnamed = row("2026-09-02T02:00:00Z", UsageKind::Subagent);
    unnamed.model = None;
    fixture.add(&unnamed);
    fixture.add(&row("2026-09-02T03:00:00Z", UsageKind::Main));
    let read = |name: &str| {
        json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "call-1", "name": "read_skill", "arguments": serde_json::to_string(&json!({"name": name})).unwrap()}
        ]})
    };
    // 2026-09-02T12:00:00Z. A later session must not be counted for this UTC day.
    fixture.session(
        1_788_350_400,
        json!([
            read("image-processing"),
            read("image-processing"),
            {"role": "assistant", "content": "", "tool_calls": [
                {"id": "browse", "name": "list_skills", "arguments": "{\"name\":\"image-processing\"}"}
            ]},
            {"role": "assistant", "content": "", "tool_calls": [
                {"id": "bad-json", "name": "read_skill", "arguments": "not json"},
                {"id": "bad-name", "name": "read_skill", "arguments": "{\"name\":\"../secret\"}"}
            ]},
            {"role": "assistant", "content": "", "toolCalls": [
                {"id": "swift", "toolName": "read_skill", "argumentsJSON": "{\"name\":\"lark-doc\"}"}
            ]}
        ]),
    );
    fixture.session(1_788_393_600, json!([read("next-day")]));
    let result = fixture.report("2026-09-02", "2026-09-02", "UTC", "all", "day");
    assert_eq!(result["totals"]["calls"], 4);
    assert_eq!(result["models"].as_array().unwrap().len(), 3);
    assert_eq!(result["agents"].as_array().unwrap().len(), 2);
    let reader = result["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group["agent"] == "model-reader")
        .unwrap();
    assert_eq!(reader["totals"]["calls"], 2);
    assert_eq!(reader["totals"]["worker_runs"], 1);
    assert_eq!(reader["provider"], "provider-a");
    let unnamed = result["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group["agent"] == "unknown")
        .unwrap();
    assert_eq!(unnamed["totals"]["unknown_run_calls"], 1);
    assert_eq!(
        result["skills"],
        json!([
            {"name": "", "executions": 2},
            {"name": "image-processing", "executions": 2},
            {"name": "lark-doc", "executions": 1}
        ])
    );
    let agents = fixture.report("2026-09-02", "2026-09-02", "UTC", "all", "agents");
    assert_eq!(agents["skills"], json!([]));
    let runs: u64 = agents["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| group["totals"]["worker_runs"].as_u64().unwrap())
        .sum();
    assert_eq!(runs, 1);
}

#[test]
fn missing_shards_are_empty_and_unreadable_shards_return_errors() {
    let fixture = Fixture::new();
    let result = fixture.report("2026-09-15", "2026-09-15", "UTC", "mobile", "summary");
    assert_eq!(result["totals"]["calls"], 0);
    assert!(result["totals"]["tokens"].is_null());
    fs::create_dir(fixture.ledger.join("2026-09.jsonl")).unwrap();
    let query = Query::parse(args("2026-09-15", "2026-09-15", "UTC", "all", "summary")).unwrap();
    let error = read_report(&fixture.ledger, &fixture.home, query)
        .err()
        .unwrap();
    assert!(error.contains("2026-09.jsonl"));
}
