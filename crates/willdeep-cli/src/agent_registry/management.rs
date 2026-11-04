//! Shared local management transactions for CLI, Web and conversation hosts.
use super::*;
use willdeep_core::agent_definition::store::{ModelBinding, Mutation, MutationRequest};

/// Non-secret identity of the library and active customer account. Hosts capture
/// this at admission, replace caller-supplied scope, and retain it through writes.
pub fn capture_scope(home: &Path) -> Result<Value> {
    let absolute = if home.is_absolute() {
        home.to_path_buf()
    } else {
        std::env::current_dir()?.join(home)
    };
    // Resolve the existing ancestor as well as a not-yet-created home. This
    // keeps /tmp and /private/tmp identities stable across the first write.
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    let root = loop {
        if let Ok(resolved) = ancestor.canonicalize() {
            break suffix
                .iter()
                .rev()
                .fold(resolved, |path, part: &&std::ffi::OsStr| path.join(part))
                .join("agents");
        }
        suffix.push(ancestor.file_name().context("invalid agent library path")?);
        ancestor = ancestor.parent().context("invalid agent library path")?;
    };
    let account = match CustomerSession::load(home) {
        Ok(session) => Some(
            json!({"origin":session.origin,"user_id":session.user_id,"tenant_id":session.tenant_id}),
        ),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            None
        }
        Err(_) => bail!("agent_scope_conflict: customer identity unavailable; sign in again"),
    };
    Ok(
        json!({"library_id":willdeep_core::agent_definition::digest(root.to_string_lossy().as_bytes()),"account":account}),
    )
}

pub(super) fn handles(operation: &str) -> bool {
    matches!(
        operation,
        "get"
            | "create"
            | "update"
            | "save"
            | "import"
            | "fork"
            | "bind"
            | "enable"
            | "pin"
            | "rollback"
    )
}

fn check_scope(home: &Path, expected: &Value) -> Result<()> {
    ensure!(
        *expected == capture_scope(home)?,
        "agent_scope_conflict: account or library changed; reload before retrying"
    );
    Ok(())
}

fn target_scope(home: &Path, library: &AgentLibrary) -> &'static str {
    if library.root() == AgentLibrary::new(home).root() {
        "local"
    } else {
        "private"
    }
}

fn check_target(
    home: &Path,
    scope: &Value,
    id: &str,
    library: &AgentLibrary,
    params: &Value,
) -> Result<()> {
    check_scope(home, scope)?;
    if let Some((current, _)) = find_installed(home, id)? {
        ensure!(
            current.root() == library.root(),
            "agent_scope_conflict: installation moved to another library; reload before retrying"
        );
    }
    if let Some(target) = params.get("target_scope").and_then(Value::as_str) {
        ensure!(
            target == target_scope(home, library),
            "agent_scope_conflict: installation library changed; reload before retrying"
        );
    }
    Ok(())
}

fn expected_revision(params: &Value) -> Result<Option<u64>> {
    match params.get("expected_revision") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => Ok(Some(value.as_u64().context(
            "agent_revision_required: expected_revision must be an installation integer",
        )?)),
    }
}

fn definition(params: &Value) -> Result<AgentDefinition> {
    let bytes = match params.get("definition_text").and_then(Value::as_str) {
        Some(text) => text.as_bytes().to_vec(),
        None => serde_json::to_vec(
            params
                .get("definition")
                .context("agent definition required")?,
        )?,
    };
    AgentDefinition::parse(&bytes)
}

fn binding(params: &Value, field: &str) -> Result<Option<ModelBinding>> {
    params
        .get(field)
        .filter(|value| !value.is_null())
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(Into::into)
}

fn import_package(params: &Value) -> Result<AgentPackage> {
    match params.get("package_text").and_then(Value::as_str) {
        Some(text) => read_package(text.as_bytes()),
        None => read_package(&serde_json::to_vec(
            params.get("package").context("agent package required")?,
        )?),
    }
}

