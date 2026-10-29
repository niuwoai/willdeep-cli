//! 出站渠道：一次事件按每条渠道自己的过滤规则并发投递。
//!
//! 投递是「礼貌通知」，不是关键路径：每条渠道在自己的 detached 任务里跑，
//! 失败只记一条错误，绝不把轮次拖失败。渠道之间也互不等待——一条 SMTP 卡住，
//! 不该让飞书那条晚到。

use std::sync::Arc;

use hmac::{Hmac, Mac};
use reqwest::Client;
use sha2::Sha256;

use super::email::{EmailMessage, EmailTransport};
use super::render;
use super::{APP_VERSION, EXECUTOR_KIND, NotificationEvent, SCHEMA_VERSION, SOURCE, Secret};
use super::{WebhookPayload, lark};
use crate::config::{NotificationChannel, NotificationEventKind, NotificationSettings};
use crate::i18n::Language;

/// 飞书、Resend 的响应体都不该很大；读超过这个数说明对面不是我们认识的接口。
const MAX_RESPONSE_BODY: usize = 8 * 1024;

/// 一条配置好的渠道。构造时就把密钥解析好——投递时再去读环境变量，等于把
/// 「配置错误」推迟到用户最不想要通知的那一刻才暴露。
pub(super) enum Channel {
    Webhook(WebhookChannel),
    Lark(LarkChannel),
    Email(EmailChannel),
}

pub(super) struct WebhookChannel {
    name: String,
    url: String,
    secret: Option<Secret>,
    events: EventMask,
}

pub(super) struct LarkChannel {
    name: String,
    webhook_url: String,
    secret: Option<Secret>,
    events: EventMask,
}

pub(super) struct EmailChannel {
    name: String,
    from: String,
    to: Vec<String>,
    events: EventMask,
    transport: EmailTransport,
}

/// 哪些事件要发到这条渠道。缺省是全部。
#[derive(Clone, Copy, Debug)]
pub(super) struct EventMask {
    task_completed: bool,
    attention_required: bool,
}

impl EventMask {
    pub(super) fn from_config(events: &[NotificationEventKind]) -> Self {
        Self {
            task_completed: events.contains(&NotificationEventKind::TaskCompleted),
            attention_required: events.contains(&NotificationEventKind::AttentionRequired),
        }
    }

    pub(super) fn wants(self, event: NotificationEvent) -> bool {
        match event {
            NotificationEvent::TaskCompleted => self.task_completed,
            NotificationEvent::AttentionRequired => self.attention_required,
        }
    }

    pub(super) fn is_empty(self) -> bool {
        !self.task_completed && !self.attention_required
    }
}

/// 把配置变成可投递的渠道。单条渠道坏掉（URL 空、密钥环境变量没设、邮件
/// provider 没选）只跳过这一条，并把原因带回给调用方——一条坏渠道不该让
/// 其余渠道一起沉默，更不该拦住 `willdeep run` 启动。
pub(super) fn build(settings: &NotificationSettings) -> (Vec<Arc<Channel>>, Vec<String>) {
    let mut channels = Vec::new();
    let mut errors = Vec::new();

    // 老的 `[notifications] webhook_*` 继续有效：那份配置与桌面端共用，
    // 不能要求用户为了多一条渠道先把老的删掉。
    if settings.webhook_enabled.unwrap_or(false) {
        let events = EventMask {
            task_completed: settings.webhook_on_task_completed.unwrap_or(true),
            attention_required: settings.webhook_on_attention_required.unwrap_or(true),
        };
        match settings
            .webhook_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(url) if !events.is_empty() => {
                channels.push(Arc::new(Channel::Webhook(WebhookChannel {
                    name: "webhook".to_owned(),
                    url: url.to_owned(),
                    secret: None,
                    events,
                })));
            }
            Some(_) => errors.push(
                "webhook: both webhook_on_task_completed and webhook_on_attention_required are false"
                    .to_owned(),
            ),
            None => {
                errors.push("webhook: notifications.webhook_url is empty".to_owned());
            }
        }
    }

    for (index, config) in settings.channels.iter().enumerate() {
        match channel_from_config(config, index + 1) {
            Ok(channel) => channels.push(Arc::new(channel)),
            Err(error) => errors.push(error),
        }
    }

    (channels, errors)
}

