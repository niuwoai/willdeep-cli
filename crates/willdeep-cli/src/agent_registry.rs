//! Shared implementation for CLI, TUI and authenticated Runtime management.
mod management;
pub mod service;
use anyhow::{Context, Result, bail, ensure};
use clap::Subcommand;
pub use management::capture_scope;
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
        AgentAction::Create { id, name, purpose, role, instructions_file, tools } => management::cli_write(home, "create", json!({"definition": {
            "schema":SCHEMA,"definition_id":id,"version":"1.0.0","name":{"zh-Hans":name},"purpose":purpose,"base_role":role,
            "instructions":std::fs::read_to_string(instructions_file)?,"input_modalities":["text"],"tools":if tools.is_empty() {vec!["workspace.read".to_owned(),"repository.inspect".to_owned()]} else {tools},"skills":[],"forked_from":null
        }})).await?,
        AgentAction::Check { file } => { let package = read_package(&std::fs::read(file)?)?; json!({"valid":true,"revision":package.sha256,"definition":package.definition}) }
        AgentAction::Import { file } => management::cli_write(home, "import", json!({"package":read_package(&std::fs::read(file)?)?})).await?,
        AgentAction::Export { id, file } => { let package = installed(home, &id)?.1; private_write(&file, &serde_json::to_vec_pretty(&package)?)?; json!({"exported":id,"revision":package.sha256}) }
        AgentAction::List => operate(home,"list",json!({})).await?,
        AgentAction::Fork { id, new_id } => management::cli_write(home,"fork",json!({"id":id,"new_id":new_id})).await?,
        AgentAction::Pin { id } => management::cli_write(home,"pin",json!({"id":id,"value":true})).await?,
        AgentAction::Unpin { id } => management::cli_write(home,"pin",json!({"id":id,"value":false})).await?,
        AgentAction::Rollback { id, revision } => management::cli_write(home,"rollback",json!({"id":id,"revision":revision})).await?,
        AgentAction::Enable { id } => management::cli_write(home,"enable",json!({"id":id,"value":true})).await?,
        AgentAction::Disable { id } => management::cli_write(home,"enable",json!({"id":id,"value":false})).await?,
        AgentAction::Bind { id, provider_profile, model, context_window, max_turns } => management::cli_write(home,"bind",json!({"id":id,"binding":{"provider_profile":provider_profile,"model":model,"context_window":context_window,"max_turns":max_turns}})).await?,
        AgentAction::Unbind { id } => management::cli_write(home,"bind",json!({"id":id,"binding":{"provider_profile":null,"model":null}})).await?,
        AgentAction::History { id } => operate(home,"history",json!({"id":id})).await?,
        AgentAction::Catalog { origin } => operate(home,"catalog",json!({"origin":origin})).await?,
        AgentAction::Install { id, revision, origin } => management::cli_write(home,"install",json!({"id":id,"revision":revision,"origin":origin})).await?,
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
    find_installed(home, id)?.context("agent not installed")
}
fn find_installed(home: &Path, id: &str) -> Result<Option<(AgentLibrary, AgentPackage)>> {
    let mut libraries = vec![AgentLibrary::new(home)];
    match CustomerSession::load(home) {
        Ok(session) => libraries.insert(0, session.library(home)),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
        Err(_) => bail!("agent_scope_conflict: customer identity unavailable; sign in again"),
    }
    for library in libraries {
        if let Some(item) = library.installations()?.get(id) {
            return Ok(Some((library.clone(), library.package(&item.sha256)?)));
        }
    }
    Ok(None)
}

pub async fn operate(home: &Path, operation: &str, params: Value) -> Result<Value> {
    if let Some(operation) = operation.strip_prefix("rsi.") {
        return crate::agent_rsi::operate(home, operation, params).await;
    }
    if management::handles(operation) {
        return management::operate(home, operation, params).await;
    }
    let local = AgentLibrary::new(home);
    // Return an installation and its account scope from one consistent snapshot.
    let _read_lock = if matches!(operation, "list" | "get" | "history" | "export") {
        Some(local.lock_scope()?)
    } else {
        None
    };
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
                json!({"items":items,"account":account,"scope":capture_scope(home)?,"builtin_roles":willdeep_core::subagent::PUBLIC_SUBAGENT_IDS}),
            )
        }
        "get" => {
            let (library, package) = installed(home, id(&params)?)?;
            let installation = library
                .installations()?
                .remove(id(&params)?)
                .context("agent not installed")?;
            Ok(
                json!({"installation_revision":installation.installation_revision,"definition_revision":installation.sha256,"installation":installation,"package":package,"scope":capture_scope(home)?}),
            )
        }
        "export" => Ok(json!(installed(home, id(&params)?)?.1)),
        "history" => {
            let (library, _) = installed(home, id(&params)?)?;
            Ok(json!(library.history(id(&params)?)?))
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
                let mut params = params;
                params["package"] = serde_json::to_value(package)?;
                return management::operate(home, "import", params).await;
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
            let _lock = local.lock_scope()?;
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
        Ok(json!({"installation":item,"package":package,"scope":scope,"installation_revision":item.installation_revision,"definition_revision":item.sha256,"compatible":incompatible_reason.is_none(),"incompatible_reason":incompatible_reason,"optional_skills_missing":package.definition.skills.iter().filter(|skill|!skill.required && skill.content.is_empty()).map(|s|s.id.clone()).collect::<Vec<_>>()}))
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
    let configured = binding_config(binding, file, parent)?;
    profile.model = Some(configured.model.clone());
    profile.provider = willdeep_core::build_provider(configured)?;
    profile.definition_model_bound = true;
    profile.hosted_job_prompt = false;
    Ok(())
}

fn binding_config(
    binding: &willdeep_core::agent_definition::store::ModelBinding,
    file: &crate::config::ConfigFile,
    parent: &willdeep_core::provider::ProviderConfig,
) -> Result<willdeep_core::provider::ProviderConfig> {
    let mut configured = match binding.provider_profile.as_deref() {
        Some(name) => crate::provider_config_from_profile(file, name)?,
        None => parent.clone(),
    };
    if let Some(model) = &binding.model {
        configured.model = model.clone();
    }
    Ok(configured)
}

pub(crate) fn resolve_binding_provider(
    _home: &Path,
    binding: &willdeep_core::agent_definition::store::ModelBinding,
    file: &crate::config::ConfigFile,
    parent: &willdeep_core::provider::ProviderConfig,
    fallback: std::sync::Arc<dyn willdeep_core::provider::Provider>,
) -> Result<std::sync::Arc<dyn willdeep_core::provider::Provider>> {
    if binding.provider_profile.is_none() && binding.model.is_none() {
        return Ok(fallback);
    }
    Ok(willdeep_core::build_provider(binding_config(
        binding, file, parent,
    )?)?)
}

pub(super) fn disable_incompatible(library: &AgentLibrary, package: &AgentPackage) -> Result<()> {
    if compatibility(&package.definition).is_err() {
        library.set_enabled(&package.definition.definition_id, false)?;
    }
    Ok(())
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
#[path = "agent_registry/management_tests.rs"]
mod management_tests;
#[cfg(test)]
#[path = "agent_registry/tests.rs"]
mod tests;
