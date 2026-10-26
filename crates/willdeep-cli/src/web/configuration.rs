use super::*;

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
    return response.json();
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
        return Err(WebError::bad_request("unknown configuration command".to_owned()));
    }
    let registry = state
        .config_tools
        .get_or_try_init(|| async { connect_config_tools(&state).await.map(Arc::new) })
        .await
        .map_err(|error| WebError::from_anyhow(anyhow::anyhow!("configuration service unavailable: {error}")))?;
    let result = registry
        .call_tool_on("willdeep-config", tool, request.arguments)
        .await
        .map_err(|error| WebError::from_anyhow(anyhow::anyhow!("configuration command failed: {error}")))?;
    Ok(Json(result))
}

async fn connect_config_tools(state: &WebState) -> anyhow::Result<willdeep_core::McpRegistry> {
    let package_root = state
        .home
        .join("plugin-data/willdeep-config/runtime")
        .join(crate::config_plugin::VERSION);
    let server_path = package_root.join("server/config_server.rb");
    anyhow::ensure!(server_path.is_file(), "bundled configuration service is missing");
    let ruby = find_ruby().context("Ruby is required by the bundled configuration service")?;
    let backup_dir = state.home.join("plugin-data/willdeep-config/backups");
    let server = willdeep_core::McpServerConfig {
        command: Some(ruby.display().to_string()),
        args: vec![server_path.display().to_string()],
        env: [
            ("WD_CONFIG_FILE".to_owned(), state.config_path.display().to_string()),
            ("WD_CONFIG_BACKUP_DIR".to_owned(), backup_dir.display().to_string()),
        ]
        .into_iter()
        .collect(),
        startup_timeout_seconds: 10,
        enabled: true,
        ..willdeep_core::McpServerConfig::default()
    };
    let servers = [("willdeep-config".to_owned(), server)].into_iter().collect();
    willdeep_core::McpRegistry::connect_in(Some(&state.home), &servers)
        .await
        .map_err(anyhow::Error::new)
}

fn find_ruby() -> Option<std::path::PathBuf> {
    ["/usr/bin/ruby", "/usr/local/bin/ruby", "/opt/homebrew/bin/ruby"]
        .into_iter()
        .map(std::path::PathBuf::from)
        .find(|path| path.is_file())
        .or_else(|| {
            std::env::var_os("PATH").and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|directory| directory.join("ruby"))
                    .find(|path| path.is_file())
            })
        })
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
}

pub(super) async fn list(
    State(state): State<Arc<WebState>>,
) -> Result<Json<ProviderSettings>, WebError> {
    read_settings(&state.config_path).map(Json).map_err(WebError::from_anyhow)
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
            if error.to_string().contains(crate::model_routing::STALE_CONFIG_MESSAGE) {
                WebError::conflict(error.to_string())
            } else {
                WebError::bad_request(error.to_string())
            }
        })
        .and_then(|_| read_settings(&state.config_path).map(Json).map_err(WebError::from_anyhow))
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
        .map(|(id, profile)| ProviderSummary {
            id: id.clone(),
            provider: profile.provider.clone().unwrap_or_else(|| "openai-compatible".to_owned()),
            api: profile.api.clone().unwrap_or_else(|| "chat-completions".to_owned()),
            api_base: profile.api_base.clone().unwrap_or_default(),
            model: profile.model.clone().unwrap_or_default(),
            api_key_configured: profile.api_key.as_ref().is_some_and(|key| !key.is_empty())
                || profile.api_key_env.as_ref().is_some_and(|key| !key.is_empty()),
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
    if !matches!(update.provider.as_str(), "openai-compatible" | "anthropic" | "some-im") {
        bail!("unsupported provider type");
    }
    if !matches!(update.api.as_str(), "chat-completions" | "anthropic-messages" | "responses") {
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
        bail!("Anthropic providers require Anthropic Messages; other provider types require a compatible API protocol");
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
    let has_existing_auth = existing.is_some_and(|profile| {
        profile.api_key.as_ref().is_some_and(|value| !value.is_empty())
            || profile.api_key_env.as_ref().is_some_and(|value| !value.is_empty())
    });
    if key.is_empty() && !has_existing_auth {
        bail!("API key is required for a new provider");
    }

    let mut patched = current;
    if old.version.is_none() {
        patched = crate::model_routing::patch_key(
            &patched,
            None,
            "version",
            Some("1".to_owned()),
        );
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
    if !key.is_empty() {
        patched = crate::model_routing::patch_key(
            &patched,
            Some(&section),
            "api_key_env",
            None,
        );
        patched = crate::model_routing::patch_key(
            &patched,
            Some(&section),
            "api_key",
            Some(crate::model_routing::toml_string(key)),
        );
    }
    if update.set_default || old.default_provider.is_none() {
        patched = crate::model_routing::patch_key(
            &patched,
            None,
            "default_provider",
            Some(crate::model_routing::toml_string(&update.id)),
        );
    }
    let parsed: crate::config::ConfigFile = toml::from_str(&patched)
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
