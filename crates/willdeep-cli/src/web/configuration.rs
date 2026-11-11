use super::*;

#[cfg(test)]
#[path = "configuration_tests.rs"]
mod tests;

#[derive(RustEmbed)]
#[folder = "assets/willdeep-config/ui"]
struct ConfigurationUiAssets;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfigToolRequest {
    command_id: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

const CONFIG_TOOLS: &[&str] = &[
    "config.schema",
    "config.snapshot",
    "config.plan",
    "config.apply",
    "config.validate",
    "config.backups",
    "config.restore",
    "config.render",
    "config.reveal_secret",
    "config.models",
    "skills.library",
    "skills.install",
    "skills.read",
    "skills.write",
    "skills.create",
    "skills.remove",
    "rules.library",
    "rules.read",
    "rules.write",
    "rules.install",
    "rules.restore",
];

pub(super) async fn page(
    State(state): State<Arc<WebState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "configuration settings are available only from a loopback Web server",
        ));
    }
    let asset = ConfigurationUiAssets::get("index.html")
        .context("load bundled configuration workspace")
        .map_err(WebError::from_anyhow)?;
    let html = String::from_utf8(asset.data.to_vec())
        .context("decode bundled configuration workspace")
        .map_err(WebError::from_anyhow)?;
    let locale = match query.get("locale").map(String::as_str) {
        Some("en") => "en",
        Some("ja") => "ja",
        _ => "zh-CN",
    };
    let scheme = if query.get("scheme").is_some_and(|value| value == "light") {
        "light"
    } else {
        "dark"
    };
    let bootstrap = format!(
        r#"<script>
window.willdeep = {{
  capabilities: [],
  getContext: function () {{ return {{ locale: {locale}, colorScheme: {scheme} }}; }},
  executeCommand: async function (commandID, args) {{
    const response = await fetch('/api/settings/config-command', {{
      method: 'POST', headers: {{ 'content-type': 'application/json' }},
      body: JSON.stringify({{ command_id: commandID, arguments: args || {{}} }})
    }});
    const messages = {{
      'zh-CN': {{ http: '配置服务请求失败', invalid: '配置服务返回了无效响应，请检查登录状态和代理服务' }},
      en: {{ http: 'Configuration service request failed', invalid: 'Invalid configuration response; check your sign-in and proxy service' }},
      ja: {{ http: '設定サービスへのリクエストが失敗しました', invalid: '設定サービスの応答が無効です。ログイン状態とプロキシを確認してください' }}
    }}[{locale}];
    let result;
    try {{ result = await response.json(); }} catch {{
      return {{ ok: false, error: 'invalid_response', message: `${{messages.invalid}} (HTTP ${{response.status}})` }};
    }}
    if (!response.ok || (result && typeof result.error === 'string' && result.ok !== true)) {{
      const detail = result && (typeof result.message === 'string' ? result.message :
        typeof result.error === 'string' ? result.error : null);
      return {{ ok: false, error: 'request_failed', message: `${{detail || messages.http}} (HTTP ${{response.status}})` }};
    }}
    if (!result || typeof result !== 'object' || (!Array.isArray(result.content) && typeof result.ok !== 'boolean')) {{
      return {{ ok: false, error: 'invalid_response', message: `${{messages.invalid}} (HTTP ${{response.status}})` }};
    }}
    return result;
  }}
}};
</script>"#,
        locale = serde_json::to_string(locale).expect("known locale is JSON serializable"),
        scheme = serde_json::to_string(scheme).unwrap_or_else(|_| "\"dark\"".to_owned()),
    );
    let html = inject_head(&html, &bootstrap);
    Ok((
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; font-src 'self' data:; form-action 'self'; base-uri 'none'",
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CACHE_CONTROL, "no-store"),
            (
                header::CONTENT_LANGUAGE,
                if locale == "ja" { "ja" } else if locale == "en" { "en" } else { "zh-CN" },
            ),
        ],
        html,
    )
        .into_response())
}

