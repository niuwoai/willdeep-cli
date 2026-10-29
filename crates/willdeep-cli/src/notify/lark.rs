//! 飞书自定义机器人。
//!
//! 两个容易踩的点，都不是猜的：
//!
//! 1. **签名不是对请求体签的。** 飞书的规则是把 `timestamp + "\n" + secret`
//!    整个当作 HMAC-SHA256 的 key，对空字节串签名，再 Base64。照事件订阅那套
//!    `timestamp + nonce + body` 来写会一直返回 19021。`timestamp` 是秒级，
//!    且只在前后一小时内有意义——重试必须重新签，不能复用。
//! 2. **业务失败通常仍是 HTTP 200。** 成功判定必须读 body 的 `code`（旧字段
//!    `StatusCode` 只作兼容回退），只看状态码会把「没发出去」记成「已送达」。
//!
//! 文档：<https://open.feishu.cn/document/client-docs/bot-v3/add-custom-bot>

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::Secret;

/// 飞书请求体上限是 20 KB；留一半余量，超了说明我们该改代码而不是加长消息。
const MAX_TEXT_BYTES: usize = 10 * 1024;

/// 组一条文本消息。`secret` 为空时不带签名——机器人没开签名校验时，多带两个
/// 字段反而会被当成未知字段。
pub(super) fn text_message(text: &str, secret: Option<&Secret>) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    if let Some(secret) = secret {
        let timestamp = now_unix_seconds().to_string();
        body.insert("timestamp".to_owned(), timestamp.clone().into());
        body.insert(
            "sign".to_owned(),
            signature(&timestamp, secret.expose()).into(),
        );
    }
    body.insert("msg_type".to_owned(), "text".into());
    body.insert(
        "content".to_owned(),
        serde_json::json!({ "text": clamp_text(text) }),
    );
    serde_json::Value::Object(body)
}

/// `sign = base64(HMAC-SHA256(key = "{timestamp}\n{secret}", data = ""))`。
fn signature(timestamp: &str, secret: &str) -> String {
    let key = format!("{timestamp}\n{secret}");
    // `new_from_slice` 对任何长度的 key 都成立（HMAC 会先做 key 归一化），
    // 这里的 unwrap 不是一个可能失败的输入校验，而是类型约束的必然结果。
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts a key of any length");
    mac.update(b"");
    STANDARD.encode(mac.finalize().into_bytes())
}

fn clamp_text(text: &str) -> String {
    if text.len() <= MAX_TEXT_BYTES {
        return text.to_owned();
    }
    // 按字符截，别把一个多字节字符切一半。
    let mut end = 0;
    for (index, character) in text.char_indices() {
        if index + character.len_utf8() > MAX_TEXT_BYTES {
            break;
        }
        end = index + character.len_utf8();
    }
    format!("{}…", &text[..end])
}

/// 读飞书的回复。HTTP 状态码只说明网关收没收到，业务结果在 body 里。
pub(super) fn check_response(status: reqwest::StatusCode, body: &str) -> Result<(), String> {
    if !status.is_success() {
        return Err(format!("lark returned HTTP {status}"));
    }
    let parsed: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "lark returned a non-JSON response".to_owned())?;
    let code = parsed
        .get("code")
        .and_then(serde_json::Value::as_i64)
        .or_else(|| parsed.get("StatusCode").and_then(serde_json::Value::as_i64))
        .unwrap_or(-1);
    if code == 0 {
        return Ok(());
    }
    let message = parsed
        .get("msg")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            parsed
                .get("StatusMessage")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or("unknown error");
    Err(format!(
        "lark rejected the message (code {code}): {}",
        message.chars().take(200).collect::<String>()
    ))
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 官方示例：timestamp=1599360473、secret=`SECRET` 时签名是固定的
    /// HMAC-SHA256(key="1599360473\nSECRET", data="")。
    #[test]
    fn signature_matches_the_documented_shape() {
        let timestamp = "1599360473";
        let secret = "SECRET";
        let expected = {
            let mut mac =
                Hmac::<Sha256>::new_from_slice(format!("{timestamp}\n{secret}").as_bytes())
                    .expect("key");
            mac.update(b"");
            STANDARD.encode(mac.finalize().into_bytes())
        };
        assert_eq!(signature(timestamp, secret), expected);
        // 32 字节的 HMAC 做 Base64 是 44 个字符（含一个填充符）。
        assert_eq!(signature(timestamp, secret).len(), 44);
    }

    #[test]
    fn unsigned_messages_carry_no_timestamp_or_sign() {
        let body = text_message("hello", None);
        assert!(body.get("timestamp").is_none());
        assert!(body.get("sign").is_none());
        assert_eq!(body["msg_type"], "text");
        assert_eq!(body["content"]["text"], "hello");
    }

    #[test]
    fn signed_messages_carry_both_fields_at_the_top_level() {
        let secret = Secret::new("SECRET".to_owned());
        let body = text_message("hello", Some(&secret));
        assert!(body["timestamp"].is_string());
        assert!(body["sign"].is_string());
        assert_eq!(body["msg_type"], "text");
    }

    #[test]
    fn a_business_error_in_an_http_200_body_is_a_failure() {
        let body = r#"{"code":19021,"msg":"sign match fail"}"#;
        let error = check_response(reqwest::StatusCode::OK, body).expect_err("must fail");
        assert!(error.contains("19021"));
        assert!(error.contains("sign match fail"));
    }

    #[test]
    fn success_is_code_zero_and_falls_back_to_the_legacy_status_field() {
        assert!(check_response(reqwest::StatusCode::OK, r#"{"code":0,"msg":"success"}"#).is_ok());
        assert!(
            check_response(
                reqwest::StatusCode::OK,
                r#"{"StatusCode":0,"StatusMessage":"success"}"#
            )
            .is_ok()
        );
        // 网关返回一页 HTML 时不能当成功。
        assert!(check_response(reqwest::StatusCode::OK, "<html>").is_err());
    }

    #[test]
    fn a_non_2xx_is_reported_without_the_url() {
        let error = check_response(reqwest::StatusCode::BAD_GATEWAY, "").expect_err("must fail");
        assert_eq!(error, "lark returned HTTP 502 Bad Gateway");
    }

    #[test]
    fn oversized_text_is_cut_on_a_character_boundary() {
        let text = "汉".repeat(MAX_TEXT_BYTES);
        let clamped = clamp_text(&text);
        assert!(clamped.len() <= MAX_TEXT_BYTES + "…".len());
        assert!(clamped.ends_with('…'));
        assert!(clamped.chars().all(|c| c == '汉' || c == '…'));
    }
}
