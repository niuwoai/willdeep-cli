use ratatui::{style::Style, text::Span};
use std::ops::Range;
use unicode_width::UnicodeWidthStr;
use willdeep_core::MessageAttachment;

fn clean_terminal_controls(value: &str) -> String {
    let mut cleaned = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        let control = if character == '\u{1b}' {
            // ESC sequence bytes are printable ASCII. Leave a following newline or
            // Unicode character intact if a fragment ends with a stray Esc.
            chars
                .next_if(|next| ('\u{20}'..='\u{7e}').contains(next))
                .map(|next| match next {
                    '[' => '\u{9b}',
                    ']' => '\u{9d}',
                    'P' | 'X' | '^' | '_' => '\u{90}',
                    '\u{20}'..='\u{2f}' => {
                        while chars
                            .peek()
                            .is_some_and(|c| ('\u{20}'..='\u{2f}').contains(c))
                        {
                            chars.next();
                        }
                        if chars
                            .peek()
                            .is_some_and(|c| ('\u{30}'..='\u{7e}').contains(c))
                        {
                            chars.next();
                        }
                        '\u{1b}'
                    }
                    _ => '\u{1b}',
                })
                .unwrap_or('\u{1b}')
        } else {
            character
        };
        match control {
            '\u{9b}' => {
                while let Some(next) = chars.next() {
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                }
            }
            '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => {
                while let Some(next) = chars.next() {
                    if next == '\u{9c}' || (control == '\u{9d}' && next == '\u{7}') {
                        break;
                    }
                    if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                cleaned.push('\n');
            }
            '\n' | '\t' => cleaned.push(control),
            other if !other.is_control() => cleaned.push(other),
            _ => {}
        }
    }
    cleaned
}

#[derive(Clone, Copy)]
struct PromptSelection {
    anchor: usize,
    head: usize,
}

#[derive(Clone, Default)]
pub struct PromptEditor {
    text: String,
    cursor: usize,
    selection: Option<PromptSelection>,
    selection_dragging: bool,
}

