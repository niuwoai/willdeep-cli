//! 中途文字与工具调用直接落进聊天区。
//!
//! 此前模型在两次工具调用之间说的话只作「思考中」临时行，下一次调工具就擦掉；
//! 工具调用只进活动区，折叠态只露最近两条。几十次工具调用的任务跑十分钟，聊天区
//! 一片空白，用户分不清是在干活还是卡死了。现在：
//!
//! - 详细模式里每段定稿的中途文字都作 `WillDeep:` 行落进记录；简洁模式只留最近
//!   一段，画在聊天区底部那一行会替换的状态上，轮次收尾才落成正文；
//! - 流式增量仍走临时行预览，压过那一行状态；
//! - 每次工具调用先落一行 `· … 名字 · 摘要`，完成后原地改成 `✓` / `✗`；
//! - 收尾文字只追加还没显示过的部分（比如轮次上限提示），不重复最后一段；
//! - 显示层把每段连续工具行收起：早先的段一行汇总，最新的段一行汇总加最新一条，
//!   `/tools` 展开。夹在中间、当前可见的思考行不切段，一段连续的只留最后一条。
//!
//! 进程内轮次与 Runtime 轮次共用这一套，行为一致。
use super::*;

/// 工具行状态标记：进行中 / 成功 / 失败。
const TOOL_PENDING: &str = "…";
const TOOL_DONE: &str = "✓";
const TOOL_FAILED: &str = "✗";

/// 最新那段连续工具行最多占这么多行：一行汇总加最新一条，正在跑的那条永远看得见。
/// 更早的段一律只留一行汇总（只有一条时原样保留）。
const LAST_TOOL_RUN_ROWS: usize = 2;
/// 汇总行的标记，与三种工具行标记都不同；渲染层照样按 `· ` 账目行的灰色画。
const TOOL_FOLD_MARKER: &str = "⋯";
const THOUGHT_ENTRY_PREFIX: &str = "· + Thought: ";
const THOUGHT_EXPANDED_PREFIX: &str = "· − Thought: ";

/// 一行工具记录。`· ` 前缀让渲染层按账目行的灰色画，不与正文抢注意力。
fn tool_line(marker: &str, name: &str, detail: Option<&str>) -> String {
    match detail.map(str::trim).filter(|detail| !detail.is_empty()) {
        Some(detail) => format!("· {marker} {name} · {detail}"),
        None => format!("· {marker} {name}"),
    }
}

/// 会话重开时按记录回放工具行：有结果的画 ✓，没结果的画 …（轮次在那里被打断）。
/// 记录里没有成败，回放不画 ✗。
pub(super) fn replayed_tool_row(trace: &willdeep_core::conversation::ToolTrace) -> String {
    tool_line(
        if trace.completed {
            TOOL_DONE
        } else {
            TOOL_PENDING
        },
        &trace.name,
        trace.detail.as_deref(),
    )
}

/// 这行是不是一段思考记录。思考行是显示层才看得到的活动记录，模型每调一次
/// 工具前都会落一段，它不该把工具行切成两段——切了之后每段只剩一条，工具
/// 行就永远折不起来。
fn is_thought_row(line: &str) -> bool {
    line.starts_with(THOUGHT_ENTRY_PREFIX) || line.starts_with(THOUGHT_EXPANDED_PREFIX)
}

/// 这行是不是工具行；是的话给出它的状态标记。
fn tool_row_marker(line: &str) -> Option<&'static str> {
    let rest = line.strip_prefix("· ")?;
    [TOOL_PENDING, TOOL_DONE, TOOL_FAILED]
        .into_iter()
        .find(|marker| {
            rest.strip_prefix(marker)
                .is_some_and(|tail| tail.starts_with(' '))
        })
}

pub(super) fn is_tool_display_row(line: &str) -> bool {
    line.starts_with("· ⋯ ") || tool_row_marker(line).is_some()
}

fn tool_row_name(line: &str) -> Option<&str> {
    let marker = tool_row_marker(line)?;
    let rest = line.strip_prefix("· ")?.strip_prefix(marker)?.trim_start();
    Some(rest.split(" · ").next().unwrap_or(rest))
}

/// 临时行里此刻流的是什么：思维链还是正文。换源时清缓冲，两种文字不拼在一起。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StreamKind {
    Reasoning,
    Reply,
}

/// 旧的滚动字幕测试使用的最大高度。
#[cfg(test)]
const THINKING_ROWS: usize = 3;

/// 从尾部截取一段，使 `prefix + 结果` 在 `width` 列内不超过 `rows` 行。空白折叠成
/// 单个空格，截过的以 `…` 开头。
#[cfg(test)]
pub(super) fn tail_within_rows(prefix: &str, text: &str, width: usize, rows: usize) -> String {
    let width = width.max(1);
    let fits = |candidate: &str| visual_lines(&format!("{prefix}{candidate}"), width) <= rows;
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if fits(&compact) {
        return compact;
    }
    let chars: Vec<char> = compact.chars().collect();
    // 每行最多 width 个字符，先按这个上界取尾巴，再逐步收缩到装得下为止。
    let mut keep = (rows * width).min(chars.len());
    loop {
        let candidate: String = std::iter::once('…')
            .chain(chars[chars.len() - keep..].iter().copied())
            .collect();
        if keep == 0 || fits(&candidate) {
            return candidate;
        }
        keep -= (keep / 10).max(1);
    }
}

impl App {
    /// 聊天区底部多出来的那一行。正在流的内容优先：思维链只显示耗时摘要，
    /// 正文显示最近一段回复。没在流时，简洁模式把还没落进记录的最近一段中途文字
    /// 摆在这里——干活过程中永远只有这一行在变，收尾才写进记录。
    pub(super) fn live_transcript_row(&self) -> Option<String> {
        self.transient_row()
            .or_else(|| self.concise_status_row())
    }

    /// 聊天区底部的临时行：思维链只显示耗时摘要；正文显示最近一段回复。
    pub(super) fn transient_row(&self) -> Option<String> {
        let thought = self.transient_thought.as_deref()?;
        let label = self.transient_label();
        Some(match self.transient_kind {
            StreamKind::Reasoning => format!("· + Thought: {}", self.thought_elapsed_label()),
            StreamKind::Reply => format!("WillDeep · {label}: {thought}"),
        })
    }

    /// 简洁模式的那一行状态：旋转符、最近一段中途文字、已运行时长。
    /// 计划卡片走记录，不从这里过。轮次没在跑（比如中断后补写之前的那一帧）不画。
    fn concise_status_row(&self) -> Option<String> {
        if !self.running || !willdeep_core::ResponseStyle::uses_short(self.response_style) {
            return None;
        }
        let held = self.held_narration.as_deref()?.trim();
        if held.is_empty() {
            return None;
        }
        let elapsed = self
            .turn_started
            .map_or(Duration::ZERO, |started| started.elapsed());
        let mut one_line = String::new();
        for character in held.chars() {
            one_line.push(if character.is_whitespace() { ' ' } else { character });
            if one_line.chars().count() >= THOUGHT_PREVIEW_CHARS {
                break;
            }
        }
        Some(format!(
            "{} {one_line} · {}",
            progress_spinner(elapsed),
            format_elapsed_span(elapsed.as_secs_f32(), 0)
        ))
    }

