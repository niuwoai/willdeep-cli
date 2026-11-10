use super::*;

#[path = "test_suite/history_folding_tests.rs"]
mod history_folding_tests;
#[path = "test_suite/prompt_history_tests.rs"]
mod prompt_history_tests;

#[path = "test_suite/model_cascade.rs"]
mod model_cascade;
#[path = "test_suite/runtime_streaming.rs"]
mod runtime_streaming;
#[path = "test_suite/stream_performance.rs"]
mod stream_performance;

#[cfg(test)]
mod queue_later_tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn super_or_ctrl_enter_queues_for_later() {
        assert!(is_queue_for_later_key(key(
            KeyCode::Enter,
            KeyModifiers::SUPER
        )));
        assert!(is_queue_for_later_key(key(
            KeyCode::Enter,
            KeyModifiers::CONTROL
        )));
        // 换行的键位（含带 Shift/Alt 的组合）不能被误认成入队。
        assert!(!is_queue_for_later_key(key(
            KeyCode::Enter,
            KeyModifiers::SHIFT
        )));
        assert!(!is_queue_for_later_key(key(
            KeyCode::Enter,
            KeyModifiers::ALT
        )));
        assert!(!is_queue_for_later_key(key(
            KeyCode::Enter,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )));
        assert!(!is_queue_for_later_key(key(
            KeyCode::Char('j'),
            KeyModifiers::CONTROL
        )));
        // 普通回车仍是立即发送。
        assert!(!is_queue_for_later_key(key(
            KeyCode::Enter,
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn stashing_sets_input_aside_and_toggles_it_back() {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.input.insert("稍后发");
        app.stash_input_for_queue();
        assert!(app.input.is_empty());
        assert_eq!(app.queue_draft.text(), "稍后发");

        // 输入框空着再按一次，是取回而不是再存一份。
        app.stash_input_for_queue();
        assert_eq!(app.input.text(), "稍后发");
        assert!(app.queue_draft.is_empty());

        // 已经暂存了一份时，新输入不会把它覆盖掉。
        app.stash_input_for_queue();
        app.input.insert("另一句");
        app.stash_input_for_queue();
        assert_eq!(app.input.text(), "另一句");
        assert_eq!(app.queue_draft.text(), "稍后发");
    }
}

#[cfg(test)]
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod command_tests {
    use super::*;

    fn outcome(first_response_millis: Option<u64>) -> willdeep_core::AgentOutcome {
        willdeep_core::AgentOutcome {
            final_text: "done".to_owned(),
            turns: 3,
            messages: Vec::new(),
            stop_reason: willdeep_core::AgentStopReason::Finished,
            input_tokens: 12_345,
            output_tokens: 678,
            first_response_millis,
            context_summary: None,
        }
    }

    /// 本轮账目跟在回答后面，作「本轮结束」分隔线，浅色，四个数都在。
    #[test]
    fn a_finished_turn_appends_its_own_bill() {
        let mut app = App::new(Vec::new(), Language::En);
        app.begin_turn(false, "thinking".to_owned());
        app.append_turn_stats(Some(&outcome(Some(1_500))));

        let line = app.transcript.last().expect("stats line");
        assert!(
            line.starts_with(TURN_DIVIDER_PREFIX),
            "分隔线前缀决定它按浅色画、不跟正文抢注意力: {line}"
        );
        // 「轮到你」不进记录，只在画最后一行时补上。
        assert!(!line.contains("your turn"), "{line}");
        assert_eq!(
            with_your_turn(line, Language::En),
            format!("{} · your turn ──", line.trim_end_matches(" ──"))
        );
        assert!(line.contains("first reply"));
        assert!(line.contains("total"));
        // 输入含系统提示词与整段历史，所以它本来就该比用户那句话大得多。
        assert!(line.contains("in 12.35K"));
        assert!(line.contains("out 678"));
        assert!(line.contains("turns 3"));
    }

    /// 拿不到首答耗时就不印那一段，而不是印一个 0。
    #[test]
    fn a_missing_first_reply_is_left_out_rather_than_faked() {
        let mut app = App::new(Vec::new(), Language::En);
        app.begin_turn(false, "thinking".to_owned());
        app.append_turn_stats(Some(&outcome(None)));

        let line = app.transcript.last().expect("stats line");
        assert!(!line.contains("first reply"));
        assert!(line.contains("total"));
    }

    /// `/exit` 本地处理，并请求退出。
    ///
    /// 命令处理器只报告「用户想走」，真正拆终端的活儿留在事件循环原来的地方：
    /// 两处都能关终端的话，谁负责恢复终端状态就说不清了。
    #[test]
    fn exit_command_requests_a_quit_locally() {
        let mut app = App::new(Vec::new(), Language::En);
        let skills = SkillCatalog::default();

        assert!(!app.quit_requested);
        assert!(app.handle_slash_command("/exit", &skills));
        assert!(app.quit_requested);
        assert!(
            app.transcript
                .last()
                .is_some_and(|line| line.contains("Exiting")),
            "退出要有回执，否则按下去像没反应"
        );
    }

    /// 别的命令不会顺手把 TUI 关掉。
    #[test]
    fn other_commands_do_not_request_a_quit() {
        let mut app = App::new(Vec::new(), Language::En);
        let skills = SkillCatalog::default();
        app.handle_slash_command("/help", &skills);
        assert!(!app.quit_requested);
    }

    /// 命令面板装不下时要跟着选中项滚，而不是把后面的静默裁掉。
    ///
    /// 这是一个真实的错觉来源：18 条命令、面板只画得下 8 行，用户看到的最后
    /// 一条是 `/sidebar`，会以为命令就这些。
    #[test]
    fn the_command_menu_scrolls_to_keep_the_selection_visible() {
        // 装得下就不滚。
        assert_eq!(command_window_offset(0, 6, 8), 0);
        assert_eq!(command_window_offset(5, 6, 8), 0);
        // 选中项还在第一屏里，窗口不动。
        assert_eq!(command_window_offset(7, 18, 8), 0);
        // 越过下沿才滚，且只滚到刚好把选中项露出来。
        assert_eq!(command_window_offset(8, 18, 8), 1);
        // 滚到底就停住，不会把窗口拉出列表之外。
        assert_eq!(command_window_offset(17, 18, 8), 10);
        // 每个选中项都必须落在窗口里，一个都不能漏。
        for selected in 0..18 {
            let offset = command_window_offset(selected, 18, 8);
            assert!(
                (offset..offset + 8).contains(&selected),
                "selected {selected} fell outside the window at offset {offset}"
            );
        }
    }

    /// 全部命令都在目录里，`/sidebar` 不是最后一条。
    #[test]
    fn the_catalog_carries_every_command() {
        let commands: Vec<&str> = crate::tui::command_catalog::command_candidates(Language::En)
            .into_iter()
            .map(|(command, _)| command)
            .collect();
        assert_eq!(commands.len(), 38);
        // 面板一屏只画得下 8 条，后面这些此前完全看不到。
        for command in [
            "/reasoning",
            "/daemon",
            "/runtime",
            "/local",
            "/session",
            "/history",
            "/workspace",
            "/new",
            "/agent",
            "/diff",
            "/skills",
            "/uistyle",
            "/clear",
            "/queue",
            "/peers",
            "/workflow",
            "/short",
            "/verbose",
        ] {
            assert!(commands.contains(&command), "{command} 不在命令目录里");
        }
    }

    /// Runtime 轮次没有 AgentOutcome，账目走本轮累计的用量。
    ///
    /// 这条路径此前完全没有账目：daemon 模式下的回答后面什么也不显示。
    #[test]
    fn a_runtime_turn_bills_from_its_accumulated_usage() {
        let mut app = App::new(Vec::new(), Language::En);
        app.begin_turn(true, "working".to_owned());
        app.record_turn_usage(&willdeep_core::types::Usage {
            input_tokens: Some(1_000),
            output_tokens: Some(200),
            total_tokens: Some(1_200),
            cache_read_tokens: None,
        });
        // 第二次请求：账目累加，而不是被最后一次覆盖。
        app.record_turn_usage(&willdeep_core::types::Usage {
            input_tokens: Some(1_500),
            output_tokens: Some(300),
            total_tokens: Some(1_800),
            cache_read_tokens: None,
        });
        app.append_turn_stats(None);

        let line = app.transcript.last().expect("stats line");
        assert!(line.contains("in 2.50K"), "两次请求的输入要加起来: {line}");
        assert!(line.contains("out 500"));
        assert!(line.contains("first reply"), "第一次用量到达的时刻就是首答");
        assert!(
            !line.contains("turns"),
            "Runtime 路径数不出轮次，就别硬报一个"
        );
    }

    /// 一个数都没有的轮次也要落分隔线：它现在是「轮到你了」的信号，不只是账目。
    /// 但没有的数一个都不印，尤其不印 0。
    #[test]
    fn an_empty_turn_still_marks_its_end_without_faking_numbers() {
        let mut app = App::new(Vec::new(), Language::En);
        let mut empty = outcome(None);
        empty.input_tokens = 0;
        empty.output_tokens = 0;
        empty.turns = 1;
        app.append_turn_stats(Some(&empty));
        assert_eq!(app.transcript, vec!["── turn finished ──"]);
    }

    #[test]
    fn goal_command_enriches_future_prompts() {
        let mut app = App::new(Vec::new(), Language::En);
        let skills = SkillCatalog::default();

        assert!(app.handle_slash_command("/goal ship the CLI", &skills));
        let enriched = app.enrich_prompt("continue", &skills);

        assert!(enriched.contains("<goal>\nship the CLI\n</goal>"));
        assert!(enriched.ends_with("continue"));
        assert!(app.handle_slash_command("/goal off", &skills));
        assert_eq!(app.enrich_prompt("continue", &skills), "continue");
    }

    /// `/name` 读写 `$WILLDEEP_HOME/identity.json`：不带参数显示当前名字，
    /// 带参数改名且 UUID 不变。没有家目录时明确报错，不假装改了。
    #[test]
    fn the_name_command_shows_and_renames_the_instance_identity() {
        let home = std::env::temp_dir().join(format!("willdeep-tui-name-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.display_home = Some(home.clone());
        let skills = SkillCatalog::default();

        assert!(app.handle_slash_command("/name", &skills));
        let shown = app.transcript.last().unwrap().clone();
        assert!(shown.contains("实例名"), "{shown}");
        let first = crate::identity::load(&home).unwrap();

        assert!(app.handle_slash_command("/name jack", &skills));
        assert!(app.transcript.last().unwrap().contains("jack"));
        let renamed = crate::identity::load(&home).unwrap();
        assert_eq!(renamed.name, "jack");
        assert_eq!(renamed.id, first.id);

        // 非法名字只说名字不合法，不能把原来的名字改掉。
        assert!(app.handle_slash_command("/name alex:2u2d", &skills));
        assert!(app.transcript.last().unwrap().starts_with("Error:"));
        assert_eq!(crate::identity::load(&home).unwrap().name, "jack");

        app.display_home = None;
        assert!(app.handle_slash_command("/name alex", &skills));
        assert!(app.transcript.last().unwrap().contains("willdeep home"));
        std::fs::remove_dir_all(home).unwrap();
    }

    /// 默认是简洁；`/short` 与 `/verbose` 各管一个方向，提示里互相点名。
    /// 它们不进提示词——风格写在系统提示词里，下一轮构建 harness 时才读。
    #[test]
    fn short_and_verbose_select_a_style_and_ignore_arguments() {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        let skills = SkillCatalog::default();
        assert_eq!(app.response_style, None);

        assert!(app.handle_slash_command("/verbose", &skills));
        assert_eq!(
            app.response_style,
            Some(willdeep_core::ResponseStyle::Detailed)
        );
        assert!(app.transcript.last().unwrap().contains("详细回复"));
        assert!(app.transcript.last().unwrap().contains("/short"));

        assert!(app.handle_slash_command("/short", &skills));
        assert_eq!(
            app.response_style,
            Some(willdeep_core::ResponseStyle::Short)
        );
        assert!(app.transcript.last().unwrap().contains("简洁回复"));
        assert!(app.transcript.last().unwrap().contains("/verbose"));

        // 带参数的不是这条命令：不能静默地把风格拨过去。
        assert!(app.handle_slash_command("/verbose on", &skills));
        assert_eq!(
            app.response_style,
            Some(willdeep_core::ResponseStyle::Short)
        );
        assert!(app.transcript.last().unwrap().contains("unknown command"));
    }

    #[test]
    fn unknown_slash_command_is_handled_locally() {
        let mut app = App::new(Vec::new(), Language::En);
        let skills = SkillCatalog::default();

        assert!(app.handle_slash_command("/wat", &skills));
        assert!(app.transcript.last().unwrap().contains("unknown command"));
    }

    #[test]
    fn delegated_webapp_command_is_not_rejected_by_the_fallback_handler() {
        let mut app = App::new(Vec::new(), Language::En);
        let skills = SkillCatalog::default();

        assert!(!app.handle_slash_command("/webapp", &skills));
        assert!(app.transcript.is_empty());
    }

    #[test]
    fn rewind_is_delegated_to_the_runtime_path_and_listed_with_the_other_commands() {
        let mut app = App::new(Vec::new(), Language::En);
        let skills = SkillCatalog::default();

        assert!(!app.handle_slash_command("/rewind", &skills));
        assert!(app.transcript.is_empty());
        assert!(
            command_candidates(Language::ZhCn)
                .iter()
                .any(|(command, description)| *command == "/rewind"
                    && description.contains("回到第 N 步"))
        );
    }

    #[test]
    fn ordinary_prompt_is_not_treated_as_command() {
        let mut app = App::new(Vec::new(), Language::En);
        assert!(!app.handle_slash_command("please inspect /docs", &SkillCatalog::default()));
    }
}
mod interaction_tests;
mod mobile_tests;
mod rewind_tests;
mod session_tests;
