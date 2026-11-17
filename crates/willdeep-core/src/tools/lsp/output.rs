//! Normalize only known query result fields. Raw server JSON, arbitrary URIs,
//! relatedInformation and executable command/data payloads never reach callers.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::client::{Document, QueryContext, QueryPayload};
use super::config::{read_document, snapshot, uri, workspace_path};
use super::{Operation, short_text, validate_position};

pub(super) const MAX_OUTPUT_BYTES: usize = 128 * 1024;
const OUTPUT_ENVELOPE_RESERVE: usize = 8192;
const MAX_RESULT_SOURCES: usize = 32;
const MAX_SYMBOL_DEPTH: usize = 64;
const MAX_INSPECTED_RESULTS: usize = 10_000;
const MAX_MESSAGE_BYTES: usize = 2000;
const MAX_LABEL_BYTES: usize = 256;

pub(super) struct Rendered {
    pub(super) text: String,
    pub(super) checked_documents: Vec<Document>,
}

struct Source {
    text: String,
    document: Document,
}

struct Normalizer<'a> {
    root: &'a Path,
    target: &'a Document,
    expected: BTreeMap<PathBuf, Document>,
    sources: BTreeMap<PathBuf, Source>,
    truncated: bool,
}

impl<'a> Normalizer<'a> {
    async fn source(&mut self, raw_uri: &str) -> Result<Option<PathBuf>, String> {
        let Some(path) = workspace_path(self.root, raw_uri) else {
            return Ok(None);
        };
        if self.sources.contains_key(&path) {
            return Ok(Some(path));
        }
        if self.sources.len() >= MAX_RESULT_SOURCES {
            self.truncated = true;
            return Ok(None);
        }
        let text = match read_document(&path).await {
            Ok(text) => text,
            Err(_) => return Ok(None),
        };
        let digest = snapshot(text.as_bytes());
        let expected = self.expected.get(&path);
        if expected.is_some_and(|document| document.snapshot != digest) {
            return Err("result source changed during LSP query; stale result discarded".into());
        }
        let document = Document {
            path: path.clone(),
            uri: uri(&path)?,
            version: expected.map_or(0, |d| d.version),
            snapshot: digest,
        };
        self.sources.insert(path.clone(), Source { text, document });
        Ok(Some(path))
    }

    async fn item(&mut self, item: &Value, operation: Operation) -> Result<Option<Value>, String> {
        if !item.is_object() {
            return Ok(None);
        }
        let (raw_uri, raw_range) = if operation == Operation::Diagnostics {
            (self.target.uri.as_str(), &item["range"])
        } else if item.get("targetUri").is_some() {
            let Some(raw_uri) = item["targetUri"].as_str() else {
                return Ok(None);
            };
            (raw_uri, &item["targetSelectionRange"])
        } else if item.get("location").is_some() {
            let Some(raw_uri) = item["location"]["uri"].as_str() else {
                return Ok(None);
            };
            (raw_uri, &item["location"]["range"])
        } else if item.get("uri").is_some() {
            let Some(raw_uri) = item["uri"].as_str() else {
                return Ok(None);
            };
            (raw_uri, &item["range"])
        } else if operation == Operation::Symbols {
            (self.target.uri.as_str(), &item["selectionRange"])
        } else {
            return Ok(None);
        };
        // Own the URI before mutably borrowing the source cache.
        let raw_uri = raw_uri.to_owned();
        let Some(path) = self.source(&raw_uri).await? else {
            return Ok(None);
        };
        let source = self.sources.get(&path).expect("loaded source");
        let Some(range) = normalized_range(&source.text, raw_range) else {
            return Ok(None);
        };
        let mut output = json!({
            "uri":source.document.uri,
            "path":path.strip_prefix(self.root).map_err(|_| "result left workspace")?.to_string_lossy(),
            "range":range, "version":source.document.snapshot,
            "lsp_version":if source.document.version > 0 {Some(source.document.version)} else {None},
        });
        if operation == Operation::Diagnostics {
            let Some(message) = item["message"].as_str() else {
                return Ok(None);
            };
            self.truncated |= message.len() > MAX_MESSAGE_BYTES;
            output["message"] = json!(short_text(message, MAX_MESSAGE_BYTES));
            output["severity"] = json!(item["severity"].as_u64().filter(|v| (1..=4).contains(v)));
            if let Some(label) = item["source"].as_str() {
                output["source"] = json!(short_text(label, MAX_LABEL_BYTES));
            }
            if item["code"].is_i64() {
                output["code"] = item["code"].clone();
            } else if let Some(code) = item["code"].as_str() {
                output["code"] = json!(short_text(code, MAX_LABEL_BYTES));
            }
        } else if operation == Operation::Symbols {
            let Some(name) = item["name"].as_str() else {
                return Ok(None);
            };
            let Some(kind) = item["kind"].as_u64().filter(|kind| (1..=26).contains(kind)) else {
                return Ok(None);
            };
            self.truncated |= name.len() > MAX_LABEL_BYTES;
            output["name"] = json!(short_text(name, MAX_LABEL_BYTES));
            output["kind"] = json!(kind);
            if let Some(container) = item["containerName"].as_str() {
                output["container_name"] = json!(short_text(container, MAX_LABEL_BYTES));
            }
        }
        Ok(Some(output))
    }
}

