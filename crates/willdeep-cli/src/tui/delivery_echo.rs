use super::*;
use willdeep_runtime_protocol::{QueuedTurnMessage, RuntimeTurn, TurnStatus};

/// A receipt owns one exact transcript record. Keep started and terminal entries
/// as tombstones until the Session changes: a replay must never claim by text.
#[derive(Clone, Debug)]
pub(super) struct PendingRuntimeEcho {
    pub(super) turn_id: uuid::Uuid,
    pub(super) request_id: uuid::Uuid,
    pub(super) status: DeliveryStatus,
    pub(super) text: String,
    pub(super) record: Option<usize>,
    // Snapshots can advance status before the ordered start event arrives.
    // Only that event establishes the record's position in the conversation.
    positioned: bool,
    pub(super) priority_requested: bool,
    pub(super) independence_confirmed: bool,
    held: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DeliveryStatus {
    Unconfirmed,
    Queued,
    Held,
    Started,
    Completed,
    Partial,
    Failed,
    Interrupted,
    Cancelled,
}

impl DeliveryStatus {
    fn from_turn(turn: &RuntimeTurn, held: bool) -> Self {
        match turn.status {
            TurnStatus::Queued if held => Self::Held,
            TurnStatus::Queued => Self::Queued,
            TurnStatus::Running | TurnStatus::WaitingApproval | TurnStatus::WaitingAnswer => {
                Self::Started
            }
            TurnStatus::Completed => Self::Completed,
            TurnStatus::Partial => Self::Partial,
            TurnStatus::Failed => Self::Failed,
            TurnStatus::Interrupted => Self::Interrupted,
            TurnStatus::Cancelled => Self::Cancelled,
        }
    }

    fn is_pending(self) -> bool {
        matches!(self, Self::Unconfirmed | Self::Queued | Self::Held)
    }

    fn is_terminal(self) -> bool {
        !self.is_pending() && self != Self::Started
    }

    pub(super) fn outcome(self) -> Option<turn_status::TurnOutcome> {
        use turn_status::TurnOutcome;
        match self {
            Self::Completed => Some(TurnOutcome::Completed),
            Self::Partial => Some(TurnOutcome::Partial),
            Self::Failed => Some(TurnOutcome::Failed),
            Self::Interrupted => Some(TurnOutcome::Interrupted),
            Self::Cancelled => Some(TurnOutcome::Cancelled),
            Self::Unconfirmed | Self::Queued | Self::Held | Self::Started => None,
        }
    }

    /// Static, single-cell receipts: Started also covers human gates, so it
    /// must not animate indefinitely or imply that a completed turn succeeded.
    pub(super) fn symbol(self) -> &'static str {
        match self {
            Self::Unconfirmed => "?",
            Self::Queued => "◇",
            Self::Held => "‖",
            Self::Started => "▶",
            Self::Completed => "✓",
            Self::Partial => "◒",
            Self::Failed => "✗",
            Self::Interrupted => "■",
            Self::Cancelled => "×",
        }
    }

    pub(super) fn color(self) -> Color {
        match self {
            Self::Unconfirmed | Self::Held | Self::Partial => Color::Yellow,
            Self::Queued => Color::Gray,
            Self::Started => Color::Cyan,
            Self::Completed => Color::Green,
            Self::Failed => Color::Red,
            Self::Interrupted | Self::Cancelled => Color::DarkGray,
        }
    }

