use super::*;
use ratatui::Frame;

#[derive(Default)]
pub(super) struct QueuePanel {
    pub(super) selected: usize,
    pub(super) editor: Option<PromptEditor>,
    pub(super) status: Option<String>,
    scroll: u16,
    hits: Vec<(Rect, KeyCode)>,
    rows: Vec<(Rect, usize)>,
}

impl App {
    pub(super) fn queue_mouse_key(&mut self, mouse: MouseEvent) -> Option<KeyEvent> {
        let panel = self.prompt_queue.as_mut()?;
        match mouse.kind {
            MouseEventKind::ScrollUp => panel.scroll = panel.scroll.saturating_sub(3),
            MouseEventKind::ScrollDown => panel.scroll = panel.scroll.saturating_add(3),
            MouseEventKind::Down(MouseButton::Left) => {
                let point = (mouse.column, mouse.row).into();
                if let Some((_, code)) = panel.hits.iter().find(|(rect, _)| rect.contains(point)) {
                    return Some(KeyEvent::new(*code, KeyModifiers::NONE));
                }
                if let Some((_, selected)) =
                    panel.rows.iter().find(|(rect, _)| rect.contains(point))
                {
                    panel.selected = *selected;
                }
            }
            _ => {}
        }
        None
    }

    pub(super) fn queued_background_prompt_ready(&self) -> bool {
        self.running
            && self.runtime_turn
            && self.waiting_background
            && self.prompt_queue.is_none()
            && self.attachment_panel.is_none()
            && self.approval.is_none()
            && self.question.is_none()
            && self.queued_prompts.front().is_some_and(|item| {
                item.attachments.is_empty() && !item.text.trim_start().starts_with('/')
            })
    }

    pub(super) fn queued_prompt_ready(&self) -> bool {
        !self.running
            && !self.queued_prompts.is_empty()
            && self.prompt_queue.is_none()
            && self.attachment_panel.is_none()
    }

    pub(super) fn queue_submitted_prompt(&mut self) {
        let text = self.take_submitted_input();
        self.append_transcript(self.queued_prompt_row(&text));
        self.queued_prompts.push_back(QueuedPrompt {
            text,
            attachments: std::mem::take(&mut self.attachments),
        });
        self.selected_attachment = 0;
        self.notice = Some(format!(
            "{} · {} {}",
            self.language.text(
                "已排队，模型回复后或本轮结束后发送",
                "Queued · sends after the model replies or this turn finishes",
                "キューに追加 · モデル応答後またはターン終了後に送信"
            ),
            self.queued_prompts.len(),
            self.language.text(
                "条等待 · Ctrl+Q 编辑/删除/插话",
                "waiting · Ctrl+Q edit/delete/steer",
                "件待機 · Ctrl+Q 編集/削除/送達"
            ),
        ));
    }

    /// 排队时保留原始命令；发送前才启用目标，避免改变仍在执行的轮次。
    pub(super) fn queued_prompt_execution(
        &mut self,
        prompt: &str,
        session: &mut Session,
        store: &SessionStore,
    ) -> Result<PromptExecution> {
        let Some(goal) = runtime_ui::goal_command_to_start(prompt) else {
            return Ok(prompt_execution(prompt));
        };
        let previous = session.goal.replace(goal.to_owned());
        if let Err(error) = store.save(session) {
            session.goal = previous;
            return Err(error.into());
        }
        self.goal = session.goal.clone();
        Ok(PromptExecution::Runtime(goal.to_owned()))
    }

    pub(super) fn open_prompt_queue(&mut self) {
        self.prompt_queue = Some(QueuePanel::default());
        self.command_menu_dismissed = true;
    }

    pub(super) fn queue_activity(&self) -> Option<String> {
        let queued = self.queued_prompts.len();
        let delivered = self.deferred_steering.len();
        if queued == 0 && delivered == 0 {
            return None;
        }
        let preview = self
            .queued_prompts
            .front()
            .map(|item| item.text.as_str())
            .or_else(|| self.deferred_steering.first().map(String::as_str))
            .unwrap_or_default();
        let preview = terminal_safe_text(preview)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let delivered_hint = if delivered > 0 {
            format!(
                " · {} {delivered}",
                self.language.text(
                    "已送达待读取",
                    "Delivered, awaiting next step",
                    "送達済み・次のステップ待ち"
                )
            )
        } else {
            String::new()
        };
        Some(format!(
            "{} {queued} · Ctrl+Q /queue{delivered_hint} · {}",
            self.language.text("待发", "Queued", "送信待ち"),
            preview.chars().take(48).collect::<String>()
        ))
    }

