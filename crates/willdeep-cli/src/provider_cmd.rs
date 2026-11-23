//! `willdeep provider`：脚本化增删查本地 Provider 档案。
//!
//! 只写 `api_base`（外加推断出的 `provider`/`api`），不落明文 `api_key`。
//! 密钥靠环境变量或 `--api-key-env`；与 Web 配置页共用 `patch_key` /
//! `write_private_atomic`，保留注释。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use willdeep_core::provider::ProviderKind;

use crate::config::{self, ConfigFile, LoadedConfig};
use crate::model_routing::{patch_key, remove_section_tree, toml_string, write_private_atomic};
use crate::web::safe_provider_id;

#[derive(Clone, Debug, Subcommand)]
pub(crate) enum ProviderAction {
    /// List configured Provider profiles.
    List {
        /// Emit one JSON array instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Add a Provider profile from an API base URL.
    Add {
        /// API base URL, for example `https://some.im/` or `https://api.anthropic.com`.
        #[arg(value_name = "URL")]
        url: String,
        /// Profile id written as `[providers.<name>]`. Defaults to a safe id derived from the host.
        #[arg(long)]
        name: Option<String>,
        /// Environment variable that holds the API key. Never writes a plaintext `api_key`.
        #[arg(long, value_name = "ENV")]
        api_key_env: Option<String>,
        /// Optional default model for this profile.
        #[arg(long)]
        model: Option<String>,
    },
    /// Remove a Provider profile by name.
    Remove {
        /// Profile id, as in `[providers.<name>]`.
        name: String,
    },
}

pub(crate) fn run(action: ProviderAction, config_path: Option<&Path>) -> Result<()> {
    match action {
        ProviderAction::List { json } => list(config_path, json),
        ProviderAction::Add {
            url,
            name,
            api_key_env,
            model,
        } => add(
            config_path,
            &url,
            name.as_deref(),
            api_key_env.as_deref(),
            model.as_deref(),
        ),
        ProviderAction::Remove { name } => remove(config_path, &name),
    }
}

fn resolve_path(config_path: Option<&Path>) -> Result<PathBuf> {
    match config_path {
        Some(path) => Ok(path.to_path_buf()),
        None => config::default_config_path(),
    }
}

fn read_contents(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("read configuration: {}", path.display())),
    }
}

fn parse_file(contents: &str) -> Result<ConfigFile> {
    if contents.trim().is_empty() {
        return Ok(ConfigFile::default());
    }
    crate::config::parse_config_text(contents).context("parse configuration")
}

