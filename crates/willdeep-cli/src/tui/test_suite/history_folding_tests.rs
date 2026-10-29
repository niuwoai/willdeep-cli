use super::*;

#[test]
fn older_thoughts_do_not_split_tools_or_steal_latest_thought_clicks() {
    let mut app = App::new(
        vec![
            "You: old".into(),
            "· + Thought: 1.0s\nold reasoning".into(),
            "· ✓ read_file".into(),
            "· + Thought: 2.0s\nmore old reasoning".into(),
            "· ✗ run_command".into(),
            "WillDeep: finished".into(),
            "You: latest".into(),
            "· + Thought: 3.0s\nlatest reasoning".into(),
        ],
        Language::En,
    );
    app.ui.show_thinking = true;
    let display = app.display_transcript();
    assert!(!display.rows.iter().any(|row| row.contains("old reasoning")));
    assert!(
        display
            .rows
            .iter()
            .any(|row| row.contains("2 earlier tool calls folded") && row.contains("1 failed"))
    );
    assert!(
        display
            .rows
            .iter()
            .any(|row| row.contains("latest reasoning"))
    );
    app.thought_hit_rows = vec![(7, 0)];
    assert!(app.toggle_thought_at(7));
    assert!(app.transcript[7].starts_with("· − Thought:"));
    assert!(app.transcript[1].starts_with("· + Thought:"));
}
