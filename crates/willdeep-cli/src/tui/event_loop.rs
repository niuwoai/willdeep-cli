use super::*;

async fn handle_queue_action(
    key: KeyEvent,
    app: &mut App,
    session: &Session,
    runtime: &TuiRuntime,
    agent: &Arc<Agent>,
) {
    if runtime_queue::handle_key(key, app, session, runtime).await {
        return;
    }
    let language = app.language;
    if key.code == KeyCode::Char('a')
        && app
            .prompt_queue
            .as_ref()
            .is_some_and(|panel| panel.editor.is_none())
    {
        let index = app.prompt_queue.as_ref().unwrap().selected;
        app.open_attachments(attachment_panel::Target::Queued(index));
        return;
    }
    if app.handle_queue_key(key) {
        app.prompt_queue.as_mut().unwrap().set_status(None);
        let index = app.prompt_queue.as_ref().unwrap().selected;
        let text = app.queued_prompts[index].text.clone();
        if app.running
            && app.queued_prompts[index].attachments.is_empty()
            && !text.trim_start().starts_with('/')
            && steer_running_turn(app, session, runtime, agent, &text).await
        {
            app.note_steering_delivered(&text, app.queued_submission_time(index));
            app.remove_queued_prompt_row(index);
            app.queued_prompts.remove(index);
            app.prompt_queue.as_mut().unwrap().show_feedback(
                language
                    .text(
                        "已送达，待模型读取",
                        "Delivered, awaiting model read",
                        "送達済み・モデルの読込待ち",
                    )
                    .to_owned(),
                Instant::now(),
            );
        } else {
            app.prompt_queue.as_mut().unwrap().set_status(Some(language.text("暂不能插话，消息仍保留在队列中（附件和命令需等待下一轮）","Cannot steer now; message remains queued (attachments and commands wait for the next turn)","今は送達できません。添付やコマンドは次のターンを待ちます").to_owned()));
        }
    }
}

