use super::*;
use crate::harness::input_scope_tests::InputFixture;
use willdeep_core::checkpoint::{CheckpointStatus, SessionCheckpointSink};

#[tokio::test]
async fn local_dispatch_reuses_agent_without_inheriting_debt_but_notifications_resume_it() {
    let mut fixture = InputFixture::new();
    let sink = SessionCheckpointSink {
        store: fixture.store.clone(),
        session_id: fixture.session.id,
    };
    let partial = fixture
        .agent
        .run_checkpointed(
            fixture.session.messages.clone(),
            Message::host_instruction("recover fixture"),
            Some(&sink),
        )
        .await
        .unwrap();
    assert_eq!(
        partial.stop_reason,
        willdeep_core::AgentStopReason::Unverified
    );
    assert_eq!(partial.turns, 3);

    let (tx, rx) = mpsc::unbounded_channel();
    let mut runtime = TuiRuntime {
        home: fixture.root.clone(),
        notifier: crate::notify::Notifier::disabled(),
        skills: Arc::new(SkillCatalog::default()),
        kernel: willdeep_core::EventKernel::new(),
        kernel_store: willdeep_core::kernel_store::KernelStore::new(&fixture.root),
        detached_jobs: willdeep_core::DetachedJobStore::new(&fixture.root),
        context_window: 128_000,
        background_tasks: Arc::new(BackgroundTaskRegistry::default()),
        runtime_submit: crate::daemon::RuntimeSubmitOptions {
            workspace: fixture.session.workspace.clone(),
            profile: None,
            model: None,
            config: None,
        },
        provider_config: willdeep_core::provider::ProviderConfig::new(
            willdeep_core::provider::ProviderKind::OpenAiCompatible,
            willdeep_core::provider::ApiDialect::ChatCompletions,
            "http://127.0.0.1:1/v1",
            "test-only",
            "offline-fixture",
        ),
        local_workspace: fixture.session.workspace.clone(),
        reasoning_level: None,
        tx,
        rx,
    };
    let mut app = App::new(Vec::new(), Language::En);
    for prompt in ["你是谁", "another independent input"] {
        let calls_before = fixture.provider.requests.lock().unwrap().len();
        let history_len = fixture
            .store
            .load(fixture.session.id)
            .unwrap()
            .messages
            .len();
        dispatch_prompt(
            &mut app,
            &mut fixture.session,
            &fixture.store,
            &runtime,
            &fixture.agent,
            &runtime.tx,
            prompt.into(),
        )
        .unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), runtime.rx.recv())
            .await
            .unwrap()
            .unwrap();
        app.local_turn.take().unwrap().await.unwrap();
        let UiMessage::Finished(result, ownership) = result else {
            panic!("expected local turn result")
        };
        let outcome = result.unwrap();
        assert_eq!(
            outcome.stop_reason,
            willdeep_core::AgentStopReason::Finished
        );
        assert_eq!(outcome.turns, 1);
        assert_eq!(outcome.final_text, "Offline identity answer");
        assert_eq!(
            fixture.provider.requests.lock().unwrap().len() - calls_before,
            1
        );
        assert!(outcome.messages[history_len..].iter().all(|message| {
            message.tool_calls.is_empty()
                && message.role != willdeep_core::types::Role::Tool
                && !message
                    .content
                    .contains("[completion-verification-required]")
                && !message.content.starts_with("[execution-recovery]")
        }));
        assert!(
            outcome.messages[history_len..]
                .iter()
                .any(|message| message.content.starts_with("[execution-context]"))
        );
        let retained = fixture
            .store
            .load(fixture.session.id)
            .unwrap()
            .execution_checkpoint
            .unwrap();
        assert_eq!(retained.status, CheckpointStatus::Partial);
        assert_eq!(
            retained.verification_baseline.as_deref(),
            Some("before-release")
        );
        assert_eq!(
            retained.verification_evidence[0].status,
            willdeep_core::tools::VerificationStatus::Failed
        );
        drop(ownership);
        app.running = false;
    }

    let calls_before = fixture.provider.requests.lock().unwrap().len();
    dispatch_notification(
        &mut app,
        &mut fixture.session,
        &fixture.store,
        &fixture.agent,
        &runtime.tx,
        "recover original task".into(),
    )
    .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), runtime.rx.recv())
        .await
        .unwrap()
        .unwrap();
    app.local_turn.take().unwrap().await.unwrap();
    let UiMessage::Finished(result, ownership) = result else {
        panic!("expected notification result")
    };
    let outcome = result.unwrap();
    assert_eq!(
        outcome.stop_reason,
        willdeep_core::AgentStopReason::Unverified
    );
    assert_eq!(outcome.turns, 3);
    assert_eq!(
        fixture.provider.requests.lock().unwrap().len() - calls_before,
        3
    );
    drop(ownership);
}
