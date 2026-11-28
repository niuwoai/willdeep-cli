//! Skill reads counted from local session history.
//!
//! The usage ledger records model calls, not which skill a turn applied, so the
//! only durable evidence is an assistant `read_skill` tool call. `list_skills`
//! only browses the catalog and is not an execution. Sessions carry no per-message
//! timestamp, so a file counts when its update falls inside the queried window.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use serde_json::Value;

pub(super) const READ_SKILL: &str = "read_skill";
const MAX_SESSION_BYTES: u64 = 64 * 1024 * 1024;
const SKILL_NAME_LIMIT: usize = 80;

#[derive(Default)]
pub(super) struct SkillActivity {
    pub executions: u64,
    pub by_name: BTreeMap<String, u64>,
}

pub(super) fn read(
    home: &Path,
    start: NaiveDate,
    end: NaiveDate,
    timezone: Tz,
) -> Result<SkillActivity, String> {
    let mut activity = SkillActivity::default();
    let directory = home.join("sessions");
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(activity),
        Err(error) => return Err(format!("Read session directory: {error}")),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("Read session directory: {error}"))?;
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_session_file)
        {
            continue;
        }
        let Some(value) = read_session(&path)? else {
            continue;
        };
        let Some(updated_at) = value.get("updated_at").and_then(Value::as_u64) else {
            continue;
        };
        let updated = DateTime::<Utc>::from_timestamp(updated_at as i64, 0);
        if updated.is_none_or(|ts| !in_window(ts.into(), start, end, timezone)) {
            continue;
        }
        let Some(messages) = value.get("messages").and_then(Value::as_array) else {
            continue;
        };
        for message in messages {
            for call in tool_calls(message) {
                if call.0 != READ_SKILL {
                    continue;
                }
                activity.executions += 1;
                *activity.by_name.entry(skill_name(call.1)).or_default() += 1;
            }
        }
    }
    Ok(activity)
}

fn tool_calls(message: &Value) -> Vec<(&str, &str)> {
    let mut calls = Vec::new();
    for (key, name_key, arguments_key) in [
        ("tool_calls", "name", "arguments"),
        ("toolCalls", "toolName", "argumentsJSON"),
    ] {
        let Some(list) = message.get(key).and_then(Value::as_array) else {
            continue;
        };
        for call in list {
            let name = call
                .get(name_key)
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = call
                .get(arguments_key)
                .and_then(Value::as_str)
                .unwrap_or_default();
            calls.push((name, arguments));
        }
    }
    calls
}

fn is_session_file(name: &str) -> bool {
    let Some(id) = name.strip_suffix(".json") else {
        return false;
    };
    let mut parts = id.split('-');
    [8, 4, 4, 4, 12].into_iter().all(|length| {
        parts.next().is_some_and(|part| {
            part.len() == length && part.chars().all(|ch| ch.is_ascii_hexdigit())
        })
    }) && parts.next().is_none()
}

fn in_window(time: SystemTime, start: NaiveDate, end: NaiveDate, timezone: Tz) -> bool {
    let Ok(elapsed) = time.duration_since(UNIX_EPOCH) else {
        return false;
    };
    DateTime::<Utc>::from_timestamp(elapsed.as_secs() as i64, 0)
        .is_some_and(|ts| (start..=end).contains(&ts.with_timezone(&timezone).date_naive()))
}

fn read_session(path: &Path) -> Result<Option<Value>, String> {
    let mut file = File::open(path).map_err(|error| format!("Open session history: {error}"))?;
    let length = file
        .metadata()
        .map_err(|error| format!("Read session history: {error}"))?
        .len();
    if length > MAX_SESSION_BYTES {
        // One oversized transcript must not blank the day's other skill counts.
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("Read session history: {error}"))?;
    match serde_json::from_slice(&bytes) {
        Ok(head) => Ok(Some(head)),
        // A transcript this plugin cannot parse is skipped; the ledger report still stands.
        Err(_) => Ok(None),
    }
}

/// The skill's identifier only. Anything that is not a `read_skill` argument
/// object counts as unnamed rather than failing the whole query.
fn skill_name(arguments: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(arguments) else {
        return String::new();
    };
    let Some(name) = value.get("name").and_then(Value::as_str) else {
        return String::new();
    };
    let name = name.trim();
    if name.is_empty()
        || name.chars().count() > SKILL_NAME_LIMIT
        || name.chars().any(|ch| ch.is_control() || "/\\".contains(ch))
    {
        return String::new();
    }
    name.to_owned()
}
