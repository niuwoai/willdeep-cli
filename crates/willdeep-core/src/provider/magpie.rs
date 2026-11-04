//! Security boundary for externally managed, loopback-only Magpie connections.
use super::ProviderError;
use reqwest::Url;

pub const CLI_MARKER: &str = "magpie-willdeep-cli";
pub const MAX_MODEL_BYTES: usize = 1024;

pub fn validate_model(model: &str) -> bool {
    !model.trim().is_empty()
        && model.len() <= MAX_MODEL_BYTES
        && !model
            .chars()
            .any(|c| matches!(c as u32, 0..=31 | 127..=159))
}

/// Validate the original spelling before URL normalization can turn shorthand
/// IPv4, escaped hosts, credentials, or dot segments into an accepted endpoint.
pub fn integration_url(value: &str) -> Result<Url, ProviderError> {
    let invalid = || ProviderError::InvalidBaseUrl("invalid Magpie loopback endpoint".into());
    let authority = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))
        .and_then(|rest| rest.strip_suffix("/v1"))
        .ok_or_else(invalid)?;
    let (host, port) = if let Some(rest) = authority.strip_prefix("[::1]") {
        ("[::1]", rest.strip_prefix(':'))
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let valid_host = host == "localhost" || host == "[::1]" || {
        let octets: Vec<_> = host.split('.').collect();
        octets.len() == 4
            && octets[0] == "127"
            && octets
                .iter()
                .all(|octet| octet.parse::<u8>().is_ok_and(|n| n.to_string() == *octet))
    };
    if !valid_host || (host == "[::1]" && authority != host && !authority.starts_with("[::1]:")) {
        return Err(invalid());
    }
    if let Some(port) = port
        && (port.is_empty()
            || port.len() > 5
            || !port.bytes().all(|c| c.is_ascii_digit())
            || !port.parse::<u16>().is_ok_and(|p| p > 0))
    {
        return Err(invalid());
    }
    let url = Url::parse(value).map_err(|_| invalid())?;
    if url.path() != "/v1"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid());
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn integration_transport_child() {
        let Ok(base) = std::env::var("WILLDEEP_MAGPIE_TEST_URL") else {
            return;
        };
        let mut config = super::super::ProviderConfig::new(
            super::super::ProviderKind::OpenAiCompatible,
            super::super::ApiDialect::ChatCompletions,
            base,
            CLI_MARKER,
            "provider/model",
        );
        config.magpie_integration = true;
        config.request_timeout_secs = 2;
        config.allow_request_replay = false;
        let result = super::super::list_models(&config).await;
        if std::env::var("WILLDEEP_MAGPIE_TEST_REDIRECT").is_ok() {
            assert!(
                matches!(result, Err(super::super::ProviderError::Http { status, .. }) if status.as_u16() == 302)
            );
        } else {
            assert_eq!(result.unwrap(), vec!["provider/model"]);
        }
        config.base_url = "http://example.com/v1".into();
        assert!(super::super::list_models(&config).await.is_err());
    }

    #[tokio::test]
    async fn integration_transport_has_no_proxy_or_redirects_and_pins_localhost() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        for redirect in [false, true] {
            let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let forbidden = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let upstream_port = upstream.local_addr().unwrap().port();
            let forbidden_url = format!("http://{}/v1/models", forbidden.local_addr().unwrap());
            let proxy = format!("http://{}", forbidden.local_addr().unwrap());
            let child = tokio::task::spawn_blocking(move || {
                let mut command = std::process::Command::new(std::env::current_exe().unwrap());
                command
                    .env_clear()
                    .args([
                        "--exact",
                        "provider::magpie::tests::integration_transport_child",
                        "--nocapture",
                    ])
                    .env(
                        "WILLDEEP_MAGPIE_TEST_URL",
                        format!("http://localhost:{upstream_port}/v1"),
                    );
                for name in [
                    "HTTP_PROXY",
                    "http_proxy",
                    "HTTPS_PROXY",
                    "https_proxy",
                    "ALL_PROXY",
                    "all_proxy",
                ] {
                    command.env(name, &proxy);
                }
                command.env("NO_PROXY", "").env("no_proxy", "");
                if redirect {
                    command.env("WILLDEEP_MAGPIE_TEST_REDIRECT", "1");
                }
                command.output().unwrap()
            });
            let (mut stream, _) =
                tokio::time::timeout(std::time::Duration::from_secs(10), upstream.accept())
                    .await
                    .unwrap()
                    .unwrap();
            let mut request = vec![0; 8192];
            let n = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..n]).to_ascii_lowercase();
            assert!(request.contains("get /v1/models"));
            assert!(request.contains(&format!("authorization: bearer {CLI_MARKER}")));
            let response = if redirect {
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: {forbidden_url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
            } else {
                let body = r#"{"data":[{"id":"provider/model"}]}"#;
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            stream.write_all(response.as_bytes()).await.unwrap();
            drop(stream);
            let output = child.await.unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(50), forbidden.accept())
                    .await
                    .is_err()
            );
        }
    }

    #[test]
    fn accepts_only_literal_loopback_grammar() {
        for value in [
            "http://localhost/v1",
            "https://[::1]:65535/v1",
            "http://127.255.0.1:1/v1",
        ] {
            assert!(integration_url(value).is_ok(), "{value}");
        }
        for value in [
            "http://127.1/v1",
            "http://127.00.0.1/v1",
            "http://2130706433/v1",
            "http://127.0.0.1:0/v1",
            "http://[::1]evil/v1",
            "HTTP://localhost/v1",
            "http://localhost/v1/",
            "http://localhost/v1?",
            "http://localhost/v1#",
            "http://user@localhost/v1",
            "http://localhost/a/../v1",
            "http://127.0.0.256/v1",
            "http://localhost:/v1",
            "http://localhost:65536/v1",
            "http://localhost\\evil/v1",
        ] {
            assert!(integration_url(value).is_err(), "{value}");
        }
    }
}
