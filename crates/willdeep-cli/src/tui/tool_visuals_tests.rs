use super::*;
use serde_json::json;

fn app() -> App {
    let mut app = App::new(Vec::new(), Language::En);
    app.transcript_width = 100;
    app.viewport_height = 60;
    // Tool calls only happen inside a turn; automatic projections follow it.
    app.running = true;
    app
}

fn screen(app: &mut App) -> String {
    app.prepare_transcript_frame();
    app.transcript_rows.join("\n")
}

fn request(app: &mut App, id: &str, command: &str) {
    app.note_tool_call_requested(
        Some(id),
        "run_command",
        Some("short summary"),
        Some(command.to_owned()),
    );
}

fn complete(app: &mut App, id: &str, is_error: bool) {
    app.note_tool_call_completed(Some(id), "run_command", None, None, is_error, None);
}

fn edit() -> serde_json::Value {
    json!({"files":[{"path":"src/example.rs", "added":1,"removed":1,"truncated":false,
        "diff":"@@ -7,3 +7,3 @@\n context before\n-old value\n+new value\n context after\n"}]})
}

#[test]
fn only_latest_pending_command_expands_and_out_of_order_completion_matches_id() {
    let mut app = app();
    request(&mut app, "first", "FIRST_FULL_COMMAND");
    request(&mut app, "second", "SECOND_FULL_COMMAND");
    let shown = screen(&mut app);
    assert!(shown.contains("SECOND_FULL_COMMAND"), "{shown}");
    assert!(!shown.contains("FIRST_FULL_COMMAND"), "{shown}");
    complete(&mut app, "first", false);
    assert!(app.transcript[0].starts_with("· ✓"));
    assert!(app.transcript[1].starts_with("· …"));
    assert!(screen(&mut app).contains("SECOND_FULL_COMMAND"));
    complete(&mut app, "second", true);
    let shown = screen(&mut app);
    assert!(!shown.contains("FULL_COMMAND"), "{shown}");
    assert!(app.transcript[1].starts_with("· ✗"));
}

#[test]
fn latest_finishing_returns_to_the_remaining_command_and_duplicates_do_not_append() {
    let mut app = app();
    request(&mut app, "one", "COMMAND_ONE");
    request(&mut app, "two", "COMMAND_TWO");
    complete(&mut app, "two", false);
    assert!(screen(&mut app).contains("COMMAND_ONE"));
    let count = app.transcript.len();
    complete(&mut app, "two", false);
    request(&mut app, "two", "COMMAND_TWO");
    assert_eq!(app.transcript.len(), count);
    complete(&mut app, "one", false);
    assert!(!screen(&mut app).contains("COMMAND_ONE"));
}

#[test]
fn manual_history_remains_open_and_current_can_be_explicitly_collapsed() {
    let mut app = app();
    request(&mut app, "one", "HISTORIC_DETAIL");
    complete(&mut app, "one", false);
    app.toggle_tool_projection(0);
    request(&mut app, "two", "CURRENT_DETAIL");
    let shown = screen(&mut app);
    assert!(
        shown.contains("HISTORIC_DETAIL") && shown.contains("CURRENT_DETAIL"),
        "{shown}"
    );
    app.toggle_tool_projection(1);
    assert!(!screen(&mut app).contains("CURRENT_DETAIL"));
    complete(&mut app, "two", false);
    assert!(screen(&mut app).contains("HISTORIC_DETAIL"));
}

#[test]
fn unknown_identified_completion_does_not_settle_a_same_name_call() {
    let mut app = app();
    request(&mut app, "one", "STILL_PENDING");
    complete(&mut app, "unseen", false);
    assert!(app.transcript[0].starts_with("· …"));
    assert!(screen(&mut app).contains("STILL_PENDING"));
}

#[test]
fn interrupted_turn_collapses_commands_without_claiming_tool_success() {
    let mut app = app();
    app.running = true;
    request(&mut app, "one", "PENDING_DETAIL");
    app.note_turn_outcome(turn_status::TurnOutcome::Interrupted);
    assert!(!screen(&mut app).contains("PENDING_DETAIL"));
    assert!(app.transcript[0].starts_with("· …"));
    app.finish_turn();
    assert!(!app.tool_row_open(0));
}

