//! Provenance travels with characters through parsing and wrapping. Display text
//! is never searched to recover an original position.
use super::*;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::tui) enum SourceId {
    Archive,
    Record(usize),
    Stream(u64),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::tui) enum SourcePart {
    Prefix,
    Body,
    AttachmentLabel(usize),
    AttachmentBody(usize),
    ToolDetail,
    Control,
    Time,
    Decoration,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::tui) struct SourcePoint {
    pub(in crate::tui) id: SourceId,
    pub(in crate::tui) part: SourcePart,
    pub(in crate::tui) byte: usize,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::tui) struct SourceChar {
    pub(in crate::tui) point: SourcePoint,
    pub(in crate::tui) end: usize,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct SourceCell {
    pub(in crate::tui) columns: Range<usize>,
    pub(in crate::tui) source: SourceChar,
    pub(in crate::tui) character: char,
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct SourceRow {
    pub(in crate::tui) fallback: SourcePoint,
    pub(in crate::tui) cells: Vec<SourceCell>,
}

impl SourceRow {
    pub(in crate::tui) fn empty(id: SourceId, part: SourcePart, byte: usize) -> Self {
        Self {
            fallback: SourcePoint { id, part, byte },
            cells: Vec::new(),
        }
    }

    pub(in crate::tui) fn point_at(&self, column: usize) -> SourcePoint {
        self.cells
            .iter()
            .find(|cell| cell.columns.end > column)
            .or_else(|| self.cells.last())
            .map_or(self.fallback, |cell| cell.source.point)
    }

    pub(in crate::tui) fn rebase(&mut self, id: SourceId) {
        self.fallback.id = id;
        for cell in &mut self.cells {
            cell.source.point.id = id;
        }
    }

    pub(in crate::tui) fn shift_columns(&mut self, by: usize) {
        for cell in &mut self.cells {
            cell.columns.start += by;
            cell.columns.end += by;
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::tui) struct SourceLine {
    pub(in crate::tui) fallback: SourcePoint,
    pub(in crate::tui) chars: Vec<Option<SourceChar>>,
}

impl SourceLine {
    pub(in crate::tui) fn literal(
        id: SourceId,
        part: SourcePart,
        value: &str,
        offset: usize,
    ) -> Self {
        Self {
            fallback: SourcePoint {
                id,
                part,
                byte: offset,
            },
            chars: value
                .char_indices()
                .map(|(byte, ch)| {
                    Some(SourceChar {
                        point: SourcePoint {
                            id,
                            part,
                            byte: offset + byte,
                        },
                        end: offset + byte + ch.len_utf8(),
                    })
                })
                .collect(),
        }
    }

    pub(in crate::tui) fn synthetic(id: SourceId, part: SourcePart, value: &str) -> Self {
        Self {
            fallback: SourcePoint { id, part, byte: 0 },
            chars: vec![None; value.chars().count()],
        }
    }

    fn append(&mut self, value: Self) {
        self.chars.extend(value.chars);
    }
}

pub(in crate::tui) fn literal_lines(
    id: SourceId,
    part: SourcePart,
    value: &str,
) -> Vec<SourceLine> {
    let mut offset = 0;
    value
        .split('\n')
        .map(|line| {
            let mapped = SourceLine::literal(id, part, line, offset);
            offset += line.len() + 1;
            mapped
        })
        .collect()
}

/// Exact bytes win. If folding hides those bytes, use the first row of that
/// record (its title), rather than its last remaining body character.
pub(in crate::tui) fn resolve(
    rows: &[SourceRow],
    point: SourcePoint,
) -> Option<ChatSelectionPoint> {
    for (row, mapped) in rows.iter().enumerate() {
        for cell in &mapped.cells {
            if cell.source.point.id == point.id
                && cell.source.point.part == point.part
                && cell.source.point.byte <= point.byte
                && point.byte < cell.source.end
            {
                return Some(ChatSelectionPoint {
                    row,
                    column: cell.columns.start,
                });
            }
        }
        if mapped.cells.is_empty() && mapped.fallback == point {
            return Some(ChatSelectionPoint { row, column: 0 });
        }
    }
    // A soft wrap can consume separating whitespace. Resolve a byte inside
    // the still-visible part to the next character, preserving that paragraph.
    // Bytes beyond its visible end are folded and instead use the record title.
    let same_part = rows
        .iter()
        .enumerate()
        .flat_map(|(row, mapped)| {
            mapped
                .cells
                .iter()
                .filter(move |cell| {
                    cell.source.point.id == point.id && cell.source.point.part == point.part
                })
                .map(move |cell| (row, cell))
        })
        .collect::<Vec<_>>();
    if same_part
        .iter()
        .map(|(_, cell)| cell.source.end)
        .max()
        .is_some_and(|end| point.byte < end)
        && let Some((row, cell)) = same_part
            .iter()
            .find(|(_, cell)| cell.source.point.byte >= point.byte)
            .or_else(|| same_part.last())
    {
        return Some(ChatSelectionPoint {
            row: *row,
            column: cell.columns.start,
        });
    }
    rows.iter()
        .position(|row| row.fallback.id == point.id)
        .or_else(|| rows.iter().position(|row| row.fallback.id >= point.id))
        .or_else(|| rows.len().checked_sub(1))
        .map(|row| ChatSelectionPoint {
            row,
            column: rows[row].cells.first().map_or(0, |cell| cell.columns.start),
        })
}

#[derive(Clone, Default)]
pub(in crate::tui) struct SourceFrame {
    pub(in crate::tui) rows: Vec<SourceRow>,
    pub(in crate::tui) parts: BTreeMap<(SourceId, SourcePart), String>,
    pub(in crate::tui) aliases: BTreeMap<SourceId, SourceId>,
}

impl SourceFrame {
    pub(in crate::tui) fn resolve(&self, mut point: SourcePoint) -> Option<ChatSelectionPoint> {
        if !self.rows.iter().any(|row| row.fallback.id == point.id)
            && let Some(id) = self.aliases.get(&point.id)
        {
            point.id = *id;
            point.byte = 0;
        }
        resolve(&self.rows, point)
    }

    pub(in crate::tui) fn point_at(&self, point: ChatSelectionPoint) -> Option<SourcePoint> {
        self.rows
            .get(point.row)
            .map(|row| row.point_at(point.column))
    }

    pub(in crate::tui) fn rebase(&mut self, ids: &BTreeMap<SourceId, SourceId>) {
        for row in &mut self.rows {
            row.rebase(
                ids.get(&row.fallback.id)
                    .copied()
                    .unwrap_or(row.fallback.id),
            );
        }
        self.parts = std::mem::take(&mut self.parts)
            .into_iter()
            .map(|((id, part), text)| ((ids.get(&id).copied().unwrap_or(id), part), text))
            .collect();
        self.aliases = std::mem::take(&mut self.aliases)
            .into_iter()
            .map(|(from, to)| {
                (
                    ids.get(&from).copied().unwrap_or(from),
                    ids.get(&to).copied().unwrap_or(to),
                )
            })
            .collect();
    }

    /// Only mapped cells are copied. Soft-wrap gaps restore original whitespace;
    /// Markdown delimiters, box padding and generated controls are not copied.
    pub(in crate::tui) fn selected_text(&self, anchor: SourcePoint, head: SourcePoint) -> String {
        let (Some(a), Some(b)) = (self.resolve(anchor), self.resolve(head)) else {
            return String::new();
        };
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let mut output = String::new();
        let mut previous: Option<SourceChar> = None;
        for (row, mapped) in self.rows.iter().enumerate().take(b.row + 1).skip(a.row) {
            for cell in &mapped.cells {
                if (row == a.row && cell.columns.end <= a.column)
                    || (row == b.row && cell.columns.start > b.column)
                {
                    continue;
                }
                let source = cell.source;
                if let Some(last) = previous {
                    if last.point.id == source.point.id && last.point.part == source.point.part {
                        if source.point.byte < last.end {
                            continue;
                        }
                        if let Some(raw) = self.parts.get(&(source.point.id, source.point.part))
                            && let Some(gap) = raw.get(last.end..source.point.byte)
                        {
                            if gap.chars().all(char::is_whitespace) {
                                output.push_str(gap);
                            } else {
                                output.extend(gap.chars().filter(|ch| *ch == '\n'));
                            }
                        }
                    } else if !(output.is_empty()
                        || output.ends_with('\n')
                        || (last.point.id == source.point.id
                            && last.point.part == SourcePart::Prefix
                            && source.point.part == SourcePart::Body))
                    {
                        output.push('\n');
                    }
                }
                output.push(cell.character);
                previous = Some(source);
            }
        }
        output
    }
}

pub(in crate::tui) fn wrap_literal(
    text: Text<'static>,
    id: SourceId,
    part: SourcePart,
    raw: &str,
    width: usize,
) -> (Text<'static>, Vec<SourceRow>) {
    wrap_styled_text_sourced(text, width, &literal_lines(id, part, raw))
}

/// Ordinary rows are literal. Thought header and body are separate parts so
/// changing '+' to '−' never changes any body byte position.
pub(in crate::tui) fn ordinary_lines(
    id: SourceId,
    raw: &str,
    text: &Text<'static>,
) -> Vec<SourceLine> {
    if let Some((header, body, expanded)) = thought_entry_parts(raw) {
        let prefix = format!("· {} Thought: {header}", if expanded { "−" } else { "+" });
        let mut lines = vec![SourceLine::literal(id, SourcePart::Prefix, &prefix, 0)];
        if expanded {
            lines.extend(
                literal_lines(id, SourcePart::Body, body)
                    .into_iter()
                    .take(body.lines().count()),
            );
        }
        return lines;
    }
    if workflow_ui::decode(raw).is_some()
        || decode_plan_entry(raw).is_some()
        || is_welcome_entry(raw)
        || raw.starts_with(TURN_DIVIDER_PREFIX)
    {
        // Cards/banners contain generated layout. Their record title is an
        // explicit fallback; generated characters never claim original bytes.
        return text
            .lines
            .iter()
            .map(|line| SourceLine::synthetic(id, SourcePart::Decoration, &line.to_string()))
            .collect();
    }
    literal_lines(id, SourcePart::Body, raw)
}

pub(in crate::tui) fn record_body(raw: &str) -> &str {
    if raw.starts_with(TURN_DIVIDER_PREFIX) {
        return "";
    }
    if let Some(body) = raw
        .strip_prefix("You: ")
        .or_else(|| raw.strip_prefix(ASSISTANT_ROW_PREFIX))
    {
        body
    } else if let Some((_, body, _)) = thought_entry_parts(raw) {
        body
    } else {
        raw
    }
}

/// Parse inline syntax using the same branches as the renderer, assigning the
/// source slice at each emission. Searches below are grammar parsing on raw text.
fn inline(id: SourceId, value: &str, offset: usize) -> SourceLine {
    let mut output = SourceLine::synthetic(id, SourcePart::Body, "");
    output.fallback.byte = offset;
    let mut rest = value;
    let mut at = offset;
    while !rest.is_empty() {
        let marker = [
            rest.find("**").map(|i| (i, "bold")),
            rest.find('`').map(|i| (i, "code")),
            rest.find("![").map(|i| (i, "image")),
            rest.find('[').map(|i| (i, "link")),
        ]
        .into_iter()
        .flatten()
        .min_by_key(|item| item.0);
        let Some((index, kind)) = marker else {
            output.append(SourceLine::literal(id, SourcePart::Body, rest, at));
            break;
        };
        if index > 0 {
            output.append(SourceLine::literal(
                id,
                SourcePart::Body,
                &rest[..index],
                at,
            ));
            rest = &rest[index..];
            at += index;
        }
        let consumed = match kind {
            "bold" if rest[2..].contains("**") => {
                let end = rest[2..].find("**").expect("checked bold") + 2;
                output.append(SourceLine::literal(
                    id,
                    SourcePart::Body,
                    &rest[2..end],
                    at + 2,
                ));
                end + 2
            }
            "code" if rest[1..].contains('`') => {
                let end = rest[1..].find('`').expect("checked code") + 1;
                output.append(SourceLine::literal(
                    id,
                    SourcePart::Body,
                    &rest[1..end],
                    at + 1,
                ));
                end + 1
            }
            "image" | "link" if rest.contains("](") => {
                let end = rest.find("](").expect("checked label");
                if let Some(url_end) = rest[end + 2..].find(')') {
                    let url_end = end + 2 + url_end;
                    if kind == "image" {
                        let raw = &rest[2..end];
                        let label = raw.trim();
                        output.append(SourceLine::synthetic(id, SourcePart::Decoration, "▧ "));
                        if label.is_empty() {
                            output.append(SourceLine::synthetic(id, SourcePart::Decoration, "—"));
                        } else {
                            output.append(SourceLine::literal(
                                id,
                                SourcePart::Body,
                                label,
                                at + 2 + raw.len() - raw.trim_start().len(),
                            ));
                        }
                        output.append(SourceLine::synthetic(
                            id,
                            SourcePart::Decoration,
                            " · Ctrl+L",
                        ));
                    } else {
                        output.append(SourceLine::literal(
                            id,
                            SourcePart::Body,
                            &rest[1..end],
                            at + 1,
                        ));
                        output.append(SourceLine::synthetic(id, SourcePart::Decoration, " ("));
                        output.append(SourceLine::literal(
                            id,
                            SourcePart::Body,
                            &rest[end + 2..url_end],
                            at + end + 2,
                        ));
                        output.append(SourceLine::synthetic(id, SourcePart::Decoration, ")"));
                    }
                    url_end + 1
                } else {
                    output.append(SourceLine::literal(id, SourcePart::Body, &rest[..1], at));
                    1
                }
            }
            _ => {
                output.append(SourceLine::literal(id, SourcePart::Body, &rest[..1], at));
                1
            }
        };
        rest = &rest[consumed..];
        at += consumed;
    }
    output
}

fn markdown_line(
    id: SourceId,
    raw: &str,
    offset: usize,
    show_prefix: bool,
    label: &str,
) -> SourceLine {
    let mut output = if show_prefix {
        SourceLine::literal(id, SourcePart::Prefix, &format!("{label}: "), 0)
    } else {
        SourceLine::synthetic(id, SourcePart::Body, "")
    };
    output.fallback = SourcePoint {
        id,
        part: SourcePart::Body,
        byte: offset,
    };
    let trimmed = raw.trim_start();
    let marker = [
        ("### ", "▸ "),
        ("## ", "◆ "),
        ("# ", "■ "),
        ("> ", "│ "),
        ("- ", "• "),
        ("* ", "• "),
    ]
    .into_iter()
    .find(|(prefix, _)| trimmed.starts_with(prefix));
    if let Some((prefix, rendered)) = marker {
        output.append(SourceLine::synthetic(id, SourcePart::Decoration, rendered));
        let skipped = raw.len() - trimmed.len() + prefix.len();
        output.append(inline(id, &raw[skipped..], offset + skipped));
    } else {
        output.append(inline(id, raw, offset));
    }
    output
}

fn html_pieces(raw: &str) -> Vec<(usize, &str)> {
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < raw.len() {
        let tag = ["<br />", "<br/>", "<br>"].into_iter().find(|tag| {
            raw.get(at..at + tag.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(tag))
        });
        if let Some(tag) = tag {
            pieces.push((start, &raw[start..at]));
            at += tag.len();
            start = at;
        } else {
            at += raw[at..]
                .chars()
                .next()
                .expect("remaining character")
                .len_utf8();
        }
    }
    pieces.push((start, &raw[start..]));
    pieces
}

/// Table cells are sliced from raw input before trimming or <br> expansion.
/// Rules and alignment padding deliberately have no selectable provenance.
fn table_sources(
    id: SourceId,
    raw_rows: &[(&str, usize)],
    rows: &[Vec<String>],
    width: usize,
) -> Vec<SourceLine> {
    let widths = table_column_widths(rows, width.max(1));
    let rule = table_horizontal_rule(&widths, '┌', '┬', '┐');
    let mut output = vec![SourceLine::synthetic(id, SourcePart::Decoration, &rule)];
    for ((raw, offset), cells) in raw_rows.iter().zip(rows) {
        let trimmed = raw.trim();
        let mut cell_at = raw.len() - raw.trim_start().len() + 1;
        let mut mappings = Vec::new();
        for (column, raw_cell) in trimmed[1..trimmed.len() - 1].split('|').enumerate() {
            let value = raw_cell.trim();
            let at = offset + cell_at + raw_cell.len() - raw_cell.trim_start().len();
            let column_width = widths.get(column).copied().unwrap_or(1);
            let mut mapped = Vec::new();
            for (piece_at, piece) in html_pieces(value) {
                let mut line = SourceLine::synthetic(id, SourcePart::Body, "");
                let mut columns = 0;
                let mut characters = Vec::new();
                let mut byte = 0;
                while byte < piece.len() {
                    let rest = &piece[byte..];
                    if rest.starts_with("**") || rest.starts_with("__") {
                        byte += 2;
                        continue;
                    }
                    let ch = rest.chars().next().expect("remaining table character");
                    if ch != '`' {
                        characters.push((byte, ch));
                    }
                    byte += ch.len_utf8();
                }
                for (byte, ch) in characters {
                    let size = UnicodeWidthChar::width(ch).unwrap_or(0);
                    if columns + size > column_width && !line.chars.is_empty() {
                        mapped.push(line);
                        line = SourceLine::synthetic(id, SourcePart::Body, "");
                        columns = 0;
                    }
                    line.append(SourceLine::literal(
                        id,
                        SourcePart::Body,
                        &ch.to_string(),
                        at + piece_at + byte,
                    ));
                    columns += size;
                    if columns >= column_width {
                        mapped.push(line);
                        line = SourceLine::synthetic(id, SourcePart::Body, "");
                        columns = 0;
                    }
                }
                if !line.chars.is_empty() || piece.is_empty() {
                    mapped.push(line);
                }
            }
            mappings.push(mapped);
            cell_at += raw_cell.len() + 1;
        }
        let wrapped = widths
            .iter()
            .enumerate()
            .map(|(column, size)| {
                wrap_table_cell(cells.get(column).map(String::as_str).unwrap_or(""), *size)
            })
            .collect::<Vec<_>>();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
        for line in 0..height {
            let mut mapping = SourceLine::synthetic(id, SourcePart::Decoration, "│ ");
            for (column, size) in widths.iter().enumerate() {
                if column > 0 {
                    mapping.append(SourceLine::synthetic(id, SourcePart::Decoration, " │ "));
                }
                let value = wrapped[column].get(line).map(String::as_str).unwrap_or("");
                if let Some(source) = mappings.get(column).and_then(|cell| cell.get(line)) {
                    mapping.append(source.clone());
                }
                mapping.append(SourceLine::synthetic(
                    id,
                    SourcePart::Decoration,
                    &" ".repeat(size.saturating_sub(UnicodeWidthStr::width(value))),
                ));
            }
            mapping.append(SourceLine::synthetic(id, SourcePart::Decoration, " │"));
            output.push(mapping);
        }
        output.push(SourceLine::synthetic(id, SourcePart::Decoration, &rule));
    }
    output
}

pub(in crate::tui) fn assistant(
    content: &str,
    width: usize,
    label: &str,
    id: SourceId,
) -> (Vec<Line<'static>>, Vec<CodeBlockLines>, Vec<SourceLine>) {
    let mut output = Vec::new();
    let mut sources = Vec::new();
    let mut blocks = Vec::new();
    let mut code_block = false;
    let mut code_start = 0;
    let mut code_content = Vec::new();
    let mut byte = 0;
    let lines = content
        .lines()
        .map(|line| {
            let at = byte;
            byte += line.len() + 1;
            (line, at)
        })
        .collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        let (raw, at) = lines[index];
        if raw.trim_start().starts_with("```") {
            if code_block {
                push_code_block(&mut blocks, code_start, output.len(), &code_content);
            } else {
                code_start = output.len();
                code_content.clear();
            }
            code_block = !code_block;
            index += 1;
            continue;
        }
        if code_block {
            code_content.push(raw.to_owned());
            let mut spans = assistant_prefix(index == 0, label);
            spans.push(Span::styled(
                raw.to_owned(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            output.push(Line::from(spans));
            let mut mapping = SourceLine::synthetic(id, SourcePart::Body, "");
            mapping.fallback.byte = at;
            if index == 0 {
                mapping.append(SourceLine::literal(
                    id,
                    SourcePart::Prefix,
                    &format!("{label}: "),
                    0,
                ));
            }
            mapping.append(SourceLine::literal(id, SourcePart::Body, raw, at));
            sources.push(mapping);
            index += 1;
            continue;
        }
        if index + 1 < lines.len()
            && parse_table_row(raw).is_some()
            && is_table_separator(lines[index + 1].0)
        {
            let mut rows = vec![parse_table_row(raw).expect("checked table")];
            let mut raw_rows = vec![(raw, at)];
            index += 2;
            while index < lines.len() {
                let Some(row) = parse_table_row(lines[index].0) else {
                    break;
                };
                rows.push(row);
                raw_rows.push(lines[index]);
                index += 1;
            }
            if output.is_empty() {
                output.push(Line::from(assistant_prefix(true, label)));
                sources.push(SourceLine::literal(
                    id,
                    SourcePart::Prefix,
                    &format!("{label}: "),
                    0,
                ));
            }
            output.extend(render_markdown_table(&rows, width));
            sources.extend(table_sources(id, &raw_rows, &rows, width));
            continue;
        }
        for (piece_index, (piece_at, piece)) in html_pieces(raw).into_iter().enumerate() {
            let show = index == 0 && piece_index == 0;
            output.push(render_markdown_line(piece, show, label));
            sources.push(markdown_line(id, piece, at + piece_at, show, label));
        }
        index += 1;
    }
    if code_block {
        push_code_block(&mut blocks, code_start, output.len(), &code_content);
    }
    if output.is_empty() {
        output.push(Line::from(assistant_prefix_with_suffix(label, ":")));
        sources.push(SourceLine::literal(
            id,
            SourcePart::Prefix,
            &format!("{label}:"),
            0,
        ));
    }
    (output, blocks, sources)
}

pub(in crate::tui) fn assistant_entry(
    content: &str,
    search: Option<&str>,
    width: usize,
    label: &str,
    id: SourceId,
) -> (Text<'static>, Vec<CodeBlockLines>, Vec<SourceRow>) {
    let (lines, blocks, mappings) = assistant(content, width, assistant_display_name(label), id);
    let mut text = Text::from(lines);
    if let Some(query) = search {
        highlight_matches(&mut text, query);
    }
    let mut offsets = vec![0];
    for line in &text.lines {
        let height = wrap_styled_text(Text::from(line.clone()), width)
            .lines
            .len();
        offsets.push(offsets.last().copied().unwrap_or_default() + height);
    }
    let (wrapped, sources) = wrap_styled_text_sourced(text, width, &mappings);
    let blocks = blocks
        .into_iter()
        .map(|block| CodeBlockLines {
            start: offsets[block.start],
            end: offsets[block.end],
            content: block.content,
        })
        .collect();
    (wrapped, blocks, sources)
}