fn inject_head(source: &str, snippet: &str) -> String {
    let lowered = source.to_ascii_lowercase();
    if let Some(start) = lowered.find("<head")
        && let Some(offset) = source[start..].find('>')
    {
        let split = start + offset + 1;
        return format!("{}{}{}", &source[..split], snippet, &source[split..]);
    }
    format!("<!doctype html><html><head>{snippet}</head><body>{source}</body></html>")
}

pub(super) async fn execute_config_tool(
    State(state): State<Arc<WebState>>,
    Json(request): Json<ConfigToolRequest>,
) -> Result<Json<serde_json::Value>, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "configuration commands are available only from a loopback Web server",
        ));
    }
    let tool = match request.command_id.as_str() {
        // The original page uses this historic alias; the server has always
        // exposed the more explicit secret tool name.
        "config.reveal" => "config.reveal_secret",
        name => name,
    };
    if !CONFIG_TOOLS.contains(&tool) {
        return Err(WebError::bad_request(
            "unknown configuration command".to_owned(),
        ));
    }
    let registry = state
        .config_tools
        .get_or_try_init(|| async { connect_config_tools(&state).await.map(Arc::new) })
        .await
        .map_err(|error| {
            WebError::from_anyhow(anyhow::anyhow!(
                "configuration service unavailable: {error}"
            ))
        })?;
    let result = registry
        .call_tool_on("willdeep-config", tool, request.arguments)
        .await
        .map_err(|error| {
            WebError::from_anyhow(anyhow::anyhow!("configuration command failed: {error}"))
        })?;
    Ok(Json(result))
}

async fn connect_config_tools(state: &WebState) -> anyhow::Result<willdeep_core::McpRegistry> {
    let package_root = state
        .home
        .join("plugin-data/willdeep-config/runtime")
        .join(crate::config_plugin::VERSION);
    let server_path = package_root.join("server/config_server.rb");
    anyhow::ensure!(
        server_path.is_file(),
        "bundled configuration service is missing"
    );
    let ruby = crate::config_plugin::find_ruby()
        .context("Ruby is required by the bundled configuration service")?;
    let backup_dir = state.home.join("plugin-data/willdeep-config/backups");
    let server = willdeep_core::McpServerConfig {
        command: Some(ruby.display().to_string()),
        args: vec![server_path.display().to_string()],
        env: [
            (
                "WD_CONFIG_FILE".to_owned(),
                state.config_path.display().to_string(),
            ),
            (
                "WD_CONFIG_BACKUP_DIR".to_owned(),
                backup_dir.display().to_string(),
            ),
            // Skills and rules live under the WillDeep home, which is not
            // always the parent of the config file (a `--config` elsewhere).
            ("WD_HOME".to_owned(), state.home.display().to_string()),
        ]
        .into_iter()
        .collect(),
        startup_timeout_seconds: 10,
        enabled: true,
        ..willdeep_core::McpServerConfig::default()
    };
    let servers = [("willdeep-config".to_owned(), server)]
        .into_iter()
        .collect();
    willdeep_core::McpRegistry::connect_in(Some(&state.home), &servers)
        .await
        .map_err(anyhow::Error::new)
}

#[derive(Serialize)]
pub(super) struct ProviderSettings {
    revision: String,
    default_provider: Option<String>,
    providers: Vec<ProviderSummary>,
    config_path: String,
}

#[derive(Serialize)]
struct ProviderSummary {
    id: String,
    provider: String,
    api: String,
    api_base: String,
    model: String,
    api_key_configured: bool,
    reasoning: Option<willdeep_core::provider::ReasoningProfile>,
    /// 界面可选档位：配置映射的键，或未配置映射时的内置档位。
    reasoning_levels: Vec<String>,
    /// `reasoning_levels` 来自内置阶梯而不是配置映射。
    reasoning_builtin: bool,
}

