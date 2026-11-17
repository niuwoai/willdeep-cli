use super::*;

/// Human gates are drawn above permission pickers and receive their keys first.
pub(super) fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    if app.question.is_some() {
        app.prompt_history = None;
        app.handle_question_key(key);
    } else if app.approval.is_some() {
        app.prompt_history = None;
        app.handle_approval_key(key);
    } else {
        return false;
    }
    true
}

/// Consume clicks while a question is visible, including clicks outside it.
pub(super) fn handle_question_mouse(app: &mut App, x: u16, y: u16) -> bool {
    if app.question.is_none() {
        return false;
    }
    if !app.question_rect.contains((x, y).into())
        || x <= app.question_rect.x + 1
        || x >= app.question_rect.right().saturating_sub(2)
    {
        return true;
    }
    if let Some((_, selected)) = app.question_hits.iter().find(|(row, _)| *row == y).copied() {
        let multi = app
            .question
            .as_ref()
            .is_some_and(|dialog| dialog.request.multi_select);
        if multi {
            if let Some(dialog) = &mut app.question {
                dialog.selected = selected;
                dialog.checked[selected] = !dialog.checked[selected];
            }
        } else if let Some(dialog) = app.question.take() {
            let answer = dialog.request.options.get(selected).cloned();
            let _ = dialog.sender.send(answer);
            app.promote_next_question();
        }
    } else if app.question_footer_row == Some(y) {
        let code = if x < app.question_rect.x + app.question_rect.width / 2 {
            KeyCode::Esc
        } else {
            KeyCode::Enter
        };
        app.handle_question_key(KeyEvent::new(code, KeyModifiers::NONE));
    }
    true
}

pub(super) fn render(f: &mut ratatui::Frame<'_>, app: &mut App) {
    app.approval_rect = Rect::default();
    app.approval_action_hits.clear();
    if let Some((description, always, _)) = &app.approval {
        let description = description.clone();
        let always = *always;
        let action_count = approval_decisions(always).len() as u16;
        let popup_height = (9 + action_count * 2).min(f.area().height);
        let popup = centered_rect(modal_width(f.area()), popup_height, f.area());
        app.approval_rect = popup;
        paint_modal_halo(f, popup);
        let block = modal_block(
            approval_title(app.language, app.approval_queue.len()),
            Color::LightYellow,
        )
        .title_alignment(Alignment::Center)
        .title_bottom(app.language.text(
            " Ctrl+X 停止当前轮次 ",
            " Ctrl+X Stop current turn ",
            " Ctrl+X 現在のターンを停止 ",
        ));
        let inner = block.inner(popup);
        f.render_widget(block, popup);
        let button_height = 1;
        let gap = u16::from(inner.height >= action_count * 2 - 1);
        let actions_height = action_count * button_height + action_count.saturating_sub(1) * gap;
        let actions_height = actions_height.min(inner.height);
        let actions = Rect::new(
            inner.x,
            inner.bottom().saturating_sub(actions_height),
            inner.width,
            actions_height,
        );
        let description_rect = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(actions_height + 1),
        );
        f.render_widget(
            Paragraph::new(description).wrap(Wrap { trim: false }),
            description_rect,
        );
        for (index, decision) in approval_decisions(always).iter().copied().enumerate() {
            let row = Rect::new(
                actions.x,
                actions
                    .y
                    .saturating_add(index as u16 * (button_height + gap)),
                actions.width,
                button_height.min(
                    actions.bottom().saturating_sub(
                        actions
                            .y
                            .saturating_add(index as u16 * (button_height + gap)),
                    ),
                ),
            );
            if row.height == 0 {
                continue;
            }
            let selected = app.approval_selected == index;
            f.render_widget(
                Paragraph::new(approval_action_text(decision, app.language, selected))
                    .style(approval_action_style(decision, selected)),
                row,
            );
            app.approval_action_hits.push((row, decision));
        }
        let halo = modal_halo(popup, f.area());
        seal_modal_background(f.buffer_mut(), halo);
    }
    app.question_rect = Rect::default();
    app.question_hits.clear();
    app.question_footer_row = None;
    if app.question.is_some() {
        render_question(f, app);
    }
}

