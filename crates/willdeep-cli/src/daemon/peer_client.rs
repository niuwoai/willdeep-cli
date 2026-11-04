use super::*;

const PEER_REQUEST_ATTEMPTS: usize = 2;

pub(crate) async fn peer_operation(
    home: &Path,
    operation: &str,
    params: serde_json::Value,
    request_id: Option<uuid::Uuid>,
) -> Result<serde_json::Value> {
    anyhow::ensure!(
        matches!(
            operation,
            "session.peers"
                | "session.peer.send"
                | "session.peer.inbox"
                | "session.peer.read"
                | "session.child.start"
                | "session.child.continue"
                | "session.child.retry"
                | "session.child.task.get"
                | "session.child.task.list"
                | "session.child.task.stop"
        ),
        "unsupported coordination operation"
    );
    let request_id = request_id.unwrap_or_else(uuid::Uuid::new_v4);
    let params = &params;
    request_with_retry(request_id, |id| async move {
        let state = state_for_operation(home, operation, params).await?;
        Ok(runtime_client(&state)?
            .call::<_, serde_json::Value>(operation, params, Some(id))
            .await?)
    })
    .await
}

async fn request_with_retry<F, Fut>(
    request_id: uuid::Uuid,
    mut call: F,
) -> Result<serde_json::Value>
where
    F: FnMut(uuid::Uuid) -> Fut,
    Fut: std::future::Future<
            Output = Result<willdeep_runtime_protocol::ApiResponse<serde_json::Value>>,
        >,
{
    let mut last_error = None;
    for _ in 0..PEER_REQUEST_ATTEMPTS {
        match call(request_id).await {
            Ok(response) => {
                return response
                    .into_result()
                    .map_err(|error| anyhow::anyhow!("Coordination request refused: {error:?}"));
            }
            Err(error) => last_error = Some(error),
        }
    }
    bail!(
        "Coordination request was not confirmed (request_id={request_id}). Before resending a note, check its inbox status; do not assume it was not queued. Last transport error: {}",
        last_error
            .map(|error| error.to_string())
            .unwrap_or_default()
    )
}

pub(crate) struct PeerSessionClient {
    pub home: PathBuf,
    pub session_id: uuid::Uuid,
    pub turn_id: Option<uuid::Uuid>,
}

#[async_trait]
impl willdeep_core::tools::SessionCollaboration for PeerSessionClient {
    fn pending(
        &self,
    ) -> std::result::Result<
        Vec<willdeep_core::peer_sessions::PeerMessage>,
        willdeep_core::tools::ToolError,
    > {
        let session = willdeep_core::SessionStore::new(&self.home)
            .load(self.session_id)
            .map_err(|error| willdeep_core::tools::ToolError::Network(error.to_string()))?;
        let project = willdeep_core::peer_sessions::project_key(&session.workspace)
            .map_err(|error| willdeep_core::tools::ToolError::Network(error.to_string()))?;
        willdeep_core::peer_sessions::PeerMessageStore::new(&self.home)
            .list(self.session_id)
            .map(|messages| {
                messages
                    .into_iter()
                    .filter(|message| {
                        message.to_session == self.session_id
                            && message.consumed_at.is_none()
                            && message.project_key == project
                    })
                    .collect()
            })
            .map_err(|error| willdeep_core::tools::ToolError::Network(error.to_string()))
    }
    fn consumed(
        &self,
        ids: &[uuid::Uuid],
    ) -> std::result::Result<(), willdeep_core::tools::ToolError> {
        let store = willdeep_core::peer_sessions::PeerMessageStore::new(&self.home);
        for id in ids {
            store
                .mark_consumed(self.session_id, *id, now())
                .map_err(|error| willdeep_core::tools::ToolError::Network(error.to_string()))?;
        }
        Ok(())
    }
    fn host_context(
        &self,
    ) -> std::result::Result<Vec<(uuid::Uuid, String)>, willdeep_core::tools::ToolError> {
        child_sessions::ChildStore::new(&self.home)
            .host_context(self.session_id)
            .map_err(|error| willdeep_core::tools::ToolError::Network(error.to_string()))
    }

    async fn execute_call(
        &self,
        call: &willdeep_core::ToolCall,
    ) -> std::result::Result<String, willdeep_core::tools::ToolError> {
        let arguments = serde_json::from_str(&call.arguments).map_err(|error| {
            willdeep_core::tools::ToolError::Network(format!(
                "invalid collaboration arguments: {error}"
            ))
        })?;
        if matches!(
            call.name.as_str(),
            "start_session" | "continue_session" | "retry_session_task" | "get_session_task"
        ) {
            let turn = self.turn_id.ok_or_else(|| {
                willdeep_core::tools::ToolError::Network(
                    "child tasks require a Runtime Turn".into(),
                )
            })?;
            let request_id = uuid::Uuid::new_v5(&turn, call.id.as_bytes());
            return self
                .execute_child(&call.name, arguments, request_id)
                .await
                .map_err(|error| willdeep_core::tools::ToolError::Network(error.to_string()));
        }
        self.execute(&call.name, arguments).await
    }

