use super::*;

/// Completed history is immutable during a text delta. Keep its styled,
/// wrapped rows instead of parsing Markdown twice for every token.
pub(super) struct TranscriptRenderCache {
    source: Vec<String>,
    width: usize,
    expanded: bool,
    language: Language,
    search: Option<String>,
    time_labels: Vec<Option<String>>,
    text: Text<'static>,
}

impl App {
    pub(super) fn prepare_transcript_frame(&mut self) -> Text<'static> {
        let time_labels =
            self.message_times
                .labels(self.transcript.len(), self.language, message_time::now());
        let search = self
            .search
            .as_ref()
            .map(|s| s.editor.text().trim().to_owned())
            .filter(|s| !s.is_empty());
        let valid = self.transcript_render_cache.as_ref().is_some_and(|cache| {
            cache.width == self.transcript_width
                && cache.expanded == self.tool_rows_expanded
                && cache.language == self.language
                && cache.search == search
                && cache.source == self.transcript
                && cache.time_labels == time_labels
        });
        if !valid {
            let folded = self.display_transcript();
            let labels = self.message_time_labels(&folded, message_time::now());
            let text = message_time::render_with_times(
                &folded.rows,
                &labels,
                search.as_deref(),
                self.transcript_width,
                self.language,
            );
            self.transcript_rows = text_rows(&text);
            self.transcript_render_cache = Some(TranscriptRenderCache {
                source: self.transcript.clone(),
                width: self.transcript_width,
                expanded: self.tool_rows_expanded,
                language: self.language,
                search: search.clone(),
                time_labels,
                text,
            });
        }
        let history = &self
            .transcript_render_cache
            .as_ref()
            .expect("prepared history")
            .text;
        self.transcript_rows.truncate(history.lines.len());
        let transient = self.transient_row().map_or_else(Text::default, |row| {
            wrap_styled_text(
                colored_transcript_at_width(&[row], search.as_deref(), self.transcript_width),
                self.transcript_width,
            )
        });
        self.transcript_rows.extend(text_rows(&transient));
        self.transcript_height = history.lines.len() + transient.lines.len();
        transient
    }

    pub(super) fn transcript_viewport(
        &self,
        transient: &Text<'static>,
        offset: usize,
    ) -> Text<'static> {
        let history = &self
            .transcript_render_cache
            .as_ref()
            .expect("prepared history")
            .text;
        // Skip history by index. Iterating from row zero would still make
        // every frame proportional to the complete conversation length.
        let history_start = offset.min(history.lines.len());
        let transient_start = offset.saturating_sub(history.lines.len());
        Text::from(
            history.lines[history_start..]
                .iter()
                .chain(transient.lines.iter().skip(transient_start))
                .take(self.viewport_height)
                .cloned()
                .collect::<Vec<_>>(),
        )
    }
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
pub(super) fn colored_transcript(
    entries: &[String],
    search_query: Option<&str>,
    width: usize,
    language: Language,
) -> Text<'static> {
    let last = entries.len().saturating_sub(1);
    let mut lines = Vec::new();
    for (index, raw) in entries.iter().enumerate() {
        // 「轮到你」只属于最后一行。逐行画的路径先统一拿掉，这里再补回来。
        let displayed = if index == last {
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
        if let Some(card) = decode_plan_entry(value) {
            lines.extend(render_plan(&card.plan, width));
            if card.expanded {
                for detail in card.details {
                    lines.extend(detail.lines().map(|line| {
                        Line::styled(line.to_owned(), Style::default().fg(Color::DarkGray))
                    }));
                }
            }
            continue;
        }
        if let Some(content) = value.strip_prefix("WillDeep: ") {
            lines.extend(render_assistant_markdown(content, width));
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
        lines.extend(
            value
                .lines()
                .map(|line| Line::styled(line.to_owned(), style)),
        );
        // 用户消息后面留一行空白，问答之间才分得开。分隔线本身已经是浅色，
        // 紧贴着上一轮输出画，不再额外空一行。
        if value.starts_with("You:") {
            lines.push(Line::default());
        }
    }
    let mut text = Text::from(lines);
    if let Some(query) = search_query {
        highlight_matches(&mut text, query);
    }
    text
}

fn thought_entry_parts(value: &str) -> Option<(&str, &str, bool)> {
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

    /// 分隔线紧贴上一轮输出：回复后面不再多空一行，分隔线后面也不再空一行。
    /// 用户消息后面的空白仍在，问答之间才分得开。
    #[test]
    fn the_turn_divider_sits_against_the_reply_it_closes() {
        let text = colored_transcript(
            &[
                "WillDeep: done".to_owned(),
                "── turn finished · total 1.0s ──".to_owned(),
                "You: next".to_owned(),
            ],
            None,
            80,
            Language::En,
        );
        let rows = text
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(rows[0], "WillDeep: done");
        assert!(rows[1].contains("turn finished"), "{rows:?}");
        assert_eq!(rows[2], "You: next");
        assert_eq!(rows.last().map(String::as_str), Some(""));
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
        let legacy = divider_without_your_turn("── turn finished · your turn ──", Language::En);
        assert_eq!(legacy, "── turn finished ──");
    }
}

fn highlight_matches(text: &mut Text<'static>, query: &str) {
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
    let width = width.max(1);
    let mut rows = Vec::new();
    for source in text.lines {
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
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        if cells.is_empty() {
            rows.push(Line::default());
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
            while index < cells.len() && !cells[index].character.is_whitespace() {
                index += 1;
            }
            let word = &cells[word_start..index];
            let whitespace_width = whitespace.iter().map(|cell| cell.width).sum::<usize>();
            let word_width = word.iter().map(|cell| cell.width).sum::<usize>();
            if word_width <= width {
                if line_width > 0
                    && line_width
                        .saturating_add(whitespace_width)
                        .saturating_add(word_width)
                        > width
                {
                    rows.push(Line::from(std::mem::take(&mut line)));
                    line_width = 0;
                    whitespace.clear();
                }
                append_styled_cells(&mut line, &whitespace);
                line_width = line_width.saturating_add(whitespace_width);
                whitespace.clear();
                append_styled_cells(&mut line, word);
                line_width = line_width.saturating_add(word_width);
                continue;
            }
            if line_width > 0 && line_width.saturating_add(whitespace_width) >= width {
                rows.push(Line::from(std::mem::take(&mut line)));
                line_width = 0;
                whitespace.clear();
            }
            for cell in whitespace.drain(..).chain(word.iter().copied()) {
                if line_width > 0 && line_width.saturating_add(cell.width) > width {
                    rows.push(Line::from(std::mem::take(&mut line)));
                    line_width = 0;
                }
                push_styled_character(&mut line, cell.character, cell.style);
                line_width = line_width.saturating_add(cell.width);
            }
        }
        for cell in whitespace {
            if line_width > 0 && line_width.saturating_add(cell.width) > width {
                rows.push(Line::from(std::mem::take(&mut line)));
                line_width = 0;
            }
            push_styled_character(&mut line, cell.character, cell.style);
            line_width = line_width.saturating_add(cell.width);
        }
        if !line.is_empty() {
            rows.push(Line::from(line));
        }
    }
    Text::from(rows)
}

#[derive(Clone, Copy)]
struct StyledCharacter {
    character: char,
    style: Style,
    width: usize,
}

fn append_styled_cells(spans: &mut Vec<Span<'static>>, cells: &[StyledCharacter]) {
    for cell in cells {
        push_styled_character(spans, cell.character, cell.style);
    }
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

pub(super) fn selected_text(rows: &[String], start: (usize, usize), end: (usize, usize)) -> String {
    if rows.is_empty() || start >= end {
        return String::new();
    }
    let last_row = end.0.min(rows.len().saturating_sub(1));
    (start.0..=last_row)
        .map(|row| {
            let start_column = if row == start.0 { start.1 } else { 0 };
            let end_column = if row == end.0 { end.1 } else { usize::MAX };
            display_column_slice(&rows[row], start_column, end_column)
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
    let mut output = Vec::new();
    let mut code_block = false;
    let lines = content.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        let raw = lines[index];
        if raw.trim_start().starts_with("```") {
            code_block = !code_block;
            index += 1;
            continue;
        }
        if code_block {
            let mut spans = assistant_prefix(index == 0);
            spans.push(Span::styled(
                raw.to_owned(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            output.push(Line::from(spans));
            index += 1;
            continue;
        }

        if index + 1 < lines.len()
            && parse_table_row(raw).is_some()
            && is_table_separator(lines[index + 1])
        {
            let mut rows = vec![parse_table_row(raw).expect("table header was checked")];
            index += 2;
            while index < lines.len() {
                let Some(row) = parse_table_row(lines[index]) else {
                    break;
                };
                rows.push(row);
                index += 1;
            }
            if output.is_empty() {
                output.push(Line::from(assistant_prefix(true)));
            }
            output.extend(render_markdown_table(&rows, width));
            continue;
        }

        let normalized = normalize_html_breaks(raw);
        for (piece_index, piece) in normalized.split('\n').enumerate() {
            output.push(render_markdown_line(piece, index == 0 && piece_index == 0));
        }
        index += 1;
    }
    if output.is_empty() {
        output.push(Line::styled("WillDeep:", Style::default().fg(Color::Green)));
    }
    output
}

fn assistant_prefix(show: bool) -> Vec<Span<'static>> {
    show.then(|| {
        Span::styled(
            "WillDeep: ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    })
    .into_iter()
    .collect()
}

fn render_markdown_line(raw: &str, show_prefix: bool) -> Line<'static> {
    let mut spans = assistant_prefix(show_prefix);
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
    let label = your_turn_label(language);
    let body = body.strip_suffix(&format!(" · {label}")).unwrap_or(body);
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

const WILLDEEP_BANNER: &str = "█   █ █████ █     █     ████  █████ █████ ████\n█   █   █   █     █     █   █ █     █     █   █\n█ █ █   █   █     █     █   █ ████  ████  ████\n██ ██   █   █     █     █   █ █     █     █\n█   █ █████ █████ █████ ████  █████ █████ █";

pub(super) fn welcome_message(
    workspace: &std::path::Path,
    session_id: uuid::Uuid,
    language: Language,
) -> String {
    let project = workspace
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(language.text("当前工作区", "current workspace", "現在のワークスペース"));
    match language {
        Language::ZhCn => format!(
            "{WILLDEEP_BANNER}\n\nSession: {session_id}\nContinue: willdeep -s {session_id}\n\nWillDeep: 你好，我已经进入 {project}。你可以直接告诉我想实现、修复或调查什么；我会先了解项目，再开始动手。输入 `/help` 查看全部命令，`/model` 切换模型。"
        ),
        Language::En => format!(
            "{WILLDEEP_BANNER}\n\nSession: {session_id}\nContinue: willdeep -s {session_id}\n\nWillDeep: Hello, I’m in {project}. Tell me what you want to build, fix, or investigate; I’ll inspect the project before making changes. Type `/help` for every command, or `/model` to switch models."
        ),
        Language::Ja => format!(
            "{WILLDEEP_BANNER}\n\nSession: {session_id}\nContinue: willdeep -s {session_id}\n\nWillDeep: こんにちは。{project} を開きました。実装、修正、調査したいことを教えてください。まずプロジェクトを確認してから作業します。`/help` で全コマンド、`/model` でモデルを切り替えられます。"
        ),
    }
}