fn list(config_path: Option<&Path>, json: bool) -> Result<()> {
    let loaded = LoadedConfig::load(config_path)?;
    let default = loaded.file.default_provider.as_deref();
    let mut rows = loaded
        .file
        .providers
        .iter()
        .map(|(name, profile)| {
            serde_json::json!({
                "name": name,
                "provider": profile.provider.clone().unwrap_or_default(),
                "api": profile.api.clone().unwrap_or_default(),
                "api_base": profile.api_base.clone().unwrap_or_default(),
                "model": profile.model.clone().unwrap_or_default(),
                "default": default == Some(name.as_str()),
                "credentials": credential_label(profile),
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        left["name"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["name"].as_str().unwrap_or_default())
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no providers configured; add one with `willdeep provider add <url>`");
        return Ok(());
    }
    println!(
        "{:<20} {:<18} {:<20} {:<8} CREDENTIALS",
        "NAME", "PROVIDER", "API", "DEFAULT"
    );
    for row in rows {
        println!(
            "{:<20} {:<18} {:<20} {:<8} {}",
            row["name"].as_str().unwrap_or(""),
            row["provider"].as_str().unwrap_or(""),
            row["api"].as_str().unwrap_or(""),
            if row["default"].as_bool().unwrap_or(false) {
                "yes"
            } else {
                ""
            },
            row["credentials"].as_str().unwrap_or(""),
        );
        let base = row["api_base"].as_str().unwrap_or("");
        let model = row["model"].as_str().unwrap_or("");
        if !base.is_empty() || !model.is_empty() {
            println!("  base={base}");
            if !model.is_empty() {
                println!("  model={model}");
            }
        }
    }
    Ok(())
}

fn credential_label(profile: &config::ProviderProfile) -> String {
    if let Some(variable) = profile.api_key_env.as_deref() {
        let present = std::env::var(variable).is_ok_and(|value| !value.trim().is_empty());
        return format!(
            "api_key_env={variable} ({})",
            if present { "set" } else { "unset" }
        );
    }
    if profile
        .api_key
        .as_deref()
        .is_some_and(|value| !value.is_empty())
    {
        return "api_key=configured".to_owned();
    }
    "missing".to_owned()
}

fn add(
    config_path: Option<&Path>,
    url: &str,
    name: Option<&str>,
    api_key_env: Option<&str>,
    model: Option<&str>,
) -> Result<()> {
    let path = resolve_path(config_path)?;
    let current = read_contents(&path)?;
    let old = parse_file(&current)?;
    config::validate(&old, &path)?;

    let normalized = normalize_api_base(url)?;
    let kind = ProviderKind::infer(&normalized);
    let (provider_label, api_label) = labels_for(kind);
    let id = resolve_provider_id(name, &normalized)?;
    if old.providers.contains_key(&id) {
        bail!(
            "provider `{id}` already exists; remove it first with `willdeep provider remove {id}`"
        );
    }
    let api_key_env = match api_key_env.map(str::trim).filter(|value| !value.is_empty()) {
        Some(variable) => {
            ensure_env_name(variable)?;
            Some(variable.to_owned())
        }
        None => None,
    };
    let model = match model.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => {
            if value.len() > 256 || value.chars().any(char::is_control) {
                bail!("model must contain 1 to 256 characters without control characters");
            }
            Some(value.to_owned())
        }
        None => None,
    };

    let mut patched = current;
    if old.version.is_none() {
        patched = patch_key(&patched, None, "version", Some("1".to_owned()));
    }
    let section = format!("providers.{id}");
    for (key, value) in [
        ("provider", provider_label),
        ("api", api_label),
        ("api_base", normalized.as_str()),
    ] {
        patched = patch_key(&patched, Some(&section), key, Some(toml_string(value)));
    }
    if let Some(variable) = &api_key_env {
        patched = patch_key(
            &patched,
            Some(&section),
            "api_key_env",
            Some(toml_string(variable)),
        );
    }
    if let Some(model) = &model {
        patched = patch_key(&patched, Some(&section), "model", Some(toml_string(model)));
    }
    let set_default = old.default_provider.is_none() && old.providers.is_empty();
    if set_default {
        patched = patch_key(&patched, None, "default_provider", Some(toml_string(&id)));
    }

    save_validated(&path, &patched)?;
    println!("added provider `{id}` ({provider_label} / {api_label})");
    println!("  api_base={normalized}");
    if set_default {
        println!("  default_provider={id}");
    }
    match api_key_env.as_deref() {
        Some(variable) => println!("  api_key_env={variable}"),
        None => {
            let hint = suggested_api_key_env(kind);
            eprintln!(
                "No API key was written. Export {hint} (or re-run with --api-key-env) before chatting."
            );
        }
    }
    Ok(())
}

fn remove(config_path: Option<&Path>, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("provider name cannot be empty");
    }
    let path = resolve_path(config_path)?;
    let current = read_contents(&path)?;
    let old = parse_file(&current)?;
    config::validate(&old, &path)?;
    if !old.providers.contains_key(name) {
        bail!(
            "provider `{name}` was not found (known: {})",
            old.providers.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    }

    let references = provider_references(&old, name);
    if !references.is_empty() {
        bail!(
            "cannot remove provider `{name}` while it is still referenced by: {}",
            references.join(", ")
        );
    }

    let mut patched = remove_section_tree(&current, &format!("providers.{name}"));
    let mut default_note = None;
    if old.default_provider.as_deref() == Some(name) {
        let remaining = old
            .providers
            .keys()
            .filter(|candidate| candidate.as_str() != name)
            .cloned()
            .collect::<Vec<_>>();
        if remaining.len() == 1 {
            let next = &remaining[0];
            patched = patch_key(&patched, None, "default_provider", Some(toml_string(next)));
            default_note = Some(format!("default_provider retargeted to `{next}`"));
        } else {
            patched = patch_key(&patched, None, "default_provider", None);
            default_note = Some(if remaining.is_empty() {
                "default_provider cleared (no providers left)".to_owned()
            } else {
                "default_provider cleared; set one with `default_provider` in config.toml"
                    .to_owned()
            });
        }
    }

    save_validated(&path, &patched)?;
    println!("removed provider `{name}`");
    if let Some(note) = default_note {
        eprintln!("{note}");
    }
    Ok(())
}

fn provider_references(file: &ConfigFile, name: &str) -> Vec<String> {
    let mut refs = Vec::new();
    if file.agent.compressor_profile.as_deref() == Some(name) {
        refs.push("agent.compressor_profile".to_owned());
    }
    for (section, settings) in &file.subagents {
        if settings.provider_profile.as_deref() == Some(name) {
            refs.push(format!("subagents.{section}.provider_profile"));
        }
    }
    for (section, settings) in &file.worker_tiers {
        if settings.provider_profile.as_deref() == Some(name) {
            refs.push(format!("worker_tiers.{section}.provider_profile"));
        }
    }
    refs
}

fn save_validated(path: &Path, patched: &str) -> Result<()> {
    let parsed = crate::config::parse_config_text(patched)
        .with_context(|| format!("validate updated configuration: {}", path.display()))?;
    config::validate(&parsed, path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create configuration directory")?;
    }
    write_private_atomic(path, patched.as_bytes()).context("save provider configuration")
}

fn normalize_api_base(url: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(url.trim()).context("invalid API base URL")?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        bail!("API base URL must use http or https, include a host, and omit credentials");
    }
    let kind = ProviderKind::infer(parsed.as_str());
    let mut normalized = parsed;
    normalized.set_fragment(None);
    // Query strings are uncommon on API bases; drop them so configs stay comparable.
    normalized.set_query(None);
    let path = normalized.path().trim_end_matches('/').to_owned();
    match kind {
        ProviderKind::Anthropic => {
            // Anthropic Messages expects the host root, not `/v1`.
            if path.is_empty() || path == "/" {
                normalized.set_path("");
            } else {
                normalized.set_path(&path);
            }
        }
        ProviderKind::OpenAiCompatible | ProviderKind::SomeIm => {
            if path.is_empty() || path == "/" || path == "/v1" {
                normalized.set_path("/v1");
            } else if path.ends_with("/v1") {
                normalized.set_path(&path);
            } else {
                normalized.set_path(&format!("{path}/v1"));
            }
        }
    }
    // API Base 统一不带尾斜杠：配置样例与 `provider_settings` 默认值都是
    // `https://api.anthropic.com` / `…/v1`，不是带 `/` 的形式。
    let mut text = normalized.to_string();
    if text.ends_with('/') {
        text.pop();
    }
    Ok(text)
}

fn resolve_provider_id(explicit: Option<&str>, api_base: &str) -> Result<String> {
    if let Some(name) = explicit.map(str::trim).filter(|value| !value.is_empty()) {
        if !safe_provider_id(name) {
            bail!("provider id must use ASCII letters, numbers, '_' or '-' (1 to 64 characters)");
        }
        return Ok(name.to_owned());
    }
    let host = reqwest::Url::parse(api_base)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .context("API base URL is missing a host")?;
    let mut derived = host
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while derived.contains("--") {
        derived = derived.replace("--", "-");
    }
    derived = derived.trim_matches('-').to_owned();
    if derived.is_empty() {
        derived = "provider".to_owned();
    }
    if derived.len() > 64 {
        derived.truncate(64);
        derived = derived.trim_end_matches('-').to_owned();
    }
    if !safe_provider_id(&derived) {
        bail!("could not derive a safe provider id from host `{host}`; pass --name");
    }
    Ok(derived)
}

fn labels_for(kind: ProviderKind) -> (&'static str, &'static str) {
    match kind {
        ProviderKind::OpenAiCompatible => ("openai-compatible", "chat-completions"),
        ProviderKind::SomeIm => ("some-im", "chat-completions"),
        ProviderKind::Anthropic => ("anthropic", "anthropic-messages"),
    }
}

fn suggested_api_key_env(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::OpenAiCompatible => "OPENAI_API_KEY",
        ProviderKind::SomeIm => "SOMEIM_API_KEY",
        ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
    }
}