    async fn execute(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> std::result::Result<String, willdeep_core::tools::ToolError> {
        let result = async {
            let (operation, params) = match name {
                "list_workspace_sessions" => ("session.peers", serde_json::json!({"id": self.session_id})),
                "read_session_messages" => ("session.peer.inbox", serde_json::json!({"session_id": self.session_id})),
                "send_session_message" => {
                    let target = arguments.get("session_id").and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("session_id is required"))?;
                    let message = arguments.get("message").and_then(serde_json::Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("message is required"))?;
                    ("session.peer.send", serde_json::json!({"source_session_id": self.session_id,
                        "session_id": target, "message": message, "reply_to": arguments.get("reply_to")}))
                }
                _ => bail!("Unknown coordination tool"),
            };
            let value = peer_operation(&self.home, operation, params, None).await?;
            Ok::<_, anyhow::Error>(serde_json::to_string(&value)?)
        }.await;
        result.map_err(|error| willdeep_core::tools::ToolError::Network(error.to_string()))
    }
}

impl PeerSessionClient {
    async fn execute_child(
        &self,
        name: &str,
        arguments: serde_json::Value,
        request_id: uuid::Uuid,
    ) -> Result<String> {
        let operation = match name {
            "start_session" => "session.child.start",
            "continue_session" => "session.child.continue",
            "retry_session_task" => "session.child.retry",
            "get_session_task" => "session.child.task.get",
            _ => bail!("unknown child session tool"),
        };
        let mut params = arguments
            .as_object()
            .cloned()
            .context("child tool arguments must be an object")?;
        anyhow::ensure!(
            !params.contains_key("parent_session_id")
                && !params.contains_key("created_from_turn_id"),
            "child task source is bound by the host"
        );
        params.insert(
            "parent_session_id".into(),
            serde_json::to_value(self.session_id)?,
        );
        if name != "get_session_task" {
            params.insert(
                "created_from_turn_id".into(),
                serde_json::to_value(self.turn_id.context("child tool requires a Runtime Turn")?)?,
            );
        }
        let value = peer_operation(&self.home, operation, params.into(), Some(request_id)).await?;
        Ok(serde_json::to_string(&value)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use willdeep_core::peer_sessions::{PeerMessage, PeerMessageStore, PeerOrigin};
    use willdeep_runtime_protocol::{ApiResponse, ErrorCode};

    #[tokio::test]
    async fn lost_ack_retry_uses_one_durable_message_identity() {
        let root = std::env::temp_dir().join(format!("peer-retry-{}", uuid::Uuid::new_v4()));
        let store = PeerMessageStore::new(&root);
        let source = uuid::Uuid::new_v4();
        let target = uuid::Uuid::new_v4();
        let request = uuid::Uuid::new_v4();
        let mut seen = Vec::new();
        let response = request_with_retry(request, |id| {
            seen.push(id);
            let note = store
                .enqueue(PeerMessage {
                    id,
                    from_session: source,
                    to_session: target,
                    project_key: root.clone(),
                    origin: PeerOrigin::Model,
                    body: "keep the original commit".into(),
                    reply_to: None,
                    created_at: 1,
                    consumed_at: None,
                    read_by: Default::default(),
                })
                .unwrap();
            std::future::ready(if seen.len() == 1 {
                Err(anyhow::anyhow!("ack lost after durable write"))
            } else {
                Ok(ApiResponse::ok(
                    serde_json::to_value(note).unwrap(),
                    "fixture",
                    Some(id),
                ))
            })
        })
        .await
        .unwrap();
        assert_eq!(seen, vec![request, request]);
        assert_eq!(response["id"], request.to_string());
        assert!(response["consumed_at"].is_null());
        assert_eq!(PeerMessageStore::new(&root).list(target).unwrap().len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn confirmed_refusal_does_not_retry() {
        let mut attempts = 0;
        let result = request_with_retry(uuid::Uuid::new_v4(), |id| {
            attempts += 1;
            std::future::ready(Ok(ApiResponse::error(
                ErrorCode::Forbidden,
                "outside project",
                false,
                "fixture",
                Some(id),
            )))
        })
        .await;
        assert!(result.unwrap_err().to_string().contains("outside project"));
        assert_eq!(attempts, 1);
    }

    #[tokio::test]
    async fn exhausted_transport_failure_preserves_uncertainty_and_request_identity() {
        let id = uuid::Uuid::new_v4();
        let error = request_with_retry(id, |_| {
            std::future::ready(Err::<ApiResponse<serde_json::Value>, _>(anyhow::anyhow!(
                "socket closed"
            )))
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains(&id.to_string()));
        assert!(error.contains("do not assume it was not queued"));
    }
}
