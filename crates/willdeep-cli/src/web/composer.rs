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
    let references = if let Some(query) = query.reference_query {
        let config = crate::config::LoadedConfig::load(Some(&state.config_path))
            .map_err(WebError::from_anyhow)?
            .file;
        let root = workspace.root.clone();
        let allowed = workspace.mcp_servers.clone();
        let home = state.home.clone();
        tokio::task::spawn_blocking(move || {
            let all = crate::composer_references::collect(&root, &home, &config, &allowed, &query)?;
            Ok::<_, anyhow::Error>(
                ["mcp", "agent", "file"]
                    .into_iter()
                    .flat_map(|kind| {
                        all.iter()
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
    let catalog = SkillCatalog::discover(&workspace.root, &[]).allow_only(&workspace.skills);
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
        commands: vec!["/help", "/goal", "/compress", "/skills", "/clear"],
        skills,
        references,
    }))
}
