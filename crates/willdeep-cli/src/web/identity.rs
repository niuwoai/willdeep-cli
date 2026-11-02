//! 实例身份的 Web 接口。
//!
//! 只有回环 Web 能读能改：身份是这台机器的标识，和公网入口、分享一样，
//! 远程连上来的浏览器不该看到它，更不该替这台机器改名。

use super::*;

pub(super) fn routes() -> Router<Arc<WebState>> {
    Router::new()
        .route("/api/identity", get(read))
        .route("/api/identity/name", post(rename))
}

#[derive(Serialize)]
struct IdentityView {
    id: String,
    name: String,
    created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    renamed_at: Option<u64>,
}

impl From<crate::identity::Identity> for IdentityView {
    fn from(identity: crate::identity::Identity) -> Self {
        Self {
            id: identity.id.to_string(),
            name: identity.name,
            created_at: identity.created_at,
            renamed_at: identity.renamed_at,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameRequest {
    name: String,
}

async fn read(State(state): State<Arc<WebState>>) -> Result<Json<IdentityView>, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "the instance identity is available only from a loopback Web server",
        ));
    }
    let identity = crate::identity::load(&state.home).map_err(WebError::from_anyhow)?;
    Ok(Json(identity.into()))
}

/// 改名。首次打开页面时 GET 已经生成了身份；POST 只负责改名字，id 不动。
async fn rename(
    State(state): State<Arc<WebState>>,
    Json(request): Json<RenameRequest>,
) -> Result<Json<IdentityView>, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "the instance identity is available only from a loopback Web server",
        ));
    }
    let identity = crate::identity::rename(&state.home, &request.name)
        .map_err(|error| WebError::bad_request(error.to_string()))?;
    Ok(Json(identity.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_serializes_the_id_as_a_string() {
        let identity = crate::identity::Identity {
            schema: crate::identity::IDENTITY_SCHEMA.to_owned(),
            id: uuid::Uuid::parse_str("6f0a1111-2222-3333-4444-55550abc1234").unwrap(),
            name: "alex".to_owned(),
            created_at: 7,
            renamed_at: Some(9),
        };
        let view: IdentityView = identity.into();
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["id"], "6f0a1111-2222-3333-4444-55550abc1234");
        assert_eq!(json["name"], "alex");
        assert_eq!(json["renamed_at"], 9);
    }
}
