//! 邮件外发：一份与后端无关的 [`EmailMessage`]，两种发送方式。
//!
//! * `smtp`：交给 lettre。只启用 rustls 栈——lettre 默认特性里的 native-tls
//!   在 Linux 上是 openssl-sys，会直接毁掉 aarch64 交叉编译，macOS/Windows 上
//!   又会换成各自系统的 TLS 实现，三端行为分叉。
//! * `resend`：Resend 的 HTTP API，复用通知层已有的 reqwest。
//!
//! 再加一个后端（Mailgun、SES API……）就是多一个变体、多一个 `send`，
//! 通知层和配置校验都不用动。

use std::time::Duration;

use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use reqwest::Client;

use super::Secret;
use crate::config::{ResendChannelSettings, SmtpChannelSettings, SmtpTls};

/// 一条命令的超时。lettre 默认 60 秒，对「礼貌通知」来说太长：一个卡住的
/// SMTP 服务器会把 `willdeep run` 的退出拖到一分钟。
const SMTP_TIMEOUT: Duration = Duration::from_secs(20);
/// 错误体只留前 4 KB。代理或网关出错时可能返回一整页 HTML，读进内存再拼进
/// 日志既没用又危险。
const MAX_ERROR_BODY: usize = 4 * 1024;
/// 错误摘要里保留的字符数。
const MAX_ERROR_CHARS: usize = 200;

pub(super) struct EmailMessage {
    pub(super) from: String,
    pub(super) to: Vec<String>,
    pub(super) subject: String,
    pub(super) text: String,
    /// Resend 用它去重；SMTP 没有这个机制，字段为空。
    pub(super) idempotency_key: Option<String>,
}

pub(super) enum EmailTransport {
    Smtp(SmtpEmail),
    Resend(ResendEmail),
}

pub(super) struct SmtpEmail {
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

pub(super) struct ResendEmail {
    endpoint: String,
    api_key: Secret,
}

impl EmailTransport {
    pub(super) fn smtp(settings: &SmtpChannelSettings, channel: &str) -> Result<Self, String> {
        let host = settings.host.trim().to_owned();
        let port = settings.port.unwrap_or(match settings.tls {
            SmtpTls::Implicit => 465,
            SmtpTls::Starttls => 587,
            SmtpTls::None => 25,
        });
        let tls = match settings.tls {
            // 隐式 TLS：连上就握手。STARTTLS：先明文 EHLO，再升级。
            SmtpTls::Implicit => Tls::Wrapper(tls_parameters(&host, channel)?),
            SmtpTls::Starttls => Tls::Required(tls_parameters(&host, channel)?),
            SmtpTls::None => Tls::None,
        };
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
            .port(port)
            .tls(tls)
            .timeout(Some(SMTP_TIMEOUT));
        if let (Some(user), Some(env)) = (
            settings.username.as_deref(),
            settings.password_env.as_deref(),
        ) {
            let password = Secret::from_env(env).map_err(|error| format!("{channel}: {error}"))?;
            builder = builder.credentials(Credentials::new(
                user.to_owned(),
                password.expose().to_owned(),
            ));
        }
        Ok(Self::Smtp(SmtpEmail {
            transport: builder.build(),
        }))
    }

    pub(super) fn resend(settings: &ResendChannelSettings, channel: &str) -> Result<Self, String> {
        let api_key = Secret::from_env(&settings.api_key_env)
            .map_err(|error| format!("{channel}: {error}"))?;
        let base = settings
            .api_base
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("https://api.resend.com")
            .trim_end_matches('/')
            .to_owned();
        Ok(Self::Resend(ResendEmail {
            endpoint: format!("{base}/emails"),
            api_key,
        }))
    }

    pub(super) async fn send(&self, client: &Client, message: &EmailMessage) -> Result<(), String> {
        match self {
            Self::Smtp(smtp) => {
                let email = build_message(message)?;
                smtp.transport
                    .send(email)
                    .await
                    .map(|_| ())
                    // SMTP 的应答里可能有换行；进日志前压平，别让它伪造一行新日志。
                    .map_err(|error| format!("smtp: {}", flatten(&error.to_string())))
            }
            Self::Resend(resend) => resend.send(client, message).await,
        }
    }

    /// 配置里的后端名字，只用于诊断输出。
    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::Smtp(_) => "smtp",
            Self::Resend(_) => "resend",
        }
    }
}

fn tls_parameters(host: &str, channel: &str) -> Result<TlsParameters, String> {
    TlsParameters::new(host.to_owned())
        .map_err(|error| format!("{channel}: cannot build TLS parameters for {host}: {error}"))
}