    pub(super) fn feedback(self, language: Language) -> &'static str {
        match self {
            Self::Unconfirmed => language.text(
                "提交未确认，草稿与附件已保留；重试会复用同一请求",
                "Submission unconfirmed; draft and attachments retained; retry reuses the same request",
                "送信を確認できません。下書きと添付を保持しました。再試行は同じリクエストを使用します",
            ),
            Self::Queued => language.text(
                "Runtime 已接收并排队，等待开始处理",
                "Runtime accepted and queued the message; waiting to start",
                "Runtime が受信してキューに追加しました。処理開始待ちです",
            ),
            Self::Held => language.text(
                "Runtime 已接收并暂缓；不会自动发送 · Ctrl+Q 里按 S 释放",
                "Runtime accepted and held the message; it will not auto-send · release with S in Ctrl+Q",
                "Runtime が受信して保留しました。自動送信しません · Ctrl+Q の S で解除",
            ),
            Self::Started | Self::Completed => self.symbol(),
            Self::Partial => language.text(
                "Runtime 已结束此轮，但任务仅部分完成",
                "Runtime ended this turn with partial completion",
                "Runtime はこのターンを終了しましたが、タスクは部分完了です",
            ),
            Self::Failed => language.text(
                "Runtime 已接收此消息，但处理失败",
                "Runtime accepted this message, but processing failed",
                "Runtime はこのメッセージを受信しましたが、処理に失敗しました",
            ),
            Self::Interrupted => language.text(
                "这条消息对应的 Runtime 轮次已中断",
                "The Runtime turn for this message was interrupted",
                "このメッセージの Runtime ターンは中断しました",
            ),
            Self::Cancelled => language.text(
                "这条消息对应的 Runtime 轮次已取消",
                "The Runtime turn for this message was cancelled",
                "このメッセージの Runtime ターンはキャンセルされました",
            ),
        }
    }
}

impl PendingRuntimeEcho {
    pub(super) fn is_pending(&self) -> bool {
        self.status.is_pending()
    }

    pub(super) fn needs_reconciliation(&self) -> bool {
        !self.status.is_terminal()
    }

    fn advance(&mut self, next: DeliveryStatus) {
        // An event may precede its queue snapshot, and a terminal receipt may
        // precede replay of the start event. Neither may roll the state back.
        if self.status.is_terminal()
            || (self.status == DeliveryStatus::Started && next.is_pending())
        {
            return;
        }
        self.status = next;
    }

    fn row(&self, language: Language) -> String {
        let mut status = self.status.symbol().to_owned();
        if self.is_pending() && self.priority_requested {
            status.push_str(language.text("·优先", " · priority", "・優先"));
        }
        if self.is_pending() && self.independence_confirmed {
            status.push_str(language.text("·独立处理", " · independent", "・独立処理"));
        }
        format!("You: {status} {}", terminal_safe_text(&self.text))
    }
}

impl App {
    /// A lost RPC response does not prove rejection. Keep its request identity
    /// eligible for the existing background Turns query, including held input.
    pub(super) fn note_runtime_submission_unconfirmed(
        &mut self,
        text: &str,
        request_id: uuid::Uuid,
        held: bool,
        record: Option<usize>,
    ) {
        if self
            .pending_runtime_echoes
            .iter()
            .any(|echo| echo.request_id == request_id)
        {
            return;
        }
        self.pending_runtime_echoes.push_back(PendingRuntimeEcho {
            turn_id: uuid::Uuid::nil(),
            request_id,
            status: DeliveryStatus::Unconfirmed,
            text: willdeep_core::conversation::visible_user_text(text).to_owned(),
            record,
            positioned: false,
            priority_requested: false,
            independence_confirmed: false,
            held,
        });
        self.render_runtime_echo(self.pending_runtime_echoes.len() - 1);
    }

