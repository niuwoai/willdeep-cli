use super::*;

const LANGUAGES: [Language; 3] = [Language::ZhCn, Language::En, Language::Ja];

fn running_app(language: Language) -> App {
    let mut app = App::new(Vec::new(), language);
    app.begin_turn(false, "test turn".to_owned());
    app
}

fn stale_progress(app: &mut App, seconds: u64) {
    let now = Instant::now();
    app.turn_started = Some(now - Duration::from_secs(seconds + 5));
    app.last_progress_at = Some(now - Duration::from_secs(seconds));
}

#[test]
fn silence_supplements_every_phase_in_all_languages() {
    let phases = [
        TurnPhase::Preparing,
        TurnPhase::WaitingModel,
        TurnPhase::Reasoning,
        TurnPhase::ReceivingReply,
        TurnPhase::Tool("run_command".to_owned()),
        TurnPhase::Compressing,
        TurnPhase::Retrying {
            attempt: 2,
            delay: Some(Duration::from_secs(3)),
        },
        TurnPhase::Background,
    ];
    for language in LANGUAGES {
        for phase in &phases {
            for idle in [2, 9, 31] {
                let label = phase.label(language);
                let summary = format_working_summary(
                    language,
                    true,
                    &label,
                    Duration::from_secs(48),
                    Duration::from_secs(idle),
                );
                assert!(summary.contains(&label), "{summary}");
                if idle == 9 {
                    assert!(
                        summary.contains(language.text(
                            "等待新事件",
                            "Waiting for an update",
                            "新しいイベントを待機中"
                        )),
                        "{summary}"
                    );
                } else if idle == 31 {
                    assert!(summary.contains("31s"), "{summary}");
                    assert!(
                        summary.contains(language.text(
                            "未收到新事件",
                            "without a new event",
                            "新しいイベントなし"
                        )),
                        "{summary}"
                    );
                }
            }
        }
    }
}

#[test]
fn human_wait_wins_over_compression_background_and_silence() {
    for language in LANGUAGES {
        let mut app = running_app(language);
        app.note_compression_started();
        app.set_background_wait(true);
        let (answer_tx, _answer_rx) = oneshot::channel();
        app.enqueue_question(AskDialog {
            request: UserQuestion {
                question: "choose".to_owned(),
                options: Vec::new(),
                multi_select: false,
            },
            selected: 0,
            checked: Vec::new(),
            answer: PromptEditor::default(),
            sender: answer_tx,
        });
        let (approval_tx, _approval_rx) = oneshot::channel();
        app.enqueue_approval(("run_command".to_owned(), false, approval_tx));
        for idle in [9, 31] {
            stale_progress(&mut app, idle);
            let summary = app.turn_phase_summary().unwrap();
            assert!(
                summary.contains(language.text(
                    "等待你审批",
                    "Waiting for your approval",
                    "承認を待っています"
                )),
                "{summary}"
            );
            assert!(
                !summary.contains(&TurnPhase::Compressing.label(language)),
                "{summary}"
            );
            assert!(
                !summary.contains(&TurnPhase::Background.label(language)),
                "{summary}"
            );
        }
        app.approval = None;
        let summary = app.turn_phase_summary().unwrap();
        assert!(
            summary.contains(language.text(
                "等待你回答",
                "Waiting for your answer",
                "回答を待っています"
            )),
            "{summary}"
        );
        app.question = None;
        assert!(
            app.turn_phase_summary()
                .unwrap()
                .contains(&TurnPhase::Compressing.label(language))
        );
        app.note_compression_completed();
        assert!(
            app.turn_phase_summary()
                .unwrap()
                .contains(&TurnPhase::Background.label(language))
        );
        app.set_background_wait(false);
        app.note_tool_requested("read_file", None, None);
        assert!(
            app.turn_phase_summary()
                .unwrap()
                .contains(&TurnPhase::Tool("read_file".to_owned()).label(language))
        );
    }
}

#[test]
fn selection_and_notice_do_not_hide_a_human_gate() {
    let mut app = running_app(Language::En);
    let (tx, _rx) = oneshot::channel();
    app.enqueue_approval(("read_file".to_owned(), false, tx));
    app.selection_mode = true;
    app.notice = Some("a routine notice".to_owned());
    let status = app.status_line();
    assert!(status.contains("Waiting for your approval"), "{status}");
    assert!(status.contains("a routine notice"), "{status}");
    app.selection_mode = false;
    app.native_selection_mode = true;
    assert!(app.status_line().contains("Waiting for your approval"));
}

