//! 手机命令 → Runtime 操作。
//!
//! 每个分支只做三件事：从信封里取参数、调一个白名单里的 Runtime 操作、把结果翻成
//! 手机端认识的信封。业务判断（会话能不能接轮次、审批还在不在等）全在 Runtime 里，
//! 这里不重复。

use std::io::Cursor;

use base64::Engine;
use willdeep_runtime_protocol::{
    MessageAttachment, PendingApproval, PendingQuestion, RuntimeInteractionResult, RuntimeSession,
    RuntimeTurn, RuntimeWorkspace as PublicWorkspace, SessionStatus,
};

use super::projection::{ack, command_error, reply, session_json, tool_updated, workspace_name};
use super::*;

/// 手机上的一张照片动辄 10 MB 以上；解码前先按原始字节数挡一道。
const MAX_PHONE_IMAGE_BYTES: usize = 25 * 1024 * 1024;

impl Gateway {
    pub(super) async fn handle_command(
        &mut self,
        server: &ServerState,
        envelope: PhoneEnvelope,
    ) -> Vec<Value> {
        let id = envelope.id.clone();
        let kind = envelope.kind.clone();
        let result = match kind.as_str() {
            "session.list" => return vec![self.snapshot(server, id.as_deref()).await],
            "session.peers" | "session.peer.send" | "session.peer.inbox" | "session.peer.read" => {
                self.peer_operation(server, &envelope).await
            }
            "session.select" => self.select_session(server, &envelope).await,
            "workspace.list" => self.workspace_list(server, &envelope).await,
            "capabilities.get" => self.capabilities(server, &envelope).await,
            "session.configure" => self.configure(server, &envelope).await,
            "message.send" => self.send_message(server, &envelope).await,
            "session.create" => self.create_session(server, &envelope).await,
            "turn.stop" => self.stop_turn(server, &envelope).await,
            "tool.decide" => self.decide_tool(server, &envelope).await,
            "review.get" => self.review(server, &envelope).await,
            "worktree.list" => self.worktree_list(server, &envelope).await,
            "queue.update" => self.queue_update(server, &envelope).await,
            // 与 macOS 桌面端 `AgentMobileGatewayCommandError.unsupportedCommand` 逐字一致：
            // 手机端靠这个前缀区分「这条命令不支持」和「连接出错」。
            _ => Err(CommandError::new(format!(
                "Unsupported mobile command: {kind}."
            ))),
        };
        match result {
            Ok(outgoing) => outgoing,
            Err(error) => vec![command_error(id.as_deref(), &kind, &error.0)],
        }
    }

    async fn peer_operation(
        &mut self,
        server: &ServerState,
        envelope: &PhoneEnvelope,
    ) -> Result<Vec<Value>, CommandError> {
        let source = envelope_session(envelope)?
            .ok_or_else(|| CommandError::new("Select a session before coordinating."))?;
        open_session(server, source).await?;
        let mut params = envelope.payload.clone();
        if !params.is_object() {
            return Err(CommandError::new("Invalid coordination request."));
        }
        match envelope.kind.as_str() {
            "session.peers" => params = json!({"id": source}),
            "session.peer.send" => params["source_session_id"] = json!(source),
            _ => params["session_id"] = json!(source),
        }
        let operation = match envelope.kind.as_str() {
            "session.peers" => "session.peers",
            "session.peer.send" => "session.peer.send",
            "session.peer.inbox" => "session.peer.inbox",
            "session.peer.read" => "session.peer.read",
            _ => return Err(CommandError::new("Unsupported coordination operation.")),
        };
        let result: Value = call(
            server,
            operation,
            params,
            Some(envelope_request_id(envelope)),
        )
        .await?;
        Ok(vec![reply(
            envelope.id.as_deref(),
            "session.peer.result",
            Some(source),
            json!({"operation": envelope.kind, "data": result}),
        )])
    }

