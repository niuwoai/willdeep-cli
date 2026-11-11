mod anthropic;
pub mod capabilities;
mod chat_completions;
mod common;
mod error_detail;
pub use error_detail::safe_error_detail;
pub mod magpie;
mod main_model;
pub mod model_metadata;
mod responses;
pub mod sse;
#[cfg(test)]
mod stream_test_support;

use std::net::IpAddr;
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Url;
use uuid::Uuid;

use crate::types::{Completion, Message, ToolDefinition};

pub use anthropic::AnthropicMessagesProvider;
pub use chat_completions::ChatCompletionsProvider;
pub use main_model::{MainModelHandle, provider_model};
pub use responses::ResponsesProvider;

/// 请求体积估算与窗口之间留的余量：估算按字符比例算，不是真 tokenizer，
/// 越贴近窗口越要留一点，免得「刚好等于」被服务端算成超出。
const WINDOW_MARGIN_DIVISOR: u64 = 64;
const MIN_WINDOW_MARGIN_TOKENS: u64 = 128;
const MAX_WINDOW_MARGIN_TOKENS: u64 = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiDialect {
    ChatCompletions,
    Responses,
    AnthropicMessages,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAiCompatible,
    SomeIm,
    Anthropic,
}

/// Logical reasoning levels configured per Provider profile. Values are kept
/// as JSON scalars so providers can map names such as `balanced` to strings,
/// booleans, or numeric budgets without assuming one shared vocabulary.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningProfile {
    #[serde(default)]
    pub default_level: Option<String>,
    #[serde(default)]
    pub parameter: Option<String>,
    #[serde(default)]
    pub levels: std::collections::BTreeMap<String, serde_json::Value>,
}

impl ReasoningProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.levels.is_empty() {
            if self.default_level.is_some() || self.parameter.is_some() {
                return Err(
                    "reasoning default_level and parameter require at least one level".into(),
                );
            }
            return Ok(());
        }
        if self.levels.len() > 32 {
            return Err("reasoning levels must contain at most 32 entries".into());
        }
        for (name, value) in &self.levels {
            if name.is_empty()
                || name.len() > 64
                || !name.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                })
            {
                return Err(format!("invalid reasoning level name `{name}`"));
            }
            if !(value.is_string() || value.is_boolean() || value.is_number()) {
                return Err(format!(
                    "reasoning level `{name}` must map to a string, number, or boolean"
                ));
            }
        }
        let default = self
            .default_level
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                "reasoning.default_level is required when levels are configured".to_owned()
            })?;
        if !self.levels.contains_key(default) {
            return Err(format!(
                "reasoning.default_level `{default}` is not defined in levels"
            ));
        }
        if let Some(parameter) = self.parameter.as_deref() {
            let parts = parameter.split('.').collect::<Vec<_>>();
            if parameter.len() > 128
                || parts.is_empty()
                || parts.len() > 8
                || parts.iter().any(|part| {
                    part.is_empty()
                        || !part.chars().all(|character| {
                            character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                        })
                })
            {
                return Err(
                    "reasoning.parameter must be a dotted path of 1 to 8 simple field names".into(),
                );
            }
        }
        Ok(())
    }
}

impl ProviderKind {
    pub fn infer(base_url: &str) -> Self {
        let host = Url::parse(base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase));
        match host.as_deref() {
            Some("some.im") | Some("api.some.im") | Some("api.niuwoai.com") => Self::SomeIm,
            Some("api.anthropic.com") => Self::Anthropic,
            _ => Self::OpenAiCompatible,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub dialect: ApiDialect,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub session_id: String,
    pub workspace_id: String,
    pub request_timeout_secs: u64,
    pub max_output_tokens: u32,
    /// 端点接受的总窗口（提示词 + 回复）。知道时，请求里的 `max_tokens` 会被
    /// 收进剩余空间，免得自己把请求顶出窗口；不知道就只发配置值。
    pub context_window: Option<u64>,
    /// Per-profile logical-level to provider-value mapping.
    pub reasoning: Option<ReasoningProfile>,
    /// Per-session override; `None` selects the profile's configured default.
    pub reasoning_level: Option<String>,
    /// Explicit non-thinking mode for bounded structured auxiliary requests.
    /// Independent of authentication and local-endpoint trust.
    pub non_thinking: bool,
    /// A lost response can still represent billable work. Metered workflows
    /// disable replay so each new attempt receives its own budget reservation.
    pub allow_request_replay: bool,
    /// Explicitly allow an OpenAI-compatible endpoint to run without an API
    /// key. This is set only for the user-configured auxiliary model; normal
    /// Provider profiles continue to fail closed when credentials are absent.
    pub allow_unauthenticated: bool,
    /// Externally managed Magpie Agent connection. Only this source receives
    /// loopback pinning and the stricter no-proxy/no-redirect transport.
    pub magpie_integration: bool,
}

impl ProviderConfig {
    pub fn new(
        kind: ProviderKind,
        dialect: ApiDialect,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            dialect,
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            session_id: Uuid::new_v4().to_string(),
            workspace_id: Uuid::new_v4().to_string(),
            request_timeout_secs: 600,
            max_output_tokens: 16_384,
            context_window: None,
            reasoning: None,
            reasoning_level: None,
            non_thinking: false,
            allow_request_replay: true,
            allow_unauthenticated: false,
            magpie_integration: false,
        }
    }

