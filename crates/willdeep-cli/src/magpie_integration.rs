//! Read-only Magpie Agent connection and an ephemeral Provider directory.
//! ConfigFile always remains the user's disk document.
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use willdeep_core::provider::{ApiDialect, ProviderConfig, ProviderKind, magpie};

use crate::config::{ConfigFile, LoadedConfig, ProviderProfile};

pub const PROFILE_ID: &str = "magpie-cli";
pub const CLIENT_ID: &str = "willdeep-cli";
pub const MAX_MAGPIE_CONNECTION_BYTES: u64 = 65_536;
const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);
const MAX_CONTEXT: u64 = 1_048_576;
const MIN_CONTEXT: u64 = 4_000;

#[derive(Debug)]
pub struct ConnectionError {
    category: &'static str,
    retryable: bool,
}
impl fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Magpie integration: {}", self.category)
    }
}
impl std::error::Error for ConnectionError {}
fn invalid(category: &'static str) -> ConnectionError {
    ConnectionError {
        category,
        retryable: false,
    }
}
fn transient(category: &'static str) -> ConnectionError {
    ConnectionError {
        category,
        retryable: true,
    }
}

struct Fields(BTreeMap<String, Value>);
impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = Fields;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("connection object")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Fields, M::Error> {
                let mut fields = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !matches!(
                        key.as_str(),
                        "schema_version"
                            | "client_id"
                            | "enabled"
                            | "base_url"
                            | "client_marker"
                            | "model"
                            | "context_window"
                            | "max_output_tokens"
                    ) {
                        return Err(serde::de::Error::custom("unknown field"));
                    }
                    if fields.contains_key(&key) {
                        return Err(serde::de::Error::custom("duplicate field"));
                    }
                    fields.insert(key, map.next_value()?);
                }
                Ok(Fields(fields))
            }
        }
        d.deserialize_map(Object)
    }
}

/// Raw bytes must reach this parser: Value would have discarded duplicate keys.
fn parse_for(
    bytes: &[u8],
    client: &str,
) -> std::result::Result<Option<ProviderProfile>, ConnectionError> {
    if bytes.len() as u64 > MAX_MAGPIE_CONNECTION_BYTES {
        return Err(invalid("file too large"));
    }
    let Fields(fields) = serde_json::from_slice(bytes).map_err(|e| {
        if e.is_eof() {
            transient("truncated JSON")
        } else {
            invalid("invalid JSON object or fields")
        }
    })?;
    let marker = match client {
        "willdeep-cli" => magpie::CLI_MARKER,
        "willdeep" => "magpie-willdeep",
        _ => return Err(invalid("client_id")),
    };
    for (key, value) in &fields {
        let valid = match key.as_str() {
            "schema_version" => value.as_u64() == Some(1),
            "client_id" => value.as_str() == Some(client),
            "client_marker" => value.as_str() == Some(marker),
            "enabled" => value.is_boolean(),
            "base_url" => value
                .as_str()
                .is_some_and(|v| magpie::integration_url(v).is_ok()),
            "model" => value.as_str().is_some_and(magpie::validate_model),
            "context_window" => {
                value.is_null()
                    || value
                        .as_u64()
                        .is_some_and(|v| (MIN_CONTEXT..=MAX_CONTEXT).contains(&v))
            }
            "max_output_tokens" => {
                value.is_null()
                    || value
                        .as_u64()
                        .is_some_and(|v| (1..=MAX_CONTEXT).contains(&v))
            }
            _ => false,
        };
        if !valid {
            return Err(invalid(match key.as_str() {
                "schema_version" => "schema_version",
                "client_id" => "client_id",
                "client_marker" => "client_marker",
                "enabled" => "enabled",
                "base_url" => "base_url",
                "model" => "model",
                "context_window" => "context_window",
                "max_output_tokens" => "max_output_tokens",
                _ => "unknown field",
            }));
        }
    }
    let context = fields.get("context_window").and_then(Value::as_u64);
    let output = fields.get("max_output_tokens").and_then(Value::as_u64);
    if matches!((context, output), (Some(c), Some(o)) if o > c) {
        return Err(invalid("max_output_tokens exceeds context_window"));
    }
    if fields.is_empty()
        || fields.get("enabled") == Some(&Value::Bool(false))
        || (fields.len() == 1 && fields.contains_key("model"))
    {
        return Ok(None);
    }
    if fields.get("enabled") != Some(&Value::Bool(true))
        || [
            "schema_version",
            "client_id",
            "base_url",
            "client_marker",
            "model",
        ]
        .iter()
        .any(|key| !fields.contains_key(*key))
    {
        return Err(invalid("incomplete connection"));
    }
    Ok(Some(ProviderProfile {
        provider: Some("openai-compatible".into()),
        api: Some("chat-completions".into()),
        api_base: fields["base_url"].as_str().map(str::to_owned),
        api_key: Some(marker.into()),
        model: fields["model"].as_str().map(str::to_owned),
        context_window: context,
        max_output_tokens: output.map(|v| v as u32),
        ..Default::default()
    }))
}