pub(super) async fn operate(home: &Path, operation: &str, params: Value) -> Result<Value> {
    if operation == "get" {
        // Capture the selected library with its revision and account identity;
        // a later lookup could select a same-revision private installation.
        let _lock = AgentLibrary::new(home).lock_scope()?;
        let id = id(&params)?;
        let (library, package) = installed(home, id)?;
        let installation = library
            .installations()?
            .remove(id)
            .context("agent not installed")?;
        return Ok(json!({
            "installation_revision":installation.installation_revision,
            "definition_revision":installation.sha256,
            "installation":installation,"package":package,
            "scope":capture_scope(home)?,"target_scope":target_scope(home, &library)
        }));
    }
    let scope = params
        .get("expected_scope")
        .context("agent_scope_conflict: expected_scope required; reload the library")?
        .clone();
    check_scope(home, &scope)?;
    let request = MutationRequest {
        request_id: params
            .get("request_id")
            .and_then(Value::as_str)
            .context("request_id required")?
            .to_owned(),
        expected_revision: expected_revision(&params)?,
        // Scope and operation participate in idempotency. No account token or
        // provider credentials enter either this payload or the persisted receipt.
        payload: json!({"operation":operation,"params":params}),
    };
    let local = AgentLibrary::new(home);
    let (library, receipt, source_id) = match operation {
        "create" | "update" | "save" | "import" => {
            if operation == "update" {
                ensure!(
                    request.expected_revision.is_some(),
                    "agent_revision_required: expected_revision required for update"
                );
            }
            if operation == "create" {
                ensure!(
                    request.expected_revision.is_none(),
                    "create requires an absent id, without expected_revision"
                );
            }
            let package = if operation == "import" {
                import_package(&params)?
            } else {
                AgentPackage::local(definition(&params)?)?
            };
            let current = find_installed(home, &package.definition.definition_id)?;
            let library = current
                .as_ref()
                .map(|(library, _)| library.clone())
                .unwrap_or_else(|| local.clone());
            let origin = if package.signature.is_some() {
                Origin::Official
            } else {
                current
                    .as_ref()
                    .and_then(|(library, _)| {
                        library
                            .installations()
                            .ok()?
                            .get(&package.definition.definition_id)
                            .map(|item| item.origin.clone())
                    })
                    .unwrap_or(Origin::Local)
            };
            let compatible = compatibility(&package.definition).is_ok();
            let keys = if origin == Origin::Official {
                compatibility(&package.definition).map_err(|error| {
                    anyhow::anyhow!(
                        "official candidate incompatible; current installation preserved: {error}"
                    )
                })?;
                let trust: Value = serde_json::from_slice(&std::fs::read(
                    local.root().join("trusted-keys.json"),
                )?)?;
                serde_json::from_value(
                    trust
                        .get("keys")
                        .cloned()
                        .context("trusted signing keys missing")?,
                )?
            } else {
                BTreeMap::new()
            };
            let new_binding = binding(&params, "binding")?.or(binding(&params, "new_binding")?);
            // Foreign imports that bind on creation must never replace an existing id.
            let create_only = operation == "create"
                || request.expected_revision.is_none()
                || params.get("new_binding").is_some();
            let receipt = library.mutate(
                &request,
                || {
                    check_target(
                        home,
                        &scope,
                        &package.definition.definition_id,
                        &library,
                        &params,
                    )
                },
                Mutation::Install {
                    package: &package,
                    origin,
                    keys: &keys,
                    binding: new_binding,
                    compatible,
                    create_only,
                    allow_unchanged: operation == "import",
                },
            )?;
            (library, receipt, None)
        }
        "fork" => {
            let source_id = id(&params)?;
            let (source, _) = installed(home, source_id)?;
            let new_id = params
                .get("new_id")
                .and_then(Value::as_str)
                .context("new_id required")?;
            let receipt = local.mutate(
                &request,
                || {
                    check_target(home, &scope, source_id, &source, &params)?;
                    check_target(home, &scope, new_id, &local, &json!({}))
                },
                Mutation::Fork {
                    source: &source,
                    id: source_id,
                    new_id,
                },
            )?;
            (local, receipt, Some(source_id.to_owned()))
        }
        _ => {
            let id = id(&params)?;
            let (library, package) = installed(home, id)?;
            let mutation = match operation {
                "bind" => Mutation::Bind {
                    id,
                    binding: binding(&params, "binding")?.context("binding required")?,
                },
                "enable" => {
                    let value = params
                        .get("value")
                        .and_then(Value::as_bool)
                        .context("boolean value required")?;
                    if value {
                        compatibility(&package.definition)?;
                    }
                    Mutation::Enable { id, value }
                }
                "pin" => Mutation::Pin {
                    id,
                    value: params
                        .get("value")
                        .and_then(Value::as_bool)
                        .context("boolean value required")?,
                },
                "rollback" => Mutation::Rollback {
                    id,
                    revision: params
                        .get("revision")
                        .and_then(Value::as_str)
                        .context("historical revision SHA required")?,
                },
                _ => bail!("unsupported agent library operation"),
            };
            let receipt = library.mutate(
                &request,
                || check_target(home, &scope, id, &library, &params),
                mutation,
            )?;
            (library, receipt, None)
        }
    };
    let item = receipt.installation;
    let package = library.package(&item.sha256)?;
    let mut value = serde_json::to_value(package)?;
    value["definition_id"] = item.definition_id.into();
    if let Some(source_id) = source_id {
        value["source_definition_id"] = source_id.into();
    }
    value["installation_revision"] = item.installation_revision.into();
    value["definition_revision"] = item.sha256.into();
    value["scope"] = scope;
    value["target_scope"] = target_scope(home, &library).into();
    value["saved"] = true.into();
    value["replayed"] = receipt.replayed.into();
    Ok(value)
}

/// CLI commands capture their own scope and read the current installation before
/// submitting the same guarded API used by the Web and conversation hosts.
pub(super) async fn cli_write(home: &Path, operation: &str, params: Value) -> Result<Value> {
    let params = cli_params(home, operation, params)?;
    super::operate(home, operation, params).await
}

pub(super) fn cli_params(home: &Path, operation: &str, mut params: Value) -> Result<Value> {
    // Capture the identity, selected library and revision from one snapshot;
    // release the read lock before the guarded transaction acquires it again.
    let _lock = AgentLibrary::new(home).lock_scope()?;
    params["expected_scope"] = capture_scope(home)?;
    params["request_id"] = uuid::Uuid::new_v4().to_string().into();
    let target = match operation {
        "create" | "update" | "save" => Some(definition(&params)?.definition_id),
        "import" => Some(import_package(&params)?.definition.definition_id),
        _ => params.get("id").and_then(Value::as_str).map(str::to_owned),
    };
    if operation != "create"
        && let Some(id) = target
    {
        match find_installed(home, &id)? {
            Some((library, _)) => {
                let item = library
                    .installations()?
                    .remove(&id)
                    .context("agent not installed")?;
                params["expected_revision"] = item.installation_revision.into();
                params["target_scope"] = if library.root() == AgentLibrary::new(home).root() {
                    "local"
                } else {
                    "private"
                }
                .into();
            }
            None if matches!(operation, "save" | "import" | "install") => {}
            None => bail!("agent not installed"),
        }
    }
    Ok(params)
}