    /// Bind the submit receipt to its existing placeholder (or append once).
    /// A missing record stays missing after explicit deletion; identity survives.
    pub(super) fn echo_runtime_submission(
        &mut self,
        text: &str,
        turn: &RuntimeTurn,
        held: bool,
        record: Option<usize>,
    ) -> Option<usize> {
        let visible = willdeep_core::conversation::visible_user_text(text);
        if self
            .pending_runtime_echoes
            .iter()
            .any(|echo| echo.turn_id == turn.id || echo.request_id == turn.request_id)
        {
            self.reconcile_runtime_deliveries(std::slice::from_ref(turn));
            return self
                .pending_runtime_echoes
                .iter()
                .find(|echo| echo.turn_id == turn.id)
                .and_then(|echo| echo.record);
        }
        let record = record
            .filter(|&index| index < self.transcript.len())
            .unwrap_or_else(|| {
                let index = self.transcript.len();
                self.append_transcript(format!("You: {}", terminal_safe_text(visible)));
                index
            });
        self.pending_runtime_echoes.push_back(PendingRuntimeEcho {
            turn_id: turn.id,
            request_id: turn.request_id,
            status: DeliveryStatus::from_turn(turn, held),
            text: visible.to_owned(),
            record: Some(record),
            positioned: false,
            priority_requested: false,
            independence_confirmed: false,
            held,
        });
        self.render_runtime_echo(self.pending_runtime_echoes.len() - 1);
        self.note_prompt_submitted(visible);
        Some(record)
    }

    /// Only queued_user_message with the same Turn identity can claim a receipt.
    /// true also covers a tombstone whose transcript row was explicitly removed.
    pub(super) fn claim_runtime_echo(&mut self, turn_id: uuid::Uuid, text: &str) -> bool {
        let Some(index) = self
            .pending_runtime_echoes
            .iter()
            .position(|echo| echo.turn_id == turn_id)
        else {
            return false;
        };
        if !self.pending_runtime_echoes[index].positioned {
            self.finish_runtime_input_boundary();
            if let Some(record) = self.pending_runtime_echoes[index].record {
                self.move_transcript_row_to_end(record);
            }
            self.pending_runtime_echoes[index].positioned = true;
        }
        let echo = &mut self.pending_runtime_echoes[index];
        if echo.is_pending() {
            echo.text = willdeep_core::conversation::visible_user_text(text).to_owned();
        }
        echo.advance(DeliveryStatus::Started);
        self.render_runtime_echo(index);
        self.clear_started_delivery_notice();
        true
    }

    fn finish_runtime_input_boundary(&mut self) {
        // The preceding turn may still own a live/held display buffer. Commit
        // it before positioning the next user; do not reset the new turn's clock.
        self.finish_thought();
        self.finish_reply_stream();
        self.flush_held_narration();
    }

    fn clear_started_delivery_notice(&mut self) {
        if !self
            .pending_runtime_echoes
            .iter()
            .any(PendingRuntimeEcho::is_pending)
            && [
                DeliveryStatus::Queued,
                DeliveryStatus::Held,
                DeliveryStatus::Unconfirmed,
            ]
            .iter()
            .any(|status| self.notice.as_deref() == Some(status.feedback(self.language)))
        {
            self.notice = None;
        }
    }

    pub(super) fn note_runtime_input_started(&mut self, turn_id: uuid::Uuid, text: &str) {
        if self.claim_runtime_echo(turn_id, text) {
            return;
        }
        self.finish_runtime_input_boundary();
        let text = willdeep_core::conversation::visible_user_text(text).to_owned();
        let record = self.transcript.len();
        self.append_transcript(format!("You: {}", terminal_safe_text(&text)));
        self.pending_runtime_echoes.push_back(PendingRuntimeEcho {
            turn_id,
            // This output event has no request ID; a later Turn query fills it.
            request_id: uuid::Uuid::nil(),
            status: DeliveryStatus::Started,
            text: text.clone(),
            record: Some(record),
            positioned: true,
            priority_requested: false,
            independence_confirmed: false,
            held: false,
        });
        self.render_runtime_echo(self.pending_runtime_echoes.len() - 1);
        self.note_prompt_submitted(&text);
    }

