//! Full, version-bound review pages. Snapshot summaries must never be used here.
use super::commands::{envelope_session, open_session, payload_string};
use super::*;
use sha2::{Digest, Sha256};
use willdeep_runtime_protocol::PendingApproval;

const PAGE_CHARACTERS: usize = 8192;
const MAX_REVIEW_BYTES: usize = 4 * 1024 * 1024;

pub(super) fn revision(approval: &PendingApproval) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{}:{}:{}",
                approval.id, approval.task_id, approval.description
            )
            .as_bytes()
        )
    )
}

pub(super) fn page(approval: &PendingApproval, payload: &Value) -> Result<Value, CommandError> {
    let revision = revision(approval);
    page_text(
        &approval.id.to_string(),
        "tool",
        "Approval",
        &approval.description,
        &revision,
        payload,
    )
}

fn page_text(
    id: &str,
    kind: &str,
    title: &str,
    text: &str,
    revision: &str,
    payload: &Value,
) -> Result<Value, CommandError> {
    if payload
        .get("revision")
        .and_then(Value::as_str)
        .is_some_and(|old| old != revision)
    {
        return Err(CommandError::new("Review changed; reload before deciding."));
    }
    let content = super::projection::redact_preserving_layout(text);
    if content.len() > MAX_REVIEW_BYTES {
        return Err(CommandError::new(
            "Review exceeds 4 MiB; review on the computer.",
        ));
    }
    let chars = content.chars().collect::<Vec<_>>();
    let offset = match payload.get("offset") {
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| CommandError::new("Invalid review offset."))?,
        None => 0,
    };
    if offset > chars.len() {
        return Err(CommandError::new("Invalid review offset."));
    }
    let end = offset.saturating_add(PAGE_CHARACTERS).min(chars.len());
    Ok(json!({
        "target_id": id, "target_kind": kind, "title": title,
        "revision": revision, "content": chars[offset..end].iter().collect::<String>(),
        "offset": offset, "next_offset": (end < chars.len()).then_some(end),
        "total_characters": chars.len(), "complete": end == chars.len(),
        "batch_count": 1, "batch_collecting": false,
        "redacted": content != text,
    }))
}

impl Gateway {
    pub(super) async fn worktree_list(
        &mut self,
        server: &ServerState,
        envelope: &PhoneEnvelope,
    ) -> Result<Vec<Value>, CommandError> {
        let id = envelope_session(envelope)?
            .ok_or_else(|| CommandError::new("worktree.list needs a session_id."))?;
        let session = open_session(server, id).await?;
        let workspace = session
            .workspace
            .ok_or_else(|| CommandError::new("Session has no workspace."))?;
        let snapshot = diff_review::unified_snapshot(
            server,
            willdeep_runtime_protocol::DiffSnapshotParams {
                workspace: workspace.clone(),
            },
        )
        .await
        .map_err(|_| CommandError::new("Workspace diff is unavailable."))?;
        let files = snapshot
            .files
            .iter()
            .map(|file| {
                json!({"path":file.path, "kind":file.kind,
            "added_lines":file.additions, "deleted_lines":file.deletions})
            })
            .collect::<Vec<_>>();
        let value = json!({"session_id":id, "repository_root":workspace, "file_count":files.len(),
            "total_added_lines":snapshot.additions, "total_deleted_lines":snapshot.deletions, "files":files});
        self.worktree_cache = Some((id, value.clone()));
        Ok(vec![super::projection::reply(
            envelope.id.as_deref(),
            "worktree.updated",
            Some(id),
            value,
        )])
    }

