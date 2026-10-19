use super::*;
use ratatui::backend::TestBackend;

fn paint(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| human_gate_ui::render(frame, app))
        .unwrap();
    terminal
}

#[tokio::test]
async fn approval_padding_clicks_resolve_the_displayed_action_and_gap_does_nothing() {
    for always in [false, true] {
        for (index, expected) in approval_decisions(always).iter().copied().enumerate() {
            for padding_row in [0, 1, 2] {
                let mut app = App::new(Vec::new(), Language::ZhCn);
                let (tx, mut rx) = oneshot::channel();
                app.approval = Some(("run_command: 探测实例健康与版本".into(), always, tx));
                let terminal = paint(&mut app, 100, 30);
                let (button, _) = app.approval_action_hits[index];
                assert_eq!(button.height, 3);
                if index > 0 {
                    let previous = app.approval_action_hits[index - 1].0;
                    assert_eq!(button.y - previous.bottom(), 1);
                    app.handle_mouse(
                        button.x + 2,
                        previous.bottom(),
                        &BackgroundTaskRegistry::default(),
                        &SkillCatalog::default(),
                    );
                    assert!(matches!(
                        rx.try_recv(),
                        Err(oneshot::error::TryRecvError::Empty)
                    ));
                }
                assert!(terminal.backend().buffer()[(button.x + 3, button.y + 1)].symbol() != " ");
                app.handle_mouse(
                    button.x + 2,
                    button.y + padding_row,
                    &BackgroundTaskRegistry::default(),
                    &SkillCatalog::default(),
                );
                assert_eq!(rx.await.unwrap(), expected);
            }
        }
    }
}

#[test]
fn small_approval_windows_keep_all_actions_inside_the_border() {
    for height in [5, 6, 8, 12, 20] {
        let mut app = App::new(Vec::new(), Language::En);
        let (tx, _rx) = oneshot::channel();
        app.approval = Some(("Run tests".into(), true, tx));
        paint(&mut app, 40, height);
        assert_eq!(app.approval_action_hits.len(), 3, "height={height}");
        for (rect, _) in &app.approval_action_hits {
            assert!(rect.y > app.approval_rect.y);
            assert!(rect.bottom() < app.approval_rect.bottom());
        }
    }
}

fn question(app: &mut App, selected: usize) -> oneshot::Receiver<Option<String>> {
    let (tx, rx) = oneshot::channel();
    app.question = Some(AskDialog {
        request: UserQuestion {
            question: "请选择目标\nWhich target?".into(),
            options: vec![
                "第一个选项 mixed language with a long wrapped description".into(),
                "第二个选项".into(),
                "最后一个选项".into(),
            ],
            multi_select: false,
        },
        selected,
        checked: vec![false; 3],
        answer: PromptEditor::default(),
        sender: tx,
    });
    rx
}

#[tokio::test]
async fn wrapped_question_rows_click_the_correct_option_and_gaps_are_inert() {
    let mut app = App::new(Vec::new(), Language::ZhCn);
    let mut rx = question(&mut app, 0);
    let terminal = paint(&mut app, 40, 30);
    let rows = app.question_hits.clone();
    let first = rows[0].0;
    assert_eq!(
        terminal.backend().buffer()[(app.question_rect.x + 2, first)].bg,
        Color::LightCyan
    );
    let first_end = rows
        .iter()
        .filter(|(_, index)| *index == 0)
        .map(|(y, _)| *y)
        .max()
        .unwrap();
    let second = rows.iter().find(|(_, index)| *index == 1).unwrap().0;
    assert_eq!(second, first_end + 2);
    app.handle_mouse(
        app.question_rect.x + 3,
        first_end + 1,
        &BackgroundTaskRegistry::default(),
        &SkillCatalog::default(),
    );
    assert!(matches!(
        rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    app.handle_mouse(
        app.question_rect.x + 3,
        second,
        &BackgroundTaskRegistry::default(),
        &SkillCatalog::default(),
    );
    assert_eq!(rx.await.unwrap().as_deref(), Some("第二个选项"));
}

#[test]
fn question_selected_option_remains_visible_after_resizing() {
    for height in [5, 8, 12] {
        let mut app = App::new(Vec::new(), Language::En);
        let _rx = question(&mut app, 2);
        paint(&mut app, 32, height);
        assert!(app.question_hits.iter().any(|(_, index)| *index == 2));
        assert!(app.question_hits.iter().all(|(row, _)| *row > app.question_rect.y && *row < app.question_rect.bottom() - 1));
    }
}