    async fn select_session(
        &mut self,
        server: &ServerState,
        envelope: &PhoneEnvelope,
    ) -> Result<Vec<Value>, CommandError> {
        let session_id = envelope_session(envelope)?
            .ok_or_else(|| CommandError::new("session.select needs a session_id."))?;
        let session = open_session(server, session_id).await?;
        self.selected = Some(session.id);
        Ok(vec![
            ack(
                envelope.id.as_deref(),
                "session.select",
                Some(session.id),
                None,
            ),
            self.snapshot(server, None).await,
        ])
    }

    async fn workspace_list(
        &mut self,
        server: &ServerState,
        envelope: &PhoneEnvelope,
    ) -> Result<Vec<Value>, CommandError> {
        let workspaces: Vec<PublicWorkspace> =
            call(server, "workspace.list", json!({}), None).await?;
        let sessions: Vec<RuntimeSession> = call(server, "session.list", json!({}), None).await?;
        let current = match self.selected {
            Some(selected) => sessions
                .iter()
                .find(|session| session.id == selected)
                .and_then(|session| session.workspace.clone()),
            None => None,
        };
        let workspaces = workspaces
            .into_iter()
            .filter_map(|workspace| {
                let root = workspace.root?;
                let session_count = sessions
                    .iter()
                    .filter(|session| {
                        session.status != SessionStatus::Archived
                            && session.workspace.as_deref() == Some(root.as_str())
                    })
                    .count();
                Some(json!({
                    "path": root,
                    "name": workspace.name,
                    "session_count": session_count,
                    "is_current": current.as_deref() == Some(root.as_str()),
                    "last_used_at": willdeep_core::session::format_iso8601(workspace.updated_at),
                }))
            })
            .collect::<Vec<_>>();
        Ok(vec![reply(
            envelope.id.as_deref(),
            "workspace.list",
            None,
            json!({ "workspaces": workspaces }),
        )])
    }

    async fn capabilities(
        &mut self,
        server: &ServerState,
        envelope: &PhoneEnvelope,
    ) -> Result<Vec<Value>, CommandError> {
        if let Some(id) = envelope_session(envelope)? {
            let session = open_session(server, id).await?;
            self.selected = Some(session.id);
        }
        // 首次连接也先确定选中会话，避免请求时序导致空目录。
        if self.selected.is_none() {
            let _ = self.snapshot(server, None).await;
        }
        let payload =
            super::run_context::capabilities(server, self.selected, &envelope.payload).await?;
        Ok(vec![reply(
            envelope.id.as_deref(),
            "capabilities.updated",
            self.selected,
            payload,
        )])
    }

    async fn configure(
        &mut self,
        server: &ServerState,
        envelope: &PhoneEnvelope,
    ) -> Result<Vec<Value>, CommandError> {
        let id = envelope_session(envelope)?
            .or(self.selected)
            .ok_or_else(|| CommandError::new("Select a session before configuring it."))?;
        let mut params = envelope.payload.clone();
        params["id"] = json!(id);
        let _: Value = call(
            server,
            "session.configure",
            params,
            Some(envelope_request_id(envelope)),
        )
        .await?;
        self.selected = Some(id);
        let mut replies = vec![ack(
            envelope.id.as_deref(),
            "session.configure",
            Some(id),
            None,
        )];
        replies.extend(self.capabilities(server, envelope).await?);
        Ok(replies)
    }

