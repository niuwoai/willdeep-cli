use super::*;

fn synthetic_history(history_len: usize) -> Vec<String> {
    (0..history_len)
        .map(|i| {
            if i % 2 == 0 {
                format!("You: inspect task {i} and explain the result")
            } else {
                format!(
                    "WillDeep: Task {i}\n\n{}\n```rust\nlet value = 42;\n```",
                    "补丁已生成，需要验证行为。Rendering **streamed output** with `code`. "
                        .repeat(8)
                )
            }
        })
        .collect()
}

fn reference_frame(app: &App) -> Text<'static> {
    let mut rows = app.display_transcript().rows;
    if let Some(row) = app.transient_row() {
        rows.push(row);
    }
    let search = app
        .search
        .as_ref()
        .map(|s| s.editor.text().trim())
        .filter(|s| !s.is_empty());
    wrap_styled_text(
        colored_transcript(&rows, search, app.transcript_width, app.language),
        app.transcript_width,
    )
}

fn assert_cached_frame_matches_reference(app: &mut App) {
    let expected = reference_frame(app);
    let transient = app.prepare_transcript_frame();
    assert_eq!(app.transcript_rows, text_rows(&expected));
    assert_eq!(app.transcript_height, expected.lines.len());
    for offset in [
        0,
        2,
        expected.lines.len().saturating_sub(4),
        expected.lines.len(),
    ] {
        let viewport = app.transcript_viewport(&transient, offset);
        assert_eq!(
            viewport.lines,
            expected
                .lines
                .iter()
                .skip(offset)
                .take(app.viewport_height)
                .cloned()
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn cached_stream_render_preserves_layout_after_all_display_changes() {
    let mut app = App::new(
        vec![
            "You: 检查一处问题".into(),
            "WillDeep: **结果**\n\n```rust\nlet x = 42;\n```".into(),
            "· ✓ read_file · first".into(),
            "· ✓ read_file · second".into(),
            "· ✓ read_file · third".into(),
        ],
        Language::En,
    );
    app.transcript_width = 30;
    app.viewport_height = 4;
    assert_cached_frame_matches_reference(&mut app);
    app.stream_transient(StreamKind::Reply, "不断流入的中文和 English 输出");
    assert_cached_frame_matches_reference(&mut app);
    app.transcript.push("WillDeep: new **answer**".into());
    assert_cached_frame_matches_reference(&mut app);
    app.transcript[0] = "You: 修改已有内容".into();
    assert_cached_frame_matches_reference(&mut app);
    app.transcript_width = 12;
    assert_cached_frame_matches_reference(&mut app);
    app.tool_rows_expanded = true;
    assert_cached_frame_matches_reference(&mut app);
    app.language = Language::ZhCn;
    assert_cached_frame_matches_reference(&mut app);
    let mut search = SearchState::default();
    search.editor.insert("new");
    app.search = Some(search);
    assert_cached_frame_matches_reference(&mut app);
    app.transient_thought = None;
    app.transcript.clear();
    assert_cached_frame_matches_reference(&mut app);
}

#[test]
fn viewport_selection_keeps_global_coordinates_and_styles() {
    let mut expected = wrap_styled_text(Text::from("first\n中文 second\nlast"), 40);
    highlight_text_selection(&mut expected, (0, 2), (2, 2));
    let mut actual = Text::from(vec![Line::from("中文 second"), Line::from("last")]);
    rendering::highlight_text_selection_at_offset(&mut actual, (0, 2), (2, 2), 1);
    assert_eq!(actual.lines, expected.lines[1..]);
}

/// Local deterministic stream: no model, gateway, network or user history.
/// Run optimized with --ignored --nocapture; JSON is suitable for before/after reports.
#[test]
#[ignore = "manual deterministic streaming performance measurement"]
fn deterministic_stream_render_benchmark() {
    const FRAMES: usize = 40;
    for history_len in [20, 500, 2_000] {
        let history = synthetic_history(history_len);
        let mut app = App::new(history, Language::En);
        app.running = true;
        app.sidebar_visible = false;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(160, 50)).unwrap();
        let skills = SkillCatalog::default();
        draw(&mut terminal, &mut app, &skills, &quiet_test_session()).unwrap();
        let mut timings = Vec::new();
        for _ in 0..FRAMES {
            let started = Instant::now();
            app.stream_transient(StreamKind::Reply, "新字");
            draw(&mut terminal, &mut app, &skills, &quiet_test_session()).unwrap();
            timings.push(started.elapsed().as_secs_f64() * 1_000.0);
        }
        timings.sort_by(f64::total_cmp);
        println!(
            "STREAM_BENCH {}",
            serde_json::json!({
                "history_entries": history_len,
                "frames": FRAMES,
                "median_ms": timings[FRAMES / 2],
                "p95_ms": timings[FRAMES * 95 / 100],
                "mean_ms": timings.iter().sum::<f64>() / FRAMES as f64,
                "input_interval_ms": 20,
                "frames_over_budget": timings.iter().filter(|ms| **ms > 20.0).count(),
            })
        );
    }
}

#[test]
#[ignore = "manual paced-stream end-to-display measurement"]
fn paced_stream_display_latency() {
    let mut app = App::new(synthetic_history(2_000), Language::En);
    app.running = true;
    app.sidebar_visible = false;
    let skills = SkillCatalog::default();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(160, 50)).unwrap();
    draw(&mut terminal, &mut app, &skills, &quiet_test_session()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let producer = std::thread::spawn(move || {
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(20));
            tx.send(Instant::now()).unwrap();
        }
    });
    let mut latency = Vec::new();
    for sent in rx {
        app.stream_transient(StreamKind::Reply, "新字");
        draw(&mut terminal, &mut app, &skills, &quiet_test_session()).unwrap();
        latency.push(sent.elapsed().as_secs_f64() * 1_000.0);
    }
    producer.join().unwrap();
    latency.sort_by(f64::total_cmp);
    println!(
        "PACED_STREAM {}",
        serde_json::json!({"events":latency.len(), "input_interval_ms":20, "median_ms":latency[50], "p95_ms":latency[95], "max_ms":latency[99]})
    );
    assert_eq!(
        app.transient_thought.as_deref(),
        Some("新字".repeat(100).as_str())
    );
}