    fn queued_transcript_index(&self, selected: usize) -> Option<usize> {
        let item = self.queued_prompts.get(selected)?;
        let ordinal = self
            .queued_prompts
            .iter()
            .take(selected)
            .filter(|p| p.text == item.text)
            .count();
        let row = self.queued_prompt_row(&item.text);
        self.transcript
            .iter()
            .enumerate()
            .filter(|(_, line)| **line == row)
            .nth(ordinal)
            .map(|(index, _)| index)
    }

    pub(super) fn remove_queued_prompt_row(&mut self, selected: usize) {
        if let Some(index) = self.queued_transcript_index(selected) {
            self.transcript.remove(index);
            self.refresh_transcript_height();
        }
    }

    /// 返回 true 时由事件循环尝试插话，只有 Runtime 接收成功才移除消息。
    pub(super) fn handle_queue_key(&mut self, key: KeyEvent) -> bool {
        let Some(mut panel) = self.prompt_queue.take() else {
            return false;
        };
        panel.selected = panel
            .selected
            .min(self.queued_prompts.len().saturating_sub(1));
        if let Some(editor) = panel.editor.as_mut() {
            match key.code {
                KeyCode::Esc => panel.editor = None,
                KeyCode::Enter
                    if key
                        .modifiers
                        .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
                {
                    editor.insert("\n")
                }
                KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    editor.insert("\n")
                }
                KeyCode::Enter if !editor.is_empty() => {
                    let row_index = self.queued_transcript_index(panel.selected);
                    if let Some(item) = self.queued_prompts.get_mut(panel.selected) {
                        item.text = editor.take();
                        let new = self.queued_prompt_row(&self.queued_prompts[panel.selected].text);
                        if let Some(index) = row_index {
                            self.transcript[index] = new;
                        }
                        self.refresh_transcript_height();
                    }
                    panel.editor = None;
                }
                KeyCode::Backspace => editor.backspace(),
                KeyCode::Delete => editor.delete(),
                KeyCode::Left => editor.left(),
                KeyCode::Right => editor.right(),
                KeyCode::Home => editor.home(),
                KeyCode::End => editor.end(),
                KeyCode::PageDown => panel.scroll = panel.scroll.saturating_add(5),
                KeyCode::PageUp => panel.scroll = panel.scroll.saturating_sub(5),
                KeyCode::Up => editor.up_visual(self.transcript_width.max(1)),
                KeyCode::Down => editor.down_visual(self.transcript_width.max(1)),
                KeyCode::Char(character)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    editor.insert(&character.to_string())
                }
                _ => {}
            }
        } else {
            if matches!(
                key.code,
                KeyCode::Up | KeyCode::Down | KeyCode::Enter | KeyCode::Char('e')
            ) {
                panel.scroll = 0;
            }
            match key.code {
                KeyCode::Esc => return false,
                KeyCode::PageDown => panel.scroll = panel.scroll.saturating_add(5),
                KeyCode::PageUp => panel.scroll = panel.scroll.saturating_sub(5),
                KeyCode::Up => panel.selected = panel.selected.saturating_sub(1),
                KeyCode::Down => {
                    panel.selected =
                        (panel.selected + 1).min(self.queued_prompts.len().saturating_sub(1))
                }
                KeyCode::Char('e') | KeyCode::Enter => {
                    if let Some(item) = self.queued_prompts.get(panel.selected) {
                        let mut editor = PromptEditor::default();
                        editor.insert(&item.text);
                        panel.editor = Some(editor);
                    }
                }
                KeyCode::Char('d') | KeyCode::Delete => {
                    self.remove_queued_prompt_row(panel.selected);
                    self.queued_prompts.remove(panel.selected);
                }
                KeyCode::Char('s') if !self.queued_prompts.is_empty() => {
                    self.prompt_queue = Some(panel);
                    return true;
                }
                _ => {}
            }
        }
        panel.selected = panel
            .selected
            .min(self.queued_prompts.len().saturating_sub(1));
        self.prompt_queue = Some(panel);
        false
    }
}

fn wrapped_rows(text: &str, width: usize) -> usize {
    Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .line_count(width.max(1) as u16)
}

