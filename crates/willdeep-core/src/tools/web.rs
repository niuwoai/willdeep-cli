//! web_fetch 与 web_search 的边界：公网判定、重定向、响应体上限和 HTML 转文本。
//!
//! 与工具派发分开放，是为了让 SSRF 规则只有一处。TUI 里用户点击下载媒体
//! 也走 [`validate_public_url`]，不能和代理的 web 工具各自判一遍。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use futures_util::StreamExt;

use super::ToolError;

pub(super) const MAX_WEB_RESPONSE_BYTES: usize = 3 * 1024 * 1024;
pub(super) const MAX_WEB_REQUEST_BYTES: usize = 1024 * 1024;
pub(super) const DEFAULT_WEB_POST_CONTENT_TYPE: &str = "application/json";
pub(super) const DEFAULT_WEB_MAX_CHARS: usize = 20_000;
pub(super) const MAX_WEB_MAX_CHARS: usize = 100_000;
pub(super) const MAX_WEB_REDIRECTS: usize = 8;

pub(super) fn web_client() -> Result<reqwest::Client, ToolError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| ToolError::Network(error.to_string()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WebMethod {
    Get,
    Post,
}

pub(super) fn parse_web_method(raw: Option<&str>) -> Result<WebMethod, ToolError> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(WebMethod::Get),
        Some(value) if value.eq_ignore_ascii_case("get") => Ok(WebMethod::Get),
        Some(value) if value.eq_ignore_ascii_case("post") => Ok(WebMethod::Post),
        Some(value) => Err(ToolError::Network(format!(
            "unsupported HTTP method: {value}; web_fetch supports GET and POST"
        ))),
    }
}

/// 「主域名」按公共后缀表取：`api.example.com` 和 `upload.example.com` 归到同
/// 一条 `example.com` 规则，而 `example.co.uk` 整体就是一个注册域名。机械地取
/// 后两段会把它截成 `co.uk`，那条规则等于放行整个英国二级域。IP 直连没有域名
/// 可归并，按字面量各自成规则。
pub(super) fn registrable_domain(url: &reqwest::Url) -> String {
    let Some(host) = url.host_str() else {
        return url.as_str().to_ascii_lowercase();
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.parse::<IpAddr>().is_ok() {
        return host.to_ascii_lowercase();
    }
    psl::domain_str(host).unwrap_or(host).to_ascii_lowercase()
}

pub(super) fn same_hostname(left: &reqwest::Url, right: &reqwest::Url) -> bool {
    left.host_str()
        .zip(right.host_str())
        .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
}

pub(super) fn redirect_key(url: &reqwest::Url) -> String {
    let mut normalized = url.clone();
    normalized.set_fragment(None);
    normalized.to_string()
}

pub(super) async fn read_web_response(response: reqwest::Response) -> Result<Vec<u8>, ToolError> {
    let mut output = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| ToolError::Network(error.to_string()))?;
        append_web_chunk(&mut output, &chunk)?;
    }
    Ok(output)
}

pub(super) fn append_web_chunk(output: &mut Vec<u8>, chunk: &[u8]) -> Result<(), ToolError> {
    if output.len().saturating_add(chunk.len()) > MAX_WEB_RESPONSE_BYTES {
        return Err(ToolError::Network(
            "response exceeds the 3 MiB limit".to_owned(),
        ));
    }
    output.extend_from_slice(chunk);
    Ok(())
}

/// Validate that an HTTP(S) URL resolves exclusively to public addresses.
///
/// Kept as a shared boundary for both agent web tools and user-triggered TUI
/// media downloads so SSRF rules cannot drift between the two call sites.
pub async fn validate_public_url(url: &reqwest::Url) -> Result<(), ToolError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ToolError::Network(
            "only HTTP(S) URLs are supported".to_owned(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ToolError::Network("URL has no host".to_owned()))?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(ToolError::Network(
            "loopback targets are refused".to_owned(),
        ));
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| ToolError::Network("URL has no usable port".to_owned()))?;
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| ToolError::Network(format!("cannot resolve host: {error}")))?;
    let mut found = false;
    for address in addresses {
        found = true;
        if !is_public_ip(address.ip()) {
            return Err(ToolError::Network(format!(
                "private, loopback, or link-local target is refused: {}",
                address.ip()
            )));
        }
    }
    if !found {
        return Err(ToolError::Network(
            "host resolved to no addresses".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(value) => is_public_ipv4(value),
        IpAddr::V6(value) => is_public_ipv6(value),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || octets[0] == 0
        || octets[0] >= 224
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113))
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8))
}

pub(super) fn html_to_text(html: &str) -> String {
    let scripts = regex::Regex::new(
        r"(?is)<script[^>]*>.*?</script>|<style[^>]*>.*?</style>|<noscript[^>]*>.*?</noscript>",
    )
    .expect("valid HTML cleanup regex")
    .replace_all(html, " ");
    let breaks = regex::Regex::new(r"(?i)</?(p|div|br|li|h[1-6]|tr|section|article)[^>]*>")
        .expect("valid HTML block regex")
        .replace_all(&scripts, "\n");
    let tags = regex::Regex::new(r"(?s)<[^>]+>")
        .expect("valid HTML tag regex")
        .replace_all(&breaks, " ");
    let decoded = tags
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let whitespace = regex::Regex::new(r"[ \t\r\x0B\x0C]+")
        .expect("valid whitespace regex")
        .replace_all(&decoded, " ");
    regex::Regex::new(r"\n\s*\n+")
        .expect("valid blank line regex")
        .replace_all(&whitespace, "\n\n")
        .trim()
        .to_owned()
}

pub(super) fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    format!(
        "{}\n[content truncated]",
        value.chars().take(max_chars).collect::<String>()
    )
}

pub(super) fn format_search_results(body: &str) -> Result<String, ToolError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| ToolError::Network(format!("invalid search response: {error}")))?;
    let data = value.get("data").unwrap_or(&value);
    let items = data
        .get("results")
        .or_else(|| data.get("items"))
        .or_else(|| data.as_array().map(|_| data))
        .and_then(serde_json::Value::as_array);
    let Some(items) = items else {
        return serde_json::to_string_pretty(data)
            .map_err(|error| ToolError::Network(error.to_string()));
    };
    let lines = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let title = item
                .get("title")
                .and_then(|value| value.as_str())
                .unwrap_or("Untitled");
            let url = item
                .get("url")
                .or_else(|| item.get("link"))
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let snippet = item
                .get("snippet")
                .or_else(|| item.get("content"))
                .or_else(|| item.get("description"))
                .and_then(|value| value.as_str())
                .unwrap_or("");
            format!(
                "{}. {title}\n{url}\n{}",
                index + 1,
                super::truncate_line(snippet, 1_000)
            )
        })
        .collect::<Vec<_>>();
    Ok(if lines.is_empty() {
        "No search results.".to_owned()
    } else {
        lines.join("\n\n")
    })
}