    async fn send_message(
        &mut self,
        server: &ServerState,
        envelope: &PhoneEnvelope,
    ) -> Result<Vec<Value>, CommandError> {
        let text = payload_string(&envelope.payload, &["text", "content"]).unwrap_or_default();
        let mut attachments = phone_images(&envelope.payload).await?;
        if text.is_empty() && attachments.is_empty() {
            return Err(CommandError::new("Message text is empty."));
        }
        let request_id = envelope_request_id(envelope);
        let session_id = self.target_session(server, envelope, request_id).await?;
        let image_count = attachments.len();
        // 旧手机随消息带配置；先校验并落盘，再提交，绝不静默忽略。
        if super::run_context::has_context(&envelope.payload) {
            let mut params = envelope.payload.clone();
            params["id"] = json!(session_id);
            // 消息字段不是配置参数。
            let params = super::run_context::context_params(params);
            let _: Value = call(
                server,
                "session.configure",
                params,
                Some(uuid::Uuid::from_u128(
                    request_id.as_u128() ^ 0xc0ff_ee00_0000_0000_0000_0000_0000_0002,
                )),
            )
            .await?;
        }
        attachments.extend(super::run_context::selected_attachments(server, session_id).await?);
        let _turn: RuntimeTurn = call(
            server,
            "turn.submit",
            serde_json::to_value(willdeep_runtime_protocol::SubmitTurnParams {
                session_id,
                turn_request_id: request_id,
                prompt: text.clone(),
                attachments,
                origin_client: Some(self.origin_client.clone()),
            })
            .map_err(|_| CommandError::new("internal Runtime error"))?,
            Some(request_id),
        )
        .await?;
        self.selected = Some(session_id);
        if envelope.kind == "queue.update" {
            return Ok(vec![
                ack(
                    envelope.id.as_deref(),
                    &envelope.kind,
                    Some(session_id),
                    None,
                ),
                self.snapshot(server, None).await,
            ]);
        }
        let echo = self.push_user_echo(session_id, request_id, &text, image_count);
        Ok(vec![
            // 回执的类型跟着信封走（`message.send` 或 `queue.update`），与 macOS 桌面端一致。
            ack(
                envelope.id.as_deref(),
                &envelope.kind,
                Some(session_id),
                None,
            ),
            echo,
            self.snapshot(server, None).await,
        ])
    }

    /// Pending user messages share the Runtime queue and stable turn IDs.
    async fn queue_update(
        &mut self,
        server: &ServerState,
        envelope: &PhoneEnvelope,
    ) -> Result<Vec<Value>, CommandError> {
        let action = payload_string(&envelope.payload, &["action"])
            .unwrap_or_default()
            .to_ascii_lowercase()
            .replace('-', "_");
        if action == "add" {
            return self.send_message(server, envelope).await;
        }
        let action = match action.as_str() {
            "edit" | "update" => "edit",
            "remove" | "delete" => "remove",
            "clear" => "clear",
            "send_now" => "send_now",
            _ => return Err(CommandError::new("Unsupported queue action.")),
        };
        let session_id = envelope_session(envelope)?
            .or(self.selected)
            .ok_or_else(|| CommandError::new("Select a session before updating its queue."))?;
        let mut params = json!({"session_id": session_id, "action": action});
        if action != "clear" {
            let id = payload_string(&envelope.payload, &["message_id", "id", "queued_id"])
                .ok_or_else(|| CommandError::new("Queue action needs a message_id."))?;
            params["message_id"] = json!(id);
        }
        for key in ["text", "expected_text"] {
            if let Some(value) = envelope.payload.get(key) {
                params[key] = value.clone();
            }
        }
        let _: Value = call(
            server,
            "turn.queue.update",
            params,
            Some(envelope_request_id(envelope)),
        )
        .await?;
        Ok(vec![
            ack(
                envelope.id.as_deref(),
                "queue.update",
                Some(session_id),
                None,
            ),
            self.snapshot(server, None).await,
        ])
    }

    /// `message.send` 的目标会话，与 macOS 桌面端同一套顺序：
    /// 1. 信封带 `session_id` 就是它；
    /// 2. 带 `workspace_path`：选中会话就在这个工作区且空闲则复用，否则在那里新建；
    /// 3. 都没带：选中会话；还没有选中的就在登记表的活跃工作区新建。
    async fn target_session(
        &mut self,
        server: &ServerState,
        envelope: &PhoneEnvelope,
        request_id: uuid::Uuid,
    ) -> Result<uuid::Uuid, CommandError> {
        if let Some(session_id) = envelope_session(envelope)? {
            return Ok(open_session(server, session_id).await?.id);
        }
        if let Some(path) = payload_string(&envelope.payload, &["workspace_path"]) {
            let root = registered_workspace_root(server, &path).await?;
            if let Some(selected) = self.selected
                && let Ok(session) = open_session(server, selected).await
                && session.workspace.as_deref() == Some(root.as_str())
                && session.active_turn_id.is_none()
            {
                return Ok(session.id);
            }
            return Ok(new_session(server, &root, derived_request_id(request_id))
                .await?
                .id);
        }
        if let Some(selected) = self.selected
            && let Ok(session) = open_session(server, selected).await
        {
            return Ok(session.id);
        }
        let root = active_workspace_root(server).await?;
        Ok(new_session(server, &root, derived_request_id(request_id))
            .await?
            .id)
    }

