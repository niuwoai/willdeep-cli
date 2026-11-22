use crate::provider::ProviderError;
use std::time::Duration;

pub(super) const MAX_RECOVERIES: usize = 2;
pub(super) const INSTRUCTION: &str = "[stream-recovery] The previous model response was interrupted. Continue the current task from the saved conversation and tool results. Its partial text is unfinished, not proof of completion. No tool calls from the interrupted response were executed. Do not repeat completed operations; inspect saved results and reconcile uncertain effects before retrying any operation.";

pub(super) fn delay(attempt: usize) -> Duration {
    Duration::from_secs(1 << attempt.saturating_sub(1).min(1))
}

pub(super) fn can_recover(error: &ProviderError) -> bool {
    let ProviderError::StreamInterrupted { source, .. } = error else {
        return false;
    };
    match source.as_ref() {
        ProviderError::Request(error) => {
            error.is_timeout() || error.is_body() || error.is_connect()
        }
        ProviderError::Http { status, .. } => status.is_server_error() || status.as_u16() == 429,
        ProviderError::InvalidResponse(message) => {
            let lower = message.to_ascii_lowercase();
            lower == "sse ended inside an event"
                || lower == "model stream ended without a completion event"
                || lower.contains("connection lost mid-reply")
                || lower.contains("connection reset by peer")
                || lower.contains("unexpected eof")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Completion;

    fn interrupted(source: ProviderError) -> ProviderError {
        ProviderError::StreamInterrupted {
            source: Box::new(source),
            partial: Box::new(Completion {
                reasoning: None,
                content: String::new(),
                tool_calls: Vec::new(),
                usage: None,
                finish_reason: Some("incomplete".into()),
            }),
        }
    }

    #[test]
    fn stream_recovery_classifies_transport_failures_without_retrying_permanent_errors() {
        for message in [
            "SSE ended inside an event",
            "model stream ended without a completion event",
            "provider emitted a stream error: api_error: Codex: connection lost mid-reply (unexpected EOF)",
        ] {
            assert!(can_recover(&interrupted(ProviderError::InvalidResponse(
                message.into()
            ))));
        }
        for source in [
            ProviderError::MissingApiKey,
            ProviderError::EmptyResponse,
            ProviderError::DeadlineExceeded,
            ProviderError::InvalidResponse("invalid API key".into()),
            ProviderError::InvalidResponse("SSE event type mismatch".into()),
            ProviderError::Http {
                status: reqwest::StatusCode::UNAUTHORIZED,
                body: "unauthorized".into(),
                retry_after: None,
            },
        ] {
            assert!(!can_recover(&interrupted(source)));
        }
        assert!(!can_recover(&ProviderError::InvalidResponse(
            "unexpected EOF".into()
        )));
    }
}