/// 落一轮已经跑完的结果；撞上并发执行冲突时，重载最新快照再写一次。
///
/// 为什么这里可以「重载后照写」，而 `SessionStore::save` 的冲突判定又必须留着：
/// 本轮的执行所有权由本进程持有（调用方在保存之后才 drop `ownership`），手里这
/// 份消息就是这条会话的权威续写。磁盘上那点变化不是另一轮对话——实测是会话被
/// 运行时接管时守护进程把 `runtime_managed` 置了位，而这个字段恰好算在执行指纹
/// 里（见 `session/execution_state.rs`），于是一次纯标志位的写入被判成了执行状态
/// 冲突。
///
/// 此前这里直接 `?`：用户敲完提示词、模型跑完一整轮，**结果刚要显示的那一刻**
/// TUI 退出，而那一轮的输出连同 token 一起丢掉。为一个标志位赔上一整轮，怎么算
/// 都不划算。
///
/// 取舍说明：如果磁盘上真有另一轮对话（两个执行者同时写一条会话），这里会用本轮
/// 结果覆盖它。那种情况本身已经是坏的，而原先的行为是两边都保不住——至少这样能
/// 保住用户刚刚等来的这一轮。
fn persist_turn_result(session: &mut Session, store: &SessionStore) -> Result<()> {
    match store.save(session) {
        Ok(()) => Ok(()),
        Err(willdeep_core::session::SessionError::ConcurrentUpdate(_)) => {
            let messages = std::mem::take(&mut session.messages);
            // `update` 在锁内从磁盘最新快照起改：别人刚写进去的字段全部保留，
            // 只把本轮的消息放上去。
            *session = store.update(session.id, |latest| latest.messages = messages)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

/// 开屏提示：没有配置文件时说明凭据来自哪里、怎么固化；配置里有旧写法时点名。
fn startup_notices(runtime: &TuiRuntime, language: Language) -> Vec<String> {
    let explicit = runtime.runtime_submit.config.as_deref();
    let mut notices = Vec::new();
    if explicit.is_none() && crate::config::default_config_path().is_ok_and(|path| !path.exists()) {
        notices.push(format!(
            "System: {}",
            language.text(
                "还没有配置文件：本次使用环境变量或命令行提供的凭据；运行 `willdeep config init` 可固化为 ~/.willdeep/config.toml",
                "No config file yet: this run uses credentials from the environment or command line; `willdeep config init` writes them into ~/.willdeep/config.toml",
                "設定ファイルはまだありません。今回は環境変数またはコマンドラインの認証情報を使います。`willdeep config init` で ~/.willdeep/config.toml に保存できます",
            )
        ));
    }
    let loaded = crate::config::LoadedConfig::load(explicit).ok();
    // 围栏是默认开的；开不了得说一声，不然用户以为自己有围栏。
    if loaded
        .as_ref()
        .is_none_or(|loaded| loaded.file.agent.sandbox.is_none())
        && !willdeep_core::sandbox::available()
    {
        notices.push(format!(
            "System: {}",
            language.text(
                "这台机器没有 OS 级写入围栏后端（macOS sandbox-exec / Linux bubblewrap），命令不受围栏保护；配置 agent.sandbox = false 可关掉本提示",
                "No OS write fence backend on this machine (macOS sandbox-exec / Linux bubblewrap): commands run unfenced; set agent.sandbox = false to silence this notice",
                "このマシンには OS 書き込み囲いのバックエンド（macOS sandbox-exec / Linux bubblewrap）がなく、コマンドは囲いなしで実行されます。agent.sandbox = false でこの通知を消せます",
            )
        ));
    }
    if let Some(loaded) = loaded {
        for note in crate::config::deprecations(&loaded.file) {
            let text = match note {
                crate::config::Deprecation::LegacyApproval { spelled } => language
                    .text(
                        "配置里 agent.approval = \"{spelled}\" 是旧写法，等同 strict（写文件、跑命令、联网都先问）；推荐 workspace-write，输入 /permissions default workspace-write 可写回",
                        "agent.approval = \"{spelled}\" in the config is a legacy spelling of strict (every write, command and network call asks); workspace-write is recommended — /permissions default workspace-write writes it back",
                        "設定の agent.approval = \"{spelled}\" は古い書き方で strict と同じ（書き込み・コマンド・通信のたびに確認）。workspace-write を推奨、/permissions default workspace-write で保存できます",
                    )
                    .replace("{spelled}", &spelled),
            };
            notices.push(format!("System: {text}"));
        }
    }
    notices
}

/// 本轮在跑时把用户的话直接送进正在跑的那一轮：Runtime 轮次走 `turn.steer`，
/// 进程内轮次直接进 Agent 收件箱，都是在下一次调模型前注入，不打断手上的活。
/// 送不进去（没有在途任务、Runtime 不可达）返回 false，由调用方排队。
async fn steer_running_turn(
    app: &App,
    session: &Session,
    runtime: &TuiRuntime,
    agent: &Arc<Agent>,
    text: &str,
) -> bool {
    if app.runtime_turn {
        matches!(
            crate::daemon::steer_remote_turn(&runtime.home, session.id, text.to_owned()).await,
            Ok(true)
        )
    } else {
        agent
            .instruction_inbox()
            .is_some_and(|inbox| inbox.push_operator(text.to_owned()))
    }
}

/// 进程内轮次结束时，收件箱里还没送出的插话交回队列，本轮结束后照常发出。
fn requeue_undelivered_steering(app: &mut App, agent: &Arc<Agent>) {
    let Some(inbox) = agent.instruction_inbox() else {
        return;
    };
    for instruction in inbox.drain() {
        if let willdeep_core::AgentInstruction::Operator(text) = instruction {
            app.requeue_steering(text);
        }
    }
}

/// 把输入框交给外部编辑器，读回来只填不发。
async fn open_external_editor(
    term: &mut Terminal<CrosstermBackend<io::Stdout>>,
    events: &mut EventStream,
    app: &mut App,
    language: Language,
) -> Result<()> {
    use external_editor::EditorOutcome;
    let editor = external_editor::resolve_editor(|name| std::env::var(name).ok());
    let original = app.input.text().to_owned();
    let workspace = app.workspace.clone();
    let session =
        external_editor::edit(term, events, &editor, &original, workspace.as_deref()).await;
    let notice = match session {
        Err(error) => format!(
            "{}: {error:#}",
            language.text(
                "外部编辑器出错",
                "External editor failed",
                "外部エディタでエラー"
            )
        ),
        Ok(session) => {
            let mut notice = match session.outcome {
                EditorOutcome::Replaced(text) => {
                    app.edit_input(|input| {
                        input.take();
                        input.insert(&text);
                    });
                    language
                        .text(
                            "已从编辑器读回 · Enter 发送",
                            "Loaded from the editor · Enter to send",
                            "エディタから読み込みました · Enter で送信",
                        )
                        .to_owned()
                }
                EditorOutcome::Unchanged => language
                    .text("编辑器里没有改动", "No changes in the editor", "エディタで変更なし")
                    .to_owned(),
                EditorOutcome::Emptied => language
                    .text(
                        "编辑器里清空了内容，按放弃处理，保留原输入",
                        "The draft was emptied in the editor; kept the original prompt",
                        "エディタで空にされたため破棄し、元の入力を残しました",
                    )
                    .to_owned(),
                EditorOutcome::Cancelled(code) => format!(
                    "{}{}",
                    language.text(
                        "编辑已放弃，保留原输入",
                        "Edit cancelled; kept the original prompt",
                        "編集を破棄し、元の入力を残しました",
                    ),
                    code.map(|code| format!(" (exit {code})")).unwrap_or_default()
                ),
                EditorOutcome::NotFound => format!(
                    "{} `{editor}` · {}",
                    language.text("找不到编辑器", "Editor not found", "エディタが見つかりません"),
                    language.text(
                        "设置 WILLDEEP_EDITOR、VISUAL 或 EDITOR；GUI 编辑器要用阻塞写法，如 mate -w",
                        "set WILLDEEP_EDITOR, VISUAL or EDITOR; GUI editors need a waiting form such as mate -w",
                        "WILLDEEP_EDITOR / VISUAL / EDITOR を設定してください。GUI エディタは mate -w など待機する形で",
                    )
                ),
            };
            if let Some(leftover) = session.leftover {
                notice.push_str(&format!(
                    " · {}: {leftover}",
                    language.text(
                        "临时草稿未删除",
                        "Draft file not removed",
                        "一時ファイルを削除できません"
                    )
                ));
            }
            notice
        }
    };
    app.notice = Some(notice);
    Ok(())
}

pub(super) async fn event_loop(
    term: &mut Terminal<CrosstermBackend<io::Stdout>>,
    agent: Arc<Agent>,
    session: &mut Session,
    store: &SessionStore,
    runtime: &mut TuiRuntime,
    language: Language,
) -> Result<()> {
    // 先跟运行时对齐，再渲染。`runtime_event_head` 会在 CLI 版本变化时停掉旧
    // 守护进程、起一个新的，而新守护在恢复阶段会重写会话文件；把这一步放在
    // 组装 transcript 之后，屏幕上就是一份守护进程刚刚改过的过期记录。
    if session.runtime_event_cursor == 0 {
        let head = crate::daemon::runtime_event_head(&runtime.home)
            .await
            .unwrap_or_default();
        session.runtime_event_cursor = head;
        // 一条还没说过话的会话不该为了记一个事件游标就落盘。此前每开一次 TUI
        // 不敲字就关，磁盘上就多一条 0 消息会话，历史列表被它们挤满——而它们
        // 什么都没记录。游标会在第一条提示词落盘时一起写下去；在那之前丢掉它
        // 的唯一后果是下次从事件流头部重读，而空会话没有任何东西要重放。
        if !session.messages.is_empty() {
            // 用 `update` 而不是 `save`：这里写的只是事件游标这一个书签，而上面
            // 那次 `runtime_event_head` 可能刚好发生过守护进程换版重启——它在恢复
            // 阶段写过这个会话，于是 `save` 手上的基线在这一刻已经过期，冲突会以
            // `concurrent session update conflicts on execution` 把整个 TUI 打死
            // （升级后第一次启动必现）。`update` 从磁盘最新快照起改，书签重放上去
            // 无损，顺带把守护进程刚恢复出来的内容接回内存。
            match store.update(session.id, |latest| latest.runtime_event_cursor = head) {
                Ok(latest) => *session = latest,
                // 还没落过盘的会话没有「最新快照」可改，按原路首次写入。
                Err(willdeep_core::session::SessionError::Io(error))
                    if error.kind() == io::ErrorKind::NotFound =>
                {
                    store.save(session)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    let mut initial_transcript = session_transcript(session, language);
    if initial_transcript.is_empty() {
        initial_transcript.push(welcome_message(&session.workspace, session.id, language));
    }
    // 首次使用与旧写法都在开屏说清楚，不让配置静默改变行为。
    initial_transcript.extend(startup_notices(runtime, language));
    let mut app = App::new(initial_transcript, language);
    app.load_message_times(&runtime.home, session.id);
    app.ui = crate::config::LoadedConfig::load(runtime.runtime_submit.config.as_deref())?
        .file
        .ui;
    if let Some(show_thinking) = crate::config::load_thinking_visibility(&runtime.home)? {
        app.ui.show_thinking = show_thinking;
    }
    app.feedback = agent
        .feedback()
        .cloned()
        .unwrap_or_else(willdeep_core::feedback::FeedbackRecorder::disabled);
    app.approval_mode = agent.approval_mode_handle().get();
    let (media_resize_tx, mut media_resize_rx) =
        mpsc::unbounded_channel::<ratatui_image::thread::ResizeRequest>();
    app.media = MediaState::detect(media_resize_tx);
    app.goal = session.goal.clone();
    app.runtime_event_cursor = session.runtime_event_cursor;
    app.workspace = Some(session.workspace.clone());
    app.workspace_status = workspace_status(&session.workspace, language);
    app.workspace_attention = workspace_attention(&session.workspace);
    app.attention_read = session.attention_read.clone();
    app.display_home = Some(runtime.home.clone());
    if let Ok(display) = crate::daemon::turn_queue::display_session(&runtime.home, session) {
        app.transcript = session_transcript(&display, app.language);
    }
    app.context_window = runtime.context_window.max(1);
    let (_update_worker, mut update_rx) =
        update_notice::spawn(runtime.home.clone(), app.ui.check_updates);
    app.background_tasks = runtime.background_tasks.snapshots();
    let mut background_rx = runtime.background_tasks.subscribe();
    let mut events = EventStream::new();
    let mut refresh = tokio::time::interval(Duration::from_secs(1));
    let mut interrupt_esc_deadline: Option<Instant> = None;
    let (runtime_snapshot_tx, mut runtime_snapshot_rx) =
        mpsc::unbounded_channel::<Result<crate::daemon::RuntimeSnapshot, String>>();
    let snapshot_in_flight = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (runtime_event_tx, mut runtime_event_rx) =
        mpsc::unbounded_channel::<Vec<crate::daemon::RemoteRuntimeEvent>>();
    let mut _runtime_event_follower = crate::daemon::start_runtime_event_follower(
        runtime.home.clone(),
        app.runtime_event_cursor,
        runtime.runtime_submit.workspace.clone(),
        runtime_event_tx.clone(),
    );
    let mut qterm = crate::qterm::ViewState::from_env();
    loop {
        qterm.observe(
            session.id,
            app.running,
            app.runtime_turn,
            app.approval.is_some(),
            app.question.is_some(),
            app.qterm_gate_generation,
        );
        if qterm.reporter.take_error() {
            app.notice = Some(
                language
                    .text(
                        "QTerm 状态连接暂不可用；任务继续运行",
                        "QTerm status connection unavailable; task continues",
                        "QTerm 状態接続を利用できません。タスクは継続します",
                    )
                    .to_owned(),
            );
        }
        if let Some(after) = app.runtime_follow_after.take() {
            while runtime_event_rx.try_recv().is_ok() {}
            _runtime_event_follower = crate::daemon::start_runtime_event_follower(
                runtime.home.clone(),
                after,
                runtime.runtime_submit.workspace.clone(),
                runtime_event_tx.clone(),
            );
        }
        update_notice::show_when_idle(&mut app, &runtime.home);
        // Ctrl+G：终端和事件流都在这一层，按键分支里交不出去。
        if std::mem::take(&mut app.pending_external_editor) {
            open_external_editor(term, &mut events, &mut app, language).await?;
        }
        // 面板里选中的工作区在这里才真正切：切换要动会话、Runtime 与事件跟随器，
        // 按键分支里做不完，也不该在一次按键里做。
        if let Some(id) = app.pending_workspace_switch.take() {
            let previous_workspace = runtime.runtime_submit.workspace.clone();
            match workspace_commands::switch(&mut app, session, store, runtime, &id.to_string())
                .await
            {
                Ok(message) => {
                    app.append_transcript(format!("System: {message}"));
                    if runtime.runtime_submit.workspace != previous_workspace {
                        while runtime_event_rx.try_recv().is_ok() {}
                        _runtime_event_follower = crate::daemon::start_runtime_event_follower(
                            runtime.home.clone(),
                            app.runtime_event_cursor,
                            runtime.runtime_submit.workspace.clone(),
                            runtime_event_tx.clone(),
                        );
                    }
                }
                Err(error) => {
                    app.notice = Some(format!(
                        "{}: {error}",
                        language.text(
                            "切换工作区失败",
                            "Workspace switch failed",
                            "ワークスペースの切り替えに失敗"
                        )
                    ));
                }
            }
        }
        if let Some(target) = app.pending_session_switch.take() {
            if app.running {
                app.notice = Some(
                    language
                        .text(
                            "当前会话正在运行，结束后才能切换历史会话",
                            "Wait for the current turn to finish before switching Sessions",
                            "現在のターンが完了してから履歴セッションを切り替えてください",
                        )
                        .to_owned(),
                );
                continue;
            }
            let target_id = match uuid::Uuid::parse_str(&target.id) {
                Ok(id) => id,
                Err(error) => {
                    app.notice = Some(format!(
                        "{}: {error}",
                        language.text(
                            "历史会话 ID 无效",
                            "Historical Session ID is invalid",
                            "履歴セッション ID が無効です"
                        )
                    ));
                    continue;
                }
            };
            let archived = if target.archived {
                true
            } else {
                crate::daemon::remote_session_states(&runtime.home)
                    .await
                    .ok()
                    .and_then(|states| {
                        states
                            .into_iter()
                            .find(|state| state.id == target_id)
                            .map(|state| state.archived)
                    })
                    .unwrap_or(false)
            };
            let unarchive = if archived {
                crate::daemon::set_remote_session_archived(&runtime.home, target_id, false).await
            } else {
                Ok(())
            };
            let switched = match unarchive {
                Ok(()) => {
                    session_commands::switch(&mut app, session, store, runtime, &target.id).await
                }
                Err(error) => Err(error),
            };
            match switched {
                Ok(message) => app.notice = Some(message),
                Err(error) => {
                    app.notice = Some(format!(
                        "{}: {error}",
                        language.text(
                            "切换历史会话失败",
                            "Switch historical Session failed",
                            "履歴セッションの切り替えに失敗"
                        )
                    ));
                }
            }
        }
        // 本轮里收下的 `/model` 在这里真正切：放在排队提示词之前，它们用的就是新模型。
        if !app.running
            && let Some(model) = app.pending_model.take()
        {
            let result = if let Some(profile) = app.pending_provider.take() {
                model_provider::switch(&profile, &model, &mut app, session, store, runtime, &agent)
                    .await
            } else {
                switch_model(&model, &mut app, session, store, runtime, &agent).await
            };
            match result {
                Ok(message) => app.append_transcript(format!("System: {message}")),
                Err(error) => app.append_transcript(format!(
                    "Error: {}: {error:#}",
                    language.text("切换模型失败", "Model switch failed", "モデル切替に失敗")
                )),
            }
        }
        // 用户已提交的队列本身就是启动下一轮的理由。待处理内核事件会在模型
        // 边界投递，不能要求它们先被另一个自动唤醒消费；Runtime 终态、恢复的
        // 事件或被限流的唤醒都可能没有后续回调，否则会永久饿死用户队列。
        if app.queued_steering_ready() {
            let text = app.queued_prompts.front().unwrap().text.clone();
            if steer_running_turn(&app, session, runtime, &agent, &text).await {
                app.note_steering_delivered(&text, app.queued_submission_time(0));
                app.remove_queued_prompt_row(0);
                app.queued_prompts.pop_front();
                app.waiting_background = false;
            } else {
                app.open_prompt_queue();
                app.prompt_queue.as_mut().unwrap().set_status(Some(
                    language
                        .text(
                            "自动发送失败，消息已保留；关闭列表后重试",
                            "Auto-send failed; message retained. Close to retry",
                            "自動送信失敗。メッセージを保持。閉じると再試行",
                        )
                        .to_owned(),
                ));
            }
            continue;
        }
        if app.queued_prompt_ready()
            && let Some(queued) = app.queued_prompts.pop_front()
        {
            let draft_attachments = std::mem::replace(
                &mut app.attachments,
                queued
                    .attachments
                    .iter()
                    .map(|item| DraftAttachment {
                        id: item.id,
                        message: item.message.clone(),
                    })
                    .collect(),
            );
            let draft_selected_attachment = app.selected_attachment;
            app.selected_attachment = 0;
            let result = async {
                match app.queued_prompt_execution(&queued.text, session, store)? {
                    PromptExecution::Local(prompt) if !prompt.is_empty() => dispatch_prompt(
                        &mut app,
                        session,
                        store,
                        &runtime.skills,
                        &agent,
                        &runtime.tx,
                        prompt,
                    ),
                    PromptExecution::Local(_) => Ok(()),
                    PromptExecution::Runtime(prompt) => {
                        runtime_ui::submit_turn(&mut app, session, store, runtime, prompt).await
                    }
                }
            }
            .await;
            app.attachments = draft_attachments;
            app.selected_attachment = draft_selected_attachment;
            match result {
                Ok(()) => app.mark_queued_prompt_sent(&queued.text),
                Err(error) => {
                    app.queued_prompts.push_front(queued);
                    app.open_prompt_queue();
                    app.prompt_queue.as_mut().unwrap().set_status(Some(format!(
                        "{}: {error}",
                        language.text(
                            "发送失败，消息与附件已保留；关闭列表后重试",
                            "Send failed; message and attachments retained. Close to retry",
                            "送信失敗。メッセージと添付を保持。閉じると再試行"
                        )
                    )));
                }
            }
            continue;
        }
        // 本轮刚结束：响一声铃，盯着别处的用户也知道轮到自己了。
        app.ring_turn_bell(term.backend_mut())?;
        if app.input.marker_query('@').is_none() {
            app.references = None;
        } else if app.references.is_none() {
            let config =
                crate::config::LoadedConfig::load(runtime.runtime_submit.config.as_deref())?;
            let root = session.workspace.clone();
            let home = runtime.home.clone();
            let allowed = if app.runtime_version.is_some() {
                match crate::daemon::remote_workspaces(&runtime.home).await {
                    Ok(workspaces) => workspaces
                        .into_iter()
                        .find(|w| w.root == root)
                        .map(|w| w.mcp_servers)
                        .unwrap_or_else(|| vec![String::new()]),
                    Err(_) => {
                        app.notice = Some(language.text("工作区 MCP 列表加载失败，仍可选择智能体和文件", "Workspace MCP list unavailable; agents and files remain available", "MCP 一覧を取得できません。エージェントとファイルは選択できます").to_owned());
                        vec![String::new()]
                    }
                }
            } else {
                Vec::new()
            };
            app.references = Some(
                tokio::task::spawn_blocking(move || {
                    crate::composer_references::collect(&root, &home, &config.file, &allowed, "")
                })
                .await??,
            );
        }
        load_attention_diagnostics(&mut app, runtime);
        draw(term, &mut app, &runtime.skills)?;
        let queue_feedback_deadline = app.queue_feedback_deadline();
        tokio::select! {
            _ = async {
                if let Some(deadline) = queue_feedback_deadline {
                    tokio::time::sleep_until(deadline.into()).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => { app.expire_queue_feedback(Instant::now()); },
            Some(release)=update_rx.recv()=>{app.update_available=Some(release);},
            _=refresh.tick()=>{
                app.save_message_times();
                if let Err(error)=peer_commands::refresh_badge(&mut app,session.id,&runtime.home) {
                    app.notice=Some(format!("{}: {error}",language.text("读取协作未读失败","Cannot read peer inbox","連携受信箱を取得できません")));
                }
                // Webhook delivery is detached; drain its failures here so a
                // dead endpoint shows up as a notice instead of silence.
                if let Some(error)=runtime.notifier.take_error(){app.notice=Some(format!("{}: {error}",language.text("通知 Webhook","Notification webhook","通知 Webhook")));}
                app.background_tasks=runtime.background_tasks.snapshots();
                // 事件内核的用户侧投影跟着秒级刷新走：它是观察面，不需要自己
                // 的通知通道。落盘也在这里收口——状态每变一次就写一次盘，
                // 打字的时候会卡在磁盘上。
                app.kernel_attention=runtime.kernel.pending_for_user().iter().map(AttentionItem::from_kernel_event).collect();
                // 脱离作业没有完成回调，只能在这里按秒看记录。发布之后要像进程内
                // 任务那样唤醒空闲会话，否则结论会一直躺在队列里等用户开口。
                let finished_jobs=match crate::detached_delivery::publish_finished_jobs_for_tui(&runtime.kernel,&runtime.detached_jobs,store,&runtime.kernel_store,session.id,app.running,app.runtime_turn) {
                    Ok(count)=>count,
                    Err(error)=>{app.notice=Some(format!("{}: {error:#}",language.text("后台结果保存失败","Background result persistence failed","バックグラウンド結果の保存に失敗")));0},
                };
                willdeep_core::kernel_store::flush(&runtime.kernel,&runtime.kernel_store);
                if finished_jobs>0 {
                    app.notice=Some(format!("{finished_jobs} background job(s) finished · queued as runtime events"));
                    execute!(term.backend_mut(),crossterm::style::Print("\x07"))?;
                    wake_for_kernel_events(&mut app,session,store,&agent,runtime)?;
                }
                // 监视器事件已经直接进了内核。唤醒是否放行由内核判：被节流的事件只
                // 入队（Enqueue），不会把空闲会话拉起来。忙的时候回合收尾会再看一次。
                if runtime.background_tasks.take_monitor_signals()>0 {
                    willdeep_core::kernel_store::flush(&runtime.kernel,&runtime.kernel_store);
                    wake_for_kernel_events(&mut app,session,store,&agent,runtime)?;
                }
                // 上一份快照还没回来就不再叠加一份：几份并行在途的快照回来的
                // 顺序没有保证，越多越容易把一份旧的排到新的后面。
                if !snapshot_in_flight.swap(true,std::sync::atomic::Ordering::AcqRel) {
                    let home=runtime.home.clone();
                    let tx=runtime_snapshot_tx.clone();
                    let workspace=runtime.runtime_submit.workspace.clone();
                    let session_id=session.id;
                    let home_for_diagnostics=home.clone();
                    let in_flight=snapshot_in_flight.clone();
                    tokio::spawn(async move {
                        let result=crate::daemon::runtime_snapshot(&home,&workspace,Some(session_id),crate::Surface::Tui)
                            .await
                            .map_err(|error| {
                                let home = home_for_diagnostics.display().to_string();
                                format!("{error:#}").replace(&home, "~")
                            });
                        in_flight.store(false,std::sync::atomic::Ordering::Release);
                        let _=tx.send(result);
                    });
                }
            },
            Some(snapshot_result)=runtime_snapshot_rx.recv()=>{
                let snapshot = match snapshot_result {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        // Health may have succeeded while a later snapshot endpoint
                        // failed. Keep the last confirmed Runtime version and expose
                        // the partial-snapshot error instead of silently dropping it.
                        app.runtime_reachable = true;
                        app.runtime_diagnostic = Some(format!(
                            "Runtime 已响应，但读取快照失败：{error}"
                        ));
                        continue;
                    }
                };
                qterm.runtime_snapshot(&snapshot, session.id, app.runtime_event_cursor, app.running);
                if let Some((id, items)) = snapshot.queue
                    && id == session.id {
                    app.sync_runtime_queue(items);
                }
                runtime.notifier.attention_snapshot(&snapshot.attention);
                if app.observe_runtime_tasks(&snapshot.tasks,session.id,snapshot.event_sequence) {
                    runtime_ui::reconcile_stale_runtime_turn(&mut app,session,runtime).await;
                }
                app.runtime_attention=snapshot.attention;
                app.runtime_gates=snapshot.gates;
                app.runtime_agents=snapshot.agents;
                app.runtime_tools=snapshot.tools;
                app.runtime_artifacts=snapshot.artifacts;
                app.mobile_status=snapshot.mobile;
                app.observe_runtime_version(snapshot.runtime_version);
                app.runtime_reachable = app.runtime_version.is_some();
                app.runtime_diagnostic = snapshot.runtime_diagnostic;
                if app.runtime_version.is_some() {
                    app.runtime_auto_start_tried = false;
                }
                if app.runtime_version.is_none()
                    && app.runtime_diagnostic.is_some()
                    && !app.runtime_auto_start_tried
                {
                    app.runtime_auto_start_tried = true;
                    daemon_commands::dispatch(
                        daemon_commands::DaemonCommand::Start,
                        runtime.home.clone(),
                        language,
                        runtime.tx.clone(),
                    );
                }
                if app.runtime_auto_upgrade_pending && !app.running {
                    app.runtime_auto_upgrade_pending=false;
                    app.runtime_auto_upgrade_tried=true;
                    daemon_commands::auto_upgrade(runtime.home.clone(),language,runtime.tx.clone());
                }
                if runtime_ui::surface_pending_gates(&mut app,&runtime.home,&runtime.tx){
                    execute!(term.backend_mut(),crossterm::style::Print("\x07"))?;
                }
            },
            Some(mut events)=runtime_event_rx.recv()=>{
                const RUNTIME_EVENT_BATCH_LIMIT: usize = 1024;
                while events.len() < RUNTIME_EVENT_BATCH_LIMIT {
                    let Ok(mut next) = runtime_event_rx.try_recv() else { break; };
                    events.append(&mut next);
                }
                let failures: Vec<uuid::Uuid> = events.iter().filter(|event| event.sequence > app.runtime_event_cursor && event.visible && event.session_id == Some(session.id) && event.kind == "task.failed")
                    .filter_map(|event| event.message.split_whitespace().find_map(|part| part.strip_prefix("task_id=")).and_then(|id| id.parse().ok())).collect();
                runtime_ui::apply_runtime_events(&mut app,events,session,store)?;
                task_diagnostics::surface_failures(
                    runtime.tx.clone(),
                    runtime.home.clone(),
                    language,
                    failures,
                );
                // Runtime 轮次正常收尾后会话已从磁盘刷新，此时的消息就是预测要看的那份。
                if app.take_runtime_turn_settled() { dispatch_input_suggestion(&app,session,&agent,&runtime.tx); }
            },
            Some(request)=media_resize_rx.recv()=>{
                let tx=runtime.tx.clone();
                tokio::spawn(async move {
                    let result=tokio::task::spawn_blocking(move || request.resize_encode().map_err(|error|error.to_string()))
                        .await
                        .unwrap_or_else(|error|Err(format!("image resize worker failed: {error}")));
                    let _=tx.send(UiMessage::MediaResized(result));
                });
            },
            event=events.next()=>if let Some(Ok(event))=event { match event {
                Event::Paste(value)=>{
                    if app.approval.is_none() && app.question.is_none() && app.attachment_panel.is_some() { app.attachment_paste(&value); continue; }
                    if app.approval.is_none() && app.question.is_none() && let Some(panel)=app.prompt_queue.as_mut() {
                        if let Some(editor)=panel.editor.as_mut() {editor.insert(&value);}
                        continue;
                    }
                    if app.approval.is_some() {
                        app.handle_approval_text(&value);
                    } else if app.routing_settings_paste(&value) {
                    } else if let Some(picker)=app.model_picker.as_mut() {
                        picker.editor.insert(&value);
                        app.refresh_model_picker_matches();
                    } else if let Some(picker)=app.workspace_picker.as_mut() {
                        picker.editor.insert(&value);
                        app.refresh_workspace_picker_matches();
                    } else if let Some(picker)=app.session_picker.as_mut() {
                        picker.editor.insert(&value);
                        refresh_session_picker(&mut app,runtime,session).await;
                    } else {
                        app.handle_paste(value);
                    }
                },
                Event::Mouse(mouse)=>{
                    interrupt_esc_deadline = None;
                    if app.question.is_some()||app.approval.is_some() {
                        if mouse.kind==MouseEventKind::Down(MouseButton::Left) {
                            app.handle_mouse(mouse.column,mouse.row,&runtime.background_tasks,&runtime.skills);
                        }
                        continue;
                    }
                    if app.attachment_panel.is_some() { continue; }
                    if app.prompt_queue.is_some() {
                        if let Some(key) = app.queue_mouse_key(mouse) {
                            handle_queue_action(key, &mut app, session, runtime, &agent).await;
                        }
                        continue;
                    }
                    // 点在弹层边界外 == 按 Esc。放在所有弹层各自的鼠标处理之前，
                    // 否则「点外面」会先被下面那些 `continue` 吃掉。
                    if mouse.kind==MouseEventKind::Down(MouseButton::Left)
                        && (app.dismiss_overlay_on_close_click(mouse.column,mouse.row)
                            || app.dismiss_overlay_on_outside_click(mouse.column,mouse.row))
                    {
                        continue;
                    }
                    if app.media.is_open() {
                        let action=app.media.handle_mouse(mouse);
                        dispatch_media_action(action,&mut app,runtime);
                        continue;
                    }
                    if app.routing_settings.is_some() {
                        continue;
                    }
                    if app.model_picker.is_some() {
                        match mouse.kind {
                            MouseEventKind::ScrollUp|MouseEventKind::ScrollDown=>{
                                let delta=if mouse.kind==MouseEventKind::ScrollUp{-1}else{1};
                                let action=app.model_picker_scroll_at(delta,mouse.column,mouse.row);
                                model_provider::handle(action,&mut app,session,store,runtime,&agent).await;
                            },
                            MouseEventKind::Down(MouseButton::Left)=>{
                                if let Some(action)=app.activate_model_picker_at(mouse.column,mouse.row) {
                                    model_provider::handle(action, &mut app, session, store, runtime, &agent).await;
                                }
                            },
                            _=>{},
                        }
                        continue;
                    }
                    if app.workspace_picker.is_some() {
                        match mouse.kind {
                            MouseEventKind::ScrollUp=>app.workspace_picker_scroll(-1),
                            MouseEventKind::ScrollDown=>app.workspace_picker_scroll(1),
                            MouseEventKind::Down(MouseButton::Left)=>{
                                if let Some(id)=app.activate_workspace_picker_at(mouse.column,mouse.row) {
                                    app.workspace_picker=None;
                                    app.pending_workspace_switch=Some(id);
                                }
                            },
                            _=>{},
                        }
                        continue;
                    }
                    if app.session_picker.is_some() {
                        if mouse.kind==MouseEventKind::Down(MouseButton::Left) {
                            app.activate_session_picker_at(mouse.column,mouse.row);
                        }
                        continue;
                    }
                    if let Some(action)=diff_review_mouse_action(app.diff_review.is_some(),mouse.kind) {
                        if let Some(review)=app.diff_review.as_mut()
                            && review.preview_draft.is_none()
                        {
                            match action {
                                DiffReviewMouseAction::ScrollUp=>review.scroll=review.scroll.saturating_sub(3),
                                DiffReviewMouseAction::ScrollDown=>review.scroll=review.scroll.saturating_add(3),
                                DiffReviewMouseAction::Consume=>{},
                            }
                        }
                        continue;
                    }
                    if mouse.kind==MouseEventKind::Down(MouseButton::Left)
                        && let Some(action)=app.diff_attention_action_at(mouse.column,mouse.row)
                    {
                        if let Err(error)=handle_diff_attention_action(action,&mut app,session,store,runtime,language).await {
                            app.notice=Some(format!("{}: {error}",language.text("Diff 操作失败","Diff action failed","Diff 操作に失敗")));
                        }
                        continue;
                    }
                    if mouse.kind==MouseEventKind::Down(MouseButton::Left)
                        && let Some(action)=app.agent_detail_action_at(mouse.column,mouse.row)
                    {
                        handle_agent_detail_action(action,&mut app,runtime,language).await;
                        continue;
                    }
                    if app.diff_review.is_none()
                        && app.agent_detail.is_none()
                        && app.task_detail.is_none()
                        && app.attention_detail.is_none()
                        && app.worktree_review.is_none()
                        && app.handle_chat_selection_mouse(mouse)
                    {
                        continue;
                    }
                    match mouse.kind {
                    MouseEventKind::Down(_)=>{
                        app.handle_mouse(mouse.column,mouse.row,&runtime.background_tasks,&runtime.skills);
                        if app.attention_detail.is_some()
                            && let Some(gate)=app.selected_remote_gate()
                        {
                            app.attention_detail=None;
                            open_remote_gate(&mut app,gate,runtime.home.clone(),runtime.tx.clone());
                        }
                    },
                    MouseEventKind::ScrollUp if app.agent_detail.is_some()=>app.agent_detail_scroll=app.agent_detail_scroll.saturating_sub(3),
                    MouseEventKind::ScrollDown if app.agent_detail.is_some()=>app.agent_detail_scroll=app.agent_detail_scroll.saturating_add(3),
                    MouseEventKind::ScrollUp if app.task_detail.is_some()=>app.task_detail_scroll=app.task_detail_scroll.saturating_sub(3),
                    MouseEventKind::ScrollDown if app.task_detail.is_some()=>app.task_detail_scroll=app.task_detail_scroll.saturating_add(3),
                    MouseEventKind::ScrollUp if app.attention_detail.is_some()=>app.attention_detail_scroll=app.attention_detail_scroll.saturating_sub(3),
                    MouseEventKind::ScrollDown if app.attention_detail.is_some()=>app.attention_detail_scroll=app.attention_detail_scroll.saturating_add(3),
                    MouseEventKind::ScrollUp if app.sidebar_rect.contains((mouse.column,mouse.row).into())=>app.sidebar_scroll_by(-3),
                    MouseEventKind::ScrollDown if app.sidebar_rect.contains((mouse.column,mouse.row).into())=>app.sidebar_scroll_by(3),
                    MouseEventKind::ScrollUp if app.transcript_rect.contains((mouse.column,mouse.row).into())=>{app.focus=FocusPane::Chat;app.scroll_up(3);},
                    MouseEventKind::ScrollDown if app.transcript_rect.contains((mouse.column,mouse.row).into())=>{app.focus=FocusPane::Chat;app.scroll_down(3);},
                    MouseEventKind::ScrollUp=>app.scroll_up(3),
                    MouseEventKind::ScrollDown=>app.scroll_down(3),
                    _=>{}
                }},
                Event::Key(key) if key.kind==KeyEventKind::Press=>{
                    if key.code != KeyCode::Esc {
                        interrupt_esc_deadline = None;
                    }
                    if app.native_selection_mode {
                        // 复制键留给终端自己的选区：放行的话 Ctrl+C 会直接退出程序。
                        if is_selection_copy_key(key)||(key.code==KeyCode::Char('c')&&key.modifiers.contains(KeyModifiers::CONTROL)) {
                            continue;
                        }
                        execute!(term.backend_mut(), EnableMouseCapture)?;
                        if selection_mode_exit_key(key) {
                            app.exit_selection_mode();
                            app.notice=Some(language.text("已恢复 WillDeep 鼠标操作","WillDeep mouse controls restored","WillDeep のマウス操作を復元しました").to_owned());
                            continue;
                        }
                        // 其它键：退出原生选区，这个键照常处理（打字就进输入框）。
                        app.release_selection_for_key(key);
                    } else if app.selection_mode {
                        if is_selection_copy_key(key) {
                            app.copy_chat_selection();
                            continue;
                        } else if key.code==KeyCode::Char('q')&&!key.modifiers.intersects(KeyModifiers::CONTROL|KeyModifiers::SUPER|KeyModifiers::ALT) {
                            app.quote_chat_selection();
                            continue;
                        } else if selection_mode_exit_key(key) {
                            app.exit_selection_mode();
                            app.notice=Some(language.text("已恢复鼠标滚动和点击","Mouse scrolling and clicks restored","マウス操作を復元しました").to_owned());
                            continue;
                        }
                        // 以前这里把其余按键一律吞掉，选过字就打不了字、也没有任何提示。
                        app.release_selection_for_key(key);
                    }
                    if app.routing_settings.is_none()&&key.modifiers.contains(KeyModifiers::CONTROL)&&key.code==KeyCode::Char('s'){
                        execute!(term.backend_mut(), DisableMouseCapture)?;
                        app.enter_native_selection_mode();
                        continue;
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL)&&key.code==KeyCode::Char('c'){break;}
                    if key.code==KeyCode::Esc&&app.mobile_qr.take().is_some(){continue;}
                    if app.question.is_some(){app.handle_question_key(key);continue;}
                    if app.approval.is_some(){
                        app.handle_approval_key(key);
                        continue;
                    }
                    if app.media.is_open() { let action=app.media.handle_key(key); dispatch_media_action(action,&mut app,runtime); continue; }
                    if app.attachment_panel.is_some() { let action=app.handle_attachment_key(key); dispatch_media_action(action,&mut app,runtime); continue; }
                    if app.prompt_queue.is_some() {
                        handle_queue_action(key, &mut app, session, runtime, &agent).await;
                        continue;
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL)&&key.code==KeyCode::Char('q') {
                        app.open_prompt_queue();
                        continue;
                    }
                    if app.media.is_open(){
                        let action=app.media.handle_key(key);
                        dispatch_media_action(action,&mut app,runtime);
                        continue;
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL)&&key.code==KeyCode::Char('l'){
                        if !app.media.open(&app.transcript){
                            app.notice=Some(language.text("当前会话里没有链接或图片","No links or images in this Session","現在のセッションにリンクや画像はありません").to_owned());
                        }
                        continue;
                    }
                    if let Some(detail)=app.attention_detail.clone(){
                        if detail.source==AttentionSource::DiffReview {
                            let action=diff_attention_action_for_key(key.code);
                            if let Some(action)=action {
                                if let Err(error)=handle_diff_attention_action(action,&mut app,session,store,runtime,language).await {
                                    app.notice=Some(format!("{}: {error}",language.text("Diff 操作失败","Diff action failed","Diff 操作に失敗")));
                                }
                            } else if key.code==KeyCode::Esc {
                                app.attention_detail=None;
                            }
                        } else {
                            match key.code {
                                KeyCode::Esc => app.attention_detail=None,
                                KeyCode::Enter => {
                                    app.attention_diagnostics = None;
                                    app.attention_diagnostics_loading = None;
                                }
                                KeyCode::Up => app.attention_detail_scroll=app.attention_detail_scroll.saturating_sub(1),
                                KeyCode::Down => app.attention_detail_scroll=app.attention_detail_scroll.saturating_add(1),
                                KeyCode::PageUp => app.attention_detail_scroll=app.attention_detail_scroll.saturating_sub(10),
                                KeyCode::PageDown => app.attention_detail_scroll=app.attention_detail_scroll.saturating_add(10),
                                KeyCode::Home => app.attention_detail_scroll=0,
                                _ => {},
                            }
                        }
                        if detail.source != AttentionSource::DiffReview && matches!(key.code,KeyCode::Char('m')|KeyCode::Char('M')) {
                            // Dismiss from the detail popup itself. Until now
                            // `M` only worked with the sidebar focused on the
                            // Inbox section, so an item whose only sane action
                            // was "stop showing me this" had no exit here.
                            if app.attention_dismiss(&detail.id) {
                                session.attention_read=app.attention_read.clone();
                                store.save(session)?;
                                app.notice=Some(language.text("已从 Inbox 移除该条目","Item dismissed from the Inbox","この項目を Inbox から削除しました").to_owned());
                            } else {
                                app.notice=Some(language.text("运行中的条目不能忽略","A running item cannot be dismissed","実行中の項目は削除できません").to_owned());
                            }
                        }
                        else if key.code==KeyCode::Enter
                            && let Some(gate)=app.selected_remote_gate()
                        {
                            app.attention_detail=None;
                            open_remote_gate(&mut app,gate,runtime.home.clone(),runtime.tx.clone());
                        }
                        continue;
                    }
                    if app.task_detail.is_some(){app.handle_task_detail_key(key,&runtime.background_tasks);continue;}
                    if let Some(review)=app.worktree_review.clone(){
                        match key.code {
                            KeyCode::Esc=>app.worktree_review=None,
                            KeyCode::Char('m')|KeyCode::Char('M') if review.can_merge=>{
                                match crate::daemon::remote_merge(&runtime.home,review.agent_id,review.id).await {
                                    Ok(result)=>{app.notice=Some(format!("{} · {}",language.text("Worktree 已合并","Worktree merged","Worktree をマージしました"),result.root_snapshot_id));app.worktree_review=None;app.agent_detail=None;},
                                    Err(error)=>app.notice=Some(format!("{}: {error}",language.text("合并失败","Merge failed","マージ失敗"))),
                                }
                            }
                            _=>{}
                        }
                        continue;
                    }
                    if let Some(agent)=app.agent_detail.clone(){
                        match key.code {
                            KeyCode::Esc=>{app.agent_detail=None;app.agent_detail_scroll=0;},
                            KeyCode::Up=>app.agent_detail_scroll=app.agent_detail_scroll.saturating_sub(1),
                            KeyCode::Down=>app.agent_detail_scroll=app.agent_detail_scroll.saturating_add(1),
                            KeyCode::PageUp=>app.agent_detail_scroll=app.agent_detail_scroll.saturating_sub(8),
                            KeyCode::PageDown=>app.agent_detail_scroll=app.agent_detail_scroll.saturating_add(8),
                            KeyCode::Home=>app.agent_detail_scroll=0,
                            KeyCode::End=>app.agent_detail_scroll=usize::MAX,
                            KeyCode::Char('i')|KeyCode::Char('I') if agent.background&&agent.status==willdeep_core::RuntimeStatus::Working=>handle_agent_detail_action(AgentDetailAction::Instruct,&mut app,runtime,language).await,
                            KeyCode::Char('k')|KeyCode::Char('K') if agent.background&&matches!(agent.status,willdeep_core::RuntimeStatus::Working|willdeep_core::RuntimeStatus::Queued)=>handle_agent_detail_action(AgentDetailAction::Stop,&mut app,runtime,language).await,
                            KeyCode::Char('r')|KeyCode::Char('R') if agent.background&&matches!(agent.status,willdeep_core::RuntimeStatus::Blocked|willdeep_core::RuntimeStatus::Failed|willdeep_core::RuntimeStatus::Interrupted|willdeep_core::RuntimeStatus::Done|willdeep_core::RuntimeStatus::Partial|willdeep_core::RuntimeStatus::Cancelled)=>handle_agent_detail_action(AgentDetailAction::Retry,&mut app,runtime,language).await,
                            KeyCode::Char('m')|KeyCode::Char('M') if agent.background&&matches!(agent.status,willdeep_core::RuntimeStatus::Blocked|willdeep_core::RuntimeStatus::Failed|willdeep_core::RuntimeStatus::Interrupted|willdeep_core::RuntimeStatus::Done|willdeep_core::RuntimeStatus::Partial|willdeep_core::RuntimeStatus::Cancelled)=>handle_agent_detail_action(AgentDetailAction::RetryWithModel,&mut app,runtime,language).await,
                            KeyCode::Char('w')|KeyCode::Char('W') if agent.dedicated_worktree=>handle_agent_detail_action(AgentDetailAction::ReviewWorktree,&mut app,runtime,language).await,
                            _=>{}
                        }
                        continue;
                    }
                    if app.diff_review.is_some(){
                        let mut close=false;
                        let mut force_full_redraw=false;
                        let mut open_file=None;
                        let mut review_action=None;
                        let mut revert_action=None;
                        let mut commit_preview_action=None;
                        let mut preview_draft_handled=false;
                        if let Some(review)=app.diff_review.as_mut(){
                            if review.commit_preview.is_some() {
                                if key.code==KeyCode::Esc{review.commit_preview=None;}
                                continue;
                            } else if let Some(draft)=review.preview_draft.as_mut() {
                                preview_draft_handled=true;
                                match key.code {
                                    KeyCode::Esc=>review.preview_draft=None,
                                    KeyCode::Tab=>draft.field=(draft.field+1)%3,
                                    KeyCode::BackTab=>draft.field=draft.field.checked_sub(1).unwrap_or(2),
                                    KeyCode::Enter if !draft.message.text().trim().is_empty()=>{
                                        commit_preview_action=Some((review.snapshot.id.clone(),draft.message.text().to_owned(),draft.remote.text().to_owned(),draft.tag.text().to_owned()));
                                        review.preview_draft=None;
                                    },
                                    KeyCode::Left=>draft.editor_mut().left(),
                                    KeyCode::Right=>draft.editor_mut().right(),
                                    KeyCode::Home=>draft.editor_mut().home(),
                                    KeyCode::End=>draft.editor_mut().end(),
                                    KeyCode::Backspace=>draft.editor_mut().backspace(),
                                    KeyCode::Delete=>draft.editor_mut().delete(),
                                    KeyCode::Char(value) if !key.modifiers.intersects(KeyModifiers::CONTROL|KeyModifiers::SUPER)=>draft.editor_mut().insert(&value.to_string()),
                                    _=>{},
                                }
                            } else if review.confirm_revert {
                                if matches!(key.code,KeyCode::Char('y')|KeyCode::Char('Y')) {
                                    revert_action=review.content.as_ref().map(|(path,_)|(review.snapshot.id.clone(),path.clone(),review.area));
                                }
                                review.confirm_revert=false;
                                if revert_action.is_none(){app.notice=Some(language.text("已取消撤销","Revert cancelled","取り消しをキャンセルしました").to_owned());}
                            } else if review.search.is_some() {
                                match key.code {
                                    KeyCode::Esc => review.search = None,
                                    KeyCode::Enter if !review.search_matches.is_empty() => {
                                        review.search_selected = if key.modifiers.contains(KeyModifiers::SHIFT) {
                                            review.search_selected.checked_sub(1).unwrap_or(review.search_matches.len() - 1)
                                        } else {
                                            (review.search_selected + 1) % review.search_matches.len()
                                        };
                                        review.scroll = review.search_matches[review.search_selected];
                                    }
                                    KeyCode::Left => review.search.as_mut().unwrap().left(),
                                    KeyCode::Right => review.search.as_mut().unwrap().right(),
                                    KeyCode::Home => review.search.as_mut().unwrap().home(),
                                    KeyCode::End => review.search.as_mut().unwrap().end(),
                                    KeyCode::Backspace => { review.search.as_mut().unwrap().backspace(); refresh_diff_search(review); }
                                    KeyCode::Delete => { review.search.as_mut().unwrap().delete(); refresh_diff_search(review); }
                                    KeyCode::Char(value) if !key.modifiers.intersects(KeyModifiers::CONTROL|KeyModifiers::SUPER) => {
                                        review.search.as_mut().unwrap().insert(&value.to_string());
                                        refresh_diff_search(review);
                                    }
                                    _ => {}
                                }
                                continue;
                            } else {match key.code {
                                KeyCode::Esc if review.content.is_some()=>{review.content=None;review.scroll=0;force_full_redraw=true;},
                                KeyCode::Esc=>{close=true;force_full_redraw=true;},
                                KeyCode::Up if review.content.is_none()=>review.selected=review.selected.checked_sub(1).unwrap_or(review.snapshot.files.len().saturating_sub(1)),
                                KeyCode::Down if review.content.is_none()&&!review.snapshot.files.is_empty()=>review.selected=(review.selected+1)%review.snapshot.files.len(),
                                KeyCode::Enter if review.content.is_none()=>open_file=review.snapshot.files.get(review.selected).map(|file|(review.snapshot.id.clone(),file.path.clone(),review.area)),
                                KeyCode::Char('v')|KeyCode::Char('V') if review.content.is_some()=>{
                                    review.view=match review.view {DiffViewMode::Unified=>DiffViewMode::SideBySide,DiffViewMode::SideBySide=>DiffViewMode::Unified};
                                    refresh_diff_search(review);
                                },
                                KeyCode::Char('s')|KeyCode::Char('S') if review.content.is_some()=>{
                                    review.area=next_diff_area(review.area);
                                    open_file=review.content.as_ref().map(|(path,_)|(review.snapshot.id.clone(),path.clone(),review.area));
                                },
                                KeyCode::Char('/') if review.content.is_some()=>review.search=Some(PromptEditor::default()),
                                KeyCode::Char('n') if !review.search_matches.is_empty()=>{
                                    review.search_selected=(review.search_selected+1)%review.search_matches.len();
                                    review.scroll=review.search_matches[review.search_selected];
                                },
                                KeyCode::Char('N') if !review.search_matches.is_empty()=>{
                                    review.search_selected=review.search_selected.checked_sub(1).unwrap_or(review.search_matches.len()-1);
                                    review.scroll=review.search_matches[review.search_selected];
                                },
                                KeyCode::Char('a')|KeyCode::Char('A') if review.content.is_some()=>review_action=review.content.as_ref().map(|(path,_)|(review.snapshot.id.clone(),path.clone(),crate::daemon::diff_review::ReviewDecision::Accepted)),
                                KeyCode::Char('d')|KeyCode::Char('D') if review.content.is_some()=>review_action=review.content.as_ref().map(|(path,_)|(review.snapshot.id.clone(),path.clone(),crate::daemon::diff_review::ReviewDecision::Rejected)),
                                KeyCode::Char('c')|KeyCode::Char('C') if review.content.is_some()=>review_action=review.content.as_ref().map(|(path,_)|(review.snapshot.id.clone(),path.clone(),crate::daemon::diff_review::ReviewDecision::ChangesRequested)),
                                KeyCode::Char('m')|KeyCode::Char('M') if review.content.is_some()=>review_action=review.content.as_ref().map(|(path,_)|(review.snapshot.id.clone(),path.clone(),crate::daemon::diff_review::ReviewDecision::Reviewed)),
                                KeyCode::Char('r')|KeyCode::Char('R') if review.content.is_some()=>review.confirm_revert=true,
                                KeyCode::Char('p')|KeyCode::Char('P')=>review.preview_draft=Some(CommitPreviewDraft::default()),
                                KeyCode::Up=>review.scroll=review.scroll.saturating_sub(1),
                                KeyCode::Down=>review.scroll=review.scroll.saturating_add(1),
                                KeyCode::PageUp=>review.scroll=review.scroll.saturating_sub(10),
                                KeyCode::PageDown=>review.scroll=review.scroll.saturating_add(10),
                                KeyCode::Home=>review.scroll=0,
                                _=>{}
                            }}
                        }
                        if preview_draft_handled&&commit_preview_action.is_none(){continue;}
                        if force_full_redraw{term.clear()?;}
                        if close{app.diff_review=None;continue;}
                        if let Some((snapshot_id,path,area))=open_file{
                            match crate::daemon::diff_review::remote_content(&runtime.home,&session.workspace,&snapshot_id,&path,area).await{
                                Ok(content)=>if let Some(review)=app.diff_review.as_mut(){review.content=Some((path,content));review.scroll=0;review.search_matches.clear();review.search_selected=0;},
                                Err(error)=>app.notice=Some(format!("{}: {error}",language.text("打开 Diff 失败","Open Diff failed","Diff を開けませんでした"))),
                            }
                        }
                        if let Some((snapshot_id,path,decision))=review_action{
                            let request=crate::daemon::diff_review::ReviewRequest{workspace:session.workspace.clone(),path:path.clone(),decision,note:None};
                            match crate::daemon::diff_review::remote_review(&runtime.home,&snapshot_id,&request).await{
                                Ok(record)=>{if let Some(review)=app.diff_review.as_mut(){review.reviews.insert(path,record.decision);}app.notice=Some(language.text("审查决定已保存","Review decision saved","レビュー結果を保存しました").to_owned());},
                                Err(error)=>app.notice=Some(format!("{}: {error}",language.text("保存审查决定失败","Save review decision failed","レビュー結果を保存できませんでした"))),
                            }
                        }
                        if let Some((snapshot_id,path,area))=revert_action{
                            let request=crate::daemon::diff_review::RevertRequest{workspace:session.workspace.clone(),path,area};
                            match crate::daemon::diff_review::remote_revert(&runtime.home,&snapshot_id,&request).await{
                                Ok(result)=>match crate::daemon::diff_review::remote_snapshot(&runtime.home,&session.workspace).await{
                                    Ok(snapshot)=>{if let Some(review)=app.diff_review.as_mut(){review.snapshot=snapshot;review.content=None;review.scroll=0;review.search_matches.clear();review.reviews.clear();review.verifications.clear();review.attributions.clear();}app.notice=Some(if let Some(path)=result.recovery_path{format!("{}: {}",language.text("已安全撤销，可从回收区恢复","Safely reverted; recovery copy","安全に戻しました。復元先"),path.display())}else{language.text("已安全撤销文件变更","File changes safely reverted","ファイル変更を安全に戻しました").to_owned()});},
                                    Err(error)=>app.notice=Some(format!("{}: {error}",language.text("撤销成功，但刷新 Diff 失败","Reverted, but refresh failed","取り消しましたが更新に失敗しました"))),
                                },
                                Err(error)=>app.notice=Some(format!("{}: {error}",language.text("安全撤销失败","Safe revert failed","安全な取り消しに失敗しました"))),
                            }
                        }
                        if let Some((snapshot_id,message,remote,tag))=commit_preview_action{
                            let tag=(!tag.trim().is_empty()).then_some(tag);
                            match crate::daemon::diff_review::remote_commit_preview(&runtime.home,&session.workspace,&snapshot_id,&message,&remote,tag.as_deref()).await{
                                Ok(preview)=>if let Some(review)=app.diff_review.as_mut(){review.commit_preview=Some(preview);},
                                Err(error)=>app.notice=Some(format!("{}: {error}",language.text("生成 Commit Preview 失败","Commit Preview failed","Commit Preview に失敗しました"))),
                            }
                        }
                        continue;
                    }
                    if app.routing_settings.is_some(){
                        match app.handle_routing_settings_key(key) {
                            RoutingSettingsAction::None=>{},
                            RoutingSettingsAction::Close=>app.routing_settings=None,
                            RoutingSettingsAction::Save(update)=>{
                                let config_path=runtime.runtime_submit.config.clone().map(Ok).unwrap_or_else(crate::config::default_config_path);
                                match config_path.and_then(|path|crate::model_routing::save(&path,runtime.runtime_submit.profile.as_deref(),&update)) {
                                    Ok(settings)=>{
                                        app.set_routing_settings_saved(settings);
                                        app.notice=Some(language.text("模型与路由设置已保存","Models and routing saved","モデルとルーティングを保存しました").to_owned());
                                    },
                                    Err(error)=>app.set_routing_settings_error(error.to_string()),
                                }
                            },
                        }
                        continue;
                    }
                    if app.permission_picker.is_some(){
                        match app.handle_permission_picker_key(key) {
                            PermissionPickerAction::None=>{},
                            PermissionPickerAction::Close=>app.permission_picker=None,
                            PermissionPickerAction::Apply(mode)=>{
                                app.permission_picker=None;
                                match permission_commands::apply(mode,&mut app,session,runtime,&agent).await {
                                    Ok(message)=>app.append_transcript(message),
                                    Err(error)=>app.append_transcript(format!("Error: {}: {error:#}",language.text("切换审批模式失败","Approval mode switch failed","承認モードの切替に失敗"))),
                                }
                            },
                        }
                        continue;
                    }
                    if app.model_picker.is_some(){
                        let action = app.handle_model_picker_key(key);
                        model_provider::handle(action, &mut app, session, store, runtime, &agent).await;
                        continue;
                    }
                    if app.workspace_picker.is_some(){
                        match app.handle_workspace_picker_key(key) {
                            WorkspacePickerAction::None=>{},
                            WorkspacePickerAction::Close=>app.workspace_picker=None,
                            WorkspacePickerAction::Select(id)=>{
                                app.workspace_picker=None;
                                app.pending_workspace_switch=Some(id);
                            },
                        }
                        continue;
                    }
                    if app.session_picker.is_some(){
                        match app.handle_session_picker_key(key) {
                            SessionPickerAction::None=>{},
                            SessionPickerAction::Close=>app.session_picker=None,
                            SessionPickerAction::Switch(target)=>{
                                app.pending_session_switch=Some(target);
                                app.session_picker=None;
                            },
                            SessionPickerAction::Refresh=>refresh_session_picker(&mut app,runtime,session).await,
                        }
                        continue;
                    }
                    if app.rewind_picker.is_some(){
                        match app.handle_rewind_picker_key(key) {
                            RewindPickerAction::None=>{},
                            RewindPickerAction::Close=>app.rewind_picker=None,
                            RewindPickerAction::Rewind{step,through_turn_id,restore_workspace}=>{
                                app.rewind_picker=None;
                                match crate::daemon::rewind_remote_session(&runtime.home,session.id,through_turn_id,restore_workspace).await {
                                    Ok(result)=>{
                                        // Runtime 已经把会话截断落盘；本地这份是旧的，整份重读再重画。
                                        match store.load(session.id) {
                                            Ok(loaded)=>{*session=loaded;app.load_session(session);},
                                            Err(error)=>app.notice=Some(format!("{}: {error}",language.text("重新读取会话失败","Reload Session failed","セッションの再読込に失敗"))),
                                        }
                                        app.append_transcript(rewind_summary(&result,step,language));
                                    },
                                    Err(error)=>app.notice=Some(format!("{}: {error}",language.text("回退失败","Rewind failed","巻き戻しに失敗"))),
                                }
                            },
                        }
                        continue;
                    }
                    if app.palette.is_some(){app.handle_palette_key(key,&runtime.background_tasks);continue;}
                    if app.search.is_some(){app.handle_search_key(key);continue;}
                    if app.handle_help_key(key) {continue;}
                    if key.code == KeyCode::F(2) {
                        app.toggle_composer_expanded();
                        continue;
                    }
                    if key.code == KeyCode::F(3) {
                        app.open_attachments(super::attachment_panel::Target::Draft);
                        continue;
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL)&&key.code==KeyCode::Char('p'){
                        app.open_palette(&runtime.skills,store,session);
                        continue;
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL)&&key.code==KeyCode::Char('r'){
                        if app.running {
                            app.notice=Some(language.text(
                                "当前会话正在运行，结束后才能切换历史会话",
                                "Wait for the current turn to finish before switching Sessions",
                                "現在のターンが完了してから履歴セッションを切り替えてください"
                            ).to_owned());
                        } else {
                            app.open_session_picker(session.id,SessionPickerRequest::default());
                            refresh_session_picker(&mut app,runtime,session).await;
                        }
                        continue;
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL)&&key.code==KeyCode::Char('f'){
                        app.search=Some(SearchState::default());
                        continue;
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL)&&key.code==KeyCode::Char('b'){
                        if app.sidebar_wide {
                            app.sidebar_visible = !app.sidebar_visible;
                            if !app.sidebar_visible {app.focus=FocusPane::Prompt;}
                        } else if app.focus==FocusPane::Sidebar {
                            app.focus=FocusPane::Prompt;
                        } else {
                            app.sidebar_visible=true;
                            app.focus=FocusPane::Sidebar;
                        }
                        continue;
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL)&&key.code==KeyCode::Char('w'){
                        app.sidebar_visible=true;
                        app.cycle_focus();
                        continue;
                    }
                    if app.focus==FocusPane::Chat {
                        match key.code {
                            KeyCode::Esc=>app.focus=FocusPane::Prompt,
                            KeyCode::Up=>app.scroll_up(1),
                            KeyCode::Down=>app.scroll_down(1),
                            KeyCode::Home=>app.scroll_to_top(),
                            KeyCode::End=>app.scroll_to_bottom(),
                            _=>{}
                        }
                        continue;
                    }
                    if app.focus==FocusPane::Activity {
                        match key.code {
                            KeyCode::Esc=>app.focus=FocusPane::Prompt,
                            KeyCode::Enter|KeyCode::Char(' ')=>app.tools_expanded = !app.tools_expanded,
                            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL)=>app.tools_expanded = !app.tools_expanded,
                            _=>{}
                        }
                        continue;
                    }
                    if app.focus==FocusPane::Sidebar {
                        match key.code {
                            KeyCode::Esc=>app.focus=FocusPane::Prompt,
                            KeyCode::Up if app.sidebar_selected==1&&app.sidebar_expanded[1]=>app.attention_move(-1),
                            KeyCode::Down if app.sidebar_selected==1&&app.sidebar_expanded[1]=>app.attention_move(1),
                            KeyCode::Up if app.sidebar_selected==2&&app.sidebar_expanded[2]=>app.runtime_agent_move(-1),
                            KeyCode::Down if app.sidebar_selected==2&&app.sidebar_expanded[2]=>app.runtime_agent_move(1),
                            KeyCode::Up=>app.sidebar_move(-1),
                            KeyCode::Down=>app.sidebar_move(1),
                            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT)=>app.sidebar_move(-1),
                            KeyCode::Tab=>app.sidebar_move(1),
                            KeyCode::Enter=>{
                                if let Some(gate)=app.selected_remote_gate(){
                                    open_remote_gate(&mut app,gate,runtime.home.clone(),runtime.tx.clone());
                                }else if app.sidebar_selected==2 {
                                    if let Some(agent)=app.selected_runtime_agent(){
                                        match crate::daemon::remote_agent_detail(&runtime.home,agent.id).await {
                                            Ok(detail)=>app.agent_detail=Some(detail),
                                            Err(error)=>app.notice=Some(format!("{}: {error}",language.text("加载 Agent 详情失败","Failed to load Agent details","Agent 詳細の読み込みに失敗"))),
                                        }
                                        app.agent_detail_scroll=0;
                                    }
                                }else{
                                    app.sidebar_activate(&runtime.background_tasks);
                                    // 打开的是 Runtime 任务详情时，把「哪条命令、为什么挂」一并取回来。
                                    load_attention_diagnostics(&mut app,runtime);
                                }
                            },
                            KeyCode::Char(' ')=>app.sidebar_toggle(),
                            KeyCode::Char('n')|KeyCode::Char('N') if app.sidebar_selected==2=>app.prefill_new_agent(),
                            KeyCode::Char('k')|KeyCode::Char('K') if app.sidebar_selected==1=>{
                                if let Some(id)=app.selected_runtime_task_id(){
                                    match crate::daemon::cancel_remote_task(&runtime.home,id).await {
                                        Ok(())=>app.notice=Some(language.text("已请求停止 Runtime 任务","Runtime task stop requested","Runtime タスクの停止を要求しました").to_owned()),
                                        Err(error)=>app.notice=Some(format!("{}: {error}",language.text("停止失败","Stop failed","停止に失敗"))),
                                    }
                                }else{app.attention_stop(&runtime.background_tasks);}
                            },
                            KeyCode::Char('r')|KeyCode::Char('R') if app.sidebar_selected==1=>{
                                if app.attention_retry(&runtime.background_tasks){
                                    session.attention_read=app.attention_read.clone();
                                    store.save(session)?;
                                }
                            },
                            KeyCode::Char('k')|KeyCode::Char('K') if app.sidebar_selected==2=>{
                                if let Some(agent)=app.selected_runtime_agent(){
                                    match crate::daemon::stop_remote_agent(&runtime.home,agent.id).await {
                                        Ok(())=>app.notice=Some(language.text("已请求停止子 Agent","Child Agent stop requested","子 Agent の停止を要求しました").to_owned()),
                                        Err(error)=>app.notice=Some(format!("{}: {error}",language.text("停止失败","Stop failed","停止に失敗"))),
                                    }
                                }
                            },
                            KeyCode::Char('r')|KeyCode::Char('R') if app.sidebar_selected==2=>{
                                if let Some(agent)=app.selected_runtime_agent(){
                                    match crate::daemon::retry_remote_agent(&runtime.home,agent.id).await {
                                        Ok(())=>app.notice=Some(language.text("已请求重试子 Agent","Child Agent retry requested","子 Agent の再試行を要求しました").to_owned()),
                                        Err(error)=>app.notice=Some(format!("{}: {error}",language.text("重试失败","Retry failed","再試行に失敗"))),
                                    }
                                }
                            },
                            KeyCode::Char('m')|KeyCode::Char('M') if app.sidebar_selected==1=>{
                                if app.attention_mark_read(){
                                    session.attention_read=app.attention_read.clone();
                                    store.save(session)?;
                                }
                            },
                            _=>{}
                        }
                        continue;
                    }
                    // Shift+Tab 只在「严格 → 智能 → 工作区可写」间循环；完全访问必须走
                    // /permissions 的确认页。
                    if key.code==KeyCode::BackTab||(key.code==KeyCode::Tab&&key.modifiers.contains(KeyModifiers::SHIFT)) {
                        let next=app.approval_mode.next_in_cycle();
                        match permission_commands::apply(next,&mut app,session,runtime,&agent).await {
                            Ok(_)=>app.notice=Some(format!("{}：{} · Shift+Tab",language.text("审批模式","Approval mode","承認モード"),permission_commands::label(next,language))),
                            Err(error)=>app.notice=Some(format!("{}: {error:#}",language.text("切换审批模式失败","Approval mode switch failed","承認モードの切替に失敗"))),
                        }
                        continue;
                    }
                    // 空输入框里的灰字预测：Tab 采用（只填入不发送），Esc 放弃。
                    // 命令 / 技能候选层的 Tab 与这里互不相干——那两层只在输入框非空时存在。
                    if key.code==KeyCode::Tab&&key.modifiers.is_empty()&&app.accept_input_suggestion() { continue; }
                    if key.code==KeyCode::Esc&&app.dismiss_input_suggestion() { continue; }
                    if let Some(action) = prompt_line_navigation_for_key(key) {
                        match action {
                            PromptLineNavigation::Start => app.edit_input(|input| input.home()),
                            PromptLineNavigation::End => app.edit_input(|input| input.end()),
                        }
                        continue;
                    }
                    if app.handle_command_key(key) || app.handle_skill_key(key, &runtime.skills) || app.handle_reference_key(key) { continue; }
                    if is_clipboard_image_paste_key(key) {
                        app.paste_clipboard_image();
                        continue;
                    }
                    match key.code {
                        KeyCode::PageUp=>app.scroll_up(app.viewport_height.saturating_sub(1).max(1)),KeyCode::PageDown=>app.scroll_down(app.viewport_height.saturating_sub(1).max(1)),
                        KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT)=>app.scroll_up(1),KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT)=>app.scroll_down(1),
                        KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL)=>app.scroll_to_top(),KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL)=>app.scroll_to_bottom(),
                        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL)=>app.tools_expanded = !app.tools_expanded,
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL)=>app.stash_input_for_queue(),
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL)=>app.delete_selected_attachment(),
                        KeyCode::Enter if key.modifiers.intersects(KeyModifiers::SHIFT|KeyModifiers::ALT)=>app.input.insert("\n"),
                        KeyCode::Enter if is_queue_for_later_key(key)&&(!app.input.is_empty()||!app.attachments.is_empty())=>{
                            // 先入队、以后再发：走服务端队列并暂缓。本地队列不行——
                            // 空闲时 queued_prompt_ready 会立刻把它发出去。
                            if let Err(error) = runtime_queue::submit_input(&mut app,session,runtime,store,true).await {
                                app.notice = Some(format!("{}: {error}", language.text("入队失败，输入与附件已保留", "Could not queue; input and attachments retained", "キューへの追加に失敗。入力と添付を保持しました")));
                            }
                        },
                        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL)=>app.input.insert("\n"),
                        KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL)=>app.pending_external_editor=true,
                        KeyCode::Enter if !app.input.is_empty()||!app.attachments.is_empty()=>{
                            if app.input.text().trim()=="/peers" || app.input.text().trim().starts_with("/peers ") {
                                let command=app.input.take();
                                if let Err(error)=peer_commands::handle(&command,&mut app,session,store,runtime).await {
                                    app.input.insert(&command);
                                    app.notice=Some(format!("{}: {error}",language.text("协作操作失败，输入已保留","Coordination failed; input retained","連携操作に失敗しました。入力は保持されています")));
                                }
                                continue;
                            }
                            if app.input.text().trim()=="/schedule" || app.input.text().trim().starts_with("/schedule ") {
                                let command=app.input.take();
                                let home=runtime.home.clone(); let tx=runtime.tx.clone();
                                tokio::spawn(async move {
                                    let result=match crate::schedule_cmd::slash(&command,&home).await {
                                        Ok(result)=>result,
                                        Err(error)=>format!("{}: {error:#}",language.text("定时任务操作失败","Schedule operation failed","定期タスク操作に失敗")),
                                    };
                                    let _=tx.send(UiMessage::RuntimeResult(result));
                                });
                                continue;
                            }
                            if app.input.text().trim()=="/attachments sent" {
                                app.input.take(); let home=runtime.home.clone(); let id=session.id; let tx=runtime.tx.clone();
                                tokio::task::spawn_blocking(move || {
                                    match SessionStore::new(&home).load(id) {
                                        Ok(saved) => {
                                            let items=saved.messages.into_iter().flat_map(|message| message.attachments.into_iter().map(|message| DraftAttachment { id: uuid::Uuid::new_v4(), message })).collect();
                                            let _=tx.send(UiMessage::SentAttachments { session_id: id, items });
                                        }
                                        Err(error) => { let _=tx.send(UiMessage::RuntimeNotice(format!("{}: {error}", language.text("读取已发送附件失败", "Could not read sent attachments", "送信済み添付を読み込めません")))); }
                                    }
                                });
                                continue;
                            }
                            if app.input.text().trim()=="/attachments" {
                                app.input.take(); app.open_attachments(attachment_panel::Target::Draft); continue;
                            }
                            if app.input.text().trim()=="/queue" {
                                app.take_submitted_input();
                                app.open_prompt_queue();
                                continue;
                            }
                            // 拿不到 Cmd 的终端用这条命令把当前输入先放进队列。
                            // 命令本身不占输入框，带的是上一次暂存的草稿。
                            if app.input.text().trim()=="/queue add" {
                                app.input.take();
                                if app.queue_draft.text().trim().is_empty() && app.queue_draft_attachments.is_empty() {
                                    app.notice = Some(language.text(
                                        "没有可入队的内容；先输入，再按 Ctrl+U 暂存，然后 /queue add",
                                        "Nothing to queue; type it, press Ctrl+U to set it aside, then /queue add",
                                        "追加するものがありません。入力して Ctrl+U で取っておき、/queue add を実行してください",
                                    ).to_owned());
                                    continue;
                                }
                                app.restore_queue_draft();
                                if let Err(error) = runtime_queue::submit_input(&mut app,session,runtime,store,true).await {
                                    app.notice = Some(format!("{}: {error}", language.text("入队失败，输入与附件已保留", "Could not queue; input and attachments retained", "キューへの追加に失敗。入力と添付を保持しました")));
                                }
                                continue;
                            }
                            if confirmation_example::try_submit(&mut app, &runtime.tx) {
                                continue;
                            }
                            // 本轮在跑时不再把 Enter 整条封死：本地命令照常执行，
                            // 提示词排队，其余命令说清楚为什么现在不行。
                            if app.running {
                                match busy_input(app.input.text()) {
                                    BusyInput::RunNow=>{},
                                    BusyInput::Queue=>{
                                        if app.runtime_turn {
                                            if let Err(error) = runtime_queue::submit_input(&mut app,session,runtime,store,false).await {
                                                app.notice = Some(format!("{}: {error}", language.text("提交失败，输入与附件已保留", "Submission failed; input and attachments retained", "送信失敗。入力と添付を保持しました")));
                                            }
                                            continue;
                                        }
                                        app.queue_submitted_prompt();
                                        continue;
                                    },
                                    BusyInput::Refuse=>{
                                        let notice = language.text(
                                            "该命令会改动会话或 Runtime，本轮结束后才能执行；Esc 可立即中断",
                                            "That command changes Session or Runtime state; it runs after this turn. Esc interrupts now",
                                            "このコマンドはセッション/Runtime を変更するため、ターン終了後に実行されます。Esc で即中断",
                                        ).to_owned();
                                        app.notice = Some(notice.clone());
                                        app.notice_hold = Some((notice, Instant::now() + Duration::from_secs(10)));
                                        continue;
                                    },
                                }
                            }
                            let prompt=app.take_submitted_input();app.append_transcript(format!("You: {prompt}"));
                            if let Some(command)=permission_commands::parse(&prompt) {
                                match command {
                                    PermissionCommand::Open=>app.open_permission_picker(false),
                                    PermissionCommand::Switch(willdeep_core::ApprovalMode::FullAccess)=>app.open_permission_picker(true),
                                    PermissionCommand::Switch(mode)=>match permission_commands::apply(mode,&mut app,session,runtime,&agent).await {
                                        Ok(message)=>app.append_transcript(message),
                                        Err(error)=>app.append_transcript(format!("Error: {}: {error:#}",language.text("切换审批模式失败","Approval mode switch failed","承認モードの切替に失敗"))),
                                    },
                                    PermissionCommand::SaveDefault(mode)=>match permission_commands::save_default(mode,runtime) {
                                        Ok(path)=>app.append_transcript(format!("System: {}：{} · {path}",language.text("默认审批模式已写入配置，下次启动生效","Default approval mode saved; applies on next start","既定の承認モードを保存しました（次回起動から有効）"),permission_commands::label(mode,language))),
                                        Err(error)=>app.append_transcript(format!("Error: {}: {error:#}",language.text("写入默认审批模式失败","Save default approval mode failed","既定の承認モードの保存に失敗"))),
                                    },
                                    PermissionCommand::Usage=>app.append_transcript(permission_commands::usage(language)),
                                }
                                continue;
                            }
                            if app.handle_mobile_command(&prompt,&runtime.home,&runtime.tx){continue;}
                            match handle_agent_command(&prompt,&mut app,runtime,session.id).await {
                                Ok(true)=>continue,
                                Ok(false)=>{},
                                Err(error)=>{app.append_transcript(format!("Error: {}: {error}",language.text("Agent 操作失败","Agent action failed","Agent 操作に失敗しました")));continue;},
                            }
                            // `/history` 与 `/session search` 都开面板，必须排在 handle_session_command 前面。
                            match parse_session_picker_command(&prompt) {
                                // 这条分支本身就要求 !app.running；真正的运行中保护在切换那一步。
                                Ok(Some(request))=>{
                                    app.open_session_picker(session.id,request);
                                    refresh_session_picker(&mut app,runtime,session).await;
                                    continue;
                                },
                                Ok(None)=>{},
                                Err(error)=>{app.append_transcript(format!("Error: {}: {error}",language.text("打开历史会话面板失败","Open Session history panel failed","履歴セッションパネルを開けませんでした")));continue;},
                            }
                            match handle_new_session_command(&prompt,&mut app,session,store,runtime).await {
                                Ok(true)=>continue,
                                Ok(false)=>{},
                                Err(error)=>{app.append_transcript(format!("Error: {}: {error}",language.text("开始新会话失败","Start new Session failed","新しいセッションの開始に失敗しました")));continue;},
                            }
                            // `/session retitle` 要 agent 与 UI 通道，落在这里而不是
                            // handle_session_command 里——摘要是网络往返，必须扔进后台任务。
                            if prompt.trim()=="/session retitle" {
                                if session.title_source==willdeep_core::TitleSource::User {
                                    app.append_transcript(format!("System: {}",language.text("这条会话的标题是你自己起的；先 /session rename 交回控制权再重算","This Session's title was set by you; hand it back with /session rename before recomputing","このセッション名はあなたが付けたものです。再計算する前に /session rename で戻してください")));
                                } else {
                                    dispatch_retitle(session,&agent,&runtime.tx,true);
                                    app.append_transcript(format!("System: {}",language.text("正在整理会话标题…","Retitling the Session…","セッション名を整理しています…")));
                                }
                                continue;
                            }
                            match handle_session_command(&prompt,&mut app,session,store,runtime).await {
                                Ok(true)=>continue,
                                Ok(false)=>{},
                                Err(error)=>{app.append_transcript(format!("Error: {}: {error}",language.text("会话操作失败","Session action failed","セッション操作に失敗しました")));continue;},
                            }
                            let previous_workspace=runtime.runtime_submit.workspace.clone();
                            match handle_workspace_command(&prompt,&mut app,session,store,runtime).await {
                                Ok(true)=>{
                                    if runtime.runtime_submit.workspace!=previous_workspace {
                                        while runtime_event_rx.try_recv().is_ok() {}
                                        _runtime_event_follower=crate::daemon::start_runtime_event_follower(
                                            runtime.home.clone(),
                                            app.runtime_event_cursor,
                                            runtime.runtime_submit.workspace.clone(),
                                            runtime_event_tx.clone(),
                                        );
                                    }
                                    continue;
                                },
                                Ok(false)=>{},
                                Err(error)=>{app.append_transcript(format!("Error: {}: {error}",language.text("工作区操作失败","Workspace action failed","ワークスペース操作に失敗しました")));continue;},
                            }
                            if let Some(model_command)=model_commands::parse(&prompt) {
                                match model_command {
                                    ModelCommand::List=>{
                                        let current=session.model.clone().unwrap_or_else(||runtime.provider_config.model.clone());
                                        request_model_list(&mut app,runtime,current);
                                    },
                                    ModelCommand::Switch(model)=>match switch_or_defer_model(&model,&mut app,session,store,runtime,&agent).await {
                                        Ok(message)=>app.append_transcript(format!("System: {message}")),
                                        Err(error)=>app.append_transcript(format!("Error: {}: {error:#}",language.text("切换模型失败","Model switch failed","モデル切替に失敗"))),
                                    },
                                }
                                continue;
                            }
                            if prompt.trim()=="/routing" {
                                let config_path=runtime.runtime_submit.config.clone().map(Ok).unwrap_or_else(crate::config::default_config_path);
                                match config_path.and_then(|path|crate::model_routing::load(&path,runtime.runtime_submit.profile.as_deref())) {
                                    Ok(settings)=>app.open_routing_settings(settings),
                                    Err(error)=>app.append_transcript(format!("Error: {}: {error}",language.text("读取模型路由设置失败","Load model routing settings failed","モデルルーティング設定の読込に失敗"))),
                                }
                                continue;
                            }
                            if prompt.trim()=="/compress" {dispatch_compress(&mut app,session,store,&agent,&runtime.tx)?;continue;}
                            if let Some(parsed)=daemon_commands::parse(&prompt) {
                                match parsed {
                                    Ok(command)=>{
                                        app.append_transcript(format!("System: {} · {}",language.text("Runtime 操作已提交","Runtime action submitted","Runtime 操作を送信しました"),prompt.trim()));
                                        daemon_commands::dispatch(command,runtime.home.clone(),language,runtime.tx.clone());
                                    },
                                    Err(usage)=>app.append_transcript(format!("Error: {usage}")),
                                }
                                continue;
                            }
                            match webapp_commands::handle_webapp_command(&prompt,&runtime.home,&runtime.runtime_submit.workspace,runtime.runtime_submit.config.as_deref(),runtime.runtime_submit.profile.as_deref(),language).await {
                                Ok(Some(message))=>{app.append_transcript(message);continue;},
                                Ok(None)=>{},
                                Err(error)=>{app.append_transcript(format!("Error: {}: {error}",language.text("启动 Web App 失败","Start Web App failed","Web App の起動に失敗しました")));continue;},
                            }
                            if prompt.trim()=="/diff" {
                                match load_diff_review_state(&runtime.home,&session.workspace).await {
                                    Ok(review)=>app.diff_review=Some(review),
                                    Err(error)=>app.append_transcript(format!("Error: {}: {error}",language.text("打开 Diff Review 失败","Open Diff Review failed","Diff Review を開けませんでした"))),
                                }
                                continue;
                            }
                            if prompt.trim()=="/rewind" {
                                match crate::daemon::remote_rewind_points(&runtime.home,session.id).await {
                                    Ok(points) if points.is_empty()=>app.append_transcript(format!("System: {}",language.text("没有可回退的步骤：只有 Runtime 跑完的轮次才能回，最后一步不算","Nothing to rewind to: only completed Runtime turns count, and the last one is where you already are","戻せるステップがありません：Runtime が完了したターンのみ対象で、最後のステップは現在地です"))),
                                    Ok(points)=>app.open_rewind_picker(points),
                                    Err(error)=>app.append_transcript(format!("Error: {}: {error}",language.text("读取可回退步骤失败","Load rewind steps failed","戻せるステップの取得に失敗"))),
                                }
                                continue;
                            }
                            if let PromptExecution::Local(local_prompt)=prompt_execution(&prompt) {
                                if local_prompt.is_empty(){app.append_transcript(format!("System: {}",language.text("用法：/local <任务>","Usage: /local <task>","使用法: /local <タスク>")));continue;}
                                if session.workspace.canonicalize()?!=runtime.local_workspace.canonicalize()?{app.append_transcript(format!("System: {}",language.text("切换工作区后 /local 已禁用；请使用 Runtime，或从目标目录重新启动 TUI","/local is disabled after switching Workspace; use Runtime or restart the TUI from the target directory","ワークスペース切替後は /local を使用できません。Runtime を使うか対象ディレクトリから TUI を再起動してください")));continue;}
                                dispatch_prompt(&mut app,session,store,&runtime.skills,&agent,&runtime.tx,local_prompt)?;
                                continue;
                            }
                            let runtime_alias=prompt.trim()=="/runtime"||prompt.trim().starts_with("/runtime ");
                            if !runtime_alias {
                                let previous_goal=app.goal.clone();
                                let previous_style=app.response_style;
                                if app.handle_slash_command(&prompt,&runtime.skills){
                                    if app.goal!=previous_goal{session.goal=app.goal.clone();}
                                    if app.response_style!=previous_style{session.response_style=app.response_style;}
                                    if app.goal!=previous_goal||app.response_style!=previous_style{store.save(session)?;}
                                    if app.quit_requested{break;}
                                    // /goal <目标> 同时是执行请求。此前只保存开关就返回，
                                    // 界面立刻显示「轮到你」，必须再发一句「继续」才会开工。
                                    if let Some(goal)=runtime_ui::goal_command_to_start(&prompt) {
                                        match runtime_ui::submit_turn(&mut app,session,store,runtime,goal.to_owned()).await {
                                            Ok(())=>app.notice=Some(language.text("AI 正在处理…","AI is working…","AI が処理しています…").to_owned()),
                                            Err(error)=>app.append_transcript(format!("Error: {}: {error}",language.text("提交 Runtime 轮次失败","Submit Runtime turn failed","Runtime ターンの送信に失敗"))),
                                        }
                                    }
                                    continue;
                                }
                            }
                            let PromptExecution::Runtime(remote_prompt)=prompt_execution(&prompt) else {unreachable!("local prompts were handled above")};
                            match runtime_ui::submit_turn(&mut app,session,store,runtime,remote_prompt).await {
                                Ok(())=>app.notice=Some(language.text("AI 正在处理…","AI is working…","AI が処理しています…").to_owned()),
                                Err(error)=>app.append_transcript(format!("Error: {}: {error}",language.text("提交 Runtime 轮次失败","Submit Runtime turn failed","Runtime ターンの送信に失敗"))),
                            }
                        }
                        // 走到这里说明没有任何弹层、焦点在输入框：Esc 就是「停下当前这轮」。
                        // 连续两次按下可避免生成时误触中断。
                        KeyCode::Esc if app.running=>{
                            let now = Instant::now();
                            if interrupt_esc_deadline.is_some_and(|deadline| now <= deadline) {
                                interrupt_esc_deadline = None;
                                match interrupt_turn(&mut app,session,runtime).await {
                                    Ok(message)=>app.notice=Some(message),
                                    Err(error)=>app.notice=Some(format!("{}: {error}",language.text("中断失败","Interrupt failed","中断に失敗"))),
                                }
                            } else {
                                interrupt_esc_deadline = Some(now + Duration::from_millis(800));
                                app.notice=Some(language.text("再按一次 Esc 可中断当前输出","Press Esc again within 800 ms to interrupt the current response","800 ミリ秒以内に Esc をもう一度押すと応答を中断します").to_owned());
                            }
                        },
                        KeyCode::Left=>app.edit_input(|input| input.left()),KeyCode::Right=>app.edit_input(|input| input.right()),
                        KeyCode::Up=>{let width=app.prompt_rect.width.saturating_sub(2).max(1) as usize;app.edit_input(|input| input.up_visual(width));},
                        KeyCode::Down=>{let width=app.prompt_rect.width.saturating_sub(2).max(1) as usize;app.edit_input(|input| input.down_visual(width));},
                        KeyCode::Home=>app.edit_input(|input| input.home()),KeyCode::End=>app.edit_input(|input| input.end()),KeyCode::Backspace=>app.edit_input(|input| input.backspace()),KeyCode::Delete=>app.edit_input(|input| input.delete()),
                        KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL|KeyModifiers::SUPER)=>app.edit_input(|input| input.insert(&c.to_string())),_=>{}
                    }
                }
                _=>{}
            }},
            Some(message)=runtime.rx.recv()=>match message {
                UiMessage::Agent(AgentEvent::ProviderProgress(event))=>match event {
                    willdeep_core::provider::ProviderEvent::RetryStarted{attempt}=>app.record_progress(format!("{} · {attempt}",language.text("正在重试","Retrying","再試行中"))),
                    willdeep_core::provider::ProviderEvent::TextDelta(text)=>app.stream_transient(StreamKind::Reply,&text),
                    willdeep_core::provider::ProviderEvent::ReasoningDelta(text)=>app.stream_transient(StreamKind::Reasoning,&text),
                    willdeep_core::provider::ProviderEvent::Usage(usage)=>{app.context_tokens=usage.input_tokens.unwrap_or(app.context_tokens);app.latest_usage=usage;},
                    willdeep_core::provider::ProviderEvent::RetryWait{attempt,delay}=>app.record_progress(format!("{} · {attempt} · {}s",language.text("等待后重试","Waiting before provider retry","再試行を待機中"),delay.as_secs_f64())),
                },
                UiMessage::Agent(AgentEvent::BackgroundWait { waiting })=>{app.set_background_wait(waiting);},
                UiMessage::Agent(AgentEvent::RunScoped { .. })=>{},
                UiMessage::Agent(AgentEvent::AssistantText(v))=>app.note_narration(&v),
                UiMessage::Agent(AgentEvent::RouteDecided{tier,profile,confidence,auto_dispatched,model,..})=>app.record_progress(format!("{} {} · {}{} · {confidence}%{}",language.text("模型路由","Model route","モデルルート"),tier.as_str(),profile.as_deref().unwrap_or("root"),model.map(|model|format!(" · {model}")).unwrap_or_default(),if auto_dispatched{language.text(" · 已自动下发"," · auto-dispatched"," · 自動ディスパッチ済み")}else{""})),
                UiMessage::Agent(AgentEvent::ModelFallback{purpose,used,reason,..})=>app.record_progress(format!("{} · {purpose} → {} · {reason}",language.text("模型已兜底","Model fallback","モデルをフォールバック"),used.as_deref().unwrap_or("-"))),
                UiMessage::Agent(AgentEvent::TurnStarted{turn})=>{app.note_model_step_started();app.record_progress(format!("{} {turn}",language.text("正在思考 · 准备轮次","Thinking · preparing turn","思考中 · ターンを準備")));},
                UiMessage::Agent(AgentEvent::TurnPreempted{turn})=>app.record_progress(format!("{} {turn}",language.text("已被运行时事件打断 · 轮次","Preempted by a runtime event · turn","ランタイムイベントで中断 · ターン"))),
                UiMessage::Agent(AgentEvent::ToolRequested(v))=>{app.record_progress(format!("{} {}",language.text("正在使用","Using","使用中"),v.name));app.tools.requested(&v.name);app.note_tool_requested(&v.name,crate::tool_detail(&v.name,&v.arguments).as_deref());},
                UiMessage::Agent(AgentEvent::ToolCompleted{call,output,is_error})=>{if !is_error && call.name=="run_command" && let Some(seconds)=willdeep_core::tools::command_wait::notice_wait_seconds(&output){app.note_delayed_command(seconds);}app.record_progress(format!("{} {}",if is_error{language.text("失败","Failed","失敗")}else{language.text("已完成","Finished","完了")},call.name));app.tools.completed(&call.name,is_error);app.note_tool_completed(&call.name,crate::tool_detail(&call.name,&call.arguments).as_deref(),is_error);if matches!(call.name.as_str(),"create_file"|"edit_file"|"run_command"|"create_worktree"){app.workspace_status=workspace_status(&session.workspace,language);app.workspace_attention=workspace_attention(&session.workspace);}},
                UiMessage::Agent(AgentEvent::Usage(v))=>{app.context_tokens=v.input_tokens.unwrap_or(app.context_tokens);app.record_turn_usage(&v);app.latest_usage=v;},
                UiMessage::Agent(AgentEvent::CompressionStarted{estimated_tokens})=>{app.context_tokens=estimated_tokens;app.record_progress(language.text("正在压缩上下文","Compressing context","コンテキストを圧縮中").to_owned());},
                UiMessage::Agent(AgentEvent::CompressionCompleted{estimated_tokens,dropped_messages})=>{app.context_tokens=estimated_tokens;let compressed=language.text("上下文已压缩","Context compressed","コンテキストを圧縮しました");app.record_progress(if dropped_messages>0{language.pick(format!("{compressed} · 本轮请求丢弃 {dropped_messages} 条最旧消息（存档不受影响）"),format!("{compressed} · dropped {dropped_messages} oldest message(s) from this request (the archive is untouched)"),format!("{compressed} · 今回のリクエストから最も古い {dropped_messages} 件を破棄（アーカイブは無変更）"))}else{compressed.to_owned()});},
                UiMessage::Agent(AgentEvent::BackgroundShellStarted{id})=>app.record_progress(format!("{} {id}",language.text("后台命令已启动","Background command started","バックグラウンドコマンド開始"))),
                UiMessage::Agent(AgentEvent::BackgroundShellCompleted{id,status,..})=>app.record_progress(format!("{} {id} · {status:?}",language.text("后台命令已结束","Background command finished","バックグラウンドコマンド完了"))),
                UiMessage::Agent(AgentEvent::SubagentQueued{id,profile,..})=>app.record_progress(format!("{} {} · {profile}",language.text("子 Agent 已排队","Subagent queued","サブエージェント待機中"),id.to_string().get(..8).unwrap_or("agent"))),
                UiMessage::Agent(AgentEvent::SubagentStarted{id,profile,background,..})=>app.record_progress(format!("{} {} · {profile} · {}",language.text("子 Agent 已启动","Subagent started","サブエージェント開始"),id.to_string().get(..8).unwrap_or("agent"),if background{language.text("后台","background","バックグラウンド")}else{language.text("前台","foreground","フォアグラウンド")})),
                UiMessage::Agent(AgentEvent::SubagentCompleted{id,status,..})=>app.record_progress(format!("{} {} · {status:?}",language.text("子 Agent 已结束","Subagent finished","サブエージェント完了"),id.to_string().get(..8).unwrap_or("agent"))),
                UiMessage::Agent(AgentEvent::SubagentTurnStarted{id,turn})=>app.record_progress(format!("{} {} · {} {turn}",language.text("子 Agent","Subagent","サブエージェント"),id.to_string().get(..8).unwrap_or("agent"),language.text("轮次","turn","ターン"))),
                UiMessage::Agent(AgentEvent::SubagentToolRequested{id,name})=>app.record_progress(format!("{} {} · {} {name}",language.text("子 Agent","Subagent","サブエージェント"),id.to_string().get(..8).unwrap_or("agent"),language.text("正在使用","using","使用中"))),
                UiMessage::Agent(AgentEvent::SubagentToolCompleted{id,name,is_error})=>app.record_progress(format!("{} {} · {} {name}",language.text("子 Agent","Subagent","サブエージェント"),id.to_string().get(..8).unwrap_or("agent"),if is_error{language.text("失败","failed","失敗")}else{language.text("已完成","finished","完了")})),
                UiMessage::Agent(AgentEvent::SubagentUsage{..})=>{},
                UiMessage::Agent(AgentEvent::SubagentRetryStarted{id,attempt})=>app.record_progress(format!("{} {} · {} {attempt}",language.text("子 Agent","Subagent","サブエージェント"),&id.to_string()[..8],language.text("正在重试","Retrying","再試行中"))),
                UiMessage::Agent(AgentEvent::SubagentRetryWait{id,attempt,delay})=>app.record_progress(format!("{} {} · {} {attempt} · {}s",language.text("子 Agent","Subagent","サブエージェント"),id.to_string().get(..8).unwrap_or("agent"),language.text("等待重试","Waiting to retry","再試行を待機中"),delay.as_secs_f64())),
                UiMessage::Agent(AgentEvent::SubagentVerdict{id,verifier_passed,attempts,..})=>{if let Some(passed)=verifier_passed{app.record_progress(format!("{} {} · {} · {attempts} {}",language.text("子 Agent","Subagent","サブエージェント"),id.to_string().get(..8).unwrap_or("agent"),if passed{language.text("验证通过","verified","検証通過")}else{language.text("验证未通过","not verified","検証失敗")},language.text("次尝试","attempt(s)","回試行")));}},
                UiMessage::Agent(AgentEvent::GoalContinuationInjected{rung})=>app.record_progress(format!("{} · {rung:?}",language.text("目标未达成 · 继续推进","Goal not met · continuing","目標未達成 · 継続します"))),
                UiMessage::Agent(AgentEvent::GoalBudgetLimited{reason})=>app.record_progress(format!("{} · {reason:?}",language.text("目标预算耗尽 · 转入收尾","Goal budget exhausted · wrapping up","目標の予算を使い切りました · まとめに移ります"))),
                UiMessage::Approval(v,a,s)=>{let detail=v.clone();if app.enqueue_approval((v,a,s)){runtime.notifier.attention_required(RuntimeStatus::WaitingApproval,"tool_approval",detail);execute!(term.backend_mut(),crossterm::style::Print("\x07"))?;}},
                UiMessage::Question(request,sender)=>{let checked=vec![false;request.options.len()];let detail=request.question.clone();if app.enqueue_question(AskDialog{request,selected:0,checked,answer:PromptEditor::default(),sender}){runtime.notifier.attention_required(RuntimeStatus::WaitingAnswer,"ask_user",detail);execute!(term.backend_mut(),crossterm::style::Print("\x07"))?;}},
                UiMessage::Finished(Ok(mut outcome), ownership)=>{qterm.finished(session.id, if outcome.stop_reason.is_complete() { RuntimeStatus::Done } else { RuntimeStatus::Partial });crate::harness::present_partial_outcome(&mut outcome, language);runtime.notifier.task_stopped(&outcome);app.note_reply(&outcome.final_text);if matches!(outcome.stop_reason,willdeep_core::AgentStopReason::GoalComplete){app.note_goal_completed();}app.append_turn_stats(Some(&outcome));store.refresh_execution(session)?;session.messages=outcome.messages;persist_turn_result(session,store)?;drop(ownership);dispatch_retitle(session,&agent,&runtime.tx,false);requeue_undelivered_steering(&mut app,&agent);app.finish_turn();wake_for_kernel_events(&mut app,session,store,&agent,runtime)?;dispatch_input_suggestion(&app,session,&agent,&runtime.tx);},
                UiMessage::Finished(Err(e), ownership)=>{qterm.finished(session.id, RuntimeStatus::Failed);store.refresh_execution(session)?;drop(ownership);app.append_transcript(format!("Error: {e}"));app.finish_turn();},
                UiMessage::Compressed(Ok(messages), ownership)=>{store.refresh_execution(session)?;let changed=session.replace_with_compressed_messages(messages);persist_turn_result(session,store)?;drop(ownership);app.append_transcript(if changed{"System: Context compressed".to_owned()}else{"System: Context is too short to compress".to_owned()});app.finish_turn();},
                UiMessage::Compressed(Err(e), ownership)=>{store.refresh_execution(session)?;drop(ownership);app.append_transcript(format!("Error: context compression failed: {e}"));app.finish_turn();},
                UiMessage::RuntimeNotice(notice)=>app.notice=Some(notice),
                UiMessage::SentAttachments { session_id, items }=>{if session.id==session_id { app.sent_attachments=items;app.open_attachments(attachment_panel::Target::Sent); }},
                UiMessage::MobileRelay(update)=>app.apply_mobile_relay(update),
                UiMessage::RuntimeResult(message)=>{app.append_transcript(message.clone());app.notice=Some(message);},
                UiMessage::AttentionDiagnosticsLoaded { item_id, text } => {
                    if app.attention_diagnostics_loading.as_deref() == Some(item_id.as_str()) {
                        app.attention_diagnostics_loading = None;
                    }
                    if app.attention_detail.as_ref().is_some_and(|item| item.id == item_id) {
                        app.attention_diagnostics = Some(AttentionDiagnostics { item_id, text });
                    }
                },
                UiMessage::ModelsLoaded { request_id, result } => model_provider::receive_models(&mut app, request_id, result),
                UiMessage::LocalProvidersDiscovered(result) => model_provider::receive_local(&mut app, runtime, result),
                UiMessage::MediaLoaded{target,result}=>app.media.finish_load(target,result),
                UiMessage::MediaResized(result)=>{
                    if let Some(error)=app.media.finish_resize(result){
                        app.notice=Some(format!("{}: {error}",language.text("图片协议已降级","Image protocol downgraded","画像プロトコルをフォールバックしました")));
                    }
                },
                // 摘要失败是静默的：列表里还留着 L1 派生的标题，为一行装饰
                // 文字往聊天区塞报错不划算。改成功了才说一句。
                UiMessage::InputSuggested{suggestion,epoch}=>{app.feedback_session=Some(session.id);app.adopt_input_suggestion(suggestion,epoch);},
                UiMessage::Retitled{title,requested}=>{let had_title=title.is_some();if crate::titling::adopt_summarized_title(session,title){store.save(session)?;runtime.notifier.set_session(&session.id.to_string(),Some(session.title.as_str()));app.notice=Some(format!("{}: {}",language.text("会话标题已整理","Session retitled","セッション名を整理しました"),session.title));}else if requested{app.append_transcript(format!("System: {}",if had_title{language.text("标题没有变化","The title is unchanged","タイトルに変更はありません")}else{language.text("标题整理失败：标题模型没有给出可用结果，沿用当前标题","Retitle failed: the title model returned nothing usable; keeping the current title","タイトル整理に失敗しました：タイトルモデルから有効な結果が得られなかったため、現在の名前を維持します")}));}},
            },
            Ok(event)=background_rx.recv()=>{
                let _=runtime.background_tasks.drain_pending();
                app.background_tasks=runtime.background_tasks.snapshots();
                // 报告已经由 await_agents 交给了模型：不再发通知，也不为它唤醒一轮。
                if runtime.background_tasks.is_delivered(&event.snapshot.id) {continue;}
                // 后台结果交给内核，不再自己排一条通知：两条路同时向模型投递
                // 会让同一个结果讲两遍。正文由内核按来源净化后在 turn 边界注入。
                runtime.kernel.publish(
                    willdeep_core::kernel::background_task_event(session.id,&event.snapshot,event.notice),
                    willdeep_core::DedupPolicy::Once,
                );
                willdeep_core::kernel_store::flush(&runtime.kernel,&runtime.kernel_store);
                app.notice=Some(format!("{} finished · queued as a runtime event",event.snapshot.id));
                execute!(term.backend_mut(),crossterm::style::Print("\x07"))?;
                // 忙的时候什么都不做：内核会在当前轮次的边界把它交出去。
                if !app.running {wake_for_kernel_events(&mut app,session,store,&agent,runtime)?;}
            },
        }
    }
    if app.runtime_event_cursor > session.runtime_event_cursor {
        runtime_ui::sync_runtime_session(&mut app, session, store)?;
    }
    app.save_message_times();
    qterm.reporter.finish(false).await;
    Ok(())
}
