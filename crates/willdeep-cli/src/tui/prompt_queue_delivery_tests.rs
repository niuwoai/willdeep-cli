use super::*;

fn remote(held: bool) -> willdeep_runtime_protocol::QueuedTurnMessage {
    willdeep_runtime_protocol::QueuedTurnMessage {
        id: uuid::Uuid::new_v4(),
        session_id: uuid::Uuid::new_v4(),
        text: "a queued message".into(),
        text_preview: "a queued message".into(),
        can_edit: true,
        image_count: 1,
        text_attachment_count: 1,
        held,
        priority_requested: true,
        independence_confirmed: true,
    }
}

#[test]
fn send_now_labels_match_active_runtime_human_wait_idle_held_and_local_actions() {
    for language in [Language::ZhCn, Language::En, Language::Ja] {
        let mut app = App::new(Vec::new(), language);
        app.sync_runtime_queue(vec![remote(true)]);
        app.open_prompt_queue();
        assert_eq!(
            send_now_label(&app),
            language.text("S 立即开始", "S Start now", "S 今すぐ開始")
        );
        app.begin_turn(true, "working".into());
        assert_eq!(
            send_now_label(&app),
            language.text(
                "S 优先发送（不打断当前轮）",
                "S Prioritize (keeps current turn)",
                "S 優先送信（現在のターンを停止しない）",
            )
        );
        app.runtime_gates.push(crate::daemon::RemoteGate::Question {
            parked: true,
            id: uuid::Uuid::new_v4(),
            task_id: uuid::Uuid::new_v4(),
            question: "Keep original request pending".into(),
            options: Vec::new(),
            multi_select: false,
        });
        assert_eq!(
            send_now_label(&app),
            language.text("S 优先处理", "S Prioritize", "S 優先処理")
        );
        app.queued_prompts[0].remote = None;
        assert_eq!(
            send_now_label(&app),
            language.text("S 插话", "S Steer", "S 割り込み")
        );
    }
}

#[test]
fn held_priority_and_independence_render_in_both_layouts_and_all_languages() {
    for language in [Language::ZhCn, Language::En, Language::Ja] {
        for mode in [crate::config::UiMode::Full, crate::config::UiMode::Compact] {
            let mut app = App::new(Vec::new(), language);
            app.ui.mode = mode;
            app.sync_runtime_queue(vec![remote(true)]);
            app.open_prompt_queue();
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(110, 34)).unwrap();
            draw(
                &mut terminal,
                &mut app,
                &SkillCatalog::default(),
                &quiet_test_session(),
            )
            .unwrap();
            let screen: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            let compact: String = screen.chars().filter(|ch| !ch.is_whitespace()).collect();
            for text in [
                language.text("暂缓", "held", "保留中"),
                language.text("不会自动发送", "will not auto-send", "自動送信なし"),
                language.text("优先处理", "priority requested", "優先処理を要求済み"),
                language.text(
                    "已确认独立处理",
                    "independent work confirmed",
                    "独立処理を確認済み",
                ),
            ] {
                let expected: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
                assert!(compact.contains(&expected), "missing {text:?}: {screen}");
            }
            assert!(app.queue_activity().unwrap().contains(language.text(
                "暂缓",
                "Held",
                "保留中"
            )));
            assert!(
                app.prompt_queue
                    .as_ref()
                    .unwrap()
                    .hits
                    .iter()
                    .any(|(_, key)| *key == KeyCode::Char('s'))
            );
        }
    }
}

#[test]
fn queue_edit_remains_visible_after_the_remote_message_disappears() {
    let mut app = App::new(Vec::new(), Language::En);
    app.sync_runtime_queue(vec![remote(false)]);
    app.open_prompt_queue();
    app.handle_queue_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.prompt_queue
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .insert(" keep rewrite");
    app.sync_runtime_queue(Vec::new());
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(110, 34)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("keep rewrite"), "{screen}");
    assert!(app.prompt_queue.as_ref().unwrap().remote_edit.is_some());
    assert!(app.queued_prompts.is_empty());
    assert!(!app.handle_queue_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(app.prompt_queue.as_ref().unwrap().editor.is_some());
    assert!(app.input.is_empty());
}
