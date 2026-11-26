use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as RoutePath, Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use willdeep_core::{SessionStore, tools::read_workspace_preview};

use super::{WebError, WebState, session_workspace_allowed};

#[derive(Deserialize)]
pub(super) struct FileQuery {
    path: String,
}

#[derive(Serialize)]
struct FilePreview {
    content: String,
}

pub(super) async fn get(
    State(state): State<Arc<WebState>>,
    RoutePath(id): RoutePath<Uuid>,
    Query(query): Query<FileQuery>,
) -> Result<Response, WebError> {
    let unavailable = || WebError::not_found("workspace file cannot be previewed");
    let home = state.home.clone();
    let session = tokio::task::spawn_blocking(move || SessionStore::new(&home).load(id))
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())?;
    if !session_workspace_allowed(&state, &session).await? {
        return Err(unavailable());
    }
    if Path::new(&query.path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_none_or(|name| crate::daemon::diff_review::sensitive_path(&name.to_ascii_lowercase()))
    {
        return Err(unavailable());
    }
    let content = tokio::task::spawn_blocking(move || {
        read_workspace_preview(&session.workspace, Path::new(&query.path))
    })
    .await
    .map_err(|_| unavailable())?
    .map_err(|_| unavailable())?;

    Ok(file_response(content))
}

fn file_response(content: String) -> Response {
    (
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        Json(FilePreview { content }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn workspace_file_preview_sends_inert_json_and_disables_caching() {
        let source = "<script>console.info('preview')</script>";
        let response = file_response(source.to_owned());
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["content"], source);
    }
}
