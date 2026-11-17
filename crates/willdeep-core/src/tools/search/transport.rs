use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use futures_util::StreamExt;

use super::{Backend, Endpoint, MAX_RESPONSE_BYTES, failure};
use crate::tools::ToolError;

/// All work lives in this future: dropping it cancels DNS, HTTP and body reading.
/// There is no detached task and neither application nor client retries.
pub(super) async fn request(
    endpoint: &Endpoint<'_>,
    query: &str,
    count: usize,
    timeout: Duration,
) -> Result<Vec<u8>, ToolError> {
    tokio::time::timeout(timeout, exchange(endpoint, query, count, timeout))
        .await
        .map_err(|_| failure("timeout", "search request exceeded its deadline"))?
}

async fn exchange(
    endpoint: &Endpoint<'_>,
    query: &str,
    count: usize,
    timeout: Duration,
) -> Result<Vec<u8>, ToolError> {
    let host = endpoint
        .url
        .host_str()
        .ok_or_else(|| failure("invalid_endpoint", "search endpoint has no host"))?
        .trim_start_matches('[')
        .trim_end_matches(']');
    let port = endpoint
        .url
        .port_or_known_default()
        .ok_or_else(|| failure("invalid_endpoint", "search endpoint has no port"))?;
    let addresses: Vec<SocketAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| failure("dns", "cannot resolve the configured search endpoint"))?
            .collect()
    };
    validate_addresses(&addresses, endpoint.allow_private, endpoint.url.scheme())?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        // The URL hostname remains intact for Host/SNI/certificate validation;
        // the transport can use only this already-validated set of addresses.
        .resolve_to_addrs(host, &addresses)
        .timeout(timeout)
        .connect_timeout(timeout)
        .build()
        .map_err(|_| failure("transport", "cannot construct the search HTTP client"))?;
    let request = match endpoint.backend {
        Backend::Searxng => client
            .get(endpoint.url.clone())
            .query(&[("q", query), ("format", "json")]),
        Backend::SomeIm => client.post(endpoint.url.clone()).json(&serde_json::json!({
            "query": query,
            "count": count,
            "provider": "auto",
        })),
    };
    let request = if let Some(key) = endpoint.key {
        let mut authorization = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| {
                failure(
                    "invalid_credential",
                    "search credential is not a valid HTTP header",
                )
            })?;
        authorization.set_sensitive(true);
        request.header(reqwest::header::AUTHORIZATION, authorization)
    } else {
        request
    };
    let response = request
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(transport_error)?;
    let status = response.status();
    // Do not read an error body: it may contain a reflected credential or query.
    if !status.is_success() {
        let (code, message) = match status.as_u16() {
            401 => (
                "authentication",
                "search endpoint rejected authentication (HTTP 401)",
            ),
            403 => ("forbidden", "search endpoint refused access (HTTP 403)"),
            429 => (
                "rate_limited",
                "search endpoint rate limit reached (HTTP 429); no retry was made",
            ),
            _ if status.is_redirection() => (
                "redirect",
                "search endpoint returned a redirect; it was not followed",
            ),
            _ => {
                return Err(failure(
                    "http",
                    &format!("search endpoint returned HTTP {}", status.as_u16()),
                ));
            }
        };
        return Err(failure(code, message));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err(failure(
            "response_limit",
            "search response exceeds the 1 MiB limit",
        ));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(transport_error)?;
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(failure(
                "response_limit",
                "search response exceeds the 1 MiB limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn transport_error(error: reqwest::Error) -> ToolError {
    if error.is_timeout() {
        failure("timeout", "search request exceeded its deadline")
    } else {
        // reqwest::Error may carry the full URL. Never format it or its source.
        failure("transport", "search HTTP exchange failed")
    }
}

fn validate_addresses(
    addresses: &[SocketAddr],
    allow_private: bool,
    scheme: &str,
) -> Result<(), ToolError> {
    if addresses.is_empty() {
        return Err(failure("dns", "search endpoint resolved to no addresses"));
    }
    for address in addresses {
        let ip = match address.ip() {
            IpAddr::V6(value) => value
                .to_ipv4_mapped()
                .map(IpAddr::V4)
                .unwrap_or(IpAddr::V6(value)),
            other => other,
        };
        if ip.is_unspecified()
            || ip.is_multicast()
            || matches!(ip, IpAddr::V4(value) if value.is_broadcast())
        {
            return Err(failure(
                "unsafe_endpoint",
                "search endpoint is not a unicast address",
            ));
        }
        let public = is_public(ip);
        if !public && !allow_private {
            return Err(failure(
                "private_endpoint",
                "private search endpoints require explicit allow_private_endpoint=true",
            ));
        }
        if public && scheme != "https" {
            return Err(failure(
                "https_required",
                "public search endpoints require HTTPS",
            ));
        }
    }
    Ok(())
}

fn is_public(ip: IpAddr) -> bool {
    if !super::super::web::is_public_ip(ip) {
        return false;
    }
    match ip {
        IpAddr::V4(value) => {
            let octets = value.octets();
            // Benchmark networks are not Internet-routable.
            !(octets[0] == 198 && matches!(octets[1], 18 | 19))
        }
        // Require global unicast; reject IPv4-compatible, NAT64, 6to4,
        // Teredo and documentation ranges rather than accepting tunnel aliases.
        IpAddr::V6(value) => {
            let segments = value.segments();
            (segments[0] & 0xe000) == 0x2000
                && segments[0] != 0x2002
                && !(segments[0] == 0x2001 && segments[1] < 0x0200)
                && !(segments[0] == 0x3fff && (segments[1] & 0xf000) == 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_validation_requires_opt_in_and_public_https() {
        let local = ["127.0.0.1:80".parse().unwrap()];
        assert!(validate_addresses(&local, false, "https").is_err());
        assert!(validate_addresses(&local, true, "http").is_ok());
        let public = ["8.8.8.8:80".parse().unwrap()];
        assert!(validate_addresses(&public, true, "http").is_err());
        assert!(validate_addresses(&public, false, "https").is_ok());
        let mixed = [public[0], local[0]];
        assert!(validate_addresses(&mixed, false, "https").is_err());
        assert!(
            validate_addresses(&["[::ffff:127.0.0.1]:80".parse().unwrap()], false, "https")
                .is_err()
        );
        assert!(validate_addresses(&["198.18.0.1:80".parse().unwrap()], false, "https").is_err());
    }

    #[tokio::test]
    async fn timeout_during_body_read_is_classified() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Endpoint {
            backend: Backend::Searxng,
            url: reqwest::Url::parse(&format!("http://{}/search", listener.local_addr().unwrap()))
                .unwrap(),
            key: None,
            allow_private: true,
        };
        let server = async {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut buffer = [0; 4096];
            assert!(
                stream.read(&mut buffer).await.unwrap() > 0,
                "the client must send request bytes before the partial response"
            );
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\n\r\n{")
                .await
                .unwrap();
            let mut buffer = [0; 1];
            // The request's timeout must close its socket, not leave a task running.
            tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
                .await
                .unwrap()
        };
        let (closed, result) = tokio::join!(
            server,
            request(&endpoint, "fixture", 8, Duration::from_millis(100))
        );
        match closed {
            Ok(count) => assert_eq!(count, 0),
            Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset),
        }
        assert!(result.unwrap_err().to_string().contains("search[timeout]"));
    }
}