    /// 这次请求真正填进 `max_tokens` 的数。
    ///
    /// `max_output_tokens` 是上限而不是承诺：端点窗口已知时，回复只能占提示词
    /// 之外剩下的那点空间。以前无论窗口多大都原样发 16384，窗口配成 16K 时
    /// 任何非空请求都必然超限，而 400 不重试——整轮直接失败。
    pub fn output_token_limit(&self, prompt_tokens: u64) -> u32 {
        let configured = self.max_output_tokens.max(1);
        let Some(window) = self.context_window.filter(|window| *window > 0) else {
            return configured;
        };
        let margin = (window / WINDOW_MARGIN_DIVISOR)
            .clamp(MIN_WINDOW_MARGIN_TOKENS, MAX_WINDOW_MARGIN_TOKENS);
        let available = window.saturating_sub(prompt_tokens).saturating_sub(margin);
        u32::try_from(available)
            .unwrap_or(u32::MAX)
            .min(configured)
            .max(1)
    }

    pub fn validate(&self) -> Result<(), ProviderError> {
        if self.magpie_integration {
            magpie::integration_url(&self.base_url)?;
            if self.api_key != magpie::CLI_MARKER
                || self.kind != ProviderKind::OpenAiCompatible
                || self.dialect != ApiDialect::ChatCompletions
                || !magpie::validate_model(&self.model)
            {
                return Err(ProviderError::InvalidBaseUrl(
                    "invalid Magpie integration connection".into(),
                ));
            }
        }
        let parsed = Url::parse(self.base_url.trim())
            .map_err(|error| ProviderError::InvalidBaseUrl(error.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ProviderError::InvalidBaseUrl(
                "scheme must be http or https".to_owned(),
            ));
        }
        if self.api_key.trim().is_empty()
            && !(self.kind == ProviderKind::OpenAiCompatible && self.allow_unauthenticated)
        {
            return Err(ProviderError::MissingApiKey);
        }
        if self.model.trim().is_empty() {
            return Err(ProviderError::MissingModel);
        }
        if let Some(reasoning) = &self.reasoning {
            reasoning
                .validate()
                .map_err(ProviderError::InvalidReasoningProfile)?;
        }
        if let Some(level) = self.reasoning_level.as_deref()
            && !self
                .reasoning
                .as_ref()
                .is_some_and(|reasoning| reasoning.levels.contains_key(level))
        {
            return Err(ProviderError::InvalidReasoningLevel(level.to_owned()));
        }
        Ok(())
    }

    /// Apply the selected logical level using the configured request path, or
    /// the protocol's conventional path when the profile leaves it unspecified.
    pub fn apply_reasoning(
        &self,
        body: &mut serde_json::Value,
        default_parameter: &str,
    ) -> Result<(), ProviderError> {
        if self.non_thinking {
            return Ok(());
        }
        let Some(reasoning) = self.reasoning.as_ref() else {
            return Ok(());
        };
        let Some(level) = self
            .reasoning_level
            .as_deref()
            .or(reasoning.default_level.as_deref())
        else {
            return Ok(());
        };
        let value = reasoning
            .levels
            .get(level)
            .cloned()
            .ok_or_else(|| ProviderError::InvalidReasoningLevel(level.to_owned()))?;
        let parameter = reasoning.parameter.as_deref().unwrap_or(default_parameter);
        set_json_path(body, parameter, value)
    }
}