    /// 流式增量进临时行：只留最后 [`THOUGHT_PREVIEW_CHARS`] 字，进程内轮次与
    /// Runtime 轮次共用。思考型模型正文常为空，思维链那行是用户唯一能看到的
    /// 「它此刻在干嘛」。
    pub(super) fn stream_transient(&mut self, kind: StreamKind, text: &str) {
        let text = terminal_safe_text(text);
        if self.transient_kind != kind {
            if self.transient_kind == StreamKind::Reasoning {
                self.finish_thought();
            } else {
                self.transient_thought = None;
            }
            self.transient_kind = kind;
        }
        if kind == StreamKind::Reasoning && self.thought_started_at.is_none() {
            self.thought_started_at = Some(Instant::now());
        }
        self.activity_line = match kind {
            StreamKind::Reasoning => self.language.text("正在思考", "Thinking", "思考中"),
            StreamKind::Reply => {
                self.language
                    .text("正在接收回复", "Receiving reply", "応答を受信中")
            }
        }
        .to_owned();
        let mut content = self.transient_thought.take().unwrap_or_default();
        content.push_str(&text);
        if kind == StreamKind::Reply {
            let skip = content
                .chars()
                .count()
                .saturating_sub(THOUGHT_PREVIEW_CHARS);
            content = content.chars().skip(skip).collect();
        }
        self.transient_thought = Some(content);
    }

    fn thought_elapsed_label(&self) -> String {
        let elapsed = self
            .thought_started_at
            .map_or(Duration::ZERO, |started| started.elapsed());
        format!("{:.1}s", elapsed.as_secs_f32())
    }

    pub(super) fn finish_thought(&mut self) {
        if self.transient_kind != StreamKind::Reasoning {
            return;
        }
        if let Some(thought) = self
            .transient_thought
            .take()
            .filter(|text| !text.trim().is_empty())
        {
            let elapsed = self.thought_elapsed_label();
            self.append_transcript(format!("{THOUGHT_ENTRY_PREFIX}{elapsed}\n{thought}"));
        }
        self.thought_started_at = None;
    }

    pub(super) fn toggle_thought_at(&mut self, y: u16) -> bool {
        let Some((_, thought_order)) = self.thought_hit_rows.iter().find(|(row, _)| *row == y)
        else {
            return false;
        };
        let Some(index) = self
            .transcript
            .iter()
            .enumerate()
            .filter(|(index, _)| *index >= self.visible_thought_start())
            .filter(|(_, row)| {
                row.starts_with(THOUGHT_ENTRY_PREFIX) || row.starts_with(THOUGHT_EXPANDED_PREFIX)
            })
            .nth(*thought_order)
            .map(|(index, _)| index)
        else {
            return false;
        };
        let row = &mut self.transcript[index];
        if row.starts_with(THOUGHT_ENTRY_PREFIX) {
            *row = row.replacen(THOUGHT_ENTRY_PREFIX, THOUGHT_EXPANDED_PREFIX, 1);
        } else {
            *row = row.replacen(THOUGHT_EXPANDED_PREFIX, THOUGHT_ENTRY_PREFIX, 1);
        }
        self.transcript_render_cache = None;
        self.refresh_transcript_height();
        self.scroll_from_bottom = self.scroll_from_bottom.min(self.max_scroll());
        true
    }

    /// 临时行前面的标签，随来源变。
    pub(super) fn transient_label(&self) -> &'static str {
        match self.transient_kind {
            StreamKind::Reasoning => self.language.text("思考中", "thinking", "思考中"),
            StreamKind::Reply => self.language.text("回复中", "replying", "応答中"),
        }
    }

    /// `/version`：终端这边的 CLI 版本与 Runtime 守护进程的版本各是多少。Runtime
    /// 版本来自快照，没连上就说没连上；两边不一致时点明，因为命令实际在 Runtime 里跑。
    pub(super) fn version_report(&self) -> String {
        let runtime = self.runtime_version.clone().unwrap_or_else(|| {
            self.language
                .text(
                    if self.runtime_reachable {
                        "版本未知（已连接）"
                    } else {
                        "未连接"
                    },
                    if self.runtime_reachable {
                        "version unknown (connected)"
                    } else {
                        "not connected"
                    },
                    if self.runtime_reachable {
                        "接続済み（バージョン不明）"
                    } else {
                        "未接続"
                    },
                )
                .to_owned()
        });
        let mut report = format!("System: CLI {} · Runtime {runtime}", willdeep_core::VERSION);
        if self.stale_runtime_version().is_some() {
            report.push_str(self.language.text(
                " · 版本不一致：",
                " · versions differ: ",
                " · バージョン不一致：",
            ));
            report.push_str(self.version_alignment_hint());
        }
        if let Some(diagnostic) = &self.runtime_diagnostic {
            report.push_str(
                self.language
                    .text(" · 诊断：", " · Diagnostic: ", " · 診断："),
            );
            let diagnostic = willdeep_core::judge::redact_credentials(diagnostic);
            report.extend(
                diagnostic
                    .chars()
                    .filter(|character| !character.is_control())
                    .take(1000),
            );
            report.push_str(self.language.text(
                " · 恢复：可运行 `/daemon status` 查看状态，再用 `/daemon start` 重试。",
                " · Recovery: run `/daemon status` for details, then `/daemon start` to retry.",
                " · 復旧：`/daemon status` で状態を確認し、`/daemon start` で再試行してください。",
            ));
        }
        if self.runtime_auto_start_tried && self.runtime_version.is_none() {
            report.push_str(self.language.text(
                " · 已自动尝试启动一次；请查看启动结果，或再次运行 `/daemon start` 重试。",
                " · Automatic startup was attempted once; check its result or run `/daemon start` to retry.",
                " · 自動起動を一度試しました。結果を確認するか、`/daemon start` で再試行してください。",
            ));
        }
        report
    }

    /// 输入框标题上的状态词：跑的时候回车只是排队，空闲时才轮到用户。
    pub(super) fn composer_state(&self) -> (&'static str, Color) {
        if self.running
            && self.waiting_background
            && self.approval.is_none()
            && self.question.is_none()
        {
            (
                self.language.text(
                    "后台任务运行中 · 可继续输入 · 普通消息回车发送 · Ctrl+Q 管理",
                    "Background running · Keep typing · Enter sends text · Ctrl+Q manage",
                    "バックグラウンド実行中 · 入力可 · Enter でテキスト送信 · Ctrl+Q 管理",
                ),
                Color::Cyan,
            )
        } else if self.running {
            (
                self.language.text(
                    "本轮进行中 · 回车发送文字 · Ctrl+Q 队列 · Esc 中止",
                    "Turn running · Enter sends text · Ctrl+Q queue · Esc stops",
                    "ターン実行中 · Enter で送達 · Ctrl+Q キュー · Esc 中断",
                ),
                Color::Yellow,
            )
        } else if !self.queued_prompts.is_empty() {
            (
                self.language.text(
                    "待发消息等待发送 · Ctrl+Q 管理",
                    "Queued messages waiting to send · Ctrl+Q manage",
                    "送信待ちのメッセージがあります · Ctrl+Q 管理",
                ),
                Color::Yellow,
            )
        } else {
            (
                self.language.text(
                    "轮到你 · Enter 发送 · Cmd+Enter 入队",
                    "Your turn · Enter sends · Cmd+Enter queues for later",
                    "あなたの番 · Enter で送信 · Cmd+Enter でキューに追加",
                ),
                Color::Green,
            )
        }
    }

    pub(super) fn note_delayed_command(&mut self, seconds: u64) {
        let notice = self.language.pick(
            format!("等待命令已转入后台（先等待约 {seconds} 秒，再执行后续步骤）。可以继续输入，原任务会继续运行，完成后自动通知。"),
            format!("Delayed command moved to background ({seconds}s before subsequent steps). Keep typing; the original job continues and completion will be reported automatically."),
            format!("待機コマンドをバックグラウンドに移しました（後続処理まで約 {seconds} 秒）。入力を続けられます。完了後に通知します。"),
        );
        self.append_transcript(format!("System: {notice}"));
        self.notice = Some(notice);
    }

    pub(super) fn set_background_wait(&mut self, waiting: bool) {
        if waiting && !self.waiting_background {
            let message = self.language.text(
                "后台等待中，可以继续输入；普通消息回车后自动发送，后台任务继续运行",
                "Waiting in background; keep typing. Enter sends text while the background job continues",
                "バックグラウンド待機中。入力して Enter で送信できます。ジョブは継続します",
            ).to_owned();
            self.record_progress(message.clone());
            self.notice = Some(message);
        }
        self.waiting_background = waiting;
    }

    /// 本轮在跑时排队的提示词先带「待发」标记回显；此前它和真正发出去的长得
    /// 一模一样，用户分不清哪句还没发。
    pub(super) fn queued_prompt_row(&self, text: &str) -> String {
        format!(
            "You: {}{}",
            self.language
                .text("［待发］ ", "[queued] ", "［送信待ち］ "),
            terminal_safe_text(text)
        )
    }

    /// 送进正在跑的那一轮却没赶上（任务在下一次调模型前就结束了）的插话：
    /// 说一声，然后排队，本轮结束后照常发出，一句话都不丢。
    pub(super) fn requeue_steering(&mut self, text: String) {
        self.append_transcript(format!(
            "System: {}",
            self.language.text(
                "上一条插话没赶上本轮，已排队，本轮结束后发送",
                "The last message missed this turn; queued to send when it finishes",
                "直前のメッセージはこのターンに間に合わず、終了後に送信するためキューに入れました",
            )
        ));
        // 还压着没显示的，就以「待发」行出现，发出时照常去掉标记。
        if let Some(index) = self
            .deferred_steering
            .iter()
            .position(|value| value.text == text)
        {
            let delivered = self.deferred_steering.remove(index);
            let row = self.queued_prompt_row(&text);
            self.append_transcript_at(row, delivered.sent_at);
        }
        self.queued_prompts.push_back(QueuedPrompt {
            remote: None,
            text,
            attachments: Vec::new(),
        });
    }

    /// 送达只代表收件箱接收成功；到模型下一步读取前持续显示待阅状态。
    pub(super) fn note_steering_delivered(&mut self, text: &str, sent_at: u64) {
        self.waiting_background = false;
        let pending_copies = self
            .deferred_steering
            .iter()
            .filter(|item| item.text == text)
            .count();
        self.message_times
            .reserve_delivery(&self.transcript, text, pending_copies, sent_at);
        self.deferred_steering.push(DeliveredPrompt {
            text: text.to_owned(),
            sent_at,
        });
    }

    /// 模型新的一步开始。插话正是在这之前注入对话的，压着的这时落进聊天区。
    pub(super) fn note_model_step_started(&mut self) {
        self.waiting_background = false;
        self.finish_thought();
        self.transient_thought = None;
        self.flush_deferred_steering();
    }

    pub(super) fn flush_deferred_steering(&mut self) {
        for message in std::mem::take(&mut self.deferred_steering) {
            self.append_transcript_at(format!("You: {}", message.text), message.sent_at);
        }
    }

    /// 真正发出后移除最早的同文待发占位，并在当前轮次边界回显原文。
    /// 只改旧行会把用户消息留在上一轮中间，下一轮回复前就看不到提问。
    pub(super) fn mark_queued_prompt_sent(&mut self, text: &str) {
        let queued = self.queued_prompt_row(text);
        let mut sent_at = message_time::now();
        if let Some(index) = self.transcript.iter().position(|line| *line == queued) {
            sent_at = self.message_times.time_at(index).unwrap_or(sent_at);
            self.transcript.remove(index);
            self.message_times.remove(index);
            self.refresh_transcript_height();
        }
        self.append_transcript_at(format!("You: {text}"), sent_at);
    }
}

