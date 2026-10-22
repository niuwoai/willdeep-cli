//! Credential-free, bounded loopback discovery. No prompts or cloud keys are sent.
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

const PROBE_TIMEOUT: Duration = Duration::from_millis(700);
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
struct LocalProvider {
    name: &'static str,
    base: String,
    models: Vec<String>,
}

pub(crate) async fn discover(config: Option<&Path>) -> Result<usize> {
    if std::env::var("WILLDEEP_LOCAL_DISCOVERY").is_ok_and(|value| value == "0") {
        return Ok(0);
    }
    let path = config
        .map(Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(crate::config::default_config_path)?;
    // Validate before probing or appending to a user file.
    crate::config::LoadedConfig::load(config)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(PROBE_TIMEOUT)
        .build()?;
    let ollama = loopback_base(
        &std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".into()),
    );
    let magpie = magpie_base();
    let (ollama, magpie) = tokio::join!(
        probe(&client, "ollama", ollama),
        probe(&client, "magpie", magpie),
    );
    let providers = [magpie, ollama].into_iter().flatten().collect::<Vec<_>>();
    if providers.is_empty() {
        return Ok(0);
    }
    tokio::task::spawn_blocking(move || persist(&path, &providers)).await?
}

fn loopback_base(value: &str) -> Option<String> {
    let value = if value.contains("://") {
        value.to_owned()
    } else {
        format!("http://{value}")
    };
    let mut url = reqwest::Url::parse(&value).ok()?;
    if url.host_str() == Some("0.0.0.0") {
        url.set_host(Some("127.0.0.1")).ok()?;
    }
    if url.host_str() == Some("[::]") {
        url.set_host(Some("[::1]")).ok()?;
    }
    let host = url.host_str()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || !(host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback()))
    {
        return None;
    }
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    Some(url.as_str().trim_end_matches('/').to_owned())
}

fn magpie_base() -> Option<String> {
    if let Ok(address) = std::env::var("MAGPIE_ADDR") {
        return loopback_base(&address);
    }
    let settings = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| std::path::PathBuf::from(home).join(".config"))
        })
        .map(|directory| directory.join("magpie/settings.json"))
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let port = settings
        .as_ref()
        .and_then(|settings| settings.get("port"))
        .and_then(Value::as_u64)
        .filter(|port| (1024..=65535).contains(port))
        .unwrap_or(3425);
    loopback_base(&format!("http://127.0.0.1:{port}"))
}

async fn json(client: &reqwest::Client, url: String) -> Option<Value> {
    let mut response = client.get(url).send().await.ok()?.error_for_status().ok()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).ok()
}

async fn probe(
    client: &reqwest::Client,
    name: &'static str,
    base: Option<String>,
) -> Option<LocalProvider> {
    let base = base?;
    let (path, field, id) = if name == "ollama" {
        ("/api/tags", "models", "name")
    } else {
        let info = json(client, format!("{base}/api/hello")).await?;
        if info.get("name")?.as_str()? != "magpie" {
            return None;
        }
        ("/v1/models", "data", "id")
    };
    let value = json(client, format!("{base}{path}")).await?;
    let items = value.get(field)?.as_array()?;
    let mut models = items
        .iter()
        .filter_map(|item| item.get(id)?.as_str())
        .filter(|model| {
            !model.is_empty() && model.len() <= 256 && !model.chars().any(char::is_control)
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    Some(LocalProvider {
        name,
        base: format!("{base}/v1"),
        models,
    })
}

fn persist(path: &Path, providers: &[LocalProvider]) -> Result<usize> {
    let parent = path
        .parent()
        .context("local Provider configuration has no parent")?;
    std::fs::create_dir_all(parent)?;
    let lock_path = parent.join(".local-provider-discovery.lock");
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    lock.lock().context("lock local Provider discovery")?;
    persist_locked(path, providers)
}

fn persist_locked(path: &Path, providers: &[LocalProvider]) -> Result<usize> {
    let original = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).context("read local Provider configuration"),
    };
    let parsed: crate::config::ConfigFile = toml::from_str(&original)?;
    crate::config::validate(&parsed, path)?;
    let mut patched = original.clone();
    let mut added = 0;
    for provider in providers {
        let duplicate = parsed.providers.values().any(|profile| {
            profile
                .api_base
                .as_ref()
                .is_some_and(|value| same_endpoint(value, &provider.base))
                || profile
                    .api_base_env
                    .as_ref()
                    .and_then(|key| std::env::var(key).ok())
                    .is_some_and(|value| same_endpoint(&value, &provider.base))
        });
        if duplicate {
            continue;
        }
        let mut name = provider.name.to_owned();
        let mut suffix = 1;
        while parsed.providers.contains_key(&name) {
            name = format!("{}-local-{suffix}", provider.name);
            suffix += 1;
        }
        if parsed.providers.is_empty() && added == 0 && parsed.default_provider.is_none() {
            patched = format!("default_provider = {}\n{patched}", quote(&name));
        }
        patched.push_str(&format!("\n# Automatically discovered local {}\n[providers.{}]\nprovider = 'openai-compatible'\napi = 'chat-completions'\napi_base = {}\napi_key = {}\n",
            provider.name, quote(&name), quote(&provider.base), quote(provider.name)));
        if let Some(model) = provider.models.first() {
            patched.push_str(&format!("model = {}\n", quote(model)));
        }
        added += 1;
    }
    if added == 0 {
        return Ok(0);
    }
    let validated: crate::config::ConfigFile =
        toml::from_str(&patched).context("parse discovered Provider configuration")?;
    // Existing credentials were validated above. New values are public local
    // placeholders; the atomic writer creates the final file with mode 0600.
    let mut semantic = validated.clone();
    for provider in semantic.providers.values_mut() {
        provider.api_key = None;
    }
    crate::config::validate(&semantic, path)?;
    let current = std::fs::read_to_string(path).unwrap_or_default();
    anyhow::ensure!(
        current == original,
        "configuration changed during local Provider discovery; retry on next model refresh"
    );
    crate::model_routing::write_private_atomic(path, patched.as_bytes())?;
    Ok(added)
}

fn quote(value: &str) -> String {
    toml::Value::String(value.to_owned()).to_string()
}

fn same_endpoint(left: &str, right: &str) -> bool {
    if reqwest::Url::parse(left)
        .ok()
        .is_none_or(|url| !matches!(url.path().trim_end_matches('/'), "" | "/v1"))
    {
        return false;
    }
    let (Some(left), Some(right)) = (loopback_base(left), loopback_base(right)) else {
        return false;
    };
    let normalize = |value: String| value.replace("localhost", "127.0.0.1");
    normalize(left) == normalize(right)
}

#[cfg(test)]
#[path = "local_provider_discovery_tests.rs"]
mod tests;
