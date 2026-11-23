use super::*;
use std::ops::Range;
use unicode_width::UnicodeWidthStr;

/// Keep controls outside the independently scrolling question body.
pub(super) fn render(f: &mut ratatui::Frame<'_>, app: &mut App) {
    let dialog = app.question.as_ref().expect("visible question");
    let width = modal_width(f.area());
    let content_width = usize::from(width.saturating_sub(4).max(1));
    let mut lines = Vec::new();
    let mut option_ranges = Vec::new();
    {
        let mut append = |value: String, style: Style| {
            let mut wrapped = wrap_styled_text(Text::from(value), content_width);
            for line in &mut wrapped.lines {
                line.style = style;
            }
            let start = lines.len();
            lines.extend(wrapped.lines);
            start..lines.len()
        };
        append(
            dialog.request.question.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        );
        append(String::new(), Style::default());
        let gap = usize::from(f.area().height >= 12);
        for (index, option) in dialog.request.options.iter().enumerate() {
            if index > 0 && gap > 0 {
                append(String::new(), Style::default());
            }
            let selected = index == dialog.selected;
            let marker = if dialog.request.multi_select {
                if dialog.checked[index] { "[x]" } else { "[ ]" }
            } else if selected {
                "(*)"
            } else {
                "( )"
            };
            let style = if selected {
                Style::default()
                    .bg(Color::LightCyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            option_ranges.push(append(
                format!("{} {marker} {option}", if selected { "▶" } else { " " }),
                style,
            ));
        }
        append(String::new(), Style::default());
    }
    let answer_prefix = answer_prefix(app.language);
    let answer_start = lines.len();
    append_answer(&mut lines, &answer_prefix, &dialog.answer, content_width);
    let help = if dialog.request.multi_select {
        app.language.text(
            "↑/↓ 选择 · Space 勾选 · 输入其他答案 · Enter 发送 · Esc 跳过",
            "↑/↓ select · Space toggle · type another answer · Enter send · Esc skip",
            "↑/↓ 選択 · Space 切替 · その他を入力 · Enter 送信 · Esc スキップ",
        )
    } else {
        app.language.text(
            "↑/↓ 选择 · 可输入其他答案 · Enter 发送 · Esc 跳过",
            "↑/↓ select · type another answer · Enter send · Esc skip",
            "↑/↓ 選択 · その他を入力 · Enter 送信 · Esc スキップ",
        )
    };
    let height = lines.len().saturating_add(4).clamp(8, u16::MAX as usize) as u16;
    let popup = centered_rect(width, height.min(f.area().height), f.area());
    let block = modal_block(
        queued_title(
            app.language.text(
                "智能体提问",
                "Question from Agent",
                "エージェントからの質問",
            ),
            app.language,
            app.question_queue.len(),
        ),
        Color::LightCyan,
    );
    let inner = block.inner(popup);
    // On short windows sacrifice the hint first, then body rows. Controls stay fixed.
    let show_help = inner.height >= 4;
    let controls_height = u16::from(inner.height > 0);
    let body = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner
            .height
            .saturating_sub(controls_height + u16::from(show_help)),
    );
    let controls = Rect::new(
        inner.x,
        inner.bottom().saturating_sub(controls_height),
        inner.width,
        controls_height,
    );
    let view = &mut app.question_view;
    let size = (body.width, body.height);
    if view.last_selected != Some(dialog.selected) {
        view.focus = QuestionFocus::Option;
        view.reveal = true;
    }
    if view.last_answer.as_deref() != Some(dialog.answer.text()) && !dialog.answer.text().is_empty()
    {
        view.focus = QuestionFocus::Answer;
        view.reveal = true;
    }
    if view.size != size {
        view.reveal = true;
    }
    view.last_selected = Some(dialog.selected);
    view.last_answer = Some(dialog.answer.text().to_owned());
    view.size = size;
    view.body = body;
    view.total_lines = lines.len();
    view.answer_prefix = answer_prefix.clone();
    view.content_width = content_width;
    let (cursor_row, cursor_column) = dialog
        .answer
        .cursor_visual_with_prefix(&answer_prefix, content_width);
    let cursor_line = answer_start + cursor_row;
    if view.reveal {
        let target = match view.focus {
            QuestionFocus::Option => option_ranges
                .get(dialog.selected)
                .cloned()
                .unwrap_or(answer_start..answer_start + 1),
            QuestionFocus::Answer => cursor_line..cursor_line + 1,
        };
        reveal(view, target);
        view.reveal = false;
    }
    view.offset = view
        .offset
        .min(lines.len().saturating_sub(usize::from(body.height)));
    view.hits.clear();
    app.question_rect = popup;
    app.question_hits.clear();
    for (index, range) in option_ranges.into_iter().enumerate() {
        for row in range {
            if let Some(rect) = visible_row(view, row) {
                app.question_hits.push((rect.y, index));
                view.hits.push((rect, QuestionAction::Option(index)));
            }
        }
    }
    for row in answer_start..lines.len() {
        if let Some(rect) = visible_row(view, row) {
            view.hits
                .push((rect, QuestionAction::Answer(row - answer_start)));
        }
    }
    paint_modal_halo(f, popup);
    f.render_widget(block, popup);
    f.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(view.offset)
                .take(usize::from(body.height))
                .collect::<Vec<_>>(),
        ),
        body,
    );
    if show_help {
        f.render_widget(
            Paragraph::new(help).style(Style::default().fg(Color::Gray)),
            Rect::new(inner.x, controls.y.saturating_sub(1), inner.width, 1),
        );
    }
    render_controls(f, view, controls, app.language);
    if body.width > 0
        && view.focus == QuestionFocus::Answer
        && let Some(rect) = visible_row(view, cursor_line)
    {
        f.set_cursor_position((
            rect.x + (cursor_column.min(usize::from(body.width.saturating_sub(1))) as u16),
            rect.y,
        ));
    }
    let halo = modal_halo(popup, f.area());
    seal_modal_background(f.buffer_mut(), halo);
}