pub fn connection_path(home: &Path) -> PathBuf {
    home.join("integrations/magpie/cli/connection.json")
}

/// An installed managed connection owns its discovery lifecycle, including
/// disconnected/model-only files. Legacy discovery must not save it to TOML.
pub fn has_connection_file() -> bool {
    crate::config::willdeep_home()
        .is_ok_and(|home| std::fs::symlink_metadata(connection_path(&home)).is_ok())
}

fn open_snapshot(path: &Path) -> std::result::Result<Option<File>, ConnectionError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK prevents a replaced FIFO from blocking before fstat.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x00200000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        #[cfg(unix)]
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(invalid("symlink is not allowed"));
        }
        Err(_) => return Err(transient("cannot open connection file")),
    };
    let metadata = file
        .metadata()
        .map_err(|_| transient("cannot inspect connection file"))?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(invalid("symlink is not allowed"));
        }
    }
    if !metadata.is_file() {
        return Err(invalid("regular file required"));
    }
    if metadata.len() > MAX_MAGPIE_CONNECTION_BYTES {
        return Err(invalid("file too large"));
    }
    Ok(Some(file))
}

pub fn read_at(path: &Path) -> std::result::Result<Option<ProviderProfile>, ConnectionError> {
    let Some(file) = open_snapshot(path)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(MAX_MAGPIE_CONNECTION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| transient("cannot read connection file"))?;
    parse_for(&bytes, CLIENT_ID)
}

pub async fn read_at_async(
    path: &Path,
) -> std::result::Result<Option<ProviderProfile>, ConnectionError> {
    for attempt in 0..2 {
        let path = path.to_owned();
        let result = tokio::task::spawn_blocking(move || read_at(&path))
            .await
            .map_err(|_| invalid("reader failed"))?;
        if attempt == 0 && result.as_ref().is_err_and(|e| e.retryable) {
            tokio::time::sleep(RETRY_DELAY).await;
        } else {
            return result;
        }
    }
    unreachable!("bounded retry returns")
}

pub struct EffectiveProviders {
    pub providers: BTreeMap<String, ProviderProfile>,
    pub default_provider: Option<String>,
    integration_error: Option<String>,
    pub connected: bool,
}
impl EffectiveProviders {
    fn from_state(
        file: &ConfigFile,
        state: std::result::Result<Option<ProviderProfile>, ConnectionError>,
    ) -> Self {
        let mut view = Self {
            providers: file.providers.clone(),
            default_provider: file.default_provider.clone(),
            integration_error: None,
            connected: false,
        };
        match state {
            Ok(Some(profile)) if !file.providers.contains_key(PROFILE_ID) => {
                view.providers.insert(PROFILE_ID.into(), profile);
                view.default_provider = Some(PROFILE_ID.into());
                view.connected = true;
            }
            Ok(Some(_)) => view.integration_error = Some(
                "Magpie integration: provider profile magpie-cli conflicts with user configuration"
                    .into(),
            ),
            Err(error) => view.integration_error = Some(error.to_string()),
            Ok(None) => {}
        }
        view
    }
    pub fn snapshot(file: &ConfigFile) -> Self {
        let state = crate::config::willdeep_home()
            .map_err(|_| invalid("configuration root unavailable"))
            .and_then(|home| read_at(&connection_path(&home)));
        Self::from_state(file, state)
    }
    pub async fn load(file: &ConfigFile) -> Self {
        let state = match crate::config::willdeep_home() {
            Ok(home) => read_at_async(&connection_path(&home)).await,
            Err(_) => Err(invalid("configuration root unavailable")),
        };
        Self::from_state(file, state)
    }
    pub fn profile(&self, name: &str) -> Result<&ProviderProfile> {
        if name == PROFILE_ID {
            if let Some(error) = &self.integration_error {
                bail!("{error}");
            }
            if !self.connected && !self.providers.contains_key(name) {
                bail!("Magpie integration is disconnected; reconnect or select another Provider");
            }
        }
        self.providers
            .get(name)
            .with_context(|| format!("provider profile not found: {name}"))
    }
    pub fn default_name(&self, integration_default: bool) -> Result<Option<String>> {
        if integration_default && let Some(error) = &self.integration_error {
            bail!("{error}");
        }
        Ok(self.default_provider.clone().or_else(|| {
            (self.providers.len() == 1)
                .then(|| self.providers.keys().next().cloned())
                .flatten()
        }))
    }
    pub fn config(&self, name: &str) -> Result<ProviderConfig> {
        self.config_with_environment(name, &crate::provider_settings::ProviderEnvironment::read())
    }
    fn config_with_environment(
        &self,
        name: &str,
        environment: &crate::provider_settings::ProviderEnvironment,
    ) -> Result<ProviderConfig> {
        let profile = self.profile(name)?;
        if name == PROFILE_ID && self.connected {
            if environment.has_connection_overrides() {
                bail!("Magpie integration conflicts with explicit connection overrides");
            }
            integration_config(profile, None, None)
        } else {
            environment.resolve(Some(profile), None)
        }
    }
}