    async fn create_session(
        &mut self,
        server: &ServerState,
        envelope: &PhoneEnvelope,
    ) -> Result<Vec<Value>, CommandError> {
        let root = match payload_string(&envelope.payload, &["workspace_path"]) {
            Some(path) => registered_workspace_root(server, &path).await?,
            None => active_workspace_root(server).await?,
        };
        let session = new_session(server, &root, envelope_request_id(envelope)).await?;
        self.selected = Some(session.id);
        Ok(vec![reply(
            envelope.id.as_deref(),
            "session.upsert",
            Some(session.id),
            json!({ "session": session_json(&session, None, true) }),
        )])
    }

    async fn stop_turn(
        &mut self,
        server: &ServerState,
        envelope: &PhoneEnvelope,
    ) -> Result<Vec<Value>, CommandError> {
        let session_id = envelope_session(envelope)?
            .or(self.selected)
            .ok_or_else(|| CommandError::new("No session is selected."))?;
        let session = open_session(server, session_id).await?;
        let turn_id = session
            .active_turn_id
            .ok_or_else(|| CommandError::new("This session has no running turn."))?;
        let _turn: RuntimeTurn = call(
            server,
            "turn.stop",
            json!({ "id": turn_id }),
            Some(envelope_request_id(envelope)),
        )
        .await?;
        Ok(vec![ack(
            envelope.id.as_deref(),
            "turn.stop",
            Some(session.id),
            None,
        )])
    }

    /// 审批只给「这一次允许 / 拒绝」：「总是允许」是长期规则，只能在桌面上定。
    async fn decide_tool(
        &mut self,
        server: &ServerState,
        envelope: &PhoneEnvelope,
    ) -> Result<Vec<Value>, CommandError> {
        let raw_id = payload_string(&envelope.payload, &["id", "tool_call_id", "approval_id"])
            .ok_or_else(|| CommandError::new("tool.decide needs an approval id."))?;
        let interaction_id = raw_id
            .parse::<uuid::Uuid>()
            .map_err(|_| CommandError::new("This approval is no longer pending."))?;
        let approved = approved_decision(&envelope.payload)?;
        let request_id = envelope_request_id(envelope);

        let approvals: Vec<PendingApproval> =
            call(server, "approval.list", json!({}), None).await?;
        if let Some(approval) = approvals
            .into_iter()
            .find(|approval| approval.id == interaction_id)
        {
            let session_id = self.task_session(server, approval.task_id).await;
            if envelope_session(envelope)?.is_some_and(|id| Some(id) != session_id) {
                return Err(CommandError::new(
                    "Approval does not belong to this session.",
                ));
            }
            if approved
                && envelope
                    .payload
                    .get("review_revision")
                    .and_then(Value::as_str)
                    .is_some_and(|expected| expected != super::review::revision(&approval))
            {
                return Err(CommandError::new("Review changed; reload before deciding."));
            }
            let _: RuntimeInteractionResult = call(
                server,
                "approval.resolve",
                json!({
                    "id": interaction_id,
                    "decision": if approved { "allow_once" } else { "deny" },
                }),
                Some(request_id),
            )
            .await?;
            let session_id = self.task_session(server, approval.task_id).await;
            return Ok(vec![
                ack(envelope.id.as_deref(), "tool.decide", session_id, None),
                tool_updated(
                    interaction_id,
                    if approved { "approved" } else { "rejected" },
                    session_id,
                ),
            ]);
        }

        let questions: Vec<PendingQuestion> =
            call(server, "question.list", json!({}), None).await?;
        if let Some(question) = questions
            .into_iter()
            .find(|question| question.id == interaction_id)
        {
            let answer = if approved {
                Some(
                    payload_string(&envelope.payload, &["answer"])
                        .ok_or_else(|| CommandError::new("An answer is required."))?,
                )
            } else {
                None
            };
            let _: RuntimeInteractionResult = call(
                server,
                "question.answer",
                json!({ "id": interaction_id, "answer": answer }),
                Some(request_id),
            )
            .await?;
            let session_id = self.task_session(server, question.task_id).await;
            return Ok(vec![
                ack(envelope.id.as_deref(), "tool.decide", session_id, None),
                tool_updated(
                    interaction_id,
                    if approved { "answered" } else { "dismissed" },
                    session_id,
                ),
            ]);
        }
        Err(CommandError::new("This approval is no longer pending."))
    }
}

