//! Sealed, free-only engineering acceptance. No provider/configuration/credentials are accepted.
use crate::{
    Message, Role,
    agent_definition::{AgentPackage, digest, store::private_write},
    provider::{Provider, ProviderError, ProviderIdentity},
    types::{Completion, ToolCall, ToolDefinition, Usage},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

const BASELINE_SHA: &str = "51b79a923ceceeae80a7cc6e20f2f0dbdfdb136f1bc516b895450ea633273bdd";
const TASK: &str = "Read fixture.txt through the file tool. Review only its stated requirement and documented input shape. Do not modify files. The host will execute a frozen fixture and reference independently; submit data only and do not claim host verification.";
const SYNTHETIC_TOKENS: u64 = 50;

fn baseline_package() -> Result<AgentPackage> {
    let golden: Value = serde_json::from_slice(include_bytes!(
        "../../../../docs/examples/agent-rsi-golden.json"
    ))?;
    let package: AgentPackage =
        serde_json::from_value(golden["values"]["candidate"]["baseline_package"].clone())?;
    ensure!(
        package.sha256 == BASELINE_SHA,
        "frozen free reviewer baseline changed"
    );
    Ok(package)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fixture {
    Bug,
    Clean,
}
impl Fixture {
    fn sha256(self) -> &'static str {
        match self {
            Self::Bug => "b1389ab2f18feeedec2e1c920519a1753ae8541fb1ea172ae831497d64d0815a",
            Self::Clean => "da7956a504b831a1f084148c49638c3a532be79e4e9d8756eeab03d10e1dae4b",
        }
    }
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Bug => include_bytes!(
                "../../../../bench/agent-rsi/reproduction-free-v1/fixtures/count-zero-bug.js"
            ),
            Self::Clean => include_bytes!(
                "../../../../bench/agent-rsi/reproduction-free-v1/fixtures/count-zero-clean.js"
            ),
        }
    }
    fn case_id(self) -> &'static str {
        match self {
            Self::Bug => "count-zero-bug",
            Self::Clean => "count-zero-clean",
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Options {
    pub fixture: Fixture,
    pub max_requests: usize,
    pub max_tokens: u64,
    pub read_rounds: usize,
    pub timeout_ms: u64,
    pub cancel_after_ms: Option<u64>,
    /// Deterministic response latency for exercising real timeout/cancellation.
    pub mock_delay_ms: u64,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            fixture: Fixture::Bug,
            max_requests: 2,
            max_tokens: 1_000,
            read_rounds: 1,
            timeout_ms: 5_000,
            cancel_after_ms: None,
            mock_delay_ms: 0,
        }
    }
}
#[derive(Default)]
struct MockState {
    receipts: Vec<Value>,
    prompt: Option<String>,
    raw_report: Option<String>,
    denied: Option<&'static str>,
}
struct ReviewMock {
    max_requests: usize,
    max_tokens: u64,
    read_rounds: usize,
    delay_ms: u64,
    state: Mutex<MockState>,
}
#[async_trait::async_trait]
impl Provider for ReviewMock {
    fn ledger_identity(&self) -> Option<ProviderIdentity> {
        Some(ProviderIdentity {
            provider: "mock".into(),
            model: "deterministic-review-output-v2".into(),
            local: true,
        })
    }
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> std::result::Result<Completion, ProviderError> {
        let tool = messages.iter().rev().find(|message| {
            message.role == Role::Tool && message.content.contains("function reviewed(input)")
        });
        let delivered = tool.is_some();
        let index = {
            let mut state = self.state.lock().unwrap();
            let index = state.receipts.len();
            let denied = if index >= self.max_requests {
                Some("request_limit")
            } else if (index as u64 + 1) * SYNTHETIC_TOKENS > self.max_tokens {
                Some("token_admission")
            } else {
                None
            };
            if let Some(reason) = denied {
                state.denied = Some(reason);
                return Err(ProviderError::Client(format!("offline_mock_{reason}")));
            }
            let prompt = messages
                .first()
                .map(|message| message.content.clone())
                .unwrap_or_default();
            state.prompt.get_or_insert_with(|| prompt.clone());
            state.receipts.push(
                json!({"request_index":index + 1,"mode":"offline_mock_only","status":"in_flight",
                "usage":null,"usage_kind":"synthetic","delivered_to_model":delivered,"system_prompt_sha256":digest(prompt.as_bytes()),"system_prompt":prompt,
                "tool_message_sha256":tool.map(|message|digest(message.content.as_bytes()))}),
            );
            index
        };
        if self.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        }
        let usage = Usage {
            input_tokens: Some(40),
            output_tokens: Some(10),
            total_tokens: Some(SYNTHETIC_TOKENS),
            cache_read_tokens: Some(0),
        };
        let completion = if index < self.read_rounds {
            Completion {
                tool_calls: vec![ToolCall {
                    id: format!("free-read-{index}"),
                    name: "read_file".into(),
                    arguments: r#"{"path":"fixture.txt"}"#.into(),
                }],
                usage: Some(usage.clone()),
                ..Completion::default()
            }
        } else {
            let source = tool.ok_or_else(|| {
                ProviderError::Client("offline mock did not receive a real file read".into())
            })?;
            let report = if source.content.contains("input.count || input.fallback") {
                json!({"has_defect":true,"line":3,"reason":"Zero must remain valid.","reproduction":{"input":{"count":0,"fallback":7},"expected":0,"actual":7}})
            } else if source.content.contains("input.count ?? input.fallback") {
                json!({"has_defect":false,"line":null,"reason":"The fixture meets the stated requirement.","reproduction":null})
            } else {
                return Err(ProviderError::Client(
                    "unrecognized public fixture source".into(),
                ));
            };
            let content = serde_json::to_string(&report).unwrap();
            self.state.lock().unwrap().raw_report = Some(content.clone());
            Completion {
                content,
                finish_reason: Some("stop".into()),
                usage: Some(usage.clone()),
                ..Completion::default()
            }
        };
        let mut state = self.state.lock().unwrap();
        state.receipts[index]["usage"] = serde_json::to_value(&usage).unwrap();
        state.receipts[index]["status"] = json!("completed");
        Ok(completion)
    }
}

