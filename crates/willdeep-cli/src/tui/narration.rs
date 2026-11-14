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
//! - 显示层只保留最近一段连续工具行，收成一行汇总，不列出具体工具，`/tools` 展开。
//!   更早的段完全不显示。夹在中间、当前可见的思考行不切段，一段连续的只留最后一条。
//!
//! 进程内轮次与 Runtime 轮次共用这一套，行为一致。
use super::*;

/// 工具行状态标记：进行中 / 成功 / 失败。
const TOOL_PENDING: &str = "…";
const TOOL_DONE: &str = "✓";
const TOOL_FAILED: &str = "✗";

/// 只有最近那段连续工具行会显示，而且只占一行汇总。更早的段完全不显示，
/// 记录本身不动，`/tools` 展开后照常逐条可见。
/// 汇总行的标记，与三种工具行标记都不同；渲染层照样按 `· ` 账目行的灰色画。
const TOOL_FOLD_MARKER: &str = "⋯";
/// 整段不显示的工具行的临时编号。比任何一段的行数都大，输出循环对不上它，
/// 定稿后再改成「本段之后的第一行」。
const HIDDEN_RUN_ROW: usize = usize::MAX;
const THOUGHT_ENTRY_PREFIX: &str = "· + Thought: ";
const THOUGHT_EXPANDED_PREFIX: &str = "· − Thought: ";

/// 点开的工具行下面那一行「收起」。三种语言各一句。
const TOOL_COLLAPSE_LABELS: [&str; 3] = ["− 收起", "− Show less", "− 折りたたむ"];

/// 这句是只改状态、不送去模型的斜杠命令。这类命令不回显成 `You:` 行：
/// `/model`、`/help` 这类操作留在聊天区里只是噪声，结果由命令自己写一行说明。
/// `/goal`、`/local`、`/runtime` 会开工，那句是用户的任务，照常显示。
pub(super) fn is_display_only_command(prompt: &str) -> bool {
    let value = prompt.trim();
    let command = value.split_whitespace().next().unwrap_or(value);
    command.starts_with('/') && command != "/goal" && command != "/local" && command != "/runtime"
}

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

/// 系统回执行（`System: ` 开头：模型已切换、工具行已展开、开屏提示这些）只在刚发生
/// 时有用。它后面又落了这么多条用户/助手消息之后就不再画：记录本身不动，只是
/// 不占屏幕。三条是「下一次问答里还看得见、再下一句发出就收」：用户问一句、模型答一句还在，
/// 用户再开口才收——正好是聊天区本来就要滚动的时刻，不会在用户看着的时候消失。
pub(super) const SYSTEM_NOTICE_FADE_MESSAGES: usize = 3;

/// 这行是不是系统回执。错误行（`Error: `）不算：失败要一直看得见。
fn is_system_notice_row(line: &str) -> bool {
    line.starts_with("System: ")
}

/// 可见区里哪些系统回执已经过时：它后面落了至少 `SYSTEM_NOTICE_FADE_MESSAGES` 条消息。
/// 从后往前数一遍就行，不用每行再往后扫。
fn faded_system_rows(rows: &[String]) -> Vec<bool> {
    let mut faded = vec![false; rows.len()];
    let mut messages_after = 0;
    for (index, row) in rows.iter().enumerate().rev() {
        if counts_as_fade_message(row) {
            messages_after += 1;
        } else if is_system_notice_row(row) {
            faded[index] = messages_after >= SYSTEM_NOTICE_FADE_MESSAGES;
        }
    }
    faded
}

/// 淡出计数只认已经落地的对话轮次：用户消息（真正发出去的）、助手回复，以及
/// 代替回复出现在记录里的计划卡。还挂在输入队列上的占位行不算——消息根本没发
/// 出去，不该把回执提前收走。
fn counts_as_fade_message(row: &str) -> bool {
    if row.starts_with(super::plan_ui::PLAN_PREFIX) || row.starts_with("WillDeep: ") {
        return true;
    }
    let Some(rest) = row.strip_prefix("You: ") else {
        return false;
    };
    !UNSENT_ROW_PREFIXES
        .iter()
        .any(|prefix| rest.starts_with(prefix))
}

/// 还没发出去的用户行前缀（三种界面语言）。`[正在提交]` 是网络往返期间的占位，
/// `［待发］` 是排队等本轮结束的插话。
const UNSENT_ROW_PREFIXES: &[&str] = &[
    "[正在提交]",
    "[submitting]",
    "[送信中]",
    "［待发］",
    "[queued]",
    "［送信待ち］",
];

/// 还没出结果的切换过程行，按事项分开记：换模型和调思考等级可以同时在等本轮结束，
/// 各自的结果只替自己的过程行。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum PendingNotice {
    ModelSwitch,
    ReasoningLevel,
}

impl App {
    /// 屏幕上还有没有一条看得见的记录。
    ///
    /// 判断首屏要不要补欢迎语只能看这个，不能看 `transcript.is_empty()`：会话里
    /// 可能只剩空白行，或者全是关掉的思考行，列表非空但整屏空白，用户会以为
    /// 程序没起来。
    pub(super) fn has_visible_transcript(&self) -> bool {
        self.transcript
            .iter()
            .any(|row| !row.trim().is_empty() && (self.ui.show_thinking || !is_thought_row(row)))
    }

    /// 屏幕上一条可见记录都没有时补上首屏欢迎语，顺带丢掉那些空白行——
    /// 留着它们只会把真正的内容往下挤。多个入口（启动、换会话、`/clear`）
    /// 共用这一条规则，别各自写一份 `is_empty()` 判断。
    /// 回复行在屏幕上显示的名字。没起名时用默认名。
    pub(super) fn assistant_name(&self) -> String {
        if self.assistant_name.is_empty() {
            ASSISTANT_LABEL.to_owned()
        } else {
            self.assistant_name.clone()
        }
    }

    /// 从家目录读实例名。读不到（没家目录、文件坏了）就保持现状，不拿默认名
    /// 把已经起好的名字盖掉。
    pub(super) fn load_assistant_name(&mut self) {
        let Some(home) = self.display_home.as_deref() else {
            return;
        };
        if let Ok(identity) = crate::identity::load(home) {
            self.assistant_name = identity.display_name(false);
        }
    }

    pub(super) fn ensure_welcome(&mut self, workspace: &std::path::Path, language: Language) {
        if self.has_visible_transcript() {
            return;
        }
        // 空白行连同它的附件槽和模型章一起丢掉，三边始终逐行对齐。
        let slots = std::mem::take(&mut self.transcript_attachments);
        let models = std::mem::take(&mut self.transcript_models);
        let kept = self
            .transcript
            .drain(..)
            .zip(slots.into_iter().chain(std::iter::repeat(Vec::new())))
            .zip(models.into_iter().chain(std::iter::repeat(None)))
            .filter(|((row, _), _)| !row.trim().is_empty())
            .collect::<Vec<_>>();
        self.transcript = kept.iter().map(|((row, _), _)| row.clone()).collect();
        self.transcript_attachments = kept.iter().map(|((_, slot), _)| slot.clone()).collect();
        self.transcript_models = kept.into_iter().map(|(_, model)| model).collect();
        self.transcript
            .push(super::rendering::welcome_message(workspace, language));
        self.transcript_attachments.push(Vec::new());
        self.transcript_models.push(None);
    }

    /// 从会话重建聊天记录。所有「换一份记录」的路径都必须走这里：
    /// 只投影消息会漏掉首屏，daemon 的 display session 那条路径就是这样把
    /// 欢迎语又覆盖掉的——列表非空、屏幕空白。
    pub(super) fn load_transcript(&mut self, session: &willdeep_core::Session) {
        let (rows, attachments, models) = super::plan_ui::project_session(
            &session.messages,
            session.current_plan.as_ref(),
            self.language,
        );
        self.transcript = rows;
        self.transcript_attachments = attachments;
        self.transcript_models = models;
        self.ensure_welcome(&session.workspace, self.language);
    }