#[test]
fn committed_diff_is_visible_with_line_numbers_colors_and_literal_source() {
    for width in [32, 100] {
        for language in [Language::ZhCn, Language::En, Language::Ja] {
            let mut app = app();
            app.running = true;
            app.transcript_width = width;
            app.language = language;
            app.note_tool_call_requested(Some("edit"), "edit_file", None, None);
            app.note_tool_call_completed(
                Some("edit"),
                "edit_file",
                None,
                None,
                false,
                Some(&edit()),
            );
            request(&mut app, "next", "NEXT_COMMAND");
            let shown = screen(&mut app);
            assert!(
                shown.contains("example.rs") && shown.contains("+1 −1"),
                "{shown}"
            );
            assert!(
                shown.contains("-old value") && shown.contains("+new value"),
                "{shown}"
            );
            assert!(shown.contains('8'), "{shown}");
            let detail = &app.tool_expansions[&0];
            assert!(detail.contains("    8 │       │ -old value"));
            assert!(detail.contains("      │     8 │ +new value"));
            let styled = detail_lines(detail, true);
            assert!(
                styled
                    .lines
                    .iter()
                    .any(|line| line.style.fg == Some(Color::Red))
            );
            assert!(
                styled
                    .lines
                    .iter()
                    .any(|line| line.style.fg == Some(Color::Green))
            );
            app.toggle_tool_projection(0);
            assert!(!screen(&mut app).contains("-old value"));
        }
    }
}

#[test]
fn committed_diff_preserves_context_indentation_spacing_and_redaction() {
    let mut display = edit();
    display["files"][0]["diff"] = concat!(
        "@@ -1,4 +1,4 @@\n",
        " context  before  \n",
        "-    old_value = (left,  right);  \n",
        "+    new_value = (left,  right);  \n",
        "     API_KEY=example  trailing  \n",
        " context  after\n",
    )
    .into();
    let expected = concat!(
        "src/example.rs  +1 −1\n",
        "@@ -1,4 +1,4 @@\n",
        "    1 │     1 │  context  before  \n",
        "    2 │       │ -    old_value = (left,  right);  \n",
        "      │     2 │ +    new_value = (left,  right);  \n",
        "    3 │     3 │      API_KEY=[REDACTED]  trailing  \n",
        "    4 │     4 │  context  after",
    );
    let mut app = app();
    app.running = true;
    app.note_tool_call_completed(Some("edit"), "edit_file", None, None, false, Some(&display));
    let detail = &app.tool_expansions[&0];
    assert_eq!(detail, expected);
    let styled = detail_lines(detail, true);
    assert_eq!(styled.lines[2].style.fg, Some(Color::Gray));
    assert_eq!(styled.lines[3].style.fg, Some(Color::Red));
    assert_eq!(styled.lines[4].style.fg, Some(Color::Green));
    assert_eq!(styled.lines[5].style.fg, Some(Color::Gray));
    let shown = screen(&mut app);
    assert!(shown.contains("│  context  before"), "{shown}");
    assert!(
        shown.contains("+    new_value = (left,  right);"),
        "{shown}"
    );
    assert!(shown.contains("API_KEY=[REDACTED]  trailing"), "{shown}");
    assert!(!shown.contains("API_KEY=example"), "{shown}");
}

#[test]
fn literal_diff_escapes_controls_after_redacting_without_flattening() {
    assert_eq!(
        safe_text(" \tAPI_KEY=example  \u{1b}[31m\r\n"),
        "    API_KEY=[REDACTED]  \\u{1b}[31m\\r\\n",
    );
}

#[test]
fn errors_and_previews_never_render_successful_diffs() {
    let display = edit();
    assert!(
        receipt(
            "edit_file",
            &json!({"edit_display": display}).to_string(),
            true
        )
        .is_none()
    );
    assert!(
        receipt(
            "run_command",
            &json!({"edit_display": edit()}).to_string(),
            false
        )
        .is_none()
    );
    assert!(
        receipt(
            "file_changes",
            r#"{"status":"preview","changes":[]}"#,
            false
        )
        .is_none()
    );
    let mut app = app();
    app.note_tool_call_completed(Some("failed"), "edit_file", None, None, true, Some(&edit()));
    assert!(!screen(&mut app).contains("old value"));
}

#[test]
fn display_bounds_controls_and_localized_truncation_are_explicit() {
    for language in [Language::ZhCn, Language::En, Language::Ja] {
        let mut display = edit();
        display["files"][0]["truncated"] = true.into();
        display["files"][0]["path"] = "name\u{1b}[31m.rs".into();
        let formatted = format_edit(&display, language).unwrap();
        assert!(!formatted.contains('\u{1b}'));
        assert!(formatted.contains(language.text(
            "差异已截断",
            "Diff truncated",
            "差分は省略されています"
        )));
        display["files"][0]["diff"] = "x".repeat(128 * 1024).into();
        assert!(format_edit(&display, language).is_none());
    }
}

