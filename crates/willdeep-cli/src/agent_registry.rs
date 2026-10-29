//! Shared implementation for CLI, TUI and authenticated Runtime management.
pub mod service;
use anyhow::{Context, Result, bail, ensure};
use clap::Subcommand;
use serde_json::{Value, json};
use service::{CustomerSession, Service};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use willdeep_core::agent_definition::store::{AgentLibrary, Origin, private_write};
use willdeep_core::agent_definition::{AgentDefinition, AgentPackage, SCHEMA};

#[derive(Clone, Debug, Subcommand)]
pub enum AgentAction {
    /// Controlled specialist experiments. Real model evaluation is not admitted in this phase.
    Rsi {
        #[command(subcommand)]
        action: crate::agent_rsi::RsiAction,
    },
    List,
    Create {
        id: String,
        name: String,
        purpose: String,
        #[arg(long, default_value = "generalist")]
        role: String,
        #[arg(long)]
        instructions_file: PathBuf,
        /// Repeat for each requested capability. Defaults to read-only capabilities.
        #[arg(long = "tool", value_parser = ["workspace.read", "workspace.edit", "process.verify", "repository.inspect"])]
        tools: Vec<String>,
    },
    Check {
        file: PathBuf,
    },
    Import {
        file: PathBuf,
    },
    Export {
        id: String,
        file: PathBuf,
    },
    Fork {
        id: String,
        new_id: String,
    },
    Pin {
        id: String,
    },
    Unpin {
        id: String,
    },
    Rollback {
        id: String,
        revision: String,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    /// Bind this installation to a local Provider/model; never uploaded.
    Bind {
        id: String,
        #[arg(long)]
        provider_profile: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        context_window: Option<u64>,
        #[arg(long)]
        max_turns: Option<usize>,
    },
    Unbind {
        id: String,
    },
    History {
        id: String,
    },
    Catalog {
        #[arg(long, default_value = service::DEFAULT_ORIGIN)]
        origin: String,
    },
    Install {
        id: String,
        revision: String,
        #[arg(long, default_value = service::DEFAULT_ORIGIN)]
        origin: String,
    },
    Login,
    Logout,
    Share {
        id: String,
    },
    Sync,
    /// Explicitly resolve one conflict returned by sync. Historical packages are retained.
    Resolve {
        id: String,
        #[arg(long, value_enum)]
        choice: service::ResolveChoice,
        #[arg(long)]
        expected_local_revision: String,
        #[arg(long)]
        expected_remote_etag: String,
        /// Create a new version retaining local instructions for a same-version fork.
        #[arg(long)]
        new_version: Option<String>,
    },
    Delete {
        id: String,
    },
}

pub async fn run(action: AgentAction, home: &Path) -> Result<()> {
    if let AgentAction::Rsi { action } = action {
        return crate::agent_rsi::run(action, home).await;
    }
    let value = match action {
        AgentAction::Create { id, name, purpose, role, instructions_file, tools } => operate(home, "save", json!({"definition": {
            "schema":SCHEMA,"definition_id":id,"version":"1.0.0","name":{"zh-Hans":name},"purpose":purpose,"base_role":role,
            "instructions":std::fs::read_to_string(instructions_file)?,"input_modalities":["text"],"tools":if tools.is_empty() {vec!["workspace.read".to_owned(),"repository.inspect".to_owned()]} else {tools},"skills":[],"forked_from":null
        }})).await?,
        AgentAction::Check { file } => { let package = read_package(&std::fs::read(file)?)?; json!({"valid":true,"revision":package.sha256,"definition":package.definition}) }
        AgentAction::Import { file } => operate(home, "import", json!({"package":read_package(&std::fs::read(file)?)?})).await?,
        AgentAction::Export { id, file } => { let package = installed(home, &id)?.1; private_write(&file, &serde_json::to_vec_pretty(&package)?)?; json!({"exported":id,"revision":package.sha256}) }
        AgentAction::List => operate(home,"list",json!({})).await?,
        AgentAction::Fork { id, new_id } => operate(home,"fork",json!({"id":id,"new_id":new_id})).await?,
        AgentAction::Pin { id } => operate(home,"pin",json!({"id":id,"value":true})).await?,
        AgentAction::Unpin { id } => operate(home,"pin",json!({"id":id,"value":false})).await?,
        AgentAction::Rollback { id, revision } => operate(home,"rollback",json!({"id":id,"revision":revision})).await?,
        AgentAction::Enable { id } => operate(home,"enable",json!({"id":id,"value":true})).await?,
        AgentAction::Disable { id } => operate(home,"enable",json!({"id":id,"value":false})).await?,
        AgentAction::Bind { id, provider_profile, model, context_window, max_turns } => operate(home,"bind",json!({"id":id,"binding":{"provider_profile":provider_profile,"model":model,"context_window":context_window,"max_turns":max_turns}})).await?,
        AgentAction::Unbind { id } => operate(home,"bind",json!({"id":id,"binding":{"provider_profile":null,"model":null}})).await?,
        AgentAction::History { id } => operate(home,"history",json!({"id":id})).await?,
        AgentAction::Catalog { origin } => operate(home,"catalog",json!({"origin":origin})).await?,
        AgentAction::Install { id, revision, origin } => operate(home,"install",json!({"id":id,"revision":revision,"origin":origin})).await?,
        AgentAction::Login => { crate::onboarding::agent_customer_login(home).await?; operate(home,"list",json!({})).await? }
        AgentAction::Logout => operate(home,"logout",json!({})).await?,
        AgentAction::Share { id } => operate(home,"share",json!({"id":id})).await?,
        AgentAction::Sync => operate(home,"sync",json!({})).await?,
        AgentAction::Resolve { id, choice, expected_local_revision, expected_remote_etag, new_version } => {
            let session=CustomerSession::load(home)?;
            operate(home,"resolve",json!({"id":id,"choice":choice,"expected_local_revision":expected_local_revision,"expected_remote_etag":expected_remote_etag,"new_version":new_version,"expected_account":{"origin":session.origin,"user_id":session.user_id,"tenant_id":session.tenant_id}})).await?
        },
        AgentAction::Delete { id } => operate(home,"delete",json!({"id":id})).await?,
        AgentAction::Rsi { .. } => unreachable!(),
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn read_package(bytes: &[u8]) -> Result<AgentPackage> {
    // Definition-only imports are a convenience; official packages always require a signature.
    if serde_json::from_slice::<Value>(bytes)?
        .get("definition")
        .is_some()
    {
        AgentPackage::parse(bytes)
    } else {
        AgentPackage::local(AgentDefinition::parse(bytes)?)
    }
}
fn id(params: &Value) -> Result<&str> {
    params
        .get("id")
        .and_then(Value::as_str)
        .context("agent id required")
}
fn installed(home: &Path, id: &str) -> Result<(AgentLibrary, AgentPackage)> {
    let mut libraries = vec![AgentLibrary::new(home)];
    if let Ok(session) = CustomerSession::load(home) {
        libraries.insert(0, session.library(home));
    }
    for library in libraries {
        if let Some(item) = library.installations()?.get(id) {
            return Ok((library.clone(), library.package(&item.sha256)?));
        }
    }
    bail!("agent not installed")
}

pub async fn operate(home: &Path, operation: &str, params: Value) -> Result<Value> {
    if let Some(operation) = operation.strip_prefix("rsi.") {
        return crate::agent_rsi::operate(home, operation, params).await;
    }
    let local = AgentLibrary::new(home);
    match operation {
        "list" => {
            let mut items = views(&local, "local")?;
            let account = match CustomerSession::load(home) {
                Ok(session) => {
                    let private = views(&session.library(home), "private")?;
                    items.retain(|item| {
                        !private.iter().any(|other| {
                            other["package"]["definition"]["definition_id"]
                                == item["package"]["definition"]["definition_id"]
                        })
                    });
                    items.extend(private);
                    Some(
                        json!({"origin":session.origin,"user_id":session.user_id,"tenant_id":session.tenant_id}),
                    )
                }
                Err(_) => None,
            };
            Ok(
                json!({"items":items,"account":account,"builtin_roles":willdeep_core::subagent::PUBLIC_SUBAGENT_IDS}),
            )
        }
        "save" => {
            let bytes = match params.get("definition_text").and_then(Value::as_str) {
                Some(text) => text.as_bytes().to_vec(),
                None => serde_json::to_vec(
                    params
                        .get("definition")
                        .context("agent definition required")?,
                )?,
            };
            let definition = AgentDefinition::parse(&bytes)?;
            ensure!(
                definition.definition_id.starts_with("user/"),
                "official edits require a fork"
            );
            let package = AgentPackage::local(definition)?;
            let library = installed(home, &package.definition.definition_id)
                .map(|v| v.0)
                .unwrap_or(local);
            let origin = if library
                .root()
                .components()
                .any(|c| c.as_os_str() == "accounts")
            {
                Origin::Private
            } else {
                Origin::Local
            };
            let etag = library
                .installations()?
                .get(&package.definition.definition_id)
                .and_then(|v| v.remote_etag.clone());
            library.install(&package, origin, &BTreeMap::new())?;
            disable_incompatible(&library, &package)?;
            if let Some(etag) = etag {
                library.set_remote_base(&package.definition.definition_id, etag)?;
            }
            Ok(json!(package))
        }
        "import" => {
            let package = match params.get("package_text").and_then(Value::as_str) {
                Some(text) => read_package(text.as_bytes())?,
                None => AgentPackage::parse(&serde_json::to_vec(
                    params.get("package").context("agent package required")?,
                )?)?,
            };
            if let Some(binding) = params.get("new_binding") {
                compatibility(&package.definition)?;
                ensure!(
                    installed(home, &package.definition.definition_id).is_err(),
                    "agent id already installed"
                );
                local.create_bound(&package, serde_json::from_value(binding.clone())?)?;
                return Ok(json!(package));
            }
            if package.definition.definition_id.starts_with("official/") {
                let trust: Value = serde_json::from_slice(&std::fs::read(local.root().join("trusted-keys.json"))?).context("official imports require trusted service signing keys; install from catalog first")?;
                let keys = serde_json::from_value(
                    trust
                        .get("keys")
                        .cloned()
                        .context("trusted signing keys missing")?,
                )?;
                install_official(&local, &package, &keys)?;
            } else {
                let library = installed(home, &package.definition.definition_id)
                    .map(|value| value.0)
                    .unwrap_or(local);
                let old = library
                    .installations()?
                    .get(&package.definition.definition_id)
                    .cloned();
                let origin = old
                    .as_ref()
                    .map(|item| item.origin.clone())
                    .unwrap_or(Origin::Local);
                library.install(&package, origin, &BTreeMap::new())?;
                if let Some(etag) = old.and_then(|item| item.remote_etag) {
                    library.set_remote_base(&package.definition.definition_id, etag)?;
                }
                disable_incompatible(&library, &package)?;
            }
            Ok(json!(package))
        }
        "export" => Ok(json!(installed(home, id(&params)?)?.1)),
        "fork" => {
            let mut definition = installed(home, id(&params)?)?.1.definition;
            let original = definition.revision()?;
            definition.definition_id = params
                .get("new_id")
                .and_then(Value::as_str)
                .context("new agent id required")?
                .into();
            ensure!(
                definition.definition_id.starts_with("user/"),
                "fork requires user namespace"
            );
            ensure!(
                !local
                    .installations()?
                    .contains_key(&definition.definition_id),
                "fork id already installed"
            );
            definition.version = "1.0.0".into();
            definition.forked_from = Some(original);
            let package = AgentPackage::local(definition)?;
            local.install(&package, Origin::Local, &BTreeMap::new())?;
            Ok(json!(package))
        }
        "history" => {
            let (library, _) = installed(home, id(&params)?)?;
            Ok(json!(library.history(id(&params)?)?))
        }
        "bind" => {
            let (library, _) = installed(home, id(&params)?)?;
            let binding = serde_json::from_value(
                params
                    .get("binding")
                    .cloned()
                    .context("local model binding required")?,
            )?;
            library.set_binding(id(&params)?, binding)?;
            Ok(json!({"updated":true,"local_only":true}))
        }
        "pin" | "enable" => {
            let (library, _) = installed(home, id(&params)?)?;
            let value = params
                .get("value")
                .and_then(Value::as_bool)
                .context("agent state required")?;
            if operation == "enable" && value {
                compatibility(&installed(home, id(&params)?)?.1.definition)?;
            }
            if operation == "pin" {
                library.set_pinned(id(&params)?, value)?;
            } else {
                library.set_enabled(id(&params)?, value)?;
            }
            Ok(json!({"updated":true}))
        }
        "rollback" => {
            let (library, _) = installed(home, id(&params)?)?;
            library.select(
                id(&params)?,
                params
                    .get("revision")
                    .and_then(Value::as_str)
                    .context("fixed revision required")?,
            )?;
            Ok(json!({"updated":true}))
        }
        "catalog" => {
            Service::new(
                params
                    .get("origin")
                    .and_then(Value::as_str)
                    .unwrap_or(service::DEFAULT_ORIGIN),
            )?
            .index()
            .await
        }
        "install" | "preview" => {
            let service = Service::new(
                params
                    .get("origin")
                    .and_then(Value::as_str)
                    .unwrap_or(service::DEFAULT_ORIGIN),
            )?;
            let package = service
                .official(
                    id(&params)?,
                    params
                        .get("revision")
                        .and_then(Value::as_str)
                        .context("fixed revision required")?,
                    home,
                )
                .await?;
            if operation == "install" {
                let trust: Value = serde_json::from_slice(&std::fs::read(
                    local.root().join("trusted-keys.json"),
                )?)?;
                let keys = serde_json::from_value(
                    trust
                        .get("keys")
                        .cloned()
                        .context("trusted signing keys missing")?,
                )?;
                install_official(&local, &package, &keys)?;
            }
            Ok(json!(package))
        }
        "share" => {
            let session = CustomerSession::load(home)?;
            let (source, package) = installed(home, id(&params)?)?;
            let source_installation = source
                .installations()?
                .remove(id(&params)?)
                .context("agent not installed")?;
            ensure!(
                package.signature.is_none()
                    && package.definition.definition_id.starts_with("user/"),
                "fork official agent before private synchronization"
            );
            let library = session.library(home);
            ensure!(
                !library.installations()?.contains_key(id(&params)?),
                "private agent already exists; edit its current revision"
            );
            library.install(&package, Origin::Private, &BTreeMap::new())?;
            library.set_binding(id(&params)?, source_installation.binding)?;
            if !source_installation.enabled {
                library.set_enabled(id(&params)?, false)?;
            }
            Ok(
                json!({"staged":true,"account":{"user_id":session.user_id,"tenant_id":session.tenant_id}}),
            )
        }
        "sync" => Ok(json!(service::synchronize(home).await?)),
        "resolve" => {
            service::resolve_scoped(
                home,
                id(&params)?,
                serde_json::from_value(
                    params
                        .get("choice")
                        .cloned()
                        .context("conflict choice required")?,
                )?,
                params
                    .get("expected_local_revision")
                    .and_then(Value::as_str)
                    .context("expected local revision required")?,
                params
                    .get("expected_remote_etag")
                    .and_then(Value::as_str)
                    .context("expected remote ETag required")?,
                params.get("new_version").and_then(Value::as_str),
                params
                    .get("expected_account")
                    .context("expected customer account required")?,
            )
            .await
        }
        "delete" => {
            let (library, package) = installed(home, id(&params)?)?;
            let item = library
                .installations()?
                .remove(id(&params)?)
                .context("agent not installed")?;
            if item.origin == Origin::Private {
                let session = CustomerSession::load(home)?;
                ensure!(
                    library.root() == session.library(home).root(),
                    "private account mismatch"
                );
                if let Some(etag) = item.remote_etag {
                    let service = Service::new(&session.origin)?;
                    service.list(&session).await?;
                    service
                        .delete(
                            &session,
                            id(&params)?,
                            &etag,
                            &service::stable_request_key(
                                &session,
                                "delete",
                                id(&params)?,
                                &package.sha256,
                                Some(&etag),
                            ),
                        )
                        .await?;
                }
            }
            library.remove(id(&params)?, &package.sha256)?;
            Ok(json!({"deleted":true}))
        }
        "logout" => {
            match std::fs::remove_file(home.join("agent-session.json")) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            };
            Ok(json!({"logged_out":true}))
        }
        _ => bail!("unsupported agent library operation"),
    }
}

fn views(library: &AgentLibrary, scope: &str) -> Result<Vec<Value>> {
    library.installations()?.values().map(|item| {
        let package = library.package(&item.sha256)?;
        let incompatible_reason = compatibility(&package.definition).err().map(|error|error.to_string());
        Ok(json!({"installation":item,"package":package,"scope":scope,"compatible":incompatible_reason.is_none(),"incompatible_reason":incompatible_reason,"optional_skills_missing":package.definition.skills.iter().filter(|skill|!skill.required && skill.content.is_empty()).map(|s|s.id.clone()).collect::<Vec<_>>()}))
    }).collect()
}

pub fn runtime_profiles(
    home: &Path,
    bases: &[willdeep_core::SubagentProfile],
    file: &crate::config::ConfigFile,
    parent: &willdeep_core::provider::ProviderConfig,
) -> Result<Vec<willdeep_core::SubagentProfile>> {
    let mut profiles = BTreeMap::new();
    let mut libraries = vec![AgentLibrary::new(home)];
    if let Ok(session) = CustomerSession::load(home) {
        libraries.push(session.library(home));
    }
    for library in libraries {
        for item in library
            .installations()?
            .values()
            .filter(|item| item.enabled)
        {
            let package = library.package(&item.sha256)?;
            let base = bases
                .iter()
                .find(|p| p.id == package.definition.base_role)
                .context("agent base role unavailable")?;
            // Incompatible agents stay visible in the library but never enter a runnable catalog.
            match package.definition.project(base) {
                Ok(mut profile) => {
                    item.binding.apply_limits(&mut profile);
                    profile.definition_binding = Some(item.binding.clone());
                    if item.binding.provider_profile.is_some() || item.binding.model.is_some() {
                        match apply_binding(&mut profile, &item.binding, file, parent) {
                            Ok(()) => {}
                            Err(error) => {
                                eprintln!(
                                    "Agent {} model binding unavailable: {error}",
                                    item.definition_id
                                );
                                continue;
                            }
                        }
                    }
                    profiles.insert(item.definition_id.clone(), profile);
                }
                Err(error) => {
                    eprintln!("Agent {} unavailable: {error}", item.definition_id);
                }
            }
        }
    }
    Ok(profiles.into_values().collect())
}

fn apply_binding(
    profile: &mut willdeep_core::SubagentProfile,
    binding: &willdeep_core::agent_definition::store::ModelBinding,
    file: &crate::config::ConfigFile,
    parent: &willdeep_core::provider::ProviderConfig,
) -> Result<()> {
    let mut configured = match binding.provider_profile.as_deref() {
        Some(name) => crate::provider_config_from_profile(file, name)?,
        None => parent.clone(),
    };
    if let Some(model) = &binding.model {
        configured.model = model.clone();
    }
    profile.model = Some(configured.model.clone());
    profile.provider = willdeep_core::build_provider(configured)?;
    profile.definition_model_bound = true;
    profile.hosted_job_prompt = false;
    Ok(())
}

pub(super) fn disable_incompatible(library: &AgentLibrary, package: &AgentPackage) -> Result<()> {
    if compatibility(&package.definition).is_err() {
        library.set_enabled(&package.definition.definition_id, false)?;
    }
    Ok(())
}

fn install_official(
    library: &AgentLibrary,
    package: &AgentPackage,
    keys: &BTreeMap<String, String>,
) -> Result<()> {
    compatibility(&package.definition).map_err(|error| {
        anyhow::anyhow!("official candidate incompatible; current installation preserved: {error}")
    })?;
    library.install(package, Origin::Official, keys)
}

pub fn compatibility(definition: &AgentDefinition) -> Result<()> {
    struct NeverRun;
    #[async_trait::async_trait]
    impl willdeep_core::provider::Provider for NeverRun {
        async fn complete(
            &self,
            _: &[willdeep_core::Message],
            _: &[willdeep_core::types::ToolDefinition],
        ) -> std::result::Result<
            willdeep_core::types::Completion,
            willdeep_core::provider::ProviderError,
        > {
            unreachable!("compatibility checks never call a model")
        }
    }
    let bases = willdeep_core::builtin_profiles(std::sync::Arc::new(NeverRun));
    let base = bases
        .iter()
        .find(|p| p.id == definition.base_role)
        .context("agent base role unavailable")?;
    definition.project(base)?;
    Ok(())
}

#[cfg(test)]
#[path = "agent_registry/tests.rs"]
mod tests;