fn integration_config(
    profile: &ProviderProfile,
    model: Option<&str>,
    output: Option<u32>,
) -> Result<ProviderConfig> {
    let mut config = ProviderConfig::new(
        ProviderKind::OpenAiCompatible,
        ApiDialect::ChatCompletions,
        profile.api_base.clone().context("Magpie base_url")?,
        magpie::CLI_MARKER,
        model
            .map(str::to_owned)
            .or_else(|| profile.model.clone())
            .context("Magpie model")?,
    );
    config.magpie_integration = true;
    config.context_window = profile.context_window;
    config.max_output_tokens = output
        .or(profile.max_output_tokens)
        .unwrap_or(16_384)
        .min(profile.context_window.unwrap_or(MAX_CONTEXT) as u32);
    config.validate()?;
    Ok(config)
}

pub struct Selection {
    pub name: Option<String>,
    pub profile: Option<ProviderProfile>,
    pub config: ProviderConfig,
}

/// Avoid first-use setup/discovery writes when this source owns the pending
/// selection. Invalid files reach the resolver's bounded retry and error path.
pub fn claims_configuration(cli: &crate::Cli) -> bool {
    let requested = cli.profile.as_deref();
    if requested == Some(PROFILE_ID) {
        return true;
    }
    if requested.is_some()
        || cli.config.is_some()
        || cli.model.is_some()
        || cli.provider.is_some()
        || cli.api.is_some()
        || cli.api_base.is_some()
        || cli.api_key.is_some()
        || crate::provider_settings::ProviderEnvironment::read().has_overrides()
    {
        return false;
    }
    crate::config::willdeep_home()
        .is_ok_and(|home| !matches!(read_at(&connection_path(&home)), Ok(None)))
}

/// Internal services store an absolute path even for the default configuration.
/// Only genuinely custom paths suppress their implicit integration selection.
pub fn custom_config(path: &Path) -> Option<PathBuf> {
    crate::config::default_config_path()
        .ok()
        .filter(|default| default == path)
        .map_or_else(|| Some(path.to_owned()), |_| None)
}

/// Resolve one task. A restored session never acquires a new integration default.
fn automatic_default(
    cli: &crate::Cli,
    resumed: Option<&willdeep_core::Session>,
    environment: &crate::provider_settings::ProviderEnvironment,
) -> bool {
    resumed.is_none()
        && cli.config.is_none()
        && cli.profile.is_none()
        && cli.model.is_none()
        && cli.api_base.is_none()
        && cli.api_key.is_none()
        && cli.provider.is_none()
        && cli.api.is_none()
        && !environment.has_overrides()
}

pub async fn resolve(
    cli: &crate::Cli,
    loaded: &LoadedConfig,
    resumed: Option<&willdeep_core::Session>,
) -> Result<Selection> {
    let environment = crate::provider_settings::ProviderEnvironment::read();
    let requested = cli
        .profile
        .as_deref()
        .or_else(|| resumed.and_then(|s| s.profile.as_deref()));
    let state = if automatic_default(cli, resumed, &environment) || requested == Some(PROFILE_ID) {
        match crate::config::willdeep_home() {
            Ok(home) => read_at_async(&connection_path(&home)).await,
            Err(_) => Err(invalid("configuration root unavailable")),
        }
    } else {
        Ok(None)
    };
    resolve_from_state(cli, loaded, resumed, &environment, state)
}