/// Creates a fresh experiment directory and persists both raw output and observable Worker evidence.
pub async fn run(directory: &Path, options: Options) -> Result<Value> {
    ensure!(
        [2, 4].contains(&options.max_requests),
        "free review supports request limits 2 or 4"
    );
    ensure!(
        (1..=3).contains(&options.read_rounds),
        "read_rounds must be 1 through 3"
    );
    ensure!(
        (1..=1_000_000).contains(&options.max_tokens),
        "invalid free review token limit"
    );
    ensure!(
        (1..=60_000).contains(&options.timeout_ms) && options.mock_delay_ms <= 60_000,
        "invalid free review timeout/delay"
    );
    ensure!(
        options
            .cancel_after_ms
            .is_none_or(|value| (1..=60_000).contains(&value)),
        "invalid cancellation interval"
    );
    ensure!(
        !directory.exists(),
        "free review output directory already exists"
    );
    std::fs::create_dir(directory)?;
    let home = directory.join("home");
    let workspace = directory.join("workspace");
    std::fs::create_dir(&home)?;
    std::fs::create_dir(&workspace)?;
    private_write(&workspace.join("fixture.txt"), options.fixture.bytes())?;
    let fixture_sha256 = digest(options.fixture.bytes());
    ensure!(
        fixture_sha256 == options.fixture.sha256(),
        "frozen public fixture changed"
    );
    let package = baseline_package()?;
    let provider = Arc::new(ReviewMock {
        max_requests: options.max_requests,
        max_tokens: options.max_tokens,
        read_rounds: options.read_rounds,
        delay_ms: options.mock_delay_ms,
        state: Mutex::default(),
    });
    let stop = match options.cancel_after_ms {
        Some(ms) if ms < options.timeout_ms => {
            super::evaluation::IsolatedStop::CancelAfter(Duration::from_millis(ms))
        }
        _ => super::evaluation::IsolatedStop::Timeout(Duration::from_millis(options.timeout_ms)),
    };
    let observed = super::evaluation::run_isolated_configured(
        &package,
        &home,
        &workspace,
        TASK,
        &json!({"max_turns":5,"max_tokens":options.max_tokens,"timeout_seconds":60}),
        provider.clone(),
        true,
        Some(stop),
    )
    .await?;
    let mut state = provider.state.lock().unwrap();
    for receipt in &mut state.receipts {
        if receipt["status"] == "in_flight" {
            receipt["status"] = json!("interrupted_usage_unknown");
        }
    }
    let read_observed = observed.events.iter().any(|event| {
        event["kind"] == "tool_completed"
            && event["name"] == "read_file"
            && event["is_error"] == false
    });
    let delivered = state
        .receipts
        .iter()
        .any(|receipt| receipt["delivered_to_model"] == true);
    let unchanged = digest(&std::fs::read(workspace.join("fixture.txt"))?) == fixture_sha256;
    let evidence = json!({"fixture_sha256":fixture_sha256,"read_observed":read_observed,"delivered_to_model":delivered,"fixture_unchanged":unchanged});
    let prompt = state.prompt.as_deref();
    if let Some(prompt) = prompt {
        ensure!(
            prompt.contains(crate::rsi::review_output::instructions())
                && prompt.contains(&crate::rsi::review_output::instructions_sha256()),
            "actual Worker missed shared review instructions"
        );
        // Agent adds runtime budget/rule context after the runner's compiled prelude.
        // Bind both independently rather than mislabeling the prelude as the wire prompt.
        let marker = format!(
            "Output instructions SHA-256: {}",
            crate::rsi::review_output::instructions_sha256()
        );
        let end = prompt
            .find(&marker)
            .ok_or_else(|| anyhow::anyhow!("missing shared instruction marker"))?
            + marker.len();
        ensure!(
            observed.context.effective_prompt_sha256 == digest(prompt[..end].as_bytes()),
            "actual request does not contain the locked Worker prelude"
        );
    } else {
        ensure!(
            observed.result.is_err(),
            "completed Worker has no Mock receipt"
        );
    }
    let (completed, error) = match &observed.result {
        Ok(_) => (true, None),
        Err(error) => (false, Some(error.to_string())),
    };
    let request_context = prompt.map(|prompt| {
        let mut context = observed.context.clone();
        context.effective_prompt_sha256 = digest(prompt.as_bytes());
        json!({"derived":true,"source":"first_admitted_mock_request","context":context})
    });
    let stop_reason = if completed {
        "completed"
    } else if let Some(denied) = state.denied {
        denied
    } else if error
        .as_deref()
        .is_some_and(|error| error.contains("offline_mock_timeout"))
    {
        "timeout"
    } else if error
        .as_deref()
        .is_some_and(|error| error.contains("offline_mock_cancelled"))
    {
        "cancelled"
    } else if matches!(
        observed.result,
        Err(crate::AgentError::TokenBudgetExceeded { .. })
    ) {
        "token_exhausted"
    } else {
        "worker_stopped"
    };
    let format_valid = state
        .raw_report
        .as_ref()
        .is_some_and(|raw| crate::rsi::review_output::validate(raw.as_bytes()).is_ok());
    let mut lifecycle = Vec::new();
    for entry in std::fs::read_dir(home.join("agent-evidence"))? {
        let path = entry?.path();
        let bytes = std::fs::read(&path)?;
        ensure!(
            path.file_stem().and_then(|name| name.to_str()) == Some(digest(&bytes).as_str()),
            "Worker lifecycle digest mismatch"
        );
        lifecycle.push(crate::rsi::contract::parse_json(&bytes)?);
    }
    if completed {
        ensure!(format_valid, "mock returned an invalid review");
        ensure!(
            observed.result.as_ref().unwrap() == state.raw_report.as_ref().unwrap(),
            "Worker changed the four-field JSON report"
        );
        ensure!(
            read_observed && delivered && unchanged,
            "completed mock lacks genuine tool evidence"
        );
    }
    let summary = json!({"schema":"willdeep.agent-rsi.free-review-observation.v2","execution_mode":"offline_mock_only",
        "verification":"unverified","model_calls_authorized":0,"network_model_calls":0,"case_id":options.fixture.case_id(),
        "limits":options,
        "completed":completed,"stop_reason":stop_reason,"error":error,"format_valid":format_valid,
        "definition_revision":package.sha256,"instructions_sha256":crate::rsi::review_output::instructions_sha256(),
        "contract_sha256":digest(crate::rsi::review_output::CONTRACT),"request_context_projection":request_context,
        "compiled_worker_prompt_sha256":observed.context.effective_prompt_sha256,"worker_context":observed.context,
        "mock_requests_admitted":state.receipts.len(),"mock_receipts":state.receipts,"events":observed.events,
        "worker_lifecycle":lifecycle,
        "evidence":evidence,"raw_report":observed.result.as_ref().ok(),"raw_model_content":state.raw_report,"effective_prompt_sha256":prompt.map(|text|digest(text.as_bytes())),
        "objective_score":null,"quality_claim":false});
    if let Some(prompt) = prompt {
        private_write(&directory.join("effective-prompt.txt"), prompt.as_bytes())?;
    }
    if let Some(raw) = &state.raw_report {
        private_write(&directory.join("raw-model-content.json"), raw.as_bytes())?;
        if completed {
            private_write(&directory.join("raw-report.json"), raw.as_bytes())?;
        }
    }
    private_write(
        &directory.join("evidence.json"),
        &serde_json::to_vec_pretty(&evidence)?,
    )?;
    private_write(
        &directory.join("observation.json"),
        &serde_json::to_vec_pretty(&summary)?,
    )?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestDirectory(std::path::PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("willdeep-free-review-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("clean isolated free-review fixture");
        }
    }
    async fn trial(options: Options) -> Value {
        let temp = TestDirectory::new();
        run(&temp.path().join("trial"), options).await.unwrap()
    }
    #[tokio::test]
    async fn legacy_isolated_worker_keeps_its_original_report_contract_and_facts() {
        let temp = TestDirectory::new();
        let home = temp.path().join("home");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(&workspace).unwrap();
        private_write(&workspace.join("fixture.txt"), Fixture::Bug.bytes()).unwrap();
        let provider = Arc::new(ReviewMock {
            max_requests: 2,
            max_tokens: 1000,
            read_rounds: 1,
            delay_ms: 0,
            state: Mutex::default(),
        });
        let observation = super::super::evaluation::run_isolated(
            &baseline_package().unwrap(),
            &home,
            &workspace,
            TASK,
            &json!({"max_turns":5,"max_tokens":1000,"timeout_seconds":60}),
            provider.clone(),
        )
        .await
        .unwrap();
        let report = observation.result.unwrap();
        assert!(report.contains("<worker-facts "));
        assert!(crate::rsi::review_output::validate(report.as_bytes()).is_err());
        let state = provider.state.lock().unwrap();
        let prompt = state.prompt.as_ref().unwrap();
        assert!(prompt.contains("CONCLUSION") && prompt.contains("OPEN QUESTIONS"));
        assert!(!prompt.contains(crate::rsi::review_output::instructions()));
    }
    #[tokio::test]
    async fn bug_and_clean_use_real_worker_reads_shared_prompt_and_exact_output() {
        for fixture in [Fixture::Bug, Fixture::Clean] {
            let result = trial(Options {
                fixture,
                ..Options::default()
            })
            .await;
            assert_eq!(result["completed"], true, "{result}");
            assert_eq!(result["mock_requests_admitted"], 2);
            for receipt in result["mock_receipts"].as_array().unwrap() {
                let actual_prompt = receipt["system_prompt"].as_str().unwrap();
                assert_eq!(
                    receipt["system_prompt_sha256"],
                    digest(actual_prompt.as_bytes())
                );
                assert!(actual_prompt.contains(crate::rsi::review_output::instructions()));
            }
            assert_eq!(result["definition_revision"], BASELINE_SHA);
            assert_eq!(result["evidence"]["read_observed"], true);
            assert_eq!(result["evidence"]["delivered_to_model"], true);
            assert_eq!(result["verification"], "unverified");
            assert_eq!(result["worker_context"]["provider"], "mock");
            assert_eq!(result["request_context_projection"]["derived"], true);
            assert_eq!(
                result["request_context_projection"]["context"]["effective_prompt_sha256"],
                result["effective_prompt_sha256"]
            );
            crate::rsi::review_output::validate(result["raw_report"].as_str().unwrap().as_bytes())
                .unwrap();
        }
    }
    #[tokio::test]
    async fn request_limit_two_stops_but_four_finishes_three_read_rounds() {
        for (max_requests, completed, admitted) in [(2, false, 2), (4, true, 4)] {
            let result = trial(Options {
                max_requests,
                read_rounds: 3,
                ..Options::default()
            })
            .await;
            assert_eq!(result["completed"], completed, "{result}");
            assert_eq!(result["mock_requests_admitted"], admitted);
            if !completed {
                assert_eq!(result["stop_reason"], "request_limit");
            }
            let receipts = result["mock_receipts"].as_array().unwrap();
            let hashes = receipts
                .iter()
                .map(|receipt| {
                    let prompt = receipt["system_prompt"].as_str().unwrap();
                    let hash = digest(prompt.as_bytes());
                    assert_eq!(receipt["system_prompt_sha256"], hash);
                    hash
                })
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(
                hashes.len(),
                admitted as usize,
                "per-round budget context must remain independently reproducible"
            );
        }
    }
    #[tokio::test]
    async fn token_admission_and_consumption_do_not_invent_requests_or_usage() {
        let denied = trial(Options {
            max_tokens: 49,
            ..Options::default()
        })
        .await;
        assert_eq!(denied["stop_reason"], "token_admission");
        assert_eq!(denied["mock_requests_admitted"], 0);
        assert_eq!(denied["mock_receipts"], json!([]));
        assert!(denied["effective_prompt_sha256"].is_null());
        assert!(denied["request_context_projection"].is_null());
        let consumed = trial(Options {
            max_tokens: 50,
            ..Options::default()
        })
        .await;
        assert_eq!(consumed["stop_reason"], "token_exhausted", "{consumed}");
        assert_eq!(consumed["mock_requests_admitted"], 1);
        assert_eq!(consumed["evidence"]["read_observed"], false);
        assert_eq!(consumed["mock_receipts"][0]["usage"]["total_tokens"], 50);
    }
    #[tokio::test]
    async fn timeout_and_cancel_drop_real_worker_leaving_inflight_usage_unknown() {
        for (cancel_after_ms, expected) in [(None, "timeout"), (Some(250), "cancelled")] {
            let result = trial(Options {
                timeout_ms: 500,
                cancel_after_ms,
                mock_delay_ms: 5_000,
                ..Options::default()
            })
            .await;
            assert_eq!(result["stop_reason"], expected, "{result}");
            assert_eq!(result["completed"], false);
            assert_eq!(result["mock_requests_admitted"], 1);
            assert_eq!(result["mock_receipts"][0]["usage"], Value::Null);
            assert_eq!(result["raw_report"], Value::Null);
            assert_eq!(result["evidence"]["read_observed"], false);
            assert!(
                result["worker_lifecycle"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|event| event["outcome"] == "interrupted"),
                "{result}"
            );
        }
    }
}