fn visible_row(view: &QuestionViewport, row: usize) -> Option<Rect> {
    (view.body.width > 0 && row >= view.offset && row - view.offset < usize::from(view.body.height))
        .then(|| {
            Rect::new(
                view.body.x,
                view.body.y + (row - view.offset) as u16,
                view.body.width,
                1,
            )
        })
}

fn reveal(view: &mut QuestionViewport, target: Range<usize>) {
    let height = usize::from(view.body.height);
    if target.start < view.offset {
        view.offset = target.start;
    } else if target.end > view.offset.saturating_add(height) {
        view.offset = target.start.min(target.end.saturating_sub(height));
    }
}

fn answer_prefix(language: Language) -> String {
    format!(
        "{}: ",
        language.text("其他答案", "Other answer", "その他の回答")
    )
}

fn append_answer(
    lines: &mut Vec<Line<'static>>,
    prefix: &str,
    answer: &PromptEditor,
    width: usize,
) {
    let mut editor = PromptEditor::default();
    editor.insert(prefix);
    editor.insert(if answer.text().is_empty() {
        " "
    } else {
        answer.text()
    });
    let wrapped = editor.wrapped_text(width);
    let mut prefix_remaining = prefix.len();
    for row in wrapped.split('\n') {
        let prefix_bytes = prefix_remaining.min(row.len());
        prefix_remaining -= prefix_bytes;
        lines.push(Line::from(vec![
            Span::styled(
                row[..prefix_bytes].to_owned(),
                Style::default().fg(Color::LightYellow),
            ),
            Span::styled(
                row[prefix_bytes..].to_owned(),
                Style::default()
                    .fg(Color::LightYellow)
                    .add_modifier(Modifier::UNDERLINED),
            ),
        ]));
    }
}

fn render_controls(
    f: &mut ratatui::Frame<'_>,
    view: &mut QuestionViewport,
    area: Rect,
    language: Language,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let labels = [
        language.text("[ 跳过 ]", "[ Skip ]", "[ スキップ ]"),
        language.text("[ 发送 ]", "[ Send ]", "[ 送信 ]"),
    ];
    let full_width = labels
        .iter()
        .map(|text| UnicodeWidthStr::width(*text) as u16)
        .sum::<u16>()
        + 1;
    let labels = if full_width <= area.width {
        labels
    } else {
        [
            language.text("跳过", "Skip", "省略"),
            language.text("发送", "Send", "送信"),
        ]
    };
    let natural = labels.map(|text| UnicodeWidthStr::width(text) as u16);
    let gap = u16::from(area.width > natural.iter().sum::<u16>());
    let available = area.width.saturating_sub(gap);
    let widths = if natural.iter().sum::<u16>() <= available {
        natural
    } else {
        [available / 2, available - available / 2]
    };
    let mut x = area.x;
    for ((label, width), action) in labels
        .into_iter()
        .zip(widths)
        .zip([QuestionAction::Skip, QuestionAction::Send])
    {
        let rect = Rect::new(x, area.y, width, 1);
        if width > 0 {
            f.render_widget(
                Paragraph::new(label).style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::LightCyan)
                        .add_modifier(Modifier::BOLD),
                ),
                rect,
            );
            view.hits.push((rect, action));
        }
        x = x.saturating_add(width).saturating_add(gap);
    }
}

/// Ask the same editor used to draw the prefixed answer for the clicked byte boundary.
/// Rebuild only this small editor so its public insert API leaves the cursor there.
pub(super) fn position_answer(
    answer: &mut PromptEditor,
    prefix: &str,
    row: usize,
    column: usize,
    width: usize,
) {
    let mut combined = PromptEditor::default();
    combined.insert(prefix);
    combined.insert(answer.text());
    combined.begin_selection_visual(0, 0, width);
    combined.extend_selection_visual(row, column, width);
    let byte = combined
        .selected_text()
        .map_or(0, str::len)
        .saturating_sub(prefix.len())
        .min(answer.text().len());
    let value = answer.take();
    answer.insert(&value[byte..]);
    answer.set_cursor_visual(0, 0, width);
    answer.insert(&value[..byte]);
}