pub(super) fn render(f: &mut Frame<'_>, app: &mut App) {
    let Some(panel) = app.prompt_queue.as_ref() else {
        return;
    };
    if app.approval.is_some() || app.question.is_some() || app.mobile_qr.is_some() {
        return;
    }
    let area = f.area();
    let width = area.width.saturating_sub(4).min(100);
    let height = area.height.saturating_sub(4).min(24);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let footer_actions = if panel.editor.is_some() {
        vec![
            (
                KeyCode::Enter,
                app.language.text("Enter 保存", "Enter Save", "Enter 保存"),
            ),
            (
                KeyCode::Esc,
                app.language
                    .text("Esc 取消编辑", "Esc Cancel edit", "Esc 編集取消"),
            ),
        ]
    } else {
        let mut actions = Vec::new();
        if !app.queued_prompts.is_empty() {
            actions.extend([
                (
                    KeyCode::Char('s'),
                    app.language.text("S 发送", "S Send", "S 送信"),
                ),
                (
                    KeyCode::Char('e'),
                    app.language.text("E 编辑", "E Edit", "E 編集"),
                ),
                (
                    KeyCode::Char('a'),
                    app.language.text("A 附件", "A Attachments", "A 添付"),
                ),
                (
                    KeyCode::Delete,
                    app.language.text("D 删除", "D Delete", "D 削除"),
                ),
            ]);
        }
        actions.push((
            KeyCode::Esc,
            app.language.text("Esc 关闭", "Esc Close", "Esc 閉じる"),
        ));
        actions
    };
    let mut footer_hits = Vec::new();
    let mut footer_labels = Vec::new();
    let available = width.saturating_sub(2);
    let (mut x, mut row) = (0u16, 0u16);
    for (code, label) in footer_actions {
        let label = format!("[{label}]");
        let length = (UnicodeWidthStr::width(label.as_str()) as u16).min(available);
        if x > 0 && x + length > available {
            x = 0;
            row += 1;
        }
        footer_labels.push((x, row, length, label));
        footer_hits.push((Rect::new(x, row, length, 1), code));
        x += length + 2;
    }
    let footer_height = (row + 1).min(height.saturating_sub(3));
    let footer_y = rect.bottom().saturating_sub(1 + footer_height);
    let mut list_rows = Vec::new();
    let mut lines = vec![
        app.language
            .text(
                "↑/↓ 或鼠标点击选择消息；底部操作可点击",
                "↑/↓ or click to select; click an action below",
                "↑/↓ またはクリックで選択。下の操作をクリック",
            )
            .to_owned(),
        app.language
            .text(
                "管理期间暂停自动发送；关闭后按顺序发送。",
                "Auto-send paused while managing; resumes in order on close.",
                "管理中は自動送信を停止。閉じると順番に送信。",
            )
            .to_owned(),
    ];
    if let Some(editor) = &panel.editor {
        lines.push(
            app.language
                .text(
                    "编辑：Enter 保存 · Alt+Enter 换行 · Esc 取消（附件保留）",
                    "Edit: Enter saves · Alt+Enter newline · Esc cancels (attachments kept)",
                    "編集：Enter 保存 · Alt+Enter 改行 · Esc 取消（添付を保持）",
                )
                .to_owned(),
        );
        lines.push(terminal_safe_text(editor.text()));
    } else {
        if app.queued_prompts.is_empty() {
            lines.push(
                app.language
                    .text(
                        "没有待发消息",
                        "No queued messages",
                        "送信待ちメッセージなし",
                    )
                    .to_owned(),
            );
        }
        let visible = usize::from(height.saturating_sub(8)).max(1);
        let offset = panel.selected.saturating_sub(visible.saturating_sub(1));
        for (index, item) in app
            .queued_prompts
            .iter()
            .enumerate()
            .skip(offset)
            .take(visible)
        {
            let text = terminal_safe_text(&item.text)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let start = lines
                .iter()
                .map(|line| wrapped_rows(line, available.max(1) as usize))
                .sum::<usize>();
            lines.push(format!(
                "{} {}. [{} {}] {}",
                if index == panel.selected { "▶" } else { " " },
                index + 1,
                item.attachments.len(),
                app.language.text("附件", "attachments", "添付"),
                text.chars()
                    .take(width.saturating_sub(22) as usize)
                    .collect::<String>()
            ));
            let count = wrapped_rows(lines.last().unwrap(), available.max(1) as usize);
            list_rows.push((start, count, index));
        }
        if let Some(item) = app.queued_prompts.get(panel.selected) {
            lines.push(format!(
                "\n{}\n{}",
                app.language.text(
                    "所选消息全文 · Enter 编辑 · A 查看附件",
                    "Selected message · Enter edits · A opens attachments",
                    "選択メッセージ · Enter 編集 · A 添付"
                ),
                terminal_safe_text(&item.text)
            ));
        }
        for text in &app.deferred_steering {
            lines.push(format!(
                "{}: {}",
                app.language.text(
                    "已送达，不可撤回",
                    "Delivered, cannot recall",
                    "送達済み・取消不可"
                ),
                terminal_safe_text(text)
            ));
        }
    }
    if let Some(status) = &panel.status {
        lines.insert(0, terminal_safe_text(status));
        let shift = wrapped_rows(&terminal_safe_text(status), available.max(1) as usize);
        for (start, _, _) in &mut list_rows {
            *start += shift;
        }
    }
    let inner_width = width.saturating_sub(2).max(1) as usize;
    let inner_height = height.saturating_sub(2 + footer_height) as usize;
    let paragraph = Paragraph::new(lines.join("\n")).wrap(Wrap { trim: false });
    let max_scroll = paragraph
        .line_count(inner_width as u16)
        .saturating_sub(inner_height);
    let mut scroll = (panel.scroll as usize).min(max_scroll);
    let cursor = panel.editor.as_ref().map(|editor| {
        let (row, column) = editor.cursor_visual(inner_width);
        let prefix_rows: usize = lines
            .iter()
            .take(lines.len().saturating_sub(1))
            .map(|line| wrapped_rows(line, inner_width))
            .sum();
        (row + prefix_rows, column)
    });
    if let Some((row, _)) = cursor {
        scroll = scroll.min(row).max((row + 1).saturating_sub(inner_height));
    }
    f.render_widget(Clear, rect);
    f.render_widget(
        Block::default()
            .title(format!(
                "{} · {}/{} · PgUp/PgDn",
                app.language.text("待发消息", "Queued messages", "送信待ち"),
                if app.queued_prompts.is_empty() {
                    0
                } else {
                    panel.selected.min(app.queued_prompts.len() - 1) + 1
                },
                app.queued_prompts.len()
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
        rect,
    );
    let content = Rect::new(rect.x + 1, rect.y + 1, available, inner_height as u16);
    f.render_widget(
        paragraph.scroll((scroll.min(u16::MAX as usize) as u16, 0)),
        content,
    );
    for (x, row, length, label) in footer_labels {
        if row < footer_height {
            f.render_widget(
                Paragraph::new(label).style(Style::default().fg(Color::Cyan)),
                Rect::new(rect.x + 1 + x, footer_y + row, length, 1),
            );
        }
    }
    let rows = list_rows
        .into_iter()
        .filter_map(|(start, count, index)| {
            let top = start.max(scroll);
            let bottom = (start + count).min(scroll + inner_height);
            (bottom > top).then(|| {
                (
                    Rect::new(
                        content.x,
                        content.y + (top - scroll) as u16,
                        content.width,
                        (bottom - top) as u16,
                    ),
                    index,
                )
            })
        })
        .collect();
    let panel = app.prompt_queue.as_mut().unwrap();
    panel.rows = rows;
    panel.hits = footer_hits
        .into_iter()
        .filter_map(|(mut hit, code)| {
            if hit.y >= footer_height {
                return None;
            }
            hit.x += rect.x + 1;
            hit.y += footer_y;
            Some((hit, code))
        })
        .collect();
    if let Some((row, column)) = cursor {
        let row = row.saturating_sub(scroll);
        if row < inner_height {
            f.set_cursor_position((
                rect.x + 1 + column.min(width.saturating_sub(3) as usize) as u16,
                rect.y + 1 + row as u16,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint_queue(app: &mut App, width: u16, height: u16) {
        let mut terminal =
            Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
    }

    fn click(x: u16, y: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn click_action(app: &mut App, code: KeyCode) -> KeyEvent {
        let rect = app
            .prompt_queue
            .as_ref()
            .unwrap()
            .hits
            .iter()
            .find(|(_, key)| *key == code)
            .unwrap()
            .0;
        app.queue_mouse_key(click(rect.x, rect.y)).unwrap()
    }

    #[test]
    fn mouse_selects_message_and_footer_dispatches_existing_actions() {
        let mut app = queued_app();
        paint_queue(&mut app, 100, 30);
        let row = app
            .prompt_queue
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .find(|(_, index)| *index == 1)
            .unwrap()
            .0;
        assert!(app.queue_mouse_key(click(row.x, row.y)).is_none());
        assert_eq!(app.prompt_queue.as_ref().unwrap().selected, 1);
        let key = click_action(&mut app, KeyCode::Char('s'));
        assert!(app.handle_queue_key(key));
        assert_eq!(
            app.queued_prompts.len(),
            3,
            "sending must await actual delivery"
        );
        let key = click_action(&mut app, KeyCode::Delete);
        app.handle_queue_key(key);
        assert_eq!(
            app.queued_prompts
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>(),
            ["first", "first"]
        );
        assert!(!app.transcript.iter().any(|line| line.contains("second")));
    }

    #[test]
    fn footer_wraps_with_inert_gaps_and_never_overlaps_scrolled_content() {
        for width in [32, 60, 100] {
            let mut app = queued_app();
            app.prompt_queue.as_mut().unwrap().scroll = 3;
            paint_queue(&mut app, width, 24);
            let panel = app.prompt_queue.as_ref().unwrap();
            assert_eq!(panel.hits.len(), 5);
            let top = panel.hits.iter().map(|(rect, _)| rect.y).min().unwrap();
            assert!(panel.rows.iter().all(|(rect, _)| rect.bottom() <= top));
            for (rect, _) in &panel.hits {
                assert_eq!(rect.height, 1);
                assert!(rect.right() < width && rect.bottom() < 24);
            }
            let first = panel.hits[0].0;
            assert!(app.queue_mouse_key(click(first.right(), first.y)).is_none());
        }
    }

    #[test]
    fn mouse_editor_cancel_keeps_original_and_save_updates_selected_only() {
        let mut app = queued_app();
        paint_queue(&mut app, 60, 24);
        let key = click_action(&mut app, KeyCode::Char('e'));
        app.handle_queue_key(key);
        paint_queue(&mut app, 60, 24);
        assert_eq!(app.prompt_queue.as_ref().unwrap().hits.len(), 2);
        app.prompt_queue
            .as_mut()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .insert(" changed");
        let key = click_action(&mut app, KeyCode::Esc);
        app.handle_queue_key(key);
        assert_eq!(app.queued_prompts[0].text, "first");
        paint_queue(&mut app, 60, 24);
        let key = click_action(&mut app, KeyCode::Char('e'));
        app.handle_queue_key(key);
        app.prompt_queue
            .as_mut()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .insert(" saved");
        paint_queue(&mut app, 60, 24);
        let key = click_action(&mut app, KeyCode::Enter);
        app.handle_queue_key(key);
        assert_eq!(app.queued_prompts[0].text, "first saved");
        assert_eq!(app.queued_prompts[1].text, "second");
        paint_queue(&mut app, 60, 24);
        let key = click_action(&mut app, KeyCode::Esc);
        app.handle_queue_key(key);
        assert!(app.prompt_queue.is_none());
    }

    #[test]
    fn empty_queue_only_offers_close_even_with_delivered_messages() {
        let mut app = queued_app();
        app.queued_prompts.clear();
        app.deferred_steering.push("already delivered".into());
        paint_queue(&mut app, 60, 24);
        let panel = app.prompt_queue.as_ref().unwrap();
        assert_eq!(panel.hits.len(), 1);
        assert_eq!(panel.hits[0].1, KeyCode::Esc);
        assert!(panel.rows.is_empty());
    }

    fn queued_app() -> App {
        let mut app = App::new(Vec::new(), Language::En);
        for text in ["first", "second", "first"] {
            app.append_transcript(app.queued_prompt_row(text));
            app.queued_prompts.push_back(QueuedPrompt {
                text: text.to_owned(),
                attachments: Vec::new(),
            });
        }
        app.open_prompt_queue();
        app
    }

    #[test]
    fn background_wait_automatically_dispatches_only_the_queue_head() {
        let mut app = queued_app();
        app.prompt_queue = None;
        app.begin_turn(true, "running".into());
        assert!(!app.queued_background_prompt_ready());
        app.waiting_background = true;
        assert!(app.queued_background_prompt_ready());
        app.open_prompt_queue();
        assert!(
            !app.queued_background_prompt_ready(),
            "editing pauses auto-send"
        );
        app.prompt_queue = None;
        app.queued_prompts.front_mut().unwrap().text = "/goal new goal".into();
        assert!(
            !app.queued_background_prompt_ready(),
            "commands cannot bypass turn ownership"
        );
        app.queued_prompts.front_mut().unwrap().text = "first".into();
        app.queued_prompts
            .front_mut()
            .unwrap()
            .attachments
            .push(encode_clipboard_image(1, 1, vec![255, 0, 0, 255]).unwrap());
        assert!(
            !app.queued_background_prompt_ready(),
            "attachments must stay with their message"
        );
        app.queued_prompts.front_mut().unwrap().attachments.clear();
        app.note_steering_delivered("first");
        assert!(
            !app.queued_background_prompt_ready(),
            "wait for the next background boundary before sending another message"
        );
        assert!(
            app.running,
            "sending a message must not complete or cancel background work"
        );
    }

    #[test]
    fn runtime_completion_dispatches_user_queue_despite_pending_kernel_events() {
        use willdeep_runtime_protocol::kernel_event::{
            EventPriority, EventSource, InterruptPolicy,
        };
        let home = std::env::temp_dir().join(format!("queued-runtime-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let store = SessionStore::new(&home);
        let mut session = Session::new(home.clone(), None, "queue test");
        let kernel = willdeep_core::EventKernel::new();
        kernel.publish(
            willdeep_core::host_event(
                session.id,
                EventSource::Task,
                "task.completed",
                EventPriority::Normal,
                InterruptPolicy::YieldAtBoundary,
                "background result",
                None,
                None,
                false,
            ),
            willdeep_core::DedupPolicy::Once,
        );
        let mut app = queued_app();
        app.prompt_queue = None;
        app.begin_turn(true, "running".into());
        assert!(!app.queued_prompt_ready());
        runtime_ui::apply_runtime_events(
            &mut app,
            vec![crate::daemon::RemoteRuntimeEvent {
                sequence: 1,
                kind: "turn.completed".into(),
                message: format!("session_id={} turn_id={}", session.id, uuid::Uuid::new_v4()),
                visible: true,
                session_id: Some(session.id),
            }],
            &mut session,
            &store,
        )
        .unwrap();
        assert!(kernel.pending_wake_authority(session.id).is_some());
        assert!(
            app.queued_prompt_ready(),
            "pending host events must not starve an already submitted user prompt"
        );
        assert_eq!(app.queued_prompts.pop_front().unwrap().text, "first");
        assert_eq!(app.queued_prompts.front().unwrap().text, "second");
        assert!(
            kernel.pending_wake_authority(session.id).is_some(),
            "user dispatch does not discard background events"
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn queued_messages_keep_idle_composer_out_of_your_turn_state() {
        let mut app = queued_app();
        app.finish_turn();
        assert_eq!(app.composer_state().1, Color::Yellow);
        assert!(app.composer_state().0.contains("Queued messages"));
        assert!(
            !app.bell_pending,
            "the user should not be signaled to send again"
        );
        assert!(
            !app.queued_prompt_ready(),
            "editing the queue pauses dispatch"
        );
        app.prompt_queue = None;
        assert!(app.queued_prompt_ready());
        app.open_attachments(attachment_panel::Target::Queued(0));
        assert!(
            !app.queued_prompt_ready(),
            "attachment editing retains the pending message"
        );
        app.attachment_panel = None;
        app.queued_prompts.clear();
        assert_eq!(app.composer_state(), ("Your turn", Color::Green));
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn goal_enter_queues_without_changing_the_running_turn_or_the_next_draft() {
        let mut app = App::new(vec!["You: hi".to_owned()], Language::ZhCn);
        app.goal = Some("当前目标".to_owned());
        app.begin_turn(true, "working".to_owned());
        let request = "/goal 请读一下 DESIGN.md 并按新设计要求更新一版";
        app.input.insert(request);
        app.attachments
            .push(encode_clipboard_image(1, 1, vec![255, 0, 0, 255]).unwrap());
        assert!(!app.handle_command_key(key(KeyCode::Enter)));
        assert_eq!(busy_input(app.input.text()), BusyInput::Queue);

        app.queue_submitted_prompt();
        assert!(app.running);
        assert_eq!(app.goal.as_deref(), Some("当前目标"));
        assert!(app.input.is_empty());
        assert!(app.attachments.is_empty());
        assert_eq!(app.queued_prompts.len(), 1);
        assert_eq!(app.queued_prompts[0].text, request);
        assert_eq!(app.queued_prompts[0].attachments.len(), 1);
        assert!(app.queue_activity().unwrap().contains("待发 1"));

        app.input.insert("下一条尚未发送的草稿");
        app.note_reply("hi 的回复里有 kubectl get pods");
        app.finish_turn();
        assert_eq!(app.input.text(), "下一条尚未发送的草稿");
        assert_eq!(app.queued_prompts[0].text, request);
        app.open_prompt_queue();
        app.handle_queue_key(key(KeyCode::Enter));
        assert_eq!(
            app.prompt_queue
                .as_ref()
                .unwrap()
                .editor
                .as_ref()
                .unwrap()
                .text(),
            request
        );
    }

    #[test]
    fn queued_goal_is_persisted_and_stripped_only_when_dispatching() {
        let root =
            std::env::temp_dir().join(format!("willdeep-queued-goal-{}", uuid::Uuid::new_v4()));
        let store = SessionStore::new(&root);
        let mut session = Session::new(root.clone(), None, "queued goal");
        session.goal = Some("old goal".to_owned());
        store.save(&mut session).unwrap();
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.goal = session.goal.clone();
        app.input.insert("未发送草稿");

        assert_eq!(
            app.queued_prompt_execution("/goal\t读 DESIGN.md\n更新一版", &mut session, &store)
                .unwrap(),
            PromptExecution::Runtime("读 DESIGN.md\n更新一版".to_owned())
        );
        assert_eq!(store.load(session.id).unwrap().goal, session.goal);
        assert_eq!(app.goal, session.goal);
        assert!(
            app.enrich_prompt("读 DESIGN.md\n更新一版", &SkillCatalog::default())
                .contains("<goal>\n读 DESIGN.md\n更新一版\n</goal>")
        );
        assert_eq!(app.input.text(), "未发送草稿");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_goal_persistence_preserves_the_previous_goal_and_queued_message() {
        let root = std::env::temp_dir().join(format!(
            "willdeep-queued-goal-failure-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&root, b"not a directory").unwrap();
        let store = SessionStore::new(&root);
        let mut session = Session::new(root.clone(), None, "queued goal failure");
        session.goal = Some("old goal".to_owned());
        let mut app = App::new(Vec::new(), Language::En);
        app.goal = session.goal.clone();
        app.input.insert("/goal new goal");
        app.queue_submitted_prompt();
        assert!(
            app.queued_prompt_execution("/goal new goal", &mut session, &store)
                .is_err()
        );
        assert_eq!(session.goal.as_deref(), Some("old goal"));
        assert_eq!(app.goal, session.goal);
        assert_eq!(app.queued_prompts[0].text, "/goal new goal");
        std::fs::remove_file(root).unwrap();
    }

    #[test]
    fn queued_input_invalidates_late_suggestions_without_appending_them() {
        let mut app = App::new(Vec::new(), Language::En);
        let epoch = app.input_suggestion_epoch;
        assert!(app.adopt_input_suggestion(Some("kubectl get pods".to_owned()), epoch));
        app.input.insert("/goal read DESIGN.md");
        app.queue_submitted_prompt();
        assert!(app.input_suggestion.is_none());
        assert!(!app.adopt_input_suggestion(Some("kubectl get pods".to_owned()), epoch));
        let current_epoch = app.input_suggestion_epoch;
        assert!(!app.adopt_input_suggestion(Some("kubectl get pods".to_owned()), current_epoch));
        assert!(!app.accept_input_suggestion());
        assert!(app.input.is_empty());
        assert_eq!(app.queued_prompts[0].text, "/goal read DESIGN.md");
    }

    #[test]
    fn goal_queue_is_visible_in_the_actual_layout_while_hi_streams() {
        use ratatui::backend::TestBackend;
        let mut app = App::new(vec!["You: hi".to_owned()], Language::ZhCn);
        app.begin_turn(true, "working".to_owned());
        app.input.insert("/goal 请读一下 DESIGN.md 并更新一版");
        app.queue_submitted_prompt();
        app.stream_transient(StreamKind::Reply, "hi 的回复仍在生成");
        let mut terminal = Terminal::new(TestBackend::new(100, 32)).unwrap();
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        let compact: String = screen
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        assert!(compact.contains("待发1"), "{screen}");
        assert!(screen.contains("DESIGN.md"), "{screen}");
        assert!(compact.contains("hi的回复仍在生成"), "{screen}");
        assert!(screen.contains("Ctrl+Q"), "{screen}");
    }

    #[test]
    fn delete_keeps_remaining_order_and_draft() {
        let mut app = queued_app();
        app.input.insert("draft");
        app.handle_queue_key(key(KeyCode::Down));
        app.handle_queue_key(key(KeyCode::Delete));
        assert_eq!(
            app.queued_prompts
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>(),
            ["first", "first"]
        );
        assert_eq!(app.input.text(), "draft");
        assert!(!app.transcript.iter().any(|row| row.contains("second")));
    }

    #[test]
    fn edit_save_and_cancel_do_not_send() {
        let mut app = queued_app();
        app.handle_queue_key(key(KeyCode::Enter));
        app.prompt_queue
            .as_mut()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .insert(" edited");
        app.handle_queue_key(key(KeyCode::Enter));
        assert_eq!(app.queued_prompts[0].text, "first edited");
        app.handle_queue_key(key(KeyCode::Enter));
        app.prompt_queue
            .as_mut()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .insert(" discarded");
        app.handle_queue_key(key(KeyCode::Esc));
        assert_eq!(app.queued_prompts[0].text, "first edited");
        assert!(app.prompt_queue.is_some());
        app.handle_queue_key(key(KeyCode::Esc));
        assert!(app.prompt_queue.is_none());
    }

    #[test]
    fn activity_remains_visible_for_queued_and_delivered_messages() {
        let mut app = queued_app();
        app.notice = None;
        assert!(app.queue_activity().unwrap().contains("Queued 3"));
        app.queued_prompts.clear();
        app.deferred_steering.push("steered".to_owned());
        assert!(
            app.queue_activity()
                .unwrap()
                .contains("awaiting next step 1")
        );
        assert!(matches!(busy_input("/queue"), BusyInput::RunNow));
    }

    #[test]
    fn duplicate_text_edit_targets_the_selected_occurrence() {
        let mut app = queued_app();
        app.handle_queue_key(key(KeyCode::Down));
        app.handle_queue_key(key(KeyCode::Down));
        app.handle_queue_key(key(KeyCode::Enter));
        app.prompt_queue
            .as_mut()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .insert(" third");
        app.handle_queue_key(key(KeyCode::Enter));
        assert_eq!(app.queued_prompts[0].text, "first");
        assert_eq!(app.queued_prompts[2].text, "first third");
        assert_eq!(app.transcript[0], "You: [queued] first");
        assert_eq!(app.transcript[2], "You: [queued] first third");
    }

    #[test]
    fn editing_keeps_attachments_and_a_steer_request_keeps_the_message_until_acknowledged() {
        let mut app = queued_app();
        app.queued_prompts[0]
            .attachments
            .push(encode_clipboard_image(1, 1, vec![255, 0, 0, 255]).unwrap());
        app.handle_queue_key(key(KeyCode::Enter));
        app.prompt_queue
            .as_mut()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .insert(" modified");
        app.handle_queue_key(key(KeyCode::Enter));
        assert_eq!(app.queued_prompts[0].attachments.len(), 1);
        assert!(app.handle_queue_key(key(KeyCode::Char('s'))));
        assert_eq!(app.queued_prompts.len(), 3);
        assert_eq!(app.queued_prompts[0].text, "first modified");
    }

    #[test]
    fn long_edit_scrolls_to_the_cursor() {
        use ratatui::backend::TestBackend;
        let mut app = queued_app();
        app.handle_queue_key(key(KeyCode::Enter));
        app.prompt_queue
            .as_mut()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .insert(&format!("{}\nvisible tail", "\nline".repeat(40)));
        let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("visible tail"), "{screen}");
    }

    #[test]
    fn real_layout_keeps_queue_visible_during_stream_and_lists_messages() {
        use ratatui::backend::TestBackend;
        for language in [Language::En, Language::ZhCn, Language::Ja] {
            for (width, height) in [(100, 32), (60, 24)] {
                let mut app = queued_app();
                app.language = language;
                app.prompt_queue = None;
                app.running = true;
                app.model_step_in_flight = true;
                app.stream_transient(StreamKind::Reply, "Streaming reply");
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
                let screen: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(screen.contains("Ctrl+Q"), "{screen}");
                assert!(screen.contains("first"), "{screen}");
                assert!(screen.contains("Streaming reply"), "{screen}");
                app.open_prompt_queue();
                draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
                let screen: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(screen.contains("1/3"), "{screen}");
                assert!(screen.contains("second"), "{screen}");
                assert!(screen.contains("Esc"), "{screen}");
            }
        }
    }
}
