use std::collections::{BTreeMap, HashSet};

use chrono::NaiveDate;
use serde::Serialize;
use uuid::Uuid;
use willdeep_core::usage_ledger::{ClientKind, Outcome, UsageKind};

use super::shards::Row;
use super::skills::SkillActivity;
use super::{DATE_FORMAT, Detail};

#[derive(Serialize)]
pub(super) struct Report {
    days: Vec<Day>,
    totals: Totals,
    agents: Vec<AgentGroup>,
    models: Vec<Group>,
    skills: Vec<SkillCount>,
    skipped_rows: u64,
}

#[derive(Serialize)]
struct Day {
    date: String,
    totals: Totals,
    source_calls: BTreeMap<&'static str, u64>,
}

#[derive(Default)]
struct DailyBucket {
    usage: Bucket,
    source_calls: BTreeMap<&'static str, u64>,
}

impl DailyBucket {
    fn record(&mut self, date: NaiveDate, record: &Row, worker: bool) {
        self.usage.record(date, record, worker);
        let source = match record.client {
            ClientKind::Cli => "cli",
            ClientKind::Tui => "tui",
            ClientKind::Mobile => "mobile",
            ClientKind::Unknown => "unknown",
        };
        *self.source_calls.entry(source).or_default() += 1;
    }
}

#[derive(Serialize)]
struct Group {
    model: Option<String>,
    provider: Option<String>,
    totals: Totals,
}

#[derive(Serialize)]
struct AgentGroup {
    agent: String,
    model: Option<String>,
    provider: Option<String>,
    totals: Totals,
}

#[derive(Serialize)]
struct SkillCount {
    name: String,
    executions: u64,
}

#[derive(Default, Serialize)]
struct Totals {
    tokens: Option<u128>,
    input: Option<u128>,
    output: Option<u128>,
    cache_read: Option<u128>,
    cache_rate: Option<f64>,
    calls: u64,
    sessions: usize,
    active_days: usize,
    known_usage_calls: u64,
    unpriced_calls: u64,
    worker_runs: usize,
    unknown_run_calls: u64,
    errors: u64,
    cancelled: u64,
}

#[derive(Default)]
struct Bucket {
    totals: Totals,
    sessions: HashSet<Uuid>,
    days: HashSet<NaiveDate>,
    runs: HashSet<Uuid>,
    paired_input: u128,
    paired_cache: u128,
}

impl Bucket {
    fn record(&mut self, date: NaiveDate, record: &Row, worker: bool) {
        let usage = &record.usage;
        let totals = &mut self.totals;
        add_known(&mut totals.tokens, usage.total_tokens);
        add_known(&mut totals.input, usage.input_tokens);
        add_known(&mut totals.output, usage.output_tokens);
        add_known(&mut totals.cache_read, usage.cache_read_tokens);
        if usage.total_tokens.is_some()
            || usage.input_tokens.is_some()
            || usage.output_tokens.is_some()
            || usage.cache_read_tokens.is_some()
        {
            totals.known_usage_calls += 1;
        }
        // Limit cache-rate coverage to calls with matching usable numerator and denominator.
        if let (Some(input), Some(cache)) = (usage.input_tokens, usage.cache_read_tokens)
            && cache <= input
        {
            self.paired_input += u128::from(input);
            self.paired_cache += u128::from(cache);
        }
        totals.calls += 1;
        // The ledger has no charge amount and there is no time-aware price table.
        totals.unpriced_calls += 1;
        totals.errors += u64::from(matches!(record.outcome, Outcome::Error));
        totals.cancelled += u64::from(matches!(record.outcome, Outcome::Cancelled));
        self.sessions.extend(record.session_id);
        self.days.insert(date);
        if worker {
            if let Some(run) = record.run_id {
                self.runs.insert(run);
            } else {
                totals.unknown_run_calls += 1;
            }
        }
    }

