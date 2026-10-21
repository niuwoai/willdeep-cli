//! 三端共用的移动运行配置。只暴露标识和目录，不上传凭据或本机路径。
use super::*;
use willdeep_core::{SkillCatalog, plugin::host::PluginHost};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextParams {
    id: uuid::Uuid,
    provider_id: Option<String>,
    model: Option<String>,
    approval_mode: Option<String>,
    skills: Option<Vec<String>>,
    plugins: Option<Vec<String>>,
    experts: Option<Vec<String>>,
}

fn loaded(
    server: &ServerState,
    id: uuid::Uuid,
) -> Result<(session_store::RuntimeSession, crate::config::LoadedConfig)> {
    let session = server.sessions.get(id)?.context("Session not found")?;
    let path = session
        .config
        .clone()
        .unwrap_or_else(|| server.home.join("config.toml"));
    let config = if session.config.is_some() || path.exists() {
        crate::config::LoadedConfig::load(Some(path.as_path()))?
    } else {
        crate::config::LoadedConfig {
            file: crate::config::ConfigFile::default(),
        }
    };
    Ok((session, config))
}

fn workspace(
    server: &ServerState,
    session: &session_store::RuntimeSession,
) -> Result<workspace_store::RuntimeWorkspace> {
    server
        .workspaces
        .list()?
        .into_iter()
        .find(|w| w.root == session.workspace)
        .context("Workspace is not registered")
}

fn effective_profile(
    session: &session_store::RuntimeSession,
    config: &crate::config::LoadedConfig,
    workspace: &workspace_store::RuntimeWorkspace,
) -> Option<String> {
    session
        .profile
        .clone()
        .or_else(|| workspace.provider_profile.clone())
        .or_else(|| config.file.default_provider.clone())
        .or_else(|| {
            (config.file.providers.len() == 1)
                .then(|| config.file.providers.keys().next().cloned())
                .flatten()
        })
}

fn catalog(
    session: &session_store::RuntimeSession,
    config: &crate::config::LoadedConfig,
    workspace: &workspace_store::RuntimeWorkspace,
) -> SkillCatalog {
    SkillCatalog::discover(&session.workspace, &config.file.skills.roots)
        .allow_only(&workspace.skills)
}

pub(super) fn has_context(value: &Value) -> bool {
    [
        "provider_id",
        "model",
        "approval_mode",
        "skills",
        "plugins",
        "experts",
    ]
    .iter()
    .any(|key| value.get(key).is_some())
}

pub(super) fn context_params(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.retain(|key, _| {
            [
                "id",
                "provider_id",
                "model",
                "approval_mode",
                "skills",
                "plugins",
                "experts",
            ]
            .contains(&key.as_str())
        });
    }
    value
}

pub(in crate::daemon) async fn configure_run_context(
    server: &ServerState,
    value: &Value,
) -> Result<Value> {
    let mut params: ContextParams = serde_json::from_value(value.clone())?;
    let (session, config) = loaded(server, params.id)?;
    let workspace = workspace(server, &session)?;
    if let Some(profile) = &params.provider_id {
        anyhow::ensure!(
            config.file.providers.contains_key(profile),
            "Unknown provider"
        );
        crate::provider_config_from_profile(&config.file, profile)?;
    }
    if let Some(model) = &params.model {
        anyhow::ensure!(
            !model.trim().is_empty() && model.len() <= 256 && !model.chars().any(char::is_control),
            "Invalid model"
        );
    }
    if params.provider_id.is_some() && params.model.is_none() {
        params.model = params
            .provider_id
            .as_ref()
            .and_then(|id| config.file.providers.get(id))
            .and_then(|p| p.model.clone());
    }
    let mode = params
        .approval_mode
        .as_deref()
        .map(parse_mode)
        .transpose()?;
    let skills = catalog(&session, &config, &workspace);
    if let Some(selected) = &params.skills {
        anyhow::ensure!(selected.len() <= 16, "Too many skills");
        for id in selected {
            anyhow::ensure!(
                skills.list().iter().any(|skill| skill.identifier == *id),
                "Unknown skill"
            );
            skills.read(id, None)?;
        }
    }
    anyhow::ensure!(
        params.experts.as_ref().is_none_or(Vec::is_empty),
        "Experts are not supported by this Runtime"
    );
    if let Some(selected) = &params.plugins {
        let host = PluginHost::discover(&server.home)?;
        for id in selected {
            anyhow::ensure!(
                host.is_enabled(id.trim_start_matches("plugin:")).await
                    && host
                        .approval_gap(id.trim_start_matches("plugin:"))
                        .await?
                        .is_none(),
                "Plugin must already be enabled and approved on desktop"
            );
        }
    }
    let configured = server.sessions.configure_mobile(
        params.id,
        params.provider_id,
        params.model,
        mode,
        params.skills,
        params.plugins,
    )?;
    if let Some(mode) = mode {
        server.tasks.approval_modes.update_session(params.id, mode);
    }
    server.events.append(
        "session.configuration_updated",
        format!("session_id={}", params.id),
    )?;
    Ok(json!({"id": configured.id}))
}