fn normalized_range(text: &str, range: &Value) -> Option<Value> {
    let start_line = u32::try_from(range["start"]["line"].as_u64()?).ok()?;
    let start_character = u32::try_from(range["start"]["character"].as_u64()?).ok()?;
    let end_line = u32::try_from(range["end"]["line"].as_u64()?).ok()?;
    let end_character = u32::try_from(range["end"]["character"].as_u64()?).ok()?;
    if (start_line, start_character) > (end_line, end_character) {
        return None;
    }
    validate_position(text, Some(start_line), Some(start_character)).ok()?;
    validate_position(text, Some(end_line), Some(end_character)).ok()?;
    Some(
        json!({"start":{"line":start_line,"character":start_character},"end":{"line":end_line,"character":end_character}}),
    )
}

pub(super) async fn render(
    root: &Path,
    context: &QueryContext,
    operation: Operation,
    limit: usize,
    payload: QueryPayload,
) -> Result<Rendered, String> {
    let mut normalizer = Normalizer {
        root,
        target: &context.document,
        expected: context
            .checked_documents
            .iter()
            .map(|d| (d.path.clone(), d.clone()))
            .collect(),
        sources: BTreeMap::from([(
            context.document.path.clone(),
            Source {
                text: context.text.clone(),
                document: context.document.clone(),
            },
        )]),
        truncated: payload.truncated,
    };
    let mut pending: Vec<(&Value, usize)> = match &payload.raw {
        Value::Null if operation != Operation::Diagnostics => Vec::new(),
        Value::Array(items) => items.iter().rev().map(|v| (v, 0)).collect(),
        Value::Object(_) if operation == Operation::Definition => vec![(&payload.raw, 0)],
        _ => return Err("language server returned an invalid query result shape".into()),
    };
    let mut results = Vec::new();
    let mut filtered = 0_usize;
    let mut inspected = 0_usize;
    let mut bytes = 0_usize;
    while let Some((item, depth)) = pending.pop() {
        if inspected >= MAX_INSPECTED_RESULTS || results.len() >= limit {
            normalizer.truncated = true;
            break;
        }
        inspected += 1;
        if operation == Operation::Symbols
            && let Some(children) = item["children"].as_array()
        {
            if depth >= MAX_SYMBOL_DEPTH {
                normalizer.truncated = true;
            } else {
                pending.extend(children.iter().rev().map(|child| (child, depth + 1)));
            }
        }
        match normalizer.item(item, operation).await? {
            Some(result) => {
                let size = serde_json::to_vec(&result)
                    .map_err(|e| format!("encode normalized LSP result: {e}"))?
                    .len()
                    + 1;
                if bytes + size > MAX_OUTPUT_BYTES - OUTPUT_ENVELOPE_RESERVE {
                    normalizer.truncated = true;
                    break;
                }
                bytes += size;
                results.push(result);
            }
            None => {
                filtered += 1;
            }
        }
    }
    let mut envelope = json!({
        "tool":"lsp", "operation":operation.name(), "position_encoding":"0-based UTF-16",
        "document":{"uri":context.document.uri,
            "path":context.document.path.strip_prefix(root).map_err(|_| "query document left workspace")?.to_string_lossy(),
            "version":context.document.snapshot,"lsp_version":context.document.version},
        "truncated":normalizer.truncated, "filtered_results":filtered,
        "results":results,
        "freshness_scope":"opened overlays refreshed before request; opened and returned source snapshots checked after response; arbitrary external filesystem writes are not transactionally isolated",
    });
    if operation == Operation::Diagnostics {
        let complete = payload.version_verified && !normalizer.truncated && filtered == 0;
        let items = envelope["results"].as_array().expect("results array");
        let has_error = items.iter().any(|item| item["severity"] == 1);
        let known_severities = items.iter().all(|item| item["severity"].as_u64().is_some());
        let clean = if has_error {
            Some(false)
        } else if complete && known_severities {
            Some(true)
        } else {
            None
        };
        envelope["diagnostics"] = json!({
            "transport":payload.diagnostics_mode,"published_version":payload.published_version,
            "version_verified":payload.version_verified,"complete":complete,"clean":clean,
            "scope":"language-server diagnostics only; automatic Rust compiler/build-script checks are disabled",
            "unverified_reason":if !payload.version_verified {Some("unversioned publication cannot prove the current snapshot clean")} else {None},
        });
    }
    let text = serde_json::to_string(&envelope).map_err(|e| format!("encode LSP output: {e}"))?;
    if text.len() > MAX_OUTPUT_BYTES {
        return Err("normalized LSP result exceeds output byte limit".into());
    }
    Ok(Rendered {
        text,
        checked_documents: normalizer
            .sources
            .into_values()
            .map(|s| s.document)
            .collect(),
    })
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
