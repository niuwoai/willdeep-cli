use super::*;
use std::sync::OnceLock;

const INPUT_BREATH_PERIOD: Duration = Duration::from_secs(8);
const INPUT_BREATH_PEAK_START: Duration = Duration::from_secs(5);
const INPUT_BREATH_PEAK_END: Duration = Duration::from_millis(6_500);

fn idle_input_background(app: &App, elapsed: Duration) -> Color {
    let can_breathe = app.composer_ready()
        && app.input.is_empty()
        && app.visible_input_suggestion().is_none()
        && !app.has_open_overlay();
    if !can_breathe {
        return COMPOSER_BACKGROUND;
    }

    let phase_ms = elapsed.as_millis() % INPUT_BREATH_PERIOD.as_millis();
    if (INPUT_BREATH_PEAK_START.as_millis()..INPUT_BREATH_PEAK_END.as_millis()).contains(&phase_ms)
    {
        Color::DarkGray
    } else {
        COMPOSER_BACKGROUND
    }
}

fn current_idle_input_background(app: &App) -> Color {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    idle_input_background(app, EPOCH.get_or_init(Instant::now).elapsed())
}

/// Fixed compact UI. Dialogs and the sidebar are rendered by the common layer.
/// The composer and optional footer always span the whole terminal, including padding.
pub(super) fn render(
    frame: &mut ratatui::Frame<'_>,
    app: &mut App,
    chat_canvas: Rect,
) -> std::rc::Rc<[Rect]> {
    let canvas = frame.area();
    // Hiding routine statistics must not hide a warning or action result.
    let show_status = app.ui.compact_status_line || app.notice.is_some();
    let width = canvas.width.saturating_sub(2).max(1) as usize;
    let lines = if app.composer_expanded {
        canvas.height.saturating_sub(3)
    } else {
        app.input.visual_line_count(width).clamp(3, 6) as u16
    };
    let areas = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(0),
        Constraint::Length(u16::from(!app.attachments.is_empty())),
        Constraint::Length(lines + 1),
        Constraint::Length(u16::from(show_status)),
    ])
    .split(canvas);
    app.transcript_rect = Rect {
        width: chat_canvas.width,
        ..areas[0]
    };
    app.transcript_width = app.transcript_rect.width.saturating_sub(2).max(1) as usize;
    app.viewport_height = app.transcript_rect.height.max(1) as usize;
    let transient = app.prepare_transcript_frame();
    let max = app.max_scroll();
    app.scroll_from_bottom = app.scroll_from_bottom.min(max);
    let offset = max.saturating_sub(app.scroll_from_bottom);
    app.transcript_render_offset = offset;
    frame.render_widget(
        Paragraph::new(app.transcript_viewport(&transient, offset))
            .block(Block::default().padding(Padding::horizontal(1))),
        app.transcript_rect,
    );

    app.activity_rect = Rect::default();
    app.prompt_rect = areas[3];
    let (row, col) = app.input.cursor_visual(width);
    let visible = areas[3].height.saturating_sub(1).max(1) as usize;
    app.prompt_scroll = row.saturating_sub(visible - 1);
    let suggestion = app.visible_input_suggestion();
    let show_placeholder = suggestion.is_none()
        && app.composer_ready()
        && app.input.is_empty()
        && !app.composer_placeholder_dismissed;
    let body = match suggestion {
        Some(text) => Text::from(Span::styled(text.to_owned(), app.composer_hint_style())),
        None if show_placeholder => Text::from(Span::styled(
            match app.language {
                Language::ZhCn => "请说出你的需求",
                Language::Ja => "ご要望を入力してください",
                Language::En => "Tell me what you need",
            },
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC),
        )),
        None => Text::from(app.input.wrapped_text(width)),
    };
    let style = if app.composer_ready() {
        app.composer_style().bg(current_idle_input_background(app))
    } else {
        Style::default().fg(Color::Cyan).bg(COMPOSER_BACKGROUND)
    };
    // Paint the complete rectangle first: text length and line wrapping must not
    // determine background coverage at the right edge or on empty rows.
    let bottom = Rect {
        y: areas[2].y,
        height: canvas.bottom().saturating_sub(areas[2].y),
        ..canvas
    };
    frame.render_widget(Block::default().style(style), bottom);
    if !app.attachments.is_empty() {
        let items = app
            .attachments
            .iter()
            .map(|item| item.summary())
            .collect::<Vec<_>>()
            .join("  ");
        frame.render_widget(
            Paragraph::new(format!("F3 · {items}")).style(style),
            areas[2],
        );
    }
    let input = Paragraph::new(body)
        .style(style)
        .block(
            Block::default()
                .borders(Borders::TOP)
                // Use a standard one-pixel stroke; the top border cell shares
                // the input background so the divider meets the transcript cleanly.
                .border_set(ratatui::symbols::border::PLAIN)
                .padding(Padding::horizontal(1))
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .scroll((app.prompt_scroll.min(u16::MAX as usize) as u16, 0));
    let input = if suggestion.is_some() || show_placeholder {
        input.wrap(Wrap { trim: false })
    } else {
        input
    };
    frame.render_widget(input, areas[3]);
    if show_status {
        frame.render_widget(
            Paragraph::new(app.status_line()).style(style.fg(Color::Gray)),
            areas[4],
        );
    }
    if app.focus == FocusPane::Prompt
        && !app.has_open_overlay()
        && areas[3].height > 1
        && canvas.width > 2
    {
        frame.set_cursor_position((
            areas[3].x + 1 + col.min(width - 1) as u16,
            areas[3].y + 1 + row.saturating_sub(app.prompt_scroll) as u16,
        ));
    }
    areas
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compact_app() -> App {
        let mut app = App::new(vec!["WillDeep: retained answer".into()], Language::En);
        app.ui.mode = crate::config::UiMode::Compact;
        app
    }

    fn screen(terminal: &Terminal<ratatui::backend::TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn full_ui_is_default_and_compact_does_not_change_on_input() {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        let mut app = App::new(vec![], Language::En);
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        assert!(!app.quiet_drawn);
        assert!(screen(&terminal).contains("Activity"));
        app.ui.mode = crate::config::UiMode::Compact;
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        assert!(app.quiet_drawn);
        app.handle_paste("first character".into());
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        assert!(app.quiet_drawn);
        assert_eq!(app.input.text(), "first character");
        assert!(!screen(&terminal).contains("Activity"));
    }

    #[test]
    fn input_separator_sits_on_the_top_edge_of_the_background() {
        for running in [false, true] {
            for footer in [false, true] {
                let mut app = compact_app();
                app.running = running;
                app.ui.compact_status_line = footer;
                let mut terminal =
                    Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
                for (width, height) in [(120, 30), (45, 18), (160, 40)] {
                    terminal.backend_mut().resize(width, height);
                    terminal.resize(Rect::new(0, 0, width, height)).unwrap();
                    draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
                    let buffer = terminal.backend().buffer();
                    let top = app.prompt_rect.y;
                    for x in 0..width {
                        let separator = &buffer[(x, top)];
                        assert_eq!(separator.symbol(), "─");
                        assert_eq!(separator.fg, Color::DarkGray);
                        assert_eq!(separator.bg, buffer[(x, top + 1)].bg);
                        assert_ne!(separator.bg, buffer[(x, top - 1)].bg);
                    }
                }
            }
        }
    }

    #[test]
    fn idle_input_background_breathes_slowly_and_stops_for_interaction() {
        let mut app = compact_app();
        assert_eq!(
            idle_input_background(&app, Duration::from_secs(1)),
            COMPOSER_BACKGROUND
        );
        assert_eq!(
            idle_input_background(&app, Duration::from_secs(5)),
            Color::DarkGray
        );
        assert_eq!(
            idle_input_background(&app, Duration::from_secs(7)),
            COMPOSER_BACKGROUND
        );

        app.input.insert("draft");
        assert_eq!(
            idle_input_background(&app, Duration::from_secs(5)),
            COMPOSER_BACKGROUND
        );
        app.input.take();
        app.begin_turn(false, "working".into());
        assert_eq!(
            idle_input_background(&app, Duration::from_secs(5)),
            COMPOSER_BACKGROUND
        );
    }

    #[test]
    fn bottom_background_covers_all_columns_after_resize_with_or_without_footer() {
        for running in [false, true] {
            for footer in [false, true] {
                let mut app = compact_app();
                app.running = running;
                app.ui.compact_status_line = footer;
                app.input.insert("unsent draft");
                let mut terminal =
                    Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
                for (width, height) in [(120, 30), (45, 18), (160, 40), (8, 4)] {
                    terminal.backend_mut().resize(width, height);
                    terminal.resize(Rect::new(0, 0, width, height)).unwrap();
                    draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
                    let buffer = terminal.backend().buffer();
                    let bg = buffer[(0, height - 1)].bg;
                    assert_eq!(bg, COMPOSER_BACKGROUND);
                    for y in app.prompt_rect.y..height {
                        for x in 0..width {
                            assert_eq!(
                                buffer[(x, y)].bg,
                                bg,
                                "unpainted ({x}, {y}), {width}x{height}"
                            );
                        }
                    }
                    assert_eq!(app.prompt_rect.width, width);
                    assert_eq!(app.input.text(), "unsent draft");
                }
            }
        }
    }

    #[test]
    fn placeholder_shows_only_when_idle_and_hides_after_click_or_typing() {
        let mut app = compact_app();
        app.language = Language::ZhCn;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 24)).unwrap();
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        let rendered = screen(&terminal).replace(' ', "");
        assert!(
            rendered.contains("请说出你的需求"),
            "rendered screen: {rendered}"
        );

        app.handle_mouse(
            app.prompt_rect.x + 5,
            app.prompt_rect.y + 1,
            &BackgroundTaskRegistry::default(),
            &SkillCatalog::default(),
        );
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        assert!(
            !screen(&terminal)
                .replace(' ', "")
                .contains("请说出你的需求")
        );

        app.handle_paste("帮我修复这个问题".into());
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        let rendered = screen(&terminal).replace(' ', "");
        assert!(rendered.contains("帮我修复这个问题"));
        assert!(!rendered.contains("请说出你的需求"));
    }

    #[test]
    fn optional_footer_uses_the_full_ui_information_and_stays_on_the_last_row() {
        let mut app = compact_app();
        app.context_tokens = 34560;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(180, 30)).unwrap();
        for language in [Language::ZhCn, Language::En, Language::Ja] {
            app.language = language;
            app.ui.mode = crate::config::UiMode::Full;
            draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
            let expected: Vec<_> = (0..180)
                .map(|x| terminal.backend().buffer()[(x, 29)].symbol().to_owned())
                .collect();
            app.ui.mode = crate::config::UiMode::Compact;
            draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
            let buffer = terminal.backend().buffer();
            let last: Vec<_> = (0..180)
                .map(|x| buffer[(x, 29)].symbol().to_owned())
                .collect();
            assert_eq!(last, expected);
            app.ui.compact_status_line = false;
            draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
            assert!(!screen(&terminal).contains("27%"));
            app.ui.compact_status_line = true;
        }
    }

    #[test]
    fn compact_ui_keeps_sidebar_queue_and_approval_operable_without_switching_mode() {
        let mut app = compact_app();
        app.input.insert("unsent draft");
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(160, 30)).unwrap();
        app.sidebar_visible = true;
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        assert!(app.quiet_drawn);
        assert!(app.sidebar_rect.width > 0);
        assert_eq!(app.prompt_rect.width, 160);
        assert!(app.sidebar_rect.bottom() <= app.prompt_rect.y);
        app.open_prompt_queue();
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        assert!(app.quiet_drawn);
        assert!(screen(&terminal).to_lowercase().contains("queue"));
        app.prompt_queue = None;
        let (sender, _receiver) = oneshot::channel();
        app.approval = Some(("approval remains visible".into(), false, sender));
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        assert!(screen(&terminal).contains("approval remains visible"));
        assert_eq!(app.input.text(), "unsent draft");
        assert!(app.quiet_drawn);
    }

    #[test]
    fn hidden_statistics_do_not_hide_important_notices() {
        let mut app = compact_app();
        app.ui.compact_status_line = false;
        app.notice = Some("Important action result".into());
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        assert!(app.quiet_drawn);
        assert!(screen(&terminal).contains("Important action result"));
        assert!(app.notice.is_none());
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        assert!(!screen(&terminal).contains("Important action result"));
    }
}