fn deserialize_present_reasoning<'de, D>(
    deserializer: D,
) -> Result<Option<Option<willdeep_core::provider::ReasoningProfile>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<willdeep_core::provider::ReasoningProfile>::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProviderUpdate {
    revision: String,
    id: String,
    provider: String,
    api: String,
    api_base: String,
    api_key: String,
    model: String,
    set_default: bool,
    #[serde(default)]
    reasoning_only: bool,
    #[serde(default, deserialize_with = "deserialize_present_reasoning")]
    reasoning: Option<Option<willdeep_core::provider::ReasoningProfile>>,
}

pub(super) async fn list(
    State(state): State<Arc<WebState>>,
) -> Result<Json<ProviderSettings>, WebError> {
    read_settings(&state.config_path)
        .map(Json)
        .map_err(WebError::from_anyhow)
}

pub(super) async fn update(
    State(state): State<Arc<WebState>>,
    Json(update): Json<ProviderUpdate>,
) -> Result<Json<ProviderSettings>, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "provider settings can only be changed from a loopback Web server",
        ));
    }
    let _guard = state.settings_write_lock.lock().await;
    save_provider(&state.config_path, update)
        .map_err(|error| {
            if error
                .to_string()
                .contains(crate::model_routing::STALE_CONFIG_MESSAGE)
            {
                WebError::conflict(error.to_string())
            } else {
                WebError::bad_request(error.to_string())
            }
        })
        .and_then(|_| {
            read_settings(&state.config_path)
                .map(Json)
                .map_err(WebError::from_anyhow)
        })
}

fn read_settings(path: &std::path::Path) -> anyhow::Result<ProviderSettings> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).context("read provider configuration"),
    };
    let file: crate::config::ConfigFile = if contents.trim().is_empty() {
        crate::config::ConfigFile::default()
    } else {
        toml::from_str(&contents).context("parse provider configuration")?
    };
    let providers = file
        .providers
        .iter()
        .map(|(id, profile)| {
            let reasoning = profile.reasoning.clone();
            let reasoning_levels = willdeep_core::provider::reasoning_levels(reasoning.as_ref());
            let reasoning_builtin = reasoning
                .as_ref()
                .is_none_or(|reasoning| reasoning.levels.is_empty());
            ProviderSummary {
                id: id.clone(),
                provider: profile
                    .provider
                    .clone()
                    .unwrap_or_else(|| "openai-compatible".to_owned()),
                api: profile
                    .api
                    .clone()
                    .unwrap_or_else(|| "chat-completions".to_owned()),
                api_base: profile.api_base.clone().unwrap_or_default(),
                model: profile.model.clone().unwrap_or_default(),
                api_key_configured: profile.api_key.as_ref().is_some_and(|key| !key.is_empty())
                    || profile
                        .api_key_env
                        .as_ref()
                        .is_some_and(|key| !key.is_empty()),
                reasoning,
                reasoning_levels,
                reasoning_builtin,
            }
        })
        .collect();
    Ok(ProviderSettings {
        revision: crate::model_routing::revision(&contents),
        default_provider: file.default_provider,
        providers,
        config_path: path.display().to_string(),
    })
}

