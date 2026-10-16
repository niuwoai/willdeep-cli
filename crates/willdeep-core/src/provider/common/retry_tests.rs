use super::tests::{probe_config, scripted_server, scripted_server_with_retry_after};
use super::*;
use crate::provider::stream_test_support::Events;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn five_retries_use_second_scale_backoff_and_preserve_retry_after() {
    let unavailable = ProviderError::Http {
        status: StatusCode::SERVICE_UNAVAILABLE,
        body: String::new(),
        retry_after: None,
    };
    assert_eq!(MAX_RETRIES, 5);
    assert_eq!(MAX_ATTEMPTS, 6);
    assert_eq!(
        (1..=MAX_RETRIES)
            .map(|attempt| retry_delay(&unavailable, attempt).as_secs())
            .collect::<Vec<_>>(),
        [2, 4, 8, 16, 32]
    );
    let required = ProviderError::Http {
        status: StatusCode::TOO_MANY_REQUESTS,
        body: String::new(),
        retry_after: Some(Duration::from_secs(45)),
    };
    for attempt in 1..=MAX_RETRIES {
        assert_eq!(retry_delay(&required, attempt), Duration::from_secs(45));
    }
}

#[tokio::test]
async fn exactly_five_retries_emit_every_wait_and_start_before_success() {
    let (url, hits, server) = scripted_server_with_retry_after(vec![Some(503); 5], Some("0")).await;
    let config = probe_config(&url);
    let events = Events::default();
    let response = send_open_retrying(
        client(&config).unwrap().get(&url),
        &config,
        &events,
        request_deadline(&config).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(hits.load(Ordering::SeqCst), 6);
    let events = events.0.lock().unwrap();
    assert_eq!(events.len(), 10);
    for attempt in 1..=MAX_RETRIES {
        let index = (attempt as usize - 1) * 2;
        assert!(
            matches!(events[index], super::super::ProviderEvent::RetryWait { attempt: observed, delay } if observed==attempt && delay.is_zero())
        );
        assert!(
            matches!(events[index + 1], super::super::ProviderEvent::RetryStarted { attempt: observed } if observed==attempt)
        );
    }
    server.abort();
}

#[tokio::test]
async fn default_backoff_does_not_extend_the_shared_request_deadline() {
    let (url, hits, server) = scripted_server(vec![Some(503)]).await;
    let config = probe_config(&url);
    let events = Events::default();
    let error = send_open_retrying(
        client(&config).unwrap().get(&url),
        &config,
        &events,
        tokio::time::Instant::now() + Duration::from_millis(500),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        ProviderError::RetryDeferred {
            retry_after_secs: 2,
            ..
        }
    ));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert!(events.0.lock().unwrap().is_empty());
    server.abort();
}

#[test]
fn permanent_certificate_configuration_errors_are_not_transient_tls_eofs() {
    let certificate = rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer);
    assert!(permanent_tls_failure(&certificate));
    let wrapped = std::io::Error::new(std::io::ErrorKind::InvalidData, certificate);
    assert!(permanent_tls_failure(&wrapped));
    for error in [
        rustls::Error::NoCertificatesPresented,
        rustls::Error::UnsupportedNameType,
        rustls::Error::NoApplicationProtocol,
        rustls::Error::BadMaxFragmentSize,
    ] {
        assert!(permanent_tls_failure(&error));
    }
    assert!(!permanent_tls_failure(&std::io::Error::from(
        std::io::ErrorKind::UnexpectedEof
    )));
}

#[tokio::test]
async fn a_dropped_tls_handshake_remains_retryable() {
    let (url, hits, server) = scripted_server(vec![None]).await;
    let url = url.replacen("http:", "https:", 1);
    let config = probe_config(&url);
    let error = client(&config).unwrap().get(&url).send().await.unwrap_err();
    assert!(is_retryable(&ProviderError::Request(error)));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn a_non_replayable_request_body_is_sent_only_once() {
    let (url, hits, server) = scripted_server(vec![None]).await;
    let config = probe_config(&url);
    let body = reqwest::Body::wrap_stream(futures_util::stream::once(async {
        Ok::<_, std::io::Error>(b"request-body".to_vec())
    }));
    let error = send_retrying(client(&config).unwrap().post(&url).body(body), &config)
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderError::Request(_)));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn successful_headers_with_a_broken_body_are_never_replayed() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    let server = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            let mut request = [0; 1024];
            let _ = socket.read(&mut request).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\nconnection: close\r\n\r\npartial",
                )
                .await
                .unwrap();
        }
    });
    let config = probe_config(&url);
    let error = send_retrying(client(&config).unwrap().get(&url), &config)
        .await
        .unwrap_err();
    // reqwest::Response::chunk labels truncated response bodies as Decode on
    // some versions and Body on others; both are after our scripted 200 headers.
    assert!(
        matches!(error, ProviderError::Request(ref inner) if inner.is_body() || inner.is_decode()),
        "{error:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    server.abort();
}
