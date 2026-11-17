//! 输入草稿的鼠标选区。范围保存在编辑器原文中，显示与命中共用正文矩形。

use super::*;

const PROMPT_SCROLL_LINES: usize = 3;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PromptSelectionKey {
    Copy(String),
    Cancel,
    Pass,
}

impl App {
    pub(super) fn prompt_content_rect(&self) -> Rect {
        Rect::new(
            self.prompt_rect.x.saturating_add(1),
            self.prompt_rect.y.saturating_add(1),
            self.prompt_rect.width.saturating_sub(2),
            self.prompt_rect
                .height
                .saturating_sub(if self.quiet_drawn { 1 } else { 2 }),
        )
    }

    fn prompt_selection_point(&self, x: u16, y: u16, clamp: bool) -> Option<(usize, usize)> {
        let body = self.prompt_content_rect();
        if body.width == 0 || body.height == 0 {
            return None;
        }
        if !clamp && !body.contains((x, y).into()) {
            return None;
        }
        let row =
            y.clamp(body.y, body.bottom() - 1).saturating_sub(body.y) as usize + self.prompt_scroll;
        // 拖到右边框可取末尾插入点，因而能选中最后一列的字。
        let column = x.clamp(body.x, body.right()).saturating_sub(body.x) as usize;
        Some((row, column))
    }

    pub(super) fn update_prompt_scroll(&mut self, cursor_row: usize, visible: usize) {
        let visible = visible.max(1);
        if !self.input.selection_active() {
            self.prompt_scroll = cursor_row.saturating_sub(visible - 1);
        }
        self.prompt_scroll = self.prompt_scroll.min(
            self.input
                .visual_line_count(self.prompt_content_rect().width.max(1) as usize)
                .saturating_sub(visible),
        );
    }

    pub(super) fn position_prompt_cursor(&mut self, x: u16, y: u16) {
        if let Some((row, column)) = self.prompt_selection_point(x, y, true) {
            self.input.set_cursor_visual(
                row,
                column,
                self.prompt_content_rect().width.max(1) as usize,
            );
        }
    }