impl PromptEditor {
    /// Load existing content verbatim. New keyboard and paste input still goes through `insert`.
    pub(crate) fn with_existing_text(text: String) -> Self {
        Self {
            cursor: text.len(),
            text,
            ..Self::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
    pub fn take(&mut self) -> String {
        self.clear_selection();
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }
    pub fn insert(&mut self, value: &str) {
        self.clear_selection();
        let value = clean_terminal_controls(value);
        self.text.insert_str(self.cursor, &value);
        self.cursor += value.len();
    }
    pub fn backspace(&mut self) {
        self.clear_selection();
        if let Some((index, _)) = self.text[..self.cursor].char_indices().next_back() {
            self.text.replace_range(index..self.cursor, "");
            self.cursor = index;
        }
    }
    pub fn delete(&mut self) {
        self.clear_selection();
        if let Some(character) = self.text[self.cursor..].chars().next() {
            self.text
                .replace_range(self.cursor..self.cursor + character.len_utf8(), "");
        }
    }
    pub fn left(&mut self) {
        self.clear_selection();
        if let Some((index, _)) = self.text[..self.cursor].char_indices().next_back() {
            self.cursor = index;
        }
    }
    pub fn right(&mut self) {
        self.clear_selection();
        if let Some(character) = self.text[self.cursor..].chars().next() {
            self.cursor += character.len_utf8();
        }
    }
    pub fn home(&mut self) {
        self.clear_selection();
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
    }
    pub fn end(&mut self) {
        self.clear_selection();
        self.cursor += self.text[self.cursor..]
            .find('\n')
            .unwrap_or(self.text.len() - self.cursor);
    }
    pub fn up_visual(&mut self, width: usize) {
        self.clear_selection();
        let (row, column) = self.cursor_visual(width);
        if row > 0 {
            self.set_cursor_visual(row - 1, column, width);
        }
    }
    pub fn down_visual(&mut self, width: usize) {
        let (row, column) = self.cursor_visual(width);
        self.set_cursor_visual(row + 1, column, width);
    }
    pub fn cursor_visual(&self, width: usize) -> (usize, usize) {
        visual_position(&self.text, self.cursor, width)
    }
    pub fn cursor_visual_with_prefix(&self, prefix: &str, width: usize) -> (usize, usize) {
        let text = format!("{prefix}{}", self.text);
        visual_position(&text, prefix.len() + self.cursor, width)
    }
    pub fn wrapped_text(&self, width: usize) -> String {
        visual_rows(&self.text, width).join("\n")
    }
    pub fn visual_line_count(&self, width: usize) -> usize {
        visual_rows(&self.text, width).len()
    }
    pub fn marker_query(&self, marker: char) -> Option<(usize, &str)> {
        let prefix = &self.text[..self.cursor];
        let start = prefix
            .char_indices()
            .rev()
            .find_map(|(index, character)| {
                character
                    .is_whitespace()
                    .then_some(index + character.len_utf8())
            })
            .unwrap_or(0);
        let token = &prefix[start..];
        let query = token.strip_prefix(marker)?;
        query
            .chars()
            .all(|character| {
                if marker == '@' {
                    !character.is_whitespace() && !matches!(character, '@' | '"')
                } else {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
                }
            })
            .then_some((start, query))
    }
    pub fn replace_before_cursor(&mut self, start: usize, value: &str) {
        self.clear_selection();
        if start <= self.cursor && self.text.is_char_boundary(start) {
            self.text.replace_range(start..self.cursor, value);
            self.cursor = start + value.len();
        }
    }
    pub fn set_cursor_visual(&mut self, target_row: usize, target_column: usize, width: usize) {
        self.clear_selection();
        self.cursor = byte_at_visual(&self.text, target_row, target_column, width);
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection = None;
        self.selection_dragging = false;
    }

    pub(crate) fn selection_active(&self) -> bool {
        self.selection.is_some()
    }

    pub(crate) fn selection_dragging(&self) -> bool {
        self.selection_dragging
    }

    /// 选区是原文的半开字节范围，鼠标落点对齐终端渲染的完整字素。
    pub(crate) fn begin_selection_visual(&mut self, row: usize, column: usize, width: usize) {
        self.set_cursor_visual(row, column, width);
        self.selection = Some(PromptSelection {
            anchor: self.cursor,
            head: self.cursor,
        });
        self.selection_dragging = true;
    }

    pub(crate) fn extend_selection_visual(&mut self, row: usize, column: usize, width: usize) {
        if let Some(selection) = self.selection.as_mut() {
            selection.head = byte_at_visual(&self.text, row, column, width);
        }
    }

    pub(crate) fn finish_selection(&mut self) {
        self.selection_dragging = false;
        if self.selected_text().is_none() {
            self.clear_selection();
        }
    }

    pub(crate) fn selected_text(&self) -> Option<&str> {
        let selection = self.selection?;
        let start = selection.anchor.min(selection.head);
        let end = selection.anchor.max(selection.head);
        (start < end).then(|| &self.text[start..end])
    }

    /// 高亮与鼠标命中共享折行边界；显式换行多画一格，空行也看得见。
    pub(crate) fn selection_visual_ranges(&self, width: usize) -> Vec<(usize, Range<usize>)> {
        let Some(selection) = self.selection else {
            return Vec::new();
        };
        let start = selection.anchor.min(selection.head);
        let end = selection.anchor.max(selection.head);
        let mut selected = Vec::new();
        for (row, bytes) in visual_row_ranges(&self.text, width).into_iter().enumerate() {
            let mut column = 0;
            let mut columns: Option<Range<usize>> = None;
            for (glyph, size) in visual_glyphs(&self.text[bytes.clone()]) {
                if (start..end).contains(&(bytes.start + glyph.start)) && size > 0 {
                    columns.get_or_insert(column..column).end = column + size;
                }
                column += size;
            }
            if (start..end).contains(&bytes.end)
                && self.text.as_bytes().get(bytes.end) == Some(&b'\n')
            {
                columns.get_or_insert(column..column).end = column + 1;
            }
            if let Some(columns) = columns {
                selected.push((row, columns));
            }
        }
        selected
    }
}

fn visual_position(text: &str, cursor: usize, width: usize) -> (usize, usize) {
    let rows = visual_row_ranges(text, width);
    // 软折行边界属于下一行；显式换行则位于上一行的末尾。
    let row = rows
        .iter()
        .rposition(|range| range.start <= cursor && cursor <= range.end)
        .unwrap_or(0);
    let column = UnicodeWidthStr::width(&text[rows[row].start..cursor]);
    (row, column)
}

fn byte_at_visual(text: &str, target_row: usize, target_column: usize, width: usize) -> usize {
    let rows = visual_row_ranges(text, width);
    let Some(bytes) = rows.get(target_row) else {
        return text.len();
    };
    let mut column = 0;
    for (glyph, size) in visual_glyphs(&text[bytes.clone()]) {
        if column >= target_column {
            return bytes.start + glyph.start;
        }
        column += size;
    }
    bytes.end
}

fn visual_rows(text: &str, width: usize) -> Vec<String> {
    visual_row_ranges(text, width)
        .into_iter()
        .map(|range| text[range].to_owned())
        .collect()
}

/// 每行对应的原文范围，不把软折行变成原文换行，也不丢弃空行与空格。
fn visual_row_ranges(text: &str, width: usize) -> Vec<Range<usize>> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut start = 0;
    let mut column = 0;
    for (glyph, size) in visual_glyphs(text) {
        if &text[glyph.clone()] == "\n" {
            // An exact-width soft wrap already emitted this physical line.
            // Consume its newline without creating an additional empty row.
            if start != glyph.start
                || rows
                    .last()
                    .is_none_or(|row: &Range<usize>| row.end != glyph.start)
            {
                rows.push(start..glyph.start);
            }
            start = glyph.end;
            column = 0;
            continue;
        }
        if column + size > width {
            rows.push(start..glyph.start);
            start = glyph.start;
            column = 0;
        }
        column += size;
        if column >= width {
            rows.push(start..glyph.end);
            start = glyph.end;
            column = 0;
        }
    }
    rows.push(start..text.len());
    rows
}

/// 复用已有 Ratatui 的字素切分，不额外引入依赖；ZWJ emoji、肤色和组合音标
/// 必须与最终绘制使用相同的边界和宽度，不能把一个显示字符拆成多个格子。
fn visual_glyphs(text: &str) -> Vec<(Range<usize>, usize)> {
    let mut glyphs = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let span = Span::raw(body);
        for glyph in span.styled_graphemes(Style::default()) {
            let end = offset + glyph.symbol.len();
            glyphs.push((offset..end, UnicodeWidthStr::width(glyph.symbol)));
            offset = end;
        }
        if line.ends_with('\n') {
            glyphs.push((offset..offset + 1, 0));
            offset += 1;
        }
    }
    glyphs
}