/// 打开一条会话：必须存在且没有归档。
pub(super) async fn open_session(
    server: &ServerState,
    session_id: uuid::Uuid,
) -> Result<RuntimeSession, CommandError> {
    let session: RuntimeSession =
        call(server, "session.get", json!({ "id": session_id }), None).await?;
    if session.status == SessionStatus::Archived {
        return Err(CommandError::new("This session is archived."));
    }
    Ok(session)
}

async fn new_session(
    server: &ServerState,
    root: &str,
    request_id: uuid::Uuid,
) -> Result<RuntimeSession, CommandError> {
    call(
        server,
        "session.create",
        json!({
            "id": null,
            "workspace": root,
            "profile": null,
            "model": null,
            "title": null,
        }),
        Some(request_id),
    )
    .await
}

/// 手机给的路径必须已经在登记表里。`session.create` 碰到新目录会顺手登记，那等于
/// 让手机把 Runtime 的访问范围扩到任意目录——所以先在这里挡住。
async fn registered_workspace_root(
    server: &ServerState,
    path: &str,
) -> Result<String, CommandError> {
    let workspaces: Vec<PublicWorkspace> = call(server, "workspace.list", json!({}), None).await?;
    let wanted = std::fs::canonicalize(path)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| path.trim_end_matches('/').to_owned());
    workspaces
        .into_iter()
        .filter_map(|workspace| workspace.root)
        .find(|root| *root == wanted)
        .ok_or_else(|| {
            CommandError::new(format!(
                "Workspace {} is not registered in the Runtime. Register it on the desktop first.",
                workspace_name(path)
            ))
        })
}

async fn active_workspace_root(server: &ServerState) -> Result<String, CommandError> {
    let workspaces: Vec<PublicWorkspace> = call(server, "workspace.list", json!({}), None).await?;
    let mut roots = workspaces
        .iter()
        .filter(|workspace| workspace.root.is_some())
        .collect::<Vec<_>>();
    roots.sort_by_key(|workspace| (!workspace.active, std::cmp::Reverse(workspace.updated_at)));
    roots
        .first()
        .and_then(|workspace| workspace.root.clone())
        .ok_or_else(|| CommandError::new("No workspace is registered in the Runtime yet."))
}

pub(super) fn envelope_session(
    envelope: &PhoneEnvelope,
) -> Result<Option<uuid::Uuid>, CommandError> {
    let raw = envelope
        .session_id
        .clone()
        .or_else(|| payload_string(&envelope.payload, &["session_id"]));
    raw.map(|raw| {
        raw.parse::<uuid::Uuid>()
            .map_err(|_| CommandError::new("Unknown session."))
    })
    .transpose()
}

/// Preserve UUIDs and deterministically map legacy string IDs for safe retries.
fn envelope_request_id(envelope: &PhoneEnvelope) -> uuid::Uuid {
    match envelope.id.as_deref() {
        Some(id) => id
            .parse()
            .unwrap_or_else(|_| uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, id.as_bytes())),
        None => uuid::Uuid::new_v4(),
    }
}

