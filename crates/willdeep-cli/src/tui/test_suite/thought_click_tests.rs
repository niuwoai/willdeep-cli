use super::*;
use crate::config::UiMode;
use ratatui::backend::TestBackend;

fn draw_app(app: &mut App, terminal: &mut Terminal<TestBackend>) {
    draw(
        terminal,
        app,
        &SkillCatalog::default(),
        &quiet_test_session(),
    )
    .unwrap();
}

fn click_thought(app: &mut App, y: u16) {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let x = app.transcript_content.x + 1;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        let mouse = MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        if !app.handle_chat_selection_mouse(mouse) && matches!(kind, MouseEventKind::Down(_)) {
            app.handle_mouse(
                x,
                y,
                &BackgroundTaskRegistry::default(),
                &SkillCatalog::default(),
            );
        }
    }
}

fn screen(terminal: &Terminal<TestBackend>) -> String {
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

fn screen_row(terminal: &Terminal<TestBackend>, text: &str) -> u16 {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .find(|&y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains(text)
        })
        .expect("visible text must have a painted row")
}

fn folded_thought_click(mode: UiMode, archived: bool) {
    let mut app = App::new(
        vec![
            "You: old question".into(),
            "· + Thought: 9.0s\nold thought".into(),
            "You: current question".into(),
            "· + Thought: 0.8s\nearlier hidden thought".into(),
            "· ✓ read_file".into(),
            "· + Thought: 0.5s\nclicked latest thought".into(),
        ],
        Language::En,
    );
    app.ui.mode = mode;
    app.ui.show_thinking = true;
    app.ui.line_spacing = 2;
    if archived {
        app.archive_floor = 2;
    }
    let original = app.transcript.clone();
    let mut terminal = Terminal::new(TestBackend::new(110, 42)).unwrap();
    draw_app(&mut app, &mut terminal);
    assert!(screen(&terminal).contains("+ Thought: 0.5s"));
    assert!(!screen(&terminal).contains("Thought: 0.8s"));
    assert_eq!(app.thought_hit_rows.len(), 1);
    let y = screen_row(&terminal, "Thought: 0.5s");
    assert_eq!(app.thought_hit_rows[0].0, y);
    click_thought(&mut app, y);
    draw_app(&mut app, &mut terminal);
    assert!(
        app.transcript[5].starts_with("· − Thought: 0.5s"),
        "clicked 0.5s must expand its own record"
    );
    assert_eq!(app.transcript[3], original[3]);
    let shown = screen(&terminal);
    assert!(shown.contains("clicked latest thought"));
    assert!(!shown.contains("earlier hidden thought"));
    assert_eq!(
        app.thought_hit_rows.len(),
        1,
        "a click must not reveal a second thought"
    );
    let y = screen_row(&terminal, "Thought: 0.5s");
    assert_eq!(app.thought_hit_rows[0].0, y);
    click_thought(&mut app, y);
    draw_app(&mut app, &mut terminal);
    assert_eq!(
        app.transcript, original,
        "collapsing restores only the clicked header"
    );
    assert!(!screen(&terminal).contains("clicked latest thought"));
}

#[test]
fn full_layout_clicks_the_visible_folded_thought() {
    for archived in [false, true] {
        folded_thought_click(UiMode::Full, archived);
    }
}

#[test]
fn compact_layout_clicks_the_visible_folded_thought() {
    for archived in [false, true] {
        folded_thought_click(UiMode::Compact, archived);
    }
}

#[test]
fn expanded_thought_body_cannot_create_another_click_target() {
    for mode in [UiMode::Full, UiMode::Compact] {
        let mut app = App::new(
            vec![
                "You: question".into(),
                "· − Thought: 0.8s\nbody mentions + Thought: 0.5s".into(),
                "· ✓ read_file".into(),
                "· + Thought: 0.5s\nlatest body".into(),
            ],
            Language::En,
        );
        app.ui.mode = mode;
        app.ui.show_thinking = true;
        let mut terminal = Terminal::new(TestBackend::new(110, 42)).unwrap();
        draw_app(&mut app, &mut terminal);
        assert_eq!(
            app.thought_hit_rows.len(),
            2,
            "only real headers can be toggled"
        );
        let (y, _) = *app.thought_hit_rows.last().unwrap();
        click_thought(&mut app, y);
        draw_app(&mut app, &mut terminal);
        assert!(app.transcript[3].starts_with("· − Thought:"));
        assert!(app.transcript[1].starts_with("· − Thought:"));
    }
}