fn set_json_path(
    root: &mut serde_json::Value,
    path: &str,
    value: serde_json::Value,
) -> Result<(), ProviderError> {
    let mut parts = path.split('.').peekable();
    let mut current = root;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            let object = current
                .as_object_mut()
                .ok_or_else(|| ProviderError::InvalidReasoningParameter(path.to_owned()))?;
            object.insert(part.to_owned(), value);
            return Ok(());
        }
        let object = current
            .as_object_mut()
            .ok_or_else(|| ProviderError::InvalidReasoningParameter(path.to_owned()))?;
        current = object
            .entry(part.to_owned())
            .or_insert_with(|| serde_json::json!({}));
        if !current.is_object() {
            return Err(ProviderError::InvalidReasoningParameter(path.to_owned()));
        }
    }
    Err(ProviderError::InvalidReasoningParameter(path.to_owned()))
}

/// Detect the conventional same-machine endpoints. Explicit auxiliary
/// endpoints may also live on another LAN host or domain; callers mark those
/// with `allow_unauthenticated` instead of pretending every remote Provider
/// profile is safe without credentials.
pub fn is_loopback_base_url(url: &Url) -> bool {
    match url.host_str().map(str::to_ascii_lowercase).as_deref() {
        Some("localhost") => true,
        Some(host) => host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback()),
        None => false,
    }
}

/// A local Magpie transport forwards requests to remote model services.
pub fn is_magpie_gateway(base_url: &str, api_key: &str) -> bool {
    Url::parse(base_url.trim()).is_ok_and(|url| {
        (is_loopback_base_url(&url) || url.host_str() == Some("[::1]"))
            && (url.port() == Some(3425)
                || matches!(api_key, "magpie" | "magpie-willdeep" | magpie::CLI_MARKER))
    })
}

/// The usage ledger's view of a Provider: a display label, the model and
/// whether the endpoint is a local model. Carries no credentials or URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderIdentity {
    /// `some.im`, `anthropic`, or the endpoint host for OpenAI-compatible
    /// profiles (`api.deepseek.com`, `localhost`).
    pub provider: String,
    pub model: String,
    /// Local inference (including explicitly configured LAN auxiliary models),
    /// excluding known remote gateways such as Magpie.
    pub local: bool,
}

impl ProviderIdentity {
    pub fn from_config(config: &ProviderConfig) -> Self {
        let url = Url::parse(config.base_url.trim()).ok();
        let magpie = is_magpie_gateway(&config.base_url, &config.api_key);
        let local = !magpie
            && (config.allow_unauthenticated || url.as_ref().is_some_and(is_loopback_base_url));
        let provider = if magpie {
            "magpie".to_owned()
        } else {
            match config.kind {
                ProviderKind::SomeIm => "some.im".to_owned(),
                ProviderKind::Anthropic => "anthropic".to_owned(),
                ProviderKind::OpenAiCompatible => url
                    .as_ref()
                    .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
                    .unwrap_or_else(|| "openai-compatible".to_owned()),
            }
        };
        Self {
            provider,
            model: config.model.clone(),
            local,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("API key is required")]
    MissingApiKey,
    #[error("model is required")]
    MissingModel,
    #[error("invalid API base URL: {0}")]
    InvalidBaseUrl(String),
    #[error("failed to build HTTP client: {0}")]
    Client(String),
    #[error("provider request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("provider returned HTTP {status}: {body}")]
    Http {
        status: reqwest::StatusCode,
        body: String,
        retry_after: Option<std::time::Duration>,
    },
    #[error("provider request deadline exceeded")]
    DeadlineExceeded,
    #[error(
        "provider retry requires waiting {retry_after_secs} seconds, exceeding the remaining request deadline: {source}"
    )]
    RetryDeferred {
        retry_after_secs: u64,
        source: Box<ProviderError>,
    },
    #[error("invalid reasoning profile: {0}")]
    InvalidReasoningProfile(String),
    #[error("reasoning level is not configured: {0}")]
    InvalidReasoningLevel(String),
    #[error("reasoning request parameter conflicts with the request body: {0}")]
    InvalidReasoningParameter(String),
    #[error("invalid provider response: {0}")]
    InvalidResponse(String),
    #[error("provider response did not include a completion")]
    EmptyResponse,
    #[error("provider stream interrupted: {source}")]
    StreamInterrupted {
        source: Box<ProviderError>,
        partial: Box<Completion>,
    },
}

