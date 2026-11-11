//! 普通 OpenAI 兼容端点的模型元数据（`GET /v1/models`）。
//!
//! `/model-capabilities` 是 some.im / tokenhub 的扩展接口，普通 vLLM 没有。
//! 但 vLLM 会在 `/v1/models` 的每条记录里带 `max_model_len`，OpenRouter 一类
//! 带 `context_length`。客户端不读它，就只能按模型名猜窗口：猜大了，第一次
//! 请求就吃 400 整轮失败；猜小了，服务端的窗口永远用不满。
//!
//! 这里负责拉取、宽松解析与落盘缓存。调用方只在「用户没有显式钉死窗口」时
//! 用它，显式配置仍然优先。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{ProviderConfig, ProviderError, ProviderKind, common};

/// 拉取元数据的期限。它挡在会话启动的路上，宁可拿不到也不能让用户干等。
const FETCH_TIMEOUT_SECS: u64 = 5;
/// 模型清单很少变；一小时足够便宜。
const FRESH_TTL: Duration = Duration::from_secs(60 * 60);
/// 端点不提供该接口（404 等）时的负缓存期。
const UNSUPPORTED_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// 拉取失败时仍可使用的陈旧缓存上限。
const STALE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const CACHE_SCHEMA_VERSION: u32 = 1;

/// 一条 `/models` 记录里与容量有关的部分。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ModelMetadata {
    pub id: String,
    #[serde(default)]
    pub context_window: Option<u64>,
}

/// 宽松解析：字符串条目、对象条目、`data` / `models` / 根数组三种信封都认；
/// 单条解析不了就跳过，不因为多一个字段或一条坏数据整表作废。
pub fn parse_model_metadata(bytes: &[u8]) -> Result<Vec<ModelMetadata>, ProviderError> {
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
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let id = entry
                .as_str()
                .or_else(|| {
                    ["id", "model", "name"]
                        .into_iter()
                        .find_map(|key| entry.get(key).and_then(serde_json::Value::as_str))
                })?
                .trim();
            if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
                return None;
            }
            Some(ModelMetadata {
                id: id.to_owned(),
                context_window: context_window_of(entry),
            })
        })
        .collect())
}

/// 各家把窗口放在不同字段里：vLLM `max_model_len`、OpenRouter
/// `context_length`、LiteLLM `max_input_tokens`、llama.cpp `meta.n_ctx`。
/// `max_input_tokens` 只算输入，拿它当总窗口偏保守，但不会顶出服务端。
fn context_window_of(entry: &serde_json::Value) -> Option<u64> {
    const KEYS: [&str; 5] = [
        "max_model_len",
        "max_context_length",
        "context_length",
        "context_window",
        "max_input_tokens",
    ];
    KEYS.iter()
        .find_map(|key| entry.get(*key).and_then(serde_json::Value::as_u64))
        .or_else(|| {
            entry
                .get("meta")
                .and_then(|meta| meta.get("n_ctx"))
                .and_then(serde_json::Value::as_u64)
        })
        .filter(|window| *window > 0)
}

pub async fn fetch_model_metadata(
    config: &ProviderConfig,
) -> Result<Vec<ModelMetadata>, ProviderError> {
    let mut config = config.clone();
    config.request_timeout_secs = config.request_timeout_secs.min(FETCH_TIMEOUT_SECS);
    let endpoint = common::endpoint(&config.base_url, "models")?;
    let request = common::openai_auth(common::client(&config)?.get(endpoint), &config);
    let bytes = common::send_retrying(request, &config).await?;
    parse_model_metadata(&bytes)
}

fn find_window(models: &[ModelMetadata], model: &str) -> Option<u64> {
    let model = model.trim();
    models
        .iter()
        .find(|entry| entry.id.trim().eq_ignore_ascii_case(model))?
        .context_window
}

#[derive(Debug, Deserialize, Serialize)]
struct CacheFile {
    schema_version: u32,
    fetched_at_unix: u64,
    supported: bool,
    #[serde(default)]
    models: Vec<ModelMetadata>,
}

/// 元数据缓存目录：`<home>/cache/model-context/`。
pub fn cache_dir(home: &Path) -> PathBuf {
    home.join("cache").join("model-context")
}

/// 缓存按「端点 + Key」分文件；文件名只含摘要，不落 Key 本身。
fn cache_path(home: &Path, config: &ProviderConfig) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(config.base_url.trim().trim_end_matches('/').as_bytes());
    hasher.update([0]);
    hasher.update(config.api_key.trim().as_bytes());
    let digest = hasher.finalize();
    let name = digest
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    cache_dir(home).join(format!("{name}.json"))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

fn cache_age(cache: &CacheFile, now: u64) -> Duration {
    Duration::from_secs(now.saturating_sub(cache.fetched_at_unix))
}

fn read_cache(path: &Path) -> Option<CacheFile> {
    let bytes = std::fs::read(path).ok()?;
    let cache: CacheFile = serde_json::from_slice(&bytes).ok()?;
    (cache.schema_version == CACHE_SCHEMA_VERSION).then_some(cache)
}

