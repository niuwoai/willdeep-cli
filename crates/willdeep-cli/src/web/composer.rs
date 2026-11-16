use super::{WebError, WebState, safe_skill_description, select_workspace};
use anyhow::Result;
use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use willdeep_core::SkillCatalog;

#[derive(Deserialize)]
pub(super) struct ComposerQuery {
    workspace: Option<String>,
    reference_query: Option<String>,
    /// 当前会话。只有群会话才会在引用列表最前面加上这个群的成员。
    session: Option<String>,
}

#[derive(Serialize)]
pub(super) struct ComposerData {
    commands: Vec<&'static str>,
    skills: Vec<ComposerSkill>,
    references: Vec<crate::composer_references::Reference>,
}

#[derive(Serialize)]
struct ComposerSkill {
    identifier: String,
    name: String,
    description: String,
}

pub(super) async fn get(
    State(state): State<Arc<WebState>>,
    Query(query): Query<ComposerQuery>,
) -> Result<Json<ComposerData>, WebError> {
    let workspace = select_workspace(&state, query.workspace.as_deref()).await?;
    let config = crate::config::LoadedConfig::load(Some(&state.config_path))
        .map_err(WebError::from_anyhow)?
        .file;
    let skill_roots = config.skills.roots.clone();
    let session_id = query
        .session
        .as_deref()
        .and_then(|id| uuid::Uuid::parse_str(id).ok());
    let references = if let Some(query) = query.reference_query {
        let root = workspace.root.clone();
        let allowed = workspace.mcp_servers.clone();
        let home = state.home.clone();
        tokio::task::spawn_blocking(move || {
            let peers = crate::composer_references::group_peers(&home, session_id);
            let all = crate::composer_references::collect(&root, &home, &config, &allowed, &query)?;
            Ok::<_, anyhow::Error>(
                // 群成员在最前，其余顺序不变。不是群会话时 peers 为空，结果与原来一致。
                std::iter::once("peer")
                    .chain(["mcp", "agent", "file"])
                    .flat_map(|kind| {
                        peers
                            .iter()
                            .chain(all.iter())
                            .filter(move |entry| entry.kind == kind)
                            .take(6)
                            .cloned()
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .await
        .map_err(|error| WebError::internal(error.to_string()))?
        .map_err(WebError::from_anyhow)?
    } else {
        Vec::new()
    };
    let catalog =
        SkillCatalog::discover(&workspace.root, &skill_roots).allow_only(&workspace.skills);
    let skills = catalog
        .list()
        .iter()
        .filter(|skill| !skill.identifier.starts_with("auto-"))
        .map(|skill| ComposerSkill {
            identifier: skill.identifier.clone(),
            name: skill.name.clone(),
            description: safe_skill_description(&skill.description),
        })
        .collect();
    Ok(Json(ComposerData {
        commands: vec![
            "/help",
            "/goal",
            "/permissions",
            "/model",
            "/compress",
            "/skills",
            "/clear",
            "/short",
            "/verbose",
        ],
        skills,
        references,
    }))
}
