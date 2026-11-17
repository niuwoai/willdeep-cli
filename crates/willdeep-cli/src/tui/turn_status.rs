//! Foreground turn state is independent of the activity log and idle timer.
use super::*;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum TurnPhase {
    #[default]
    Preparing,
    WaitingModel,
    Reasoning,
    ReceivingReply,
    Tool(String),
    Compressing,
    Retrying {
        attempt: u32,
        delay: Option<Duration>,
    },
    Background,
}

impl TurnPhase {
    pub(super) fn label(&self, language: Language) -> String {
        match self {
            Self::Preparing => language
                .text("正在准备请求", "Preparing request", "リクエストを準備中")
                .to_owned(),
            Self::WaitingModel => language
                .text("等待模型回复", "Waiting for model", "モデルの応答待ち")
                .to_owned(),
            Self::Reasoning => language.text("正在思考", "Thinking", "思考中").to_owned(),
            Self::ReceivingReply => language
                .text("正在接收回复", "Receiving reply", "応答を受信中")
                .to_owned(),
            Self::Tool(name) => format!(
                "{} · {name}",
                language.text("正在执行工具", "Running tool", "ツールを実行中")
            ),
            Self::Compressing => language
                .text(
                    "正在压缩上下文",
                    "Compressing context",
                    "コンテキストを圧縮中",
                )
                .to_owned(),
            Self::Retrying { attempt, delay } => {
                let label = language.text("正在重试", "Retrying", "再試行中");
                match delay {
                    Some(delay) => format!(
                        "{label} · {attempt} · {} {}",
                        language.text("等待", "wait", "待機"),
                        format_elapsed_span(delay.as_secs_f32(), 1)
                    ),
                    None => format!("{label} · {attempt}"),
                }
            }
            Self::Background => language
                .text(
                    "后台任务运行中 · 可以继续输入，完成后自动通知",
                    "Background running · Keep typing; completion will be reported",
                    "バックグラウンド実行中 · 入力を続けられます。完了後に通知",
                )
                .to_owned(),
        }
    }
}

/// Set only from an explicit terminal event. Cleanup and reconnection are not completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TurnOutcome {
    Completed,
    Partial,
    Failed,
    Interrupted,
    Cancelled,
}

impl TurnOutcome {
    pub(super) fn from_stop_reason(reason: willdeep_core::AgentStopReason) -> Self {
        if reason.is_complete() {
            Self::Completed
        } else {
            Self::Partial
        }
    }

    pub(super) fn label(self, language: Language) -> &'static str {
        match self {
            Self::Completed => delivery_echo::DeliveryStatus::Completed.symbol(),
            Self::Partial => language.text("本轮未完成", "Turn incomplete", "ターン未完了"),
            Self::Failed => language.text("本轮失败", "Turn failed", "ターン失敗"),
            Self::Interrupted => language.text("本轮已中断", "Turn interrupted", "ターン中断"),
            Self::Cancelled => language.text("本轮已取消", "Turn cancelled", "ターン取消"),
        }
    }
}

/// A finalized paragraph keeps the identity and time of its source event while held.
#[derive(Clone, Debug)]
pub(super) struct HeldNarration {
    pub(super) text: String,
    pub(super) model: Option<String>,
    pub(super) recorded_at: u64,
}

impl std::ops::Deref for HeldNarration {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

/// Silence supplements the phase; it never replaces it or implies a terminal result.
/// Kept compatible with the original helper so its callers can delegate here.
pub(super) fn format_working_summary(
    language: Language,
    _runtime_turn: bool,
    activity_line: &str,
    elapsed: Duration,
    idle: Duration,
) -> String {
    let phase_separator = if activity_line.is_empty() { "" } else { " " };
    let silence = if idle >= PROGRESS_STALE_AFTER {
        format!(
            " · {} {}",
            format_elapsed_span(idle.as_secs_f32(), 0),
            language.text("未收到新事件", "without a new event", "新しいイベントなし"),
        )
    } else if idle >= PROGRESS_WAITING_AFTER {
        format!(
            " · {}",
            language.text(
                "等待新事件",
                "Waiting for an update",
                "新しいイベントを待機中"
            )
        )
    } else {
        String::new()
    };
    format!(
        "{}{phase_separator}{activity_line}{silence} · {} {}",
        progress_spinner(elapsed),
        language.text("已运行", "elapsed", "経過"),
        format_elapsed_span(elapsed.as_secs_f32(), 1),
    )
}

impl App {
    /// A real foreground event also acts as a heartbeat, without growing the activity log.
    pub(super) fn note_turn_phase(&mut self, phase: TurnPhase) {
        self.turn_phase = phase;
        self.last_progress_at = Some(Instant::now());
    }

