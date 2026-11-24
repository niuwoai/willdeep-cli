use super::*;

mod question_view;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum QuestionFocus {
    #[default]
    Option,
    Answer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuestionAction {
    Option(usize),
    Answer(usize),
    Send,
    Skip,
}

/// Persistent body scroll and the exact controls painted during the last frame.
pub(super) struct QuestionViewport {
    pub(super) offset: usize,
    pub(super) body: Rect,
    pub(super) hits: Vec<(Rect, QuestionAction)>,
    total_lines: usize,
    answer_prefix: String,
    content_width: usize,
    last_selected: Option<usize>,
    last_answer: Option<String>,
    size: (u16, u16),
    pub(super) focus: QuestionFocus,
    pub(super) reveal: bool,
}

impl Default for QuestionViewport {
    fn default() -> Self {
        Self {
            offset: 0,
            body: Rect::default(),
            hits: Vec::new(),
            total_lines: 0,
            answer_prefix: String::new(),
            content_width: 1,
            last_selected: None,
            last_answer: None,
            size: (0, 0),
            focus: QuestionFocus::Option,
            reveal: true,
        }
    }
}

const QUESTION_SCROLL_ROWS: usize = 3;
const APPROVAL_SCROLL_ROWS: usize = 3;

/// Scroll state for the approval description. Long commands must be read
/// to the end before Allow is accepted.
pub(super) struct ApprovalViewport {
    pub(super) offset: usize,
    pub(super) body: Rect,
    pub(super) total_lines: usize,
}

impl Default for ApprovalViewport {
    fn default() -> Self {
        Self {
            offset: 0,
            body: Rect::default(),
            total_lines: 0,
        }
    }
}

pub(super) fn approval_decision_blocked(
    decision: ApprovalDecision,
    view: &ApprovalViewport,
) -> bool {
    matches!(
        decision,
        ApprovalDecision::AllowOnce | ApprovalDecision::AlwaysAllow
    ) && !view.at_bottom()
}

pub(super) fn approval_allow_if_read(
    view: &ApprovalViewport,
    decision: ApprovalDecision,
) -> Option<ApprovalDecision> {
    if approval_decision_blocked(decision, view) {
        None
    } else {
        Some(decision)
    }
}

impl ApprovalViewport {
    pub(super) fn at_bottom(&self) -> bool {
        self.total_lines <= usize::from(self.body.height)
            || self.offset + usize::from(self.body.height) >= self.total_lines
    }

    pub(super) fn scroll_by(&mut self, delta: isize) {
        if delta < 0 {
            self.offset = self
                .offset
                .saturating_sub((-delta) as usize * APPROVAL_SCROLL_ROWS);
        } else {
            self.offset = self
                .offset
                .saturating_add(delta as usize * APPROVAL_SCROLL_ROWS)
                .min(
                    self.total_lines
                        .saturating_sub(usize::from(self.body.height)),
                );
        }
    }
}

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

/// Human gates own every mouse event, including events outside the modal.
pub(super) fn handle_mouse(app: &mut App, mouse: MouseEvent) -> bool {
    if app.question.is_some() {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => question_click(app, mouse.column, mouse.row),
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let view = &mut app.question_view;
                if view.body.contains((mouse.column, mouse.row).into()) {
                    view.offset = if mouse.kind == MouseEventKind::ScrollUp {
                        view.offset.saturating_sub(QUESTION_SCROLL_ROWS)
                    } else {
                        view.offset.saturating_add(QUESTION_SCROLL_ROWS)
                    }
                    .min(
                        view.total_lines
                            .saturating_sub(usize::from(view.body.height)),
                    );
                    // Never dispatch a body hit painted at the previous offset.
                    view.hits.retain(|(_, action)| {
                        matches!(action, QuestionAction::Send | QuestionAction::Skip)
                    });
                    app.question_hits.clear();
                    view.reveal = false;
                }
            }
            _ => {}
        }
        return true;
    }
    if app.approval.is_some() {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let point = (mouse.column, mouse.row).into();
                if let Some((_, decision)) = app
                    .approval_action_hits
                    .iter()
                    .find(|(rect, _)| rect.contains(point))
                    .copied()
                {
                    if approval_decision_blocked(decision, &app.approval_view) {
                        return true;
                    }
                    app.resolve_approval(|_| decision);
                    app.approval_action_hits.clear();
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let view = &mut app.approval_view;
                if view.body.contains((mouse.column, mouse.row).into()) {
                    view.scroll_by(if mouse.kind == MouseEventKind::ScrollUp {
                        -1
                    } else {
                        1
                    });
                }
            }
            _ => {}
        }
        return true;
    }
    false
}

