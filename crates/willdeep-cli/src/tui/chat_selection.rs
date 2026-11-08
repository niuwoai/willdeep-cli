//! 聊天区的文本选择：拖选、复制、引用，以及「按下 → 松手」的点击时序。
//!
//! 从 `app_state.rs` 拆出来：那个文件本来就压在 3000 行的线边上，再加一段就
//! 过不去，而这一块是自成体系的——它只碰选区、剪贴板与焦点。

use super::*;

impl App {
    pub(super) fn transcript_selection_point(
        &self,
        x: u16,
        y: u16,
        clamp_to_viewport: bool,
    ) -> Option<ChatSelectionPoint> {
        let content = self.transcript_content;
        if self.transcript_rows.is_empty()
            || content.width < 1
            || content.height < 1
            || self.transcript_rect.width < 3
            || self.transcript_rect.height < 3
        {
            return None;
        }
        // 落点按「正文矩形」算，不按外框：完整 UI 有边框、简洁 UI 只有左右内边距，
        // 两种布局正文的起点不同，拿外框加固定偏移会让整列偏一格、整行偏一行。
        let left = content.x;
        let right = content.right().saturating_sub(1);
        let top = content.y;
        let bottom = content.bottom().saturating_sub(1);
        if top > bottom || left > right {
            return None;
        }
        if !clamp_to_viewport && (x < left || x > right || y < top || y > bottom) {
            return None;
        }
        let visible_row = y.clamp(top, bottom).saturating_sub(top) as usize;
        let row = self
            .transcript_render_offset
            .saturating_add(visible_row)
            .min(self.transcript_rows.len().saturating_sub(1));
        let row_width = UnicodeWidthStr::width(self.transcript_rows[row].as_str());
        let column = x
            .clamp(left, right)
            .saturating_sub(left)
            .min(row_width.saturating_sub(1).min(u16::MAX as usize) as u16)
            as usize;
        Some(ChatSelectionPoint { row, column })
    }