impl ProviderError {
    /// 端点因为窗口装不下而拒绝了这个请求。外层 `Some` 表示「这是窗口问题」，
    /// 内层是错误正文里引用的上限（说不上来就是 `None`）。
    ///
    /// 只认 400/413/422 且正文带窗口关键词的错误。参数写错、模型不存在这类
    /// 400 收窗重试只会白烧一次调用，还可能把真正的错误盖掉。
    pub fn context_overflow(&self) -> Option<Option<u64>> {
        let Self::Http { status, body, .. } = self else {
            return None;
        };
        if !matches!(status.as_u16(), 400 | 413 | 422) {
            return None;
        }
        const MARKERS: [&str; 11] = [
            "context length",
            "context_length",
            "context window",
            "context_window",
            "context size",
            "maximum context",
            "prompt is too long",
            "too many tokens",
            "reduce the length of the messages",
            "max_model_len",
            "exceeds the available context",
        ];
        let lower = body.to_ascii_lowercase();
        MARKERS
            .iter()
            .any(|marker| lower.contains(marker))
            .then(|| context_limit_from(body))
    }
}

/// 从错误正文里摘出服务端声明的上下文上限。
///
/// 各家的说法不一样，按可信度顺序试：
/// - Anthropic：`prompt is too long: 213431 tokens > 200000 maximum`
/// - vLLM / OpenAI：`maximum context length is 32768 tokens`
/// - 其他：`context window (8192 tokens) exceeded`、`... exceeds the maximum (8192)`
///
/// 只取第一个命中的数字。vLLM 的正文里同时有上限和「你请求了 N 个」，
/// 先匹配上限的说法，才不会把请求体积当成窗口。
fn context_limit_from(body: &str) -> Option<u64> {
    use std::sync::LazyLock;
    static PATTERNS: LazyLock<Vec<regex::Regex>> = LazyLock::new(|| {
        [
            r"(?i)>\s*(\d{3,9})\s*maximum",
            r"(?i)maximum\s+(?:context\s+)?(?:length|window|size)[^0-9\n]{0,32}(\d{3,9})",
            r"(?i)exceed\w*[^0-9\n]{0,48}(\d{3,9})",
            r"(?i)context\s+(?:length|window|size)[^0-9\n]{0,32}(\d{3,9})",
            r"(?i)max_model_len[^0-9\n]{0,32}(\d{3,9})",
        ]
        .into_iter()
        .map(|pattern| regex::Regex::new(pattern).expect("valid context limit pattern"))
        .collect()
    });
    PATTERNS.iter().find_map(|pattern| {
        pattern
            .captures(body)
            .and_then(|captures| captures.get(1))
            .and_then(|value| value.as_str().parse::<u64>().ok())
            .filter(|limit| (1_000..=10_000_000).contains(limit))
    })
}

#[async_trait]
pub trait ProviderEventSink: Send + Sync {
    async fn emit(&self, event: ProviderEvent);
}

#[derive(Clone, Debug)]
pub enum ProviderEvent {
    TextDelta(String),
    /// 思维链增量（DeepSeek / GLM 的 `reasoning_content`）。只给界面做「思考中」
    /// 提示：不进正文、不进检查点、不进会话记录。解析器按段冲刷，不是每个
    /// SSE 块一条，免得事件流被刷爆。
    ReasoningDelta(String),
    Usage(crate::types::Usage),
    RetryWait {
        attempt: u32,
        delay: std::time::Duration,
    },
    RetryStarted {
        attempt: u32,
    },
}

pub struct NoopProviderEvents;

#[async_trait]
impl ProviderEventSink for NoopProviderEvents {
    async fn emit(&self, _event: ProviderEvent) {}
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// Who answers this Provider's requests, for the local usage ledger.
    /// Test doubles and wrappers that do not know leave it `None`; the ledger
    /// then records `provider`/`model` as null instead of guessing.
    fn ledger_identity(&self) -> Option<ProviderIdentity> {
        None
    }

    /// 这个 Provider 会请求的回复上限。Agent 用它给提示词预算预留同等空间；
    /// 测试替身和包装器不知道时留 `None`，调用方回落到窗口的 1/8。
    fn max_output_tokens(&self) -> Option<u32> {
        None
    }

