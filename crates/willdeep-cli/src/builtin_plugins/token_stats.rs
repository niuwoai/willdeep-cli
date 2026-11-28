//! Read-only statistics over the local usage ledger.

#[path = "token_stats/aggregate.rs"]
mod aggregate;
#[path = "token_stats/shards.rs"]
mod shards;
#[path = "token_stats/skills.rs"]
mod skills;
#[cfg(test)]
#[path = "token_stats/tests.rs"]
mod tests;

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::NaiveDate;
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{Value, json};
use willdeep_core::usage_ledger::{ClientKind, ledger_dir};

use self::aggregate::{Report, ReportBuilder};
use self::skills::SkillActivity;
use super::stdio_server::{BuiltinPlugin, HostClient};

const QUERY: &str = "query";
const MAX_DAYS: i64 = 366;
const DATE_FORMAT: &str = "%Y-%m-%d";

pub(crate) struct TokenStats {
    home: PathBuf,
}

impl TokenStats {
    pub(crate) fn new(home: PathBuf) -> Self {
        Self { home }
    }
}

#[async_trait]
impl BuiltinPlugin for TokenStats {
    fn server_name(&self) -> &'static str {
        "token_stats"
    }

    fn tools(&self) -> Vec<Value> {
        vec![json!({
            "name": QUERY,
            "description": "Query aggregate local model usage for up to 366 inclusive calendar days.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["start", "end", "timezone", "source", "detail"],
                "properties": {
                    "start": {"type": "string", "format": "date"},
                    "end": {"type": "string", "format": "date"},
                    "timezone": {"type": "string", "description": "IANA timezone name"},
                    "source": {"enum": ["all", "cli", "tui", "mobile", "unknown"]},
                    "detail": {"enum": ["summary", "day", "agents"]}
                }
            }
        })]
    }

    async fn call(
        &self,
        name: &str,
        arguments: Value,
        _host: &HostClient,
    ) -> Result<String, String> {
        if name != QUERY {
            return Err("Unknown token statistics tool".to_owned());
        }
        let query = Query::parse(arguments)?;
        let home = self.home.clone();
        tokio::task::spawn_blocking(move || {
            let report = read_report(&ledger_dir(&home), &home, query)?;
            serde_json::to_string(&report)
                .map_err(|error| format!("Encode token statistics: {error}"))
        })
        .await
        .map_err(|error| format!("Token statistics task failed: {error}"))?
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Source {
    All,
    Cli,
    Tui,
    Mobile,
    Unknown,
}

impl Source {
    fn includes(self, client: ClientKind) -> bool {
        matches!(
            (self, client),
            (Self::All, _)
                | (Self::Cli, ClientKind::Cli)
                | (Self::Tui, ClientKind::Tui)
                | (Self::Mobile, ClientKind::Mobile)
                | (Self::Unknown, ClientKind::Unknown)
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Detail {
    Summary,
    Day,
    Agents,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    start: String,
    end: String,
    timezone: String,
    source: Source,
    detail: Detail,
}

struct Query {
    start: NaiveDate,
    end: NaiveDate,
    timezone: Tz,
    source: Source,
    detail: Detail,
}

impl Query {
    fn parse(value: Value) -> Result<Self, String> {
        let args: Args = serde_json::from_value(value).map_err(|_| {
            "Expected start, end, timezone, source and detail with valid types and enum values"
                .to_owned()
        })?;
        let start = parse_date(&args.start, "start")?;
        let end = parse_date(&args.end, "end")?;
        let span = (end - start).num_days() + 1;
        if !(1..=MAX_DAYS).contains(&span) {
            return Err(format!(
                "Expected start <= end and at most {MAX_DAYS} inclusive days"
            ));
        }
        if args.detail == Detail::Day && span != 1 {
            return Err("Day detail requires start and end to be the same date".to_owned());
        }
        let timezone = args
            .timezone
            .parse()
            .map_err(|_| "Expected an IANA timezone name".to_owned())?;
        Ok(Self {
            start,
            end,
            timezone,
            source: args.source,
            detail: args.detail,
        })
    }
}

fn parse_date(raw: &str, field: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(raw, DATE_FORMAT)
        .ok()
        .filter(|date| date.format(DATE_FORMAT).to_string() == raw && raw.len() == 10)
        .ok_or_else(|| format!("Expected {field} as YYYY-MM-DD"))
}

fn read_report(dir: &Path, home: &Path, query: Query) -> Result<Report, String> {
    let mut report = ReportBuilder::new(query.start, query.end, query.detail);
    let skipped_rows = shards::visit(dir, query.start, query.end, |record, ts| {
        let local_date = ts.with_timezone(&query.timezone).date_naive();
        if (query.start..=query.end).contains(&local_date) && query.source.includes(record.client) {
            report.record(local_date, record);
        }
    })?;
    // Skill reads live in session history, which has no client attribution, so the
    // source filter does not apply to them. A failure here is reported, not hidden.
    let skills = if query.detail == Detail::Day {
        skills::read(home, query.start, query.end, query.timezone)?
    } else {
        SkillActivity::default()
    };
    Ok(report.finish(skipped_rows, skills))
}
