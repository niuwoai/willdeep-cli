use super::*;

#[path = "viewport_source.rs"]
pub(super) mod source;
use source as viewport_source;

/// 一个 Markdown 代码块在聊天区样式行里的范围，以及去掉围栏后的原文。
/// `start` 含、`end` 不含；`content` 是复制时真正要拿走的正文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CodeBlockLines {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) content: String,
}

/// Completed history is immutable during a text delta. Keep its styled,
/// wrapped rows instead of parsing Markdown twice for every token.
pub(super) struct TranscriptRenderCache {
    source: Vec<String>,
    sources: source::SourceFrame,
    archive_floor: usize,
    show_thinking: bool,
    expanded_users: BTreeMap<usize, String>,
    /// 颜色与待发提示可独立于记录正文变化，必须参与缓存判定。
    queued_users: BTreeSet<usize>,
    models: Vec<Option<String>>,
    width: usize,
    expanded: bool,
    /// 点开的工具行和它的完整内容。内容不在记录里，缓存键必须带上，否则点开没有变化。
    expanded_tools: BTreeMap<usize, String>,
    /// 哪些行处于展开状态。和内容分开记：收起不删内容，只改这一项。
    open_tools: BTreeSet<usize>,
    /// 工具行的自动展开只属于进行中的轮次；收尾后同一份记录要重新折叠。
    running: bool,
    tool_visuals: tool_visuals::ToolVisuals,
    language: Language,
    search: Option<String>,
    time_labels: Vec<Option<String>>,
    /// 回复前缀。缓存键必须带上：`/name` 改名之后旧前缀的画面不能继续用。
    assistant_name: String,
    /// 聊天区每条记录之间额外空出的行数。改了它整屏高度都变，缓存必须重算。
    line_spacing: u8,
    /// 代码块范围按缓存里这份文字的样式行号记；插入等待行时再平移。
    code_blocks: Vec<CodeBlockLines>,
    selection_bounds: Vec<Option<std::ops::Range<usize>>>,
    text: Text<'static>,
}

/// 在 `at` 处插入 `inserted` 行后，平移代码块的行号。
fn shift_code_block_rows(blocks: &mut [CodeBlockLines], at: usize, inserted: usize) {
    if inserted == 0 {
        return;
    }
    for block in blocks {
        if block.start >= at {
            block.start += inserted;
            block.end += inserted;
        } else if block.end > at {
            block.end += inserted;
        }
    }
}

impl App {
    #[allow(clippy::type_complexity)]
    pub(super) fn prepare_transcript_frame(
        &mut self,
    ) -> (
        Text<'static>,
        Option<(usize, Text<'static>)>,
        Option<(usize, Text<'static>)>,
    ) {
        self.viewport_begin_frame();
        let time_labels =
            self.message_times
                .labels(self.transcript.len(), self.language, message_time::now());
        let search = self.active_search_query();
        let queued_users = self.queued_user_records();
        let valid = self.transcript_render_cache.as_ref().is_some_and(|cache| {
            cache.width == self.transcript_width
                && cache.archive_floor == self.archive_floor
                && cache.show_thinking == self.ui.show_thinking
                && cache.expanded_users == self.expanded_user_messages
                && cache.queued_users == queued_users
                && cache.models == self.transcript_models
                && cache.expanded == self.tool_rows_expanded
                && cache.running == self.running
                && cache.expanded_tools == self.tool_expansions
                && cache.open_tools == self.expanded_tool_rows
                && cache.tool_visuals == self.tool_visuals
                && cache.language == self.language
                && cache.search == search
                && cache.assistant_name == self.assistant_name()
                && cache.line_spacing == self.ui.line_spacing
                && cache.source == self.transcript
                && cache.time_labels == time_labels
        });
        if !valid {
            let folded = self.display_transcript();
            let labels = self.message_time_labels(&folded, message_time::now());
            let frame =
                self.render_user_transcript(&folded, &labels, search.as_deref(), folded.rows.len());
            let message_time::FoldedFrame {
                text,
                mut sources,
                user_controls,
                tool_controls,
                attachment_hits,
                code_blocks,
                selection_bounds,
            } = frame;
            self.viewport_record_sources(&mut sources);
            self.user_message_controls = user_controls;
            self.tool_row_controls = tool_controls;
            self.attachment_controls = attachment_hits;
            self.transcript_rows = text_rows(&text);
            self.transcript_render_cache = Some(TranscriptRenderCache {
                source: self.transcript.clone(),
                sources,
                archive_floor: self.archive_floor,
                show_thinking: self.ui.show_thinking,
                expanded_users: self.expanded_user_messages.clone(),
                queued_users,
                models: self.transcript_models.clone(),
                width: self.transcript_width,
                expanded: self.tool_rows_expanded,
                running: self.running,
                expanded_tools: self.tool_expansions.clone(),
                open_tools: self.expanded_tool_rows.clone(),
                tool_visuals: self.tool_visuals.clone(),
                language: self.language,
                search: search.clone(),
                assistant_name: self.assistant_name(),
                line_spacing: self.ui.line_spacing,
                time_labels,
                code_blocks,
                selection_bounds,
                text,
            });
        }
        let cached = self
            .transcript_render_cache
            .as_ref()
            .expect("prepared history");
        let cached_code_blocks = cached.code_blocks.clone();
        let mut sources = cached.sources.clone();
        self.transcript_selection_bounds
            .clone_from(&cached.selection_bounds);
        // Every frame starts from history, not yesterday's inserted waiting row.
        self.transcript_rows = text_rows(&cached.text);
        // 压缩状态与其他临时 AI 输出一样在历史记录之后渲染，不进入会话记录。
        let notice = None;
        // 等待动画每秒变一帧，也不进缓存。它只在最后一条是用户消息且模型尚未
        // 开口时出现，因此插在对应用户消息之后，点名行号才对得上。
        let waiting = self.awaiting_reply_row().map(|row| {
            let waiting = self.awaiting_reply_lines(&row, search.as_deref());
            let at = self.waiting_insert_at();
            self.transcript_rows.splice(at..at, text_rows(&waiting));
            (at, waiting)
        });
        // 压缩行和等待行每帧现插，缓存的代码块行号要跟着平移，否则徽标和双击
        // 会落在这两行上，或者错开一行。插在块中间时只把块尾延长。
        let mut code_blocks = cached_code_blocks;
        for (at, drawn) in [notice.as_ref(), waiting.as_ref()].into_iter().flatten() {
            shift_code_block_rows(&mut code_blocks, *at, drawn.lines.len());
            let id = sources
                .rows
                .get(at.saturating_sub(1))
                .map_or(source::SourceId::Archive, |row| row.fallback.id);
            sources.rows.splice(
                *at..*at,
                (0..drawn.lines.len())
                    .map(|_| source::SourceRow::empty(id, source::SourcePart::Decoration, 0)),
            );
            self.transcript_selection_bounds
                .splice(*at..*at, std::iter::repeat_n(None, drawn.lines.len()));
        }
        self.code_copy.set_blocks(
            code_blocks
                .into_iter()
                .map(|block| code_copy::CodeBlock {
                    start: block.start,
                    end: block.end,
                    content: block.content,
                })
                .collect(),
        );
        self.link_open.refresh(&self.transcript_rows);
        let stream_id = self.viewport_stream_id();
        let transient = self
            .live_transcript_row()
            .map_or_else(Text::default, |row| {
                let mut colored = colored_transcript_at_width(&[row], None, self.transcript_width);
                if self.compression.is_none()
                    && self.transient_kind == StreamKind::Reply
                    && self.transient_thought.is_some()
                    && let Some(line) = colored.lines.first_mut()
                {
                    let label = self.assistant_name();
                    if let Some(suffix) = line.to_string().strip_prefix(&label) {
                        *line = Line::from(vec![
                            assistant_name_span(&label),
                            Span::styled(suffix.to_owned(), line.style),
                        ]);
                    }
                }
                if let Some(query) = search.as_deref() {
                    highlight_matches(&mut colored, query);
                }
                let mappings =
                    if self.compression.is_none() && self.transient_kind == StreamKind::Reply {
                        if let Some(body) = self.transient_thought.as_deref() {
                            sources
                                .parts
                                .insert((stream_id, source::SourcePart::Body), body.to_owned());
                            let prefix =
                                format!("{} · {}: ", self.assistant_name(), self.transient_label());
                            let mut mappings =
                                source::literal_lines(stream_id, source::SourcePart::Body, body);
                            if let Some(first) = mappings.first_mut() {
                                let mut chars = source::SourceLine::literal(
                                    stream_id,
                                    source::SourcePart::Prefix,
                                    &prefix,
                                    0,
                                )
                                .chars;
                                chars.append(&mut first.chars);
                                first.chars = chars;
                            }
                            mappings.truncate(colored.lines.len());
                            mappings
                        } else {
                            colored
                                .lines
                                .iter()
                                .map(|line| {
                                    source::SourceLine::synthetic(
                                        stream_id,
                                        source::SourcePart::Decoration,
                                        &line.to_string(),
                                    )
                                })
                                .collect()
                        }
                    } else {
                        colored
                            .lines
                            .iter()
                            .map(|line| {
                                source::SourceLine::synthetic(
                                    stream_id,
                                    source::SourcePart::Decoration,
                                    &line.to_string(),
                                )
                            })
                            .collect()
                    };
                let (mut transient, mut mapped) =
                    wrap_styled_text_sourced(colored, self.transcript_width, &mappings);
                if self
                    .transcript
                    .last()
                    .is_some_and(|row| row.starts_with("You: "))
                {
                    transient.lines.insert(0, Line::default());
                    mapped.insert(
                        0,
                        source::SourceRow::empty(stream_id, source::SourcePart::Decoration, 0),
                    );
                }
                sources.rows.extend(mapped);
                transient
            });
        self.transcript_rows.extend(text_rows(&transient));
        self.transcript_selection_bounds
            .resize(self.transcript_rows.len(), None);
        self.transcript_height = self.transcript_rows.len();
        debug_assert_eq!(sources.rows.len(), self.transcript_rows.len());
        self.viewport_finish_frame(sources);
        (transient, notice, waiting)
    }

    /// 等待动画画在最后一条用户消息的正下方：跳过其他可见行后紧贴着插。
    fn waiting_insert_at(&self) -> usize {
        self.transcript_selection_bounds
            .iter()
            .rposition(Option::is_some)
            .map_or(self.transcript_rows.len(), |last_user_line| {
                last_user_line + 1
            })
    }

    /// 用户消息正下方的 `.` / `..` / `...`：与 `System:` 行同色，避免灰字难辨认。
    fn awaiting_reply_lines(&self, row: &str, search: Option<&str>) -> Text<'static> {
        let mut text = wrap_styled_text(
            Text::from(Line::styled(
                row.to_owned(),
                Style::default().fg(Color::Yellow),
            )),
            self.transcript_width,
        );
        if let Some(query) = search {
            highlight_matches(&mut text, query);
        }
        text
    }

    pub(super) fn transcript_viewport(
        &self,
        transient: &Text<'static>,
        notice: Option<&(usize, Text<'static>)>,
        waiting: Option<&(usize, Text<'static>)>,
        offset: usize,
    ) -> Text<'static> {
        let history = &self
            .transcript_render_cache
            .as_ref()
            .expect("prepared history")
            .text;
        // 压缩行插在历史中间，底部状态行又接在最后。`transcript_rows` 是这一帧
        // 拼好的全部行，样式按段取：历史走缓存，压缩行用准备帧时画好的那一份，
        // 状态行走调用方传入的那一份。
        let rows = offset..self.transcript_rows.len();
        Text::from(
            rows.take(self.viewport_height)
                .map(|index| {
                    if let Some(drawn) = styled_insert_at(notice, index) {
                        return drawn;
                    }
                    if let Some(drawn) = styled_insert_at(waiting, index) {
                        return drawn;
                    }
                    let inserted_before = styled_insert_rows_before(notice, index)
                        + styled_insert_rows_before(waiting, index);
                    let tail = history.lines.len()
                        + notice.as_ref().map_or(0, |(_, drawn)| drawn.lines.len())
                        + waiting.as_ref().map_or(0, |(_, drawn)| drawn.lines.len());
                    if index < tail {
                        history.lines[index - inserted_before].clone()
                    } else {
                        transient.lines[index - tail].clone()
                    }
                })
                .collect::<Vec<_>>(),
        )
    }
}

/// 这一行落在临时插入段里时，返回它现画好的样式。
fn styled_insert_at(
    insert: Option<&(usize, Text<'static>)>,
    index: usize,
) -> Option<Line<'static>> {
    let (at, drawn) = insert?;
    (*at..*at + drawn.lines.len())
        .contains(&index)
        .then(|| drawn.lines[index - at].clone())
}

/// 这一行前面被临时插入段占掉了几行。
fn styled_insert_rows_before(insert: Option<&(usize, Text<'static>)>, index: usize) -> usize {
    insert
        .filter(|(at, _)| index > *at)
        .map(|(_, drawn)| drawn.lines.len())
        .unwrap_or(0)
}

pub(super) fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

pub(super) fn colored_transcript_at_width(
    entries: &[String],
    search_query: Option<&str>,
    width: usize,
) -> Text<'static> {
    colored_transcript(entries, search_query, width, Language::En)
}

/// 画聊天区。分隔线只有在最后一行时才带「轮到你」，语言跟界面走。
///
/// 这条路径逐条画、看不到下一条是什么，所以不留空行：空行属于整段渲染
/// （[`super::message_time::render_with_user_folds`]），它才知道用户消息前面
/// 该不该让开一行。
pub(super) fn colored_transcript(
    entries: &[String],
    search_query: Option<&str>,
    width: usize,
    language: Language,
) -> Text<'static> {
    colored_transcript_with_name(entries, search_query, width, language, "")
}

pub(super) fn colored_transcript_with_name(
    entries: &[String],
    search_query: Option<&str>,
    width: usize,
    language: Language,
    assistant_name: &str,
) -> Text<'static> {
    colored_transcript_rows(entries, search_query, width, language, assistant_name, true)
}

