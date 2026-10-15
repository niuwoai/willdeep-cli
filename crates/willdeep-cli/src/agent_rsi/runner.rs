use super::*;
use std::{
    fs::OpenOptions,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub(super) struct RunLock {
    _file: std::fs::File,
}
impl RunLock {
    pub(super) fn acquire(path: &Path) -> Result<Self> {
        std::fs::create_dir_all(path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options.open(path.join("run.lock"))?;
        ensure!(
            file.metadata()?.is_file(),
            "RSI owner lock must be a regular file"
        );
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => {
                anyhow::anyhow!("another RSI operation owns this experiment or quota")
            }
            std::fs::TryLockError::Error(error) => error.into(),
        })?;
        // Keep the inode: the OS releases ownership after a crash, without stealing a live run.
        Ok(Self { _file: file })
    }
}

fn timestamp() -> String {
    willdeep_core::session::format_iso8601(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
}

pub(super) fn evidence(path: &Path) -> Result<Vec<Value>> {
    let trials = path.join("trials");
    let mut values = Vec::new();
    if !trials.exists() {
        return Ok(values);
    }
    for entry in std::fs::read_dir(trials)? {
        let p = entry?.path().join("evidence.json");
        if p.exists() {
            let value = contract::parse_json(&std::fs::read(p)?)?;
            contract::validate(&value)?;
            values.push(value);
        }
    }
    values.sort_by_key(|v| v["trial_id"].as_str().unwrap_or("").to_owned());
    Ok(values)
}
fn admissions(path: &Path) -> Result<Vec<Value>> {
    let trials = path.join("trials");
    let mut values = Vec::new();
    if trials.exists() {
        for e in std::fs::read_dir(trials)? {
            let p = e?.path().join("admission.json");
            if p.exists() {
                values.push(contract::parse_json(&std::fs::read(p)?)?);
            }
        }
    }
    Ok(values)
}
fn trial_base(spec: &Value, admission: &Value, task: &Value) -> Value {
    let mut value = contract::golden()["values"]["evidence"].clone();
    for key in [
        "experiment_id",
        "definition_id",
        "runtime",
        "model",
        "conditions",
        "budget",
    ] {
        value[key] = spec[key].clone();
    }
    for key in [
        "trial_id",
        "task_id",
        "arm",
        "repetition",
        "attempt",
        "resume_of",
    ] {
        value[key] = admission[key].clone();
    }
    value["task_sha256"] = task["task_sha256"].clone();
    value["split"] = task["split"].clone();
    value["definition_revision"] = if admission["arm"] == "baseline" {
        spec["baseline_revision"].clone()
    } else {
        spec["candidate_revision"].clone()
    };
    value["skills_sha256"] = spec["conditions"]["skills_sha256"].clone();
    value["dataset_sha256"] = spec["dataset"]["suite_sha256"].clone();
    value["fixture_sha256"] = spec["dataset"]["fixture_sha256"].clone();
    value["model_route"] = json!([]);
    value["target_executed"] = false.into();
    value["worker_run_id"] = Value::Null;
    value["root_run_id"] = format!("root-{}", admission["trial_id"].as_str().unwrap()).into();
    value["effective_prompt_sha256"] = digest(b"").into();
    value["started_at"] = timestamp().into();
    value["finished_at"] = Value::Null;
    value["events_sha256"] = Value::Null;
    value["status"] = "interrupted".into();
    value["trust"] = "local_mock".into();
    value["verifier"] = json!({"verifier_id":"rust-offline-read-and-prefix-v1","verifier_sha256":spec["dataset"]["verifier_sha256"],"input_sha256":digest(br#"{"schema":"willdeep.rust-offline-verifier-unexecuted.v1"}"#),"result":"unverified","exit_code":null});
    value["usage"] = json!({"input_tokens":null,"output_tokens":null,"auxiliary_tokens":null,"elapsed_ms":null,"cost_microusd":null});
    value
}

pub(super) async fn execute(home: &Path, id: &str, revision: &str, resume: bool) -> Result<Value> {
    let (path, spec, candidate, suite) = load(home, id)?;
    ensure!(
        spec["execution_mode"] == "direct",
        "team evaluation requires an independently validated adapter"
    );
    ensure!(
        spec["candidate_revision"] == revision,
        "--agent-revision must name the frozen candidate revision"
    );
    ensure!(
        spec["runtime"] == runtime()?,
        "runtime or binary changed; register a new experiment"
    );
    ensure!(
        spec["gate"]["gate_sha256"] == contract::hash(&gate())?,
        "gate changed after registration"
    );
    let _lock = RunLock::acquire(&path)?;
    if path.join("report.json").exists() && !resume {
        anyhow::bail!(
            "experiment already has a report; use --resume to read or continue without duplicate samples"
        );
    }
    let mut history = evidence(&path)?;
    for row in &history {
        contract::bind_evidence(&spec, row)?;
    }
    let mut admitted = admissions(&path)?;
    validate_history(&path, &spec, &candidate, &history, &admitted)?;
    // Explicit resume finalizes orphan starts as interrupted; never infer a pass.
    for admission in &admitted {
        let trial = admission["trial_id"].as_str().unwrap();
        if history.iter().any(|e| e["trial_id"] == trial) {
            continue;
        }
        ensure!(resume, "orphan admission requires explicit --resume");
        let task = spec["dataset"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["task_id"] == admission["task_id"])
            .context("orphan task missing from manifest")?;
        let mut value = trial_base(&spec, admission, task);
        value["finished_at"] = timestamp().into();
        contract::bind_evidence(&spec, &value)?;
        immutable(
            &path.join("trials").join(trial).join("evidence.json"),
            &value,
        )?;
        history.push(value);
    }
    validate_history(&path, &spec, &candidate, &history, &admitted)?;
    let mut stop: Option<String> = None;
    let mut ordered: Vec<_> = suite.tasks.iter().collect();
    ordered.sort_by_key(|task| match task.split.as_str() {
        "validation" => 0,
        "regression" => 1,
        _ => 2,
    });
    'tasks: for (task_index, task) in ordered.into_iter().enumerate() {
        if task.split == "holdout" && !stage_one_complete(&spec, &history) {
            stop = Some("stage_one_not_passed".into());
            break;
        }
        let manifest = spec["dataset"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["task_id"] == task.task_id)
            .context("fixture task absent from manifest")?;
        ensure!(
            manifest["task_sha256"] == contract::hash(&serde_json::to_value(task)?)?,
            "task fixture changed"
        );
        for repetition in 1..=spec["repetitions"].as_u64().unwrap() {
            let arms = if (task_index as u64 + repetition).is_multiple_of(2) {
                ["baseline", "candidate"]
            } else {
                ["candidate", "baseline"]
            };
            for arm in arms {
                let mut previous: Vec<_> = history
                    .iter()
                    .filter(|e| {
                        e["task_id"] == task.task_id
                            && e["arm"] == arm
                            && e["repetition"] == repetition
                    })
                    .collect();
                previous.sort_by_key(|v| v["attempt"].as_u64().unwrap());
                let last = previous.last().copied();
                if last.is_some_and(|v| {
                    !["interrupted", "infra_error"].contains(&v["status"].as_str().unwrap())
                }) {
                    continue;
                }
                let attempt = last
                    .map(|v| v["attempt"].as_u64().unwrap() + 1)
                    .unwrap_or(1);
                if attempt > spec["budget"]["max_attempts"].as_u64().unwrap() {
                    stop = Some("attempt_budget_exhausted".into());
                    break 'tasks;
                }
                let holdout_accesses = admitted
                    .iter()
                    .filter(|a| {
                        suite
                            .tasks
                            .iter()
                            .any(|t| t.split == "holdout" && a["task_id"] == t.task_id)
                    })
                    .count() as u64;
                if task.split == "holdout"
                    && holdout_accesses >= spec["budget"]["max_holdout_accesses"].as_u64().unwrap()
                {
                    stop = Some("holdout_budget_exhausted".into());
                    break 'tasks;
                }
                let trial_id = format!(
                    "trial-{}",
                    digest(
                        format!(
                            "{}:{arm}:{}:{repetition}:{attempt}",
                            contract::hash(&spec)?,
                            task.task_id
                        )
                        .as_bytes()
                    )
                );
                if task.split == "holdout"
                    && !reserve_holdout(
                        path.parent()
                            .and_then(Path::parent)
                            .context("invalid experiment namespace")?,
                        &spec,
                        &trial_id,
                    )?
                {
                    stop = Some("shared_holdout_budget_exhausted".into());
                    break 'tasks;
                }
                let admission = json!({"schema":"willdeep.agent-rsi.admission.v1","trial_id":trial_id,"task_id":task.task_id,"arm":arm,"repetition":repetition,"attempt":attempt,"resume_of":last.map(|v|v["trial_id"].clone()),"estimated_cost_microusd":0});
                contract::validate(&admission)?;
                let trial = path.join("trials").join(&trial_id);
                immutable(&trial.join("admission.json"), &admission)?;
                admitted.push(admission.clone());
                let workspace = trial.join("workspace");
                std::fs::create_dir_all(&workspace)?;
                private_write(&workspace.join("fixture.txt"), task.fixture.as_bytes())?;
                let package = package(
                    &candidate[if arm == "baseline" {
                        "baseline_package"
                    } else {
                        "candidate_package"
                    }],
                )?;
                let mut row = trial_base(&spec, &admission, manifest);
                immutable(&trial.join("started.json"), &row)?;
                let start = Instant::now();
                let result = willdeep_core::subagent::evaluation::run_offline(
                    &package,
                    &trial.join("home"),
                    &workspace,
                    &task.prompt,
                    &task.marker,
                    &task.expected_output,
                    &spec["budget"],
                )
                .await;
                row["finished_at"] = timestamp().into();
                row["usage"]["elapsed_ms"] = (start.elapsed().as_millis() as u64).into();
                match result {
                    Ok(observed) => {
                        let actual_skill_hashes: Vec<_> = package
                            .definition
                            .skills
                            .iter()
                            .map(|skill| skill.sha256.clone())
                            .collect();
                        ensure!(
                            observed.context.skills_sha256 == actual_skill_hashes,
                            "actual loaded skill list differs from target package"
                        );
                        let skills = contract::hash(&json!(package.definition.skills))?;
                        ensure!(
                            skills == spec["conditions"]["skills_sha256"],
                            "actual loaded skills differ from experiment"
                        );
                        ensure!(
                            contract::hash(&json!(observed.context.effective_tools))?
                                == spec["conditions"]["tools_sha256"],
                            "actual tools differ from experiment"
                        );
                        ensure!(
                            observed.context.provider.as_deref() == Some("mock")
                                && observed.context.model.as_deref()
                                    == Some("deterministic-reviewer-v1"),
                            "actual model route differs from experiment"
                        );
                        ensure!(
                            digest(&std::fs::read(workspace.join("fixture.txt"))?)
                                == digest(task.fixture.as_bytes()),
                            "fixture modified by worker"
                        );
                        row["target_executed"] = true.into();
                        row["worker_run_id"] = observed.context.worker_run_id.clone().into();
                        row["effective_prompt_sha256"] =
                            observed.context.effective_prompt_sha256.clone().into();
                        row["definition_revision"] =
                            observed.context.definition_revision.clone().into();
                        row["root_run_id"] = observed
                            .context
                            .root_run_id
                            .clone()
                            .context("actual root run identity missing")?
                            .into();
                        row["model_route"] = json!([{"provider":"mock","model":"deterministic-reviewer-v1","model_revision":null}]);
                        row["verifier"]["input_sha256"] = observed.verifier_input_sha256.into();
                        let events = json!({"worker_context":observed.context,"events":observed.events,"report_sha256":observed.report_sha256,"verifier_input":observed.verifier_input});
                        row["events_sha256"] = contract::hash(&events)?.into();
                        immutable(&trial.join("events.json"), &events)?;
                        row["verifier"]["result"] = if observed.verified {
                            "passed"
                        } else {
                            "failed"
                        }
                        .into();
                        row["verifier"]["exit_code"] = if observed.verified { 0 } else { 1 }.into();
                        row["status"] = if observed.verified {
                            "passed"
                        } else {
                            "failed"
                        }
                        .into();
                        row["usage"]["input_tokens"] = observed.input_tokens.into();
                        row["usage"]["output_tokens"] = observed.output_tokens.into();
                        row["usage"]["auxiliary_tokens"] = 0.into();
                        row["usage"]["cost_microusd"] = 0.into();
                    }
                    Err(error) => {
                        row["status"] = match error.downcast_ref::<willdeep_core::AgentError>() {
                            Some(willdeep_core::AgentError::SubagentPartial {
                                reason:
                                    willdeep_core::AgentStopReason::MaxTurns
                                    | willdeep_core::AgentStopReason::BudgetLimited,
                                ..
                            })
                            | Some(willdeep_core::AgentError::TokenBudgetExceeded { .. })
                            | Some(willdeep_core::AgentError::MaxTurns(_)) => "budget_exhausted",
                            Some(willdeep_core::AgentError::SubagentPartial {
                                reason: willdeep_core::AgentStopReason::Unverified,
                                ..
                            }) => "unverified",
                            _ => "infra_error",
                        }
                        .into();
                    }
                }
                contract::bind_evidence(&spec, &row)?;
                immutable(&trial.join("evidence.json"), &row)?;
                history.push(row);
            }
        }
    }
    history.sort_by_key(|row| row["trial_id"].as_str().unwrap_or("").to_owned());
    let report = summary(&spec, &history, stop)?;
    private_write(
        &path.join("report.json"),
        &serde_json::to_vec_pretty(&report)?,
    )?;
    let markdown = format!(
        "# RSI 离线验收\n\n实验：{}\n\n结论：{}\n\n独立配对样本：{}；全部尝试：{}；缺失结果：{}。\n\n本次使用确定性 Mock，通过真实 Worker 与文件工具执行；费用为 0，不代表模型质量提升，不允许官方晋升。\n\n报告包含不可变 revision、实际提示词摘要、模型路由、资源、数据与验证器血缘。\n",
        id,
        report["verdict"].as_str().unwrap(),
        report["independent_samples"],
        history.len(),
        report["missing_results"]
    );
    private_write(&path.join("report.md"), markdown.as_bytes())?;
    Ok(report)
}
fn summary(spec: &Value, history: &[Value], stop: Option<String>) -> Result<Value> {
    let expected = spec["dataset"]["tasks"].as_array().unwrap().len() as u64
        * spec["repetitions"].as_u64().unwrap()
        * 2;
    let final_rows = latest_rows(history);
    let missing = expected.saturating_sub(final_rows.len() as u64);
    let all_passed = missing == 0 && final_rows.values().all(|e| e["status"] == "passed");
    let verdict = if all_passed {
        "offline_pipeline_passed"
    } else {
        "inconclusive"
    };
    Ok(
        json!({"schema":"willdeep.agent-rsi.offline-report.v1","experiment_id":spec["experiment_id"],"spec_sha256":contract::hash(spec)?,"verdict":verdict,"quality_improvement_claimed":false,"official_promotion_allowed":false,"paid_execution_allowed":false,"independent_samples":final_rows.len()/2,"attempts":history.len(),"missing_results":missing,"stop_reason":stop,"evidence":history,"cost_microusd":0,"runtime":spec["runtime"]}),
    )
}
fn latest_rows(history: &[Value]) -> std::collections::BTreeMap<String, &Value> {
    let mut rows = std::collections::BTreeMap::<String, &Value>::new();
    for row in history {
        let key = format!("{}:{}:{}", row["task_id"], row["arm"], row["repetition"]);
        if rows
            .get(&key)
            .is_none_or(|p| p["attempt"].as_u64() < row["attempt"].as_u64())
        {
            rows.insert(key, row);
        }
    }
    rows
}
fn stage_one_complete(spec: &Value, history: &[Value]) -> bool {
    let latest = latest_rows(history);
    spec["dataset"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|task| task["split"] != "holdout")
        .all(|task| {
            (1..=spec["repetitions"].as_u64().unwrap()).all(|repetition| {
                ["baseline", "candidate"].into_iter().all(|arm| {
                    latest.values().any(|row| {
                        row["task_id"] == task["task_id"]
                            && row["arm"] == arm
                            && row["repetition"] == repetition
                            && row["status"] == "passed"
                    })
                })
            })
        })
}

/// Account + grant + frozen suite; changing experiment ID never refunds holdout access.
fn reserve_holdout(namespace: &Path, spec: &Value, trial_id: &str) -> Result<bool> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ledger {
        limit: u64,
        trials: Vec<String>,
    }
    let scope = json!({"grant_id":spec["grant_id"],"suite_sha256":spec["dataset"]["suite_sha256"]});
    let path = namespace.join("holdout").join(contract::hash(&scope)?);
    let _lock = RunLock::acquire(&path)?;
    let file = path.join("accesses.json");
    let requested_limit = spec["budget"]["max_holdout_accesses"].as_u64().unwrap();
    let mut ledger: Ledger = if file.exists() {
        serde_json::from_value(contract::parse_json(&std::fs::read(&file)?)?)?
    } else {
        Ledger {
            limit: requested_limit,
            trials: Vec::new(),
        }
    };
    ensure!(
        (1..=128).contains(&ledger.limit) && ledger.trials.len() <= 128,
        "invalid shared holdout ledger"
    );
    ledger.limit = ledger.limit.min(requested_limit);
    if ledger.trials.iter().any(|value| value == trial_id) {
        private_write(&file, &serde_json::to_vec(&ledger)?)?;
        return Ok(true);
    }
    if ledger.trials.len() as u64 >= ledger.limit {
        private_write(&file, &serde_json::to_vec(&ledger)?)?;
        return Ok(false);
    }
    ledger.trials.push(trial_id.into());
    private_write(&file, &serde_json::to_vec(&ledger)?)?;
    Ok(true)
}

fn validate_history(
    path: &Path,
    spec: &Value,
    candidate: &Value,
    history: &[Value],
    admitted: &[Value],
) -> Result<()> {
    let mut identities = std::collections::BTreeSet::new();
    for admission in admitted {
        contract::validate(admission)?;
        ensure!(
            identities.insert(admission["trial_id"].as_str().unwrap()),
            "duplicate admitted trial"
        );
    }
    for row in history {
        contract::bind_evidence(spec, row)?;
        if !row["events_sha256"].is_null() {
            let events = contract::parse_json(&std::fs::read(
                path.join("trials")
                    .join(row["trial_id"].as_str().unwrap())
                    .join("events.json"),
            )?)?;
            ensure!(
                row["events_sha256"] == contract::hash(&events)?,
                "persisted execution events were changed"
            );
            ensure!(
                row["verifier"]["input_sha256"] == contract::hash(&events["verifier_input"])?
                    && events["verifier_input"]["report_sha256"] == events["report_sha256"],
                "actual verifier input changed"
            );
            let context = &events["worker_context"];
            for key in [
                "definition_id",
                "definition_revision",
                "effective_prompt_sha256",
                "root_run_id",
                "worker_run_id",
            ] {
                ensure!(row[key] == context[key], "actual worker lineage changed");
            }
            let package = &candidate[if row["arm"] == "baseline" {
                "baseline_package"
            } else {
                "candidate_package"
            }];
            let skills: Vec<Value> = package["definition"]["skills"]
                .as_array()
                .unwrap()
                .iter()
                .map(|skill| skill["sha256"].clone())
                .collect();
            ensure!(
                json!(skills) == context["skills_sha256"],
                "persisted loaded skill list differs from package"
            );
            ensure!(
                contract::hash(&package["definition"]["skills"])? == row["skills_sha256"]
                    && contract::hash(&context["effective_tools"])?
                        == spec["conditions"]["tools_sha256"],
                "actual worker dependencies changed"
            );
        }
        let admission = admitted
            .iter()
            .find(|a| a["trial_id"] == row["trial_id"])
            .context("evidence has no admission")?;
        for key in ["task_id", "arm", "repetition", "attempt", "resume_of"] {
            ensure!(row[key] == admission[key], "admission lineage mismatch");
        }
        let attempt = row["attempt"].as_u64().unwrap();
        if attempt == 1 {
            ensure!(row["resume_of"].is_null(), "first attempt cannot resume");
        } else {
            let parent = history
                .iter()
                .find(|p| p["trial_id"] == row["resume_of"])
                .context("resume parent missing")?;
            for key in ["task_id", "arm", "repetition"] {
                ensure!(parent[key] == row[key], "cross-sample resume denied");
            }
            ensure!(
                parent["attempt"].as_u64() == Some(attempt - 1)
                    && ["interrupted", "infra_error"].contains(&parent["status"].as_str().unwrap()),
                "invalid retry lineage"
            );
        }
        ensure!(
            history
                .iter()
                .filter(|p| p["task_id"] == row["task_id"]
                    && p["arm"] == row["arm"]
                    && p["repetition"] == row["repetition"]
                    && p["attempt"] == row["attempt"])
                .count()
                == 1,
            "duplicate scoring attempt"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn durable_lock_rejects_live_owner_and_can_reopen_a_stale_inode() {
        let path = std::env::temp_dir().join(format!("rsi-owner-{}", uuid::Uuid::new_v4()));
        let guard = RunLock::acquire(&path).unwrap();
        assert!(RunLock::acquire(&path).is_err());
        drop(guard);
        assert!(path.join("run.lock").exists());
        let next = RunLock::acquire(&path).unwrap();
        drop(next);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn partial_stage_one_cannot_open_holdout() {
        let spec = contract::golden()["values"]["experiment"].clone();
        assert!(!stage_one_complete(&spec, &[]));
        let baseline = contract::golden()["values"]["evidence"].clone();
        let mut candidate = baseline.clone();
        candidate["arm"] = "candidate".into();
        assert!(!stage_one_complete(
            &spec,
            &[baseline.clone(), candidate.clone()]
        ));
        let mut regression_baseline = baseline.clone();
        regression_baseline["task_id"] = "regression-1".into();
        regression_baseline["split"] = "regression".into();
        let mut regression_candidate = regression_baseline.clone();
        regression_candidate["arm"] = "candidate".into();
        assert!(stage_one_complete(
            &spec,
            &[
                baseline,
                candidate,
                regression_baseline,
                regression_candidate
            ]
        ));
    }
    #[test]
    fn a_frozen_namespace_keeps_quota_after_account_switch_and_limit_cannot_increase() {
        let home = std::env::temp_dir().join(format!("rsi-quota-owner-{}", uuid::Uuid::new_v4()));
        let session = crate::agent_registry::service::CustomerSession {
            origin: "https://some.im".into(),
            token: "local-test-only".into(),
            user_id: "user-a".into(),
            tenant_id: "tenant".into(),
        };
        session.save(&home).unwrap();
        let namespace = storage_base(&home).unwrap();
        let mut spec = contract::golden()["values"]["experiment"].clone();
        spec["budget"]["max_holdout_accesses"] = 1.into();
        assert!(reserve_holdout(&namespace, &spec, "trial-a").unwrap());
        crate::agent_registry::service::CustomerSession {
            user_id: "user-b".into(),
            ..session
        }
        .save(&home)
        .unwrap();
        spec["budget"]["max_holdout_accesses"] = 128.into();
        assert!(!reserve_holdout(&namespace, &spec, "trial-b").unwrap());
        assert!(!storage_base(&home).unwrap().join("holdout").exists());
        std::fs::remove_dir_all(home).unwrap();
    }
}
