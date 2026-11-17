//! Local-only raw SSE events. Public clients continue to use redacted NDJSON.
use super::*;

const MAX_EVENT_BYTES: usize = 2 * 1024 * 1024;

impl RuntimeClient {
    /// Subscribe to the existing authenticated local SSE endpoint, whose payloads
    /// include tool display details. Never proxy this stream to Web/mobile clients.
    pub async fn stream_local_events(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<LocalEventStream, ClientError> {
        let response = self
            .http
            .get(format!(
                "{}/v1/events/stream?after={after}&limit={}",
                self.base_url,
                limit.clamp(1, 1_000)
            ))
            .header(TOKEN_HEADER, &self.token)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(ClientError::HttpStatus(response.status().as_u16()));
        }
        Ok(LocalEventStream {
            chunks: Box::pin(response.bytes_stream()),
            buffer: Vec::new(),
            data: Vec::new(),
            max_event_bytes: MAX_EVENT_BYTES,
        })
    }
}

pub struct LocalEventStream {
    chunks: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: Vec<u8>,
    data: Vec<u8>,
    max_event_bytes: usize,
}

impl LocalEventStream {
    pub async fn next_event(&mut self) -> Result<Option<RuntimeEvent>, ClientError> {
        loop {
            if let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
                if newline > self.max_event_bytes {
                    return Err(ClientError::LocalEventTooLarge(self.max_event_bytes));
                }
                let line = self.buffer.drain(..=newline).collect::<Vec<_>>();
                let line = line.strip_suffix(b"\n").unwrap_or(&line);
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                if line.is_empty() && !self.data.is_empty() {
                    let data = std::mem::take(&mut self.data);
                    return serde_json::from_slice(&data)
                        .map(Some)
                        .map_err(ClientError::InvalidLocalEvent);
                }
                if let Some(data) = line.strip_prefix(b"data:") {
                    let data = data.strip_prefix(b" ").unwrap_or(data);
                    if self.data.len().saturating_add(data.len()).saturating_add(1)
                        > self.max_event_bytes
                    {
                        return Err(ClientError::LocalEventTooLarge(self.max_event_bytes));
                    }
                    self.data.extend_from_slice(data);
                    self.data.push(b'\n');
                }
                continue;
            }
            if self.buffer.len() > self.max_event_bytes {
                return Err(ClientError::LocalEventTooLarge(self.max_event_bytes));
            }
            let Some(chunk) = self.chunks.next().await else {
                return if self.buffer.is_empty() && self.data.is_empty() {
                    Ok(None)
                } else {
                    Err(ClientError::TruncatedLocalEvent)
                };
            };
            self.buffer.extend_from_slice(&chunk?);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(chunks: Vec<Bytes>, cap: usize) -> LocalEventStream {
        LocalEventStream {
            chunks: Box::pin(futures_util::stream::iter(chunks.into_iter().map(Ok))),
            buffer: Vec::new(),
            data: Vec::new(),
            max_event_bytes: cap,
        }
    }

    #[tokio::test]
    async fn split_utf8_comments_crlf_and_multiple_events_keep_raw_display_data() {
        let event = serde_json::json!({"sequence":7,"timestamp":1,"kind":"task.output",
            "message":"task {\"expansion\":\"完整命令\",\"edit_display\":{\"files\":[]}}"});
        let wire = format!(
            ": keepalive\r\n\r\nid: 7\r\nevent: task.output\r\ndata: {event}\r\n\r\ndata:{event}\n\n"
        );
        let mut input = stream(
            wire.bytes().map(|byte| Bytes::from(vec![byte])).collect(),
            MAX_EVENT_BYTES,
        );
        for _ in 0..2 {
            let event = input.next_event().await.unwrap().unwrap();
            assert_eq!(event.sequence, 7);
            assert!(event.message.contains("完整命令"));
            assert!(event.message.contains("edit_display"));
        }
        assert!(input.next_event().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn malformed_truncated_and_oversized_events_fail_without_partial_delivery() {
        for wire in ["data: {}", "data: {}\n"] {
            assert!(matches!(
                stream(vec![Bytes::from(wire)], 64).next_event().await,
                Err(ClientError::TruncatedLocalEvent)
            ));
        }
        assert!(matches!(
            stream(vec![Bytes::from("data: not json\n\n")], 64)
                .next_event()
                .await,
            Err(ClientError::InvalidLocalEvent(_))
        ));
        for wire in [
            "x".repeat(65),
            format!("data: {}\n\n", "x".repeat(65)),
            "data: 1234567890\n".repeat(8),
        ] {
            assert!(matches!(
                stream(vec![Bytes::from(wire)], 64).next_event().await,
                Err(ClientError::LocalEventTooLarge(64))
            ));
        }
    }

    #[tokio::test]
    async fn local_subscription_uses_existing_authenticated_sse_endpoint() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let mut chunk = [0u8; 1024];
                let size = socket.read(&mut chunk).await.unwrap();
                assert!(size > 0);
                request.extend_from_slice(&chunk[..size]);
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("GET /v1/events/stream?after=9&limit=1000 HTTP/1.1"));
            assert!(
                request
                    .to_lowercase()
                    .contains("x-willdeep-token: fixture-token")
            );
            let body = "data: {\"sequence\":10,\"timestamp\":1,\"kind\":\"task.output\",\"message\":\"raw detail\"}\n\n";
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        });
        let client = RuntimeClient::new(format!("http://{address}"), "fixture-token").unwrap();
        let mut events = client.stream_local_events(9, 5000).await.unwrap();
        assert_eq!(events.next_event().await.unwrap().unwrap().sequence, 10);
        assert!(events.next_event().await.unwrap().is_none());
        server.await.unwrap();
    }
}