fn build_message(message: &EmailMessage) -> Result<Message, String> {
    let from = message
        .from
        .parse::<Mailbox>()
        .map_err(|_| "invalid from address".to_owned())?;
    let mut builder = Message::builder()
        .from(from)
        .subject(flatten(&message.subject));
    for recipient in &message.to {
        let mailbox = recipient
            .parse::<Mailbox>()
            .map_err(|_| "invalid recipient address".to_owned())?;
        builder = builder.to(mailbox);
    }
    builder
        .header(ContentType::TEXT_PLAIN)
        .body(message.text.clone())
        .map_err(|error| format!("cannot build the message: {error}"))
}

impl ResendEmail {
    async fn send(&self, client: &Client, message: &EmailMessage) -> Result<(), String> {
        let mut request = client
            .post(&self.endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key.expose()))
            .json(&serde_json::json!({
                "from": message.from,
                "to": message.to,
                "subject": message.subject,
                "text": message.text,
            }));
        if let Some(key) = message.idempotency_key.as_deref() {
            request = request.header("Idempotency-Key", key);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("resend request failed: {}", error.without_url()))?;
        let status = response.status();
        let body = super::channels::read_body_limited(response, MAX_ERROR_BODY).await;
        if !status.is_success() {
            return Err(format!(
                "resend returned HTTP {status}: {}",
                describe_error(&body)
            ));
        }
        // 202 也是「排队成功」，但只有带 id 才算真的收下。
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        if parsed
            .get("id")
            .and_then(serde_json::Value::as_str)
            .is_none()
        {
            return Err(format!(
                "resend accepted the request but returned no message id: {}",
                describe_error(&body)
            ));
        }
        Ok(())
    }
}

/// Resend 的错误体是 `{statusCode, name, message}`。只取 name/message，别把整段
/// JSON（可能含收件人）原样写进日志。
fn describe_error(body: &str) -> String {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) else {
        return flatten(&body.chars().take(MAX_ERROR_CHARS).collect::<String>());
    };
    let name = parsed
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("error");
    let message = parsed
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("no detail");
    flatten(&format!("{name}: {message}"))
}

