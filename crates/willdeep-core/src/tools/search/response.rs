use serde::Serialize;
use serde_json::Value;

use super::{Backend, failure};
use crate::tools::ToolError;

const MAX_TITLE_CHARS: usize = 400;
const MAX_CONTENT_CHARS: usize = 2_000;
const MAX_URL_BYTES: usize = 8_192;
const MAX_ENGINE_CHARS: usize = 80;
const MAX_ENGINES: usize = 8;

#[derive(Serialize)]
struct SearchResult {
    title: String,
    url: String,
    content: String,
    sources: Vec<String>,
}

pub(super) fn format_results(
    body: &[u8],
    backend: Backend,
    endpoint: &str,
    count: usize,
    key: Option<&str>,
) -> Result<String, ToolError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| malformed())?;
    let data = match backend {
        Backend::Searxng => &value,
        Backend::SomeIm => value.get("data").unwrap_or(&value),
    };
    let items = match backend {
        Backend::Searxng => data.get("results").and_then(Value::as_array),
        Backend::SomeIm => data
            .get("results")
            .or_else(|| data.get("items"))
            .and_then(Value::as_array)
            .or_else(|| data.as_array()),
    }
    .ok_or_else(malformed)?;
    let mut results = Vec::new();
    let mut truncated = items.len() > count;
    for (index, item) in items.iter().enumerate() {
        // Validate every entry, including ones outside the requested count.
        let object = item.as_object().ok_or_else(malformed)?;
        let title = object
            .get("title")
            .and_then(Value::as_str)
            .ok_or_else(malformed)?;
        let url = object
            .get("url")
            .or_else(|| {
                matches!(backend, Backend::SomeIm)
                    .then(|| object.get("link"))
                    .flatten()
            })
            .and_then(Value::as_str)
            .ok_or_else(malformed)?;
        let content_field = match backend {
            Backend::Searxng => object.get("content"),
            Backend::SomeIm => object
                .get("snippet")
                .or_else(|| object.get("content"))
                .or_else(|| object.get("description")),
        };
        let content = match content_field {
            Some(value) => value.as_str().ok_or_else(malformed)?,
            None if matches!(backend, Backend::SomeIm) => "",
            None => return Err(malformed()),
        };
        if title.trim().is_empty() || url.len() > MAX_URL_BYTES {
            return Err(malformed());
        }
        let parsed_url = reqwest::Url::parse(url).map_err(|_| malformed())?;
        if !matches!(parsed_url.scheme(), "https" | "http")
            || parsed_url.host_str().is_none()
            || parsed_url
                .as_str()
                .split('/')
                .nth(2)
                .is_some_and(|authority| authority.contains('@'))
        {
            return Err(malformed());
        }
        let mut sources = Vec::new();
        if let Some(engines) = object.get("engines") {
            for engine in engines.as_array().ok_or_else(malformed)? {
                let engine = engine.as_str().ok_or_else(malformed)?;
                if sources.len() < MAX_ENGINES {
                    sources.push(bounded(engine, MAX_ENGINE_CHARS, key, &mut truncated));
                } else {
                    truncated = true;
                }
            }
        }
        if let Some(engine) = object.get("engine") {
            let engine = engine.as_str().ok_or_else(malformed)?;
            let engine = bounded(engine, MAX_ENGINE_CHARS, key, &mut truncated);
            if !sources.contains(&engine) {
                if sources.len() < MAX_ENGINES {
                    sources.push(engine);
                } else {
                    truncated = true;
                }
            }
        }
        if sources.is_empty() {
            sources.push(backend.name().into());
        }
        if index < count {
            results.push(SearchResult {
                title: bounded(title, MAX_TITLE_CHARS, key, &mut truncated),
                url: redact(url, key),
                content: bounded(content, MAX_CONTENT_CHARS, key, &mut truncated),
                sources,
            });
        }
    }
    serde_json::to_string_pretty(&serde_json::json!({
        "backend": backend.name(),
        "source": redact(endpoint, key),
        "empty": results.is_empty(),
        "truncated": truncated,
        "returned_count": results.len(),
        "received_count": items.len(),
        "results": results,
    }))
    .map_err(|_| failure("encoding", "cannot encode search results"))
}

fn redact(value: &str, key: Option<&str>) -> String {
    match key.filter(|value| !value.is_empty()) {
        Some(key) => value.replace(key, "[redacted]"),
        None => value.to_owned(),
    }
}

fn bounded(value: &str, limit: usize, key: Option<&str>, truncated: &mut bool) -> String {
    let value = redact(value, key);
    if value.chars().count() > limit {
        *truncated = true;
        value.chars().take(limit).collect()
    } else {
        value
    }
}

fn malformed() -> ToolError {
    failure(
        "malformed_response",
        "search response must contain a results array with string title, HTTP(S) url and content fields",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_response_rejects_wrong_types_and_unknown_shapes() {
        for body in [
            r#"{}"#,
            r#"{"results":{}}"#,
            r#"{"results":[{"title":"a","url":"https://example.org","content":42}]}"#,
            r#"{"results":[{"title":"a","url":"file:///tmp/x","content":"b"}]}"#,
            r#"{"results":[{"title":"a","url":"https://u:p@example.org","content":"b"}]}"#,
            r#"{"results":[{"title":"a","url":"https://example.org","content":"b","engines":[42]}]}"#,
        ] {
            let error = format_results(
                body.as_bytes(),
                Backend::Searxng,
                "https://search.invalid",
                8,
                None,
            )
            .unwrap_err();
            assert!(error.to_string().contains("malformed_response"));
        }
    }

    #[test]
    fn someim_allows_absent_snippets_but_rejects_invalid_field_types() {
        let body = br#"{"data":{"results":[{"title":"one","url":"https://example.org/1"},{"title":"two","url":"https://example.org/2"}]}}"#;
        let value: Value = serde_json::from_str(
            &format_results(body, Backend::SomeIm, "https://search.invalid", 1, None).unwrap(),
        )
        .unwrap();
        assert_eq!(value["returned_count"], 1);
        assert_eq!(value["results"][0]["content"], "");
        assert_eq!(value["truncated"], true);
        for bad in ["null", "42", "false", "[]"] {
            let body = format!(
                r#"{{"data":{{"items":[{{"title":"x","link":"https://example.org","snippet":{bad}}}]}}}}"#
            );
            assert!(
                format_results(
                    body.as_bytes(),
                    Backend::SomeIm,
                    "https://search.invalid",
                    8,
                    None
                )
                .is_err()
            );
        }
    }

    #[test]
    fn results_redact_credentials_and_bound_unicode_fields() {
        let body = serde_json::to_vec(&serde_json::json!({"results":[{
            "title": "fixture-secret", "url": "https://example.org", "content": "界".repeat(MAX_CONTENT_CHARS + 1),
            "engines": ["fixture-secret"]
        }]})).unwrap();
        let output = format_results(
            &body,
            Backend::Searxng,
            "https://search.invalid",
            8,
            Some("fixture-secret"),
        )
        .unwrap();
        assert!(!output.contains("fixture-secret"));
        let value: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["truncated"], true);
        assert_eq!(
            value["results"][0]["content"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            MAX_CONTENT_CHARS
        );
    }
}
