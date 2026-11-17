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

const MAX_REFERENCES_PER_KIND: usize = 6;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composer_references::Reference;

    fn reference(kind: &'static str, name: &str) -> Reference {
        Reference {
            kind,
            name: name.to_owned(),
            value: if kind == "peer" {
                format!("@{name}")
            } else {
                format!("@{kind}:{name}")
            },
        }
    }

    #[test]
    fn members_are_matched_before_limiting_and_respect_explicit_types() {
        let mut peers: Vec<_> = (0..MAX_REFERENCES_PER_KIND)
            .map(|index| reference("peer", &format!("member-{index}")))
            .collect();
        peers.push(reference("peer", "Zebra"));
        let entries = vec![
            reference("file", "README.md"),
            reference("agent", "reviewer"),
            reference("mcp", "search"),
        ];
        let matches = visible_references(&peers, &entries, "zEbRa");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].value, "@Zebra");
        assert!(visible_references(&peers, &entries, "zzz").is_empty());
        for (query, kind) in [
            ("file:README", "file"),
            ("agent:reviewer", "agent"),
            ("mcp:search", "mcp"),
        ] {
            let matches = visible_references(&peers, &entries, query);
            assert_eq!(matches.len(), 1, "{query}");
            assert_eq!(matches[0].kind, kind, "{query}");
        }
        let matches = visible_references(&peers, &entries, "");
        assert_eq!(matches.len(), MAX_REFERENCES_PER_KIND + entries.len());
        assert!(
            matches[..MAX_REFERENCES_PER_KIND]
                .iter()
                .all(|entry| entry.kind == "peer")
        );
        assert_eq!(
            matches[MAX_REFERENCES_PER_KIND..]
                .iter()
                .map(|entry| entry.kind)
                .collect::<Vec<_>>(),
            ["mcp", "agent", "file"]
        );
    }
}

fn visible_references(
    peers: &[crate::composer_references::Reference],
    entries: &[crate::composer_references::Reference],
    query: &str,
) -> Vec<crate::composer_references::Reference> {
    let mut matches = crate::composer_references::reference_matches(query);
    let mut references = Vec::new();
    // 群成员在最前；每类先按统一规则过滤，再截取候选，保留原始插入协议。
    for kind in ["peer", "mcp", "agent", "file"] {
        references.extend(
            peers
                .iter()
                .chain(entries.iter())
                .filter(|entry| entry.kind == kind && matches(entry))
                .take(MAX_REFERENCES_PER_KIND)
                .cloned(),
        );
    }
    references
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
            Ok::<_, anyhow::Error>(visible_references(&peers, &all, &query))
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