fn channel_from_config(config: &NotificationChannel, index: usize) -> Result<Channel, String> {
    match config {
        NotificationChannel::Webhook {
            name,
            url,
            secret_env,
            events,
        } => {
            let name = channel_name(name, "webhook", index);
            Ok(Channel::Webhook(WebhookChannel {
                secret: secret_from_env(&name, secret_env.as_deref())?,
                url: url.trim().to_owned(),
                events: EventMask::from_config(events),
                name,
            }))
        }
        NotificationChannel::Lark {
            name,
            webhook_url,
            secret_env,
            events,
        } => {
            let name = channel_name(name, "lark", index);
            Ok(Channel::Lark(LarkChannel {
                secret: secret_from_env(&name, secret_env.as_deref())?,
                webhook_url: webhook_url.trim().to_owned(),
                events: EventMask::from_config(events),
                name,
            }))
        }
        NotificationChannel::Email {
            name,
            from,
            to,
            events,
            smtp,
            resend,
        } => {
            let name = channel_name(name, "email", index);
            let transport = match (smtp, resend) {
                (Some(smtp), None) => EmailTransport::smtp(smtp, &name)?,
                (None, Some(resend)) => EmailTransport::resend(resend, &name)?,
                _ => return Err(format!("{name}: configure either smtp or resend")),
            };
            Ok(Channel::Email(EmailChannel {
                name,
                from: from.trim().to_owned(),
                to: to.iter().map(|value| value.trim().to_owned()).collect(),
                events: EventMask::from_config(events),
                transport,
            }))
        }
    }
}

fn channel_name(name: &Option<String>, fallback: &str, index: usize) -> String {
    name.as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{fallback}-{index}"))
}

fn secret_from_env(channel: &str, env: Option<&str>) -> Result<Option<Secret>, String> {
    let Some(env) = env else { return Ok(None) };
    Secret::from_env(env)
        .map(Some)
        .map_err(|error| format!("{channel}: {error}"))
}

fn describe_kind(kind: &str, name: &str) -> String {
    if name == kind {
        kind.to_owned()
    } else {
        format!("{kind}:{name}")
    }
}

impl Channel {
    pub(super) fn name(&self) -> &str {
        match self {
            Self::Webhook(channel) => &channel.name,
            Self::Lark(channel) => &channel.name,
            Self::Email(channel) => &channel.name,
        }
    }

    /// 给 `willdeep doctor` 看的短描述。刻意不带 URL：webhook 地址里可能
    /// 藏着 token，诊断报告会被贴进 issue。
    pub(super) fn describe(&self) -> String {
        match self {
            // 老配置生成的那条渠道名字本来就是 `webhook`，别写成 `webhook:webhook`。
            Self::Webhook(channel) => describe_kind("webhook", &channel.name),
            Self::Lark(channel) => describe_kind("lark", &channel.name),
            Self::Email(channel) => {
                format!("email:{} ({})", channel.name, channel.transport.kind())
            }
        }
    }

    pub(super) fn events(&self) -> EventMask {
        match self {
            Self::Webhook(channel) => channel.events,
            Self::Lark(channel) => channel.events,
            Self::Email(channel) => channel.events,
        }
    }

    pub(super) async fn deliver(
        &self,
        client: &Client,
        event: NotificationEvent,
        payload: &WebhookPayload,
        language: Language,
    ) -> Result<(), String> {
        match self {
            Self::Webhook(channel) => channel.deliver(client, payload).await,
            Self::Lark(channel) => channel.deliver(client, event, payload, language).await,
            Self::Email(channel) => channel.deliver(client, event, payload, language).await,
        }
    }
}

impl WebhookChannel {
    async fn deliver(&self, client: &Client, payload: &WebhookPayload) -> Result<(), String> {
        let body = serde_json::to_vec(payload)
            .map_err(|error| format!("cannot serialize the payload: {error}"))?;
        let mut request = client
            .post(&self.url)
            // 桌面端和本仓库的接收方都按这几个头路由，不解析 body 也能分发。
            .header("X-App-Version", APP_VERSION)
            .header("X-Agent-Source", SOURCE)
            .header("X-Agent-Executor", EXECUTOR_KIND)
            .header("X-Webhook-Schema", SCHEMA_VERSION)
            .header("X-Agent-Event", payload.event)
            .header("Content-Type", "application/json")
            .body(body.clone());
        if let Some(secret) = &self.secret {
            request = request.header(
                "X-Willdeep-Signature",
                format!("sha256={}", hex_hmac(secret.expose().as_bytes(), &body)),
            );
        }
        match request.send().await {
            Ok(response) if response.status().is_success() => Ok(()),
            Ok(response) => Err(format!("returned HTTP {}", response.status())),
            // reqwest 的错误串里带完整 URL，而 webhook 地址本身可能就是凭据。
            Err(error) => Err(format!("delivery failed: {}", error.without_url())),
        }
    }
}

impl LarkChannel {
    async fn deliver(
        &self,
        client: &Client,
        event: NotificationEvent,
        payload: &WebhookPayload,
        language: Language,
    ) -> Result<(), String> {
        let rendered = render::render(event, payload, language);
        let body = lark::text_message(&rendered.body, self.secret.as_ref());
        let response = client
            .post(&self.webhook_url)
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("request failed: {}", error.without_url()))?;
        let status = response.status();
        let text = read_body_limited(response, MAX_RESPONSE_BODY).await;
        lark::check_response(status, &text)
    }
}