    /// 删掉记录里的一行，连同它的附件槽、模型章和时间一起，保持逐行对齐。
    pub(super) fn drop_transcript_row(&mut self, index: usize) {
        for queued in &mut self.queued_prompts {
            if let Some(row) = queued.transcript_row.as_mut() {
                if *row == index {
                    queued.transcript_row = None;
                } else if *row > index {
                    *row -= 1;
                }
            }
        }
        self.transcript.remove(index);
        if index < self.transcript_attachments.len() {
            self.transcript_attachments.remove(index);
        }
        if index < self.transcript_models.len() {
            self.transcript_models.remove(index);
        }
        self.message_times.remove(index);
        // 展开状态按记录位置记，删了一行后面的都得前移一格，不然点开的那条会
        // 落到邻居身上（用户消息的展开还比对行文，工具行和已点开的工具行只认位置）。
        self.shift_row_keyed_state_after_removal(index);
        self.refresh_transcript_height();
    }

    fn shift_row_keyed_state_after_removal(&mut self, removed: usize) {
        fn shift_map<V>(map: &mut BTreeMap<usize, V>, removed: usize) {
            let tail = map.split_off(&removed);
            for (key, value) in tail {
                if key != removed {
                    map.insert(key - 1, value);
                }
            }
        }
        shift_map(&mut self.expanded_user_messages, removed);
        shift_map(&mut self.tool_expansions, removed);
        let tail = self.expanded_tool_rows.split_off(&removed);
        self.expanded_tool_rows.extend(
            tail.into_iter()
                .filter(|key| *key != removed)
                .map(|key| key - 1),
        );
        if self.archive_floor > removed {
            self.archive_floor -= 1;
        }
        // 搜索命中也按位置记；不跟着前移的话，删行后按 Enter 会跳到邻居身上。
        if let Some(search) = &mut self.search {
            let selected_row = search.matches.get(search.selected).copied();
            search.matches.retain(|index| *index != removed);
            for index in &mut search.matches {
                if *index > removed {
                    *index -= 1;
                }
            }
            search.selected = match selected_row {
                Some(row) if row == removed => {
                    search.selected.min(search.matches.len().saturating_sub(1))
                }
                Some(row) => {
                    let row = if row > removed { row - 1 } else { row };
                    search
                        .matches
                        .iter()
                        .position(|index| *index == row)
                        .unwrap_or(0)
                }
                None => 0,
            };
        }
    }

    /// 一次切换的过程行（「正在切换」「本轮结束后切换到」）：落进记录并记住原文，
    /// 等结果回来替掉。同一件事只留一句话：「正在」不该和「已」并排占两行；
    /// 同一轮里再来一次，上一次的过程行也让位。
    pub(super) fn note_switch_in_progress(&mut self, kind: PendingNotice, row: String) {
        self.settle_pending_notice(kind);
        self.append_transcript(row.clone());
        self.pending_notice_rows
            .insert(kind, terminal_safe_text(&row));
    }

    /// 一次切换的结果：换掉还在记录里的过程行，然后落结果。过程行找不到
    /// （换过会话、`/clear` 清过）就只落结果，不动别的行。
    pub(super) fn note_switch_settled(&mut self, kind: PendingNotice, row: String) {
        self.settle_pending_notice(kind);
        self.append_transcript(row);
    }

    fn settle_pending_notice(&mut self, kind: PendingNotice) {
        let Some(previous) = self.pending_notice_rows.remove(&kind) else {
            return;
        };
        if let Some(index) = self.transcript.iter().rposition(|row| *row == previous) {
            self.drop_transcript_row(index);
            self.transcript_render_cache = None;
        }
    }

    /// 把附件挂到最近一条用户消息上。发送时记录行已经写好（`You: 原文`），
    /// 附件在这之后才对得上是哪一行。
    pub(super) fn attach_to_latest_user_row(&mut self, attachments: Vec<MessageAttachment>) {
        if attachments.is_empty() {
            return;
        }
        if let Some(index) = self
            .transcript
            .iter()
            .rposition(|row| row.starts_with("You: "))
        {
            self.transcript_attachments
                .resize_with(self.transcript.len(), Vec::new);
            self.transcript_attachments[index] = attachments;
            // 附件不在记录文本里，渲染缓存的键感知不到，得主动作废。
            self.transcript_render_cache = None;
        }
    }
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

/// 点开的工具行下面那一行「收起」，按界面语言挑一句。
pub(super) fn tool_collapse_label(language: Language) -> &'static str {
    match language {
        Language::ZhCn => TOOL_COLLAPSE_LABELS[0],
        Language::En => TOOL_COLLAPSE_LABELS[1],
        Language::Ja => TOOL_COLLAPSE_LABELS[2],
    }
}