#[derive(Clone)]
pub struct DraftAttachment {
    pub id: uuid::Uuid,
    pub message: MessageAttachment,
}
impl DraftAttachment {
    pub fn summary(&self) -> String {
        match &self.message {
            // 带一截正文预览：剪贴板里装的到底是不是想贴的东西，发送前就该看得出来。
            // 出过一回事——拖选聊天区自动复制顶掉了刚复制的网址，条目只写
            // 「3 lines · 146 B」，谁也看不出贴进去的是上一条回复。
            MessageAttachment::Text { content, name } => format!(
                "Pasted text · {} lines · {} · {name} · 「{}」",
                content.lines().count().max(1),
                human_bytes(content.len()),
                text_preview(content)
            ),
            MessageAttachment::Image {
                name,
                width,
                height,
                data,
                ..
            } => format!(
                "{name} · {width}×{height} · {}",
                human_bytes(data.len() * 3 / 4)
            ),
        }
    }
}
/// 正文里第一行有字的内容，截到 [`PREVIEW_CHARS`] 个字符；截断了就加省略号。
const PREVIEW_CHARS: usize = 36;
fn text_preview(content: &str) -> String {
    let line = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let mut preview = line.chars().take(PREVIEW_CHARS).collect::<String>();
    if line.chars().count() > PREVIEW_CHARS {
        preview.push('…');
    }
    preview
}