    /// The first explicit terminal result wins; duplicate terminal events are harmless.
    /// Existing turn/task ownership filtering must run before this method.
    pub(super) fn note_turn_outcome(&mut self, outcome: TurnOutcome) {
        if self.running && self.last_turn_outcome.is_none() {
            self.last_turn_outcome = Some(outcome);
            self.tool_visuals.finish();
            self.transcript_render_cache = None;
        }
    }

    pub(super) fn human_turn_wait(&self) -> Option<&'static str> {
        if self.approval.is_some() {
            Some(self.language.text(
                "等待你审批",
                "Waiting for your approval",
                "承認を待っています",
            ))
        } else if self.question.is_some() {
            Some(self.language.text(
                "等待你回答",
                "Waiting for your answer",
                "回答を待っています",
            ))
        } else {
            None
        }
    }

    pub(super) fn turn_phase_summary(&self) -> Option<String> {
        let started = self.turn_started?;
        let elapsed = started.elapsed();
        let idle = self
            .last_progress_at
            .map_or(elapsed, |progress| progress.elapsed());
        let phase = if let Some(wait) = self.human_turn_wait() {
            wait.to_owned()
        } else if self.compression_in_progress() {
            TurnPhase::Compressing.label(self.language)
        } else if self.waiting_background {
            TurnPhase::Background.label(self.language)
        } else {
            self.turn_phase.label(self.language)
        };
        Some(format_working_summary(
            self.language,
            self.runtime_turn,
            &phase,
            elapsed,
            idle,
        ))
    }

    /// A compact layout can keep this visible even when optional statistics are hidden.
    pub(super) fn essential_turn_status(&self) -> Option<String> {
        if self.running {
            self.turn_phase_summary()
        } else if let Some(wait) = self.human_turn_wait() {
            Some(wait.to_owned())
        } else {
            self.last_turn_outcome.map(|_| self.idle_turn_status())
        }
    }

    pub(super) fn idle_turn_status(&self) -> String {
        let outcome = self
            .last_turn_outcome
            .map(|outcome| outcome.label(self.language));
        if self.queued_prompts.is_empty() {
            outcome
                .unwrap_or_else(|| self.language.text("就绪", "Ready", "準備完了"))
                .to_owned()
        } else {
            let queued = self.language.text(
                "待发消息等待发送",
                "Queued messages waiting to send",
                "送信待ちがあります",
            );
            outcome.map_or_else(
                || queued.to_owned(),
                |outcome| format!("{outcome} · {queued}"),
            )
        }
    }

    pub(super) fn append_held_narration(&mut self, held: HeldNarration) -> String {
        let text = held.text;
        let previous_len = self.transcript.len();
        let previous_time = self.message_times.event_time.replace(held.recorded_at);
        self.append_transcript(format!("{ASSISTANT_ROW_PREFIX}{text}"));
        self.message_times.event_time = previous_time;
        if self.transcript.len() > previous_len {
            self.viewport_commit_stream(previous_len, StreamKind::Reply);
            // Do not stamp a paragraph with whichever model happens to be selected now.
            self.transcript_models[previous_len] = held.model;
        }
        text
    }

    pub(super) fn flush_held_narration(&mut self) -> Option<String> {
        let held = self.held_narration.take()?;
        Some(self.append_held_narration(held))
    }
}

#[cfg(test)]
#[path = "turn_status_tests.rs"]
mod tests;