fn flatten(value: &str) -> String {
    let flattened = value.split_whitespace().collect::<Vec<_>>().join(" ");
    flattened.chars().take(MAX_ERROR_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn message() -> EmailMessage {
        EmailMessage {
            from: "WillDeep <bot@example.com>".to_owned(),
            to: vec!["ops@example.com".to_owned()],
            subject: "任务完成 · 整理发布说明".to_owned(),
            text: "任务完成\n\n状态: 成功\n".to_owned(),
            idempotency_key: Some("willdeep-runtime-1-2026".to_owned()),
        }
    }

    #[test]
    fn non_ascii_headers_and_bodies_survive_message_building() {
        let built = build_message(&message()).expect("build");
        let formatted = built.formatted();
        let text = String::from_utf8_lossy(&formatted);
        assert!(text.contains("Subject:"));
        // 非 ASCII 主题必须走 RFC 2047 编码，不能原样塞进 header。
        assert!(!text.contains("任务完成"));
        assert!(text.contains("=?utf-8?"));
        assert!(text.contains("Content-Type: text/plain"));
    }

    #[test]
    fn a_header_injection_attempt_is_flattened() {
        let mut mail = message();
        mail.subject = "hello\r\nBcc: evil@example.com".to_owned();
        let built = build_message(&mail).expect("build");
        let formatted = String::from_utf8_lossy(&built.formatted()).into_owned();
        // 换行被压成空格：Bcc 只会留在 Subject 的值里，不会变成独立的一行 header。
        assert!(formatted.contains("Subject: hello Bcc: evil@example.com"));
        assert!(!formatted.lines().any(|line| line.starts_with("Bcc:")));
    }

    #[test]
    fn invalid_addresses_fail_without_echoing_them() {
        let mut mail = message();
        mail.to = vec!["not an address".to_owned()];
        let error = build_message(&mail).expect_err("must fail");
        assert_eq!(error, "invalid recipient address");
        assert!(!error.contains("not an address"));
    }

    /// 一个回环 HTTP 服务器，记录收到的请求并返回固定响应。
    async fn capture_resend(
        status: u16,
        response_body: &'static str,
    ) -> (
        String,
        Arc<Mutex<Vec<serde_json::Value>>>,
        Arc<Mutex<Vec<String>>>,
    ) {
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let keys = Arc::new(Mutex::new(Vec::new()));
        let sink = bodies.clone();
        let key_sink = keys.clone();
        let app = axum::Router::new().route(
            "/emails",
            axum::routing::post(
                move |headers: axum::http::HeaderMap,
                      axum::Json(body): axum::Json<serde_json::Value>| {
                    let sink = sink.clone();
                    let key_sink = key_sink.clone();
                    async move {
                        if let Some(key) =
                            headers.get("idempotency-key").and_then(|v| v.to_str().ok())
                            && let Ok(mut keys) = key_sink.lock()
                        {
                            keys.push(key.to_owned());
                        }
                        if let Ok(mut sink) = sink.lock() {
                            sink.push(body.clone());
                        }
                        (
                            axum::http::StatusCode::from_u16(status).expect("status"),
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            response_body,
                        )
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://127.0.0.1:{port}"), bodies, keys)
    }

    fn resend_settings(api_base: &str, api_key_env: &str) -> ResendChannelSettings {
        ResendChannelSettings {
            api_key_env: api_key_env.to_owned(),
            api_base: Some(api_base.to_owned()),
        }
    }

    #[tokio::test]
    async fn resend_posts_the_documented_body_with_an_idempotency_key() {
        // 环境变量在测试进程里是全局的；每个用例用自己的变量名，并行跑也不打架。
        // SAFETY: 这个变量只被本用例读写。
        unsafe { std::env::set_var("WILLDEEP_TEST_RESEND_KEY_BODY", "re_test_key") };
        let (base, bodies, keys) =
            capture_resend(200, r#"{"id":"49a3999c-0ce1-4ea6-ab68-afcd6dc2e794"}"#).await;
        let transport = EmailTransport::resend(
            &resend_settings(&base, "WILLDEEP_TEST_RESEND_KEY_BODY"),
            "email",
        )
        .expect("build");
        let client = Client::builder().no_proxy().build().expect("client");
        transport.send(&client, &message()).await.expect("send");

        let bodies = bodies.lock().expect("lock").clone();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["from"], "WillDeep <bot@example.com>");
        assert_eq!(bodies[0]["to"][0], "ops@example.com");
        assert_eq!(bodies[0]["subject"], "任务完成 · 整理发布说明");
        assert!(bodies[0]["text"].as_str().expect("text").contains("状态"));
        assert_eq!(
            keys.lock().expect("lock").first().map(String::as_str),
            Some("willdeep-runtime-1-2026")
        );
        unsafe { std::env::remove_var("WILLDEEP_TEST_RESEND_KEY_BODY") };
    }

    #[tokio::test]
    async fn resend_reports_a_business_error_without_the_api_key() {
        // SAFETY: 这个变量只被本用例读写。
        unsafe { std::env::set_var("WILLDEEP_TEST_RESEND_KEY_ERROR", "re_test_key") };
        let (base, _, _) = capture_resend(
            403,
            r#"{"statusCode":403,"name":"validation_error","message":"The example.com domain is not verified."}"#,
        )
        .await;
        let transport = EmailTransport::resend(
            &resend_settings(&base, "WILLDEEP_TEST_RESEND_KEY_ERROR"),
            "email",
        )
        .expect("build");
        let client = Client::builder().no_proxy().build().expect("client");
        let error = transport
            .send(&client, &message())
            .await
            .expect_err("must fail");
        assert!(error.contains("403"));
        assert!(error.contains("validation_error"));
        assert!(!error.contains("re_test_key"));
        unsafe { std::env::remove_var("WILLDEEP_TEST_RESEND_KEY_ERROR") };
    }

    #[tokio::test]
    async fn resend_treats_a_200_without_an_id_as_a_failure() {
        // SAFETY: 这个变量只被本用例读写。
        unsafe { std::env::set_var("WILLDEEP_TEST_RESEND_KEY_NO_ID", "re_test_key") };
        let (base, _, _) = capture_resend(200, r#"{"ok":true}"#).await;
        let transport = EmailTransport::resend(
            &resend_settings(&base, "WILLDEEP_TEST_RESEND_KEY_NO_ID"),
            "email",
        )
        .expect("build");
        let client = Client::builder().no_proxy().build().expect("client");
        let error = transport
            .send(&client, &message())
            .await
            .expect_err("must fail");
        assert!(error.contains("no message id"));
        unsafe { std::env::remove_var("WILLDEEP_TEST_RESEND_KEY_NO_ID") };
    }

    #[test]
    fn a_missing_secret_names_the_variable_but_not_a_value() {
        let settings = ResendChannelSettings {
            api_key_env: "WILLDEEP_DEFINITELY_UNSET_KEY".to_owned(),
            api_base: None,
        };
        let error = match EmailTransport::resend(&settings, "ops-mail") {
            Ok(_) => panic!("must fail"),
            Err(error) => error,
        };
        assert!(error.contains("WILLDEEP_DEFINITELY_UNSET_KEY"));
        assert!(error.starts_with("ops-mail:"));
    }
}
