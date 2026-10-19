//! One provider resolver for foreground, Runtime and named worker profiles.
use super::*;
use std::collections::BTreeMap;

pub(crate) struct ProviderEnvironment {
    variables: BTreeMap<String, String>,
    base: Option<String>,
    key: Option<String>,
    model: Option<String>,
    openai_base: Option<String>,
    anthropic_base: Option<String>,
    openai_key: Option<String>,
    anthropic_key: Option<String>,
    someim_key: Option<String>,
}

#[cfg(test)]
#[path = "provider_settings_tests.rs"]
mod tests;

fn nonempty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

impl ProviderEnvironment {
    pub fn read() -> Self {
        Self::from_values(
            std::env::vars_os()
                .filter_map(|(key, value)| {
                    Some((key.into_string().ok()?, value.into_string().ok()?))
                })
                .collect(),
        )
    }

    fn from_values(variables: BTreeMap<String, String>) -> Self {
        let get = |name| nonempty(variables.get(name).map(String::as_str));
        Self {
            base: get("WILLDEEP_API_BASE"),
            key: get("WILLDEEP_API_KEY"),
            model: get("WILLDEEP_MODEL"),
            openai_base: get("OPENAI_BASE_URL"),
            anthropic_base: get("ANTHROPIC_BASE_URL"),
            openai_key: get("OPENAI_API_KEY"),
            anthropic_key: get("ANTHROPIC_API_KEY"),
            someim_key: get("SOMEIM_API_KEY"),
            variables,
        }
    }

    pub fn inferred_provider(&self) -> Option<ProviderArg> {
        self.inferred_provider_for(true)
    }

    fn inferred_provider_for(&self, process: bool) -> Option<ProviderArg> {
        if process && self.base.is_some() {
            return provider_from_keys(
                self.someim_key.is_some(),
                self.anthropic_key.is_some(),
                self.openai_key.is_some(),
            )
            .or(Some(ProviderArg::Auto));
        }
        if self.anthropic_base.is_some() {
            return Some(ProviderArg::Anthropic);
        }
        if self.openai_base.is_some() {
            return Some(ProviderArg::OpenAiCompatible);
        }
        if !process {
            return None;
        }
        provider_from_keys(
            self.someim_key.is_some(),
            self.anthropic_key.is_some(),
            false,
        )
    }

    pub fn profile_base(&self, profile: &ProviderProfile) -> Result<String> {
        let provider = profile
            .provider
            .as_deref()
            .map(parse_provider)
            .transpose()?
            .or_else(|| self.inferred_provider_for(false));
        self.base(None, Some(profile), provider, false)
    }

    pub fn base(
        &self,
        explicit: Option<&str>,
        profile: Option<&ProviderProfile>,
        kind: Option<ProviderArg>,
        process: bool,
    ) -> Result<String> {
        let named = profile.and_then(|p| p.api_base_env.as_deref());
        let mut base = nonempty(explicit).or_else(|| process.then(|| self.base.clone()).flatten());
        if base.is_none() {
            base = if let Some(variable) = named {
                Some(nonempty(self.variables.get(variable).map(String::as_str))
                .with_context(|| format!("provider API base environment variable {variable} is unavailable or empty"))?)
            } else {
                profile.and_then(|p| nonempty(p.api_base.as_deref()))
            };
        }
        let alias = match kind.unwrap_or(ProviderArg::Auto) {
            ProviderArg::Anthropic => self.anthropic_base.clone(),
            ProviderArg::OpenAiCompatible | ProviderArg::Auto => self.openai_base.clone(),
            ProviderArg::SomeIm => None,
        };
        let default = match kind {
            Some(ProviderArg::Anthropic) => Some("https://api.anthropic.com".to_owned()),
            Some(ProviderArg::SomeIm) => Some("https://some.im/v1".to_owned()),
            _ => None,
        };
        let base = base.or(alias).or(default)
            .context("API base is required; use api_base/api_base_env, WILLDEEP_API_BASE, OPENAI_BASE_URL, ANTHROPIC_BASE_URL, or --api-base")?;
        let parsed =
            reqwest::Url::parse(&base).context("provider API base must be a valid HTTP(S) URL")?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            bail!("provider API base must be an HTTP(S) URL with a host");
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            bail!("provider API base must not contain credentials; use api_key_env");
        }
        Ok(base)
    }

    pub fn key(
        &self,
        explicit: Option<&str>,
        profile: Option<&ProviderProfile>,
        kind: ProviderKind,
        process: bool,
    ) -> Result<String> {
        let named = profile.and_then(|p| p.api_key_env.as_deref());
        let mut key = nonempty(explicit).or_else(|| process.then(|| self.key.clone()).flatten());
        if key.is_none() {
            key = if let Some(variable) = named {
                Some(nonempty(self.variables.get(variable).map(String::as_str))
                .with_context(|| format!("provider API key environment variable {variable} is unavailable or empty"))?)
            } else {
                profile.and_then(|p| nonempty(p.api_key.as_deref()))
            };
        }
        let fallback = match kind {
            ProviderKind::Anthropic => self.anthropic_key.clone(),
            ProviderKind::OpenAiCompatible => self.openai_key.clone(),
            ProviderKind::SomeIm => self.someim_key.clone(),
        };
        key.or(fallback)
            .context("API key is required; use api_key/api_key_env, WILLDEEP_API_KEY, a provider-specific environment variable, or --api-key")
    }

    pub fn resolve(
        &self,
        profile: Option<&ProviderProfile>,
        cli: Option<&Cli>,
    ) -> Result<ProviderConfig> {
        let profile_provider = profile
            .and_then(|p| p.provider.as_deref())
            .map(parse_provider)
            .transpose()?;
        let explicit_base =
            profile.is_some_and(|p| p.api_base.is_some() || p.api_base_env.is_some());
        let selected = cli
            .and_then(|c| c.provider)
            .or(profile_provider)
            .or_else(|| explicit_base.then_some(ProviderArg::Auto))
            .or_else(|| self.inferred_provider_for(cli.is_some() && profile.is_none()));
        let base = self.base(
            cli.and_then(|c| c.api_base.as_deref()),
            profile,
            selected,
            cli.is_some(),
        )?;
        let kind = resolve_provider(selected.unwrap_or(ProviderArg::Auto), &base);
        let api = cli
            .and_then(|c| c.api)
            .or(profile
                .and_then(|p| p.api.as_deref())
                .map(parse_api)
                .transpose()?)
            .unwrap_or(ApiArg::Auto);
        let key = self.key(
            cli.and_then(|c| c.api_key.as_deref()),
            profile,
            kind,
            cli.is_some(),
        )?;
        let model = cli
            .and_then(|c| nonempty(c.model.as_deref()))
            .or_else(|| cli.and(self.model.clone()))
            .or_else(|| profile.and_then(|p| nonempty(p.model.as_deref())))
            .or_else(|| cli.and_then(|_| default_model(kind).map(str::to_owned)))
            .context("model is required; use the provider profile, WILLDEEP_MODEL, or --model")?;
        let mut config = ProviderConfig::new(kind, resolve_dialect(api, kind), base, key, model);
        config.max_output_tokens = cli
            .and_then(|c| c.max_output_tokens)
            .or_else(|| profile.and_then(|p| p.max_output_tokens))
            .unwrap_or(16_384);
        Ok(config)
    }
}
