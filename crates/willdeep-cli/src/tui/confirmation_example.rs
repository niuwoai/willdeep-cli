use super::*;

const TRIGGER: &str = "#1983#";

/// A local-only preview of the real confirmation UI, never a Runtime request.
pub(super) fn try_submit(app: &mut App, tx: &TuiSender) -> bool {
    if app.input.text().trim() != TRIGGER || app.approval.is_some() || app.question.is_some() {
        return false;
    }
    app.take_submitted_input();
    let (sender, receiver) = oneshot::channel();
    let language = app.language;
    app.approval = Some((
        language.text(
            "确认示例\n这是一条用于预览菜单的示例内容。\n请选择允许或拒绝，只显示选择结果。\n不会执行命令、访问网络或修改文件。",
            "Confirmation example\nThis sample previews the confirmation menu.\nAllow or deny to see the result only.\nNo commands, network requests, or file changes are performed.",
            "確認サンプル\n確認メニューをプレビューするサンプルです。\n許可または拒否を選ぶと結果だけ表示します。\nコマンド実行、通信、ファイル変更は行いません。",
        ).to_owned(),
        false,
        sender,
    ));
    app.approval_selected = 0;
    let tx = tx.clone();
    tokio::spawn(async move {
        let Ok(decision) = receiver.await else { return };
        let result = if decision == ApprovalDecision::AllowOnce {
            language.text(
                "确认示例：已允许",
                "Confirmation example: allowed",
                "確認サンプル：許可",
            )
        } else {
            language.text(
                "确认示例：已拒绝",
                "Confirmation example: denied",
                "確認サンプル：拒否",
            )
        };
        let _ = tx.send(UiMessage::RuntimeNotice(result.to_owned()));
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn example_handles_yes_and_no_locally_without_queuing_or_losing_attachments() {
        for key in ['y', 'n'] {
            let mut app = App::new(Vec::new(), Language::En);
            app.begin_turn(true, "background task".into());
            app.input.insert(TRIGGER);
            app.attachments
                .push(encode_clipboard_image(1, 1, vec![255, 0, 0, 255]).unwrap());
            let (tx, mut rx) = channel();
            assert!(try_submit(&mut app, &tx));
            assert!(app.input.text().is_empty());
            assert_eq!(app.attachments.len(), 1);
            assert!(app.queued_prompts.is_empty());
            assert!(app.transcript.is_empty());
            assert!(app.running && app.runtime_turn);
            app.handle_approval_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
            let Some(UiMessage::RuntimeNotice(result)) = rx.recv().await else {
                panic!("missing local result")
            };
            assert!(result.contains(if key == 'y' { "allowed" } else { "denied" }));
            assert!(app.approval.is_none());
        }
    }

    #[tokio::test]
    async fn example_requires_exact_input_and_never_replaces_a_real_approval() {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        let (tx, _rx) = channel();
        app.input.insert("explain #1983#");
        assert!(!try_submit(&mut app, &tx));
        app.input.take();
        app.input.insert(TRIGGER);
        let (sender, receiver) = oneshot::channel();
        app.approval = Some(("real request".into(), false, sender));
        assert!(!try_submit(&mut app, &tx));
        assert_eq!(app.input.text(), TRIGGER);
        app.handle_approval_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(receiver.await.unwrap(), ApprovalDecision::Deny);
    }
}