/// 单条记录不知道自己是否为聊天区末尾，不自行添加等待输入提示。
pub(super) fn colored_transcript_entry_with_name(
    entry: &str,
    search_query: Option<&str>,
    width: usize,
    language: Language,
    assistant_name: &str,
) -> Text<'static> {
    colored_transcript_rows(
        &[entry.to_owned()],
        search_query,
        width,
        language,
        assistant_name,
        false,
    )
}

fn colored_transcript_rows(
    entries: &[String],
    search_query: Option<&str>,
    width: usize,
    language: Language,
    assistant_name: &str,
    prompt_last: bool,
) -> Text<'static> {
    let label = assistant_display_name(assistant_name);
    let last = entries.len().saturating_sub(1);
    let mut lines = Vec::new();
    for (index, raw) in entries.iter().enumerate() {
        // 「轮到你」只属于最后一行。逐行画的路径先统一拿掉，这里再补回来。
        let displayed = if prompt_last && index == last {
            with_your_turn(raw, language)
        } else {
            divider_without_your_turn(raw, language)
        };
        let value = displayed.as_str();
        if let Some((header, body, expanded)) = thought_entry_parts(value) {
            lines.push(Line::styled(
                format!("· {} Thought: {header}", if expanded { "−" } else { "+" }),
                Style::default().fg(Color::LightYellow),
            ));
            if expanded {
                lines.extend(body.lines().map(|line| {
                    Line::styled(line.to_owned(), Style::default().fg(Color::DarkGray))
                }));
            }
            continue;
        }
        if let Some(card) = workflow_ui::decode(value) {
            lines.extend(workflow_ui::render(&card, width, language));
            continue;
        }
        if let Some(card) = decode_plan_entry(value) {
            lines.extend(render_plan(&card.plan, width, language));
            if card.expanded {
                for detail in card.details {
                    lines.extend(detail.lines().map(|line| {
                        Line::styled(line.to_owned(), Style::default().fg(Color::DarkGray))
                    }));
                }
            }
            continue;
        }
        if let Some(content) = value.strip_prefix(ASSISTANT_ROW_PREFIX) {
            lines.extend(render_assistant_markdown_labeled(content, width, label).0);
            // 回复后面跟的是浅色分隔线，两者已经分开了，不再多空一行。
            continue;
        }
        // 工具摘要需要比普通账目行更清晰，避免深色终端上低对比度的 DarkGray 难以辨认。
        let style = if value.starts_with("· + Thought: ") || value.starts_with("· − Thought: ")
        {
            Style::default().fg(Color::LightYellow)
        } else if narration::is_tool_display_row(value) {
            Style::default().fg(Color::Gray)
        } else if value.starts_with("· ") || value.starts_with(TURN_DIVIDER_PREFIX) {
            Style::default().fg(Color::DarkGray)
        } else if value.starts_with("You:") {
            Style::default().fg(Color::Cyan)
        } else if value.starts_with("Error:") {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Yellow)
        };
        // Only the generated welcome banner contains a nested assistant greeting.
        // A user's multiline quote or a tool/code row must keep its literal text.
        let is_welcome = is_welcome_entry(value);
        lines.extend(value.lines().map(|line| {
            if let Some(content) = line
                .strip_prefix(ASSISTANT_ROW_PREFIX)
                .filter(|_| is_welcome)
            {
                // The welcome greeting stays plain text: workspace names may contain Markdown.
                let mut spans = assistant_prefix(true, label);
                spans.push(Span::styled(content.to_owned(), style));
                Line::from(spans)
            } else {
                banner::styled_line(line, width)
                    .or_else(|| banner::styled_name_line(line))
                    .unwrap_or_else(|| Line::styled(line.to_owned(), style))
            }
        }));
    }
    let mut text = Text::from(lines);
    if let Some(query) = search_query {
        highlight_matches(&mut text, query);
    }
    text
}

