use super::narration::FoldedTranscript;
use super::*;

pub(super) const PREVIEW_LINES: usize = 8;

// 已发正文为青色，待发为黄色；低对比背景与复制坐标不随状态改变。
pub(super) const BLOCK_BG: Color = Color::Rgb(36, 48, 62);
pub(super) const SENT_FG: Color = Color::Cyan;
pub(super) const QUEUED_FG: Color = Color::Yellow;
const MAX_BLOCK_WIDTH: usize = 100;
const NARROW_VIEWPORT_WIDTH: usize = 20;
const BLOCK_WIDTH_NUMERATOR: usize = 4;
const BLOCK_WIDTH_DENOMINATOR: usize = 5;
const HORIZONTAL_PADDING: usize = 1;

pub(super) fn queue_hint(language: Language) -> &'static str {
    language.text(
        "Ctrl+Q 管理待发队列",
        "Ctrl+Q Manage queued messages",
        "Ctrl+Q 送信待ちキューを管理",
    )
}

fn block_width(width: usize) -> usize {
    let width = width.max(1);
    if width < NARROW_VIEWPORT_WIDTH {
        width
    } else {
        (width / BLOCK_WIDTH_DENOMINATOR * BLOCK_WIDTH_NUMERATOR).min(MAX_BLOCK_WIDTH)
    }
}

fn block_padding(width: usize) -> usize {
    usize::from(width >= NARROW_VIEWPORT_WIDTH) * HORIZONTAL_PADDING
}

pub(super) fn content_width(width: usize) -> usize {
    block_width(width)
        .saturating_sub(2 * block_padding(width))
        .max(1)
}

/// 只移除记录生成的角色前缀；用户正文不再经过角色或 Markdown 解析。
pub(super) fn render_body(
    row: &str,
    search: Option<&str>,
    width: usize,
    id: rendering::source::SourceId,
    queued: bool,
) -> (Text<'static>, Vec<rendering::source::SourceRow>) {
    let body = row.strip_prefix("You: ").unwrap_or(row);
    let color = if queued { QUEUED_FG } else { SENT_FG };
    let mut text = Text::from(
        body.split('\n')
            .map(|line| Line::styled(line.to_owned(), Style::default().fg(color)))
            .collect::<Vec<_>>(),
    );
    if let Some(query) = search {
        rendering::highlight_matches(&mut text, query);
    }
    rendering::source::wrap_literal(text, id, rendering::source::SourcePart::Body, body, width)
}

/// 消息块靠右、块内各行左对齐。缩进是透明的，只有正文和左右内边距带背景。
/// 不添加样式行，折叠按钮和附件标签的纵向命中位置保持不变。
pub(super) fn style_block(entry: &mut Text<'static>, width: usize) -> Vec<std::ops::Range<usize>> {
    let width = width.max(1);
    let padding = block_padding(width);
    let body_width = entry.lines.iter().map(Line::width).max().unwrap_or(0);
    let block_width = body_width.saturating_add(2 * padding).min(width);
    let indent = width.saturating_sub(block_width);
    let mut content_bounds = Vec::with_capacity(entry.lines.len());
    for line in &mut entry.lines {
        let start = indent + padding;
        content_bounds.push(start..start + line.width());
        let trailing = block_width.saturating_sub(padding + line.width());
        let base = line.style.bg(BLOCK_BG);
        let mut spans = vec![
            Span::raw(" ".repeat(indent)),
            Span::styled(" ".repeat(padding), base),
        ];
        spans.extend(
            std::mem::take(&mut line.spans)
                .into_iter()
                .map(|span| Span::styled(span.content, base.patch(span.style))),
        );
        spans.push(Span::styled(" ".repeat(trailing), base));
        *line = Line::from(spans);
    }
    content_bounds
}

