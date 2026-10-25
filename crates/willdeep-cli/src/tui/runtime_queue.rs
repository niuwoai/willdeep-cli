use super::*;

pub(super) enum ParkedRemoteDialog {
    Approval(ApprovalRequest),
    Question(AskDialog),
}

pub(super) fn reconcile_parked_dialogs(app: &mut App) {
    let live = app
        .runtime_gates
        .iter()
        .map(crate::daemon::RemoteGate::id)
        .collect::<BTreeSet<_>>();
    app.parked_remote_dialogs.retain(|id, _| live.contains(id));
    app.requested_gate_presentations.retain(|id| {
        app.runtime_gates
            .iter()
            .any(|gate| gate.id() == *id && gate.parked())
    });
    if app.approval.is_none() && app.question.is_none() {
        app.active_remote_gate = None;
    }
    let Some(id) = app.active_remote_gate else {
        return;
    };
    if !app.requested_gate_presentations.contains(&id)
        && app
            .runtime_gates
            .iter()
            .any(|gate| gate.id() == id && gate.parked())
    {
        if let Some(request) = app.approval.take() {
            app.parked_remote_dialogs
                .insert(id, ParkedRemoteDialog::Approval(request));
        }
        if let Some(dialog) = app.question.take() {
            app.parked_remote_dialogs
                .insert(id, ParkedRemoteDialog::Question(dialog));
        }
        app.active_remote_gate = None;
        app.append_transcript(format!("System: {} · {}", app.language.text("待确认/待输入已收起，原请求保留；处理独立队列消息后恢复", "Pending confirmation/input parked; request retained and restored after independent queued work", "確認・入力待ちを折りたたみました。独立タスクの後に復帰します"), id));
    }
}

impl App {
    pub(super) fn sync_runtime_queue(
        &mut self,
        items: Vec<willdeep_runtime_protocol::QueuedTurnMessage>,
    ) {
        // Editing retains the original identity and expected text. The server
        // rejects the save if that message changed or started in another client.
        if self
            .prompt_queue
            .as_ref()
            .is_some_and(|panel| panel.editor.is_some())
        {
            return;
        }
        let selected = self
            .prompt_queue
            .as_ref()
            .and_then(|panel| self.queued_prompts.get(panel.selected))
            .and_then(|item| item.remote.as_ref())
            .map(|item| item.id);
        self.queued_prompts.retain(|item| item.remote.is_none());
        for item in items {
            self.queued_prompts.push_back(QueuedPrompt {
                text: item.text.clone(),
                attachments: Vec::new(),
                remote: Some(item),
            });
        }
        if let Some(panel) = self.prompt_queue.as_mut() {
            panel.selected = selected
                .and_then(|id| {
                    self.queued_prompts
                        .iter()
                        .position(|item| item.remote.as_ref().is_some_and(|item| item.id == id))
                })
                .unwrap_or_else(|| {
                    panel
                        .selected
                        .min(self.queued_prompts.len().saturating_sub(1))
                });
        }
    }
}

pub(super) async fn submit_input(
    app: &mut App,
    session: &Session,
    runtime: &TuiRuntime,
) -> Result<()> {
    crate::daemon::turn_queue::list(&runtime.home, session.id).await?;
    let text = app.input.text().to_owned();
    let attachments = app
        .attachments
        .iter()
        .map(|item| item.message.clone())
        .collect::<Vec<_>>();
    let prompt = app.enrich_prompt(&text, &runtime.skills);
    let fingerprint = serde_json::to_string(&(&prompt, &attachments, session.id))?;
    if app
        .runtime_queue_submission
        .as_ref()
        .is_none_or(|(key, _)| key != &fingerprint)
    {
        app.runtime_queue_submission = Some((fingerprint, uuid::Uuid::new_v4()));
    }
    let request_id = app.runtime_queue_submission.as_ref().unwrap().1;
    crate::daemon::submit_runtime_turn_with_id(
        &runtime.home,
        session.id,
        prompt,
        attachments,
        crate::Surface::Tui,
        request_id,
    )
    .await?;
    app.runtime_queue_submission = None;
    app.take_submitted_input();
    app.attachments.clear();
    app.selected_attachment = 0;
    app.notice = Some(
        app.language
            .text(
                "已保存到 Runtime 队列，各端同步",
                "Saved to Runtime queue; synced across clients",
                "Runtime キューに保存・端末間で同期",
            )
            .to_owned(),
    );
    if let Ok(items) = crate::daemon::turn_queue::list(&runtime.home, session.id).await {
        app.sync_runtime_queue(items);
    }
    Ok(())
}