    fn with_model(&self, _model: &str) -> Result<Arc<dyn Provider>, ProviderError> {
        Err(ProviderError::InvalidResponse(
            "provider does not support model reconfiguration".to_owned(),
        ))
    }

    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<Completion, ProviderError>;

    async fn complete_with_events(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        _events: &dyn ProviderEventSink,
    ) -> Result<Completion, ProviderError> {
        self.complete(messages, tools).await
    }
}

/// Return the model identifiers exposed by an OpenAI-compatible `/models`
/// endpoint. Providers in the wild use a few equivalent envelopes, so the
/// parser accepts `data`, `models`, a root array, and string/object entries.
pub async fn list_models(config: &ProviderConfig) -> Result<Vec<String>, ProviderError> {
    let endpoint = if config.kind == ProviderKind::Anthropic {
        let base_url = config
            .base_url
            .trim()
            .trim_end_matches('/')
            .trim_end_matches("/v1");
        common::endpoint(base_url, "v1/models")?
    } else {
        common::endpoint(&config.base_url, "models")?
    };
    let request = common::client(config)?.get(endpoint);
    let request = if config.kind == ProviderKind::Anthropic {
        common::anthropic_auth(request, config)
    } else {
        common::openai_auth(request, config)
    };
    let bytes = common::send_retrying(request, config).await?;
    parse_model_list(&bytes)
}