/// 汇总行不再点名具体工具，这个解析只留给测试核对工具行的格式。
#[cfg(test)]
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
        self.transient_row().or_else(|| self.concise_status_row())
    }

    /// 压缩开始：状态行挂到用户刚发的那句下面，读秒从这一刻起。
    ///
    /// 压缩发生在「轮次开始」之后、正文之前，期间聊天区一行都不长，用户会以为
    /// 卡住了。活动区那行照旧记（`record_progress`），这里只管聊天区这一行。
    /// 这一行说的比等待动画具体，所以动画让位，不两行叠在一起。
    pub(super) fn note_compression_started(&mut self) {
        self.dismiss_awaiting_reply();
        self.compression = Some(CompressionNotice {
            started: Instant::now(),
            done: false,
        });
    }

    /// 压缩完成：同一行整行换成「上下文已压缩」，不再读秒。正文或工具一来就撤掉。
    pub(super) fn note_compression_completed(&mut self) {
        if let Some(notice) = self.compression.as_mut() {
            notice.done = true;
        }
    }

    /// 压缩请求正在飞。压完但提示还挂在屏幕上不算：那时输入照常处理。
    ///
    /// 这段时间模型在重写上下文，插话进不了正在飞的那次请求，本地命令也改不动
    /// 正在压缩的会话。输入先排队，等压缩结束再按普通队列发出去。
    pub(super) fn compression_in_progress(&self) -> bool {
        self.compression.as_ref().is_some_and(|notice| !notice.done)
    }

    /// 把一条提示按住一段时间。状态行每帧都会把普通提示取走，不按住的话
    /// 「为什么没发出去」会在下一帧消失。
    pub(super) fn hold_notice(&mut self, notice: String, duration: Duration) {
        self.notice_hold = Some((notice.clone(), Instant::now() + duration));
        self.notice = Some(notice);
    }

    /// 真正的输出开始了（正文、思维链、工具、计划），压缩那一行的使命结束。
    pub(super) fn dismiss_compression_notice(&mut self) {
        self.compression = None;
    }

    /// 用户消息已经回显、模型还一个字都没回来：记下这一刻。
    ///
    /// 从按下回车到 Runtime 回第一份事件之间，聊天区除了刚发出的那句什么都没有，
    /// 看起来像卡住了。这只记起点，画面由 [`Self::awaiting_reply_row`] 每帧现画。
    pub(super) fn note_awaiting_reply(&mut self) {
        if self.awaiting_since.is_none() {
            self.awaiting_since = Some(Instant::now());
        }
    }

    /// 模型有了第一个可见动作（思考行、正文、工具、计划），等待动画到此为止。
    pub(super) fn dismiss_awaiting_reply(&mut self) {
        self.awaiting_since = None;
    }

    /// 挂在最后一条用户消息正下方的等待行：`.`、`..`、`...` 每秒轮换。
    ///
    /// 只在「最后一条是用户消息、模型还没开口」时画。思考行一落进记录，或流式
    /// 预览已经在走，这行就不再出现。
    pub(super) fn awaiting_reply_row(&self) -> Option<String> {
        let started = self.awaiting_since?;
        if !self
            .transcript
            .last()
            .is_some_and(|row| row.starts_with("You: "))
        {
            return None;
        }
        let dots = started.elapsed().as_secs() % 3 + 1;
        Some(".".repeat(dots as usize))
    }

    /// 挂在最后一条用户消息正下方的压缩行。压缩中带已耗时，压完只留一句。
    /// 聊天区最后一条不是用户消息时不画：这行只回答「我刚发的那句怎么没反应」。
    pub(super) fn compression_notice_row(&self) -> Option<String> {
        let notice = self.compression.as_ref()?;
        if !self
            .transcript
            .last()
            .is_some_and(|row| row.starts_with("You: "))
        {
            return None;
        }
        Some(if notice.done {
            format!(
                "· {}",
                self.language.text(
                    "上下文已压缩",
                    "Context compressed",
                    "コンテキストを圧縮しました"
                )
            )
        } else {
            format!(
                "· {} · {}",
                self.language.text(
                    "正在压缩上下文",
                    "Compressing context",
                    "コンテキストを圧縮中"
                ),
                format_elapsed_span(notice.started.elapsed().as_secs_f32(), 0)
            )
        })
    }

    /// 聊天区底部的临时行：思维链只显示耗时摘要；正文显示最近一段回复。
    pub(super) fn transient_row(&self) -> Option<String> {
        let thought = self.transient_thought.as_deref()?;
        let label = self.transient_label();
        Some(match self.transient_kind {
            StreamKind::Reasoning => format!("· + Thought: {}", self.thought_elapsed_label()),
            StreamKind::Reply => format!("{} · {label}: {thought}", self.assistant_name()),
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
            one_line.push(if character.is_whitespace() {
                ' '
            } else {
                character
            });
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
        // 流式输出一开始，压缩状态行和等待动画都让位：用户已经看到 AI 在动了。
        self.dismiss_compression_notice();
        self.dismiss_awaiting_reply();
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

    /// 点工具行展开，点它下面的「收起」收回。
    ///
    /// 展开只记在显示层：记录里那行原样不动。完整命令在发起时就记在
    /// `tool_expansions` 里，点开直接取；没有记下的（比如重开会话回放出来的）
    /// 点开只多一行「收起」。点在别的地方返回 `false`，让点击继续走原来的处理。
    pub(super) fn toggle_tool_row_at(&mut self, y: u16) -> bool {
        // 同一个屏幕行上「收起」优先于工具行本身：窄屏折行时两段会挤到一行，
        // 点下去应该收回而不是再展开一次。
        let Some((_, source, collapse)) = self
            .tool_row_hits
            .iter()
            .filter(|(row, _, _)| *row == y)
            .max_by_key(|(_, _, collapse)| *collapse)
        else {
            return false;
        };
        let source = *source;
        // 点「收起」，或者点一行已经展开的工具行，都收回；点收着的工具行才展开。
        // 上面已经按「收起优先」挑过命中，这里不用再分两种收回。
        if *collapse || self.expanded_tool_rows.contains(&source) {
            self.expanded_tool_rows.remove(&source);
        } else {
            self.expanded_tool_rows.insert(source);
        }
        self.transcript_render_cache = None;
        self.tool_row_hits.clear();
        self.refresh_transcript_height();
        self.scroll_from_bottom = self.scroll_from_bottom.min(self.max_scroll());
        true
    }

    /// 记下这一行工具调用点开后要显示的完整内容。按记录位置记，记录本身不改。
    fn remember_tool_expansion(&mut self, expansion: Option<String>) {
        let Some(expansion) = expansion.filter(|text| !text.trim().is_empty()) else {
            return;
        };
        let index = self.transcript.len().saturating_sub(1);
        self.tool_expansions.insert(index, expansion);
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
        } else if self.compression_in_progress() {
            (
                self.language.text(
                    "正在压缩上下文 · 回车排队，压缩结束后发送 · 打字不受影响",
                    "Compressing context · Enter queues until it finishes · typing still works",
                    "コンテキスト圧縮中 · Enter でキューに追加、完了後に送信 · 入力はそのまま",
                ),
                Color::Yellow,
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
                    "轮到你 · Enter 发送 · Cmd/Ctrl+Enter 入队",
                    "Your turn · Enter sends · Cmd/Ctrl+Enter queues for later",
                    "あなたの番 · Enter で送信 · Cmd/Ctrl+Enter でキューに追加",
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
        // A was submitted before B even if B's queued echo was drawn before A's inbox
        // was drained. Keep the original submission order without moving either echo.
        // The instruction inbox trims text before storing it. Match on that same
        // normalized value, then restore the original user input for the queue.
        let delivered = self
            .deferred_steering
            .iter()
            .position(|value| value.text.trim() == text.trim())
            .map(|index| self.deferred_steering.remove(index));
        let text = delivered
            .as_ref()
            .map(|item| item.text.clone())
            .unwrap_or(text);
        let submission_order = delivered
            .as_ref()
            .map(|item| item.submission_order)
            .unwrap_or_else(|| self.next_submission_order());
        let sent_at = delivered
            .map(|item| item.sent_at)
            .unwrap_or_else(message_time::now);
        let row = self.queued_prompt_row(&text);
        self.append_transcript_at(row, sent_at);
        let transcript_row = Some(self.transcript.len() - 1);
        let insert_at = self
            .queued_prompts
            .iter()
            .position(|item| item.submission_order > submission_order)
            .unwrap_or(self.queued_prompts.len());
        self.queued_prompts.insert(
            insert_at,
            QueuedPrompt {
                remote: None,
                text,
                attachments: Vec::new(),
                submission_order,
                transcript_row,
            },
        );
    }

    /// 测试里模拟直接送达；实际队列沿用提交时分配的顺序号。
    #[cfg(test)]
    pub(super) fn note_steering_delivered(&mut self, text: &str, sent_at: u64) {
        let order = self.next_submission_order();
        self.note_steering_delivered_with_order(text, sent_at, order);
    }

    /// 送达只代表收件箱接收成功；到模型下一步读取前持续显示待阅状态。
    pub(super) fn note_steering_delivered_with_order(
        &mut self,
        text: &str,
        sent_at: u64,
        submission_order: u64,
    ) {
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
            submission_order,
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

    /// 忙时提交一成功就回显，不等下一轮任务启动才发的那条事件。
    /// 行文与事件到来时拼的那行一致，事件认领时直接对上，不会再写一行。
    pub(super) fn echo_runtime_submission(&mut self, text: &str) {
        let visible = willdeep_core::conversation::visible_user_text(text);
        let row = format!("You: {}", terminal_safe_text(visible));
        self.append_transcript(row.clone());
        // 忙时回显的这句同样是「已发出、还没人接」：本轮结束后它才被处理。
        self.note_awaiting_reply();
        self.pending_runtime_echoes
            .push_back(runtime_queue::PendingRuntimeEcho {
                text: visible.to_owned(),
                row,
            });
        self.note_prompt_submitted(visible);
    }

    /// 向上箭头在会话文件落盘前也要能找回刚发出的这句。同一句只留最近一次。
    pub(super) fn note_prompt_submitted(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.prompt_history_pending.retain(|entry| entry != text);
        self.prompt_history_pending.push_back(text.to_owned());
        let overflow = self
            .prompt_history_pending
            .len()
            .saturating_sub(prompt_history::HISTORY_LIMIT);
        self.prompt_history_pending.drain(..overflow);
    }

    /// 会话文件已经记下这些输入，向上箭头直接从消息里读，不再另记一份。
    pub(super) fn note_prompt_history_persisted(&mut self, messages: &[Message]) {
        self.prompt_history_pending.retain(|text| {
            !messages.iter().any(|message| {
                message.role == willdeep_core::types::Role::User
                    && willdeep_core::conversation::visible_user_text(&message.content) == text
            })
        });
    }

    /// 下一轮任务启动时的 `queued_user_message` 认领已经回显的那一行。
    /// 对上了就不再写；对不上（别的端发来的、或本端没来得及回显）照旧补一行。
    pub(super) fn claim_runtime_echo(&mut self, text: &str) -> bool {
        let visible = willdeep_core::conversation::visible_user_text(text);
        let row = format!("You: {}", terminal_safe_text(visible));
        let claimed = self
            .pending_runtime_echoes
            .iter()
            .position(|echo| echo.row == row)
            .or_else(|| {
                self.pending_runtime_echoes
                    .iter()
                    .position(|echo| echo.text == visible)
            });
        if let Some(position) = claimed {
            self.pending_runtime_echoes.remove(position);
            return true;
        }
        false
    }

    /// 队列里这条消息被删掉或改掉了：还挂着的回显行改成「未发送」。
    /// 已经认领过的（下一轮已经开始）不动，那句话确实发出去了。
    /// 同文多行时改最早那条，与入队顺序一致。
    pub(super) fn retract_runtime_echoes(&mut self, remaining: &[&str]) {
        let mut kept = VecDeque::new();
        for echo in std::mem::take(&mut self.pending_runtime_echoes) {
            if remaining.contains(&echo.text.as_str()) {
                kept.push_back(echo);
                continue;
            }
            if let Some(index) = self.transcript.iter().position(|row| *row == echo.row) {
                // 没发出去，附件也不该再挂在这行下面。
                if let Some(slot) = self.transcript_attachments.get_mut(index) {
                    slot.clear();
                }
                self.transcript[index] = format!(
                    "You: {}{}",
                    self.language
                        .text("［未发送］ ", "[not sent] ", "［未送信］ ",),
                    terminal_safe_text(&echo.text)
                );
            }
        }
        self.pending_runtime_echoes = kept;
    }

    /// 测试里模拟旧记录无行号的发送路径。
    #[cfg(test)]
    pub(super) fn mark_queued_prompt_sent(&mut self, text: &str) {
        self.mark_queued_prompt_sent_at(text, None);
    }

    /// 真正发出后移除这条消息自己的待发占位，并在当前轮次边界回显原文。
    /// 只改旧行会把用户消息留在上一轮中间，下一轮回复前就看不到提问。
    pub(super) fn mark_queued_prompt_sent_at(&mut self, text: &str, transcript_row: Option<usize>) {
        let queued = self.queued_prompt_row(text);
        let mut sent_at = message_time::now();
        let index = match transcript_row {
            Some(index) => (self.transcript.get(index) == Some(&queued)).then_some(index),
            None => self.transcript.iter().position(|line| *line == queued),
        };
        if let Some(index) = index {
            sent_at = self.message_times.time_at(index).unwrap_or(sent_at);
            self.drop_transcript_row(index);
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
/// 可见的思考行——思考行不切段。只有最近那段工具行会显示，收成一行汇总，
/// 不留任何具体工具行；更早的段连汇总都不显示。一段连续的思考行只留最后一条，
/// 更早的指向它（搜索跳转仍落得上）；用户展开过的那条原样留着。`expanded`
/// 只展开工具行（`/tools`），不展开思考。
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
        // 只显示最近一段：收成一行汇总，一条具体工具行都不留。更早的段连汇总
        // 都不显示。`/tools` 展开后逐条可见，只有一条时原样保留，不值得配汇总。
        let keep_tools = if expanded || tool_count == 1 {
            tool_count
        } else {
            0
        };
        let hidden_tools = tool_count - keep_tools;
        let show_summary = is_last && hidden_tools > 0;
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
        // 先定每行的去向，再统一编号。最近一段被收起的工具行共用一行汇总；更早的段
        // 整段不显示，那些行先记一个越界编号，定稿后再夹到本段之后的第一行。
        // 被收起的思考行指向本段留下的最后一条思考。
        let summary_at = rows.len();
        let mut placed = Vec::with_capacity(run.len());
        let mut emitted = usize::from(show_summary);
        for (offset, line) in run.iter().enumerate() {
            let hidden_tool = tool_row_marker(line).is_some()
                && run[..=offset]
                    .iter()
                    .filter(|row| tool_row_marker(row).is_some())
                    .count()
                    <= hidden_tools;
            if hidden_tool && show_summary {
                placed.push(summary_at);
            } else if hidden_tool {
                // 这段不显示。越界编号对不上任何输出位置，自然不画。
                placed.push(HIDDEN_RUN_ROW);
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
        if show_summary {
            rows.push(fold_summary(&hidden, language));
        }
        // 不显示的段没有自己的行，那些越界编号留到最后统一夹到最后一行：
        // 搜索跳转落在那里。这里不能夹——本段还没输出完，当时的行数恰好等于
        // 本段起点，夹完第一条会被当成可见行画出来。
        for (offset, line) in run.iter().enumerate() {
            if placed[offset] == rows.len() && !folded_thought(offset) {
                rows.push(line.clone());
            }
            index_of.push(placed[offset]);
        }
    }
    // 不显示的段没有行可指。搜索跳转落到它后面的第一条消息上：那段工具调用
    // 属于它前面那一轮，跳到下一句至少还在附近。后面什么都没有才落到最后一行。
    let tail = rows.len().saturating_sub(1);
    for index in 0..index_of.len() {
        if index_of[index] != HIDDEN_RUN_ROW {
            continue;
        }
        let after = index_of[index + 1..]
            .iter()
            .copied()
            .find(|target| *target != HIDDEN_RUN_ROW);
        index_of[index] = after.unwrap_or(tail);
    }
    FoldedTranscript { rows, index_of }
}

/// 折叠完成后把淡出的回执行从视图里删掉。它们折叠时仍占着位置、充当工具行的
/// 分段边界，所以只能最后删；删完把每条记录的落点重映射到它后面第一条还活着
/// 的输出行（没有就落到最后一行），搜索跳转仍然落得上。
fn remove_faded_rows(folded: &mut FoldedTranscript, faded: &[bool]) {
    let mut removed = vec![false; folded.rows.len()];
    for (entry, faded) in faded.iter().enumerate() {
        if *faded && let Some(&row) = folded.index_of.get(entry) {
            removed[row] = true;
        }
    }
    // 一条都不删（平时就是这样）时保持原样，连一次克隆都不做。
    if !removed.iter().any(|row| *row) {
        return;
    }
    let kept = removed.iter().filter(|row| !**row).count();
    if kept == 0 {
        return;
    }
    let mut placed = vec![usize::MAX; removed.len()];
    let mut next = 0;
    for (row, removed) in removed.iter().enumerate() {
        if !removed {
            placed[row] = next;
            next += 1;
        }
    }
    let mut after = None;
    for row in (0..removed.len()).rev() {
        if removed[row] {
            placed[row] = after.unwrap_or(next - 1);
        } else {
            after = Some(placed[row]);
        }
    }
    for target in &mut folded.index_of {
        *target = placed[*target];
    }
    folded.rows = folded
        .rows
        .iter()
        .zip(removed.iter())
        .filter(|(_, removed)| !**removed)
        .map(|(row, _)| row.clone())
        .collect();
}

/// `· ⋯ 已收起 12 条工具调用 · 2 失败 · /tools 展开`
/// 只报条数和失败数，不列具体工具；具体是哪些 `/tools` 展开后逐条看。
fn fold_summary(hidden: &[String], language: Language) -> String {
    let failed = hidden
        .iter()
        .filter(|line| tool_row_marker(line) == Some(TOOL_FAILED))
        .count();
    let mut summary = language
        .text(
            "已收起 {n} 条工具调用",
            "{n} tool calls folded",
            "ツール呼び出し {n} 件を折りたたみ",
        )
        .replace("{n}", &hidden.len().to_string());
    if failed > 0 {
        summary.push_str(
            &language
                .text(" · {k} 失败", " · {k} failed", " · {k} 失敗")
                .replace("{k}", &failed.to_string()),
        );
    }
    summary.push_str(language.text(" · /tools 展开", " · /tools to expand", " · /tools で展開"));
    format!("· {TOOL_FOLD_MARKER} {summary}")
}

impl App {
    /// 搜索框里正在用的词：去掉首尾空白，空的不算。渲染缓存的键和折叠视图都认这一份。
    pub(super) fn active_search_query(&self) -> Option<String> {
        self.search
            .as_ref()
            .map(|search| search.editor.text().trim().to_owned())
            .filter(|query| !query.is_empty())
    }

    fn visible_thought_start(&self) -> usize {
        self.transcript
            .iter()
            .rposition(|row| row.starts_with("You: "))
            .unwrap_or(0)
            .max(self.archive_floor)
    }

    /// 聊天区实际画出来的记录：工具行按规则收起，过时的系统回执不画，其余原样。
    pub(super) fn display_transcript(&self) -> FoldedTranscript {
        // 归档区不进渲染循环：只折叠还可见的那一段，顶上补一行说明。
        // 记录本身没动，`archive_floor` 只是渲染起点。
        let floor = self.archive_floor.min(self.transcript.len());
        let live = &self.transcript[floor..];
        let thought_start = self.visible_thought_start();
        // 系统回执过时与否按整份记录算，不按可见区：归档线在哪不该影响一条提示还显不显。
        // 正在搜索时全部放回来：搜索匹配的是整份记录，命中了却看不见那行就是坐进了黑屋。
        // 按「有没有搜索词」判，和渲染缓存的键一致：只开面板不打字，缓存不会重算。
        let faded = if self.active_search_query().is_some() {
            vec![false; self.transcript.len()]
        } else {
            faded_system_rows(&self.transcript)
        };
        // 淡出的回执先留在 entries 里：它虽然不画，但仍是工具行之间的分段边界。
        // 先删再折叠会让夹在两条工具行中间的回执把两段并成一段，而旧轮次里整段
        // 工具行都不显示——回执过期不该顺带藏掉别的记录。折叠完再删这些行。
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
        let mut entry_faded = visible_indices
            .iter()
            .map(|&index| faded[floor + index])
            .collect::<Vec<_>>();
        if floor > 0 {
            entries.insert(0, self.archive_placeholder());
            entry_faded.insert(0, false);
        }
        let mut folded = fold_tool_rows(&entries, self.tool_rows_expanded, self.language);
        remove_faded_rows(&mut folded, &entry_faded);
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
        // 正文到了，压缩那一行和等待动画都不用再解释「为什么还没动静」。
        self.dismiss_compression_notice();
        self.dismiss_awaiting_reply();
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
    ///
    /// `expansion` 是点开后显示的完整内容，与摘要行分开记：摘要要短、要打码，
    /// 点开是为了看清整条命令。
    pub(super) fn note_tool_requested(
        &mut self,
        name: &str,
        detail: Option<&str>,
        expansion: Option<String>,
    ) {
        self.finish_thought();
        self.transient_thought = None;
        // 工具调用是这一轮的第一份可见输出，压缩状态行和等待动画到此为止。
        self.dismiss_compression_notice();
        self.dismiss_awaiting_reply();
        self.append_transcript(tool_line(TOOL_PENDING, name, detail));
        self.remember_tool_expansion(expansion);
    }

    /// 工具结束：把最近那条同名的进行中行改成成功/失败。只按名字对行，完成事件
    /// 不带摘要也能对上；实在找不到（比如重开会话后才收到完成事件）就补一行，
    /// 宁可多一行也不丢结果。
    pub(super) fn note_tool_completed(
        &mut self,
        name: &str,
        detail: Option<&str>,
        expansion: Option<String>,
        is_error: bool,
    ) {
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
            // 对不上进行中的那行就补一行，完整内容跟着补上的这一行走。
            self.append_transcript(tool_line(marker, name, detail));
            self.remember_tool_expansion(expansion);
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
        if let Some(held) = held
            .as_deref()
            .map(str::trim)
            .filter(|held| !held.is_empty())
        {
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

    /// 把聊天区真正画到内存里的终端上，读出用户消息下面那一行。
    ///
    /// 走完整的 `draw`，不走只拼记录的那条路径：动画是显示层每帧现插的，
    /// 只有画出来的那一屏才能证明它的位置和内容。
    fn drawn_lines(app: &mut App) -> Vec<String> {
        let backend = ratatui::backend::TestBackend::new(100, 34);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let session = Session::new(std::path::PathBuf::from("/tmp"), None, "");
        super::super::draw(&mut term, app, &SkillCatalog::default(), &session).unwrap();
        let buffer = term.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    /// 用户消息正下方那一行等待动画：第 0 秒一个点，之后每秒多一个，三个一轮。
    fn waiting_dots(app: &mut App, elapsed: Duration) -> Option<String> {
        app.awaiting_since = Some(Instant::now() - elapsed);
        app.prepare_transcript_frame();
        app.transcript_rows
            .iter()
            .find(|row| !row.is_empty() && row.chars().all(|character| character == '.'))
            .cloned()
    }

    /// 发出去之后、模型开口之前，用户消息下面有一行 `.` `..` `...` 在转。
    /// 思考行、正文、压缩行一到，它就消失，而且不写进记录。
    #[test]
    fn dots_cycle_under_the_user_message_until_the_model_speaks() {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.transcript_width = 80;
        app.viewport_height = 12;
        app.input.insert("别磨叽了，直接合给 main，提交啊。");
        app.take_prompt_for_submission();

        for (elapsed, expected) in [
            (Duration::from_millis(200), "."),
            (Duration::from_secs(1), ".."),
            (Duration::from_secs(2), "..."),
            (Duration::from_secs(3), "."),
        ] {
            assert_eq!(waiting_dots(&mut app, elapsed).as_deref(), Some(expected));
        }
        // 同一轮不留空行，等待动画紧贴在用户消息正下方。
        let rows = &app.transcript_rows;
        let user = rows.iter().position(|row| row.starts_with("You:")).unwrap();
        assert_eq!(rows[user + 1], ".", "{rows:?}");

        // 思考行一落进记录，动画就撤，记录里不留那几个点。
        app.stream_transient(StreamKind::Reasoning, "先看分支");
        app.prepare_transcript_frame();
        assert!(app.awaiting_since.is_none());
        assert!(
            app.transcript_rows
                .iter()
                .all(|row| { row.is_empty() || row.chars().any(|character| character != '.') }),
            "{:?}",
            app.transcript_rows
        );
        assert!(app.transcript.iter().all(|row| row != "."));

        // 正文是第一份可见输出时同样撤掉。
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.transcript_width = 80;
        app.input.insert("继续");
        app.take_prompt_for_submission();
        assert!(waiting_dots(&mut app, Duration::ZERO).is_some());
        app.note_narration("好的");
        app.prepare_transcript_frame();
        assert!(app.awaiting_since.is_none());
        assert!(app.transcript_rows.iter().all(|row| row != "."));

        // 压缩行说得更具体，动画给它让位，不两行叠着。
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.transcript_width = 80;
        app.input.insert("再来");
        app.take_prompt_for_submission();
        app.note_compression_started();
        app.prepare_transcript_frame();
        assert!(app.awaiting_since.is_none());
        assert!(
            app.transcript_rows
                .iter()
                .any(|row| row.contains("正在压缩上下文"))
        );

        // 轮次结束时还没等到回复，动画也不能一直转下去。
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.input.insert("停");
        app.take_prompt_for_submission();
        app.finish_turn();
        assert!(app.awaiting_reply_row().is_none());

        // 只改状态的斜杠命令不回显成用户消息，也就没有什么可等的。
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.input.insert("/model");
        app.take_prompt_for_submission();
        assert!(app.awaiting_since.is_none());
    }

    /// 动画在真正画出来的那一屏上：紧贴用户消息，每秒从 `.` 轮到 `...`，
    /// 思考行一出现就整行消失。
    #[test]
    fn the_drawn_screen_shows_the_dots_then_drops_them_for_the_thought() {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.ui.mode = crate::config::UiMode::Full;
        app.input.insert("别磨叽了，直接合给 main，提交啊。");
        app.take_prompt_for_submission();

        let stage = |app: &mut App, elapsed: Duration| {
            app.awaiting_since = Some(Instant::now() - elapsed);
            drawn_lines(app)
        };
        for (elapsed, expected) in [
            (Duration::from_millis(200), "."),
            (Duration::from_secs(1), ".."),
            (Duration::from_secs(2), "..."),
        ] {
            let lines = stage(&mut app, elapsed);
            let message = lines.iter().position(|line| line.contains("You:")).unwrap();
            // 边框是 `│`，中间那一格才是内容。紧贴消息的下一行应该只有那几个点。
            let below = lines[message + 1]
                .trim_matches(|ch: char| "│ ".contains(ch))
                .to_owned();
            assert_eq!(below, expected, "at {elapsed:?}: {lines:?}");
        }

        app.stream_transient(StreamKind::Reasoning, "先看分支");
        let lines = drawn_lines(&mut app);
        assert!(
            lines.iter().any(|line| line.contains("Thought:")),
            "the thought replaced the dots: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .all(|line| line.trim().is_empty() || line.trim().chars().any(|ch| ch != '.')),
            "dots stayed on screen: {lines:?}"
        );
    }

    /// 这一帧里某条工具记录对应的屏幕行。`collapse` 选「收起」那一行，否则选工具行本身。
    /// 读的是点击实际匹配的命中表，不是记录位置。
    fn screen_row(app: &App, source: usize, collapse: bool) -> u16 {
        app.tool_row_hits
            .iter()
            .find(|(_, row_source, is_collapse)| *row_source == source && *is_collapse == collapse)
            .map(|(row, _, _)| *row)
            .unwrap_or_else(|| {
                panic!(
                    "no hit for {source} collapse={collapse}: {:?}",
                    app.tool_row_hits
                )
            })
    }

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

    /// 只改状态的斜杠命令不进聊天区；会开工的三条命令和普通提问照常显示。
    #[test]
    fn display_only_commands_stay_out_of_the_transcript() {
        for command in [
            "/model",
            "/model claude",
            "  /help  ",
            "/tools",
            "/session rename x",
        ] {
            assert!(
                is_display_only_command(command),
                "{command} changes state only and should not echo"
            );
        }
        for prompt in [
            "/goal ship it",
            "/local cargo test",
            "/runtime status",
            "看一下日志",
        ] {
            assert!(
                !is_display_only_command(prompt),
                "{prompt} reaches the model and should echo"
            );
        }
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

    /// 点工具行展开，点下面的「收起」收回；点在别处什么都不做。
    #[test]
    fn clicking_a_tool_row_expands_it_and_collapse_folds_it_back() {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        // 命中表按视口过滤。测试里没有布局，给一个装得下整帧的高度。
        app.viewport_height = 40;
        let command = "ssh -o BatchMode=yes -o ConnectTimeout=12 q4-pub '/home/ml-data/rsi-smoke/venv/bin/python'";
        app.note_tool_requested(
            "run_command",
            Some("ssh -o BatchMode=yes …"),
            Some(command.to_owned()),
        );
        app.note_tool_completed("run_command", None, None, false);
        let source = app.transcript.len() - 1;
        app.prepare_transcript_frame();
        app.update_user_message_hits(0, 0);
        let tool_row = screen_row(&app, source, false);
        assert!(app.toggle_tool_row_at(tool_row));
        assert!(app.expanded_tool_rows.contains(&source));
        assert!(
            !app.toggle_tool_row_at(9),
            "a miss must not change anything"
        );

        app.prepare_transcript_frame();
        app.update_user_message_hits(0, 0);
        let shown = app.transcript_rows.join("\n");
        assert!(shown.contains("/home/ml-data/rsi-smoke"), "{shown}");
        assert!(shown.contains("− 收起"), "{shown}");

        // 展开后点的是这一帧真实画出来的「收起」那一行，而不是手造的命中。
        // 点一下会清空命中表，所以先按这一帧重新算屏幕行。
        app.update_user_message_hits(0, 0);
        let collapse_row = screen_row(&app, source, true);
        assert!(app.toggle_tool_row_at(collapse_row), "{collapse_row:?}");
        assert!(
            app.expanded_tool_rows.is_empty(),
            "still expanded: {:?}",
            app.expanded_tool_rows
        );
        app.prepare_transcript_frame();
        let after = app.transcript_rows.join("\n");
        assert!(!after.contains("− 收起"), "{after}");
        assert!(
            !after.contains("/home/ml-data/rsi-smoke"),
            "collapsed row still shows the command: {after}"
        );
    }

    /// 只有一条的段原样保留，不值得配一行汇总。
    #[test]
    fn a_single_tool_row_stays_as_it_is() {
        let entries = tool_rows(1);
        let folded = fold_tool_rows(&entries, false, Language::En);
        assert_eq!(folded.rows, entries);
        assert_eq!(folded.index_of, vec![0]);
    }

    /// 最近一段只留一行汇总，一条具体工具行都不留，也不列出是哪些工具。
    #[test]
    fn the_latest_run_folds_to_one_summary_without_naming_tools() {
        let mut entries = vec!["You: go".to_owned()];
        entries.extend(tool_rows(8));
        entries.push("WillDeep: done".to_owned());
        let folded = fold_tool_rows(&entries, false, Language::En);
        assert_eq!(folded.rows.len(), 1 + 1 + 1, "{:?}", folded.rows);
        assert_eq!(folded.rows[0], "You: go");
        let summary = &folded.rows[1];
        assert_eq!(
            summary, "· ⋯ 8 tool calls folded · 2 failed · /tools to expand",
            "{summary}"
        );
        assert!(
            !summary.contains('×'),
            "the summary names no tool: {summary}"
        );
        assert_eq!(folded.rows[2], "WillDeep: done");
        assert_eq!(folded.index_of, vec![0, 1, 1, 1, 1, 1, 1, 1, 1, 2]);
    }

    /// 更早的段连汇总都不显示，只有最近一段留一行；`/tools` 展开后全部回来。
    #[test]
    fn earlier_runs_disappear_and_expanded_view_shows_all() {
        let mut entries = tool_rows(5);
        entries.push("WillDeep: half way".to_owned());
        entries.extend(tool_rows(3));
        let folded = fold_tool_rows(&entries, false, Language::ZhCn);
        assert_eq!(folded.rows.len(), 1 + 1, "{:?}", folded.rows);
        assert_eq!(folded.rows[0], "WillDeep: half way");
        assert!(
            folded.rows[1].starts_with("· ⋯ 已收起 3 条工具调用")
                && folded.rows[1].ends_with("/tools 展开"),
            "{}",
            folded.rows[1]
        );
        assert!(
            !folded.rows.iter().any(|row| row.contains('×')),
            "no tool names: {:?}",
            folded.rows
        );
        // 被收起的早先段没有汇总行可指，搜索跳转落到它后面的第一条消息上。
        assert_eq!(folded.index_of, vec![0, 0, 0, 0, 0, 0, 1, 1, 1]);

        // 只有一条的段不配汇总，早先那段也照样留着：单条没有什么可收的。
        let mut single = tool_rows(1);
        single.push("WillDeep: x".to_owned());
        single.extend(tool_rows(1));
        let folded = fold_tool_rows(&single, false, Language::ZhCn);
        assert_eq!(
            folded.rows,
            vec![
                single[0].clone(),
                "WillDeep: x".to_owned(),
                single[2].clone()
            ]
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
        // 四段思考只留最后一条；四条工具行收成一条汇总，一条具体的都不留。
        assert_eq!(folded.rows.len(), 1 + 1 + 1, "{:?}", folded.rows);
        assert_eq!(
            folded.rows[1], "· ⋯ 4 tool calls folded · /tools to expand",
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
        assert!(
            !folded.rows.iter().any(|row| tool_row_marker(row).is_some()),
            "no individual tool row stays: {:?}",
            folded.rows
        );
        // 被收起的工具行指向汇总行，被收起的思考行指向留下的那条。
        assert_eq!(folded.index_of, vec![0, 2, 1, 2, 1, 2, 1, 2, 1]);
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
        // 收起时只剩一行汇总。
        assert_eq!(app.display_transcript().rows.len(), 1);

        assert!(app.handle_slash_command("/tools", &SkillCatalog::default()));
        assert!(app.tool_rows_expanded);
        assert!(app.transcript.last().unwrap().starts_with("System: "));
        assert_eq!(app.display_transcript().rows.len(), 9 + 1);

        assert!(app.handle_slash_command("/tools", &SkillCatalog::default()));
        assert!(!app.tool_rows_expanded);
        // 再收起：一行汇总，加上两次 /tools 各自写的 System 行。
        assert_eq!(app.display_transcript().rows.len(), 1 + 2);
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
        app.note_tool_requested("run_command", None, None);
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

        app.note_tool_requested("read_file", None, None);
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
        app.note_tool_requested("run_command", Some("cargo test -p"), None);
        app.note_tool_requested("read_file", None, None);
        assert_eq!(
            app.transcript,
            vec!["· … run_command · cargo test -p", "· … read_file"]
        );

        // 完成事件不带摘要也要对上发起行；同名多行时改最近那条。
        app.note_tool_completed("run_command", None, None, true);
        app.note_tool_completed("read_file", None, None, false);
        assert_eq!(
            app.transcript,
            vec!["· ✗ run_command · cargo test -p", "· ✓ read_file"]
        );

        // 找不到发起行（重开会话后才收到完成事件）就补一行。
        app.note_tool_completed("edit_file", Some("main.rs"), None, false);
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
            app.note_reply("Now update the log line"),
            None
        );
        assert_eq!(app.transcript, vec!["WillDeep: Now update the log line"]);
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
        assert!(
            !row.contains("先看日志"),
            "only the latest line shows: {row}"
        );
        assert!(
            row.contains('s'),
            "the status row carries the elapsed time: {row}"
        );

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

        // 忙时发出的那句是记录里的一行，不进状态行，也不被状态行盖住。
        app.begin_turn(true, "working".to_owned());
        app.note_narration("还在查");
        app.echo_runtime_submission("限定用 postgres");
        assert!(
            app.transcript
                .iter()
                .any(|row| row == "You: 限定用 postgres")
        );
        let row = app.live_transcript_row().unwrap();
        assert!(row.contains("还在查"), "{row}");
        assert!(
            !row.contains("postgres"),
            "the echo is not the status row: {row}"
        );

        // 收尾在最后一段前面拼了提示：两段都落，镜像拿到完整文字。
        app.begin_turn(true, "working".to_owned());
        app.note_narration("写完了");
        assert_eq!(app.note_reply("写完了"), Some("写完了".to_owned()));
        assert!(!app.transcript.iter().any(|row| row.contains("轮次上限")));
        assert_eq!(
            app.transcript
                .iter()
                .filter(|row| row.contains("写完了"))
                .count(),
            2
        );
    }

    /// 压缩期间聊天区一行都不长，用户盯着自己刚发的那句会以为卡住了。状态行就挂在
    /// 那句正下方：压缩中读秒，压完整行换成一句，正文或工具一来就整行撤掉。
    #[test]
    fn compression_notice_sits_under_the_message_that_triggered_it() {
        let mut app = App::new(vec!["You: 现在结果怎么样了？".to_owned()], Language::ZhCn);
        app.begin_turn(true, "working".to_owned());

        app.note_compression_started();
        let row = app.compression_notice_row().unwrap();
        assert!(row.contains("正在压缩上下文"), "{row}");
        assert!(row.contains('s'), "the row counts up: {row}");
        // 不进记录：重开会话不该留下一行「正在压缩」。
        assert_eq!(app.transcript, vec!["You: 现在结果怎么样了？".to_owned()]);

        // 压完是同一行换词，不是再追加一行。
        app.note_compression_completed();
        let row = app.compression_notice_row().unwrap();
        assert!(row.contains("上下文已压缩"), "{row}");
        assert!(!row.contains("正在压缩"), "{row}");
        assert!(
            !row.contains('s'),
            "a finished row does not keep counting: {row}"
        );

        // 真正的输出一开始，这行就不用再解释了。
        app.stream_transient(StreamKind::Reply, "结果是");
        assert!(app.compression_notice_row().is_none());

        // 工具调用同样是第一份可见输出。
        app.note_compression_started();
        app.note_compression_completed();
        app.note_tool_requested("read_file", None, None);
        assert!(app.compression_notice_row().is_none());

        // 中断或换会话时这行不能残留。
        app.note_compression_started();
        app.finish_turn();
        assert!(app.compression_notice_row().is_none());

        // 最后一条不是用户消息时不画：这行只回答「我刚发的那句怎么没反应」。
        app.begin_turn(true, "working".to_owned());
        app.note_narration("先说一句");
        app.note_compression_started();
        assert!(app.compression_notice_row().is_none());
    }

    /// 只有压缩请求还在飞才算「正在压缩」。压完但那一行还挂着时，输入恢复正常处理。
    #[test]
    fn finished_compression_stops_queueing_new_input() {
        let mut app = App::new(vec!["You: 再问一句".to_owned()], Language::ZhCn);
        app.begin_turn(true, "working".to_owned());
        app.note_compression_started();
        assert!(app.compression_in_progress());
        assert!(app.composer_state().0.contains("回车排队"));

        app.note_compression_completed();
        assert!(!app.compression_in_progress());
        assert!(
            app.compression_notice_row()
                .unwrap()
                .contains("上下文已压缩")
        );
        assert!(app.composer_state().0.contains("本轮进行中"));
    }

    /// 压缩行画在用户消息正下方，读秒换词都落在同一行，输出一开始整行消失。
    #[test]
    fn compression_notice_renders_on_the_line_below_the_user_message() {
        let mut app = App::new(
            vec![
                "WillDeep: 上一轮的结论".to_owned(),
                "You: 现在结果怎么样了？".to_owned(),
            ],
            Language::ZhCn,
        );
        app.transcript_width = 80;
        app.viewport_height = 12;
        app.note_compression_started();

        let (_transient, _notice, _waiting) = app.prepare_transcript_frame();
        let rows = app.transcript_rows.clone();
        let user = rows.iter().position(|row| row.starts_with("You:")).unwrap();
        let notice_row = &rows[user + 1];
        assert!(
            notice_row.contains("正在压缩上下文"),
            "the notice sits on the line right under the message: {rows:?}"
        );
        assert!(notice_row.contains('s'), "{notice_row}");

        // 压完还是这一行，只是整行换了词。
        app.note_compression_completed();
        let (transient, notice, waiting) = app.prepare_transcript_frame();
        let rows = &app.transcript_rows;
        let shown = rows.iter().filter(|row| row.contains("压缩")).count();
        assert_eq!(shown, 1, "completion replaces the row: {rows:?}");
        assert!(rows.iter().any(|row| row.contains("上下文已压缩")));

        // 贴底时这行在视口里，不会被算在屏幕外面。视口用的是压完之后那一帧。
        let offset = app.max_scroll();
        let viewport =
            app.transcript_viewport(&transient, notice.as_ref(), waiting.as_ref(), offset);
        assert!(
            viewport
                .lines
                .iter()
                .any(|line| line.to_string().contains("上下文已压缩")),
            "following the bottom still shows the notice"
        );

        // 往上翻时，压缩行后面的历史不能错位或丢行。
        let earlier = app.transcript_viewport(&transient, notice.as_ref(), waiting.as_ref(), 0);
        assert_eq!(
            earlier.lines[0].to_string(),
            "WillDeep: 上一轮的结论",
            "history before the notice keeps its place"
        );

        // 正文一到，这一行就没了，记录里也不留。
        app.note_narration("查到了");
        app.prepare_transcript_frame();
        assert!(
            app.transcript_rows.iter().all(|row| !row.contains("压缩")),
            "{:?}",
            app.transcript_rows
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
                app.append_transcript(format!("You: round {round}"));
                app.append_transcript(format!("WillDeep: done {round}"));
                app.append_transcript(format!("{TURN_DIVIDER_PREFIX}turn {round} ──"));
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
        app.transcript_attachments.clear();
        app.transcript_models.clear();
        app.turn_started = Some(std::time::Instant::now());
        app.note_goal_completed();
        assert!(
            app.transcript[0].starts_with("System: ✓ 目标完成 · 用时 "),
            "{}",
            app.transcript[0]
        );
    }

    /// 系统回执后面落了三条消息就不再画；记录本身一条不动。工具行、分隔线、别的
    /// 系统行都不算「消息」，错误行永远不收。
    #[test]
    fn system_notices_fade_after_three_messages_but_errors_stay() {
        let mut app = App::new(Vec::new(), Language::En);
        app.append_transcript("System: Model switched: qwen".into());
        app.append_transcript("Error: Switch failed: boom".into());
        app.append_transcript("You: one".into());
        app.append_transcript("· ✓ read_file · a.rs".into());
        app.append_transcript("WillDeep: two".into());
        app.append_transcript(format!("{TURN_DIVIDER_PREFIX}turn 1 ──"));
        // 两条消息：还在。
        let rows = app.display_transcript().rows;
        assert!(
            rows.iter().any(|row| row.contains("Model switched")),
            "{rows:?}"
        );

        app.append_transcript("You: three".into());
        // 第三条消息落下：系统行收起，错误行留着，记录不动。
        let folded = app.display_transcript();
        assert!(
            !folded.rows.iter().any(|row| row.contains("Model switched")),
            "{:?}",
            folded.rows
        );
        assert!(folded.rows.iter().any(|row| row.contains("Switch failed")));
        assert!(app.transcript[0].contains("Model switched"));
        // 收起的行在折叠视图里指向它后面的第一行，搜索跳转仍落得上。
        assert_eq!(folded.index_of.len(), app.transcript.len());
        assert_eq!(folded.index_of[0], folded.index_of[1]);

        // 打开搜索并输入词：全部放回来，命中的行看得见。
        app.search = Some(SearchState::default());
        app.search.as_mut().unwrap().editor.insert("switched");
        let rows = app.display_transcript().rows;
        assert!(
            rows.iter().any(|row| row.contains("Model switched")),
            "{rows:?}"
        );
        app.search = None;
        assert!(
            !app.display_transcript()
                .rows
                .iter()
                .any(|row| row.contains("Model switched"))
        );
    }

    /// 过时与否按整份记录算：归档线后面的系统行，计数不因归档而改变。
    #[test]
    fn faded_rows_are_computed_over_the_whole_transcript() {
        let rows = vec![
            "System: a".to_owned(),
            "You: 1".to_owned(),
            "WillDeep: 1".to_owned(),
            "System: b".to_owned(),
            "You: 2".to_owned(),
            "Error: keep".to_owned(),
            "WillDeep: 2".to_owned(),
        ];
        assert_eq!(
            faded_system_rows(&rows),
            vec![true, false, false, false, false, false, false]
        );
        let mut app = App::new(rows, Language::En);
        app.archive_floor = 1;
        let folded = app.display_transcript();
        // 归档说明行 + 可见区六行（记录 1..7）；记录 0 在归档区本来就不画。
        assert_eq!(folded.rows.len(), 1 + 6, "{:?}", folded.rows);
        assert!(folded.rows.iter().any(|row| row.contains("System: b")));
    }

    /// 切换的过程行在结果出来时被换掉，聊天区只留一句；两个事项互不干扰。
    #[test]
    fn switch_notices_replace_their_own_in_progress_row() {
        let mut app = App::new(Vec::new(), Language::En);
        app.append_transcript("You: hi".into());
        app.note_switch_in_progress(
            PendingNotice::ModelSwitch,
            "System: Switching model: qwen".into(),
        );
        app.note_switch_in_progress(
            PendingNotice::ReasoningLevel,
            "System: Reasoning level set to high for the next task".into(),
        );
        assert_eq!(app.transcript.len(), 3);
        // 同一轮里再换一次模型：上一条「正在切换」让位，思考等级那条不动。
        app.note_switch_in_progress(
            PendingNotice::ModelSwitch,
            "System: Switching model: glm".into(),
        );
        assert_eq!(
            app.transcript,
            vec![
                "You: hi",
                "System: Reasoning level set to high for the next task",
                "System: Switching model: glm",
            ]
        );
        app.note_switch_settled(
            PendingNotice::ModelSwitch,
            "System: Model switched: glm".into(),
        );
        app.note_switch_settled(
            PendingNotice::ReasoningLevel,
            "System: Reasoning level switched: high".into(),
        );
        assert_eq!(
            app.transcript,
            vec![
                "You: hi",
                "System: Model switched: glm",
                "System: Reasoning level switched: high",
            ]
        );
        assert!(app.pending_notice_rows.is_empty());
        // 边表与记录始终逐行对齐。
        assert_eq!(app.transcript_attachments.len(), app.transcript.len());
        assert_eq!(app.transcript_models.len(), app.transcript.len());
        // 过程行已经不在（比如 /clear 过）：只落结果，不动别的行。
        app.note_switch_in_progress(
            PendingNotice::ModelSwitch,
            "System: Switching model: x".into(),
        );
        app.transcript.clear();
        app.transcript_attachments.clear();
        app.transcript_models.clear();
        app.note_switch_settled(
            PendingNotice::ModelSwitch,
            "System: Model switched: x".into(),
        );
        assert_eq!(app.transcript, vec!["System: Model switched: x"]);
    }

    /// 删掉一行后，按记录位置记的展开状态跟着前移，不落到邻居身上。
    #[test]
    fn dropping_a_row_shifts_position_keyed_expansions() {
        let mut app = App::new(
            vec![
                "System: Switching model: a".to_owned(),
                "You: long".to_owned(),
                "· ✓ run_command · ls".to_owned(),
            ],
            Language::En,
        );
        app.expanded_user_messages.insert(1, "You: long".to_owned());
        app.expanded_tool_rows.insert(2);
        app.tool_expansions.insert(2, "ls -la".to_owned());
        app.drop_transcript_row(0);
        assert_eq!(
            app.expanded_user_messages.get(&0).map(String::as_str),
            Some("You: long")
        );
        assert!(app.expanded_tool_rows.contains(&1) && !app.expanded_tool_rows.contains(&2));
        assert_eq!(
            app.tool_expansions.get(&1).map(String::as_str),
            Some("ls -la")
        );
        // 删的就是展开的那行：它的状态一起丢，后面的照常前移。
        app.drop_transcript_row(0);
        assert!(app.expanded_user_messages.is_empty());
        assert!(app.expanded_tool_rows.contains(&0));
        assert_eq!(
            app.tool_expansions.get(&0).map(String::as_str),
            Some("ls -la")
        );
    }

    /// 淡出的回执只是不画，仍要当工具行的分段边界：夹在两条工具行中间的回执
    /// 过期后，两侧的单工具行不能被并成一段（旧轮次里那样会两条一起消失）。
    #[test]
    fn fading_a_notice_between_tool_rows_keeps_both_visible() {
        let app = App::new(
            vec![
                "You: q".to_owned(),
                "· ✓ run_command · ls".to_owned(),
                "System: Waiting command moved to background".to_owned(),
                "· ✓ read_file · a.rs".to_owned(),
                "WillDeep: a".to_owned(),
                "You: q2".to_owned(),
                "WillDeep: b".to_owned(),
            ],
            Language::En,
        );
        // 回执后面落了三条消息：回执收起，但两条工具行各自成段，都得留着。
        let folded = app.display_transcript();
        assert!(
            !folded
                .rows
                .iter()
                .any(|row| row.contains("moved to background")),
            "{:?}",
            folded.rows
        );
        assert!(
            folded.rows.iter().any(|row| row.contains("run_command")),
            "{:?}",
            folded.rows
        );
        assert!(
            folded.rows.iter().any(|row| row.contains("read_file")),
            "{:?}",
            folded.rows
        );
    }

    /// 计划卡代替回复出现在记录里，淡出计数要把它算成一条助手消息。
    #[test]
    fn plan_cards_count_as_messages_for_notice_fade() {
        let app = App::new(
            vec![
                "System: Model switched: qwen".to_owned(),
                "You: one".to_owned(),
                "Plan: {\"summary\":\"a\"}".to_owned(),
                "You: two".to_owned(),
                "Plan: {\"summary\":\"b\"}".to_owned(),
            ],
            Language::En,
        );
        assert!(
            !app.display_transcript()
                .rows
                .iter()
                .any(|row| row.contains("Model switched")),
            "{:?}",
            app.display_transcript().rows
        );
    }

    /// 还挂在待发队列上的用户行不算「落了新消息」，不提前把回执收走。
    #[test]
    fn queued_prompts_do_not_count_for_notice_fade() {
        let app = App::new(
            vec![
                "System: Model switched: qwen".to_owned(),
                "You: ［待发］ a".to_owned(),
                "You: ［待发］ b".to_owned(),
                "You: ［待发］ c".to_owned(),
            ],
            Language::En,
        );
        assert!(
            app.display_transcript()
                .rows
                .iter()
                .any(|row| row.contains("Model switched")),
            "{:?}",
            app.display_transcript().rows
        );
    }

    /// 删行时搜索命中跟着前移，Enter 不会跳到邻居身上。
    #[test]
    fn dropping_a_row_shifts_search_matches() {
        let mut app = App::new(
            vec![
                "System: Switching model: a".to_owned(),
                "You: ls".to_owned(),
                "WillDeep: ls".to_owned(),
            ],
            Language::En,
        );
        app.search = Some(SearchState::default());
        app.search.as_mut().unwrap().editor.insert("ls");
        app.refresh_search_matches();
        assert_eq!(app.search.as_ref().unwrap().matches, vec![1, 2]);
        app.drop_transcript_row(0);
        assert_eq!(app.search.as_ref().unwrap().matches, vec![0, 1]);
        // 删的就是当前命中的行：选中位置顺延到后面的那条。
        app.drop_transcript_row(0);
        assert_eq!(app.search.as_ref().unwrap().matches, vec![0]);
        assert_eq!(app.search.as_ref().unwrap().selected, 0);
    }
}