#[test]
fn each_nonempty_stream_delta_refreshes_heartbeat_without_growing_log() {
    let mut app = running_app(Language::En);
    let log_len = app.progress_log.len();
    for kind in [
        StreamKind::Reasoning,
        StreamKind::Reply,
        StreamKind::Reasoning,
        StreamKind::Reply,
    ] {
        stale_progress(&mut app, 31);
        let before = Instant::now();
        app.stream_transient(kind, "live");
        assert!(app.last_progress_at.unwrap() >= before);
        assert_eq!(app.progress_log.len(), log_len);
        let summary = app.turn_phase_summary().unwrap();
        assert!(!summary.contains("without a new event"), "{summary}");
        assert_eq!(
            app.turn_phase,
            match kind {
                StreamKind::Reasoning => TurnPhase::Reasoning,
                StreamKind::Reply => TurnPhase::ReceivingReply,
            }
        );
    }
    stale_progress(&mut app, 31);
    let before = Instant::now();
    app.note_narration("finalized paragraph");
    assert!(app.last_progress_at.unwrap() >= before);
    assert_eq!(app.progress_log.len(), log_len);
}

#[test]
fn reply_stream_retains_its_original_prefix_beyond_the_old_preview_limit() {
    let mut app = running_app(Language::En);
    let first = "开头 👩‍💻\n".repeat(400);
    let second = "中段かな\n".repeat(400);
    app.stream_transient(StreamKind::Reply, &first);
    assert_eq!(app.transient_thought.as_deref(), Some(first.as_str()));
    app.stream_transient(StreamKind::Reply, &second);
    let complete = format!("{first}{second}");
    assert_eq!(app.transient_thought.as_deref(), Some(complete.as_str()));
    assert!(app.transient_row().unwrap().contains(&first));
    app.note_narration(&complete);
    app.note_reply(&complete);
    assert_eq!(
        app.transcript,
        [format!("{ASSISTANT_ROW_PREFIX}{}", complete.trim())]
    );
}

#[test]
fn model_preparation_and_request_waiting_are_distinct_from_reasoning() {
    let mut app = running_app(Language::En);
    app.note_narration("the prior step is finalized");
    app.note_model_step_started();
    assert_eq!(app.turn_phase, TurnPhase::Preparing);
    assert!(app.held_narration.is_none());
    assert_eq!(
        app.transcript,
        [format!("{ASSISTANT_ROW_PREFIX}the prior step is finalized")]
    );
    app.note_turn_phase(TurnPhase::WaitingModel);
    assert!(
        app.turn_phase_summary()
            .unwrap()
            .contains("Waiting for model")
    );
    app.stream_transient(StreamKind::Reasoning, "reasoning delta");
    assert_eq!(app.turn_phase, TurnPhase::Reasoning);
}

#[test]
fn completing_one_tool_keeps_another_pending_tool_as_the_foreground_phase() {
    let mut app = running_app(Language::En);
    for name in ["read_file", "run_command"] {
        app.tools.requested(name);
        app.note_tool_requested(name, None, None);
    }
    app.tools.completed("run_command", false);
    app.note_tool_completed("run_command", None, None, false);
    assert_eq!(app.turn_phase, TurnPhase::Tool("read_file".to_owned()));
    app.tools.completed("read_file", true);
    app.note_tool_completed("read_file", None, None, true);
    assert_eq!(app.turn_phase, TurnPhase::Preparing);
    assert_eq!(
        app.last_turn_outcome, None,
        "one failed tool is not a failed turn"
    );
}

#[test]
fn only_complete_stop_reasons_produce_a_completed_outcome() {
    use willdeep_core::AgentStopReason;
    for (reason, expected) in [
        (AgentStopReason::Finished, TurnOutcome::Completed),
        (AgentStopReason::GoalComplete, TurnOutcome::Completed),
        (AgentStopReason::BudgetLimited, TurnOutcome::Partial),
        (AgentStopReason::MaxTurns, TurnOutcome::Partial),
        (AgentStopReason::Incomplete, TurnOutcome::Partial),
        (AgentStopReason::Unverified, TurnOutcome::Partial),
    ] {
        assert_eq!(TurnOutcome::from_stop_reason(reason), expected);
    }
}

#[test]
fn explicit_outcomes_survive_cleanup_and_duplicate_terminal_events() {
    for outcome in [
        TurnOutcome::Completed,
        TurnOutcome::Partial,
        TurnOutcome::Failed,
        TurnOutcome::Interrupted,
        TurnOutcome::Cancelled,
    ] {
        for language in LANGUAGES {
            let mut app = running_app(language);
            app.note_turn_outcome(outcome);
            app.note_turn_outcome(TurnOutcome::Completed);
            assert_eq!(app.last_turn_outcome, Some(outcome));
            app.finish_turn();
            app.finish_turn();
            app.note_turn_outcome(TurnOutcome::Failed);
            assert_eq!(app.last_turn_outcome, Some(outcome));
            assert!(app.status_line().contains(outcome.label(language)));
            assert_eq!(
                app.essential_turn_status().as_deref(),
                Some(outcome.label(language))
            );
            app.begin_turn(false, "next turn".to_owned());
            assert_eq!(app.last_turn_outcome, None);
            assert_eq!(app.turn_phase, TurnPhase::Preparing);
        }
    }
}