pub(super) fn thought_entry_parts(value: &str) -> Option<(&str, &str, bool)> {
    let (expanded, prefix) = if let Some(rest) = value.strip_prefix("· + Thought: ") {
        (false, rest)
    } else if let Some(rest) = value.strip_prefix("· − Thought: ") {
        (true, rest)
    } else {
        return None;
    };
    let (header, body) = prefix.split_once('\n').unwrap_or((prefix, ""));
    Some((header, body, expanded))
}

#[cfg(test)]
mod tool_row_style_tests {
    use super::*;

    #[test]
    fn thought_transcript_row_hides_details_until_expanded() {
        let folded = colored_transcript_at_width(
            &["· + Thought: 1.8s\nprivate reasoning".to_owned()],
            None,
            80,
        );
        assert_eq!(folded.lines.len(), 1);
        assert!(folded.lines[0].to_string().contains("Thought: 1.8s"));
        assert!(!folded.lines[0].to_string().contains("private reasoning"));

        let expanded = colored_transcript_at_width(
            &["· − Thought: 1.8s\nprivate reasoning".to_owned()],
            None,
            80,
        );
        assert!(
            expanded
                .lines
                .iter()
                .any(|line| line.to_string() == "private reasoning")
        );
    }

    #[test]
    fn tool_summaries_use_more_contrast_than_regular_status_lines() {
        let tool = colored_transcript_at_width(
            &["· ⋯ 已收起 3 条工具调用 · web_fetch×1".to_owned()],
            None,
            80,
        );
        let status = colored_transcript_at_width(&["· total 3.2s".to_owned()], None, 80);

        assert_eq!(tool.lines[0].style.fg, Some(Color::Gray));
        assert_eq!(status.lines[0].style.fg, Some(Color::DarkGray));
    }

    /// 用户消息与 Thought 之间、下一轮用户开口前各留一行；
    /// 回复与本轮分隔线紧贴。空行由整段渲染补上，逐条上色看不到下一条记录。
    #[test]
    fn the_turn_divider_keeps_one_blank_row_before_the_next_user_message() {
        let text = message_time::render_with_times(
            &[
                "You: first".to_owned(),
                "· + Thought: 3.0s".to_owned(),
                "WillDeep: done".to_owned(),
                "── turn finished · total 1.0s ──".to_owned(),
                "You: next".to_owned(),
            ],
            &[None, None, None, None, None],
            None,
            80,
            Language::En,
        );
        let rows = text
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(rows[0].trim(), "first");
        assert_eq!(rows[1], "");
        assert!(rows[2].contains("Thought: 3.0s"), "{rows:?}");
        assert_eq!(rows[3], "WillDeep: done");
        assert!(rows[4].contains("turn finished"), "{rows:?}");
        assert!(!rows[4].contains("your turn"), "{rows:?}");
        assert_eq!(rows[5], "");
        assert_eq!(rows[6].trim(), "next");
        assert_eq!(rows.len(), 7, "{rows:?}");
    }

    /// 用户直接开口、中间没有「本轮结束」分隔线时，前面也要让开一行。
    /// 聊天区第一条用户消息前面没有内容，不留空行。
    #[test]
    fn a_user_message_keeps_one_blank_row_before_it_even_without_a_divider() {
        let text = message_time::render_with_times(
            &[
                "You: first".to_owned(),
                "WillDeep: done".to_owned(),
                "You: next".to_owned(),
            ],
            &[None, None, None],
            None,
            80,
            Language::En,
        );
        let rows = text
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(
            rows.iter().map(|row| row.trim()).collect::<Vec<_>>(),
            vec!["first", "", "WillDeep: done", "", "next"]
        );
    }

