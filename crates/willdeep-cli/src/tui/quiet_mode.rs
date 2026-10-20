use super::*;

const QUIET_AFTER: Duration = Duration::from_secs(5);
const QUIET_INPUT_BACKGROUND: Color = Color::Rgb(24, 32, 46);

impl App {
    pub(super) fn quiet_mode_at(&self, now: Instant) -> bool {
        !self.sidebar_visible
            && !self.has_open_overlay()
            && !self.selection_mode
            && !self.native_selection_mode
            && !self.composer_expanded
            && self.attachments.is_empty()
            && self.notice.is_none()
            && now.saturating_duration_since(self.last_user_activity) >= QUIET_AFTER
    }

    pub(super) fn note_user_activity(&mut self) {
        self.last_user_activity = Instant::now();
        self.quiet_drawn = false;
    }

    /// 首次点击按简洁布局定位焦点，只唤醒，不在重排后的坐标误触动作。
    pub(super) fn wake_quiet_mouse(&mut self, mouse: MouseEvent) -> bool {
        if mouse.kind == MouseEventKind::Moved {
            return false;
        }
        let was_quiet = self.quiet_drawn;
        self.note_user_activity();
        if was_quiet && matches!(mouse.kind, MouseEventKind::Down(_)) {
            self.focus = if self.prompt_rect.contains((mouse.column, mouse.row).into()) {
                FocusPane::Prompt
            } else {
                FocusPane::Chat
            };
            return true;
        }
        false
    }
}

/// 简洁模式只保留正文与输入区；更新的是显示几何，不改变任务、草稿或队列。
pub(super) fn render(frame: &mut ratatui::Frame<'_>, app: &mut App) {
    let canvas = frame.area();
    let width = canvas.width.saturating_sub(2).max(1) as usize;
    let lines = app.input.visual_line_count(width).clamp(3, 6) as u16;
    let areas = Layout::vertical([Constraint::Min(1), Constraint::Length(lines + 1)]).split(canvas);
    app.transcript_rect = areas[0];
    app.transcript_width = width;
    app.viewport_height = areas[0].height.max(1) as usize;
    let transient = app.prepare_transcript_frame();
    let max = app.max_scroll();
    app.scroll_from_bottom = app.scroll_from_bottom.min(max);
    let offset = max.saturating_sub(app.scroll_from_bottom);
    app.transcript_render_offset = offset;
    frame.render_widget(
        Paragraph::new(app.transcript_viewport(&transient, offset))
            .block(Block::default().padding(Padding::horizontal(1))),
        areas[0],
    );

    app.activity_rect = Rect::default();
    app.sidebar_rect = Rect::default();
    app.prompt_rect = areas[1];
    let (row, col) = app.input.cursor_visual(width);
    let visible = areas[1].height.saturating_sub(1).max(1) as usize;
    app.prompt_scroll = row.saturating_sub(visible - 1);
    let suggestion = app.visible_input_suggestion();
    let body = match suggestion {
        Some(text) => Text::from(Span::styled(text.to_owned(), app.composer_hint_style())),
        None => Text::from(app.input.wrapped_text(width)),
    };
    let style = if app.composer_ready() {
        app.composer_style()
    } else {
        Style::default().fg(Color::Cyan).bg(QUIET_INPUT_BACKGROUND)
    };
    let input = Paragraph::new(body)
        .style(style)
        .block(
            Block::default()
                .borders(Borders::TOP)
                .padding(Padding::horizontal(1))
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .scroll((app.prompt_scroll.min(u16::MAX as usize) as u16, 0));
    let input = if suggestion.is_some() {
        input.wrap(Wrap { trim: false })
    } else {
        input
    };
    frame.render_widget(input, areas[1]);
    if app.focus == FocusPane::Prompt && areas[1].height > 1 && canvas.width > 2 {
        frame.set_cursor_position((
            areas[1].x + 1 + col.min(width - 1) as u16,
            areas[1].y + 1 + row.saturating_sub(app.prompt_scroll) as u16,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle_app() -> App {
        let mut app = App::new(vec!["WillDeep: retained answer".into()], Language::En);
        app.sidebar_visible = false;
        app.last_user_activity = Instant::now() - QUIET_AFTER;
        app
    }

    #[test]
    fn idle_threshold_and_sidebar_are_independent_of_model_progress() {
        let mut app = idle_app();
        let start = Instant::now();
        app.last_user_activity = start;
        assert!(!app.quiet_mode_at(start + Duration::from_millis(4999)));
        assert!(app.quiet_mode_at(start + QUIET_AFTER));
        app.begin_turn(true, "working".into());
        app.stream_transient(narration::StreamKind::Reply, "streaming");
        assert!(app.quiet_mode_at(start + QUIET_AFTER));
        app.sidebar_visible = true;
        assert!(!app.quiet_mode_at(start + Duration::from_secs(500)));
    }

    #[test]
    fn dialogs_and_selection_stay_detailed() {
        let mut app = idle_app();
        app.open_prompt_queue();
        assert!(!app.quiet_mode_at(Instant::now()));
        app.prompt_queue = None;
        app.help_visible = true;
        assert!(!app.quiet_mode_at(Instant::now()));
        app.help_visible = false;
        let (sender, _receiver) = oneshot::channel();
        app.approval = Some(("approval remains visible".into(), false, sender));
        assert!(!app.quiet_mode_at(Instant::now()));
        app.approval = None;
        app.selection_mode = true;
        assert!(!app.quiet_mode_at(Instant::now()));
    }

    #[test]
    fn compact_frame_hides_chrome_preserves_input_and_wakes_on_click() {
        let mut app = idle_app();
        app.input.insert("unsent draft");
        app.queued_prompts.push_back(QueuedPrompt {
            text: "queued follow-up".into(),
            attachments: Vec::new(),
        });
        app.activity_line = "private activity chrome".into();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        let skills = SkillCatalog::default();
        draw(&mut terminal, &mut app, &skills).unwrap();
        assert!(app.quiet_drawn);
        let buffer = terminal.backend().buffer();
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("retained answer"));
        assert!(text.contains("unsent draft"));
        for hidden in [
            "Activity",
            "Workspace write",
            "Your turn",
            "Ready",
            "private activity chrome",
            "┌",
            "└",
            "│",
        ] {
            assert!(!text.contains(hidden), "unexpected chrome: {hidden}");
        }
        assert_eq!(app.activity_rect, Rect::default());
        let prompt = app.prompt_rect;
        assert_ne!(buffer[(1, prompt.y + 1)].bg, buffer[(1, 1)].bg);
        assert!(app.wake_quiet_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: prompt.y + 1,
            modifiers: KeyModifiers::NONE,
        }));
        draw(&mut terminal, &mut app, &skills).unwrap();
        assert!(!app.quiet_drawn);
        assert_eq!(app.focus, FocusPane::Prompt);
        assert_eq!(app.input.text(), "unsent draft");
        assert_eq!(app.queued_prompts.front().unwrap().text, "queued follow-up");
        assert!(app.activity_rect.height > 0);
    }

    #[test]
    fn keyboard_wake_keeps_the_first_character_and_mouse_motion_does_not_wake() {
        let mut app = idle_app();
        app.quiet_drawn = true;
        assert!(!app.wake_quiet_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 2,
            row: 2,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(app.quiet_drawn);
        app.note_user_activity();
        app.handle_paste("first character".into());
        assert!(!app.quiet_mode_at(Instant::now()));
        assert_eq!(app.input.text(), "first character");
    }
}