fn save_provider(path: &std::path::Path, update: ProviderUpdate) -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    let current = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).context("read provider configuration"),
    };
    if crate::model_routing::revision(&current) != update.revision {
        bail!(crate::model_routing::STALE_CONFIG_MESSAGE);
    }
    if !safe_provider_id(&update.id) {
        bail!("provider id must use ASCII letters, numbers, '_' or '-' (1 to 64 characters)");
    }
    // The reasoning panel hides connection fields. Their summary values may be
    // inferred defaults or empty (for api_base_env), never connection edits.
    if update.reasoning_only {
        let old: crate::config::ConfigFile =
            toml::from_str(&current).context("parse provider configuration")?;
        anyhow::ensure!(
            old.providers.contains_key(&update.id),
            "provider was not found"
        );
        anyhow::ensure!(update.reasoning.is_some(), "reasoning update is required");
        return save_reasoning_only(path, &current, &old, &update);
    }
    if !matches!(
        update.provider.as_str(),
        "openai-compatible" | "anthropic" | "some-im"
    ) {
        bail!("unsupported provider type");
    }
    if !matches!(
        update.api.as_str(),
        "chat-completions" | "anthropic-messages" | "responses"
    ) {
        bail!("unsupported API protocol");
    }
    let base = reqwest::Url::parse(update.api_base.trim()).context("invalid API Base URL")?;
    if !matches!(base.scheme(), "http" | "https")
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
    {
        bail!("API Base URL must use http or https and include a host");
    }
    let key = update.api_key.trim();
    if key.len() > 8_192 || key.chars().any(char::is_control) {
        bail!("API key must be at most 8192 bytes and contain no control characters");
    }
    if (update.provider == "anthropic") != (update.api == "anthropic-messages") {
        bail!(
            "Anthropic providers require Anthropic Messages; other provider types require a compatible API protocol"
        );
    }
    let model = update.model.trim();
    if model.is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
        bail!("model must contain 1 to 256 characters without control characters");
    }
    let old: crate::config::ConfigFile = if current.trim().is_empty() {
        crate::config::ConfigFile::default()
    } else {
        toml::from_str(&current).context("parse provider configuration")?
    };
    let existing = old.providers.get(&update.id);
    if key.is_empty() && existing.is_none() {
        bail!("API key is required for a new provider");
    }

    let mut patched = current;
    if old.version.is_none() {
        patched = crate::model_routing::patch_key(&patched, None, "version", Some("1".to_owned()));
    }
    let section = format!("providers.{}", update.id);
    for (name, value) in [
        ("provider", update.provider.as_str()),
        ("api", update.api.as_str()),
        ("api_base", update.api_base.trim()),
        ("model", model),
    ] {
        patched = crate::model_routing::patch_key(
            &patched,
            Some(&section),
            name,
            Some(crate::model_routing::toml_string(value)),
        );
    }
    patched = patch_reasoning(
        &patched,
        &section,
        existing.and_then(|profile| profile.reasoning.as_ref()),
        &update.reasoning,
    )?;
    if !key.is_empty() {
        patched = crate::model_routing::patch_key(&patched, Some(&section), "api_key_env", None);
        patched = crate::model_routing::patch_key(
            &patched,
            Some(&section),
            "api_key",
            Some(crate::model_routing::toml_string(key)),
        );
    }
    if update.set_default || (old.default_provider.is_none() && existing.is_none()) {
        patched = crate::model_routing::patch_key(
            &patched,
            None,
            "default_provider",
            Some(crate::model_routing::toml_string(&update.id)),
        );
    }
    save_validated_configuration(path, &patched)
}

fn save_reasoning_only(
    path: &std::path::Path,
    current: &str,
    old: &crate::config::ConfigFile,
    update: &ProviderUpdate,
) -> anyhow::Result<()> {
    let section = format!("providers.{}", update.id);
    let patched = patch_reasoning(
        current,
        &section,
        old.providers
            .get(&update.id)
            .and_then(|profile| profile.reasoning.as_ref()),
        &update.reasoning,
    )?;
    save_validated_configuration(path, &patched)
}

