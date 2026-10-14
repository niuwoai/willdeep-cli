use super::*;
use std::collections::BTreeMap;
use willdeep_runtime_protocol::agent_team::{
    AgentTeamPolicy, AgentTeamSummary, AgentUsageSummary, AgentWorkerSummary,
};

mod ledger;

impl AgentRunStore {
    pub(crate) fn has_budget(&self, run_id: Option<Uuid>) -> Result<bool, String> {
        if self
            .team_policy()?
            .is_some_and(|policy| policy.token_budget.is_some())
        {
            return Ok(true);
        }
        Ok(match run_id {
            Some(id) => self.get(id)?.is_some_and(|run| run.token_budget.is_some()),
            None => false,
        })
    }

    pub(crate) fn budget_gate(&self, ledger: &Path) -> Result<(), String> {
        let Some(budget) = self.team_policy()?.and_then(|policy| policy.token_budget) else {
            return Ok(());
        };
        let mut summary = AgentTeamSummary {
            parent_session_id: self.parent,
            policy: None,
            running: 0,
            queued: 0,
            completed_runs: 0,
            workers: Vec::new(),
            usage: Default::default(),
            parent_usage: Default::default(),
            child_usage: Default::default(),
            auxiliary_usage: Default::default(),
            compression_usage: Default::default(),
            ledger_incomplete: false,
        };
        let recorded = read_ledger(ledger, &mut summary)?;
        let entries = self.entries()?;
        summary.ledger_incomplete |= entries.iter().any(|entry| {
            entry.record.usage.model_calls > recorded.get(&entry.record.id).copied().unwrap_or(0)
        });
        if summary.ledger_incomplete || summary.usage.unknown_usage_calls > 0 {
            return Err("Agent team budget cannot be verified because model usage is unknown or the ledger is incomplete".into());
        }
        if entries
            .iter()
            .any(|entry| entry.record.usage.uncertain_execution)
        {
            return Err(
                "Agent team budget cannot be verified after an interrupted execution".into(),
            );
        }
        if summary.usage.accounted_tokens >= budget {
            return Err(format!(
                "Agent team token budget exhausted: used {}, limit {budget}",
                summary.usage.accounted_tokens
            ));
        }
        Ok(())
    }

