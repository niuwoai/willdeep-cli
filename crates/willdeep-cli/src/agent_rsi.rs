//! Revision-targeted experiments. The native pilot only talks to a local admission broker.
mod free_review;
mod native;
mod native_provider;
mod runner;
mod service;
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use willdeep_core::{
    agent_definition::store::private_write,
    agent_definition::{AgentPackage, digest},
    rsi::contract,
};

#[derive(Clone, Debug, Subcommand)]
pub enum RsiAction {
    /// Free-only Worker/file-tool acceptance; never constructs a network provider.
    FreeReview(free_review::Arguments),
    /// One separately admitted native trial through the local budget broker; never uses user keys.
    NativeTrial {
        #[arg(long)]
        control: PathBuf,
        /// Capture the real first Worker request and stop before network transmission.
        #[arg(long)]
        preview: bool,
    },
    /// Verify strict shared JSON, hashes and immutable candidate constraints.
    Check {
        file: PathBuf,
    },
    /// Save a candidate without installing or selecting it.
    Candidate {
        #[arg(long)]
        baseline: PathBuf,
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long)]
        id: String,
    },
    /// Freeze a local paired experiment. Only the deterministic mock is admitted.
    Prepare {
        #[arg(long)]
        baseline: PathBuf,
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        suite: Option<PathBuf>,
    },
    /// Run both arms using the production Worker and tools, with no network calls.
    Run {
        id: String,
        #[arg(long)]
        agent_revision: String,
        #[arg(long)]
        resume: bool,
    },
    List,
    Report {
        id: String,
    },
    /// Explicitly upload summary-only records through a separately authorized grant.
    Upload {
        id: String,
        #[arg(long)]
        grant_id: String,
    },
}

pub(crate) async fn run_free_review_standalone() -> Option<Result<()>> {
    free_review::run_standalone(std::env::args_os().collect()).await
}