    fn finish(mut self) -> Totals {
        self.totals.sessions = self.sessions.len();
        self.totals.active_days = self.days.len();
        self.totals.worker_runs = self.runs.len();
        if self.paired_input > 0 {
            self.totals.cache_rate = Some(self.paired_cache as f64 / self.paired_input as f64);
        }
        self.totals
    }
}

fn add_known(sum: &mut Option<u128>, value: Option<u64>) {
    if let Some(value) = value {
        *sum.get_or_insert(0) += u128::from(value);
    }
}

fn is_worker(record: &Row) -> bool {
    record.run_id.is_some()
        || matches!(record.kind, UsageKind::Subagent)
        || (matches!(record.kind, UsageKind::Auxiliary | UsageKind::Compression)
            && record.agent_id.as_ref().is_some_and(|id| !id.is_empty()))
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct ModelKey {
    model: Option<String>,
    provider: Option<String>,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct AgentKey {
    agent: String,
    model: Option<String>,
    provider: Option<String>,
}

pub(super) struct ReportBuilder {
    detail: Detail,
    seen: HashSet<Uuid>,
    days: BTreeMap<NaiveDate, DailyBucket>,
    period: Bucket,
    groups: BTreeMap<ModelKey, Bucket>,
    agents: BTreeMap<AgentKey, Bucket>,
}

impl ReportBuilder {
    pub(super) fn new(start: NaiveDate, end: NaiveDate, detail: Detail) -> Self {
        Self {
            detail,
            seen: HashSet::new(),
            days: start
                .iter_days()
                .take_while(|date| *date <= end)
                .map(|date| (date, DailyBucket::default()))
                .collect(),
            period: Bucket::default(),
            groups: BTreeMap::new(),
            agents: BTreeMap::new(),
        }
    }

    pub(super) fn record(&mut self, date: NaiveDate, record: Row) {
        if !self.seen.insert(record.id) {
            return;
        }
        let worker = is_worker(&record);
        self.period.record(date, &record, worker);
        if let Some(day) = self.days.get_mut(&date) {
            day.record(date, &record, worker);
        }
        if self.detail == Detail::Day {
            let key = ModelKey {
                model: record.model.clone(),
                provider: record.provider.clone(),
            };
            self.groups
                .entry(key)
                .or_default()
                .record(date, &record, worker);
        }
        if worker && matches!(self.detail, Detail::Day | Detail::Agents) {
            let key = AgentKey {
                agent: agent_name(&record),
                model: record.model.clone(),
                provider: record.provider.clone(),
            };
            self.agents
                .entry(key)
                .or_default()
                .record(date, &record, true);
        }
    }

    pub(super) fn finish(self, skipped_rows: u64, skills: SkillActivity) -> Report {
        let agents = self
            .agents
            .into_iter()
            .map(|(key, bucket)| AgentGroup {
                agent: key.agent,
                model: key.model,
                provider: key.provider,
                totals: bucket.finish(),
            })
            .collect();
        let models = match self.detail {
            Detail::Day => self
                .groups
                .into_iter()
                .map(|(key, bucket)| Group {
                    model: key.model,
                    provider: key.provider,
                    totals: bucket.finish(),
                })
                .collect(),
            Detail::Summary | Detail::Agents => Vec::new(),
        };
        let skills = match self.detail {
            Detail::Day => skills
                .by_name
                .into_iter()
                .map(|(name, executions)| SkillCount { name, executions })
                .collect(),
            Detail::Summary | Detail::Agents => Vec::new(),
        };
        Report {
            days: self
                .days
                .into_iter()
                .map(|(date, bucket)| Day {
                    date: date.format(DATE_FORMAT).to_string(),
                    totals: bucket.usage.finish(),
                    source_calls: bucket.source_calls,
                })
                .collect(),
            totals: self.period.finish(),
            agents,
            models,
            skills,
            skipped_rows,
        }
    }
}

/// The ledger stores an opaque agent id, never the sub-agent's name, so the
/// only stable label it can offer is the model the run actually called.
fn agent_name(record: &Row) -> String {
    record
        .model
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}
