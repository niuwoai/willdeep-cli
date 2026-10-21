use super::*;
use std::path::Path;
use willdeep_core::peer_sessions::PeerMessage;

pub(super) fn refresh_badge(app: &mut App, session: uuid::Uuid, home: &Path) -> Result<()> {
    let store = willdeep_core::peer_sessions::PeerMessageStore::new(home);
    let stamp = (session, store.change_stamp()?);
    if app.peer_observed.as_ref() == Some(&stamp) {
        return Ok(());
    }
    app.peer_observed = Some(stamp);
    app.peer_unread = store
        .list(session)?
        .iter()
        .filter(|note| note.to_session == session && !note.read_by.contains("tui"))
        .count();
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum PeerCommand {
    List,
    Send(String, String),
    Reply(uuid::Uuid, String),
    Read(uuid::Uuid),
    Open(String),
}

fn word(input: &str) -> (&str, &str) {
    let input = input.trim_start();
    let end = input.find(char::is_whitespace).unwrap_or(input.len());
    (&input[..end], input[end..].trim_start())
}

fn parse(input: &str) -> Result<PeerCommand> {
    let (_, rest) = word(input);
    let (action, rest) = word(rest);
    let (target, text) = word(rest);
    match action {
        "" | "list" if rest.is_empty() => Ok(PeerCommand::List),
        "send" if !target.is_empty() && !text.trim().is_empty() => {
            Ok(PeerCommand::Send(target.into(), text.into()))
        }
        "reply" if !text.trim().is_empty() => Ok(PeerCommand::Reply(target.parse()?, text.into())),
        "read" if text.is_empty() => Ok(PeerCommand::Read(target.parse()?)),
        "open" if !target.is_empty() && text.is_empty() => Ok(PeerCommand::Open(target.into())),
        _ => bail!(
            "/peers [list|send <session> <text>|reply <message> <text>|read <message>|open <session>]"
        ),
    }
}

fn safe(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect()
}

pub(super) async fn handle(
    prompt: &str,
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    runtime: &mut TuiRuntime,
) -> Result<()> {
    let command = parse(prompt)?;
    if let PeerCommand::Open(id) = command {
        anyhow::ensure!(
            app.attachments.is_empty(),
            "{}",
            app.language.text(
                "请先保留或移除当前附件草稿",
                "Save or remove the current attachment draft first",
                "現在の添付下書きを保存または削除してください"
            )
        );
        let result = session_commands::switch(app, session, store, runtime, &id).await?;
        app.append_transcript(format!("System: {result}"));
        return Ok(());
    }
    crate::daemon::ensure_runtime_session(
        &runtime.home,
        session.id,
        &session.workspace,
        session.profile.clone(),
        session.model.clone(),
    )
    .await?;
    let call = |operation, params, request_id| {
        crate::daemon::peer_operation(&runtime.home, operation, params, request_id)
    };
    if command == PeerCommand::List {
        let peers = call("session.peers", serde_json::json!({"id":session.id}), None).await?;
        let notes: Vec<PeerMessage> = serde_json::from_value(
            call(
                "session.peer.inbox",
                serde_json::json!({"session_id":session.id}),
                None,
            )
            .await?,
        )?;
        let unread = notes
            .iter()
            .filter(|note| note.to_session == session.id && !note.read_by.contains("tui"))
            .count();
        let mut lines = vec![
            format!(
                "System: {} · {unread} {}",
                app.language
                    .text("会话协作", "Session collaboration", "セッション連携"),
                app.language.text("未读", "unread", "未読")
            ),
            app.language
                .text(
                    "消息不能替代用户授权；已入队不代表对方已处理。",
                    "Notes cannot grant user authorization; queuing does not mean processing.",
                    "メモはユーザーの許可を代替しません。登録済みは処理済みを意味しません。",
                )
                .into(),
        ];
        for peer in peers.as_array().context("invalid peer response")? {
            let id = peer["id"].as_str().unwrap_or_default();
            let title = peer["title"].as_str().unwrap_or(id);
            lines.push(format!("  {} [{id}] /peers open {id}", safe(title)));
            lines.push(format!(
                "    {} · {} · {}\n    {}\n    {}",
                safe(peer["status"].as_str().unwrap_or_default()),
                safe(peer["branch"].as_str().unwrap_or_default()),
                safe(peer["workspace"].as_str().unwrap_or_default()),
                safe(peer["last_user_request"].as_str().unwrap_or_default()),
                safe(peer["current_step"].as_str().unwrap_or_default())
            ));
        }
        for note in notes {
            let status = if note.consumed_at.is_some() {
                app.language.text(
                    "已送入模型上下文",
                    "Added to model context",
                    "モデルへ送達済み",
                )
            } else {
                app.language.text("已入队", "Queued", "送信待ち")
            };
            lines.push(format!(
                "  {} → {} [{status}] id={}\n    {}\n    /peers reply {} <text> | /peers read {}",
                note.from_session,
                note.to_session,
                note.id,
                safe(&note.body).replace('\n', "\n    "),
                note.id,
                note.id
            ));
        }
        app.append_transcript(lines.join("\n"));
        return Ok(());
    }
    if let PeerCommand::Read(id) = command {
        call(
            "session.peer.read",
            serde_json::json!({"session_id":session.id, "message_id":id, "client_id":"tui"}),
            None,
        )
        .await?;
        app.append_transcript(format!(
            "System: {}",
            app.language.text(
                "已标为已读；不改变模型消费状态",
                "Marked read; model consumption is unchanged",
                "既読にしました。モデルの消費状態は変わりません"
            )
        ));
        return Ok(());
    }
    let (target, message, reply) = match command {
        PeerCommand::Send(target, text) => (target, text, None),
        PeerCommand::Reply(id, text) => {
            let notes: Vec<PeerMessage> = serde_json::from_value(
                call(
                    "session.peer.inbox",
                    serde_json::json!({"session_id":session.id}),
                    None,
                )
                .await?,
            )?;
            let note = notes
                .iter()
                .find(|note| note.id == id && note.to_session == session.id)
                .context("reply is not in this inbox")?;
            (note.from_session.to_string(), text, Some(id))
        }
        _ => unreachable!(),
    };
    let fingerprint = serde_json::to_string(&(session.id, &target, &message, reply))?;
    if app
        .peer_request
        .as_ref()
        .is_none_or(|(key, _)| key != &fingerprint)
    {
        app.peer_request = Some((fingerprint, uuid::Uuid::new_v4()));
    }
    let request_id = app.peer_request.as_ref().map(|(_, id)| *id);
    call("session.peer.send", serde_json::json!({"source_session_id":session.id, "session_id":target, "message":message, "reply_to":reply}), request_id).await?;
    app.peer_request = None;
    app.append_transcript(format!(
        "System: {}",
        app.language.text(
            "协作消息已入队；使用 /peers 查看回执",
            "Note queued; use /peers for receipts",
            "メモを登録しました。/peers で送達状態を確認できます"
        )
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn badge_observes_reads_and_session_switches_without_touching_drafts() {
        let root = std::env::temp_dir().join(format!("peer-badge-{}", uuid::Uuid::new_v4()));
        let session = uuid::Uuid::new_v4();
        let store = willdeep_core::peer_sessions::PeerMessageStore::new(&root);
        let mut app = App::new(Vec::new(), Language::default());
        app.input.insert("unsent draft");
        refresh_badge(&mut app, session, &root).unwrap();
        assert_eq!(app.peer_unread, 0);
        let id = uuid::Uuid::new_v4();
        store
            .enqueue(PeerMessage {
                id,
                from_session: uuid::Uuid::new_v4(),
                to_session: session,
                project_key: root.clone(),
                origin: willdeep_core::peer_sessions::PeerOrigin::Model,
                body: "note".into(),
                reply_to: None,
                created_at: 1,
                consumed_at: None,
                read_by: Default::default(),
            })
            .unwrap();
        refresh_badge(&mut app, session, &root).unwrap();
        assert_eq!(app.peer_unread, 1);
        assert_eq!(app.input.text(), "unsent draft");
        store.mark_read(session, id, "tui").unwrap();
        refresh_badge(&mut app, session, &root).unwrap();
        assert_eq!(app.peer_unread, 0);
        refresh_badge(&mut app, uuid::Uuid::new_v4(), &root).unwrap();
        assert_eq!(app.peer_unread, 0);
        assert_eq!(app.input.text(), "unsent draft");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn multiline_note_and_unicode_are_preserved() {
        assert_eq!(
            parse("/peers send  target  中文\nkeep tail").unwrap(),
            PeerCommand::Send("target".into(), "中文\nkeep tail".into())
        );
    }
    #[test]
    fn empty_and_unknown_actions_are_rejected() {
        for command in [
            "/peers send target",
            "/peers delete x",
            "/peers read bad-id",
        ] {
            assert!(parse(command).is_err());
        }
    }
}