    /// Freeze the policy once per parent session; restarting must not silently
    /// replace a live team's limits with newly edited global configuration.
    pub fn configure_team(
        &self,
        settings: super::super::scheduler::AgentTeamSettings,
    ) -> Result<super::super::scheduler::AgentTeamSettings, String> {
        settings.validate()?;
        self.locked(|| {
            if let Some(policy) = self.team_policy()? {
                let saved = super::super::scheduler::AgentTeamSettings {
                    max_concurrent: policy.max_concurrent,
                    max_queued: policy.max_queued,
                    token_budget: policy.token_budget,
                };
                saved.validate()?;
                return Ok(saved);
            }
            let policy = AgentTeamPolicy {
                max_concurrent: settings.max_concurrent,
                max_queued: settings.max_queued,
                token_budget: settings.token_budget,
            };
            let path = self.directory.join(".policy.json");
            let temporary = self.directory.join(format!(".{}.policy", Uuid::new_v4()));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary).map_err(storage_error)?;
            file.write_all(&serde_json::to_vec(&policy).map_err(storage_error)?)
                .map_err(storage_error)?;
            file.sync_all().map_err(storage_error)?;
            std::fs::rename(temporary, path).map_err(storage_error)?;
            #[cfg(unix)]
            std::fs::File::open(&self.directory)
                .and_then(|file| file.sync_all())
                .map_err(storage_error)?;
            Ok(settings)
        })
    }

    pub(super) fn team_policy(&self) -> Result<Option<AgentTeamPolicy>, String> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(self.directory.join(".policy.json")) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage_error(error)),
        };
        if !file.metadata().map_err(storage_error)?.is_file() {
            return Err("Agent team policy is not a file".into());
        }
        let mut bytes = Vec::new();
        file.take(4097)
            .read_to_end(&mut bytes)
            .map_err(storage_error)?;
        if bytes.len() > 4096 {
            return Err("Agent team policy exceeds size limit".into());
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(storage_error)
    }

    /// Read-only projection. Public results never include private dispatch data.
    pub fn team_summary(&self, ledger_directory: &Path) -> Result<AgentTeamSummary, String> {
        let entries = self.entries()?;
        let recorded_runs: BTreeMap<_, _> = entries
            .iter()
            .map(|entry| (entry.record.id, entry.record.usage.model_calls))
            .collect();
        let mut records: Vec<_> = entries.into_iter().map(|entry| entry.record).collect();
        records.sort_by_key(|run| run.sequence);
        let mut summary = AgentTeamSummary {
            parent_session_id: self.parent,
            policy: self.team_policy()?,
            running: 0,
            queued: 0,
            completed_runs: 0,
            workers: Vec::new(),
            usage: Default::default(),
            parent_usage: Default::default(),
            child_usage: Default::default(),
            auxiliary_usage: Default::default(),
            compression_usage: Default::default(),
            ledger_incomplete: false,
        };
        let mut workers: BTreeMap<Uuid, AgentWorkerSummary> = BTreeMap::new();
        for mut run in records {
            // Automatic activity polling exposes metadata, never worker reports.
            // Reports are available only through the explicit paginated runs API.
            run.report = None;
            match run.status {
                AgentStatus::Queued => summary.queued += 1,
                AgentStatus::Running => summary.running += 1,
                _ => summary.completed_runs += 1,
            }
            let worker = workers
                .entry(run.agent_id)
                .or_insert_with(|| AgentWorkerSummary {
                    agent_id: run.agent_id,
                    latest_run: run.clone(),
                    run_count: 0,
                    queue_position: None,
                    cumulative_usage: Default::default(),
                });
            worker.run_count += 1;
            merge(&mut worker.cumulative_usage, &run.usage);
            worker.queue_position = (run.status == AgentStatus::Queued).then_some(summary.queued);
            worker.latest_run = run;
        }
        summary.workers = workers.into_values().collect();
        summary
            .workers
            .sort_by_key(|worker| worker.latest_run.sequence);
        let recorded = read_ledger(ledger_directory, &mut summary)?;
        summary.ledger_incomplete |= recorded_runs
            .iter()
            .any(|(id, calls)| *calls > recorded.get(id).copied().unwrap_or(0));
        summary.usage.uncertain_execution = summary
            .workers
            .iter()
            .any(|worker| worker.cumulative_usage.uncertain_execution);
        Ok(summary)
    }
}

fn merge(target: &mut AgentUsageSummary, source: &AgentUsageSummary) {
    target.model_calls = target.model_calls.saturating_add(source.model_calls);
    target.unknown_usage_calls = target
        .unknown_usage_calls
        .saturating_add(source.unknown_usage_calls);
    target.accounted_tokens = target
        .accounted_tokens
        .saturating_add(source.accounted_tokens);
    target.uncertain_execution |= source.uncertain_execution;
    add(&mut target.input_tokens, source.input_tokens);
    add(&mut target.output_tokens, source.output_tokens);
    add(&mut target.cache_read_tokens, source.cache_read_tokens);
    add(&mut target.total_tokens, source.total_tokens);
}

fn counted(usage: &Usage) -> AgentUsageSummary {
    let total = usage.total_tokens.or_else(|| {
        usage
            .input_tokens
            .zip(usage.output_tokens)
            .map(|(input, output)| input.saturating_add(output))
    });
    AgentUsageSummary {
        model_calls: 1,
        unknown_usage_calls: u64::from(total.is_none()),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        total_tokens: total,
        accounted_tokens: usage.billable_tokens(),
        uncertain_execution: false,
    }
}

fn read_ledger(
    directory: &Path,
    summary: &mut AgentTeamSummary,
) -> Result<BTreeMap<Uuid, u64>, String> {
    ledger::read(directory, summary)
}