fn resolve_from_state(
    cli: &crate::Cli,
    loaded: &LoadedConfig,
    resumed: Option<&willdeep_core::Session>,
    environment: &crate::provider_settings::ProviderEnvironment,
    state: std::result::Result<Option<ProviderProfile>, ConnectionError>,
) -> Result<Selection> {
    let requested = cli
        .profile
        .as_deref()
        .or_else(|| resumed.and_then(|s| s.profile.as_deref()));
    let automatic = automatic_default(cli, resumed, environment);
    let directory = EffectiveProviders::from_state(
        &loaded.file,
        if automatic || requested == Some(PROFILE_ID) {
            state
        } else {
            Ok(None)
        },
    );
    if requested == Some(PROFILE_ID)
        && resumed.is_some_and(|session| session.magpie_integration)
        && !directory.connected
    {
        bail!(
            "Magpie integration is disconnected or invalid; reconnect or select another Provider"
        );
    }
    let name = if let Some(name) = requested {
        Some(name.to_owned())
    } else {
        directory.default_name(automatic)?
    };
    let profile = name
        .as_deref()
        .map(|name| directory.profile(name).cloned())
        .transpose()?;
    let model = cli.model.as_deref().or_else(|| {
        resumed
            .filter(|session| {
                cli.profile
                    .as_deref()
                    .is_none_or(|profile| session.profile.as_deref() == Some(profile))
            })
            .and_then(|session| session.model.as_deref())
    });
    let config = if name.as_deref() == Some(PROFILE_ID) && directory.connected {
        if cli.api_base.is_some()
            || cli.api_key.is_some()
            || cli.provider.is_some()
            || cli.api.is_some()
            || environment.has_connection_overrides()
        {
            bail!("Magpie integration conflicts with explicit connection overrides");
        }
        integration_config(
            profile.as_ref().context("Magpie profile unavailable")?,
            model.or(environment.model_override()),
            cli.max_output_tokens,
        )?
    } else {
        let mut config = environment.resolve(profile.as_ref(), Some(cli))?;
        if let Some(model) = model {
            config.model = model.to_owned();
        }
        config
    };
    Ok(Selection {
        name,
        profile,
        config,
    })
}

/// Pin new Runtime sessions without requiring credentials for unrelated profiles.
/// Existing IDs are adopted with their original route and never get a default.
pub async fn pin_new(
    home: &Path,
    request: &mut crate::daemon::session_store::CreateRuntimeSession,
    workspace_profile: Option<String>,
) -> Result<bool> {
    if let Some(id) = request.id {
        match willdeep_core::SessionStore::new(home).load(id) {
            Ok(_) => return Ok(false),
            Err(willdeep_core::session::SessionError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .context("inspect Core Session before applying a new-session default");
            }
        }
    }
    if request.profile.is_none() {
        request.profile = workspace_profile;
    }
    let loaded = LoadedConfig::load_runtime(request.config.as_deref())?;
    let mut cli = crate::Cli::bare();
    cli.config = request.config.clone();
    cli.profile = request.profile.clone();
    cli.model = request.model.clone();
    let env = crate::provider_settings::ProviderEnvironment::read();
    let eligible = cli.profile.as_deref() == Some(PROFILE_ID)
        || (cli.profile.is_none()
            && cli.config.is_none()
            && cli.model.is_none()
            && !env.has_overrides());
    if eligible {
        let directory = EffectiveProviders::load(&loaded.file).await;
        let name = cli.profile.clone().or(directory.default_name(true)?);
        if name.as_deref() == Some(PROFILE_ID) {
            let selection = resolve(&cli, &loaded, None).await?;
            request.profile = selection.name;
            request.model = Some(selection.config.model);
            return Ok(selection.config.magpie_integration);
        }
    }
    if request.profile.is_none() {
        request.profile = loaded.file.default_provider.clone().or_else(|| {
            (loaded.file.providers.len() == 1)
                .then(|| loaded.file.providers.keys().next().cloned())
                .flatten()
        });
    }
    if request.model.is_none() {
        request.model = request
            .profile
            .as_ref()
            .and_then(|n| loaded.file.providers.get(n))
            .and_then(|p| p.model.clone());
    }
    Ok(false)
}

#[cfg(test)]
#[path = "magpie_integration_tests.rs"]
mod tests;
