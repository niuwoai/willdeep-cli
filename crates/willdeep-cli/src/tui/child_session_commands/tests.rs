use super::*;

fn runtime(root: &std::path::Path) -> TuiRuntime {
    let (tx, rx) = mpsc::unbounded_channel();
    TuiRuntime {
        home: root.to_owned(),
        notifier: crate::notify::Notifier::disabled(),
        skills: Arc::new(SkillCatalog::default()),
        kernel: willdeep_core::EventKernel::new(),
        kernel_store: willdeep_core::kernel_store::KernelStore::new(root),
        detached_jobs: willdeep_core::DetachedJobStore::new(root),
        context_window: 128_000,
        background_tasks: Arc::new(BackgroundTaskRegistry::default()),
        runtime_submit: crate::daemon::RuntimeSubmitOptions {
            workspace: root.to_owned(),
            profile: None,
            model: None,
            config: None,
        },
        provider_config: willdeep_core::provider::ProviderConfig::new(
            willdeep_core::provider::ProviderKind::OpenAiCompatible,
            willdeep_core::provider::ApiDialect::ChatCompletions,
            "https://provider.example/v1",
            "test-key",
            "test-model",
        ),
        local_workspace: root.to_owned(),
        tx,
        rx,
    }
}

#[test]
fn terminal_wording_refers_to_assignment_and_waiting_stays_open() {
    assert_eq!(
        status(ChildSessionTaskStatus::Completed, Language::En),
        "This task has ended"
    );
    assert_eq!(
        status(ChildSessionTaskStatus::WaitingAnswer, Language::En),
        "Waiting for an answer"
    );
    assert!(!ChildSessionTaskStatus::WaitingAnswer.is_terminal());
    assert_eq!(safe("report\u{1b}[2J\nnext"), "report[2J\nnext");
}

#[test]
fn task_projection_distinguishes_control_subscription_and_dependency_waiting() {
    let fixture =
        include_str!("../../../../willdeep-runtime-protocol/fixtures/child-session-v1.json");
    let mut task: ChildSessionTask = serde_json::from_str(fixture).unwrap();
    task.control_state = ChildSessionControlState::UserOwned;
    task.control_revision = 2;
    task.subscription = ChildSessionSubscription::Inbox;
    task.subscription_revision = 4;
    task.depends_on = vec![uuid::Uuid::new_v4()];
    task.waiting_on_dependencies = true;
    task.status = ChildSessionTaskStatus::Preparing;
    let text = summary(&task, Language::En, false);
    for value in [
        "Taken over by user",
        "Control revision: 2",
        "Task list only",
        "Subscription revision: 4",
        "Waiting for prerequisites",
        &task.depends_on[0].to_string(),
    ] {
        assert!(text.contains(value), "missing {value}: {text}");
    }
    task.control_state = ChildSessionControlState::Detached;
    assert!(summary(&task, Language::En, false).contains("Detached from parent"));
    for language in [Language::ZhCn, Language::En, Language::Ja] {
        let help = usage(language);
        for command in [
            "takeover|detach",
            "link",
            "subscription set",
            "--depends-on",
            "--request-id",
            "--expected-control-revision",
            "--expected-subscription-revision",
        ] {
            assert!(help.contains(command));
        }
    }
}

#[test]
fn older_task_responses_infer_subscription_from_notify_without_overwriting_p2() {
    for (notify, expected) in [("none", "none"), ("on_task_terminal", "auto_wake")] {
        let mut value = serde_json::json!({"notify":notify});
        legacy_subscription(&mut value);
        assert_eq!(value["subscription"], expected);
    }
    let mut value = serde_json::json!({"notify":"none", "subscription":"inbox"});
    legacy_subscription(&mut value);
    assert_eq!(value["subscription"], "inbox");
}

#[tokio::test]
async fn invalid_commands_keep_draft_focus_and_request_ids_without_turn_submission() {
    let root = std::env::temp_dir().join(format!("child-command-{}", uuid::Uuid::new_v4()));
    let store = SessionStore::new(&root);
    let mut session = Session::new(root.clone(), None, "test");
    let mut runtime = runtime(&root);
    let mut app = App::new(Vec::new(), Language::En);
    let draft = "/session children start --depends-on invalid Audit | Read it";
    app.input.insert(draft);
    app.composer_expanded = true;
    app.focus = FocusPane::Prompt;
    let id = uuid::Uuid::new_v4();
    app.child_request_ids.insert("pending".into(), id);
    let original = session.id;
    assert!(dispatch(&mut app, &mut session, &store, &mut runtime).await);
    assert_eq!(app.input.text(), draft);
    assert!(app.composer_expanded);
    assert_eq!(app.focus, FocusPane::Prompt);
    assert_eq!(app.child_request_ids.get("pending"), Some(&id));
    assert_eq!(session.id, original);
    assert!(!app.running);
    assert!(
        app.transcript
            .iter()
            .any(|line| line.contains("input retained"))
    );
    assert!(!root.join("daemon").exists());
}

#[tokio::test]
async fn takeover_confirmation_keeps_command_editable_and_detached_parent_is_not_active() {
    let root = std::env::temp_dir().join(format!("child-confirm-{}", uuid::Uuid::new_v4()));
    let store = SessionStore::new(&root);
    let mut session = Session::new(root.clone(), None, "test");
    let mut runtime = runtime(&root);
    let mut app = App::new(Vec::new(), Language::En);
    let draft = format!(
        "/session children takeover {} --expected-control-revision 1",
        uuid::Uuid::new_v4()
    );
    app.input.insert(&draft);
    assert!(dispatch(&mut app, &mut session, &store, &mut runtime).await);
    assert_eq!(app.input.text(), draft);
    assert!(app.transcript.iter().any(|line| line.contains("--yes")));
    assert!(app.child_request_ids.is_empty());
    session.parent_session_id = Some(uuid::Uuid::new_v4());
    session.delegation_detached = true;
    assert!(
        handle("parent", &mut app, &mut session, &store, &mut runtime)
            .await
            .is_err()
    );
    refresh_session_lineage(&mut app, &session, &store);
    let source = session_lineage_line(&app, &session).unwrap();
    assert!(source.contains("Original source"));
    assert!(!source.contains("children parent"));
    assert!(!app.running);
}

#[test]
fn child_controls_are_local_while_navigation_waits_for_a_running_turn() {
    assert!(matches!(
        busy_input("/session children takeover id --expected-control-revision 1 --yes"),
        BusyInput::RunNow
    ));
    assert!(matches!(
        busy_input("/session children subscription set id"),
        BusyInput::RunNow
    ));
    assert!(matches!(
        busy_input("/session children open id"),
        BusyInput::Refuse
    ));
    assert!(matches!(
        busy_input("/session children parent"),
        BusyInput::Refuse
    ));
}