    pub(super) async fn review(
        &mut self,
        server: &ServerState,
        envelope: &PhoneEnvelope,
    ) -> Result<Vec<Value>, CommandError> {
        let session_id = envelope_session(envelope)?
            .ok_or_else(|| CommandError::new("review.get needs a session_id."))?;
        let session = open_session(server, session_id).await?;
        if envelope.payload.get("target_kind").and_then(Value::as_str) == Some("worktree") {
            let workspace = session
                .workspace
                .as_deref()
                .ok_or_else(|| CommandError::new("Session has no workspace."))?;
            let path = envelope
                .payload
                .get("id")
                .and_then(Value::as_str)
                .filter(|path| !path.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| CommandError::new("Diff path is required."))?;
            let params = willdeep_runtime_protocol::DiffSnapshotParams {
                workspace: workspace.into(),
            };
            let before = diff_review::unified_snapshot(server, params.clone())
                .await
                .map_err(|_| CommandError::new("Workspace diff is unavailable."))?;
            if !before.files.iter().any(|file| file.path == path) {
                return Err(CommandError::new("File is not in this workspace diff."));
            }
            let raw = diff_review::full_file_diff(Path::new(workspace), &path, MAX_REVIEW_BYTES)
                .map_err(|error| CommandError::new(error.to_string()))?;
            let after = diff_review::unified_snapshot(server, params)
                .await
                .map_err(|_| CommandError::new("Workspace diff is unavailable."))?;
            if before.id != after.id {
                return Err(CommandError::new("Workspace changed; reload the diff."));
            }
            let revision = format!(
                "{:x}",
                Sha256::digest(format!("{}:{}:{}", before.id, path, raw))
            );
            return Ok(vec![super::projection::reply(
                envelope.id.as_deref(),
                "review.detail",
                Some(session_id),
                page_text(&path, "worktree", &path, &raw, &revision, &envelope.payload)?,
            )]);
        }
        if envelope.payload.get("target_kind").and_then(Value::as_str) != Some("tool") {
            return Err(CommandError::new(
                "This host does not support pending patch reviews.",
            ));
        }
        let id = payload_string(&envelope.payload, &["id"])
            .and_then(|value| value.parse::<uuid::Uuid>().ok())
            .ok_or_else(|| CommandError::new("review.get needs an approval id."))?;
        let approvals: Vec<PendingApproval> =
            call(server, "approval.list", json!({}), None).await?;
        let approval = approvals
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| CommandError::new("This approval is no longer pending."))?;
        if self.task_session(server, approval.task_id).await != Some(session_id) {
            return Err(CommandError::new(
                "Approval does not belong to this session.",
            ));
        }
        Ok(vec![super::projection::reply(
            envelope.id.as_deref(),
            "review.detail",
            Some(session_id),
            page(&approval, &envelope.payload)?,
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approval(description: String) -> PendingApproval {
        PendingApproval {
            parked: false,
            id: uuid::Uuid::new_v4(),
            task_id: uuid::Uuid::new_v4(),
            description,
            always_allow_available: false,
            created_at: 1,
        }
    }

    #[test]
    fn pages_preserve_unicode_and_command_tail() {
        let value = approval(format!("{}COMMAND_END", "中文👩🏽‍💻\n".repeat(4000)));
        let mut offset = 0;
        let mut result = String::new();
        loop {
            let page = page(
                &value,
                &json!({"offset": offset, "revision": revision(&value)}),
            )
            .unwrap();
            result.push_str(page["content"].as_str().unwrap());
            if page["complete"] == true {
                break;
            }
            let next = page["next_offset"].as_u64().unwrap();
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(result, value.description);
    }

    #[test]
    fn stale_and_invalid_pages_are_rejected() {
        let value = approval("command".into());
        for params in [
            json!({"revision":"old"}),
            json!({"offset": -1}),
            json!({"offset": 999}),
        ] {
            assert!(page(&value, &params).is_err());
        }
    }

    #[test]
    fn secrets_are_redacted_and_oversize_is_explicit() {
        let value = approval(
            "curl -H 'Authorization: Bearer sk-abcdefghijklmnopqrstuvwxyz' https://example.test"
                .into(),
        );
        let response = page(&value, &json!({})).unwrap();
        assert!(
            !response["content"]
                .as_str()
                .unwrap()
                .contains("sk-abcdefghijklmnopqrstuvwxyz")
        );
        assert!(page(&approval("x".repeat(MAX_REVIEW_BYTES + 1)), &json!({})).is_err());
    }
}