/// 同一条手机命令里要连做两个写操作（先建会话再提交轮次）时，第二个不能复用
/// 同一个幂等键——键相同而操作不同会被判成冲突。派生一个确定的兄弟键，重发时仍然
/// 落在同一条缓存上。
pub(super) fn derived_request_id(request_id: uuid::Uuid) -> uuid::Uuid {
    uuid::Uuid::from_u128(request_id.as_u128() ^ 0x5eed_0000_0000_0000_0000_0000_0000_0001)
}

pub(super) fn payload_string(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        payload
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

/// 与 macOS 桌面端 `approvedDecision` 同一套取值。
pub(super) fn approved_decision(payload: &Value) -> Result<bool, CommandError> {
    if let Some(approved) = payload.get("approved").and_then(Value::as_bool) {
        return Ok(approved);
    }
    match payload_string(payload, &["decision"])
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("approve" | "approved" | "allow" | "confirm" | "yes" | "true") => Ok(true),
        Some("reject" | "rejected" | "deny" | "denied" | "no" | "false") => Ok(false),
        _ => Err(CommandError::new("tool.decide needs approve or reject.")),
    }
}

/// `payload.images` 里的 data URL → 图片附件。解码后限边、统一转 JPEG，与插件的
/// 图片附件同一口径：手机原图动辄四千像素，原样塞给模型既慢又贵。
async fn phone_images(payload: &Value) -> Result<Vec<MessageAttachment>, CommandError> {
    let Some(items) = payload.get("images").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    if items.len() > crate::plugin_ai_media::MAX_IMAGES {
        return Err(CommandError::new(format!(
            "At most {} images can be sent at once.",
            crate::plugin_ai_media::MAX_IMAGES
        )));
    }
    let urls = items.iter().filter_map(image_data_url).collect::<Vec<_>>();
    let mut attachments = Vec::with_capacity(urls.len());
    for (index, url) in urls.into_iter().enumerate() {
        let attachment = tokio::task::spawn_blocking(move || phone_image(&url, index + 1))
            .await
            .map_err(|_| CommandError::new("The image could not be processed."))??;
        attachments.push(attachment);
    }
    Ok(attachments)
}

fn image_data_url(value: &Value) -> Option<String> {
    let raw = value
        .as_str()
        .or_else(|| value.get("url").and_then(Value::as_str))
        .or_else(|| value.get("image_url").and_then(Value::as_str))
        .or_else(|| value.pointer("/image_url/url").and_then(Value::as_str))?;
    Some(raw.trim().to_owned())
}

pub(super) fn phone_image(
    data_url: &str,
    ordinal: usize,
) -> Result<MessageAttachment, CommandError> {
    let invalid =
        || CommandError::new("Images must be base64 data URLs of PNG, JPEG, GIF or WebP.");
    let rest = data_url.strip_prefix("data:").ok_or_else(invalid)?;
    let (metadata, data) = rest.split_once(',').ok_or_else(invalid)?;
    let mut parts = metadata.split(';');
    let media_type = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    if !media_type.starts_with("image/") || !parts.any(|part| part.eq_ignore_ascii_case("base64")) {
        return Err(invalid());
    }
    if data.len() > MAX_PHONE_IMAGE_BYTES / 3 * 4 {
        return Err(CommandError::new("The image is too large."));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|_| invalid())?;
    let decoded = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|_| invalid())?
        .decode()
        .map_err(|_| invalid())?;
    let limit = crate::plugin_ai_media::MAX_IMAGE_PIXELS;
    let fitted = if decoded.width().max(decoded.height()) > limit {
        decoded.resize(limit, limit, image::imageops::FilterType::Triangle)
    } else {
        decoded
    };
    // JPEG 不带透明通道，先压成 RGB，否则编码器直接报错。
    let rgb = fitted.to_rgb8();
    let mut encoded = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 85)
        .encode_image(&rgb)
        .map_err(|_| invalid())?;
    Ok(MessageAttachment::Image {
        name: format!("phone-{ordinal}.jpg"),
        media_type: "image/jpeg".to_owned(),
        data: base64::engine::general_purpose::STANDARD.encode(&encoded),
        width: rgb.width(),
        height: rgb.height(),
    })
}