fn question_click(app: &mut App, x: u16, y: u16) {
    let point = (x, y).into();
    let action = app
        .question_view
        .hits
        .iter()
        .find(|(rect, _)| rect.contains(point))
        .map(|(_, action)| *action);
    match action {
        Some(QuestionAction::Option(selected)) => {
            let Some(dialog) = app.question.as_mut() else {
                return;
            };
            if selected >= dialog.request.options.len() {
                return;
            }
            if dialog.request.multi_select {
                dialog.selected = selected;
                if let Some(checked) = dialog.checked.get_mut(selected) {
                    *checked = !*checked;
                }
                app.question_view.focus = QuestionFocus::Option;
            } else if let Some(dialog) = app.question.take() {
                let answer = dialog.request.options.get(selected).cloned();
                let _ = dialog.sender.send(answer);
                app.promote_next_question();
            }
        }
        Some(QuestionAction::Answer(row)) => {
            let view = &mut app.question_view;
            if let Some(dialog) = &mut app.question {
                question_view::position_answer(
                    &mut dialog.answer,
                    &view.answer_prefix,
                    row,
                    usize::from(x.saturating_sub(view.body.x)),
                    view.content_width,
                );
                view.focus = QuestionFocus::Answer;
                view.reveal = true;
            }
        }
        Some(QuestionAction::Send) => {
            app.handle_question_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        }
        Some(QuestionAction::Skip) => {
            app.handle_question_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        }
        None => {}
    }
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
        let content_width = usize::from(description_rect.width.max(1));
        let wrapped = rendering::wrap_styled_text(Text::from(description.clone()), content_width);
        let lines = wrapped.lines;
        let view = &mut app.approval_view;
        view.body = description_rect;
        view.total_lines = lines.len();
        view.offset = view.offset.min(
            lines
                .len()
                .saturating_sub(usize::from(description_rect.height)),
        );
        let visible: Vec<_> = lines
            .into_iter()
            .skip(view.offset)
            .take(usize::from(description_rect.height))
            .collect();
        f.render_widget(Paragraph::new(visible), description_rect);
        let scrollable = view.total_lines > usize::from(description_rect.height);
        if scrollable && !view.at_bottom() {
            let hint = app.language.text(
                "滚到底才可允许",
                "Scroll to the end before allowing",
                "最後までスクロールしてから許可",
            );
            let hint_row = description_rect.bottom().saturating_sub(1);
            if hint_row > description_rect.y {
                f.render_widget(
                    Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
                    Rect::new(description_rect.x, hint_row, description_rect.width, 1),
                );
            }
        }
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
            let blocked = approval_decision_blocked(decision, view);
            let mut style = approval_action_style(decision, selected);
            if blocked {
                style = style.fg(Color::DarkGray).remove_modifier(Modifier::BOLD);
            }
            f.render_widget(
                Paragraph::new(approval_action_text(decision, app.language, selected)).style(style),
                row,
            );
            app.approval_action_hits.push((row, decision));
        }
        let halo = modal_halo(popup, f.area());
        seal_modal_background(f.buffer_mut(), halo);
    }
    app.question_rect = Rect::default();
    app.question_hits.clear();
    if app.question.is_some() {
        question_view::render(f, app);
    } else {
        app.question_view = QuestionViewport::default();
    }
}