/// Intercept remote mutations BEFORE the process-local queue editor changes
/// anything. An error leaves the original text and attachments with Runtime.
pub(super) async fn handle_key(
    key: KeyEvent,
    app: &mut App,
    session: &Session,
    runtime: &TuiRuntime,
) -> bool {
    let Some(panel) = app.prompt_queue.as_ref() else {
        return false;
    };
    let Some(remote) = app
        .queued_prompts
        .get(panel.selected)
        .and_then(|item| item.remote.clone())
    else {
        return false;
    };
    let editing = panel.editor.as_ref();
    let action = match key.code {
        KeyCode::Enter
            if editing.is_some()
                && !key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
        {
            willdeep_runtime_protocol::QueueAction::Edit
        }
        KeyCode::Delete | KeyCode::Char('d') if editing.is_none() => {
            willdeep_runtime_protocol::QueueAction::Remove
        }
        KeyCode::Char('s') if editing.is_none() => willdeep_runtime_protocol::QueueAction::SendNow,
        KeyCode::Char('i') if editing.is_none() => {
            willdeep_runtime_protocol::QueueAction::RunIndependently
        }
        KeyCode::Char('a') if editing.is_none() => {
            app.notice = Some(format!(
                "{}: {} · {}: {}",
                app.language.text("图片", "Images", "画像"),
                remote.image_count,
                app.language
                    .text("文本附件", "Text attachments", "テキスト添付"),
                remote.text_attachment_count
            ));
            return true;
        }
        KeyCode::Enter | KeyCode::Char('e') if editing.is_none() && !remote.can_edit => {
            app.notice = Some(
                app.language
                    .text(
                        "此旧消息的上下文不能安全编辑",
                        "Legacy message context cannot be edited safely",
                        "旧メッセージのコンテキストは安全に編集できません",
                    )
                    .to_owned(),
            );
            return true;
        }
        _ => return false,
    };
    let text = editing.map(|editor| editor.text().to_owned());
    let params = willdeep_runtime_protocol::UpdateTurnQueueParams {
        session_id: session.id,
        action,
        message_id: Some(remote.id),
        text,
        expected_text: Some(remote.text),
    };
    match crate::daemon::turn_queue::update(&runtime.home, &params).await {
        Ok(items) => {
            app.prompt_queue.as_mut().unwrap().editor = None;
            app.sync_runtime_queue(items);
            app.prompt_queue.as_mut().unwrap().show_feedback(
                app.language
                    .text(
                        "Runtime 队列已更新",
                        "Runtime queue updated",
                        "Runtime キューを更新しました",
                    )
                    .to_owned(),
                Instant::now(),
            );
        }
        Err(error) => app.prompt_queue.as_mut().unwrap().set_status(Some(format!(
            "{}: {error}",
            app.language.text(
                "更新失败，消息仍由 Runtime 保管",
                "Update failed; Runtime retains the message",
                "更新失敗。メッセージは Runtime に保持されています"
            )
        ))),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> willdeep_runtime_protocol::QueuedTurnMessage {
        willdeep_runtime_protocol::QueuedTurnMessage {
            independence_confirmed: false,
            priority_requested: false,
            id: uuid::Uuid::new_v4(),
            session_id: uuid::Uuid::new_v4(),
            text: text.to_owned(),
            text_preview: text.to_owned(),
            can_edit: true,
            image_count: 1,
            text_attachment_count: 0,
        }
    }

    #[test]
    fn remote_messages_are_visible_but_never_dispatched_by_the_client() {
        let mut app = App::new(Vec::new(), Language::En);
        app.sync_runtime_queue(vec![message("from Android")]);
        assert_eq!(app.queued_prompts[0].text, "from Android");
        assert!(!app.queued_prompt_ready());
        app.running = true;
        assert!(!app.queued_steering_ready());
        app.sync_runtime_queue(Vec::new());
        assert!(app.queued_prompts.is_empty());
    }

    #[test]
    fn selection_tracks_identity_across_external_removal() {
        let mut app = App::new(Vec::new(), Language::En);
        let first = message("same text");
        let second = message("same text");
        app.sync_runtime_queue(vec![first, second.clone()]);
        app.open_prompt_queue();
        app.prompt_queue.as_mut().unwrap().selected = 1;
        app.sync_runtime_queue(vec![second.clone()]);
        assert_eq!(app.prompt_queue.as_ref().unwrap().selected, 0);
        assert_eq!(app.queued_prompts[0].remote.as_ref().unwrap().id, second.id);
    }

    #[test]
    fn external_refresh_cannot_replace_the_message_being_edited() {
        let mut app = App::new(Vec::new(), Language::En);
        let original = message("original");
        app.sync_runtime_queue(vec![original.clone()]);
        app.open_prompt_queue();
        app.prompt_queue.as_mut().unwrap().editor = Some(PromptEditor::default());
        app.sync_runtime_queue(vec![message("replacement")]);
        assert_eq!(
            app.queued_prompts[0].remote.as_ref().unwrap().id,
            original.id
        );
    }

    #[test]
    fn parking_an_approval_keeps_its_original_sender_alive() {
        let mut app = App::new(Vec::new(), Language::En);
        let id = uuid::Uuid::new_v4();
        let (sender, mut receiver) = oneshot::channel();
        app.approval = Some(("original operation".into(), false, sender));
        app.active_remote_gate = Some(id);
        app.runtime_gates = vec![crate::daemon::RemoteGate::Approval {
            id,
            task_id: uuid::Uuid::new_v4(),
            description: "original operation".into(),
            always_allow_available: false,
            parked: true,
        }];
        reconcile_parked_dialogs(&mut app);
        assert!(app.approval.is_none());
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        let ParkedRemoteDialog::Approval((_, _, sender)) =
            app.parked_remote_dialogs.remove(&id).unwrap()
        else {
            panic!("approval identity changed");
        };
        sender.send(ApprovalDecision::AllowOnce).unwrap();
        assert_eq!(receiver.try_recv().unwrap(), ApprovalDecision::AllowOnce);
    }
}
