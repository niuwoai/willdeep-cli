//! Bounded diagnostic text shared by Provider failures and private Web summaries.
//! Never serialize an upstream event or request body to explain an error.
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

pub const MAX_ERROR_DETAIL_CHARS: usize = 1_200;
const MAX_ERROR_INPUT_CHARS: usize = 32_000;
const REDACTED: &str = "[REDACTED]";

static PRIVATE_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)-----BEGIN [^-]+-----.*?(?:-----END [^-]+-----|$)")
        .expect("valid private block pattern")
});
static CREDENTIAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?ix)(?:[\"']?\b(?:authorization|proxy-authorization|api[_-]?key|access[_-]?token|refresh[_-]?token|token|pass(?:word|wd)|secret|cookie|set-cookie)[\"']?\s*[:=]\s*)(?:\"[^\"]*(?:\"|$)|'[^']*(?:'|$)|(?:bearer|basic)\s+[^\s,;}]+|[^\s,;}]+)"#)
        .expect("valid credential pattern")
});
static ADDRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(?:https?|wss?|file)://[^\s\"'<>]+"#).expect("valid address pattern")
});
static QUOTED_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:\"(?:/|[A-Za-z]:[\\/]|\\\\)[^\"]*(?:\"|$)|'(?:/|[A-Za-z]:[\\/]|\\\\)[^']*(?:'|$))"#,
    )
    .expect("valid quoted path pattern")
});
static PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:[A-Za-z]:[\\/]|\\\\|~/|/)[^\s\"'<>),;\]}]+"#).expect("valid path pattern")
});
static PRIVATE_PAYLOAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)\b(?:command|arguments|request_body|request|prompt|messages|output|report|headers)[\"']?\s*[:=]\s*.*"#)
        .expect("valid private payload pattern")
});
static JWT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b").expect("valid JWT pattern")
});

/// This projection carries only explanatory text, with credentials, addresses,
/// local paths and explicitly labelled request/command payloads removed.
/// Structured suffixes contribute only their code/type/message scalar fields.
pub fn safe_error_detail(message: &str) -> String {
    let bounded: String = message.chars().take(MAX_ERROR_INPUT_CHARS).collect();
    let selected = if let Some(start) = bounded.find('{') {
        // HTTP failures often contain a JSON body after a status/context prefix.
        // An unparseable object is still private data, never a text fallback.
        let prefix = &bounded[..start];
        match serde_json::from_str::<Value>(&bounded[start..]) {
            Ok(value) => format!(
                "{prefix}{}",
                selected_fields(value.get("error").unwrap_or(&value))
            ),
            Err(_) => format!("{prefix}{REDACTED}"),
        }
    } else {
        bounded
    };
    let selected = PRIVATE_BLOCK.replace_all(&selected, REDACTED);
    let selected = CREDENTIAL.replace_all(&selected, REDACTED);
    let selected = ADDRESS.replace_all(&selected, "[address]");
    let selected = QUOTED_PATH.replace_all(&selected, "[path]");
    let selected = PATH.replace_all(&selected, "[path]");
    let selected = PRIVATE_PAYLOAD.replace_all(&selected, REDACTED);
    let selected = JWT.replace_all(&selected, REDACTED);
    let selected = crate::judge::redact_credentials(&selected);
    let normalized = selected
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let normalized = normalized.trim();
    let mut result = normalized
        .chars()
        .take(MAX_ERROR_DETAIL_CHARS)
        .collect::<String>();
    if normalized.chars().count() > MAX_ERROR_DETAIL_CHARS {
        result.pop();
        result.push('…');
    }
    result
}

fn selected_fields(error: &Value) -> String {
    ["code", "type", "message"]
        .into_iter()
        .filter_map(|key| error.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(": ")
}

pub(super) fn stream_error_detail(error: &Value, fallback: &str) -> String {
    let detail = safe_error_detail(&selected_fields(error));
    if detail.is_empty() {
        fallback.to_owned()
    } else {
        format!("{fallback}: {detail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_explanation_without_event_or_request_body() {
        let detail = stream_error_detail(
            &json!({
                "code": "context_length_exceeded", "type": "invalid_request_error",
                "message": "Maximum context length is 8192 tokens; received 9200",
                "request": {"messages": "private conversation"}, "debug": "private provider internals"
            }),
            "Provider stream failed",
        );
        assert!(detail.contains("context_length_exceeded"));
        assert!(detail.contains("received 9200"));
        assert!(!detail.contains("private"));
        assert_eq!(
            stream_error_detail(&json!({"message": {"request": "private"}}), "fallback"),
            "fallback"
        );
    }

    #[test]
    fn removes_json_credentials_paths_addresses_and_payloads() {
        let secret = ["test", "credential", "value"].join("-");
        let error = format!(
            r#"HTTP 401: {{"error":{{"message":"Authentication failed token={secret} at /Users/fixture/private.txt via https://example.invalid/request?token={secret}"}},"request":{{"prompt":"private prompt"}}}}"#
        );
        let detail = safe_error_detail(&error);
        assert!(detail.contains("Authentication failed"));
        for private in [
            &secret,
            "/Users/fixture",
            "example.invalid",
            "private prompt",
        ] {
            assert!(!detail.contains(private), "private field leaked");
        }
        let credential_key = ["pass", "word"].concat();
        let error = format!(
            r#"Failure: {credential_key}='{secret}' Authorization: Bearer {secret} C:\Users\fixture\private.txt command=deploy --private-argument confidential"#
        );
        let detail = safe_error_detail(&error);
        for private in [&secret, "fixture", "private-argument", "confidential"] {
            assert!(!detail.contains(private), "private field leaked");
        }
        assert!(
            !safe_error_detail(r#"cannot open "/Users/test user/private config""#)
                .contains("private config")
        );
    }

    #[test]
    fn malformed_structured_errors_and_multibyte_limits_are_safe() {
        let detail = safe_error_detail("request failed: {\"messages\":\"private unfinished");
        assert_eq!(detail, "request failed: [REDACTED]");
        let detail = safe_error_detail(&"配额不足。".repeat(MAX_ERROR_DETAIL_CHARS));
        assert_eq!(detail.chars().count(), MAX_ERROR_DETAIL_CHARS);
        assert!(detail.ends_with('…'));
        assert_eq!(
            safe_error_detail("  provider\n rejected\t model  "),
            "provider rejected model"
        );
    }
}
