use super::*;
use ratatui::Frame;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    Draft,
    Queued(usize),
    Sent,
}
pub(super) struct AttachmentPanel {
    target: Target,
    selected: usize,
    editor: Option<PromptEditor>,
    preview: bool,
    scroll: u16,
    confirm_clear: bool,
}
impl App {
    pub(super) fn append_draft_attachment(&mut self, item: DraftAttachment) -> bool {
        const MAX_COUNT: usize = 12;
        const MAX_BYTES: usize = 10 * 1024 * 1024;
        let size = self
            .attachments
            .iter()
            .map(|item| &item.message)
            .chain(std::iter::once(&item.message))
            .try_fold(2usize, |total, message| {
                serde_json::to_vec(message).map(|bytes| total.saturating_add(bytes.len() + 1))
            });
        if self.attachments.len() >= MAX_COUNT
            || size.as_ref().is_ok_and(|bytes| *bytes > MAX_BYTES)
        {
            self.notice = Some(
                self.language
                    .text(
                        "附件超过限制（最多 12 个、合计 10 MiB），请先删减",
                        "Attachment limit exceeded (12 files, 10 MiB total); remove some first",
                        "添付の上限（12 件・合計 10 MiB）を超えています。先に削減してください",
                    )
                    .to_owned(),
            );
            return false;
        }
        if let Err(error) = size {
            self.notice = Some(format!("Attachment serialization failed: {error}"));
            return false;
        }
        self.attachments.push(item);
        self.selected_attachment = self.attachments.len() - 1;
        true
    }
    pub(super) fn open_attachments(&mut self, target: Target) {
        self.attachment_panel = Some(AttachmentPanel {
            target,
            selected: 0,
            editor: None,
            preview: false,
            scroll: 0,
            confirm_clear: false,
        });
    }
    fn attachment_items(&self, target: Target) -> &[DraftAttachment] {
        match target {
            Target::Draft => &self.attachments,
            Target::Sent => &self.sent_attachments,
            Target::Queued(index) => self
                .queued_prompts
                .get(index)
                .map(|item| item.attachments.as_slice())
                .unwrap_or_default(),
        }
    }
    fn attachment_items_mut(&mut self, target: Target) -> Option<&mut Vec<DraftAttachment>> {
        match target {
            Target::Draft => Some(&mut self.attachments),
            Target::Sent => None,
            Target::Queued(index) => self
                .queued_prompts
                .get_mut(index)
                .map(|item| &mut item.attachments),
        }
    }
    pub(super) fn attachment_paste(&mut self, text: &str) {
        if let Some(editor) = self
            .attachment_panel
            .as_mut()
            .and_then(|panel| panel.editor.as_mut())
        {
            editor.insert(text);
        }
    }
    pub(super) fn handle_attachment_key(&mut self, key: KeyEvent) -> MediaAction {
        let Some(mut panel) = self.attachment_panel.take() else {
            return MediaAction::None;
        };
        let mut action = MediaAction::None;
        panel.selected = panel
            .selected
            .min(self.attachment_items(panel.target).len().saturating_sub(1));
        if panel.target == Target::Sent {
            if key.code == KeyCode::Char('r')
                && let Some(item) = self.sent_attachments.get(panel.selected).cloned()
            {
                let mut copy = item;
                copy.id = uuid::Uuid::new_v4();
                if self.append_draft_attachment(copy) {
                    self.notice = Some(
                        self.language
                            .text(
                                "附件已引用到草稿",
                                "Attachment copied to draft",
                                "添付を下書きに引用しました",
                            )
                            .to_owned(),
                    );
                }
            }
            if matches!(
                key.code,
                KeyCode::Char('e')
                    | KeyCode::Char('d')
                    | KeyCode::Delete
                    | KeyCode::Char('c')
                    | KeyCode::Char('k')
                    | KeyCode::Char('j')
            ) {
                self.attachment_panel = Some(panel);
                return action;
            }
        }
        if panel.confirm_clear {
            if key.code == KeyCode::Char('y')
                && let Some(items) = self.attachment_items_mut(panel.target)
            {
                items.clear();
            }
            panel.confirm_clear = false;
        } else if let Some(editor) = panel.editor.as_mut() {
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
                KeyCode::Enter => {
                    if let Some(item) = self
                        .attachment_items_mut(panel.target)
                        .and_then(|items| items.get_mut(panel.selected))
                        && let MessageAttachment::Text { content, .. } = &mut item.message
                    {
                        *content = editor.take();
                    }
                    panel.editor = None;
                }
                KeyCode::Backspace => editor.backspace(),
                KeyCode::Delete => editor.delete(),
                KeyCode::Left => editor.left(),
                KeyCode::Right => editor.right(),
                KeyCode::Home => editor.home(),
                KeyCode::End => editor.end(),
                KeyCode::Up => editor.up_visual(self.transcript_width.max(1)),
                KeyCode::Down => editor.down_visual(self.transcript_width.max(1)),
                KeyCode::Char(value)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    editor.insert(&value.to_string())
                }
                _ => {}
            }
        } else if panel.preview {
            match key.code {
                KeyCode::Esc => panel.preview = false,
                KeyCode::Down => panel.scroll = panel.scroll.saturating_add(1),
                KeyCode::Up => panel.scroll = panel.scroll.saturating_sub(1),
                KeyCode::PageDown => panel.scroll = panel.scroll.saturating_add(10),
                KeyCode::PageUp => panel.scroll = panel.scroll.saturating_sub(10),
                _ => {}
            }
        } else {
            match key.code {
                KeyCode::Esc => return action,
                KeyCode::Up => panel.selected = panel.selected.saturating_sub(1),
                KeyCode::Down => {
                    panel.selected = (panel.selected + 1)
                        .min(self.attachment_items(panel.target).len().saturating_sub(1))
                }
                KeyCode::Home => panel.selected = 0,
                KeyCode::End => {
                    panel.selected = self.attachment_items(panel.target).len().saturating_sub(1)
                }
                KeyCode::Delete | KeyCode::Char('d') => {
                    if let Some(items) = self.attachment_items_mut(panel.target)
                        && panel.selected < items.len()
                    {
                        items.remove(panel.selected);
                    }
                }
                KeyCode::Char('c') => panel.confirm_clear = true,
                KeyCode::Char('k') | KeyCode::Char('j') => {
                    let delta: isize = if key.code == KeyCode::Char('k') {
                        -1
                    } else {
                        1
                    };
                    if let Some(items) = self.attachment_items_mut(panel.target)
                        && let Some(next) = panel.selected.checked_add_signed(delta)
                        && next < items.len()
                    {
                        items.swap(panel.selected, next);
                        panel.selected = next;
                    }
                }
                KeyCode::Char('e') => {
                    if let Some(item) = self.attachment_items(panel.target).get(panel.selected)
                        && let MessageAttachment::Text { content, .. } = &item.message
                    {
                        let mut editor = PromptEditor::default();
                        editor.insert(content);
                        panel.editor = Some(editor);
                    }
                }
                KeyCode::Enter => {
                    if let Some(item) = self.attachment_items(panel.target).get(panel.selected) {
                        match &item.message {
                            MessageAttachment::Text { .. } => {
                                panel.preview = true;
                                panel.scroll = 0;
                            }
                            MessageAttachment::Image {
                                name,
                                media_type,
                                data,
                                ..
                            } => {
                                let label = name.clone();
                                let target = format!("data:{media_type};base64,{data}");
                                action = self.media.open_attachment(label, target);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        self.selected_attachment = self
            .selected_attachment
            .min(self.attachments.len().saturating_sub(1));
        self.attachment_panel = Some(panel);
        action
    }
}

pub(super) fn render(frame: &mut Frame<'_>, app: &App) {
    let Some(panel) = &app.attachment_panel else {
        return;
    };
    let popup = centered_rect(
        modal_width(frame.area()),
        frame.area().height.saturating_sub(4).max(1),
        frame.area(),
    );
    let items = app.attachment_items(panel.target);
    let title = match panel.target {
        Target::Draft => app
            .language
            .text("草稿附件", "Draft attachments", "下書きの添付"),
        Target::Queued(_) => app.language.text(
            "待发消息附件",
            "Queued message attachments",
            "送信待ちの添付",
        ),
        Target::Sent => app
            .language
            .text("已发送附件", "Sent attachments", "送信済みの添付"),
    };
    let hint = if panel.target == Target::Sent && !panel.preview {
        app.language.text("已发送附件只读 · ↑/↓ 选择 · Enter 预览 · r 引用到草稿 · Esc 返回", "Sent attachments are read-only · ↑/↓ select · Enter preview · r copy to draft · Esc back", "送信済み・読み取り専用 · ↑/↓ 選択 · Enter 表示 · r 下書きに引用 · Esc 戻る")
    } else if panel.confirm_clear {
        app.language.text(
            "清空附件？y 确认，其它键取消",
            "Clear attachments? y confirms, any other key cancels",
            "全添付を削除？y で確認、他のキーで取消",
        )
    } else if panel.editor.is_some() {
        app.language.text(
            "Enter 保存 · Alt+Enter 换行 · Esc 取消",
            "Enter save · Alt+Enter newline · Esc cancel",
            "Enter 保存 · Alt+Enter 改行 · Esc 取消",
        )
    } else if panel.preview {
        app.language.text(
            "↑/↓ 翻阅 · Esc 返回",
            "↑/↓ scroll · Esc back",
            "↑/↓ スクロール · Esc 戻る",
        )
    } else {
        app.language.text(
            "↑/↓ 选择 · Enter 预览 · e 编辑 · d 删除 · k/j 移动 · c 清空 · Esc 返回",
            "↑/↓ select · Enter preview · e edit · d remove · k/j move · c clear · Esc back",
            "↑/↓ 選択 · Enter 表示 · e 編集 · d 削除 · k/j 移動 · c 全削除 · Esc 戻る",
        )
    };
    let lines = if let Some(editor) = &panel.editor {
        editor
            .text()
            .lines()
            .map(|text| Line::raw(terminal_safe_text(text)))
            .collect::<Vec<_>>()
    } else if panel.preview {
        items
            .get(panel.selected)
            .and_then(|item| match &item.message {
                MessageAttachment::Text { content, .. } => Some(
                    content
                        .lines()
                        .map(|text| Line::raw(terminal_safe_text(text)))
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_default()
    } else {
        items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                Line::styled(
                    format!("{} {}", index + 1, terminal_safe_text(&item.summary())),
                    Style::default().fg(if index == panel.selected {
                        Color::LightCyan
                    } else {
                        Color::Gray
                    }),
                )
            })
            .collect()
    };
    frame.render_widget(Clear, popup);
    let cursor = panel
        .editor
        .as_ref()
        .map(|editor| editor.cursor_visual(popup.width.saturating_sub(2).max(1) as usize));
    let scroll = if let Some((row, _)) = cursor {
        row.saturating_sub(popup.height.saturating_sub(3) as usize)
            .min(u16::MAX as usize) as u16
    } else if panel.preview {
        panel.scroll
    } else {
        panel
            .selected
            .saturating_sub(popup.height.saturating_sub(4) as usize) as u16
    };
    let widget = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!("{title} · {}", items.len()))
                .title_bottom(hint)
                .borders(Borders::ALL),
        )
        .scroll((scroll, 0));
    frame.render_widget(
        if panel.preview || panel.editor.is_some() {
            widget.wrap(Wrap { trim: false })
        } else {
            widget
        },
        popup,
    );
    if let Some((row, column)) = cursor {
        let row = row.saturating_sub(scroll as usize);
        if row < popup.height.saturating_sub(2) as usize {
            frame.set_cursor_position((
                popup.x + 1 + column.min(popup.width.saturating_sub(3) as usize) as u16,
                popup.y + 1 + row as u16,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn text(content: &str) -> DraftAttachment {
        DraftAttachment {
            id: uuid::Uuid::new_v4(),
            message: MessageAttachment::Text {
                name: "same.txt".to_owned(),
                content: content.to_owned(),
            },
        }
    }
    #[test]
    fn moving_and_deleting_same_named_files_preserves_identity() {
        let mut app = App::new(Vec::new(), Language::En);
        let first = text("first");
        let second = text("second");
        let first_id = first.id;
        let second_id = second.id;
        app.attachments = vec![first, second];
        app.open_attachments(Target::Draft);
        app.handle_attachment_key(key(KeyCode::Char('j')));
        assert_eq!(app.attachments[0].id, second_id);
        assert_eq!(app.attachments[1].id, first_id);
        app.handle_attachment_key(key(KeyCode::Delete));
        assert_eq!(app.attachments.len(), 1);
        assert_eq!(app.attachments[0].id, second_id);
    }
    #[test]
    fn queued_edit_and_cancel_keep_draft_and_parent_queue() {
        let mut app = App::new(Vec::new(), Language::En);
        app.attachments = vec![text("draft")];
        app.queued_prompts.push_back(QueuedPrompt {
            text: "queued".to_owned(),
            attachments: vec![text("original")],
        });
        app.open_prompt_queue();
        app.open_attachments(Target::Queued(0));
        app.handle_attachment_key(key(KeyCode::Char('e')));
        app.attachment_paste(" changed");
        app.handle_attachment_key(key(KeyCode::Esc));
        assert!(
            matches!(&app.queued_prompts[0].attachments[0].message, MessageAttachment::Text { content, .. } if content == "original")
        );
        app.handle_attachment_key(key(KeyCode::Char('e')));
        app.attachment_paste(" saved");
        app.handle_attachment_key(key(KeyCode::Enter));
        assert!(
            matches!(&app.queued_prompts[0].attachments[0].message, MessageAttachment::Text { content, .. } if content == "original saved")
        );
        assert!(
            matches!(&app.attachments[0].message, MessageAttachment::Text { content, .. } if content == "draft")
        );
        app.handle_attachment_key(key(KeyCode::Esc));
        assert!(app.attachment_panel.is_none());
        assert!(app.prompt_queue.is_some());
    }
    #[test]
    fn clearing_requires_confirmation() {
        let mut app = App::new(Vec::new(), Language::En);
        app.attachments.push(text("keep"));
        app.open_attachments(Target::Draft);
        app.handle_attachment_key(key(KeyCode::Char('c')));
        app.handle_attachment_key(key(KeyCode::Esc));
        assert_eq!(app.attachments.len(), 1);
        app.handle_attachment_key(key(KeyCode::Char('c')));
        app.handle_attachment_key(key(KeyCode::Char('y')));
        assert!(app.attachments.is_empty());
    }
}