fn write_cache(path: &Path, cache: &CacheFile) {
    let Some(parent) = path.parent() else {
        return;
    };
    // 缓存写不进去只是下次多拉一次，不影响本次会话，所以失败静默。
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    if let Ok(bytes) = serde_json::to_vec(cache) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// 端点声明的模型窗口：优先新鲜缓存，否则拉取；拉取失败时用不超过 7 天的
/// 陈旧缓存。端点不提供该接口、或没列出这个模型时返回 `None`，调用方沿用
/// 模型名缺省。只对普通 OpenAI 兼容端点生效：Anthropic 没有这个字段，
/// Magpie 后面是任意上游，拉到的数不一定属于当前模型。
pub async fn load_server_context_window(home: &Path, config: &ProviderConfig) -> Option<u64> {
    if config.kind != ProviderKind::OpenAiCompatible || config.magpie_integration {
        return None;
    }
    let path = cache_path(home, config);
    let now = now_unix();
    let cached = read_cache(&path);
    if let Some(cache) = cached.as_ref() {
        let ttl = if cache.supported {
            FRESH_TTL
        } else {
            UNSUPPORTED_TTL
        };
        if cache_age(cache, now) < ttl {
            return find_window(&cache.models, &config.model);
        }
    }
    match fetch_model_metadata(config).await {
        Ok(models) => {
            let found = find_window(&models, &config.model);
            write_cache(
                &path,
                &CacheFile {
                    schema_version: CACHE_SCHEMA_VERSION,
                    fetched_at_unix: now,
                    supported: true,
                    models,
                },
            );
            found
        }
        Err(ProviderError::Http { status, .. })
            if status.is_client_error() && status != reqwest::StatusCode::TOO_MANY_REQUESTS =>
        {
            write_cache(
                &path,
                &CacheFile {
                    schema_version: CACHE_SCHEMA_VERSION,
                    fetched_at_unix: now,
                    supported: false,
                    models: Vec::new(),
                },
            );
            None
        }
        Err(_) => cached
            .filter(|cache| cache.supported && cache_age(cache, now) < STALE_TTL)
            .and_then(|cache| find_window(&cache.models, &config.model)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "object": "list",
      "data": [
        {"id": "qwen3.8-27b", "object": "model", "owned_by": "vllm", "max_model_len": 131072},
        {"id": "openrouter-model", "context_length": 200000, "top_provider": {"max_completion_tokens": 8000}},
        {"id": "llama-local", "meta": {"n_ctx": 32768}},
        {"id": "no-window"},
        {"object": "model"},
        {"id": 42}
      ]
    }"#;

    #[test]
    fn parses_every_known_window_field_and_skips_malformed_entries() {
        let models = parse_model_metadata(SAMPLE.as_bytes()).expect("parse sample");
        let ids = models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "qwen3.8-27b",
                "openrouter-model",
                "llama-local",
                "no-window"
            ]
        );
        assert_eq!(models[0].context_window, Some(131_072));
        assert_eq!(models[1].context_window, Some(200_000));
        assert_eq!(models[2].context_window, Some(32_768));
        assert_eq!(models[3].context_window, None);
    }

    #[test]
    fn lookup_is_case_insensitive_and_exact() {
        let models = parse_model_metadata(SAMPLE.as_bytes()).unwrap();
        assert_eq!(
            find_window(&models, "QWEN3.8-27B"),
            Some(131_072),
            "模型 ID 大小写不敏感"
        );
        assert_eq!(
            find_window(&models, "Qwen"),
            None,
            "只认完整 ID，不做前缀匹配"
        );
    }

    #[test]
    fn accepts_root_arrays_and_string_entries() {
        let models = parse_model_metadata(br#"["plain-model", {"model": "named"}]"#).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "plain-model");
        assert_eq!(models[1].id, "named");
        assert_eq!(models[1].context_window, None);
    }

    #[test]
    fn cache_round_trip_is_key_scoped() {
        let home =
            std::env::temp_dir().join(format!("willdeep-model-meta-{}", uuid::Uuid::new_v4()));
        let config = ProviderConfig::new(
            ProviderKind::OpenAiCompatible,
            super::super::ApiDialect::ChatCompletions,
            "http://127.0.0.1:8000/v1",
            "local-key",
            "qwen3.8-27b",
        );
        let path = cache_path(&home, &config);
        let models = parse_model_metadata(SAMPLE.as_bytes()).unwrap();
        write_cache(
            &path,
            &CacheFile {
                schema_version: CACHE_SCHEMA_VERSION,
                fetched_at_unix: now_unix(),
                supported: true,
                models,
            },
        );
        let cached = read_cache(&path).unwrap();
        assert_eq!(find_window(&cached.models, &config.model), Some(131_072));
        let mut other = config.clone();
        other.base_url = "http://127.0.0.1:8001/v1".to_owned();
        assert_ne!(cache_path(&home, &other), path);
        let _ = std::fs::remove_dir_all(&home);
    }
}