/// 折叠后的聊天区视图：只给显示层用，记录本身一行不动。
pub(super) struct FoldedTranscript {
    pub(super) rows: Vec<String>,
    /// 每条原始记录在折叠视图里的位置；被收起的工具行指向替它们说话的汇总行，
    /// 搜索跳转命中它们时就落在汇总上。
    pub(super) index_of: Vec<usize>,
}

/// 每段连续的活动行在显示层收起。活动行指工具行，以及夹在它们中间、当前
/// 可见的思考行——思考行不切段。收起时只收工具行：不是最新那段的一律收成
/// 一行汇总；最新那段最多留 [`LAST_TOOL_RUN_ROWS`] 条工具行，正在跑的那条
/// 永远看得见。一段连续的思考行只留最后一条，更早的指向它（搜索跳转仍落得上）；
/// 用户展开过的那条原样留着。`expanded` 只展开工具行（`/tools`），不展开思考。
pub(super) fn fold_tool_rows(
    entries: &[String],
    expanded: bool,
    language: Language,
) -> FoldedTranscript {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut cursor = 0;
    while cursor < entries.len() {
        if tool_row_marker(&entries[cursor]).is_none() && !is_thought_row(&entries[cursor]) {
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor < entries.len()
            && (tool_row_marker(&entries[cursor]).is_some() || is_thought_row(&entries[cursor]))
        {
            cursor += 1;
        }
        // 思考也要收：一段连续的只留最后一条。纯思考段与夹着工具的段同一条规则。
        runs.push((start, cursor));
    }
    let last_run = runs.last().copied();
    let latest_turn = entries
        .iter()
        .rposition(|row| row.starts_with("You: "))
        .unwrap_or(0);
    let mut rows = Vec::with_capacity(entries.len());
    let mut index_of = Vec::with_capacity(entries.len());
    let mut next_run = 0;
    let mut cursor = 0;
    while cursor < entries.len() {
        let Some(&(start, end)) = runs.get(next_run).filter(|(start, _)| *start == cursor) else {
            index_of.push(rows.len());
            rows.push(entries[cursor].clone());
            cursor += 1;
            continue;
        };
        next_run += 1;
        cursor = end;
        let run = &entries[start..end];
        let tool_count = run
            .iter()
            .filter(|row| tool_row_marker(row).is_some())
            .count();
        let is_last = start >= latest_turn && last_run == Some((start, end));
        let keep_tools = if expanded || tool_count == 1 {
            tool_count
        } else if !is_last {
            0
        } else if tool_count <= LAST_TOOL_RUN_ROWS {
            tool_count
        } else {
            1
        };
        let hidden_tools = tool_count - keep_tools;
        // 收起的总是靠前的工具行：正在跑的那条（最新的）永远留着。
        let hidden = run
            .iter()
            .filter(|row| tool_row_marker(row).is_some())
            .take(hidden_tools)
            .cloned()
            .collect::<Vec<_>>();
        // 一段连续的思考只留最后一条。展开过的不算「被后来的盖住」：用户点开的
        // 那条留在原处，它后面的折叠照旧。被收起的指向留下的那条，搜索跳转仍落得上。
        let kept_thought = run.iter().rposition(|line| is_thought_row(line));
        let folded_thought = |offset: usize| {
            is_thought_row(&run[offset])
                && !run[offset].starts_with(THOUGHT_EXPANDED_PREFIX)
                && kept_thought.is_some_and(|kept| offset < kept)
        };
        // 先定每行的去向，再统一编号。被收起的工具行共用一行汇总，被收起的思考行
        // 指向本段留下的最后一条思考。
        let summary_at = rows.len();
        let mut placed = Vec::with_capacity(run.len());
        let mut emitted = usize::from(hidden_tools > 0);
        for (offset, line) in run.iter().enumerate() {
            let hidden_tool = tool_row_marker(line).is_some()
                && run[..=offset]
                    .iter()
                    .filter(|row| tool_row_marker(row).is_some())
                    .count()
                    <= hidden_tools;
            if hidden_tool {
                placed.push(summary_at);
            } else if folded_thought(offset) {
                // 占位，等最后一条思考的位置定下来再回填。
                placed.push(usize::MAX);
            } else {
                placed.push(summary_at + emitted);
                emitted += 1;
            }
        }
        if let Some(kept) = kept_thought {
            for offset in 0..run.len() {
                if folded_thought(offset) {
                    placed[offset] = placed[kept];
                }
            }
        }
        if hidden_tools > 0 {
            rows.push(fold_summary(&hidden, is_last, language));
        }
        for (offset, line) in run.iter().enumerate() {
            let hidden_tool = hidden_tools > 0 && placed[offset] == summary_at;
            if !hidden_tool && !folded_thought(offset) {
                debug_assert_eq!(placed[offset], rows.len());
                rows.push(line.clone());
            }
            index_of.push(placed[offset]);
        }
    }
    FoldedTranscript { rows, index_of }
}

/// `· ⋯ 已收起 12 条工具调用 · read_file×3 · run_command×9 · 2 失败 · /tools 展开`
/// 「/tools 展开」只挂在最新那段的汇总上，早先的段不重复提示。
fn fold_summary(hidden: &[String], with_hint: bool, language: Language) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut failed = 0;
    for line in hidden {
        if let Some(name) = tool_row_name(line) {
            *counts.entry(name).or_default() += 1;
        }
        if tool_row_marker(line) == Some(TOOL_FAILED) {
            failed += 1;
        }
    }
    let mut summary = language
        .text(
            "已收起 {n} 条工具调用",
            "{n} earlier tool calls folded",
            "ツール呼び出し {n} 件を折りたたみ",
        )
        .replace("{n}", &hidden.len().to_string());
    for (name, count) in counts {
        summary.push_str(&format!(" · {name}×{count}"));
    }
    if failed > 0 {
        summary.push_str(
            &language
                .text(" · {k} 失败", " · {k} failed", " · {k} 失敗")
                .replace("{k}", &failed.to_string()),
        );
    }
    if with_hint {
        summary.push_str(language.text(
            " · /tools 展开",
            " · /tools to expand",
            " · /tools で展開",
        ));
    }
    format!("· {TOOL_FOLD_MARKER} {summary}")
}