    #[test]
    fn consecutive_assistant_replies_have_one_blank_row_without_trailing_blank() {
        let text = message_time::render_with_times(
            &[
                "You: question".to_owned(),
                "WillDeep: first".to_owned(),
                "WillDeep: second".to_owned(),
                "── turn finished ──".to_owned(),
            ],
            &[None, None, None, None],
            None,
            80,
            Language::En,
        );
        let rows = text
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 6, "{rows:?}");
        assert_eq!(rows[0].trim(), "question");
        assert_eq!(rows[1], "");
        assert!(rows[2].ends_with(": first"), "{rows:?}");
        assert_eq!(rows[3], "");
        assert!(rows[4].ends_with(": second"), "{rows:?}");
        assert!(rows[5].starts_with("── turn finished"), "{rows:?}");
    }

    /// 起过名的实例，回复行显示那个名字而不是 WillDeep；记录原文仍是默认前缀。
    #[test]
    fn a_named_instance_shows_its_name_instead_of_willdeep() {
        let frame = message_time::render_transcript(
            &["WillDeep: 你好".to_owned(), "You: 嗯".to_owned()],
            &[None, None],
            None,
            80,
            Language::ZhCn,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &[],
            "滚小滚",
            0,
            &BTreeSet::new(),
        );
        let rows = text_rows(&frame.text);
        assert_eq!(rows[0], "滚小滚: 你好", "all rows: {rows:?}");
        assert_eq!(frame.text.lines[0].spans[0].content, "滚小滚");
        assert_eq!(frame.text.lines[0].spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(frame.text.lines[0].spans[1].content, ": ");
        assert_eq!(frame.text.lines[0].spans[1].style.fg, Some(Color::Green));
        assert_eq!(frame.text.lines[0].spans[2].style.fg, Some(Color::Green));
        assert_eq!(rows.last().map(|row| row.trim()), Some("嗯"));
        // 没起名时画面不变。
        let plain = message_time::render_with_times(
            &["WillDeep: 你好".to_owned()],
            &[None],
            None,
            80,
            Language::ZhCn,
        );
        assert_eq!(text_rows(&plain)[0], "WillDeep: 你好");
    }

    #[test]
    fn an_empty_reply_keeps_the_name_accent_without_an_extra_space() {
        let (text, _) =
            render_assistant_entry_with_code_blocks(ASSISTANT_ROW_PREFIX, None, 80, "滚小滚");
        assert_eq!(text.lines[0].to_string(), "滚小滚:");
        assert_eq!(text.lines[0].spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(text.lines[0].spans[1].style.fg, Some(Color::Green));
    }

    #[test]
    fn streaming_reply_accents_only_its_instance_name() {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.assistant_name = "滚小滚".to_owned();
        app.stream_transient(StreamKind::Reply, "你好");
        let (transient, _, _) = app.prepare_transcript_frame();
        let line = &transient.lines[0];
        assert!(line.to_string().starts_with("滚小滚 · 回复中: 你好"));
        assert_eq!(line.spans[0].content, "滚小滚");
        assert_eq!(line.spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(line.spans[1].style.fg, Some(Color::Yellow));
    }

    #[test]
    fn a_colon_in_the_reply_body_does_not_become_an_instance_name() {
        let rows = ["WillDeep: Status: done".to_owned()];
        let named = message_time::render_transcript(
            &rows,
            &[None],
            Some("Status"),
            80,
            Language::En,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &[],
            "滚小滚",
            0,
            &BTreeSet::new(),
        );
        assert_eq!(text_rows(&named.text), ["滚小滚: Status: done"]);
        assert!(
            named.text.lines[0]
                .spans
                .iter()
                .any(|span| span.content == "Status" && span.style.bg == Some(Color::Yellow))
        );
        assert_eq!(rows, ["WillDeep: Status: done"]);

        let default = colored_transcript_at_width(&rows, None, 80);
        assert_eq!(text_rows(&default), ["WillDeep: Status: done"]);
    }

    #[test]
    fn a_named_instance_relabels_the_welcome_greeting_after_the_whale() {
        let welcome = welcome_message(std::path::Path::new("/tmp/project"), Language::En);
        let frame = message_time::render_transcript(
            std::slice::from_ref(&welcome),
            &[None],
            None,
            120,
            Language::En,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &[],
            "滚小滚",
            0,
            &BTreeSet::new(),
        );
        let rows = text_rows(&frame.text);
        assert!(
            rows.iter()
                .any(|row| row.starts_with(&banner::name_line("v"))),
            "{rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.starts_with("滚小滚: Hello")),
            "{rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.starts_with("WillDeep: ")),
            "{rows:?}"
        );
        assert!(welcome.contains("WillDeep: Hello"));
        let greeting = frame
            .text
            .lines
            .iter()
            .find(|line| line.to_string().starts_with("滚小滚: Hello"))
            .unwrap();
        assert_eq!(greeting.spans[0].content, "滚小滚");
        assert_eq!(greeting.spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(greeting.spans[1].style.fg, Some(Color::Green));
    }

    #[test]
    fn legacy_wordmark_welcomes_still_relabel_the_assistant_greeting() {
        let legacy = format!("willdeep  v0.1.0\n\n{ASSISTANT_ROW_PREFIX}Hello");
        let text = colored_transcript_with_name(&[legacy], None, 80, Language::En, "demo");
        assert!(text_rows(&text).iter().any(|row| row == "demo: Hello"));
    }

    #[test]
    fn welcome_workspace_name_with_markdown_syntax_stays_literal() {
        let welcome = welcome_message(
            std::path::Path::new("/tmp/repo[archive](2026)"),
            Language::En,
        );
        let displayed = colored_transcript_with_name(
            &[welcome],
            Some("repo[archive](2026)"),
            120,
            Language::En,
            "滚小滚",
        );
        let greeting = displayed
            .lines
            .iter()
            .find(|line| line.to_string().starts_with("滚小滚: Hello"))
            .unwrap();
        assert!(greeting.to_string().contains("repo[archive](2026)"));
        assert!(greeting.spans.iter().any(|span| {
            span.content == "repo[archive](2026)" && span.style.bg == Some(Color::Yellow)
        }));
    }

    #[test]
    fn renamed_assistant_search_matches_the_visible_label() {
        let welcome = welcome_message(std::path::Path::new("/tmp/project"), Language::En);
        let mut app = App::new(
            vec![welcome, "WillDeep: Status: done".to_owned()],
            Language::En,
        );
        app.assistant_name = "滚小滚".to_owned();
        for (query, expected) in [
            ("WillDeep:", Vec::new()),
            ("滚小滚:", vec![0, 1]),
            ("Status: done", vec![1]),
        ] {
            app.search = Some(SearchState::default());
            app.search.as_mut().unwrap().editor.insert(query);
            app.refresh_search_matches();
            assert_eq!(app.search.as_ref().unwrap().matches, expected, "{query}");
        }
        let text = colored_transcript_with_name(
            &[app.transcript[0].clone()],
            Some("滚小滚"),
            120,
            Language::En,
            &app.assistant_name,
        );
        assert!(text.lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content == "滚小滚" && span.style.bg == Some(Color::Yellow))
        }));
    }

    #[test]
    fn multiline_user_and_tool_rows_do_not_relabel_quoted_assistant_text() {
        let rows = [
            "You: quoted line\nWillDeep: Status: done".to_owned(),
            "System: output\nWillDeep: Status: done".to_owned(),
        ];
        let text = colored_transcript_with_name(&rows, None, 80, Language::En, "滚小滚");
        assert_eq!(
            text_rows(&text),
            [
                "You: quoted line",
                "WillDeep: Status: done",
                "System: output",
                "WillDeep: Status: done"
            ]
        );
    }

    /// 行距是每条记录之间额外的空行，最后一条后面不加。
    #[test]
    fn line_spacing_adds_blank_rows_between_entries_but_not_after_the_last() {
        let frame = message_time::render_transcript(
            &["You: 一".to_owned(), "WillDeep: 二".to_owned()],
            &[None, None],
            None,
            80,
            Language::ZhCn,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &[],
            "",
            2,
            &BTreeSet::new(),
        );
        let rows = text_rows(&frame.text);
        // 用户与助手之间基础间距一行，再加配置的两行。
        let blanks = rows.iter().filter(|row| row.is_empty()).count();
        assert_eq!(blanks, 3, "{rows:?}");
        assert_eq!(rows.first().map(|row| row.trim()), Some("一"));
        assert_eq!(rows.last().map(String::as_str), Some("WillDeep: 二"));
        // 行距为 0 时仍保留角色之间的基础空行。
        let tight = message_time::render_transcript(
            &["You: 一".to_owned(), "WillDeep: 二".to_owned()],
            &[None, None],
            None,
            80,
            Language::ZhCn,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &[],
            "",
            0,
            &BTreeSet::new(),
        );
        assert_eq!(
            text_rows(&tight.text)
                .iter()
                .map(|row| row.trim())
                .collect::<Vec<_>>(),
            vec!["一", "", "WillDeep: 二"]
        );
        // 最后一条后面不加：单独一条记录时行距不产生任何空白。
        let single = message_time::render_transcript(
            &["WillDeep: 只有一句".to_owned()],
            &[None],
            None,
            80,
            Language::ZhCn,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &[],
            "",
            2,
            &BTreeSet::new(),
        );
        assert_eq!(text_rows(&single.text), vec!["WillDeep: 只有一句"]);
    }

    /// 一段里没有空格也要按标点断开。模型写首页文案时整句不留空白，只按空白分词
    /// 会把整句当成一个词，超宽时从字中间硬切，屏幕上就是一大段挤在一行里。
    #[test]
    fn prose_without_spaces_breaks_at_punctuation_instead_of_mid_word() {
        let sentence = "**你的 AI Agent，长任务真能跑完。** 用消费级显卡部署多智能体，5090 + 千问 3.8-27B 就能启动一整套 Worker 池。长上下文不是 PPT 能力——100K、200K token 的任务，实测验证过。";
        let row = format!("WillDeep: {sentence}");
        let rendered = wrap_styled_text(colored_transcript_at_width(&[row], None, 40), 40);
        let drawn = rendered
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert!(
            drawn.len() > 3,
            "the sentence stayed on too few rows: {drawn:?}"
        );
        assert!(
            drawn
                .iter()
                .all(|line| UnicodeWidthStr::width(line.as_str()) <= 40),
            "a row ran past the width: {drawn:?}"
        );
        // 断开落在标点后面，不把一个词切成两半。
        assert!(
            !drawn.iter().any(|line| line.ends_with('千')),
            "broke inside a word: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|line| {
                line.trim_end().ends_with('，') || line.trim_end().ends_with('。')
            }),
            "never broke after punctuation: {drawn:?}"
        );
    }

    #[test]
    fn folded_dividers_touch_the_reply_and_prompt_only_at_the_end() {
        for (language, divider) in [
            (Language::ZhCn, "── 本轮结束 ──"),
            (Language::En, "── turn finished ──"),
            (Language::Ja, "── ターン終了 ──"),
        ] {
            let prompt = your_turn_label(language);
            let entries = [
                format!("{ASSISTANT_ROW_PREFIX}first paragraph\n\nlast line"),
                with_your_turn(divider, language),
                "You: next".to_owned(),
                format!("{ASSISTANT_ROW_PREFIX}current reply"),
                divider.to_owned(),
            ];
            for width in [32, 100] {
                for spacing in [0, 2] {
                    for end in [4, 5] {
                        let frame = message_time::render_transcript(
                            &entries[..end],
                            &[Some("6m ago · fixture/model".to_owned())],
                            None,
                            width,
                            language,
                            &BTreeMap::new(),
                            &BTreeMap::new(),
                            &[],
                            "fixture",
                            spacing,
                            &BTreeSet::new(),
                        );
                        let rows = text_rows(&frame.text);
                        let dividers = rows
                            .iter()
                            .enumerate()
                            .filter(|(_, row)| row.starts_with(TURN_DIVIDER_PREFIX))
                            .collect::<Vec<_>>();
                        assert_eq!(dividers.len(), end - 3, "{rows:?}");
                        for (index, _) in &dividers {
                            assert!(*index > 0 && !rows[index - 1].is_empty(), "{rows:?}");
                            assert_ne!(
                                frame.sources.rows[index - 1].fallback.part,
                                rendering::source::SourcePart::Decoration,
                                "{rows:?}"
                            );
                        }
                        assert!(!dividers[0].1.contains(prompt), "{rows:?}");
                        assert_eq!(
                            rows.iter().filter(|row| row.contains(prompt)).count(),
                            usize::from(end == entries.len()),
                            "{rows:?}"
                        );
                    }
                }
            }
        }
    }

    /// 「轮到你」只属于最后一行。后面有了新输出，旧分隔线就不再说这话。
    #[test]
    fn your_turn_appears_only_on_the_last_divider() {
        let rows = [
            "── turn finished ──".to_owned(),
            "You: next".to_owned(),
            "── turn finished ──".to_owned(),
        ];
        let text = colored_transcript(&rows, None, 80, Language::ZhCn);
        let drawn = text
            .lines
            .iter()
            .map(ToString::to_string)
            .filter(|row| row.starts_with(TURN_DIVIDER_PREFIX))
            .collect::<Vec<_>>();
        assert_eq!(drawn.len(), 2, "{drawn:?}");
        assert!(!drawn[0].contains("轮到你"), "{drawn:?}");
        assert!(drawn[1].ends_with("· 轮到你 ──"), "{drawn:?}");

        // 旧版本把「轮到你」写进了记录：画的时候先拿掉，再只补最后一行。
        for stored in [Language::ZhCn, Language::En, Language::Ja] {
            let legacy = with_your_turn("── turn finished ──", stored);
            for current in [Language::ZhCn, Language::En, Language::Ja] {
                assert_eq!(
                    divider_without_your_turn(&legacy, current),
                    "── turn finished ──"
                );
            }
        }
    }
}