/// Tint only the identity-bound receipt cell. Never recognize status from a
/// user's text, recolor its body, or change provenance/selection coordinates.
fn style_delivery_symbol(line: &mut Line<'static>, column: usize, color: Color) {
    let mut offset = 0;
    for index in 0..line.spans.len() {
        let span = &line.spans[index];
        if (offset..offset + span.width()).contains(&column) {
            let Some((byte, character)) = span.content.char_indices().find(|(byte, _)| {
                UnicodeWidthStr::width(&span.content[..*byte]) == column - offset
            }) else {
                return;
            };
            let end = byte + character.len_utf8();
            let span = line.spans.remove(index);
            let parts = [
                Span::styled(span.content[..byte].to_owned(), span.style),
                Span::styled(
                    span.content[byte..end].to_owned(),
                    span.style.fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(span.content[end..].to_owned(), span.style),
            ];
            line.spans.splice(index..index, parts);
            return;
        }
        offset += span.width();
    }
}

impl App {
    /// 普通 Enter 的本地反馈先准备好，调用方立刻绘制，再执行命令或提交 Runtime。
    pub(super) fn take_prompt_for_submission(&mut self) -> String {
        let prompt = self.take_submitted_input();
        if !narration::is_display_only_command(&prompt) {
            self.append_transcript(format!("You: {prompt}"));
            // 斜杠命令不是发给模型的消息，附件还留在草稿里，不挂到这一行上。
            if !prompt.trim_start().starts_with('/') {
                let attachments = self
                    .attachments
                    .iter()
                    .map(|item| item.message.clone())
                    .collect();
                self.attach_to_latest_user_row(attachments);
            }
            // 这句已经在屏幕上，模型还没开口：下面那行开始转 `.` `..` `...`。
            self.note_awaiting_reply();
            self.note_prompt_submitted(&prompt);
            self.scroll_to_bottom();
        }
        prompt
    }

    /// Only identity-bound Runtime receipts or explicit local queue entries
    /// can add queue affordances; an unconfirmed submission proves no queue.
    pub(super) fn queued_user_records(&self) -> BTreeSet<usize> {
        self.pending_runtime_echoes
            .iter()
            .filter(|echo| echo.status.is_queued())
            .filter_map(|echo| echo.record)
            .chain(
                self.queued_prompts
                    .iter()
                    .filter(|queued| queued.remote.is_none())
                    .filter_map(|queued| queued.transcript_row),
            )
            .collect()
    }

    pub(super) fn render_user_transcript(
        &self,
        folded: &FoldedTranscript,
        labels: &[Option<String>],
        search: Option<&str>,
        end: usize,
    ) -> message_time::FoldedFrame {
        let floor = self.archive_floor.min(self.transcript.len());
        let shift = usize::from(floor > 0);
        let users: BTreeMap<_, _> = self
            .transcript
            .iter()
            .enumerate()
            .skip(floor)
            .filter(|(_, row)| row.starts_with("You: "))
            .filter_map(|(source, row)| {
                let target = *folded.index_of.get(source - floor + shift)?;
                let expanded = self.expanded_user_messages.get(&source) == Some(row);
                Some((target, (source, expanded)))
            })
            .collect();
        // 点开的工具行按记录位置认。折叠视图里同一行可能对应好几条记录，
        // 只有用户点开的那条才展开，其余的照旧收着。
        let tools = self
            .transcript
            .iter()
            .enumerate()
            .skip(floor)
            .filter(|(_, row)| narration::is_tool_display_row(row))
            .filter_map(|(source, _)| {
                let target = *folded.index_of.get(source - floor + shift)?;
                if folded.sources.get(target) != Some(&Some(source)) {
                    return None;
                }
                // 行本身一直可点，但完整命令只在点开之后才画出来。
                // 记下的展开内容不管展开状态，收起时照样留着，下次点开还在。
                let expansion = self
                    .tool_row_open(source)
                    .then(|| self.tool_expansions.get(&source).cloned())
                    .flatten();
                Some((
                    target,
                    (source, expansion, self.tool_visuals.is_diff(source)),
                ))
            })
            .collect();
        let queued_records = self.queued_user_records();
        let queued_users = users
            .iter()
            .filter(|(_, (source, _))| queued_records.contains(source))
            .map(|(&target, _)| target)
            .collect();
        // Local queue prefixes remain internal bookkeeping. Display and copy
        // the identity-bound body instead of treating generated labels as text.
        let mut displayed = std::borrow::Cow::Borrowed(&folded.rows[..end]);
        for queued in self
            .queued_prompts
            .iter()
            .filter(|item| item.remote.is_none())
        {
            if let Some(source) = queued.transcript_row
                && let Some((&target, _)) = users.iter().find(|(_, (record, _))| *record == source)
                && target < end
            {
                displayed.to_mut()[target] = format!("You: {}", terminal_safe_text(&queued.text));
            }
        }
        let mut frame = message_time::render_transcript(
            &displayed,
            &labels[..end],
            search,
            self.transcript_width,
            self.language,
            &users,
            &tools,
            &self.transcript_attachments,
            &self.assistant_name(),
            self.ui.line_spacing,
            &queued_users,
        );
        use rendering::source::{SourceId, SourcePart};
        // Folded indices are layout positions. Translate them through index_of,
        // then prepare_transcript_frame translates original indices to stable IDs.
        let mut representatives = BTreeMap::new();
        let mut exact = BTreeSet::new();
        for (target, source) in folded.sources.iter().take(end).enumerate() {
            if let Some(source) = source {
                representatives.insert(target, *source);
                exact.insert(target);
            }
        }
        for source in floor..self.transcript.len() {
            if let Some(&target) = folded.index_of.get(source - floor + shift)
                && target < end
            {
                representatives.entry(target).or_insert(source);
            }
        }
        let mut ids = BTreeMap::new();
        for target in 0..end {
            let id = if target == 0 && floor > 0 {
                SourceId::Archive
            } else {
                representatives
                    .get(&target)
                    .map_or(SourceId::Archive, |source| SourceId::Record(*source))
            };
            ids.insert(SourceId::Record(target), id);
            if !exact.contains(&target) {
                for row in &mut frame.sources.rows {
                    if row.fallback.id == SourceId::Record(target) {
                        row.cells.clear();
                        row.fallback.part = SourcePart::Control;
                    }
                }
            }
        }
        frame.sources.rebase(&ids);
        // Status prefixes belong to a receipt, not the user's original body.
        // Their byte length can change from queued to started without moving it.
        for echo in &self.pending_runtime_echoes {
            let Some(record) = echo.record else { continue };
            let Some(raw) = self.transcript.get(record) else {
                continue;
            };
            let body = terminal_safe_text(&echo.text);
            let prefix = rendering::source::record_body(raw)
                .len()
                .saturating_sub(body.len());
            let id = SourceId::Record(record);
            for (line, row) in frame.text.lines.iter_mut().zip(&mut frame.sources.rows) {
                if row.fallback.id != id {
                    continue;
                }
                if !echo.status.symbol().is_empty()
                    && let Some(cell) = row.cells.iter().find(|cell| {
                        cell.source.point.part == SourcePart::Body && cell.source.point.byte == 0
                    })
                {
                    style_delivery_symbol(line, cell.columns.start, echo.status.color());
                }
                row.cells.retain_mut(|cell| {
                    if cell.source.point.part != SourcePart::Body {
                        return true;
                    }
                    if cell.source.point.byte < prefix {
                        return false;
                    }
                    cell.source.point.byte -= prefix;
                    cell.source.end -= prefix;
                    true
                });
                if row.fallback.part == SourcePart::Body {
                    row.fallback.byte = row.fallback.byte.saturating_sub(prefix);
                }
            }
            frame.sources.parts.insert((id, SourcePart::Body), body);
        }
        for source in 0..floor {
            frame
                .sources
                .aliases
                .insert(SourceId::Record(source), SourceId::Archive);
        }
        for source in floor..self.transcript.len() {
            if let Some(target) = folded.index_of.get(source - floor + shift)
                && let Some(id) = ids.get(&SourceId::Record(*target))
                && *id != SourceId::Record(source)
            {
                frame.sources.aliases.insert(SourceId::Record(source), *id);
            }
        }
        frame
    }

    pub(super) fn update_user_message_hits(&mut self, offset: usize, top: u16) {
        self.update_thought_hits(offset, top);
        self.user_message_hit_rows = self
            .user_message_controls
            .iter()
            .filter(|(line, _)| *line >= offset && *line < offset + self.viewport_height)
            .map(|(line, source)| (top.saturating_add((line - offset) as u16), *source))
            .collect();
        self.tool_row_hits = self
            .tool_row_controls
            .iter()
            .filter(|(line, _, _)| *line >= offset && *line < offset + self.viewport_height)
            .map(|(line, source, collapse)| {
                (
                    top.saturating_add((line - offset) as u16),
                    *source,
                    *collapse,
                )
            })
            .collect();
        self.attachment_hit_rows = self
            .attachment_controls
            .iter()
            .filter(|(line, _, _)| *line >= offset && *line < offset + self.viewport_height)
            .map(|(line, source, attachment)| {
                (
                    top.saturating_add((line - offset) as u16),
                    *source,
                    *attachment,
                )
            })
            .collect();
    }

    /// 点在某一行的附件标签上：文本附件打开预览，图片交给图片查看器。
    /// 返回 `None` 表示这一行不是附件标签。
    pub(super) fn open_attachment_at(&mut self, y: u16) -> Option<MediaAction> {
        let (_, source, attachment_index) = self
            .attachment_hit_rows
            .iter()
            .find(|(line, _, _)| *line == y)
            .copied()?;
        let attachment = self
            .transcript_attachments
            .get(source)?
            .get(attachment_index)?;
        match attachment {
            MessageAttachment::Text { name, content } => {
                self.open_text_attachment(name.clone(), content.clone());
                Some(MediaAction::None)
            }
            MessageAttachment::Image {
                name,
                media_type,
                data,
                ..
            } => {
                let target = format!("data:{media_type};base64,{data}");
                Some(self.media.open_attachment(name.clone(), target))
            }
        }
    }

    pub(super) fn toggle_user_message_at(&mut self, y: u16) -> bool {
        let Some((_, source)) = self
            .user_message_hit_rows
            .iter()
            .find(|(line, _)| *line == y)
        else {
            return false;
        };
        let source = *source;
        let Some(row) = self
            .transcript
            .get(source)
            .filter(|row| row.starts_with("You: "))
        else {
            return false;
        };
        let collapsing = self.expanded_user_messages.get(&source) == Some(row);
        if collapsing {
            self.expanded_user_messages.remove(&source);
        } else {
            self.expanded_user_messages.insert(source, row.clone());
        }
        self.transcript_render_cache = None;
        self.user_message_hit_rows.clear();
        self.follow_bottom = false;
        // Preserve the old frame's source; prepare_transcript_frame resolves it
        // after folding, falling back to this message's title if it is hidden.
        self.refresh_transcript_height();
        true
    }
}

#[cfg(test)]
#[path = "user_message_layout_tests.rs"]
mod layout_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_submission_prepares_the_visible_message_and_clears_the_composer() {
        let mut app = App::new(Vec::new(), Language::En);
        app.input.insert("show this before the Runtime responds");
        app.composer_expanded = true;
        app.follow_bottom = false;
        let prompt = app.take_prompt_for_submission();
        assert_eq!(prompt, "show this before the Runtime responds");
        assert!(app.input.is_empty());
        assert!(!app.composer_expanded);
        assert!(app.follow_bottom);
        assert_eq!(
            app.transcript.last().unwrap(),
            "You: show this before the Runtime responds"
        );
        assert_eq!(
            app.prompt_history_pending.back().map(String::as_str),
            Some(prompt.as_str())
        );
    }

    #[test]
    fn model_command_consumes_input_without_echoing_it_as_a_user_message() {
        let mut app = App::new(Vec::new(), Language::En);
        app.input.insert("/model selected-model");
        assert_eq!(app.take_prompt_for_submission(), "/model selected-model");
        assert!(app.input.is_empty());
        assert!(app.transcript.is_empty());
    }
}