#[test]
fn wrapped_thought_title_continuation_is_clickable() {
    for mode in [UiMode::Full, UiMode::Compact] {
        let mut app = App::new(
            vec!["· + Thought: 0.5s\nbody stays separate".into()],
            Language::En,
        );
        app.ui.mode = mode;
        app.ui.show_thinking = true;
        let mut terminal = Terminal::new(TestBackend::new(14, 42)).unwrap();
        draw_app(&mut app, &mut terminal);
        let y = screen_row(&terminal, "0.5s");
        assert!(app.thought_hit_rows.len() > 1, "title must really wrap");
        assert!(y > app.thought_hit_rows[0].0);
        click_thought(&mut app, y);
        assert!(app.transcript[0].starts_with("· − Thought:"));
        draw_app(&mut app, &mut terminal);
        let y = screen_row(&terminal, "0.5s");
        click_thought(&mut app, y);
        assert!(app.transcript[0].starts_with("· + Thought:"));
    }
}

#[test]
fn unpainted_removal_or_reorder_cannot_toggle_the_wrong_thought() {
    for mode in [UiMode::Full, UiMode::Compact] {
        for remove in [false, true] {
            let mut app = App::new(
                vec![
                    "You: question".into(),
                    "· − Thought: 0.8s\nfirst body".into(),
                    "· − Thought: 0.5s\nsecond body".into(),
                ],
                Language::En,
            );
            app.ui.mode = mode;
            app.ui.show_thinking = true;
            let mut terminal = Terminal::new(TestBackend::new(110, 42)).unwrap();
            draw_app(&mut app, &mut terminal);
            let y = screen_row(&terminal, "Thought: 0.8s");
            if remove {
                app.drop_transcript_row(0);
            } else {
                app.move_transcript_row_to_end(1);
            }
            let before = app.transcript.clone();
            assert!(
                !app.toggle_thought_at(y),
                "old frame must not keep stale indices"
            );
            assert_eq!(app.transcript, before);
            draw_app(&mut app, &mut terminal);
            let y = screen_row(&terminal, "Thought: 0.8s");
            click_thought(&mut app, y);
            let first = app
                .transcript
                .iter()
                .find(|row| row.ends_with("first body"))
                .unwrap();
            let second = app
                .transcript
                .iter()
                .find(|row| row.ends_with("second body"))
                .unwrap();
            assert!(first.starts_with("· + Thought: 0.8s"));
            assert!(second.starts_with("· − Thought: 0.5s"));
        }
    }
}

#[test]
fn scrolled_thought_click_uses_its_record_after_hidden_thoughts() {
    for mode in [UiMode::Full, UiMode::Compact] {
        let mut app = App::new(
            vec![
                "You: question".into(),
                "· + Thought: 0.8s\nhidden".into(),
                "· ✓ read_file".into(),
                "· + Thought: 0.5s\nlatest body".into(),
                format!("Assistant: {}", "answer line\n".repeat(70)),
            ],
            Language::En,
        );
        app.ui.mode = mode;
        app.ui.show_thinking = true;
        let mut terminal = Terminal::new(TestBackend::new(90, 28)).unwrap();
        draw_app(&mut app, &mut terminal);
        app.scroll_to_top();
        draw_app(&mut app, &mut terminal);
        assert!(!app.thought_hit_rows.is_empty());
        let (y, _) = app.thought_hit_rows[0];
        click_thought(&mut app, y);
        draw_app(&mut app, &mut terminal);
        assert!(app.transcript[3].starts_with("· − Thought:"));
        assert!(app.transcript[1].starts_with("· + Thought:"));
    }
}