    /// 必须先于代码块与聊天选区接鼠标；输入区拖出边界仍归原来的选区。
    pub(super) fn handle_prompt_selection_mouse(&mut self, mouse: MouseEvent) -> bool {
        if self.has_open_overlay() || self.permission_picker.is_some() {
            return false;
        }
        let width = self.prompt_content_rect().width.max(1) as usize;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.input.clear_selection();
                let point = (mouse.column, mouse.row).into();
                if [
                    self.sidebar_rect,
                    self.command_rect,
                    self.reference_rect,
                    self.skill_rect,
                ]
                .iter()
                .any(|rect| rect.contains(point))
                {
                    return false;
                }
                let Some((row, column)) =
                    self.prompt_selection_point(mouse.column, mouse.row, false)
                else {
                    return false;
                };
                self.exit_selection_mode();
                self.pending_chat_click = None;
                self.focus = FocusPane::Prompt;
                self.composer_placeholder_dismissed = true;
                self.input.begin_selection_visual(row, column, width);
                true
            }
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
                if self.input.selection_dragging() =>
            {
                if let Some((row, column)) =
                    self.prompt_selection_point(mouse.column, mouse.row, true)
                {
                    self.input.extend_selection_visual(row, column, width);
                }
                if matches!(mouse.kind, MouseEventKind::Up(_)) {
                    self.input.finish_selection();
                    if let Some(value) = self.prompt_selection_copy_text(true) {
                        self.copy_prompt_text(&value);
                    } else if self.input.selected_text().is_some() {
                        self.notice = Some(
                            self.language
                                .text(
                                    "已选中输入文字 · Ctrl/Cmd+C 复制 · Esc 取消",
                                    "Prompt text selected · Ctrl/Cmd+C copies · Esc cancels",
                                    "入力文字を選択 · Ctrl/Cmd+C でコピー · Esc で解除",
                                )
                                .to_owned(),
                        );
                    }
                }
                true
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if self.input.selection_active()
                    && self.prompt_rect.contains((mouse.column, mouse.row).into()) =>
            {
                let body = self.prompt_content_rect();
                let max = self
                    .input
                    .visual_line_count(width)
                    .saturating_sub(body.height as usize);
                self.prompt_scroll = if mouse.kind == MouseEventKind::ScrollUp {
                    self.prompt_scroll.saturating_sub(PROMPT_SCROLL_LINES)
                } else {
                    self.prompt_scroll
                        .saturating_add(PROMPT_SCROLL_LINES)
                        .min(max)
                };
                true
            }
            _ => false,
        }
    }

    /// 自动复制遵守配置；显式复制始终使用原文，空格和空行也是有效内容。
    pub(super) fn prompt_selection_copy_text(&self, automatic: bool) -> Option<String> {
        if automatic && !self.ui.copy_on_select {
            return None;
        }
        self.input.selected_text().map(str::to_owned)
    }

    pub(super) fn prompt_selection_key(&self, key: KeyEvent) -> PromptSelectionKey {
        // 审批、提问等弹层可能在拖选之后异步出现，优先保留它们的按键语义。
        if self.has_open_overlay() || self.permission_picker.is_some() {
            return PromptSelectionKey::Pass;
        }
        if let Some(value) = self.input.selected_text()
            && matches!(key.code, KeyCode::Char('c' | 'C'))
            && key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        {
            return PromptSelectionKey::Copy(value.to_owned());
        }
        if self.input.selection_active() && key.code == KeyCode::Esc {
            return PromptSelectionKey::Cancel;
        }
        PromptSelectionKey::Pass
    }

    /// 返回 true 才消费按键；y/q 与其它输入键交还正常编辑路径。
    pub(super) fn handle_prompt_selection_key(&mut self, key: KeyEvent) -> bool {
        match self.prompt_selection_key(key) {
            PromptSelectionKey::Copy(value) => self.copy_prompt_text(&value),
            PromptSelectionKey::Cancel => {
                self.input.clear_selection();
                self.notice = None;
            }
            PromptSelectionKey::Pass => {
                self.input.clear_selection();
                return false;
            }
        }
        true
    }

    fn copy_prompt_text(&mut self, value: &str) {
        match self.code_copy.write_clipboard(value) {
            Ok(route) => {
                let message = match route {
                    code_copy::ClipboardRoute::Local => self.language.text(
                        "已复制输入文字 {n} 字 · Esc 取消选区",
                        "Copied {n} prompt chars · Esc clears selection",
                        "入力文字 {n} 文字をコピー · Esc で選択解除",
                    ),
                    code_copy::ClipboardRoute::Terminal => self.language.text(
                        "已发送输入文字 {n} 字到终端剪贴板 · Esc 取消选区",
                        "Sent {n} prompt chars to the terminal clipboard · Esc clears selection",
                        "入力文字 {n} 文字を端末のクリップボードへ送信 · Esc で選択解除",
                    ),
                };
                self.notice = Some(message.replace("{n}", &value.chars().count().to_string()));
            }
            Err(error) => {
                self.notice = Some(format!(
                    "{}: {error}",
                    self.language.text(
                        "复制输入文字到剪贴板失败",
                        "Copy prompt text to clipboard failed",
                        "入力文字のクリップボードへのコピーに失敗",
                    )
                ));
            }
        }
    }

    pub(super) fn render_prompt_selection(&self, frame: &mut ratatui::Frame<'_>) {
        let body = self.prompt_content_rect();
        let style = Style::default()
            .fg(MODAL_FG)
            .bg(MODAL_BG)
            .add_modifier(Modifier::BOLD);
        for (row, columns) in self
            .input
            .selection_visual_ranges(body.width.max(1) as usize)
        {
            let Some(visible_row) = row.checked_sub(self.prompt_scroll) else {
                continue;
            };
            if visible_row >= body.height as usize {
                continue;
            }
            for column in columns.start..columns.end.min(body.width as usize) {
                let point = (body.x + column as u16, body.y + visible_row as u16);
                if frame.area().contains(point.into()) {
                    frame.buffer_mut()[point].set_style(style);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "prompt_selection_tests.rs"]
mod tests;