#[test]
fn cleanup_or_connection_recovery_does_not_infer_completion() {
    let mut app = running_app(Language::En);
    app.finish_turn();
    assert_eq!(app.last_turn_outcome, None);
    assert_eq!(app.idle_turn_status(), "Ready");
    assert_eq!(app.essential_turn_status(), None);
    app.ensure_runtime_turn();
    app.finish_turn();
    assert_eq!(app.last_turn_outcome, None);
    assert!(!app.status_line().contains("Turn completed"));
}

#[test]
fn idle_outcome_and_queued_input_are_both_visible() {
    for language in LANGUAGES {
        let mut app = running_app(language);
        app.input.insert("follow-up");
        app.queue_submitted_prompt();
        app.note_turn_outcome(TurnOutcome::Failed);
        app.finish_turn();
        let status = app.status_line();
        assert!(
            status.contains(TurnOutcome::Failed.label(language)),
            "{status}"
        );
        assert!(
            status.contains(language.text(
                "待发消息等待发送",
                "Queued messages waiting to send",
                "送信待ちがあります"
            )),
            "{status}"
        );
    }
}

#[test]
fn concise_paragraphs_keep_their_models_and_times_across_tool_and_model_steps() {
    let mut app = running_app(Language::En);
    app.reply_model = Some("model-a".to_owned());
    app.message_times.event_time = Some(100);
    app.note_narration("first conclusion");
    let held = app.held_narration.as_ref().unwrap();
    assert_eq!(held.model.as_deref(), Some("model-a"));
    assert_eq!(held.recorded_at, 100);

    app.reply_model = Some("model-b".to_owned());
    app.message_times.event_time = Some(200);
    app.note_narration("second conclusion");
    assert_eq!(app.transcript_models[0].as_deref(), Some("model-a"));
    assert_eq!(app.message_times.time_at(0), Some(100));
    app.reply_model = Some("model-c".to_owned());
    app.message_times.event_time = Some(300);
    app.note_tool_requested("read_file", None, None);
    assert_eq!(app.transcript_models[1].as_deref(), Some("model-b"));
    assert_eq!(app.message_times.time_at(1), Some(200));
    assert!(app.transcript[2].contains("read_file"));
    assert!(app.held_narration.is_none());

    app.note_narration("third conclusion");
    app.reply_model = Some("model-d".to_owned());
    app.message_times.event_time = Some(400);
    app.note_model_step_started();
    assert_eq!(app.transcript_models[3].as_deref(), Some("model-c"));
    assert_eq!(app.message_times.time_at(3), Some(300));
    assert_eq!(app.note_reply("third conclusion"), None);
    app.finish_turn();
    assert_eq!(app.transcript.len(), 4);
    for text in ["first conclusion", "second conclusion", "third conclusion"] {
        assert_eq!(
            app.transcript
                .iter()
                .filter(|row| row.ends_with(text))
                .count(),
            1
        );
    }
}

#[test]
fn a_paragraph_without_source_model_never_borrows_the_later_selected_model() {
    let mut app = running_app(Language::En);
    app.message_times.event_time = Some(100);
    app.note_narration("unknown source");
    app.reply_model = Some("new-model".to_owned());
    app.display_model = Some("selected-model".to_owned());
    app.message_times.event_time = Some(200);
    app.note_model_step_started();
    assert_eq!(app.transcript_models[0], None);
    assert_eq!(app.message_times.time_at(0), Some(100));
}

#[test]
fn final_reply_and_partial_prefix_are_each_committed_once_with_original_metadata() {
    let mut app = running_app(Language::En);
    app.reply_model = Some("model-a".to_owned());
    app.message_times.event_time = Some(100);
    app.note_narration("partial result");
    app.reply_model = Some("model-b".to_owned());
    app.message_times.event_time = Some(200);
    let final_text = "Stopped at turn limit\n\npartial result";
    assert_eq!(app.note_reply(final_text).as_deref(), Some(final_text));
    assert_eq!(
        app.transcript,
        [
            format!("{ASSISTANT_ROW_PREFIX}Stopped at turn limit"),
            format!("{ASSISTANT_ROW_PREFIX}partial result"),
        ]
    );
    assert_eq!(
        app.transcript_models,
        [Some("model-b".to_owned()), Some("model-a".to_owned())]
    );
    assert_eq!(app.message_times.time_at(0), Some(200));
    assert_eq!(app.message_times.time_at(1), Some(100));
    assert_eq!(app.note_reply(final_text), None);
    app.finish_turn();
    assert_eq!(app.transcript.len(), 2);
}

