use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use chrono::{DateTime, Datelike, Months, NaiveDate, Utc};
use serde::Deserialize;
use uuid::Uuid;
use willdeep_core::types::Usage;
use willdeep_core::usage_ledger::{ClientKind, MAX_LINE_BYTES, Outcome, SCHEMA, UsageKind};

// Deserialize only fields used by this query. The ledger can add unrelated metadata
// without changing reports, and paths and other unused IDs never enter aggregation.
#[derive(Deserialize)]
pub(super) struct Row {
    schema: String,
    pub id: Uuid,
    pub ts: String,
    pub client: ClientKind,
    pub kind: UsageKind,
    pub outcome: Outcome,
    pub session_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    #[serde(flatten)]
    pub usage: Usage,
}

impl Row {
    fn utc_timestamp(&self) -> Option<DateTime<Utc>> {
        if !self.ts.ends_with('Z') {
            return None;
        }
        DateTime::parse_from_rfc3339(&self.ts)
            .ok()
            .map(|ts| ts.to_utc())
    }
}

pub(super) fn visit(
    dir: &Path,
    start: NaiveDate,
    end: NaiveDate,
    mut accept: impl FnMut(Row, DateTime<Utc>),
) -> Result<u64, String> {
    // All IANA UTC offsets are smaller than a day. Pad in calendar dates, then
    // filter each call by its own local date to handle folds, gaps and skipped days.
    let mut month = month_start(start.pred_opt().unwrap_or(start));
    let last = month_start(end.succ_opt().unwrap_or(end));
    let mut skipped = 0;
    loop {
        let name = format!("{}.jsonl", month.format("%Y-%m"));
        match File::open(dir.join(&name)) {
            Ok(file) => {
                skipped += visit_file(BufReader::new(file), &mut accept)
                    .map_err(|error| format!("Read usage ledger shard {name}: {error}"))?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Open usage ledger shard {name}: {error}")),
        }
        if month == last {
            return Ok(skipped);
        }
        month = month
            .checked_add_months(Months::new(1))
            .ok_or_else(|| "Usage ledger month is out of range".to_owned())?;
    }
}

fn month_start(date: NaiveDate) -> NaiveDate {
    date.with_day(1).expect("every month starts on day 1")
}

fn visit_file(
    mut reader: impl BufRead,
    accept: &mut impl FnMut(Row, DateTime<Utc>),
) -> io::Result<u64> {
    let mut line = Vec::with_capacity(MAX_LINE_BYTES);
    let mut skipped = 0;
    loop {
        line.clear();
        let len = (&mut reader)
            .take(MAX_LINE_BYTES as u64)
            .read_until(b'\n', &mut line)?;
        let terminated = line.ends_with(b"\n");
        if len == MAX_LINE_BYTES {
            if !terminated {
                reader.skip_until(b'\n')?;
            }
            skipped += 1;
            continue;
        }
        if !terminated {
            // EOF or an unfinished append. A subsequent query will retry the row.
            return Ok(skipped);
        }
        if let Ok(record) = serde_json::from_slice::<Row>(&line)
            && record.schema == SCHEMA
            && let Some(ts) = record.utc_timestamp()
        {
            accept(record, ts);
        } else {
            skipped += 1;
        }
    }
}
