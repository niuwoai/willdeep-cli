use super::*;

pub(super) enum ParkedRemoteDialog {
    Approval(ApprovalRequest),
    Question(AskDialog),
}

pub(super) use super::delivery_echo::PendingRuntimeEcho;
use willdeep_runtime_protocol::{QueueAction, QueuedTurnMessage, RuntimeTurn};

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
        // The editor owns its original remote identity and compare-and-set text.
        // Refresh the list even while editing; never replace that edit target.
        self.sync_runtime_delivery_queue(&items);
        let selected = self
            .prompt_queue
            .as_ref()
            .and_then(|panel| self.queued_prompts.get(panel.selected))
            .and_then(|item| item.remote.as_ref())
            .map(|item| item.id);
        self.queued_prompts.retain(|item| item.remote.is_none());
        for item in items {
            let submission_order = self.next_submission_order();
            self.queued_prompts.push_back(QueuedPrompt {
                text: item.text.clone(),
                attachments: Vec::new(),
                remote: Some(item),
                submission_order,
                transcript_row: None,
            });
        }
        // Absence can mean claimed, cancelled, or a stale snapshot. A Turn
        // query/event, rather than queue absence, confirms delivery state.
        let edit_conflict = self.prompt_queue.as_ref().and_then(|panel| {
            panel
                .remote_edit
                .as_ref()
                .and_then(|original| remote_edit_conflict(self, original).map(str::to_owned))
        });
        if let Some(panel) = self.prompt_queue.as_mut() {
            if let Some(message) = edit_conflict {
                panel.set_status(Some(message));
            }
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

/// 普通输入不能代替审批或回答。恢复原入口，草稿留在输入框中。
pub(super) fn retain_input_for_pending_gate(
    app: &mut App,
    home: &std::path::Path,
    ui: &mpsc::UnboundedSender<UiMessage>,
) -> bool {
    let Some(gate) = app.runtime_gates.first().cloned() else {
        return false;
    };
    let message = format!(
        "{} · {}",
        gate_progress(app.language, &gate),
        app.language.text(
            "输入与附件已保留；请处理待确认项，或用 Cmd/Ctrl+Enter 明确入队",
            "Input and attachments retained; handle the pending request or use Cmd/Ctrl+Enter to queue",
            "入力と添付を保持しました。確認待ちを処理するか、Cmd/Ctrl+Enter でキューに追加してください",
        ),
    );
    app.append_transcript(format!("System: {message}"));
    app.notice = Some(message);
    runtime_ui::open_remote_gate(app, gate, home.to_path_buf(), ui.clone());
    true
}

fn gate_progress(language: Language, gate: &crate::daemon::RemoteGate) -> String {
    let (status, detail) = match gate {
        crate::daemon::RemoteGate::Approval { description, .. } => (
            language.text("等待审批", "Waiting for approval", "承認待ち"),
            description,
        ),
        crate::daemon::RemoteGate::Question { question, .. } => (
            language.text("等待回答", "Waiting for answer", "回答待ち"),
            question,
        ),
    };
    format!("{status}: {detail}")
}

/// 只使用已收到的工具/审批快照，不把入队当成模型已读，也不推断完成比例。
fn current_progress(app: &App, session_id: uuid::Uuid) -> String {
    if let Some(gate) = app.runtime_gates.first() {
        return gate_progress(app.language, gate);
    }
    use willdeep_runtime_protocol::ToolStatus;
    let tools = app
        .runtime_tools
        .iter()
        .filter(|tool| tool.session_id == Some(session_id));
    let running = tools
        .clone()
        .filter(|tool| tool.status == ToolStatus::Running)
        .max_by_key(|tool| tool.started_at_ms);
    let recent = tools
        .filter(|tool| tool.status != ToolStatus::Running)
        .max_by_key(|tool| tool.completed_at_ms.unwrap_or(tool.started_at_ms));
    let mut progress = Vec::new();
    if let Some(tool) = running {
        progress.push(format!(
            "{}: {}",
            app.language.text(
                "最近状态：正在执行",
                "Latest status: running",
                "直近の状態：実行中"
            ),
            tool.name
        ));
    } else if let Some(tool) = app
        .tools
        .details
        .iter()
        .rev()
        .find_map(|line| line.strip_prefix("… "))
    {
        progress.push(format!(
            "{}: {tool}",
            app.language.text(
                "最近状态：正在执行",
                "Latest status: running",
                "直近の状態：実行中"
            )
        ));
    }
    if let Some(tool) = recent {
        let status = match tool.status {
            ToolStatus::Completed => app
                .language
                .text("最近完成", "Last completed", "直近の完了"),
            ToolStatus::Failed => app.language.text("最近失败", "Last failed", "直近の失敗"),
            ToolStatus::Interrupted => {
                app.language
                    .text("最近中断", "Last interrupted", "直近の中断")
            }
            _ => app
                .language
                .text("最近部分完成", "Last partially completed", "直近の部分完了"),
        };
        progress.push(format!("{status}: {}", tool.name));
    } else if let Some(tool) = app
        .tools
        .details
        .iter()
        .rev()
        .find_map(|line| line.strip_prefix("✓ "))
    {
        progress.push(format!(
            "{}: {tool}",
            app.language
                .text("最近完成", "Last completed", "直近の完了")
        ));
    }
    if progress.is_empty() {
        return app
            .language
            .text(
                "当前轮次仍在运行；尚未收到新进展",
                "The current turn is still running; no new progress received yet",
                "現在のターンは実行中です。新しい進捗はまだ届いていません",
            )
            .to_owned();
    }
    progress.join(" · ")
}

pub(super) async fn submit_input(
    app: &mut App,
    session: &mut Session,
    runtime: &TuiRuntime,
    store: &SessionStore,
    defer_until_released: bool,
    term: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    if app.running {
        app.append_transcript(format!("System: {}", current_progress(app, session.id)));
    }
    let (draft, expanded, row) = prepare_input_submission(app, defer_until_released);
    let text = draft.text().to_owned();
    // 先显示输入已转入聊天区、但尚未获确认；技能读取、落盘与 RPC 都在这一帧后。
    let result = async {
        redraw::redraw(term, app, &runtime.skills, session)?;
        let attachments = app
            .attachments
            .iter()
            .map(|item| item.message.clone())
            .collect::<Vec<_>>();
        let prompt = app.enrich_prompt(&text, &runtime.skills);
        let request_id =
            submission_request_id(app, &prompt, &attachments, session.id, defer_until_released)?;
        // 空白会话在第一次真正提交时补写并登记，否则服务端不认这条会话。
        runtime_ui::persist_missing_core_session(session, store)?;
        // 忙时提交走的是另一条路，归档会话同样要先解除，否则照样被拒。
        runtime_ui::resume_archived_for_send(app, session, runtime).await?;
        crate::daemon::ensure_runtime_session(
            &runtime.home,
            session.id,
            &session.workspace,
            session.profile.clone(),
            session.model.clone(),
        )
        .await?;
        session.runtime_managed = true;
        store.save(session)?;
        crate::daemon::turn_queue::list(&runtime.home, session.id).await?;
        let turn = crate::daemon::submit_runtime_turn_with_options(
            &runtime.home,
            session.id,
            prompt,
            attachments,
            crate::Surface::Tui,
            request_id,
            defer_until_released,
        )
        .await?;
        Ok(turn)
    }
    .await;
    finish_input_submission(app, draft, expanded, row, defer_until_released, &result);
    result?;
    if let Ok(items) = crate::daemon::turn_queue::list(&runtime.home, session.id).await {
        app.sync_runtime_queue(items);
    }
    Ok(())
}

pub(super) fn submission_request_id(
    app: &mut App,
    prompt: &str,
    attachments: &[MessageAttachment],
    session_id: uuid::Uuid,
    defer_until_released: bool,
) -> Result<uuid::Uuid> {
    // 暂缓与否是提交的一部分；失败重试保留原幂等键。
    let fingerprint =
        serde_json::to_string(&(prompt, attachments, session_id, defer_until_released))?;
    if app
        .runtime_queue_submission
        .as_ref()
        .is_none_or(|(key, _)| key != &fingerprint)
    {
        app.runtime_queue_submission = Some((fingerprint, uuid::Uuid::new_v4()));
    }
    Ok(app.runtime_queue_submission.as_ref().unwrap().1)
}

fn prepare_input_submission(app: &mut App, deferred: bool) -> (PromptEditor, bool, usize) {
    let draft = std::mem::take(&mut app.input);
    let expanded = std::mem::replace(&mut app.composer_expanded, false);
    let status = if deferred {
        app.language.text(
            "正在加入待发队列",
            "Adding to queue for later",
            "送信待ちキューに追加しています",
        )
    } else {
        app.language.text("正在提交", "submitting", "送信中")
    };
    let row = app.transcript.len();
    // 提交尚无回执，用未确认符号占位；详细反馈仍留在状态区。
    // 不在请求返回之前显示代表已接收或成功的符号。
    if !deferred {
        app.note_awaiting_reply();
    }
    app.append_transcript(format!(
        "You: {} {}",
        delivery_echo::DeliveryStatus::Unconfirmed.symbol(),
        draft.text()
    ));
    app.notice = Some(status.to_owned());
    // Queueing a supplement must not move a reader away from their source.
    // Existing follow mode still follows the appended placeholder naturally.
    (draft, expanded, row)
}

fn finish_input_submission(
    app: &mut App,
    draft: PromptEditor,
    expanded: bool,
    row: usize,
    deferred: bool,
    result: &Result<RuntimeTurn>,
) {
    app.input = draft;
    match result {
        Ok(turn) => {
            app.runtime_queue_submission = None;
            let submitted = app.take_submitted_input();
            let sent_attachments = std::mem::take(&mut app.attachments)
                .into_iter()
                .map(|item| item.message)
                .collect::<Vec<_>>();
            app.selected_attachment = 0;
            app.publish_gallery();
            // Keep the placeholder's exact record, timestamp and metadata. The
            // receipt can already be running or terminal after an idempotent retry.
            let record = app.echo_runtime_submission(&submitted, turn, deferred, Some(row));
            if record != Some(row) {
                app.drop_transcript_row(row);
            }
            if let Some(record) = app
                .pending_runtime_echoes
                .iter()
                .find(|echo| echo.turn_id == turn.id)
                .and_then(|echo| echo.record)
            {
                app.transcript_attachments
                    .resize_with(app.transcript.len(), Vec::new);
                app.transcript_attachments[record] = sent_attachments;
            }
            let status = app
                .pending_runtime_echoes
                .iter()
                .find(|echo| echo.turn_id == turn.id)
                .expect("a successful submit registers a receipt")
                .status;
            if !app.running
                && (deferred
                    || !app
                        .pending_runtime_echoes
                        .iter()
                        .any(PendingRuntimeEcho::is_pending))
            {
                app.dismiss_awaiting_reply();
            }
            if !app.running
                && let Some(outcome) = status.outcome()
            {
                app.last_turn_outcome = Some(outcome);
            }
            let feedback = status.feedback(app.language).to_owned();
            // Delivery is already shown on the identity-bound user record and
            // in the status area. A permanent System row would become stale
            // as soon as Runtime starts this turn.
            app.notice = Some(feedback);
        }
        Err(error) => {
            app.dismiss_awaiting_reply();
            app.composer_expanded = expanded;
            let message = format!(
                "{}: {error:#}",
                delivery_echo::DeliveryStatus::Unconfirmed.feedback(app.language)
            );
            app.transcript[row] =
                terminal_safe_text(&format!("Error: {message}\n{}", app.input.text()));
            if let Some((_, request_id)) = app.runtime_queue_submission.as_ref() {
                let request_id = *request_id;
                let text = app.input.text().to_owned();
                app.note_runtime_submission_unconfirmed(&text, request_id, deferred, Some(row));
                app.transcript_attachments[row] = app
                    .attachments
                    .iter()
                    .map(|item| item.message.clone())
                    .collect();
            }
            app.notice = Some(message);
        }
    }
    app.transcript_render_cache = None;
    app.refresh_transcript_height();
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
    let editing = panel.editor.as_ref();
    // Saving always targets the original ID and original expected text. The
    // selected row may now refer to a different item, or the list may be empty.
    let remote = if editing.is_some() {
        panel.remote_edit.clone()
    } else {
        app.queued_prompts
            .get(panel.selected)
            .and_then(|item| item.remote.clone())
    };
    let Some(remote) = remote else {
        return false;
    };
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
    if action == QueueAction::Edit
        && let Some(conflict) = remote_edit_conflict(app, &remote)
    {
        app.prompt_queue
            .as_mut()
            .unwrap()
            .set_status(Some(conflict.to_owned()));
        return true;
    }
    let text = editing.map(|editor| editor.text().to_owned());
    let params = willdeep_runtime_protocol::UpdateTurnQueueParams {
        session_id: session.id,
        action,
        message_id: Some(remote.id),
        text,
        expected_text: Some(remote.text.clone()),
    };
    match crate::daemon::turn_queue::update(&runtime.home, &params).await {
        Ok(items) => {
            let panel = app.prompt_queue.as_mut().unwrap();
            panel.editor = None;
            panel.remote_edit = None;
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
        Err(error) => {
            if let Ok(items) = crate::daemon::turn_queue::list(&runtime.home, session.id).await {
                app.sync_runtime_queue(items);
            }
            let detail = remote_edit_conflict(app, &remote).unwrap_or_else(|| {
                app.language.text(
                    "更新未确认；改写已保留，请刷新后重试",
                    "Update unconfirmed; your rewrite is retained; refresh before retrying",
                    "更新を確認できません。編集内容を保持しました。更新後に再試行してください",
                )
            });
            app.prompt_queue
                .as_mut()
                .unwrap()
                .set_status(Some(format!("{detail}: {error}")));
        }
    }
    true
}

fn remote_edit_conflict(app: &App, original: &QueuedTurnMessage) -> Option<&'static str> {
    let current = app
        .queued_prompts
        .iter()
        .filter_map(|item| item.remote.as_ref())
        .find(|item| item.id == original.id);
    if current.is_some_and(|item| item.text != original.text) {
        return Some(app.language.text(
            "原消息正文已变更；改写已保留，不会作为新消息发送",
            "The original message text changed; your rewrite is retained and will not be sent as a new message",
            "元のメッセージ本文が変更されました。編集内容を保持し、新規送信はしません",
        ));
    }
    if app
        .pending_runtime_echoes
        .iter()
        .any(|echo| echo.turn_id == original.id && !echo.is_pending())
    {
        return Some(app.language.text(
            "原消息已开始处理或已结束；改写已保留，不会作为新消息发送",
            "The original message started or settled; your rewrite is retained and will not be sent as a new message",
            "元のメッセージは処理開始済み、または終了済みです。編集内容を保持し、新規送信はしません",
        ));
    }
    current.is_none().then(|| app.language.text(
        "原消息已离开待发队列，可能已开始或被删除；改写已保留",
        "The original message left the queue and may have started or been removed; your rewrite is retained",
        "元のメッセージはキューを離れ、処理開始または削除された可能性があります。編集内容を保持しました",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> willdeep_runtime_protocol::QueuedTurnMessage {
        willdeep_runtime_protocol::QueuedTurnMessage {
            independence_confirmed: false,
            priority_requested: false,
            held: false,
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
    fn external_refresh_keeps_the_edit_target_without_freezing_the_list() {
        let mut app = App::new(Vec::new(), Language::En);
        let original = message("original");
        app.sync_runtime_queue(vec![original.clone()]);
        app.open_prompt_queue();
        app.handle_queue_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let replacement = message("replacement");
        app.sync_runtime_queue(vec![replacement.clone()]);
        assert_eq!(
            app.queued_prompts[0].remote.as_ref().unwrap().id,
            replacement.id
        );
        let panel = app.prompt_queue.as_ref().unwrap();
        assert_eq!(panel.remote_edit.as_ref().unwrap().id, original.id);
        assert_eq!(panel.remote_edit.as_ref().unwrap().text, "original");
        assert_eq!(panel.editor.as_ref().unwrap().text(), "original");
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

    #[tokio::test]
    async fn waiting_input_restores_original_approval_without_submitting_or_deciding() {
        let mut app = App::new(Vec::new(), Language::En);
        app.begin_turn(true, "working".into());
        app.input.insert("what is the progress?");
        app.attachments.push(DraftAttachment {
            id: uuid::Uuid::new_v4(),
            message: MessageAttachment::Text {
                name: "keep.txt".into(),
                content: "keep me".into(),
            },
        });
        let id = uuid::Uuid::new_v4();
        let (sender, mut receiver) = oneshot::channel();
        app.parked_remote_dialogs.insert(
            id,
            ParkedRemoteDialog::Approval(("read directory".into(), false, sender)),
        );
        app.runtime_gates.push(crate::daemon::RemoteGate::Approval {
            id,
            task_id: uuid::Uuid::new_v4(),
            description: "read directory".into(),
            always_allow_available: false,
            parked: false,
        });
        // 恢复缓存中的 sender 不访问磁盘或 Runtime；使用唯一隔离路径即可。
        let home =
            std::env::temp_dir().join(format!("willdeep-gate-test-{}", uuid::Uuid::new_v4()));
        let (ui, mut events) = mpsc::unbounded_channel();
        assert!(retain_input_for_pending_gate(&mut app, &home, &ui));
        assert_eq!(app.input.text(), "what is the progress?");
        assert_eq!(app.attachments.len(), 1);
        assert_eq!(app.active_remote_gate, Some(id));
        assert!(app.approval.is_some());
        assert!(app.queued_prompts.is_empty());
        assert!(app.pending_runtime_echoes.is_empty());
        assert!(app.runtime_queue_submission.is_none());
        assert!(app.running);
        assert!(
            app.transcript
                .iter()
                .any(|row| row.contains("Waiting for approval: read directory"))
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn progress_feedback_reports_observed_tools_and_filters_other_sessions() {
        let mut app = App::new(Vec::new(), Language::En);
        let session_id = uuid::Uuid::new_v4();
        assert!(current_progress(&app, session_id).contains("no new progress received yet"));
        app.runtime_tools
            .push(willdeep_runtime_protocol::RuntimeTool {
                id: uuid::Uuid::new_v4(),
                session_id: Some(uuid::Uuid::new_v4()),
                turn_id: None,
                task_id: uuid::Uuid::new_v4(),
                agent_id: uuid::Uuid::new_v4(),
                name: "unrelated tool".into(),
                status: willdeep_runtime_protocol::ToolStatus::Running,
                started_at_ms: 20,
                completed_at_ms: None,
            });
        assert!(!current_progress(&app, session_id).contains("unrelated tool"));
        app.tools.requested("read_file");
        app.tools.completed("read_file", false);
        app.tools.requested("list_directory");
        let progress = current_progress(&app, session_id);
        assert!(progress.contains("Latest status: running: list_directory"));
        assert!(progress.contains("Last completed: read_file"));
        app.runtime_gates.push(crate::daemon::RemoteGate::Question {
            id: uuid::Uuid::new_v4(),
            task_id: uuid::Uuid::new_v4(),
            question: "Which directory?".into(),
            options: vec![],
            multi_select: false,
            parked: true,
        });
        assert_eq!(
            current_progress(&app, session_id),
            "Waiting for answer: Which directory?"
        );
    }
}

#[cfg(test)]
#[path = "runtime_queue_tests.rs"]
mod delivery_tests;