impl App {
    fn visible_thought_start(&self) -> usize {
        self.transcript
            .iter()
            .rposition(|row| row.starts_with("You: "))
            .unwrap_or(0)
            .max(self.archive_floor)
    }

    /// 聊天区实际画出来的记录：工具行按规则收起，其余原样。
    pub(super) fn display_transcript(&self) -> FoldedTranscript {
        // 归档区不进渲染循环：只折叠还可见的那一段，顶上补一行说明。
        // 记录本身没动，`archive_floor` 只是渲染起点。
        let floor = self.archive_floor.min(self.transcript.len());
        let live = &self.transcript[floor..];
        let thought_start = self.visible_thought_start();
        let visible_indices = live
            .iter()
            .enumerate()
            .filter(|(index, row)| {
                !is_thought_row(row) || (self.ui.show_thinking && floor + index >= thought_start)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let mut entries = visible_indices
            .iter()
            .map(|&index| live[index].clone())
            .collect::<Vec<_>>();
        if floor > 0 {
            entries.insert(0, self.archive_placeholder());
        }
        let mut folded = fold_tool_rows(&entries, self.tool_rows_expanded, self.language);
        let shift = usize::from(floor > 0);
        let mut index_of = Vec::with_capacity(live.len() + shift);
        if floor > 0 {
            index_of.push(0);
        }
        for index in 0..live.len() {
            let visible = visible_indices.partition_point(|&source| source < index);
            index_of.push(
                folded
                    .index_of
                    .get(visible + shift)
                    .copied()
                    .unwrap_or_else(|| folded.rows.len().saturating_sub(1)),
            );
        }
        folded.index_of = index_of;
        folded
    }

    /// 归档区的说明行：告诉用户上面还有东西、怎么放回来。
    fn archive_placeholder(&self) -> String {
        let rounds = self
            .transcript
            .iter()
            .take(self.archive_floor)
            .filter(|row| row.starts_with(TURN_DIVIDER_PREFIX))
            .count();
        format!(
            "System: {}",
            self.language.pick(
                format!("⋯ 更早的 {rounds} 轮已归档 · 翻到顶部继续载入"),
                format!("⋯ {rounds} earlier turns archived · scroll to the top to load them"),
                format!("⋯ 以前の {rounds} ターンをアーカイブ · 先頭までスクロールで読み込み"),
            )
        )
    }

    /// 记录变了之后按折叠视图重算总高度；滚动、跟随底部全靠它。
    pub(super) fn refresh_transcript_height(&mut self) {
        self.transcript_height = self.timed_transcript_height();
    }

    /// `/tools`：临时展开或收起全部工具行。返回给用户看的一句反馈。
    pub(super) fn toggle_tool_rows(&mut self) -> String {
        self.tool_rows_expanded = !self.tool_rows_expanded;
        self.refresh_transcript_height();
        self.scroll_from_bottom = self.scroll_from_bottom.min(self.max_scroll());
        format!(
            "System: {}",
            if self.tool_rows_expanded {
                self.language.text(
                    "工具调用行已全部展开 · 再输入 /tools 收起",
                    "All tool-call rows expanded · /tools again to fold",
                    "ツール行をすべて展開 · もう一度 /tools で折りたたみ",
                )
            } else {
                self.language.text(
                    "工具调用行已收起，只留最新几条 · 再输入 /tools 展开",
                    "Tool-call rows folded to the latest few · /tools again to expand",
                    "ツール行を最新数件に折りたたみ · もう一度 /tools で展開",
                )
            }
        )
    }

    /// 模型这一轮定稿的一段话。记住它，收尾时据此去重。
    ///
    /// 详细模式逐段落进记录。简洁模式只留最近一段：干活过程中聊天区只该有计划
    /// 卡片和一行状态，每段话都落一行会把过程堆成详细模式。计划围栏例外，它要
    /// 原位更新卡片，两种模式都直接进记录。
    pub(super) fn note_narration(&mut self, text: &str) {
        // 目标完成标签是给引擎认的协议，不往聊天区送；存储层仍保留原文。
        let text = willdeep_core::conversation::visible_assistant_text(text).trim();
        if text.is_empty() {
            return;
        }
        self.finish_thought();
        self.transient_thought = None;
        self.activity_line = self
            .language
            .text("正在整理思路", "Working through it", "考えを整理中")
            .to_owned();
        if self.concise_turn() && !willdeep_core::conversation::is_plan_reply(text) {
            // 时间按最终落进记录的那行记：进记录前还会过一遍终端安全处理。
            let row = terminal_safe_text(&format!("WillDeep: {text}"));
            self.message_times
                .reserve(&row, self.message_times.recording_time());
            self.held_narration = Some(text.to_owned());
        } else {
            self.append_transcript(format!("WillDeep: {text}"));
            self.held_narration = None;
        }
        self.turn_narration = Some(text.to_owned());
    }

    /// 这一轮按简洁风格显示。没显式选过风格即简洁，与提示词侧同一条判定。
    fn concise_turn(&self) -> bool {
        willdeep_core::ResponseStyle::uses_short(self.response_style)
    }

    /// 工具刚发起：先占一行，完成时原地改标记。
    pub(super) fn note_tool_requested(&mut self, name: &str, detail: Option<&str>) {
        self.finish_thought();
        self.transient_thought = None;
        self.append_transcript(tool_line(TOOL_PENDING, name, detail));
    }

    /// 工具结束：把最近那条同名的进行中行改成成功/失败。只按名字对行，完成事件
    /// 不带摘要也能对上；实在找不到（比如重开会话后才收到完成事件）就补一行，
    /// 宁可多一行也不丢结果。
    pub(super) fn note_tool_completed(&mut self, name: &str, detail: Option<&str>, is_error: bool) {
        let marker = if is_error { TOOL_FAILED } else { TOOL_DONE };
        let pending = tool_line(TOOL_PENDING, name, None);
        let with_detail = format!("{pending} · ");
        let settled = self
            .transcript
            .iter_mut()
            .rev()
            .find(|line| **line == pending || line.starts_with(&with_detail))
            .map(|line| *line = line.replacen(TOOL_PENDING, marker, 1))
            .is_some();
        if settled {
            self.refresh_transcript_height();
        } else {
            self.append_transcript(tool_line(marker, name, detail));
        }
    }

    /// 收尾文字：只追加还没显示过的部分，返回追加了什么，供会话镜像持久化。
    pub(super) fn note_reply(&mut self, final_text: &str) -> Option<String> {
        self.finish_thought();
        self.transient_thought = None;
        // 与中途文字同一条投影：先剥协议标签，再比「这段是不是已经显示过」。
        let final_text = willdeep_core::conversation::visible_assistant_text(final_text);
        let remainder = unshown_reply(final_text, self.turn_narration.as_deref());
        // 简洁模式的中途文字只摆在状态行上。收尾是它进记录的时候：正文与它
        // 相同就落这一段，正文在它前面拼了提示就两段都落。之后清掉，免得
        // 轮次复位再补写一遍。
        let held = self.held_narration.take();
        if let Some(held) = held.as_deref().map(str::trim).filter(|held| !held.is_empty()) {
            let narrated = self.turn_narration.as_deref().map(str::trim);
            if narrated == Some(held)
                && let Some(head) = &remainder
                && final_text.trim() != head.trim()
            {
                self.append_transcript(format!("WillDeep: {head}"));
                self.append_transcript(format!("WillDeep: {held}"));
                return Some(final_text.trim().to_owned());
            }
            self.append_transcript(format!("WillDeep: {held}"));
        }
        if let Some(text) = &remainder {
            self.append_transcript(format!("WillDeep: {text}"));
        }
        match (&held, &remainder) {
            (Some(held), Some(head)) if final_text.trim() != head.trim() => {
                Some(format!("{head}\n\n{held}"))
            }
            (Some(held), _) => Some(held.clone()),
            (None, remainder) => remainder.clone(),
        }
    }
}

/// 收尾文字里还没在聊天区出现过的部分。
///
/// 进程内 `final_text` 就是最后一段中途文字；撞轮次上限时守护进程会在前面
/// 拼一段提示（`{提示}\n\n{最后一段}`）。两种情况都只追加新增的那部分，
/// 否则最后一段会在聊天区出现两遍。
fn unshown_reply(final_text: &str, narrated: Option<&str>) -> Option<String> {
    let final_text = final_text.trim();
    if final_text.is_empty() {
        return None;
    }
    let Some(narrated) = narrated.map(str::trim).filter(|value| !value.is_empty()) else {
        return Some(final_text.to_owned());
    };
    if final_text == narrated {
        return None;
    }
    match final_text.strip_suffix(narrated).map(str::trim_end) {
        Some("") => None,
        Some(head) => Some(head.to_owned()),
        None => Some(final_text.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 交替的 run_command / read_file，每第三条失败。
    fn tool_rows(count: usize) -> Vec<String> {
        (0..count)
            .map(|index| {
                tool_line(
                    if index % 3 == 2 {
                        TOOL_FAILED
                    } else {
                        TOOL_DONE
                    },
                    if index % 2 == 0 {
                        "run_command"
                    } else {
                        "read_file"
                    },
                    None,
                )
            })
            .collect()
    }

    #[test]
    fn tool_line_keeps_detail_only_when_present() {
        assert_eq!(
            tool_line(TOOL_PENDING, "run_command", Some(" cargo test ")),
            "· … run_command · cargo test"
        );
        assert_eq!(
            tool_line(TOOL_DONE, "read_file", Some("  ")),
            "· ✓ read_file"
        );
        assert_eq!(tool_line(TOOL_FAILED, "edit_file", None), "· ✗ edit_file");
    }

    #[test]
    fn tool_rows_are_recognised_by_marker_and_name() {
        assert_eq!(
            tool_row_marker("· … run_command · cargo test"),
            Some(TOOL_PENDING)
        );
        assert_eq!(
            tool_row_name("· ✗ run_command · cargo test -p"),
            Some("run_command")
        );
        assert_eq!(tool_row_name("· ✓ read_file"), Some("read_file"));
        // 账目行、汇总行、正文都不是工具行。
        assert_eq!(tool_row_marker("· total 3.2s"), None);
        assert_eq!(tool_row_marker("· ⋯ 4 earlier tool calls folded"), None);
        assert_eq!(tool_row_marker("WillDeep: ✓ done"), None);
    }

    #[test]
    fn one_or_two_row_runs_stay_as_they_are() {
        for count in 1..=LAST_TOOL_RUN_ROWS {
            let entries = tool_rows(count);
            let folded = fold_tool_rows(&entries, false, Language::En);
            assert_eq!(folded.rows, entries);
            assert_eq!(folded.index_of, (0..count).collect::<Vec<_>>());
        }
    }

    #[test]
    fn the_latest_run_keeps_one_summary_plus_its_newest_row() {
        let mut entries = vec!["You: go".to_owned()];
        entries.extend(tool_rows(8));
        entries.push("WillDeep: done".to_owned());
        let folded = fold_tool_rows(&entries, false, Language::En);
        assert_eq!(
            folded.rows.len(),
            1 + LAST_TOOL_RUN_ROWS + 1,
            "{:?}",
            folded.rows
        );
        assert_eq!(folded.rows[0], "You: go");
        let summary = &folded.rows[1];
        assert!(
            summary.starts_with("· ⋯ 7 earlier tool calls folded"),
            "{summary}"
        );
        assert!(
            summary.contains(" · read_file×3 · run_command×4"),
            "{summary}"
        );
        assert!(summary.contains(" · 2 failed"), "{summary}");
        assert!(summary.ends_with(" · /tools to expand"), "{summary}");
        assert_eq!(folded.rows[2], entries[8], "the newest row stays visible");
        assert_eq!(folded.rows[3], "WillDeep: done");
        assert_eq!(folded.index_of, vec![0, 1, 1, 1, 1, 1, 1, 1, 2, 3]);
    }

    #[test]
    fn earlier_runs_collapse_to_one_line_and_expanded_view_shows_all() {
        let mut entries = tool_rows(5);
        entries.push("WillDeep: half way".to_owned());
        entries.extend(tool_rows(3));
        let folded = fold_tool_rows(&entries, false, Language::ZhCn);
        assert_eq!(
            folded.rows.len(),
            1 + 1 + LAST_TOOL_RUN_ROWS,
            "{:?}",
            folded.rows
        );
        assert!(
            folded.rows[0].starts_with("· ⋯ 已收起 5 条工具调用"),
            "{}",
            folded.rows[0]
        );
        assert!(
            !folded.rows[0].contains("/tools"),
            "the hint belongs to the latest run only: {}",
            folded.rows[0]
        );
        assert_eq!(folded.rows[1], "WillDeep: half way");
        assert!(
            folded.rows[2].starts_with("· ⋯ 已收起 2 条工具调用")
                && folded.rows[2].ends_with("/tools 展开"),
            "{}",
            folded.rows[2]
        );
        assert_eq!(folded.rows[3], entries[8]);
        assert_eq!(folded.index_of, vec![0, 0, 0, 0, 0, 1, 2, 2, 3]);

        // 早先的段只有一条时原样保留，不值得配一行汇总。
        let mut single = tool_rows(1);
        single.push("WillDeep: x".to_owned());
        single.extend(tool_rows(3));
        assert_eq!(
            fold_tool_rows(&single, false, Language::ZhCn).rows[0],
            single[0]
        );

        assert_eq!(fold_tool_rows(&entries, true, Language::ZhCn).rows, entries);
    }

    #[test]
    fn visible_thoughts_do_not_split_the_latest_tool_run() {
        let mut entries = vec!["You: go".to_owned()];
        for index in 0..4 {
            entries.push(format!("{THOUGHT_ENTRY_PREFIX}{index}.0s\nreason {index}"));
            entries.push(tool_line(
                TOOL_DONE,
                if index % 2 == 0 {
                    "run_command"
                } else {
                    "read_file"
                },
                None,
            ));
        }
        let folded = fold_tool_rows(&entries, false, Language::En);
        // 四段思考只留最后一条；四条工具行收成一条汇总加最新一条。
        assert_eq!(folded.rows.len(), 1 + 1 + 1 + 1, "{:?}", folded.rows);
        assert!(
            folded.rows[1].starts_with("· ⋯ 3 earlier tool calls folded"),
            "{}",
            folded.rows[1]
        );
        assert!(
            folded.rows[1].contains(" · read_file×1 · run_command×2"),
            "{}",
            folded.rows[1]
        );
        assert_eq!(
            folded
                .rows
                .iter()
                .filter(|row| row.contains("reason "))
                .count(),
            1,
            "a consecutive run keeps only its last thought: {:?}",
            folded.rows
        );
        assert!(folded.rows[2].contains("reason 3"), "{:?}", folded.rows);
        assert_eq!(
            folded
                .rows
                .iter()
                .filter(|row| tool_row_marker(row).is_some())
                .count(),
            1,
            "only the newest tool row stays: {:?}",
            folded.rows
        );
        // 被收起的工具行指向汇总行，被收起的思考行指向留下的那条。
        assert_eq!(folded.index_of, vec![0, 2, 1, 2, 1, 2, 1, 2, 3]);
    }

    #[test]
    fn a_run_of_consecutive_thoughts_keeps_only_the_last() {
        let entries = vec![
            "You: go".to_owned(),
            format!("{THOUGHT_ENTRY_PREFIX}1.0s\nfirst"),
            format!("{THOUGHT_ENTRY_PREFIX}2.0s\nsecond"),
            format!("{THOUGHT_ENTRY_PREFIX}3.0s\nthird"),
        ];
        let folded = fold_tool_rows(&entries, false, Language::En);
        assert_eq!(
            folded.rows,
            vec!["You: go".to_owned(), entries[3].clone()],
            "only the newest thought of a consecutive run stays"
        );
        assert_eq!(folded.index_of, vec![0, 1, 1, 1]);
        assert!(!folded.rows.iter().any(|row| row.contains("tool calls")));

        // 展开过的不参与折叠：它留在原处，后面的连续段照旧只留最后一条。
        let mut expanded = entries.clone();
        expanded[1] = expanded[1].replacen(THOUGHT_ENTRY_PREFIX, THOUGHT_EXPANDED_PREFIX, 1);
        let folded = fold_tool_rows(&expanded, false, Language::En);
        assert_eq!(folded.rows.len(), 3, "{:?}", folded.rows);
        assert!(folded.rows[1].contains("first"));
        assert!(folded.rows[2].contains("third"));
        assert_eq!(folded.index_of, vec![0, 1, 2, 2]);

        // `/tools` 只展开工具行，不把收起的思考放回来。
        let folded = fold_tool_rows(&entries, true, Language::En);
        assert_eq!(folded.rows.len(), 2);
    }

    #[test]
    fn tools_command_toggles_folding_and_reports() {
        let mut app = App::new(Vec::new(), Language::En);
        for line in tool_rows(9) {
            app.append_transcript(line);
        }
        assert_eq!(app.display_transcript().rows.len(), LAST_TOOL_RUN_ROWS);

        assert!(app.handle_slash_command("/tools", &SkillCatalog::default()));
        assert!(app.tool_rows_expanded);
        assert!(app.transcript.last().unwrap().starts_with("System: "));
        assert_eq!(app.display_transcript().rows.len(), 9 + 1);

        assert!(app.handle_slash_command("/tools", &SkillCatalog::default()));
        assert!(!app.tool_rows_expanded);
        assert_eq!(app.display_transcript().rows.len(), LAST_TOOL_RUN_ROWS + 2);
    }

    #[test]
    fn transient_line_switches_source_and_preserves_reasoning_separately() {
        let mut app = App::new(Vec::new(), Language::En);
        app.stream_transient(StreamKind::Reasoning, "先看日志");
        app.stream_transient(StreamKind::Reasoning, "，再改");
        assert_eq!(app.transient_thought.as_deref(), Some("先看日志，再改"));
        assert_eq!(app.transient_label(), "thinking");
        assert_eq!(app.activity_line, "Thinking");

        // 正文一来，思维链进入独立折叠记录，两种文字不拼在一起。
        app.stream_transient(StreamKind::Reply, "好的");
        assert!(app.transcript[0].starts_with(THOUGHT_ENTRY_PREFIX));
        assert!(app.transcript[0].ends_with("先看日志，再改"));
        assert_eq!(app.transient_thought.as_deref(), Some("好的"));
        assert_eq!(app.transient_label(), "replying");
        assert_eq!(app.activity_line, "Receiving reply");

        app.stream_transient(StreamKind::Reply, &"x".repeat(THOUGHT_PREVIEW_CHARS * 2));
        assert_eq!(
            app.transient_thought
                .as_deref()
                .map(|value| value.chars().count()),
            Some(THOUGHT_PREVIEW_CHARS)
        );
    }

    #[test]
    fn terminal_safe_text_expands_tabs_and_keeps_newlines() {
        assert_eq!(terminal_safe_text("a\tb\nc\td"), "a   b\nc   d");
        assert_eq!(
            terminal_safe_text("\tif err != nil {"),
            "    if err != nil {"
        );
        assert_eq!(terminal_safe_text("x\r\u{1b}[0m"), "x\\u{1b}[0m");
    }

    #[test]
    fn thinking_tail_fits_the_last_rows_and_collapses_whitespace() {
        let prefix = "· thinking: ";
        let text = "word ".repeat(200);
        let tail = tail_within_rows(prefix, &text, 40, THINKING_ROWS);
        assert!(visual_lines(&format!("{prefix}{tail}"), 40) <= THINKING_ROWS);
        assert!(tail.starts_with('…'), "{tail}");
        assert!(tail.ends_with("word"), "{tail}");
        assert_eq!(
            tail_within_rows(prefix, "short\n\n  thought", 40, THINKING_ROWS),
            "short thought"
        );
    }

    #[test]
    fn transient_reasoning_is_a_compact_summary_and_full_thought_is_retained() {
        let mut app = App::new(Vec::new(), Language::En);
        app.transcript_width = 30;
        app.stream_transient(StreamKind::Reasoning, &"think ".repeat(100));
        let row = app.transient_row().unwrap();
        assert!(row.starts_with("· + Thought: "), "{row}");
        assert!(row.ends_with('s') && !row.ends_with("ms"), "{row}");
        assert!(!row.contains("think"));

        // 正文开始后，完整思维链折叠进可展开的记录，回复继续单独预览。
        app.stream_transient(StreamKind::Reply, "Hello");
        assert!(app.transcript[0].starts_with(THOUGHT_ENTRY_PREFIX));
        assert!(app.transcript[0].contains(&"think ".repeat(100)));
        assert_eq!(
            app.transient_row().as_deref(),
            Some("WillDeep · replying: Hello")
        );
        app.thought_hit_rows = vec![(7, 0)];
        assert!(app.toggle_thought_at(7));
        assert!(app.transcript[0].starts_with(THOUGHT_EXPANDED_PREFIX));
    }

    #[test]
    fn version_command_reports_cli_and_runtime_and_flags_a_mismatch() {
        let mut app = App::new(Vec::new(), Language::En);
        assert!(app.handle_slash_command("/version", &SkillCatalog::default()));
        let line = app.transcript.last().cloned().unwrap();
        assert!(line.contains(&format!("CLI {}", willdeep_core::VERSION)));
        assert!(line.contains("Runtime not connected"));

        app.runtime_version = Some(willdeep_core::VERSION.to_owned());
        assert!(!app.version_report().contains("versions differ"));

        app.runtime_version = Some("0.1.0".to_owned());
        let report = app.version_report();
        assert!(report.contains("Runtime 0.1.0"), "{report}");
        assert!(report.contains("versions differ"), "{report}");
    }

    #[test]
    fn undelivered_steering_is_requeued_with_a_notice() {
        let mut app = App::new(Vec::new(), Language::En);
        app.requeue_steering("先别删".to_owned());
        assert_eq!(app.queued_prompts.len(), 1);
        assert_eq!(app.queued_prompts[0].text, "先别删");
        assert!(app.transcript[0].starts_with("System: The last message missed this turn"));
    }

    #[test]
    fn steering_during_a_streaming_reply_lands_after_that_reply() {
        let mut app = App::new(Vec::new(), Language::En);
        app.begin_turn(false, "working".to_owned());
        app.note_model_step_started();
        app.stream_transient(StreamKind::Reply, "listing");
        app.note_steering_delivered("clean it up", message_time::now());
        assert!(app.transcript.iter().all(|row| !row.starts_with("You:")));

        app.response_style = Some(willdeep_core::ResponseStyle::Detailed);
        app.note_narration("listing done");
        app.note_model_step_started();
        let reply = app
            .transcript
            .iter()
            .position(|row| row == "WillDeep: listing done");
        let steer = app
            .transcript
            .iter()
            .position(|row| row == "You: clean it up");
        assert!(
            reply.is_some() && steer.is_some() && reply < steer,
            "{:?}",
            app.transcript
        );

        // 工具在跑时也要等模型读取，不能把“已送达”当成“已处理”。
        app.note_tool_requested("run_command", None);
        app.note_steering_delivered("stop", message_time::now());
        assert!(app.queue_activity().unwrap().contains("Delivered"));
        app.note_model_step_started();
        assert_eq!(app.transcript.last().map(String::as_str), Some("You: stop"));
    }

    #[test]
    fn deferred_steering_is_shown_at_turn_end_or_as_queued_when_it_missed() {
        let mut app = App::new(Vec::new(), Language::En);
        app.begin_turn(false, "working".to_owned());
        app.note_model_step_started();
        app.note_steering_delivered("missed", message_time::now());
        app.note_steering_delivered("consumed", message_time::now());
        app.requeue_steering("missed".to_owned());
        assert_eq!(
            app.transcript.last().map(String::as_str),
            Some("You: [queued] missed")
        );
        app.finish_turn();
        assert_eq!(
            app.transcript.last().map(String::as_str),
            Some("You: consumed")
        );
        assert!(app.deferred_steering.is_empty());
    }

    #[test]
    fn queued_prompt_is_echoed_after_the_previous_turn_before_the_next_reply() {
        for language in [Language::ZhCn, Language::En, Language::Ja] {
            let mut app = App::new(Vec::new(), language);
            app.begin_turn(false, "working".to_owned());
            app.input.insert("fix it");
            app.queue_submitted_prompt();
            app.note_reply("previous reply");
            app.append_turn_stats(None);
            app.finish_turn();
            let divider = app.transcript.last().unwrap().clone();
            app.queued_prompts.pop_front().unwrap();

            app.mark_queued_prompt_sent("fix it");
            app.note_reply("next reply");

            assert_eq!(
                app.transcript,
                vec![
                    "WillDeep: previous reply".to_owned(),
                    divider,
                    "You: fix it".to_owned(),
                    "WillDeep: next reply".to_owned(),
                ]
            );
        }
    }

    #[test]
    fn identical_queued_prompts_keep_their_own_delivery_positions() {
        let mut app = App::new(Vec::new(), Language::En);
        let queued = app.queued_prompt_row("again");
        app.append_transcript(queued.clone());
        app.note_reply("between submissions");
        app.append_transcript(queued.clone());
        app.note_reply("previous reply");

        app.mark_queued_prompt_sent("again");
        assert_eq!(app.transcript[0], "WillDeep: between submissions");
        assert_eq!(app.transcript[1], queued);
        assert_eq!(app.transcript.last().unwrap(), "You: again");
        app.note_reply("first queued reply");
        app.mark_queued_prompt_sent("again");
        app.note_reply("second queued reply");
        assert_eq!(
            &app.transcript[2..],
            [
                "You: again",
                "WillDeep: first queued reply",
                "You: again",
                "WillDeep: second queued reply",
            ]
        );
    }

    #[test]
    fn queued_prompt_without_a_placeholder_is_still_echoed_safely() {
        let mut app = App::new(Vec::new(), Language::En);
        app.note_reply("previous reply");
        app.mark_queued_prompt_sent("first\tline\nsecond line");
        assert_eq!(
            app.transcript.last().unwrap(),
            &terminal_safe_text("You: first\tline\nsecond line")
        );
    }

    #[test]
    fn turn_end_leaves_a_divider_rings_the_bell_and_flips_the_composer_state() {
        let mut app = App::new(Vec::new(), Language::En);
        assert!(app.composer_state().0.starts_with("Your turn"));
        app.begin_turn(false, "working".to_owned());
        assert!(app.composer_state().0.contains("Enter sends text"));
        assert!(!app.bell_pending);

        app.note_tool_requested("read_file", None);
        app.append_turn_stats(None);
        let divider = app.transcript.last().cloned().unwrap();
        assert!(divider.starts_with(TURN_DIVIDER_PREFIX), "{divider}");
        assert!(divider.contains("turn finished"), "{divider}");
        assert!(
            !divider.contains("your turn"),
            "记录里不带「轮到你」，它只在画最后一行时补: {divider}"
        );

        app.finish_turn();
        assert!(std::mem::take(&mut app.bell_pending));
        assert!(app.composer_state().0.starts_with("Your turn"));
    }

    #[test]
    fn background_wait_explains_input_and_preserves_draft() {
        let mut app = App::new(Vec::new(), Language::En);
        app.begin_turn(true, "working".into());
        app.input.insert("unsent draft");
        app.note_delayed_command(240);
        assert!(app.transcript.last().unwrap().contains("240s"));
        app.set_background_wait(true);
        assert!(app.composer_state().0.contains("Keep typing"));
        assert!(app.working_summary().unwrap().contains("Enter sends text"));
        assert_eq!(app.input.text(), "unsent draft");
        app.set_background_wait(false);
        assert!(app.composer_state().0.contains("Enter sends text"));
    }

    #[test]
    fn unshown_reply_skips_what_narration_already_showed() {
        // 正常收尾：收尾文字就是最后一段，不再重复。
        assert_eq!(unshown_reply("All done.", Some("All done.")), None);
        assert_eq!(unshown_reply("  All done.\n", Some("All done.")), None);
        // 轮次上限：守护进程把提示拼在最后一段前面，只补提示。
        assert_eq!(
            unshown_reply("⚠ limit hit\n\nAll done.", Some("All done.")),
            Some("⚠ limit hit".to_owned())
        );
        // 没有中途文字，或收尾文字另起一段：整段照常显示。
        assert_eq!(
            unshown_reply("Fresh answer", None),
            Some("Fresh answer".to_owned())
        );
        assert_eq!(
            unshown_reply("Different closing", Some("All done.")),
            Some("Different closing".to_owned())
        );
        assert_eq!(unshown_reply("   ", Some("All done.")), None);
    }

    #[test]
    fn tool_rows_settle_in_place_and_fall_back_to_a_new_row() {
        let mut app = App::new(Vec::new(), Language::En);
        app.note_tool_requested("run_command", Some("cargo test -p"));
        app.note_tool_requested("read_file", None);
        assert_eq!(
            app.transcript,
            vec!["· … run_command · cargo test -p", "· … read_file"]
        );

        // 完成事件不带摘要也要对上发起行；同名多行时改最近那条。
        app.note_tool_completed("run_command", None, true);
        app.note_tool_completed("read_file", None, false);
        assert_eq!(
            app.transcript,
            vec!["· ✗ run_command · cargo test -p", "· ✓ read_file"]
        );

        // 找不到发起行（重开会话后才收到完成事件）就补一行。
        app.note_tool_completed("edit_file", Some("main.rs"), false);
        assert_eq!(
            app.transcript.last().map(String::as_str),
            Some("· ✓ edit_file · main.rs")
        );
    }

    #[test]
    fn narration_then_reply_does_not_duplicate_the_last_paragraph() {
        // 详细模式：中途文字逐条落进记录，收尾只补没见过的部分。
        let mut app = App::new(Vec::new(), Language::En);
        app.response_style = Some(willdeep_core::ResponseStyle::Detailed);
        app.transient_thought = Some("streaming…".to_owned());
        app.note_narration("  Now update the log line  ");
        assert!(app.transient_thought.is_none());
        assert_eq!(app.transcript, vec!["WillDeep: Now update the log line"]);

        assert_eq!(app.note_reply("Now update the log line"), None);
        assert_eq!(app.transcript.len(), 1);

        assert_eq!(
            app.note_reply("⚠ limit\n\nNow update the log line"),
            Some("⚠ limit".to_owned())
        );
        assert_eq!(
            app.transcript,
            vec!["WillDeep: Now update the log line", "WillDeep: ⚠ limit"]
        );
    }

    #[test]
    fn concise_narration_stays_on_one_status_row_until_the_turn_ends() {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.begin_turn(true, "working".to_owned());
        app.note_narration("先看日志");
        app.note_narration("再改这一行");
        assert!(
            app.transcript.is_empty(),
            "concise mode keeps mid-turn prose out of the transcript: {:?}",
            app.transcript
        );
        let row = app.live_transcript_row().unwrap();
        assert!(row.contains("再改这一行"), "{row}");
        assert!(!row.contains("先看日志"), "only the latest line shows: {row}");
        assert!(row.contains('s'), "the status row carries the elapsed time: {row}");

        // 流式增量压过状态行；定稿后又回到状态行。
        app.stream_transient(StreamKind::Reply, "正在写");
        assert!(app.live_transcript_row().unwrap().contains("正在写"));
        app.note_narration("写完了");
        assert!(app.live_transcript_row().unwrap().contains("写完了"));

        // 计划围栏照旧进记录，原位更新卡片；它就是最新一段，状态行让位给卡片。
        app.note_narration("```plan\n1. 验证接口\n2. 发布\n```");
        assert_eq!(app.transcript.len(), 1);
        assert!(decode_plan_entry(&app.transcript[0]).is_some());
        assert!(app.live_transcript_row().is_none());

        // 收尾与最后一段相同：落成一段正文，不再重复，状态行随之撤掉。
        assert_eq!(app.note_reply("写完了"), Some("写完了".to_owned()));
        assert_eq!(app.transcript.len(), 2);
        assert!(app.transcript[1].contains("写完了"));
        app.finish_turn();
        assert!(app.live_transcript_row().is_none());

        // 收尾在最后一段前面拼了提示：两段都落，镜像拿到完整文字。
        app.begin_turn(true, "working".to_owned());
        app.note_narration("写完了");
        assert_eq!(
            app.note_reply("⚠ 轮次上限\n\n写完了"),
            Some("⚠ 轮次上限\n\n写完了".to_owned())
        );
        assert!(app.transcript.iter().any(|row| row.contains("轮次上限")));
        assert_eq!(
            app.transcript
                .iter()
                .filter(|row| row.contains("写完了"))
                .count(),
            2
        );
    }

    #[test]
    fn an_interrupted_concise_turn_keeps_its_last_line() {
        let mut app = App::new(Vec::new(), Language::En);
        app.begin_turn(false, "working".to_owned());
        app.note_narration("halfway through");
        assert!(app.transcript.is_empty());
        app.finish_turn();
        assert_eq!(app.transcript, vec!["WillDeep: halfway through"]);
        // 补写只发生一次。
        app.finish_turn();
        assert_eq!(app.transcript.len(), 1);
    }

    #[test]
    fn goal_status_marker_never_reaches_the_transcript() {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.response_style = Some(willdeep_core::ResponseStyle::Detailed);
        app.note_narration("<goal-status>complete</goal-status> 训练闭环。");
        assert_eq!(app.transcript, vec!["WillDeep: 训练闭环。"]);
        // 收尾文字与中途文字走同一条投影，不会因为标签而重复显示。
        assert_eq!(
            app.note_reply("<goal-status>complete</goal-status> 训练闭环。"),
            None
        );
        assert_eq!(app.transcript.len(), 1);
        // 纯标签不占一行。
        app.note_narration("<goal-status>complete</goal-status>");
        assert_eq!(app.transcript.len(), 1);
    }

    #[test]
    fn long_transcripts_archive_older_rounds_and_reveal_them_on_scroll() {
        fn push_rounds(app: &mut App, count: usize) {
            for round in 0..count {
                app.transcript.push(format!("You: round {round}"));
                app.transcript.push(format!("WillDeep: done {round}"));
                app.transcript
                    .push(format!("{TURN_DIVIDER_PREFIX}turn {round} ──"));
            }
        }

        let mut app = App::new(Vec::new(), Language::En);
        push_rounds(&mut app, 60);
        app.archive_old_rounds();
        assert_eq!(app.archive_floor, 0, "没到上限不归档");

        push_rounds(&mut app, 60);
        app.archive_old_rounds();
        assert!(app.archive_floor > 0, "120 轮要收拢渲染范围");
        let folded = app.display_transcript();
        assert!(
            folded.rows[0].contains("earlier turns archived"),
            "{:?}",
            folded.rows[0]
        );
        assert!(folded.rows.len() < app.transcript.len());

        app.transcript_height = 1_000;
        app.viewport_height = 10;
        let archived = app.archive_floor;
        assert!(app.reveal_archived_rounds());
        assert!(app.archive_floor < archived, "翻到顶要放回一批");
        while app.reveal_archived_rounds() {}
        assert_eq!(app.archive_floor, 0);
        assert!(!app.display_transcript().rows[0].contains("archived"));
    }

    #[test]
    fn goal_completion_gets_a_system_line() {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.note_goal_completed();
        assert_eq!(app.transcript, vec!["System: ✓ 目标完成"]);
        app.transcript.clear();
        app.turn_started = Some(std::time::Instant::now());
        app.note_goal_completed();
        assert!(
            app.transcript[0].starts_with("System: ✓ 目标完成 · 用时 "),
            "{}",
            app.transcript[0]
        );
    }
}