fn ensure_env_name(value: &str) -> Result<()> {
    if !(1..=128).contains(&value.len())
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
        || value
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
    {
        bail!(
            "api-key-env must be 1 to 128 ASCII letters, numbers or '_', and must not start with a digit"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home() -> (TempDirGuard, PathBuf) {
        // 用系统临时目录直接拼 UUID，测完 Drop 时删；不另引 tempfile。
        let root =
            std::env::temp_dir().join(format!("willdeep-provider-cmd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("temp home");
        let path = root.join("config.toml");
        (TempDirGuard(root), path)
    }

    struct TempDirGuard(PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn normalize_appends_v1_for_openai_compatible_and_some_im() {
        assert_eq!(
            normalize_api_base("https://some.im/").unwrap(),
            "https://some.im/v1"
        );
        assert_eq!(
            normalize_api_base("https://api.openai.com/v1/").unwrap(),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_api_base("https://api.anthropic.com").unwrap(),
            "https://api.anthropic.com"
        );
        assert_eq!(
            normalize_api_base("https://api.anthropic.com/").unwrap(),
            "https://api.anthropic.com"
        );
    }

    #[test]
    fn add_list_remove_round_trip_without_plaintext_key() {
        let (_guard, path) = temp_home();
        add(Some(&path), "https://some.im/", Some("someim"), None, None).expect("add");
        let written = std::fs::read_to_string(&path).expect("read");
        assert!(written.contains("[providers.someim]"));
        assert!(written.contains("api_base = \"https://some.im/v1\""));
        assert!(written.contains("provider = \"some-im\""));
        assert!(written.contains("default_provider = \"someim\""));
        assert!(!written.contains("api_key"));

        let duplicate = add(Some(&path), "https://some.im/", Some("someim"), None, None);
        assert!(
            duplicate
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );

        list(Some(&path), true).expect("list");
        remove(Some(&path), "someim").expect("remove");
        let after = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(!after.contains("[providers.someim]"));
    }

    #[test]
    fn remove_refuses_when_compressor_still_points_here() {
        let (_guard, path) = temp_home();
        std::fs::write(
            &path,
            r#"version = 1
default_provider = "main"

[agent]
compressor_profile = "compress"

[providers.main]
provider = "openai-compatible"
api = "chat-completions"
api_base = "https://example.com/v1"
api_key_env = "OPENAI_API_KEY"

[providers.compress]
provider = "openai-compatible"
api = "chat-completions"
api_base = "http://127.0.0.1:8080/v1"
api_key_env = "LOCAL_KEY"
"#,
        )
        .unwrap();
        let error = remove(Some(&path), "compress").unwrap_err().to_string();
        assert!(error.contains("agent.compressor_profile"), "{error}");
    }

    #[test]
    fn derived_name_uses_host() {
        assert_eq!(
            resolve_provider_id(None, "https://api.example.com/v1").unwrap(),
            "api-example-com"
        );
    }
}