    /// Queue presence is authoritative for editable text/flags, never absence.
    pub(super) fn sync_runtime_delivery_queue(&mut self, items: &[QueuedTurnMessage]) {
        for index in 0..self.pending_runtime_echoes.len() {
            let echo = &mut self.pending_runtime_echoes[index];
            if let Some(item) = items.iter().find(|item| item.id == echo.turn_id) {
                if !echo.is_pending() {
                    continue;
                }
                echo.text = item.text.clone();
                echo.status = if item.held {
                    DeliveryStatus::Held
                } else {
                    DeliveryStatus::Queued
                };
                echo.held = item.held;
                echo.priority_requested = item.priority_requested;
                echo.independence_confirmed = item.independence_confirmed;
                self.render_runtime_echo(index);
            }
        }
    }

    /// A list omitting an ID proves nothing. Only a returned Turn changes state.
    pub(super) fn reconcile_runtime_deliveries(&mut self, turns: &[RuntimeTurn]) {
        for index in 0..self.pending_runtime_echoes.len() {
            let echo = &mut self.pending_runtime_echoes[index];
            if let Some(turn) = turns.iter().find(|turn| {
                turn.id == echo.turn_id
                    || (!echo.request_id.is_nil() && turn.request_id == echo.request_id)
            }) {
                echo.turn_id = turn.id;
                echo.request_id = turn.request_id;
                echo.advance(DeliveryStatus::from_turn(turn, echo.held));
                self.render_runtime_echo(index);
            }
        }
        // A start event may arrive before a lost receipt is recovered. Merge
        // only proven identities: keep its established position, carrying the
        // original submission metadata across from the earlier placeholder.
        let mut index = 0;
        while index < self.pending_runtime_echoes.len() {
            let turn_id = self.pending_runtime_echoes[index].turn_id;
            if !turn_id.is_nil() {
                while let Some(duplicate) = (index + 1..self.pending_runtime_echoes.len())
                    .find(|&other| self.pending_runtime_echoes[other].turn_id == turn_id)
                {
                    let other = self
                        .pending_runtime_echoes
                        .remove(duplicate)
                        .expect("known duplicate");
                    self.pending_runtime_echoes[index].advance(other.status);
                    let original = self.pending_runtime_echoes[index].record;
                    let prefer_positioned = !self.pending_runtime_echoes[index].positioned
                        && other.positioned
                        && original.is_some()
                        && other.record.is_some();
                    let (kept, removed) = if prefer_positioned {
                        let kept = other.record.expect("positioned record");
                        let removed = original.expect("original placeholder");
                        self.message_times.copy_time(removed, kept);
                        self.viewport_merge_record(removed, kept);
                        self.pending_runtime_echoes[index].record = Some(kept);
                        self.pending_runtime_echoes[index].text = other.text;
                        (Some(kept), Some(removed))
                    } else {
                        (original, other.record)
                    };
                    self.pending_runtime_echoes[index].positioned |= other.positioned;
                    if let Some(removed) = removed.filter(|record| Some(*record) != kept) {
                        if let Some(kept) = kept
                            && self.transcript_attachments[kept].is_empty()
                        {
                            self.transcript_attachments[kept] =
                                self.transcript_attachments[removed].clone();
                        }
                        self.drop_transcript_row(removed);
                    }
                }
                self.render_runtime_echo(index);
            }
            index += 1;
        }
    }

    /// Hook this into drop_transcript_row before the transcript is mutated.
    pub(super) fn delivery_record_removed(&mut self, removed: usize) {
        for echo in &mut self.pending_runtime_echoes {
            if let Some(record) = echo.record.as_mut() {
                if *record == removed {
                    echo.record = None;
                } else if *record > removed {
                    *record -= 1;
                }
            }
        }
    }

    fn render_runtime_echo(&mut self, index: usize) {
        let echo = &self.pending_runtime_echoes[index];
        let Some(record) = echo.record.filter(|&record| record < self.transcript.len()) else {
            return;
        };
        let row = echo.row(self.language);
        if self.transcript[record] != row {
            self.transcript[record] = row;
            self.transcript_render_cache = None;
            self.refresh_transcript_height();
        }
    }
}

#[cfg(test)]
#[path = "delivery_echo_tests.rs"]
mod tests;
