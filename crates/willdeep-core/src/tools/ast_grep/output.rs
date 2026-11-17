use serde::Deserialize;
use serde_json::{Value, json};

use super::snapshot::Snapshot;
use super::{ToolError, error};

const MAX_MATCHES_PER_FILE: usize = 4096;
const MAX_EXCERPT_CHARACTERS: usize = 160;

#[derive(Deserialize)]
struct Match {
    file: String,
    text: String,
    range: Range,
}

#[derive(Deserialize)]
struct Range {
    #[serde(rename = "byteOffset")]
    byte_offset: ByteOffset,
}

#[derive(Deserialize)]
struct ByteOffset {
    start: usize,
    end: usize,
}

pub(super) fn normalize(
    output: &std::process::Output,
    snapshot: &Snapshot,
) -> Result<Vec<Value>, ToolError> {
    let code = output.status.code();
    if !matches!(code, Some(0 | 1)) {
        return Err(error(format!(
            "CLI failed for {} (exit {code:?}); no result accepted",
            snapshot.relative
        )));
    }
    if !output.stderr.is_empty() {
        return Err(error(
            "CLI emitted diagnostics; no successful or empty search inferred from stderr",
        ));
    }
    let values: Vec<Value> = serde_json::from_slice(&output.stdout).map_err(|_| {
        error("CLI stdout is not a complete UTF-8 JSON array; no partial or tail JSON accepted")
    })?;
    if values.len() > MAX_MATCHES_PER_FILE {
        return Err(error("CLI match count exceeded per-file budget"));
    }
    if code == Some(1) && !values.is_empty() {
        return Err(error(
            "CLI no-match exit carried matches; inconsistent output rejected",
        ));
    }
    values
        .into_iter()
        .map(|value| {
            if value.get("replacement").is_some() || value.get("replacementOffsets").is_some() {
                return Err(error(
                    "CLI returned rewrite data on a query-only invocation",
                ));
            }
            let item: Match = serde_json::from_value(value)
                .map_err(|_| error("CLI match has an invalid schema"))?;
            // Official run --stdin uses exactly STDIN. Child-supplied paths are
            // never resolved, read or copied into the returned host location.
            if item.file != "STDIN" {
                return Err(error(
                    "CLI match claimed a non-stdin file; external or forged location rejected",
                ));
            }
            let start = item.range.byte_offset.start;
            let end = item.range.byte_offset.end;
            if start >= end || snapshot.text.get(start..end) != Some(item.text.as_str()) {
                return Err(error(
                    "CLI match byte range or text disagrees with the UTF-8 host snapshot",
                ));
            }
            let excerpt: String = item.text.chars().take(MAX_EXCERPT_CHARACTERS).collect();
            Ok(json!({
                "path":snapshot.relative, "hash":snapshot.hash,
                "byte_range":{"start":start,"end":end},
                "text_truncated":excerpt.len() != item.text.len(), "text":excerpt
            }))
        })
        .collect()
}