impl EmailChannel {
    async fn deliver(
        &self,
        client: &Client,
        event: NotificationEvent,
        payload: &WebhookPayload,
        language: Language,
    ) -> Result<(), String> {
        let rendered = render::render(event, payload, language);
        // 同一条事件重投时，Resend 靠这个头去重；SMTP 没有对应机制，多送一封
        // 也只能多送一封。
        let idempotency_key = Some(match payload.session_id.as_deref() {
            Some(session) => format!(
                "willdeep-{session}-{}-{}",
                payload.event, payload.occurred_at
            ),
            None => format!("willdeep-{}-{}", payload.event, payload.occurred_at),
        });
        let message = EmailMessage {
            from: self.from.clone(),
            to: self.to.clone(),
            subject: rendered.subject,
            text: rendered.body,
            idempotency_key,
        };
        self.transport.send(client, &message).await
    }
}

/// 读响应体，最多留 `limit` 字节。代理返回一整页 HTML 时不能一次性读进内存，
/// 更不能把它整段拼进日志。
pub(super) async fn read_body_limited(mut response: reqwest::Response, limit: usize) -> String {
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        body.extend_from_slice(&chunk);
        if body.len() >= limit {
            break;
        }
    }
    body.truncate(limit);
    String::from_utf8_lossy(&body).into_owned()
}

/// 与 `[intake]` 验签用的是同一套格式：对原始请求体做 HMAC-SHA256，十六进制。
pub(super) fn hex_hmac(key: &[u8], body: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts a key of any length");
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NotificationSettings;

    fn webhook_settings() -> NotificationSettings {
        NotificationSettings {
            webhook_enabled: Some(true),
            webhook_url: Some("http://127.0.0.1:1/willdeep".to_owned()),
            ..NotificationSettings::default()
        }
    }

    #[test]
    fn the_legacy_webhook_still_becomes_a_channel() {
        let (channels, errors) = build(&webhook_settings());
        assert!(errors.is_empty());
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].name(), "webhook");
        assert!(channels[0].events().wants(NotificationEvent::TaskCompleted));
        assert!(
            channels[0]
                .events()
                .wants(NotificationEvent::AttentionRequired)
        );
    }

    #[test]
    fn legacy_event_switches_survive() {
        let mut settings = webhook_settings();
        settings.webhook_on_task_completed = Some(false);
        let (channels, errors) = build(&settings);
        assert!(errors.is_empty());
        assert!(!channels[0].events().wants(NotificationEvent::TaskCompleted));
        assert!(
            channels[0]
                .events()
                .wants(NotificationEvent::AttentionRequired)
        );
    }

    #[test]
    fn a_legacy_webhook_with_both_switches_off_is_reported_not_silently_dropped() {
        let mut settings = webhook_settings();
        settings.webhook_on_task_completed = Some(false);
        settings.webhook_on_attention_required = Some(false);
        let (channels, errors) = build(&settings);
        assert!(channels.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("are false"));
    }

    #[test]
    fn a_missing_secret_skips_only_that_channel() {
        let mut settings = webhook_settings();
        settings.channels = vec![NotificationChannel::Lark {
            name: Some("ops".to_owned()),
            webhook_url: "https://open.feishu.cn/open-apis/bot/v2/hook/token".to_owned(),
            secret_env: Some("WILLDEEP_SURELY_UNSET_SECRET".to_owned()),
            events: vec![NotificationEventKind::TaskCompleted],
        }];
        let (channels, errors) = build(&settings);
        assert_eq!(channels.len(), 1, "the legacy webhook still works");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with("ops:"));
        assert!(errors[0].contains("WILLDEEP_SURELY_UNSET_SECRET"));
    }

    #[test]
    fn an_email_channel_without_a_provider_is_reported_at_build_time() {
        let settings = NotificationSettings {
            channels: vec![NotificationChannel::Email {
                name: Some("ops".to_owned()),
                from: "bot@example.com".to_owned(),
                to: vec!["me@example.com".to_owned()],
                events: vec![NotificationEventKind::TaskCompleted],
                smtp: None,
                resend: None,
            }],
            ..NotificationSettings::default()
        };
        let (channels, errors) = build(&settings);
        assert!(channels.is_empty());
        assert_eq!(errors[0], "ops: configure either smtp or resend");
    }

    #[test]
    fn the_signature_is_hex_hmac_of_the_exact_body() {
        let signature = hex_hmac(b"secret", br#"{"event":"task_completed"}"#);
        assert_eq!(signature.len(), 64);
        assert!(signature.chars().all(|value| value.is_ascii_hexdigit()));
        assert_eq!(
            signature,
            hex_hmac(b"secret", br#"{"event":"task_completed"}"#)
        );
        assert_ne!(
            signature,
            hex_hmac(b"secret", br#"{"event":"attention_required"}"#)
        );
    }
}