fn human_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 附件条目要露出正文首行：贴错了东西发送前就能看见。
    #[test]
    fn pasted_text_summary_previews_the_first_non_empty_line() {
        let attachment = DraftAttachment {
            id: uuid::Uuid::new_v4(),
            message: MessageAttachment::Text {
                name: "paste-1.txt".to_owned(),
                content: "\nscripts/tsing_hub_probe.rb 是现成脚手架，可以加一组 effort 探针在 beta 环境跑。\n\n· 首答 9.11s".to_owned(),
            },
        };
        let summary = attachment.summary();
        assert!(summary.starts_with("Pasted text · 4 lines · "), "{summary}");
        assert!(
            summary.ends_with("「scripts/tsing_hub_probe.rb 是现成脚手架，可以…」"),
            "{summary}"
        );

        let short = text_preview("  https://hub-beta.example.com/v1  ");
        assert_eq!(short, "https://hub-beta.example.com/v1");
        assert_eq!(text_preview("\n\n"), "");
    }

    #[test]
    fn input_strips_terminal_sequences_and_keeps_unicode_editing() {
        let mut editor = PromptEditor::default();
        editor.insert("中文\u{1b}[31m红色\u{1b}[0m\r\n\t🙂\0\u{7}\u{8}\u{b}\u{c}\u{7f}");
        assert_eq!(editor.text(), "中文红色\n\t🙂");
        editor.left();
        editor.insert("\u{1b}]0;title\u{7}中\u{1b}Ppayload\u{1b}\\文");
        assert_eq!(editor.text(), "中文红色\n\t中文🙂");
        editor.backspace();
        editor.delete();
        assert_eq!(editor.text(), "中文红色\n\t中");
    }

    #[test]
    fn input_drops_c1_controls_and_unterminated_escape_payloads() {
        for (raw, expected) in [
            ("a\u{9b}31mb\u{9b}0m", "ab"),
            ("a\u{9d}title\u{9c}b", "ab"),
            ("a\u{1b}_payload\u{1b}\\b", "ab"),
            ("a\u{1b}(Bb", "ab"),
            ("a\u{1b}[123", "a"),
            ("a\u{1b}]title", "a"),
            ("a\u{1b}", "a"),
            ("a\u{1b}中\u{1b}\nb", "a中\nb"),
            ("a\u{1b}[<35;49;20Mb", "ab"),
            ("a\r\nb\rc", "a\nb\nc"),
            (r"literal \\x1b[31m", r"literal \\x1b[31m"),
        ] {
            let mut editor = PromptEditor::default();
            editor.insert(raw);
            assert_eq!(editor.text(), expected, "{raw:?}");
            editor.insert("尾");
            assert_eq!(editor.text(), format!("{expected}尾"));
        }
    }

    #[test]
    fn edits_multiline_text_and_moves_cursor() {
        let mut e = PromptEditor::default();
        e.insert("one\ntwo");
        e.up_visual(80);
        e.home();
        e.insert("X");
        assert_eq!(e.text(), "Xone\ntwo");
        e.down_visual(80);
        e.end();
        e.backspace();
        assert_eq!(e.text(), "Xone\ntw");
    }
    #[test]
    fn maps_cjk_visual_cursor() {
        let mut e = PromptEditor::default();
        e.insert("中文ab");
        e.set_cursor_visual(0, 2, 10);
        e.insert("X");
        assert_eq!(e.text(), "中X文ab");
    }
    #[test]
    fn wraps_cjk_with_the_same_rows_used_by_the_cursor() {
        let mut editor = PromptEditor::default();
        editor.insert("甲乙丙丁\n最后一行");

        assert_eq!(editor.wrapped_text(6), "甲乙丙\n丁\n最后一\n行");
        assert_eq!(editor.visual_line_count(6), 4);
        assert_eq!(editor.cursor_visual(6), (3, 2));
    }
    #[test]
    fn keeps_a_trailing_cursor_row_after_an_exact_width_wrap() {
        let mut editor = PromptEditor::default();
        editor.insert("中文");

        assert_eq!(editor.wrapped_text(4), "中文\n");
        assert_eq!(editor.cursor_visual(4), (1, 0));
    }
    #[test]
    fn exact_width_then_newline_does_not_add_an_extra_blank_row() {
        for (text, width, expected, cursor) in [
            ("abcd\nx", 4, "abcd\nx", (1, 1)),
            ("中文\n\n尾", 4, "中文\n\n尾", (2, 2)),
            ("\n\n", 4, "\n\n", (2, 0)),
            ("abcd\n", 4, "abcd\n", (1, 0)),
        ] {
            let mut editor = PromptEditor::default();
            editor.insert(text);
            assert_eq!(editor.wrapped_text(width), expected);
            assert_eq!(editor.cursor_visual(width), cursor);
        }
    }

    #[test]
    fn finds_and_replaces_marker_query_at_cursor() {
        let mut editor = PromptEditor::default();
        editor.insert("please use $image-pro");
        let (start, query) = editor.marker_query('$').unwrap();
        assert_eq!(query, "image-pro");

        editor.replace_before_cursor(start, "$image-processing ");
        assert_eq!(editor.text(), "please use $image-processing ");
        assert!(editor.marker_query('$').is_none());
    }
}