fn parse_model_list(bytes: &[u8]) -> Result<Vec<String>, ProviderError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| ProviderError::InvalidResponse(error.to_string()))?;
    let entries = value
        .as_array()
        .or_else(|| value.get("data").and_then(serde_json::Value::as_array))
        .or_else(|| value.get("models").and_then(serde_json::Value::as_array))
        .ok_or_else(|| {
            ProviderError::InvalidResponse(
                "model list must be an array or contain a data/models array".to_owned(),
            )
        })?;
    let mut models = entries
        .iter()
        .filter_map(|entry| {
            entry.as_str().or_else(|| {
                ["id", "model", "name"]
                    .into_iter()
                    .find_map(|key| entry.get(key).and_then(serde_json::Value::as_str))
            })
        })
        .map(str::trim)
        .filter(|model| {
            !model.is_empty() && model.len() <= 256 && !model.chars().any(char::is_control)
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    models.sort_by_key(|model| model.to_ascii_lowercase());
    models.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    Ok(models)
}

pub fn build_provider(config: ProviderConfig) -> Result<Arc<dyn Provider>, ProviderError> {
    config.validate()?;
    match config.dialect {
        ApiDialect::ChatCompletions => Ok(Arc::new(ChatCompletionsProvider::new(config)?)),
        ApiDialect::Responses => Ok(Arc::new(ResponsesProvider::new(config)?)),
        ApiDialect::AnthropicMessages => Ok(Arc::new(AnthropicMessagesProvider::new(config)?)),
    }
}

#[cfg(test)]
mod reasoning_profile_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_inference_is_host_exact() {
        assert_eq!(
            ProviderKind::infer("https://some.im/v1"),
            ProviderKind::SomeIm
        );
        assert_eq!(
            ProviderKind::infer("https://api.niuwoai.com/v1"),
            ProviderKind::SomeIm
        );
        assert_eq!(
            ProviderKind::infer("https://api.anthropic.com"),
            ProviderKind::Anthropic
        );
        assert_eq!(
            ProviderKind::infer("https://some.im.example/v1"),
            ProviderKind::OpenAiCompatible
        );
    }

    #[test]
    fn credential_free_openai_requires_explicit_auxiliary_opt_in() {
        let mut auxiliary = ProviderConfig::new(
            ProviderKind::OpenAiCompatible,
            ApiDialect::ChatCompletions,
            "https://models.home.example/v1",
            "",
            "gemma4:e4b-it-qat",
        );
        auxiliary.allow_unauthenticated = true;
        assert!(auxiliary.validate().is_ok());

        let ordinary_provider = ProviderConfig::new(
            ProviderKind::OpenAiCompatible,
            ApiDialect::ChatCompletions,
            "https://provider.example/v1",
            "",
            "model",
        );
        assert!(matches!(
            ordinary_provider.validate(),
            Err(ProviderError::MissingApiKey)
        ));
    }

    #[test]
    fn model_list_accepts_common_envelopes_and_deduplicates_ids() {
        let models = parse_model_list(
            br#"{"data":[{"id":"qwen3"},{"model":"GPT-5"},"glm-5",{"name":"QWEN3"},"bad\nmodel"]}"#,
        )
        .unwrap();

        assert_eq!(models, ["glm-5", "GPT-5", "qwen3"]);
        assert_eq!(
            parse_model_list(br#"{"models":[{"name":"local-model"}]}"#).unwrap(),
            ["local-model"]
        );
    }

    #[test]
    fn magpie_gateway_has_remote_ledger_identity() {
        for (base, key) in [
            ("http://localhost:3425/v1", "fixture"),
            ("http://127.0.0.1:4567/v1", "magpie"),
        ] {
            let config = ProviderConfig::new(
                ProviderKind::OpenAiCompatible,
                ApiDialect::ChatCompletions,
                base,
                key,
                "codex/gpt-6.1-sol",
            );
            let identity = ProviderIdentity::from_config(&config);
            assert!(!identity.local);
            assert_eq!(identity.provider, "magpie");
        }
        assert!(!is_magpie_gateway("https://example.com/v1", "magpie"));
        assert!(!is_magpie_gateway("http://localhost:11434/v1", "ollama"));
    }

    fn config_with_window(window: Option<u64>, output: u32) -> ProviderConfig {
        let mut config = ProviderConfig::new(
            ProviderKind::OpenAiCompatible,
            ApiDialect::ChatCompletions,
            "http://127.0.0.1:8000/v1",
            "local",
            "qwen3.8-27b",
        );
        config.context_window = window;
        config.max_output_tokens = output;
        config
    }

    #[test]
    fn output_limit_is_untouched_without_a_declared_window() {
        let config = config_with_window(None, 16_384);
        assert_eq!(config.output_token_limit(100_000), 16_384);
    }

    #[test]
    fn output_limit_shrinks_so_prompt_plus_completion_fits_the_window() {
        let config = config_with_window(Some(16_384), 16_384);
        // 提示词 12K 时只剩 4K 左右；正是「窗口 16K、max_tokens 16K」以前
        // 必然超限的场景。
        let limit = config.output_token_limit(12_288);
        assert!(limit <= 4_096, "limit was {limit}");
        assert!(limit > 0);
        // 空提示词也保留估算余量，但仍贴着配置上限。
        assert_eq!(config.output_token_limit(0), 16_128);
        // 提示词已经超出窗口时只保留最小值，让服务端的 400 说明问题，
        // 而不是再叠加一个我们自己造的超限。
        assert_eq!(config.output_token_limit(20_000), 1);
    }

    #[test]
    fn output_limit_never_exceeds_the_configured_ceiling() {
        let config = config_with_window(Some(128_000), 8_192);
        assert_eq!(config.output_token_limit(10_000), 8_192);
    }

    #[test]
    fn context_overflow_is_recognised_and_its_limit_extracted() {
        let cases = [
            (
                r#"{"error":{"message":"This model's maximum context length is 32768 tokens. However, you requested 40000 tokens (8000 in the messages, 32000 in the completion).","code":"context_length_exceeded"}}"#,
                Some(32_768),
            ),
            (
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 213431 tokens > 200000 maximum"}}"#,
                Some(200_000),
            ),
            (
                "the request exceeds the available context size, try increasing it",
                None,
            ),
            (
                r#"{"error":{"message":"context window (8192 tokens) exceeded"}}"#,
                Some(8_192),
            ),
        ];
        for (body, limit) in cases {
            let error = ProviderError::Http {
                status: reqwest::StatusCode::BAD_REQUEST,
                body: body.to_owned(),
                retry_after: None,
            };
            assert_eq!(error.context_overflow(), Some(limit), "{body}");
        }
        // 参数错误不是窗口问题：收窗重试只会白烧调用。
        let invalid = ProviderError::Http {
            status: reqwest::StatusCode::BAD_REQUEST,
            body: r#"{"error":{"message":"unknown parameter: tools"}}"#.to_owned(),
            retry_after: None,
        };
        assert_eq!(invalid.context_overflow(), None);
        let server_error = ProviderError::Http {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: "maximum context length is 32768 tokens".to_owned(),
            retry_after: None,
        };
        assert_eq!(server_error.context_overflow(), None);
    }
}