pub(super) fn highlight_matches(text: &mut Text<'static>, query: &str) {
    let Ok(pattern) = RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()
    else {
        return;
    };
    let highlight = Style::default()
        .fg(Color::Black)
        .bg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    for line in &mut text.lines {
        let spans = std::mem::take(&mut line.spans);
        line.spans = spans
            .into_iter()
            .flat_map(|span| {
                let value = span.content.into_owned();
                let mut output = Vec::new();
                let mut offset = 0;
                for found in pattern.find_iter(&value) {
                    if found.start() > offset {
                        output.push(Span::styled(
                            value[offset..found.start()].to_owned(),
                            span.style,
                        ));
                    }
                    output.push(Span::styled(
                        value[found.start()..found.end()].to_owned(),
                        span.style.patch(highlight),
                    ));
                    offset = found.end();
                }
                if offset < value.len() {
                    output.push(Span::styled(value[offset..].to_owned(), span.style));
                }
                if output.is_empty() {
                    output.push(Span::styled(value, span.style));
                }
                output
            })
            .collect();
    }
}

#[cfg(test)]
pub(super) fn rendered_transcript_height(entries: &[String], width: usize) -> usize {
    wrap_styled_text(colored_transcript_at_width(entries, None, width), width)
        .lines
        .len()
}

pub(super) fn wrap_styled_text(text: Text<'static>, width: usize) -> Text<'static> {
    wrap_styled_text_sourced(text, width, &[]).0
}

pub(super) fn wrap_styled_text_sourced(
    text: Text<'static>,
    width: usize,
    sources: &[viewport_source::SourceLine],
) -> (Text<'static>, Vec<viewport_source::SourceRow>) {
    use viewport_source::{SourceId, SourcePart, SourcePoint, SourceRow};
    let width = width.max(1);
    let mut rows = Vec::new();
    for (source_index, source) in text.lines.into_iter().enumerate() {
        let mapping = sources.get(source_index);
        let fallback = mapping.map_or(
            SourcePoint {
                id: SourceId::Archive,
                part: SourcePart::Decoration,
                byte: 0,
            },
            |line| line.fallback,
        );
        // A `Line` carries a style of its own, and `Line::styled` puts the
        // colour *there* while leaving its spans raw. Flattening to characters
        // has to fold that base in, or every line coloured that way — the whole
        // `You:` / `Error:` transcript palette — renders in the terminal's
        // default foreground.
        let base = source.style;
        let cells = source
            .spans
            .into_iter()
            .flat_map(|span| {
                let style = base.patch(span.style);
                span.content
                    .chars()
                    .map(move |character| StyledCharacter {
                        character,
                        style,
                        width: UnicodeWidthChar::width(character).unwrap_or(0),
                        source: None,
                    })
                    .collect::<Vec<_>>()
            })
            .enumerate()
            .map(|(index, mut cell)| {
                cell.source = mapping
                    .and_then(|line| line.chars.get(index))
                    .copied()
                    .flatten();
                cell
            })
            .collect::<Vec<_>>();
        if cells.is_empty() {
            rows.push((fallback, Vec::new()));
            continue;
        }
        let mut line = Vec::new();
        let mut line_width: usize = 0;
        let mut whitespace = Vec::new();
        let mut index = 0;
        while index < cells.len() {
            if cells[index].character.is_whitespace() {
                whitespace.push(cells[index]);
                index += 1;
                continue;
            }
            let word_start = index;
            // 一段里没有空白也要能断开：中文句子和 markdown 行内符号都不靠空格分词，
            // 只认空白的话，整句会被当成一个词，一旦超过宽度就从字符中间硬切，
            // 看起来就是一整段挤在一行上。断点留在上一行末尾，不掂到下一行开头。
            while index < cells.len()
                && !cells[index].character.is_whitespace()
                && !wrap_break_after(cells[index].character)
            {
                index += 1;
            }
            if index < cells.len() && wrap_break_after(cells[index].character) {
                index += 1;
            }
            let word = &cells[word_start..index];
            let whitespace_width = whitespace.iter().map(|cell| cell.width).sum::<usize>();
            let word_width = word.iter().map(|cell| cell.width).sum::<usize>();
            if word_width <= width
                && (line_width > 0 || whitespace_width.saturating_add(word_width) <= width)
            {
                if line_width > 0
                    && line_width
                        .saturating_add(whitespace_width)
                        .saturating_add(word_width)
                        > width
                {
                    rows.push((fallback, std::mem::take(&mut line)));
                    line_width = 0;
                    whitespace.clear();
                }
                line.extend_from_slice(&whitespace);
                line_width = line_width.saturating_add(whitespace_width);
                whitespace.clear();
                line.extend_from_slice(word);
                line_width = line_width.saturating_add(word_width);
                continue;
            }
            if line_width > 0 && line_width.saturating_add(whitespace_width) >= width {
                rows.push((fallback, std::mem::take(&mut line)));
                line_width = 0;
                whitespace.clear();
            }
            for cell in whitespace.drain(..).chain(word.iter().copied()) {
                if line_width > 0 && line_width.saturating_add(cell.width) > width {
                    rows.push((fallback, std::mem::take(&mut line)));
                    line_width = 0;
                }
                line.push(cell);
                line_width = line_width.saturating_add(cell.width);
            }
        }
        for cell in whitespace {
            if line_width > 0 && line_width.saturating_add(cell.width) > width {
                rows.push((fallback, std::mem::take(&mut line)));
                line_width = 0;
            }
            line.push(cell);
            line_width = line_width.saturating_add(cell.width);
        }
        if !line.is_empty() {
            rows.push((fallback, line));
        }
    }
    let mut text = Text::default();
    let mut mappings = Vec::with_capacity(rows.len());
    for (fallback, cells) in rows {
        let mut spans = Vec::new();
        let mut mapped = SourceRow {
            fallback,
            cells: Vec::new(),
        };
        let mut column = 0;
        for cell in cells {
            push_styled_character(&mut spans, cell.character, cell.style);
            if let Some(source) = cell.source {
                let columns = if cell.width == 0 {
                    mapped
                        .cells
                        .last()
                        .map_or(column..column, |previous| previous.columns.clone())
                } else {
                    column..column + cell.width
                };
                mapped.cells.push(viewport_source::SourceCell {
                    columns,
                    source,
                    character: cell.character,
                });
            }
            column += cell.width;
        }
        if let Some(first) = mapped.cells.first() {
            mapped.fallback = first.source.point;
        }
        text.lines.push(Line::from(spans));
        mappings.push(mapped);
    }
    (text, mappings)
}

#[derive(Clone, Copy)]
struct StyledCharacter {
    character: char,
    style: Style,
    width: usize,
    source: Option<viewport_source::SourceChar>,
}

/// 折行时可以在它后面断开的字符。
///
/// 空白由调用方单独处理，这里只管非空白的断点：中文标点，以及 markdown 行内常用的
/// 分隔符。模型写首页文案时整句不留空格，没有这些断点就只能从字中间硬切。
fn wrap_break_after(character: char) -> bool {
    matches!(
        character,
        '\u{2014}'
            | '\u{2013}'
            | '-'
            | '\u{00b7}'
            | '\u{30fb}'
            | '\u{3001}'
            | '\u{3002}'
            | '\u{ff0c}'
            | '\u{ff01}'
            | '\u{ff1f}'
            | '\u{ff1a}'
            | '\u{ff1b}'
            | '\u{300a}'
            | '\u{300b}'
            | '\u{300c}'
            | '\u{300d}'
            | '\u{201c}'
            | '\u{201d}'
            | '\u{2018}'
            | '\u{2019}'
            | '('
            | ')'
            | '\u{ff08}'
            | '\u{ff09}'
            | '/'
            | '|'
            | '*'
            | '#'
            | '+'
            | '='
            | '\u{2026}'
    )
}

fn push_styled_character(spans: &mut Vec<Span<'static>>, character: char, style: Style) {
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.content.to_mut().push(character);
        return;
    }
    spans.push(Span::styled(character.to_string(), style));
}

