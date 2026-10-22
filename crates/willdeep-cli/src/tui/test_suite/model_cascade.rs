use super::*;

fn picker() -> App {
    let mut app = App::new(Vec::new(), Language::En);
    app.open_model_picker("alpha-current".into());
    let state = app.model_picker.as_mut().unwrap();
    state.active_profile = Some("alpha".into());
    state.profile = Some("alpha".into());
    state.profiles = vec!["alpha".into(), "beta".into()];
    app.refresh_provider_matches();
    app.set_model_picker_result(Ok(vec!["alpha-first".into(), "alpha-current".into()]));
    app
}

fn key(app: &mut App, code: KeyCode) -> ModelPickerAction {
    app.handle_model_picker_key(KeyEvent::new(code, KeyModifiers::NONE))
}

#[test]
fn opens_on_current_model_and_tabs_link_provider_highlight_without_committing() {
    let mut app = picker();
    assert_eq!(app.model_picker.as_ref().unwrap().selected, 1);
    assert!(!app.model_picker.as_ref().unwrap().choosing_provider);
    key(&mut app, KeyCode::Tab);
    assert!(
        matches!(key(&mut app, KeyCode::Down), ModelPickerAction::Preview(name) if name == "beta")
    );
    assert_eq!(
        app.model_picker.as_ref().unwrap().active_profile.as_deref(),
        Some("alpha")
    );
    assert!(
        matches!(key(&mut app, KeyCode::Enter), ModelPickerAction::Provider(Some(name)) if name == "beta")
    );
}

#[test]
fn provider_and_model_searches_are_independent_and_cache_survives_refresh_failure() {
    let mut app = picker();
    key(&mut app, KeyCode::Char('c'));
    key(&mut app, KeyCode::Tab);
    assert!(
        matches!(key(&mut app, KeyCode::Char('b')), ModelPickerAction::Preview(name) if name == "beta")
    );
    let state = app.model_picker.as_ref().unwrap();
    assert_eq!(state.editor.text(), "c");
    assert_eq!(state.provider_editor.text(), "b");
    assert_eq!(state.provider_filtered, vec![1]);
    key(&mut app, KeyCode::Tab);
    app.set_model_picker_result(Err("offline".into()));
    assert!(
        matches!(key(&mut app, KeyCode::Enter), ModelPickerAction::Select(name) if name == "alpha-current")
    );
    assert_eq!(
        app.model_picker.as_ref().unwrap().cache[&Some("alpha".to_owned())].len(),
        2
    );
}

#[test]
fn mouse_hit_regions_distinguish_columns_and_wheels_follow_the_pointer() {
    let mut app = picker();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
    terminal.draw(|f| render_model_picker(f, &mut app)).unwrap();
    let provider = app.model_provider_hits[1].0;
    let model = app.model_picker_hits[1].0;
    assert!(!provider.intersects(model));
    assert!(
        matches!(app.activate_model_picker_at(provider.x, provider.y), Some(ModelPickerAction::Preview(name)) if name == "beta")
    );
    assert!(
        matches!(app.activate_model_picker_at(model.x, model.y), Some(ModelPickerAction::Select(name)) if name == "alpha-current")
    );
    assert!(
        matches!(app.model_picker_scroll_at(1, provider.x, provider.y), ModelPickerAction::Preview(name) if name == "beta")
    );
    assert!(app.model_picker.as_ref().unwrap().choosing_provider);
    app.model_picker_scroll_at(-1, model.x, model.y);
    assert!(!app.model_picker.as_ref().unwrap().choosing_provider);
    assert_eq!(app.model_picker.as_ref().unwrap().selected, 0);
}

#[test]
fn narrow_terminal_esc_returns_to_provider_and_mouse_enters_models() {
    let mut app = picker();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(44, 16)).unwrap();
    terminal.draw(|f| render_model_picker(f, &mut app)).unwrap();
    assert!(!app.model_picker.as_ref().unwrap().wide);
    assert!(app.model_provider_hits.is_empty());
    assert!(matches!(
        key(&mut app, KeyCode::Esc),
        ModelPickerAction::None
    ));
    terminal.draw(|f| render_model_picker(f, &mut app)).unwrap();
    assert!(app.model_picker_hits.is_empty());
    let beta = app.model_provider_hits[1].0;
    assert!(
        matches!(app.activate_model_picker_at(beta.x, beta.y), Some(ModelPickerAction::Provider(Some(name))) if name == "beta")
    );
    assert!(matches!(
        key(&mut app, KeyCode::Esc),
        ModelPickerAction::Close
    ));
}

#[test]
fn resized_short_windows_keep_every_hit_and_cursor_inside_the_terminal() {
    let mut app = picker();
    for (width, height) in [
        (120, 35),
        (76, 12),
        (75, 12),
        (40, 10),
        (24, 8),
        (12, 4),
        (1, 1),
    ] {
        for provider_focus in [false, true] {
            app.model_picker.as_mut().unwrap().choosing_provider = provider_focus;
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| render_model_picker(f, &mut app)).unwrap();
            let area = Rect::new(0, 0, width, height);
            for (hit, _) in app.model_provider_hits.iter().chain(&app.model_picker_hits) {
                assert_eq!(hit.intersection(area), *hit);
            }
            let cursor = terminal.get_cursor_position().unwrap();
            assert!(cursor.x < width && cursor.y < height);
        }
    }
}

#[test]
fn wide_esc_cancels_without_changing_the_active_provider_or_model() {
    let mut app = picker();
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Down);
    assert!(matches!(
        key(&mut app, KeyCode::Esc),
        ModelPickerAction::Close
    ));
    let state = app.model_picker.as_ref().unwrap();
    assert_eq!(state.active_profile.as_deref(), Some("alpha"));
    assert_eq!(state.current_model, "alpha-current");
}