fn render_question(f: &mut ratatui::Frame<'_>, app: &mut App) {
    let dialog = app.question.as_ref().unwrap();
    let width = modal_width(f.area());
    let content_width = usize::from(width.saturating_sub(4).max(1));
    let mut lines = Vec::new();
    let mut option_ranges = Vec::new();
    {
        let mut append = |text: String, style: Style| {
            let mut text = Text::from(text);
            for line in &mut text.lines {
                line.style = style;
            }
            let mut wrapped = wrap_styled_text(text, content_width);
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
        // Preserve all choices via a scrolling viewport when the terminal is short.
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
    let answer_prefix = format!(
        "{}: ",
        app.language
            .text("其他答案", "Other answer", "その他の回答")
    );
    let answer_line_start = lines.len();
    let answer_value = if dialog.answer.text().is_empty() {
        " ".to_owned()
    } else {
        dialog.answer.text().to_owned()
    };
    // Use the editor's exact visual rows for both content and cursor placement.
    // A single styled Line containing newlines is not a multiline Text, and
    // word wrapping here would disagree with the editor's character wrapping.
    let mut answer_editor = PromptEditor::default();
    answer_editor.insert(&format!("{answer_prefix}{answer_value}"));
    let wrapped_answer = answer_editor.wrapped_text(content_width);
    let mut prefix_remaining = answer_prefix.len();
    for row in wrapped_answer.split('\n') {
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
    let footer_start = lines.len();
    let mut footer_text = Text::from(help.to_owned());
    for line in &mut footer_text.lines {
        line.style = Style::default().fg(Color::Gray);
    }
    let mut wrapped_footer = wrap_styled_text(footer_text, content_width);
    for line in &mut wrapped_footer.lines {
        line.style = Style::default().fg(Color::Gray);
    }
    lines.extend(wrapped_footer.lines);
    let footer = footer_start..lines.len();
    let height = (lines.len().saturating_add(2).min(u16::MAX as usize) as u16)
        .max(8)
        .min(f.area().height);
    let popup = centered_rect(width, height, f.area());
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
    let viewport = usize::from(inner.height);
    let selected = option_ranges.get(dialog.selected);
    let (answer_cursor_row, answer_cursor_column) = dialog
        .answer
        .cursor_visual_with_prefix(&answer_prefix, content_width);
    let answer_cursor_line = answer_line_start + answer_cursor_row;
    let offset = if !dialog.answer.text().is_empty() {
        answer_cursor_line.saturating_sub(viewport.saturating_sub(1))
    } else {
        selected.map_or(0, |range| range.end.saturating_sub(viewport))
    };
    app.question_rect = popup;
    for (index, range) in option_ranges.into_iter().enumerate() {
        for row in range {
            if row >= offset && row - offset < viewport {
                app.question_hits
                    .push((inner.y + (row - offset) as u16, index));
            }
        }
    }
    // Only a fully visible hint row is actionable, never blank space or borders.
    if footer.len() == 1 && footer.start >= offset && footer.start - offset < viewport {
        app.question_footer_row = Some(inner.y + (footer.start - offset) as u16);
    }
    paint_modal_halo(f, popup);
    f.render_widget(block, popup);
    f.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(offset)
                .take(viewport)
                .collect::<Vec<_>>(),
        ),
        inner,
    );
    if answer_cursor_line >= offset && answer_cursor_line - offset < viewport {
        let cursor_x = inner.x + (answer_cursor_column as u16).min(inner.width.saturating_sub(1));
        let cursor_y = inner.y + (answer_cursor_line - offset) as u16;
        f.set_cursor_position((cursor_x, cursor_y));
    }
    let halo = modal_halo(popup, f.area());
    seal_modal_background(f.buffer_mut(), halo);
}