fn patch_reasoning(
    current: &str,
    section: &str,
    existing: Option<&willdeep_core::provider::ReasoningProfile>,
    update: &Option<Option<willdeep_core::provider::ReasoningProfile>>,
) -> anyhow::Result<String> {
    if update.is_none() {
        return Ok(current.to_owned());
    }
    if let Some(Some(reasoning)) = update {
        reasoning
            .validate()
            .map_err(|error| anyhow::anyhow!("invalid reasoning profile: {error}"))?;
    }
    // Locate inline values with the TOML parser, not line matching: a valid
    // inline table cannot also be extended by a later explicit table header.
    #[derive(Deserialize)]
    struct Source {
        #[serde(default)]
        providers: std::collections::BTreeMap<String, ProviderSource>,
    }
    #[derive(Deserialize)]
    struct ProviderSource {
        reasoning: Option<toml::Spanned<ReasoningSource>>,
    }
    #[derive(Deserialize)]
    struct ReasoningSource {
        levels: Option<toml::Spanned<toml::Value>>,
    }
    let source: Source = toml::from_str(current).context("locate reasoning configuration")?;
    let provider_id = section
        .strip_prefix("providers.")
        .context("invalid provider section")?;
    let source_reasoning = source
        .providers
        .get(provider_id)
        .and_then(|value| value.reasoning.as_ref());
    let inline_span = |span: std::ops::Range<usize>| {
        current
            .get(span.clone())
            .is_some_and(|text| text.trim_start().starts_with('{'))
            .then_some(span)
    };
    let mut patched = current.to_owned();
    if let Some(span) = source_reasoning.and_then(|value| inline_span(value.span())) {
        let value = match update.as_ref().and_then(Option::as_ref) {
            Some(reasoning) => toml::Value::try_from(reasoning)?,
            None => toml::Value::Table(Default::default()),
        };
        patched.replace_range(span, &value.to_string());
        return Ok(patched);
    }
    let levels_span = source_reasoning
        .and_then(|value| value.get_ref().levels.as_ref())
        .and_then(|value| inline_span(value.span()));
    let inline_levels = levels_span.is_some();
    if let Some(span) = levels_span {
        let value = match update.as_ref().and_then(Option::as_ref) {
            Some(reasoning) => toml::Value::try_from(&reasoning.levels)?,
            None => toml::Value::Table(Default::default()),
        };
        patched.replace_range(span, &value.to_string());
    }
    let reasoning_section = format!("{section}.reasoning");
    let levels_section = format!("{reasoning_section}.levels");
    let old_levels = existing.map(|reasoning| &reasoning.levels);
    let new_levels = update
        .as_ref()
        .and_then(Option::as_ref)
        .map(|reasoning| &reasoning.levels);
    if !inline_levels {
        for old_level in old_levels.into_iter().flat_map(|levels| levels.keys()) {
            if !new_levels.is_some_and(|levels| levels.contains_key(old_level)) {
                patched = crate::model_routing::patch_key(
                    &patched,
                    Some(&levels_section),
                    old_level,
                    None,
                );
            }
        }
    }
    if let Some(Some(reasoning)) = update {
        for (name, value) in [
            ("default_level", reasoning.default_level.as_deref()),
            ("parameter", reasoning.parameter.as_deref()),
        ] {
            patched = crate::model_routing::patch_key(
                &patched,
                Some(&reasoning_section),
                name,
                value.map(crate::model_routing::toml_string),
            );
        }
        if !inline_levels {
            for (level, value) in &reasoning.levels {
                let value = match value {
                    serde_json::Value::String(value) => crate::model_routing::toml_string(value),
                    serde_json::Value::Bool(value) => value.to_string(),
                    serde_json::Value::Number(value) => value.to_string(),
                    _ => anyhow::bail!(
                        "reasoning level `{level}` must map to a string, number, or boolean"
                    ),
                };
                patched = crate::model_routing::patch_key(
                    &patched,
                    Some(&levels_section),
                    level,
                    Some(value),
                );
            }
        }
    } else if update.is_some() {
        for name in ["default_level", "parameter"] {
            patched =
                crate::model_routing::patch_key(&patched, Some(&reasoning_section), name, None);
        }
    }
    Ok(patched)
}

fn save_validated_configuration(path: &std::path::Path, patched: &str) -> anyhow::Result<()> {
    let parsed: crate::config::ConfigFile = toml::from_str(patched)
        .with_context(|| format!("validate updated configuration: {}", path.display()))?;
    crate::config::validate(&parsed, path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create provider configuration directory")?;
    }
    crate::model_routing::write_private_atomic(path, patched.as_bytes())
        .context("save provider configuration")
}

fn safe_provider_id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}