pub async fn run(action: RsiAction, home: &Path) -> Result<()> {
    let value = match action {
        RsiAction::FreeReview(arguments) => free_review::execute(arguments).await?,
        RsiAction::NativeTrial { control, preview } => native::execute(&control, preview).await?,
        RsiAction::Check { file } => {
            let v = contract::parse_json(&std::fs::read(file)?)?;
            contract::validate(&v)?;
            json!({"valid":true,"sha256":contract::hash(&v)?})
        }
        RsiAction::Candidate {
            baseline,
            candidate,
            id,
        } => candidate_save(
            home,
            &id,
            read_package(&baseline)?,
            read_package(&candidate)?,
        )?,
        RsiAction::Prepare {
            baseline,
            candidate,
            id,
            suite,
        } => prepare(
            home,
            &id,
            read_package(&baseline)?,
            read_package(&candidate)?,
            suite
                .as_deref()
                .map(read_suite)
                .transpose()?
                .unwrap_or_else(Suite::builtin),
        )?,
        RsiAction::Run {
            id,
            agent_revision,
            resume,
        } => runner::execute(home, &id, &agent_revision, resume).await?,
        RsiAction::List => list(home)?,
        RsiAction::Report { id } => report(home, &id)?,
        RsiAction::Upload { id, grant_id } => service::upload(home, &id, &grant_id).await?,
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
pub async fn operate(home: &Path, operation: &str, params: Value) -> Result<Value> {
    let field = |key: &str| {
        params[key]
            .as_str()
            .map(str::to_owned)
            .with_context(|| format!("RSI {key} required"))
    };
    match operation {
        "list" => list(home),
        "report" => report(home, &field("id")?),
        "candidate" => candidate_save(
            home,
            &field("id")?,
            package(&params["baseline_package"])?,
            candidate_param(&params)?,
        ),
        "prepare" => prepare(
            home,
            &field("id")?,
            package(&params["baseline_package"])?,
            candidate_param(&params)?,
            Suite::builtin(),
        ),
        "run" => {
            runner::execute(
                home,
                &field("id")?,
                &field("agent_revision")?,
                params["resume"].as_bool().unwrap_or(false),
            )
            .await
        }
        "upload" => service::upload(home, &field("id")?, &field("grant_id")?).await,
        _ => anyhow::bail!("unknown RSI operation"),
    }
}
fn read_package(path: &Path) -> Result<AgentPackage> {
    package(&contract::parse_json(&std::fs::read(path)?)?)
}
fn package(value: &Value) -> Result<AgentPackage> {
    AgentPackage::parse(&serde_json::to_vec(value)?)
}
fn candidate_param(params: &Value) -> Result<AgentPackage> {
    if let Some(value) = params.get("candidate_package") {
        package(value)
    } else {
        AgentPackage::local(willdeep_core::agent_definition::AgentDefinition::parse(
            &serde_json::to_vec(&params["candidate_definition"])?,
        )?)
    }
}
pub(super) fn root(home: &Path, id: &str) -> Result<PathBuf> {
    ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:/-".contains(&b)),
        "invalid experiment ID"
    );
    Ok(storage_base(home)?
        .join("experiments")
        .join(digest(id.as_bytes())))
}
fn storage_base(home: &Path) -> Result<PathBuf> {
    let base = home.join("agent-rsi");
    if !home.join("agent-session.json").exists() {
        return Ok(base.join("local"));
    }
    let session = crate::agent_registry::service::CustomerSession::load(home)?;
    crate::agent_registry::service::Service::new(&session.origin)?;
    Ok(account_storage(home, &session))
}
fn account_storage(
    home: &Path,
    session: &crate::agent_registry::service::CustomerSession,
) -> PathBuf {
    home.join("agent-rsi/accounts").join(digest(
        format!(
            "{}\n{}\n{}",
            session.origin, session.user_id, session.tenant_id
        )
        .as_bytes(),
    ))
}
fn immutable(path: &Path, value: &Value) -> Result<()> {
    let bytes = contract::canonical(value)?.into_bytes();
    if path.exists() {
        ensure!(
            std::fs::read(path)? == bytes,
            "immutable RSI record conflict"
        );
        return Ok(());
    }
    let temporary = path.with_extension(format!("immutable-{}", uuid::Uuid::new_v4()));
    private_write(&temporary, &bytes)?;
    let result = std::fs::hard_link(&temporary, path);
    std::fs::remove_file(&temporary)?;
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            ensure!(
                std::fs::read(path)? == bytes,
                "immutable RSI record conflict"
            );
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}
fn candidate_value(id: &str, baseline: &AgentPackage, candidate: &AgentPackage) -> Result<Value> {
    let v = json!({"schema":"willdeep.agent-rsi.candidate.v1","candidate_id":id,"grant_id":"local-only","baseline_package":baseline,"candidate_package":candidate});
    contract::validate(&v)?;
    Ok(v)
}
fn candidate_save(
    home: &Path,
    id: &str,
    baseline: AgentPackage,
    candidate: AgentPackage,
) -> Result<Value> {
    let path = root(home, id)?
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("candidates")
        .join(format!("{}.json", digest(id.as_bytes())));
    let value = candidate_value(id, &baseline, &candidate)?;
    immutable(&path, &value)?;
    Ok(json!({"candidate":value,"activated":false}))
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Suite {
    pub suite_id: String,
    pub tasks: Vec<FixtureTask>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FixtureTask {
    pub task_id: String,
    pub split: String,
    pub prompt: String,
    pub marker: String,
    pub fixture: String,
    pub expected_output: String,
}
impl Suite {
    fn builtin() -> Self {
        Self {
            suite_id: "reviewer-offline-v1".into(),
            tasks: ["validation", "regression", "holdout"]
                .into_iter()
                .map(|split| FixtureTask {
                    task_id: format!("{split}-1"),
                    split: split.into(),
                    prompt: "Read fixture.txt and return the bounded fixture review.".into(),
                    marker: format!("RSI-FIXTURE-{split}"),
                    fixture: format!(
                        "// RSI-FIXTURE-{split}\npub fn add(a: i32, b: i32) -> i32 {{ a - b }}\n"
                    ),
                    expected_output: "Offline fixture review: add subtracts its second operand."
                        .into(),
                })
                .collect(),
        }
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.tasks.is_empty() && self.tasks.len() <= 128,
            "invalid fixture count"
        );
        for task in &self.tasks {
            ensure!(
                !task.marker.is_empty()
                    && task.fixture.contains(&task.marker)
                    && !task.expected_output.is_empty(),
                "invalid offline fixture"
            );
        }
        Ok(())
    }
}
fn read_suite(path: &Path) -> Result<Suite> {
    Ok(serde_json::from_value(contract::parse_json(
        &std::fs::read(path)?,
    )?)?)
}
pub(super) fn runtime() -> Result<Value> {
    Ok(
        json!({"adapter":"rust-worker-offline-v1","product_version":env!("CARGO_PKG_VERSION"),"commit":option_env!("WILLDEEP_BUILD_COMMIT").unwrap_or("unknown"),"binary_sha256":digest(&std::fs::read(std::env::current_exe()?)?),"platform":std::env::consts::OS,"architecture":std::env::consts::ARCH}),
    )
}
pub(super) fn empty_hash() -> Result<String> {
    contract::hash(&json!([]))
}
pub(super) fn gate() -> Value {
    json!({"schema":"willdeep.offline-rsi-gate.v1","all_targets_required":true,"all_verifiers_required":true,"all_splits_required":true,"minimum_gain_pp":0,"maximum_token_growth_percent":15,"maximum_time_growth_percent":20,"official_promotion_allowed":false})
}
fn prepare(
    home: &Path,
    id: &str,
    baseline: AgentPackage,
    candidate: AgentPackage,
    suite: Suite,
) -> Result<Value> {
    suite.validate()?;
    let candidate_id = format!("candidate-{id}");
    let record = candidate_value(&candidate_id, &baseline, &candidate)?;
    let mut spec = contract::golden()["values"]["experiment"].clone();
    spec["experiment_id"] = id.into();
    spec["candidate_id"] = candidate_id.into();
    spec["grant_id"] = Value::Null;
    spec["definition_id"] = baseline.definition.definition_id.clone().into();
    spec["baseline_revision"] = baseline.sha256.clone().into();
    spec["candidate_revision"] = candidate.sha256.clone().into();
    spec["hypothesis"]="Offline verification of the revision-targeted evaluation pipeline; no model quality claim.".into();
    spec["runtime"] = runtime()?;
    let suite_value = serde_json::to_value(&suite)?;
    let tasks:Vec<Value>=suite.tasks.iter().map(|task|Ok(json!({"task_id":task.task_id,"split":task.split,"task_sha256":contract::hash(&serde_json::to_value(task)?)?}))).collect::<Result<_>>()?;
    spec["budget"]["max_holdout_accesses"] = ((suite
        .tasks
        .iter()
        .filter(|task| task.split == "holdout")
        .count() as u64
        * 4)
    .min(128))
    .into();
    let fixtures: Vec<_> = suite
        .tasks
        .iter()
        .map(
            |task| json!({"task_id":task.task_id,"content_sha256":digest(task.fixture.as_bytes())}),
        )
        .collect();
    spec["dataset"] = json!({"suite_id":suite.suite_id,"suite_sha256":contract::hash(&suite_value)?,"fixture_sha256":contract::hash(&json!(fixtures))?,"verifier_sha256":contract::hash(&json!({"verifier_id":"rust-offline-read-and-prefix-v1"}))?,"split_sha256":contract::hash(&json!(tasks))?,"tasks":tasks});
    let skills = &baseline.definition.skills;
    spec["conditions"] = json!({"runtime_parameters_sha256":contract::hash(&spec["budget"] )?,"tools_sha256":contract::hash(&json!(willdeep_core::subagent::evaluation::effective_tools(&baseline)?))?,"skills_sha256":contract::hash(&json!(skills))?,"workspace_sha256":spec["dataset"]["fixture_sha256"],"rules_sha256":empty_hash()?,"memory_sha256":empty_hash()?});
    spec["gate"] = json!({"gate_id":"rust-offline-gate-v1","gate_sha256":contract::hash(&gate())?});
    contract::validate(&spec)?;
    let path = root(home, id)?;
    let _guard = runner::RunLock::acquire(&path)?;
    immutable(&path.join("candidate.json"), &record)?;
    immutable(&path.join("suite.json"), &suite_value)?;
    immutable(&path.join("spec.json"), &spec)?;
    Ok(json!({"experiment":spec,"activated":false,"paid_execution_allowed":false}))
}
fn list(home: &Path) -> Result<Value> {
    let path = storage_base(home)?.join("experiments");
    let mut items = Vec::new();
    if path.exists() {
        for e in std::fs::read_dir(path)? {
            let p = e?.path().join("spec.json");
            if p.exists() {
                items.push(contract::parse_json(&std::fs::read(p)?)?);
            }
        }
    }
    Ok(json!({"items":items,"paid_execution_allowed":false}))
}
pub(super) fn load(home: &Path, id: &str) -> Result<(PathBuf, Value, Value, Suite)> {
    let path = root(home, id)?;
    let spec = contract::parse_json(&std::fs::read(path.join("spec.json"))?)?;
    contract::validate(&spec)?;
    ensure!(spec["experiment_id"] == id, "experiment identity mismatch");
    let candidate = contract::parse_json(&std::fs::read(path.join("candidate.json"))?)?;
    contract::validate(&candidate)?;
    let suite: Suite = serde_json::from_value(contract::parse_json(&std::fs::read(
        path.join("suite.json"),
    )?)?)?;
    suite.validate()?;
    ensure!(
        spec["candidate_id"] == candidate["candidate_id"]
            && spec["baseline_revision"] == candidate["baseline_package"]["sha256"]
            && spec["candidate_revision"] == candidate["candidate_package"]["sha256"],
        "candidate binding changed"
    );
    ensure!(
        contract::hash(&serde_json::to_value(&suite)?)? == spec["dataset"]["suite_sha256"],
        "suite changed after registration"
    );
    Ok((path, spec, candidate, suite))
}
fn report(home: &Path, id: &str) -> Result<Value> {
    let path = root(home, id)?;
    contract::parse_json(&std::fs::read(path.join("report.json"))?)
        .context("experiment has no report yet")
}

#[cfg(test)]
#[path = "agent_rsi/tests.rs"]
mod tests;