#[test]
fn unrelated_final_text_returns_both_newly_committed_paragraphs_for_mirroring() {
    let mut app = running_app(Language::En);
    app.note_narration("earlier conclusion");
    assert_eq!(
        app.note_reply("final answer").as_deref(),
        Some("earlier conclusion\n\nfinal answer")
    );
    assert_eq!(app.transcript.len(), 2);
    assert_eq!(app.note_reply("final answer"), None);
}

#[test]
fn failure_interruption_and_cancellation_flush_the_held_paragraph_once() {
    for outcome in [
        TurnOutcome::Failed,
        TurnOutcome::Interrupted,
        TurnOutcome::Cancelled,
    ] {
        let mut app = running_app(Language::En);
        app.reply_model = Some("model-a".to_owned());
        app.message_times.event_time = Some(100);
        app.note_narration("work retained so far");
        app.message_times.event_time = Some(900);
        app.reply_model = Some("later-model".to_owned());
        app.note_turn_outcome(outcome);
        app.finish_turn();
        app.finish_turn();
        assert_eq!(
            app.transcript,
            [format!("{ASSISTANT_ROW_PREFIX}work retained so far")]
        );
        assert_eq!(app.transcript_models[0].as_deref(), Some("model-a"));
        assert_eq!(app.message_times.time_at(0), Some(100));
        assert_eq!(app.last_turn_outcome, Some(outcome));
    }
}

#[test]
fn stopping_before_the_final_text_event_preserves_the_live_reply_once() {
    for outcome in [
        TurnOutcome::Failed,
        TurnOutcome::Interrupted,
        TurnOutcome::Cancelled,
    ] {
        let mut app = running_app(Language::En);
        app.reply_model = Some("stream-model".to_owned());
        app.stream_transient(StreamKind::Reply, "Already received 中文😀\n");
        app.stream_transient(StreamKind::Reply, "the next fragment");
        app.note_turn_outcome(outcome);
        app.finish_turn();
        app.finish_turn();
        assert_eq!(
            app.transcript,
            [format!(
                "{ASSISTANT_ROW_PREFIX}Already received 中文😀\nthe next fragment"
            )]
        );
        assert_eq!(app.transcript_models[0].as_deref(), Some("stream-model"));
        assert_eq!(app.last_turn_outcome, Some(outcome));
        assert!(app.transient_thought.is_none());
    }
}

#[test]
fn a_style_change_or_compression_does_not_discard_a_finalized_paragraph() {
    let mut app = running_app(Language::En);
    app.note_narration("concise conclusion");
    app.response_style = Some(willdeep_core::ResponseStyle::Detailed);
    app.note_narration("detailed conclusion");
    assert_eq!(
        app.transcript,
        [
            format!("{ASSISTANT_ROW_PREFIX}concise conclusion"),
            format!("{ASSISTANT_ROW_PREFIX}detailed conclusion"),
        ]
    );
    app.response_style = None;
    app.note_narration("before compression");
    app.note_compression_started();
    assert!(app.held_narration.is_none());
    assert!(app.transcript[2].ends_with("before compression"));
    assert_eq!(app.turn_phase, TurnPhase::Compressing);
}

#[test]
fn full_and_compact_render_the_same_localized_phase_and_terminal_outcome() {
    for mode in [crate::config::UiMode::Full, crate::config::UiMode::Compact] {
        for language in LANGUAGES {
            let mut app = running_app(language);
            app.ui.mode = mode;
            app.ui.compact_status_line = true;
            app.note_tool_requested("read_file", None, None);
            stale_progress(&mut app, 31);
            let session = Session::new(std::path::PathBuf::from("/tmp"), None, "");
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(180, 40)).unwrap();
            let mut screen = |app: &mut App| {
                super::super::draw(&mut terminal, app, &SkillCatalog::default(), &session).unwrap();
                let buffer = terminal.backend().buffer();
                (0..buffer.area.height)
                    .map(|y| {
                        let mut line = String::new();
                        let mut x = 0;
                        while x < buffer.area.width {
                            let symbol = buffer[(x, y)].symbol();
                            line.push_str(symbol);
                            x += UnicodeWidthStr::width(symbol).max(1) as u16;
                        }
                        line
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let rendered = screen(&mut app);
            assert!(
                rendered.contains(&TurnPhase::Tool("read_file".to_owned()).label(language)),
                "{rendered}"
            );
            app.note_turn_outcome(TurnOutcome::Failed);
            app.finish_turn();
            let rendered = screen(&mut app);
            assert!(
                rendered.contains(TurnOutcome::Failed.label(language)),
                "{rendered}"
            );
        }
    }
}