pub(super) fn text_rows(text: &Text<'static>) -> Vec<String> {
    text.lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

pub(super) fn selected_text(
    rows: &[String],
    bounds: &[Option<std::ops::Range<usize>>],
    start: (usize, usize),
    end: (usize, usize),
) -> String {
    if rows.is_empty() || start >= end {
        return String::new();
    }
    let last_row = end.0.min(rows.len().saturating_sub(1));
    (start.0..=last_row)
        .map(|row| {
            let mut start_column = if row == start.0 { start.1 } else { 0 };
            let mut end_column = if row == end.0 { end.1 } else { usize::MAX };
            if let Some(Some(content)) = bounds.get(row) {
                start_column = start_column.max(content.start);
                end_column = end_column.min(content.end);
            }
            if start_column >= end_column {
                String::new()
            } else {
                display_column_slice(&rows[row], start_column, end_column)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn display_column_slice(value: &str, start: usize, end: usize) -> String {
    let mut output = String::new();
    let mut column = 0;
    let mut previous_selected = false;
    for character in value.chars() {
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        let selected = if width == 0 {
            previous_selected
        } else {
            column < end && column.saturating_add(width) > start
        };
        if selected {
            output.push(character);
        }
        previous_selected = selected;
        column = column.saturating_add(width);
    }
    output
}

#[cfg(test)]
pub(super) fn highlight_text_selection(
    text: &mut Text<'static>,
    start: (usize, usize),
    end: (usize, usize),
) {
    highlight_text_selection_at_offset(text, start, end, 0);
}

pub(super) fn highlight_text_selection_at_offset(
    text: &mut Text<'static>,
    start: (usize, usize),
    end: (usize, usize),
    offset: usize,
) {
    if start >= end {
        return;
    }
    // 与弹窗同一对固定色：调色板里的 Blue / White 在浅色系主题下对比度很低。
    let selection_style = Style::default()
        .fg(super::MODAL_FG)
        .bg(super::MODAL_BG)
        .add_modifier(Modifier::BOLD);
    for (row, line) in text.lines.iter_mut().enumerate() {
        let row = row + offset;
        if row < start.0 || row > end.0 {
            continue;
        }
        let start_column = if row == start.0 { start.1 } else { 0 };
        let end_column = if row == end.0 { end.1 } else { usize::MAX };
        let source = std::mem::take(&mut line.spans);
        let mut column = 0;
        let mut previous_selected = false;
        let mut highlighted = Vec::new();
        for span in source {
            for character in span.content.chars() {
                let width = UnicodeWidthChar::width(character).unwrap_or(0);
                let selected = if width == 0 {
                    previous_selected
                } else {
                    column < end_column && column.saturating_add(width) > start_column
                };
                let style = if selected {
                    span.style.patch(selection_style)
                } else {
                    span.style
                };
                push_styled_character(&mut highlighted, character, style);
                previous_selected = selected;
                column = column.saturating_add(width);
            }
        }
        line.spans = highlighted;
    }
}

pub(super) fn render_assistant_markdown(content: &str, width: usize) -> Vec<Line<'static>> {
    render_assistant_markdown_labeled(content, width, ASSISTANT_LABEL).0
}

/// 和 [`render_assistant_markdown`] 一样，只是第一行的名字用调用方给的那个。
pub(super) fn render_assistant_markdown_labeled(
    content: &str,
    width: usize,
    label: &str,
) -> (Vec<Line<'static>>, Vec<CodeBlockLines>) {
    render_assistant_markdown_annotated(content, width, label)
}

/// 和 `render_assistant_markdown` 画得完全一样，额外报出每个代码块落在哪些输出行。
/// 输出行号是折行前的；调用方按每行折出来的高度累加，才能对上屏幕行号。
fn render_assistant_markdown_annotated(
    content: &str,
    width: usize,
    label: &str,
) -> (Vec<Line<'static>>, Vec<CodeBlockLines>) {
    // Ordinary rendering discards provenance, but uses the same parser.
    let (output, blocks, _) = source::assistant(content, width, label, source::SourceId::Archive);
    (output, blocks)
}

fn push_code_block(blocks: &mut Vec<CodeBlockLines>, start: usize, end: usize, content: &[String]) {
    let body = content.join("\n");
    // 只有空白的块画出来也是空的，不给它一个可复制的徽标。
    if body.trim().is_empty() {
        return;
    }
    blocks.push(CodeBlockLines {
        start,
        end,
        content: body,
    });
}

/// 画一条 `WillDeep: ` 记录，同时给出代码块折行后的行号范围。
/// 每个输出行单独折行（空行也占一行），所以行号可以逐行累加，不需要再解析一遍。
#[cfg(test)]
pub(super) fn render_assistant_entry_with_code_blocks(
    row: &str,
    search: Option<&str>,
    width: usize,
    assistant_name: &str,
) -> (Text<'static>, Vec<CodeBlockLines>) {
    let Some(content) = row.strip_prefix(ASSISTANT_ROW_PREFIX) else {
        let rows = [row.to_owned()];
        let text = wrap_styled_text(colored_transcript_at_width(&rows, search, width), width);
        return (text, Vec::new());
    };
    let (text, blocks, _) = source::assistant_entry(
        content,
        search,
        width,
        assistant_name,
        source::SourceId::Archive,
    );
    (text, blocks)
}

fn assistant_prefix(show: bool, label: &str) -> Vec<Span<'static>> {
    if show {
        assistant_prefix_with_suffix(label, ": ")
    } else {
        Vec::new()
    }
}

fn assistant_name_span(label: &str) -> Span<'static> {
    Span::styled(
        label.to_owned(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
}

fn assistant_prefix_with_suffix(label: &str, suffix: &str) -> Vec<Span<'static>> {
    vec![
        assistant_name_span(label),
        Span::styled(
            suffix.to_owned(),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    ]
}

fn render_markdown_line(raw: &str, show_prefix: bool, label: &str) -> Line<'static> {
    let mut spans = assistant_prefix(show_prefix, label);
    let trimmed = raw.trim_start();
    let (marker, body, base) = if let Some(body) = trimmed.strip_prefix("### ") {
        (
            "▸ ",
            body,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else if let Some(body) = trimmed.strip_prefix("## ") {
        (
            "◆ ",
            body,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else if let Some(body) = trimmed.strip_prefix("# ") {
        (
            "■ ",
            body,
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        )
    } else if let Some(body) = trimmed.strip_prefix("> ") {
        (
            "│ ",
            body,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )
    } else if let Some(body) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        ("• ", body, Style::default().fg(Color::Green))
    } else {
        ("", raw, Style::default().fg(Color::Green))
    };
    if !marker.is_empty() {
        spans.push(Span::styled(marker, base));
    }
    spans.extend(render_inline_markdown(body, base));
    Line::from(spans)
}

fn normalize_html_breaks(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(index) = rest.find('<') {
        output.push_str(&rest[..index]);
        let candidate = &rest[index..];
        let tag_length = ["<br />", "<br/>", "<br>"]
            .into_iter()
            .find(|tag| {
                candidate
                    .get(..tag.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(tag))
            })
            .map(str::len);
        if let Some(tag_length) = tag_length {
            output.push('\n');
            rest = &candidate[tag_length..];
        } else {
            output.push('<');
            rest = &candidate[1..];
        }
    }
    output.push_str(rest);
    output
}

fn parse_table_row(value: &str) -> Option<Vec<String>> {
    let trimmed = value.trim();
    if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
        return None;
    }
    let cells = trimmed[1..trimmed.len() - 1]
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect::<Vec<_>>();
    (cells.len() >= 2).then_some(cells)
}

fn is_table_separator(value: &str) -> bool {
    parse_table_row(value).is_some_and(|cells| {
        cells.iter().all(|cell| {
            let marker = cell.trim().trim_matches(':').trim();
            marker.len() >= 3 && marker.bytes().all(|byte| byte == b'-')
        })
    })
}

fn terminal_cell_text(value: &str) -> String {
    normalize_html_breaks(value)
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
}

fn table_column_widths(rows: &[Vec<String>], width: usize) -> Vec<usize> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut natural = vec![1; columns];
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            for line in terminal_cell_text(cell).split('\n') {
                natural[column] = natural[column].max(UnicodeWidthStr::width(line).max(1));
            }
        }
    }
    let separators = columns.saturating_sub(1) * 3 + 4;
    let available = width.max(columns + separators).saturating_sub(separators);
    if natural.iter().sum::<usize>() <= available {
        return natural;
    }
    let mut result = vec![1; columns];
    let mut remaining = available.saturating_sub(columns);
    while remaining > 0 {
        let mut changed = false;
        for column in 0..columns {
            if result[column] < natural[column] && remaining > 0 {
                result[column] += 1;
                remaining -= 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    result
}

fn wrap_table_cell(value: &str, width: usize) -> Vec<String> {
    let mut output = Vec::new();
    for explicit_line in terminal_cell_text(value).split('\n') {
        let mut line = String::new();
        let mut columns = 0;
        for character in explicit_line.chars() {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if columns + character_width > width && !line.is_empty() {
                output.push(std::mem::take(&mut line));
                columns = 0;
            }
            line.push(character);
            columns += character_width;
            if columns >= width {
                output.push(std::mem::take(&mut line));
                columns = 0;
            }
        }
        if !line.is_empty() || explicit_line.is_empty() {
            output.push(line);
        }
    }
    if output.is_empty() {
        output.push(String::new());
    }
    output
}

fn table_horizontal_rule(widths: &[usize], left: char, junction: char, right: char) -> String {
    let inner = widths
        .iter()
        .map(|width| "─".repeat(width + 2))
        .collect::<Vec<_>>()
        .join(&junction.to_string());
    format!("{left}{inner}{right}")
}

fn render_markdown_table(rows: &[Vec<String>], width: usize) -> Vec<Line<'static>> {
    let widths = table_column_widths(rows, width.max(1));
    let rule_style = Style::default().fg(Color::DarkGray);
    let mut output = vec![Line::styled(
        table_horizontal_rule(&widths, '┌', '┬', '┐'),
        rule_style,
    )];
    for (row_index, row) in rows.iter().enumerate() {
        let cells = widths
            .iter()
            .enumerate()
            .map(|(column, width)| {
                wrap_table_cell(row.get(column).map(String::as_str).unwrap_or(""), *width)
            })
            .collect::<Vec<_>>();
        let row_height = cells.iter().map(Vec::len).max().unwrap_or(1);
        for line_index in 0..row_height {
            let mut spans = vec![Span::styled("│ ", rule_style)];
            for (column, column_width) in widths.iter().enumerate() {
                if column > 0 {
                    spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
                }
                let value = cells[column]
                    .get(line_index)
                    .map(String::as_str)
                    .unwrap_or("");
                let padding = column_width.saturating_sub(UnicodeWidthStr::width(value));
                let style = if row_index == 0 {
                    Style::default()
                        .fg(Color::LightCyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Green)
                };
                spans.push(Span::styled(value.to_owned(), style));
                spans.push(Span::styled(" ".repeat(padding), style));
            }
            spans.push(Span::styled(" │", rule_style));
            output.push(Line::from(spans));
        }
        let (left, junction, right) = if row_index + 1 == rows.len() {
            ('└', '┴', '┘')
        } else {
            ('├', '┼', '┤')
        };
        output.push(Line::styled(
            table_horizontal_rule(&widths, left, junction, right),
            rule_style,
        ));
    }
    output
}

fn render_inline_markdown(value: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = value;
    while !rest.is_empty() {
        let bold = rest.find("**").map(|index| (index, "bold"));
        let code = rest.find('`').map(|index| (index, "code"));
        let image = rest.find("![").map(|index| (index, "image"));
        let link = rest.find('[').map(|index| (index, "link"));
        let Some((index, kind)) = [bold, code, image, link]
            .into_iter()
            .flatten()
            .min_by_key(|item| item.0)
        else {
            spans.push(Span::styled(rest.to_owned(), base));
            break;
        };
        if index > 0 {
            spans.push(Span::styled(rest[..index].to_owned(), base));
            rest = &rest[index..];
        }
        match kind {
            "bold" if rest[2..].find("**").is_some() => {
                let end = rest[2..].find("**").unwrap() + 2;
                spans.push(Span::styled(
                    rest[2..end].to_owned(),
                    base.add_modifier(Modifier::BOLD),
                ));
                rest = &rest[end + 2..];
            }
            "code" if rest[1..].find('`').is_some() => {
                let end = rest[1..].find('`').unwrap() + 1;
                spans.push(Span::styled(
                    rest[1..end].to_owned(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ));
                rest = &rest[end + 1..];
            }
            "image" if rest.find("](").is_some() => {
                let label_end = rest.find("](").expect("image label was checked");
                if let Some(url_end) = rest[label_end + 2..].find(')') {
                    let url_end = label_end + 2 + url_end;
                    let label = rest[2..label_end].trim();
                    spans.push(Span::styled(
                        format!("▧ {} · Ctrl+L", if label.is_empty() { "—" } else { label }),
                        Style::default()
                            .fg(Color::LightGreen)
                            .add_modifier(Modifier::BOLD),
                    ));
                    rest = &rest[url_end + 1..];
                } else {
                    spans.push(Span::styled(rest[..1].to_owned(), base));
                    rest = &rest[1..];
                }
            }
            "link" if rest.find("](").is_some() => {
                let label_end = rest.find("](").unwrap();
                if let Some(url_end) = rest[label_end + 2..].find(')') {
                    let url_end = label_end + 2 + url_end;
                    spans.push(Span::styled(
                        rest[1..label_end].to_owned(),
                        Style::default()
                            .fg(Color::LightBlue)
                            .add_modifier(Modifier::UNDERLINED),
                    ));
                    spans.push(Span::styled(
                        format!(" ({})", &rest[label_end + 2..url_end]),
                        base,
                    ));
                    rest = &rest[url_end + 1..];
                } else {
                    spans.push(Span::styled(rest[..1].to_owned(), base));
                    rest = &rest[1..];
                }
            }
            _ => {
                spans.push(Span::styled(rest[..1].to_owned(), base));
                rest = &rest[1..];
            }
        }
    }
    spans
}

/// 流式回复预览只留最后这么多字符，进程内与 Runtime 轮次共用。
pub(super) const THOUGHT_PREVIEW_CHARS: usize = 1200;

/// 「本轮结束」分隔线的前缀。按账目行的灰色画，紧贴上一轮输出，不另占空行。
pub(super) const TURN_DIVIDER_PREFIX: &str = "── ";

/// 助手回复在记录里的固定前缀；显示名由渲染调用方传入。
/// 改名不改记录，历史里的旧行重开后也跟着新名字走。
pub(super) const ASSISTANT_ROW_PREFIX: &str = "WillDeep: ";

/// 没给实例起名时屏幕上用的名字。
pub(super) const ASSISTANT_LABEL: &str = "WillDeep";

/// 显示名只由调用方给出；记录本身只含固定前缀和原始正文。
fn assistant_display_name(name: &str) -> &str {
    if name.is_empty() {
        ASSISTANT_LABEL
    } else {
        name
    }
}

fn is_welcome_entry(value: &str) -> bool {
    value.contains("\nWillDeep: ")
        && (value
            .lines()
            .next()
            .is_some_and(|line| banner::styled_name_line(line).is_some())
            || value.starts_with(&banner::art_rows().join("\n")))
        && value
            .lines()
            .any(|line| banner::styled_name_line(line).is_some())
}

/// 搜索与聊天区使用同一个助手显示名，但会话记录仍保留固定前缀。
/// 只投影显示名，不解释 Markdown，也不改变正文和会话文件。
pub(super) fn searchable_transcript_row<'a>(
    row: &'a str,
    assistant_name: &str,
) -> std::borrow::Cow<'a, str> {
    let label = assistant_display_name(assistant_name);
    if label == ASSISTANT_LABEL {
        return std::borrow::Cow::Borrowed(row);
    }
    if let Some(body) = row.strip_prefix(ASSISTANT_ROW_PREFIX) {
        return std::borrow::Cow::Owned(format!("{label}: {body}"));
    }
    if is_welcome_entry(row)
        && let Some((banner, greeting)) = row.rsplit_once("\nWillDeep: ")
    {
        return std::borrow::Cow::Owned(format!("{banner}\n{label}: {greeting}"));
    }
    std::borrow::Cow::Borrowed(row)
}

/// 分隔线末尾的「轮到你」。只在聊天区最后一行才有意义：新输出一来它就不再是事实。
pub(super) fn your_turn_label(language: Language) -> &'static str {
    language.text("轮到你", "your turn", "あなたの番")
}

/// 分隔线去掉末尾的「轮到你」。
///
/// 旧版本把它写进了记录，新版本只在画的时候决定。逐行画的路径分不清这行是不是
/// 聊天区最后一行，所以先统一拿掉，由整段渲染在最后一行补回去。
pub(super) fn divider_without_your_turn(row: &str, language: Language) -> String {
    let Some(body) = row
        .strip_prefix(TURN_DIVIDER_PREFIX)
        .and_then(|value| value.strip_suffix(" ──"))
    else {
        return row.to_owned();
    };
    // 历史记录可能来自其他界面语言；切换语言也不能保留旧等待提示。
    let body = [language, Language::ZhCn, Language::En, Language::Ja]
        .into_iter()
        .find_map(|language| body.strip_suffix(&format!(" · {}", your_turn_label(language))))
        .unwrap_or(body);
    format!("{TURN_DIVIDER_PREFIX}{body} ──")
}

/// 聊天区最后一行是分隔线时，补上「轮到你」。
pub(super) fn with_your_turn(row: &str, language: Language) -> String {
    let row = divider_without_your_turn(row, language);
    let Some(body) = row
        .strip_prefix(TURN_DIVIDER_PREFIX)
        .and_then(|value| value.strip_suffix(" ──"))
    else {
        return row;
    };
    format!(
        "{TURN_DIVIDER_PREFIX}{body} · {} ──",
        your_turn_label(language)
    )
}

/// 进聊天区之前把终端会「吃掉」的字符处理掉：制表符按 4 列展开，换行保留，
/// `\r` 丢弃，其余控制字符转成可见转义。模型思维链和代码里的原始 `\t` 直接写到
/// 终端时光标跳到制表位，而 Ratatui 只当它是一格，两边失步后旧字符盖不掉，
/// 屏幕上留下一堆残影，看起来像「thinking 反复从头显示」。
pub(super) fn terminal_safe_text(value: &str) -> String {
    const TAB_WIDTH: usize = 4;
    let mut output = String::with_capacity(value.len());
    let mut column = 0;
    for character in value.chars() {
        match character {
            '\n' => {
                output.push('\n');
                column = 0;
            }
            '\t' => {
                let spaces = TAB_WIDTH - column % TAB_WIDTH;
                output.push_str(&" ".repeat(spaces));
                column += spaces;
            }
            '\r' => {}
            character if character.is_control() => {
                for escaped in character.escape_default() {
                    output.push(escaped);
                    column += UnicodeWidthChar::width(escaped).unwrap_or(0);
                }
            }
            character => {
                output.push(character);
                column += UnicodeWidthChar::width(character).unwrap_or(0);
            }
        }
    }
    output
}

pub(super) fn visual_lines(text: &str, width: usize) -> usize {
    let width = width.max(1);
    text.split('\n')
        .map(|line| {
            line.chars()
                .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
                .sum::<usize>()
                .max(1)
                .div_ceil(width)
        })
        .sum()
}

#[cfg(test)]
pub(super) fn transcript(messages: &[Message]) -> Vec<String> {
    projected_transcript(messages, None, Language::En)
}

/// 状态栏里的 token 计数：过千收敛成 K/M，保留两位小数。
///
/// 千以下原样输出——把 42 写成 `0.04K` 只会让人多算一次乘法。
pub(super) fn format_token_count(value: u64) -> String {
    match value {
        ..1_000 => value.to_string(),
        // 上界不是 1_000_000：999_999 四舍五入到两位小数就是 1000.00K，
        // 与其印一个比 M 还长的 K，不如让它直接进位。
        1_000..999_995 => format!("{:.2}K", value as f64 / 1_000.0),
        _ => format!("{:.2}M", value as f64 / 1_000_000.0),
    }
}

/// 提示词缓存命中率。Provider 没报缓存数就返回 `None`，状态栏据此整段隐藏，
/// 而不是显示一个像「真的一次没命中」的 0.00%。
pub(super) fn cache_hit_rate(usage: &willdeep_core::types::Usage) -> Option<f64> {
    let cached = usage.cache_read_tokens?;
    let input = usage.input_tokens.unwrap_or(0);
    if input == 0 {
        return None;
    }
    Some(cached as f64 / input as f64 * 100.0)
}

/// 首屏：吉祥物 + 产品名与版本。图形按实际聊天宽度适配，记录保持完整。
fn welcome_header() -> String {
    let art = banner::art_rows().join("\n");
    format!(
        "{art}\n\n{}\n",
        banner::name_line(concat!("v", env!("CARGO_PKG_VERSION")))
    )
}

/// 首屏欢迎语：吉祥物、产品名与版本，然后是一句问候。
///
/// 不再写会话 id 与续接命令——那两行退出时已经打印了（见 `run_tui` 收尾），
/// 首屏再抄一遍只是把真正的入口往后挤，用户每天开一次终端并不需要复制它。
pub(super) fn welcome_message(workspace: &std::path::Path, language: Language) -> String {
    let project = workspace
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(language.text("当前工作区", "current workspace", "現在のワークスペース"));
    let header = welcome_header();
    // 问候走默认助手记录行，显示时和别的回复一样换成实例名。
    match language {
        Language::ZhCn => format!(
            "{header}\n{ASSISTANT_ROW_PREFIX}你好，我已经进入 {project}。你可以直接告诉我想实现、修复或调查什么；我会先了解项目，再开始动手。输入 `/help` 查看全部命令，`/model` 切换模型。"
        ),
        Language::En => format!(
            "{header}\n{ASSISTANT_ROW_PREFIX}Hello, I’m in {project}. Tell me what you want to build, fix, or investigate; I’ll inspect the project before making changes. Type `/help` for every command, or `/model` to switch models."
        ),
        Language::Ja => format!(
            "{header}\n{ASSISTANT_ROW_PREFIX}こんにちは。{project} を開きました。実装、修正、調査したいことを教えてください。まずプロジェクトを確認してから作業します。`/help` で全コマンド、`/model` でモデルを切り替えられます。"
        ),
    }
}

#[cfg(test)]
mod code_block_tests {
    use super::*;

    #[test]
    fn code_blocks_report_their_wrapped_rows_and_source() {
        let row = "WillDeep: intro\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\nafter";
        let (text, blocks) = render_assistant_entry_with_code_blocks(row, None, 80, "");
        let rows = text_rows(&text);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content, "fn main() {\n    println!(\"hi\");\n}");
        let body = rows[blocks[0].start..blocks[0].end]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert_eq!(body, vec!["fn main() {", "    println!(\"hi\");", "}"]);
        assert_eq!(rows[blocks[0].start - 1], "WillDeep: intro");
        assert_eq!(rows[blocks[0].end], "after");
    }

    #[test]
    fn a_named_reply_keeps_markdown_newlines_code_blocks_and_highlights() {
        let row = "WillDeep: Status: done\n**bold** and `code`\n```rust\nlet status = \"Status: done\";\n```\nafter";
        let (text, blocks) =
            render_assistant_entry_with_code_blocks(row, Some("Status"), 80, "滚小滚");
        let rows = text_rows(&text);
        assert_eq!(
            rows,
            [
                "滚小滚: Status: done",
                "bold and code",
                "let status = \"Status: done\";",
                "after",
            ]
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!((blocks[0].start, blocks[0].end), (2, 3));
        assert_eq!(blocks[0].content, "let status = \"Status: done\";");
        assert!(text.lines[1].spans.iter().any(|span| {
            span.content == "bold" && span.style.add_modifier.contains(Modifier::BOLD)
        }));
        assert!(
            text.lines[0]
                .spans
                .iter()
                .any(|span| { span.content == "Status" && span.style.bg == Some(Color::Yellow) })
        );
        assert!(
            text.lines[2]
                .spans
                .iter()
                .any(|span| { span.content == "Status" && span.style.bg == Some(Color::Yellow) })
        );
    }

    /// 长代码行要在块内折行，块的行数按折行后的屏幕行算，复制内容仍是原始一行。
    #[test]
    fn long_code_lines_wrap_inside_the_block() {
        let long = "x".repeat(40);
        let row = format!("WillDeep: ```\n{long}\n```");
        let (text, blocks) = render_assistant_entry_with_code_blocks(&row, None, 10, "");
        let rows = text_rows(&text);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].start, 0);
        assert_eq!(blocks[0].end, rows.len());
        assert_eq!(rows[blocks[0].start..blocks[0].end].len(), 4);
        assert_eq!(blocks[0].content, long);
        assert!(
            rows.iter()
                .all(|row| UnicodeWidthStr::width(row.as_str()) <= 10)
        );
    }

    #[test]
    fn an_unclosed_fence_reports_the_body_to_the_end() {
        let row = "WillDeep: ```\nlet x = 1;";
        let (text, blocks) = render_assistant_entry_with_code_blocks(row, None, 80, "");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content, "let x = 1;");
        assert_eq!(blocks[0].start, 0);
        assert_eq!(blocks[0].end, text.lines.len());
    }

    #[test]
    fn every_block_in_a_message_is_reported_in_order() {
        let row = "WillDeep: ```\na\n```\ntext\n```\nb\n```";
        let (_, blocks) = render_assistant_entry_with_code_blocks(row, None, 80, "");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].content, "a");
        assert_eq!(blocks[1].content, "b");
        assert!(blocks[0].end <= blocks[1].start);
    }

    /// 时间标签另起一行且落在块内时，块尾要跟着加一，不能少高亮最后一行。
    #[test]
    fn a_time_label_inserted_inside_a_block_extends_its_range() {
        let rows = vec!["WillDeep: ```\nlet x = 1;\n\n```".to_owned()];
        let frame = message_time::render_with_user_folds(
            &rows,
            &[Some("now".to_owned())],
            None,
            12,
            Language::En,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &[],
        );
        assert_eq!(frame.code_blocks.len(), 1);
        assert_eq!(frame.code_blocks[0].start, 0);
        assert_eq!(frame.code_blocks[0].end, 3);
        assert_eq!(frame.code_blocks[0].content, "let x = 1;\n");
        let drawn = text_rows(&frame.text);
        assert_eq!(drawn.len(), 3, "{drawn:?}");
        assert_eq!(drawn[0], "let x = 1;");
        assert!(drawn[1].contains("now"), "{drawn:?}");
        assert_eq!(drawn[2], "");
    }

    #[test]
    fn insertions_shift_blocks_below_or_straddling_them() {
        let mut blocks = vec![
            CodeBlockLines {
                start: 1,
                end: 3,
                content: "a".to_owned(),
            },
            CodeBlockLines {
                start: 4,
                end: 5,
                content: "b".to_owned(),
            },
        ];
        shift_code_block_rows(&mut blocks, 2, 1);
        assert_eq!((blocks[0].start, blocks[0].end), (1, 4));
        assert_eq!((blocks[1].start, blocks[1].end), (5, 6));
    }
}