#[test]
fn large_committed_diff_elides_middle_lines_with_localized_marker() {
    let mut body = String::from("@@ -1,200 +1,200 @@\n context\n");
    for i in 0..120 {
        body.push_str(&format!("-{i}\n+{i}\n"));
    }
    let display = json!({
        "files": [{
            "path": "src/large.rs",
            "added": 120,
            "removed": 120,
            "truncated": false,
            "diff": body,
        }]
    });
    for language in [Language::ZhCn, Language::En, Language::Ja] {
        let formatted = format_edit(&display, language).expect("bounded diff");
        let line_count = formatted.lines().count();
        assert!(line_count < 200, "{line_count} lines for {language:?}");
        assert!(formatted.contains(language.text("省略", "omitted", "省略")));
    }
}

#[test]
fn edit_diff_collapses_when_the_turn_has_its_final_result_and_reopens_on_click() {
    let mut app = app();
    app.running = true;
    app.note_tool_call_requested(Some("edit"), "edit_file", None, None);
    app.note_tool_call_completed(Some("edit"), "edit_file", None, None, false, Some(&edit()));
    assert!(screen(&mut app).contains("+new value"));
    assert!(app.tool_row_open(0));

    app.finish_turn();
    let collapsed = screen(&mut app);
    assert!(!collapsed.contains("+new value"), "{collapsed}");
    assert!(!collapsed.contains("example.rs"), "{collapsed}");
    assert!(collapsed.contains("✓ edit_file"), "{collapsed}");
    assert!(!app.tool_row_open(0));
    // The projection is retained: the user can still open the real receipt.
    app.toggle_tool_projection(0);
    let reopened = screen(&mut app);
    assert!(reopened.contains("+new value"), "{reopened}");
    assert!(app.tool_visuals.is_diff(0));
}

#[test]
fn a_user_opened_edit_diff_stays_open_after_the_turn_finishes() {
    let mut app = app();
    app.running = true;
    app.note_tool_call_requested(Some("edit"), "edit_file", None, None);
    app.note_tool_call_completed(Some("edit"), "edit_file", None, None, false, Some(&edit()));
    assert!(screen(&mut app).contains("+new value"));
    app.toggle_tool_projection(0);
    assert!(!screen(&mut app).contains("+new value"));
    app.toggle_tool_projection(0);
    assert!(screen(&mut app).contains("+new value"));

    app.finish_turn();
    assert!(
        screen(&mut app).contains("+new value"),
        "explicit expansion must survive the automatic collapse"
    );
}

#[test]
fn record_reordering_and_removal_keep_call_and_diff_identity() {
    let mut app = app();
    app.running = true;
    app.append_transcript("You: queued".to_owned());
    request(&mut app, "command", "COMMAND_DETAIL");
    app.note_tool_call_requested(Some("edit"), "edit_file", None, None);
    app.note_tool_call_completed(Some("edit"), "edit_file", None, None, false, Some(&edit()));
    app.move_transcript_row_to_end(0);
    assert!(app.tool_row_open(0) && app.tool_visuals.is_diff(1));
    complete(&mut app, "command", false);
    assert!(app.transcript[0].starts_with("· ✓"));
    assert!(!app.tool_row_open(0));
    app.drop_transcript_row(0);
    assert!(app.tool_visuals.is_diff(0));
    assert!(screen(&mut app).contains("+new value"));
}

#[test]
fn runtime_event_entrypoint_uses_call_identity_and_committed_receipt() {
    let mut app = app();
    app.running = true;
    for (id, command) in [("one", "RUNTIME_ONE"), ("two", "RUNTIME_TWO")] {
        runtime_ui::apply_runtime_output(
            &mut app,
            &format!(
                "task {}",
                json!({"type":"tool_requested","id":id,"name":"run_command","expansion":command})
            ),
        );
    }
    runtime_ui::apply_runtime_output(
        &mut app,
        &format!(
            "task {}",
            json!({"type":"tool_completed","id":"one","name":"run_command","is_error":false})
        ),
    );
    assert!(app.transcript[0].starts_with("· ✓"));
    assert!(app.transcript[1].starts_with("· …"));
    assert!(screen(&mut app).contains("RUNTIME_TWO"));
    runtime_ui::apply_runtime_output(
        &mut app,
        &format!(
            "task {}",
            json!({"type":"tool_completed","id":"edit","name":"edit_file","is_error":false,"edit_display":edit()})
        ),
    );
    assert!(screen(&mut app).contains("+new value"));
}