fn parse_mode(raw: &str) -> Result<WorkspaceAccess> {
    let raw = match raw {
        "ask_every_time" => "strict",
        "smart_approval" => "smart",
        "workspace_writable" => "workspace-write",
        other => other,
    };
    let mode = willdeep_core::ApprovalMode::parse(raw).context("Unknown approval mode")?;
    anyhow::ensure!(
        mode != willdeep_core::ApprovalMode::ReadOnly,
        "Read-only is a workspace policy"
    );
    Ok(WorkspaceAccess::from_approval_mode(mode))
}

pub(super) async fn capabilities(
    server: &ServerState,
    id: Option<uuid::Uuid>,
    request: &Value,
) -> Result<Value, CommandError> {
    build_capabilities(server, id, request)
        .await
        .map_err(|error| {
            let _ = error;
            eprintln!("mobile capability discovery failed for session {:?}", id);
            CommandError::new("Could not load desktop capabilities.")
        })
}

async fn build_capabilities(
    server: &ServerState,
    id: Option<uuid::Uuid>,
    request: &Value,
) -> Result<Value> {
    let Some(id) = id else {
        return Ok(
            json!({"providers": [], "models": [], "skills": [], "experts": [], "plugins": [], "supports_configuration": true, "review_kinds":["tool", "worktree"], "session_collaboration":true, "approval_modes": ["strict", "smart", "workspace-write", "full-access"]}),
        );
    };
    let (session, config) = loaded(server, id)?;
    let workspace = workspace(server, &session)?;
    let active_profile = effective_profile(&session, &config, &workspace);
    let queried_profile = request
        .get("provider_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| active_profile.clone());
    let profile = queried_profile
        .as_ref()
        .and_then(|id| config.file.providers.get(id));
    let active_model = session.model.clone().or_else(|| {
        active_profile
            .as_ref()
            .and_then(|id| config.file.providers.get(id))
            .and_then(|p| p.model.clone())
    });
    let providers = config
        .file
        .providers
        .keys()
        .map(|id| json!({"id": id, "title": id, "is_active": Some(id) == active_profile.as_ref()}))
        .collect::<Vec<_>>();
    let mut models = std::collections::BTreeSet::new();
    if let Some(profile) = profile {
        models.extend(profile.model.iter().cloned());
        models.extend(profile.vision_model.iter().cloned());
    }
    if queried_profile == active_profile {
        models.extend(active_model.iter().cloned());
    }
    let (remote_models, models_status) =
        provider_models(server, &config, queried_profile.as_deref()).await;
    models.extend(remote_models);
    let models = models.into_iter().map(|id| json!({"is_active": Some(&id) == active_model.as_ref(), "id": id, "title": id, "subtitle": queried_profile})).collect::<Vec<_>>();
    let catalog = catalog(&session, &config, &workspace);
    let skills = catalog.list().iter().map(|skill| json!({"id": skill.identifier, "title": skill.name, "subtitle": projection::clip(&skill.description, 600), "is_active": session.mobile_skills.contains(&skill.identifier)})).collect::<Vec<_>>();
    let host = PluginHost::discover(&server.home)?;
    let mut plugins = Vec::new();
    for package in host.packages() {
        if host.is_enabled(&package.id).await && host.approval_gap(&package.id).await?.is_none() {
            let id = format!("plugin:{}", package.id);
            plugins.push(json!({"id": id, "title": package.id, "is_active": session.mobile_plugins.contains(&id)}));
        }
    }
    let effective_mode = if workspace.access == WorkspaceAccess::ReadOnly {
        workspace.access
    } else {
        session.approval_mode.unwrap_or(workspace.access)
    };
    Ok(
        json!({"providers": providers, "models": models, "skills": skills, "plugins": plugins, "experts": [],
        "active_provider_id": active_profile, "active_model_id": active_model,
        "active_approval_mode": effective_mode.approval_mode().as_str(),
        "approval_modes": if workspace.access == WorkspaceAccess::ReadOnly { Vec::<&str>::new() } else { vec!["strict", "smart", "workspace-write", "full-access"] },
        "supports_configuration": true, "models_status": models_status, "experts_supported": false,
        "review_kinds":["tool", "worktree"], "session_collaboration":true}),
    )
}

/// 正文通过已有的文本附件通道进入模型请求，手机用户气泡仍只显示原始输入。
pub(super) async fn selected_attachments(
    server: &ServerState,
    id: uuid::Uuid,
) -> Result<Vec<willdeep_runtime_protocol::MessageAttachment>, CommandError> {
    let build = async {
        let (session, config) = loaded(server, id)?;
        let workspace = workspace(server, &session)?;
        let catalog = catalog(&session, &config, &workspace);
        let mut content = String::new();
        for id in &session.mobile_skills {
            content.push_str("\n\n<selected_skill>\n");
            content.push_str(&catalog.read(id, None)?);
            content.push_str("\n</selected_skill>");
        }
        if !session.mobile_plugins.is_empty() {
            let host = PluginHost::discover(&server.home)?;
            for id in &session.mobile_plugins {
                anyhow::ensure!(
                    host.is_enabled(id.trim_start_matches("plugin:")).await
                        && host
                            .approval_gap(id.trim_start_matches("plugin:"))
                            .await?
                            .is_none(),
                    "Selected plugin is no longer enabled"
                );
            }
            content.push_str(&format!(
                "\n\nUse these desktop-approved plugins for this task: {}",
                session.mobile_plugins.join(", ")
            ));
        }
        Ok::<_, anyhow::Error>(if content.is_empty() {
            Vec::new()
        } else {
            vec![willdeep_runtime_protocol::MessageAttachment::Text {
                name: "selected-skills-and-plugins.txt".to_owned(),
                content,
            }]
        })
    };
    build.await.map_err(|_| {
        CommandError::new("Selected skill or plugin is no longer available; refresh capabilities.")
    })
}

type ModelCache = HashMap<u64, (Instant, Vec<String>, &'static str)>;
static MODEL_CACHE: std::sync::OnceLock<tokio::sync::Mutex<ModelCache>> =
    std::sync::OnceLock::new();

/// 配置回读共用短缓存，不为每次技能勾选都阻塞网关去请求供应商。
async fn provider_models(
    server: &ServerState,
    config: &crate::config::LoadedConfig,
    profile: Option<&str>,
) -> (Vec<String>, &'static str) {
    use std::hash::{Hash, Hasher};
    let Some(profile) = profile else {
        return (Vec::new(), "configured");
    };
    let Ok(provider) = crate::provider_config_from_profile(&config.file, profile) else {
        return (Vec::new(), "unavailable");
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // 凭据只参与内存 hash，不进入缓存键文本、日志、文件或手机回包。
    (
        &server.home,
        profile,
        &provider.base_url,
        &provider.api_key,
        &provider.model,
        format!("{:?}", provider.kind),
    )
        .hash(&mut hasher);
    let key = hasher.finish();
    let cache = MODEL_CACHE.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()));
    if let Some((at, models, status)) = cache.lock().await.get(&key) {
        let ttl = if *status == "complete" {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(5)
        };
        if at.elapsed() < ttl {
            return (models.clone(), *status);
        }
    }
    let (models, status) = match tokio::time::timeout(
        Duration::from_secs(5),
        willdeep_core::provider::list_models(&provider),
    )
    .await
    {
        Ok(Ok(models)) => (models, "complete"),
        _ => (Vec::new(), "unavailable"),
    };
    let mut cache = cache.lock().await;
    if cache.len() >= 128 {
        cache.retain(|_, (at, _, _)| at.elapsed() < Duration::from_secs(60));
    }
    cache.insert(key, (Instant::now(), models.clone(), status));
    (models, status)
}