    pub(super) fn handle_chat_selection_mouse(&mut self, mouse: MouseEvent) -> bool {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(point) = self.transcript_selection_point(mouse.column, mouse.row, false)
                else {
                    // 点在聊天区外（通常是输入框）：退出选区，让这一下照常去切焦点。
                    // 以前这里原样返回 `selection_mode`，选过字之后点输入框会被整个
                    // 吃掉，焦点切不过去、键盘也还锁在选区模式里。
                    if self.selection_mode {
                        self.exit_selection_mode();
                    }
                    return false;
                };
                self.chat_selection = Some(ChatSelection {
                    anchor: point,
                    head: point,
                });
                self.pending_chat_click = None;
                self.focus = FocusPane::Chat;
                self.selection_mode
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(mut selection) = self.chat_selection else {
                    return false;
                };
                let Some(point) = self.transcript_selection_point(mouse.column, mouse.row, true)
                else {
                    return false;
                };
                selection.head = point;
                self.chat_selection = Some(selection);
                // 真拖起来了：按下时记的「单击点开」作废，别在松手后又把那一行
                // 展开一次——用户要的是选字，不是展开。
                self.pending_chat_click = None;
                self.selection_mode = true;
                self.native_selection_mode = false;
                self.focus = FocusPane::Chat;
                true
            }
            MouseEventKind::Up(MouseButton::Left) if self.chat_selection.is_some() => {
                if !self.selection_mode {
                    self.chat_selection = None;
                    // 没拖动就是一次单击：按下时先记下那一行，到松手才点开，
                    // 这样从用户消息或工具行上起手拖选不会被点开动作打断。
                    if let Some(y) = self.pending_chat_click.take() {
                        // 附件标签优先：点它是查看，不是折叠。
                        if let Some(action) = self.open_attachment_at(y) {
                            self.pending_media_action = Some(action);
                        } else if !self.toggle_user_message_at(y) && !self.toggle_thought_at(y) {
                            self.toggle_tool_row_at(y);
                        }
                    }
                    return false;
                }
                if let Some(point) = self.transcript_selection_point(mouse.column, mouse.row, true)
                    && let Some(selection) = self.chat_selection.as_mut()
                {
                    selection.head = point;
                }
                // 拖完松手就复制（Claude Code 的手感），不用再按键。有人只是拖一下
                // 看看，不想把剪贴板里已经复制的网址顶掉——那是 `[ui] copy_on_select
                // = false` 的事；默认开，是因为「选完还要记着按 Ctrl+C」更容易忘。
                match self.selection_copy_text() {
                    Some(value) => {
                        let chars = value.chars().count();
                        self.write_selection_to_clipboard(&value, chars);
                    }
                    None => {
                        let chars = self.selected_chat_text().chars().count();
                        self.notice = Some(
                            self.language
                                .text(
                                    "已选中 {n} 字 · Ctrl+C 或 y 复制 · q 引用 · Esc 取消",
                                    "{n} chars selected · Ctrl+C or y copies · q quotes · Esc cancels",
                                    "{n} 文字を選択 · Ctrl+C か y でコピー · q で引用 · Esc で解除",
                                )
                                .replace("{n}", &chars.to_string()),
                        );
                    }
                }
                true
            }
            MouseEventKind::ScrollUp if self.selection_mode => {
                self.scroll_up(3);
                true
            }
            MouseEventKind::ScrollDown if self.selection_mode => {
                self.scroll_down(3);
                true
            }
            _ => false,
        }
    }

    pub(super) fn enter_native_selection_mode(&mut self) {
        self.selection_mode = true;
        self.native_selection_mode = true;
        self.chat_selection = None;
        self.focus = FocusPane::Chat;
    }

    /// 选区模式里按了一个不属于选区的键：退出选区，把这个键交还给正常处理。
    /// 能打进输入框的键顺带把焦点切到输入框——用户是想打字，不是想被锁住。
    pub(super) fn release_selection_for_key(&mut self, key: KeyEvent) {
        self.exit_selection_mode();
        let typing = match key.code {
            KeyCode::Char(_) => !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER),
            KeyCode::Backspace | KeyCode::Enter | KeyCode::Delete => true,
            _ => false,
        };
        if typing {
            self.focus = FocusPane::Prompt;
        }
    }

    pub(super) fn exit_selection_mode(&mut self) {
        self.selection_mode = false;
        self.native_selection_mode = false;
        self.chat_selection = None;
    }

    pub(super) fn selected_chat_text(&self) -> String {
        let Some(selection) = self.chat_selection else {
            return String::new();
        };
        let (start, end) = selection.ordered_range();
        selected_text(&self.transcript_rows, start, end)
    }

    /// 松手时该不该自动复制、复制什么。关掉 `copy_on_select` 或选区为空都返回 `None`，
    /// 调用方退回到「已选中 N 字」的提示，Ctrl+C / y 仍然可用。
    pub(super) fn selection_copy_text(&self) -> Option<String> {
        if !self.ui.copy_on_select {
            return None;
        }
        let value = self.selected_chat_text();
        (!value.is_empty()).then_some(value)
    }

    /// 写剪贴板并把结果说清楚：失败时保留错误原因，不假装复制成功。
    fn write_selection_to_clipboard(&mut self, value: &str, chars: usize) {
        match arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_text(value.to_owned()))
        {
            Ok(()) => {
                self.notice = Some(
                    self.language
                        .text(
                            "已复制 {n} 字到剪贴板 · q 引用 · Esc 取消",
                            "Copied {n} chars to clipboard · q quotes · Esc cancels",
                            "{n} 文字をクリップボードにコピー · q で引用 · Esc で解除",
                        )
                        .replace("{n}", &chars.to_string()),
                );
            }
            Err(error) => {
                self.notice = Some(format!(
                    "{}: {error}",
                    self.language.text(
                        "复制到剪贴板失败",
                        "Copy to clipboard failed",
                        "クリップボードへのコピーに失敗"
                    )
                ));
            }
        }
    }

    pub(super) fn copy_chat_selection(&mut self) {
        let value = self.selected_chat_text();
        if value.is_empty() {
            self.notice = Some(
                self.language
                    .text(
                        "请先在聊天区拖动选择文字",
                        "Drag to select chat text first",
                        "先にチャット内の文字をドラッグ選択してください",
                    )
                    .to_owned(),
            );
            return;
        }
        let chars = value.chars().count();
        self.write_selection_to_clipboard(&value, chars);
    }

    pub(super) fn quote_chat_selection(&mut self) {
        let value = self.selected_chat_text();
        if value.is_empty() {
            self.notice = Some(
                self.language
                    .text(
                        "请先在聊天区拖动选择文字",
                        "Drag to select chat text first",
                        "先にチャット内の文字をドラッグ選択してください",
                    )
                    .to_owned(),
            );
            return;
        }
        if !self.input.is_empty() {
            self.input.insert("\n\n");
        }
        self.input.insert(&quote_selected_text(&value));
        self.selection_mode = false;
        self.native_selection_mode = false;
        self.chat_selection = None;
        self.focus = FocusPane::Prompt;
        self.notice = Some(
            self.language
                .text(
                    "已引用到输入框",
                    "Selection quoted into the prompt",
                    "選択範囲を入力欄に引用しました",
                )
                .to_owned(),
        );
    }
}
