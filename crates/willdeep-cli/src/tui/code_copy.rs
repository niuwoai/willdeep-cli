//! 代码块的整块复制：双击选中，以及 MuseTerm 里浮在块右上角的复制徽标。
//!
//! 代码块的行范围由 `rendering` 在画 Markdown 时标注，这里只负责交互状态、命中
//! 和剪贴板。徽标只在 MuseTerm（`gallery::hosted()`）里出现；其他终端保留双击
//! 复制，不依赖任何终端扩展。

use super::*;

/// 一个可以整块复制的代码块。`start` 含、`end` 不含，坐标是 `transcript_rows`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CodeBlock {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) content: String,
}

impl CodeBlock {
    fn line_count(&self) -> usize {
        // 末尾空行也算一行：它们是块正文的一部分，复制出去的也是这些字节。
        self.content.split('\n').count().max(1)
    }
}

/// 双击窗口。桌面系统一般在 400–500ms，这里取中间值。
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(450);
/// 徽标上的成功/失败状态保留两秒，和 Web 的复制按钮一致。
const FEEDBACK_VISIBLE: Duration = Duration::from_secs(2);
/// 状态栏提示至少停留两秒，免得被鼠标移动触发的下一帧立刻吃掉。
const NOTICE_HOLD: Duration = Duration::from_secs(2);
/// 徽标和代码块右边缘之间留一列。
const BADGE_MARGIN: u16 = 1;

#[derive(Default)]
pub(super) struct CodeCopyState {
    /// 当前终端是不是 MuseTerm：决定画不画徽标、复制走不走 OSC 52。
    hosted: bool,
    /// 最近一帧渲染出的代码块，行号和 `transcript_rows` 对齐。
    blocks: Vec<CodeBlock>,
    /// 正在显示徽标的块。
    active: Option<usize>,
    /// 徽标上的成功/失败反馈。
    feedback: Option<Feedback>,
    /// 这一帧真正画出的徽标矩形与块下标；鼠标命中的就是它。
    badge: Option<(Rect, usize)>,
    /// 上一次按下落在哪个块、什么时候，用来识别双击。
    last_down: Option<(usize, Instant)>,
    /// 这次按下之后拖动过；拖过就不是双击。
    dragged: bool,
    /// 第二次按下且中间没有拖动，等抬起时复制。
    double_click: bool,
    /// 第二次按下落在哪个块；抬起必须在同一个块上才算数。
    double_click_block: Option<usize>,
    /// 双击选中的块。选区文本优先返回它的干净正文。
    selected: Option<usize>,
    /// 非 MuseTerm 的「双击可复制」提示只出现一次。
    hint_shown: bool,
    /// 徽标按下后要连拖动、抬起一起吃掉，避免旧选区在松手时又复制一次。
    badge_pressed: bool,
}

struct Feedback {
    block: usize,
    at: Instant,
    kind: FeedbackKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FeedbackKind {
    /// 本机系统剪贴板确实写成功了。
    Copied,
    /// OSC 52 已经写出，但终端不提供回执；说「已发送」，不说「已验证」。
    Sent,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClipboardRoute {
    Local,
    Terminal,
}

pub(super) enum CodeCopyAction {
    /// 复制这个块。
    Copy(usize),
    /// 事件已被徽标吃掉，调用方不要再做选区处理。
    Consume,
}

impl CodeCopyState {
    pub(super) fn new(hosted: bool) -> Self {
        Self {
            hosted,
            ..Self::default()
        }
    }

    pub(super) fn set_blocks(&mut self, blocks: Vec<CodeBlock>) {
        self.blocks = blocks;
        if self.active.is_some_and(|index| index >= self.blocks.len()) {
            self.active = None;
        }
        if self
            .selected
            .is_some_and(|index| index >= self.blocks.len())
        {
            self.selected = None;
        }
        if self
            .feedback
            .as_ref()
            .is_some_and(|feedback| feedback.block >= self.blocks.len())
        {
            self.feedback = None;
        }
    }

    pub(super) fn clear(&mut self) {
        self.active = None;
        self.feedback = None;
        self.last_down = None;
        self.dragged = false;
        self.double_click = false;
        self.double_click_block = None;
        self.selected = None;
        self.badge_pressed = false;
    }

    pub(super) fn clear_selection(&mut self) {
        self.selected = None;
    }

    /// 双击选中的干净正文；没有时返回 `None`，选区文本回落到普通拖选。
    pub(super) fn selected_content(&self) -> Option<String> {
        self.selected
            .and_then(|index| self.blocks.get(index))
            .map(|block| block.content.clone())
    }

    fn block_at_row(&self, row: usize) -> Option<usize> {
        self.blocks
            .iter()
            .position(|block| (block.start..block.end).contains(&row))
    }

    fn block_at_point(&self, app: &App, x: u16, y: u16) -> Option<usize> {
        let content = app.transcript_content;
        if x < content.x || x >= content.right() || y < content.y || y >= content.bottom() {
            return None;
        }
        let row = app
            .transcript_render_offset
            .saturating_add(y.saturating_sub(content.y) as usize);
        self.block_at_row(row)
    }

    fn label_at(&self, language: Language, block: usize, now: Instant) -> String {
        if let Some(feedback) = &self.feedback
            && feedback.block == block
            && now.saturating_duration_since(feedback.at) <= FEEDBACK_VISIBLE
        {
            return match feedback.kind {
                FeedbackKind::Copied => language.text("✓ 已复制", "✓ Copied", "✓ コピー済み"),
                FeedbackKind::Sent => language.text("✓ 已发送", "✓ Sent", "✓ 送信済み"),
                FeedbackKind::Failed => language.text("✗ 失败", "✗ Failed", "✗ 失敗"),
            }
            .to_owned();
        }
        language.text("⧉ 复制", "⧉ Copy", "⧉ コピー").to_owned()
    }
}

/// 徽标画在代码块可见部分的最上面一行、靠右。块完全滚出视口时不画。
fn badge_rect(app: &App, block: &CodeBlock, label_width: u16) -> Option<Rect> {
    let content = app.transcript_content;
    let width = label_width.saturating_add(2);
    if content.width < width.saturating_add(BADGE_MARGIN) || content.height == 0 {
        return None;
    }
    let offset = app.transcript_render_offset;
    let viewport = app.viewport_height.max(1);
    let start = block.start.max(offset);
    let end = block.end.min(offset.saturating_add(viewport));
    if start >= end {
        return None;
    }
    let y = content.y + (start - offset) as u16;
    if y >= content.bottom() {
        return None;
    }
    let right_limit = content.right().saturating_sub(BADGE_MARGIN);
    let x = right_limit.saturating_sub(width);
    if x < content.x {
        return None;
    }
    Some(Rect {
        x,
        y,
        width,
        height: 1,
    })
}

/// 把徽标画在聊天区之上。调用方保证这时弹层还没画，徽标不会盖住弹层。
pub(super) fn draw(frame: &mut ratatui::Frame<'_>, app: &mut App) {
    app.code_copy.badge = None;
    if !app.code_copy.hosted {
        return;
    }
    let Some(block_index) = app.code_copy.active else {
        return;
    };
    let Some(block) = app.code_copy.blocks.get(block_index) else {
        app.code_copy.active = None;
        return;
    };
    let now = Instant::now();
    let label = app.code_copy.label_at(app.language, block_index, now);
    let label_width = UnicodeWidthStr::width(label.as_str()).min(u16::MAX as usize) as u16;
    let Some(rect) = badge_rect(app, block, label_width) else {
        return;
    };
    let feedback = app.code_copy.feedback.as_ref().filter(|feedback| {
        feedback.block == block_index
            && now.saturating_duration_since(feedback.at) <= FEEDBACK_VISIBLE
    });
    let style = match feedback.map(|feedback| feedback.kind) {
        Some(FeedbackKind::Copied) => Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD),
        Some(FeedbackKind::Sent) => Style::default()
            .fg(Color::Black)
            .bg(Color::LightGreen)
            .add_modifier(Modifier::BOLD),
        Some(FeedbackKind::Failed) => Style::default()
            .fg(Color::White)
            .bg(Color::Red)
            .add_modifier(Modifier::BOLD),
        None => Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    };
    let buffer = frame.buffer_mut();
    // 先铺底色，再写文字：徽标是一块实心标签，不是浮在代码上的几个字。
    // 如果左邻格是一个宽字符的第一格，连它一起盖掉，否则终端仍会把那个宽字
    // 画进徽标的第一列。
    let mut clear_from = rect.x;
    if clear_from > 0 {
        let left = &buffer[(clear_from - 1, rect.y)];
        if UnicodeWidthStr::width(left.symbol()) > 1 {
            clear_from -= 1;
        }
    }
    for x in clear_from..rect.right() {
        let cell = &mut buffer[(x, rect.y)];
        cell.set_char(' ');
        cell.set_style(style);
    }
    buffer.set_string(rect.x + 1, rect.y, label.as_str(), style);
    app.code_copy.badge = Some((rect, block_index));
}

impl App {
    /// 处理代码块相关鼠标事件。返回 `Some` 时调用方必须跳过本轮的选区/点击处理。
    pub(super) fn handle_code_copy_mouse(
        &mut self,
        mouse: MouseEvent,
        now: Instant,
    ) -> Option<CodeCopyAction> {
        let point_block = self.code_copy.block_at_point(self, mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.code_copy.badge_pressed = false;
                if let Some((rect, block)) = self.code_copy.badge
                    && rect.contains((mouse.column, mouse.row).into())
                {
                    // 徽标优先于区块本身：点右上角是复制，不是重新拖选。
                    self.code_copy.badge_pressed = true;
                    self.code_copy.active = Some(block);
                    self.code_copy.last_down = None;
                    self.code_copy.double_click = false;
                    self.code_copy.double_click_block = None;
                    self.code_copy.dragged = false;
                    self.pending_chat_click = None;
                    return Some(CodeCopyAction::Copy(block));
                }
                match point_block {
                    Some(block) => {
                        let double = !self.code_copy.dragged
                            && self.code_copy.last_down.is_some_and(|(last, at)| {
                                last == block
                                    && now.saturating_duration_since(at) <= DOUBLE_CLICK_WINDOW
                            });
                        self.code_copy.selected = None;
                        self.code_copy.double_click = double;
                        self.code_copy.double_click_block = double.then_some(block);
                        self.code_copy.last_down = if double { None } else { Some((block, now)) };
                        self.code_copy.dragged = false;
                        if self.code_copy.hosted {
                            self.code_copy.active = Some(block);
                        }
                    }
                    None => {
                        self.code_copy.active = None;
                        self.code_copy.selected = None;
                        self.code_copy.last_down = None;
                        self.code_copy.double_click = false;
                        self.code_copy.double_click_block = None;
                        self.code_copy.dragged = false;
                    }
                }
                None
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.code_copy.badge_pressed {
                    return Some(CodeCopyAction::Consume);
                }
                self.code_copy.dragged = true;
                self.code_copy.double_click = false;
                self.code_copy.double_click_block = None;
                self.code_copy.selected = None;
                if self.code_copy.hosted {
                    self.code_copy.active = point_block;
                }
                None
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.code_copy.badge_pressed {
                    self.code_copy.badge_pressed = false;
                    self.pending_chat_click = None;
                    return Some(CodeCopyAction::Consume);
                }
                if self.code_copy.double_click {
                    self.code_copy.double_click = false;
                    self.code_copy.dragged = false;
                    self.code_copy.last_down = None;
                    let expected = self.code_copy.double_click_block.take();
                    if let Some(block) = point_block
                        && Some(block) == expected
                    {
                        self.code_copy.selected = Some(block);
                        self.code_copy.active = Some(block);
                        self.select_code_block(block);
                        self.pending_chat_click = None;
                        return Some(CodeCopyAction::Copy(block));
                    }
                    return None;
                }
                self.code_copy.dragged = false;
                // 非 MuseTerm 没有徽标，第一次单击时给一句提示；只提示一次，
                // 不每点一下代码块就顶掉用户正在看的状态。
                if point_block.is_some()
                    && self.code_copy.last_down.is_some()
                    && !self.code_copy.hosted
                    && !self.code_copy.hint_shown
                {
                    self.code_copy.hint_shown = true;
                    let message = self
                        .language
                        .text(
                            "双击代码块可复制整块",
                            "Double-click a code block to copy it whole",
                            "コードブロックをダブルクリックすると全体をコピーできます",
                        )
                        .to_owned();
                    self.notice = Some(message.clone());
                    self.notice_hold = Some((message, now + NOTICE_HOLD));
                }
                None
            }
            _ => None,
        }
    }

    fn select_code_block(&mut self, block_index: usize) {
        let Some(block) = self.code_copy.blocks.get(block_index) else {
            return;
        };
        let (start, end) = (block.start, block.end);
        if end <= start || self.transcript_rows.is_empty() {
            return;
        }
        self.chat_selection = Some(ChatSelection {
            anchor: ChatSelectionPoint {
                row: start,
                column: 0,
            },
            head: ChatSelectionPoint {
                row: end - 1,
                column: usize::MAX,
            },
        });
        self.selection_mode = true;
        self.native_selection_mode = false;
        self.focus = FocusPane::Chat;
    }

    pub(super) fn copy_code_block(&mut self, index: usize) {
        let Some(block) = self.code_copy.blocks.get(index) else {
            return;
        };
        let content = block.content.clone();
        let lines = block.line_count();
        let now = Instant::now();
        match write_code_clipboard(&content, self.code_copy.hosted) {
            Ok(route) => {
                let kind = match route {
                    ClipboardRoute::Local => FeedbackKind::Copied,
                    ClipboardRoute::Terminal => FeedbackKind::Sent,
                };
                self.code_copy.feedback = Some(Feedback {
                    block: index,
                    at: now,
                    kind,
                });
                self.code_copy.active = Some(index);
                let message = copy_success_message(self.language, lines, route);
                self.notice = Some(message.clone());
                self.notice_hold = Some((message, now + NOTICE_HOLD));
            }
            Err(error) => {
                self.code_copy.feedback = Some(Feedback {
                    block: index,
                    at: now,
                    kind: FeedbackKind::Failed,
                });
                self.code_copy.active = Some(index);
                let message = format!(
                    "{}: {error}",
                    self.language.text(
                        "复制代码块失败",
                        "Copy code block failed",
                        "コードブロックのコピーに失敗",
                    )
                );
                self.notice = Some(message.clone());
                self.notice_hold = Some((message, now + NOTICE_HOLD));
            }
        }
    }
}

fn copy_success_message(language: Language, lines: usize, route: ClipboardRoute) -> String {
    let single = lines == 1;
    let template = match (route, single) {
        (ClipboardRoute::Local, true) => language.text(
            "已复制 1 行代码",
            "Copied 1 line of code",
            "1 行のコードをコピーしました",
        ),
        (ClipboardRoute::Local, false) => language.text(
            "已复制 {n} 行代码",
            "Copied {n} lines of code",
            "{n} 行のコードをコピーしました",
        ),
        (ClipboardRoute::Terminal, true) => language.text(
            "已发送 1 行代码到终端剪贴板",
            "Sent 1 line of code to the terminal clipboard",
            "1 行のコードを端末のクリップボードへ送信しました",
        ),
        (ClipboardRoute::Terminal, false) => language.text(
            "已发送 {n} 行代码到终端剪贴板",
            "Sent {n} lines of code to the terminal clipboard",
            "{n} 行のコードを端末のクリップボードへ送信しました",
        ),
    };
    template.replace("{n}", &lines.to_string())
}

/// 先写本机系统剪贴板；MuseTerm 和 SSH 会话再发 OSC 52，让终端把代码写进用户
/// 本机的剪贴板。SSH 上不碰远端 arboard——那通常只会失败，对用户也没有意义。
/// OSC 52 没有回执，成功返回 `Terminal`，调用方据此说「已发送」而不是「已复制」。
fn write_code_clipboard(text: &str, hosted: bool) -> Result<ClipboardRoute, String> {
    let ssh = ["SSH_CONNECTION", "SSH_TTY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    let mut terminal = (hosted || ssh) && crate::gallery::write_osc52(text).is_ok();
    let local = if ssh {
        None
    } else {
        Some(
            arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text.to_owned())),
        )
    };
    if let Some(Ok(())) = &local {
        return Ok(ClipboardRoute::Local);
    }
    if !terminal && !hosted {
        // 普通终端里 arboard 失败时也试一次 OSC 52；写不出去就明确报错。
        terminal = crate::gallery::write_osc52(text).is_ok();
    }
    if terminal {
        return Ok(ClipboardRoute::Terminal);
    }
    match local {
        Some(Err(error)) => Err(error.to_string()),
        _ => Err("terminal clipboard unavailable".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_block() -> App {
        let mut app = App::new(
            vec!["WillDeep: ```\nlet x = 1;\n```".to_owned()],
            Language::En,
        );
        app.transcript_width = 20;
        app.transcript_rect = Rect::new(0, 0, 20, 7);
        app.transcript_content = Rect::new(1, 1, 18, 5);
        app.viewport_height = 5;
        app.transcript_render_offset = 0;
        app.transcript_rows = vec!["WillDeep: ".to_owned(), "let x = 1;".to_owned()];
        app.code_copy.hosted = false;
        app.code_copy.set_blocks(vec![CodeBlock {
            start: 1,
            end: 2,
            content: "let x = 1;".to_owned(),
        }]);
        app
    }

    fn mouse(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 2,
            row: 2,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn double_click_selects_and_copies_the_whole_block() {
        let mut app = app_with_block();
        let start = Instant::now();
        let down = mouse(MouseEventKind::Down(MouseButton::Left));
        let up = mouse(MouseEventKind::Up(MouseButton::Left));

        assert!(app.handle_code_copy_mouse(down, start).is_none());
        assert!(
            app.handle_code_copy_mouse(up, start + Duration::from_millis(30))
                .is_none()
        );
        assert!(
            app.handle_code_copy_mouse(down, start + Duration::from_millis(60))
                .is_none(),
            "第二次按下只记状态，不立刻复制"
        );
        let action = app.handle_code_copy_mouse(up, start + Duration::from_millis(90));
        assert!(matches!(action, Some(CodeCopyAction::Copy(0))));
        assert!(app.selection_mode, "整块选中要能用 Ctrl+C 再复制一次");
        assert_eq!(app.selected_chat_text(), "let x = 1;");
    }

    #[test]
    fn a_drag_between_clicks_cancels_the_double_click() {
        let mut app = app_with_block();
        let start = Instant::now();
        let down = mouse(MouseEventKind::Down(MouseButton::Left));
        let up = mouse(MouseEventKind::Up(MouseButton::Left));
        let drag = mouse(MouseEventKind::Drag(MouseButton::Left));

        app.handle_code_copy_mouse(down, start);
        app.handle_code_copy_mouse(up, start + Duration::from_millis(20));
        app.handle_code_copy_mouse(down, start + Duration::from_millis(40));
        app.handle_code_copy_mouse(drag, start + Duration::from_millis(50));
        app.handle_code_copy_mouse(up, start + Duration::from_millis(60));
        app.handle_code_copy_mouse(down, start + Duration::from_millis(70));
        assert!(
            app.handle_code_copy_mouse(up, start + Duration::from_millis(80))
                .is_none()
        );
        assert!(app.code_copy.selected.is_none());
    }

    #[test]
    fn a_slow_second_click_is_not_a_double_click() {
        let mut app = app_with_block();
        let start = Instant::now();
        let down = mouse(MouseEventKind::Down(MouseButton::Left));
        let up = mouse(MouseEventKind::Up(MouseButton::Left));

        app.handle_code_copy_mouse(down, start);
        app.handle_code_copy_mouse(up, start + Duration::from_millis(10));
        app.handle_code_copy_mouse(down, start + Duration::from_millis(600));
        let action = app.handle_code_copy_mouse(up, start + Duration::from_millis(610));
        assert!(action.is_none());
        assert!(app.code_copy.selected.is_none());
    }

    #[test]
    fn the_badge_click_is_consumed_before_the_block_selection() {
        let mut app = app_with_block();
        app.code_copy.hosted = true;
        app.code_copy.active = Some(0);
        app.code_copy.badge = Some((Rect::new(2, 2, 7, 1), 0));
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        assert!(matches!(
            app.handle_code_copy_mouse(down, Instant::now()),
            Some(CodeCopyAction::Copy(0))
        ));
        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 3,
            row: 2,
            modifiers: KeyModifiers::NONE,
        };
        assert!(matches!(
            app.handle_code_copy_mouse(up, Instant::now()),
            Some(CodeCopyAction::Consume)
        ));
    }

    #[test]
    fn the_badge_only_exists_for_hosted_terminals() {
        let app = app_with_block();
        let block = &app.code_copy.blocks[0];
        assert!(badge_rect(&app, block, 4).is_some());

        let mut app = app_with_block();
        app.transcript_render_offset = 5;
        assert!(
            badge_rect(&app, &app.code_copy.blocks[0], 4).is_none(),
            "滚出视口后不画徽标"
        );
    }

    #[test]
    fn the_badge_sits_on_the_first_visible_row_of_the_block() {
        let mut app = app_with_block();
        app.code_copy.blocks[0].start = 0;
        app.code_copy.blocks[0].end = 10;
        app.transcript_render_offset = 3;
        app.viewport_height = 4;
        let block = &app.code_copy.blocks[0];
        // 内容矩形 x=1..19，徽标宽 4+2=6，右边留 1 列。
        assert_eq!(badge_rect(&app, block, 4), Some(Rect::new(12, 1, 6, 1)));
    }

    #[test]
    fn the_feedback_label_expires_after_two_seconds() {
        let mut state = CodeCopyState::new(true);
        let now = Instant::now();
        state.feedback = Some(Feedback {
            block: 0,
            at: now,
            kind: FeedbackKind::Copied,
        });
        assert_eq!(state.label_at(Language::En, 0, now), "✓ Copied");
        assert_eq!(
            state.label_at(Language::En, 0, now + Duration::from_secs(3)),
            "⧉ Copy"
        );

        state.feedback = Some(Feedback {
            block: 0,
            at: now,
            kind: FeedbackKind::Sent,
        });
        assert_eq!(state.label_at(Language::En, 0, now), "✓ Sent");
        state.feedback = Some(Feedback {
            block: 0,
            at: now,
            kind: FeedbackKind::Failed,
        });
        assert_eq!(state.label_at(Language::En, 0, now), "✗ Failed");
    }

    #[test]
    fn a_single_click_explains_double_click_once() {
        let mut app = app_with_block();
        let start = Instant::now();
        let down = mouse(MouseEventKind::Down(MouseButton::Left));
        let up = mouse(MouseEventKind::Up(MouseButton::Left));
        app.handle_code_copy_mouse(down, start);
        app.handle_code_copy_mouse(up, start + Duration::from_millis(10));
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("Double-click")),
            "{:?}",
            app.notice
        );

        app.notice = None;
        app.handle_code_copy_mouse(down, start + Duration::from_millis(600));
        app.handle_code_copy_mouse(up, start + Duration::from_millis(610));
        assert!(app.notice.is_none(), "提示只出现一次");
    }

    #[test]
    fn stale_blocks_drop_the_badge_and_selection() {
        let mut state = CodeCopyState::new(true);
        state.set_blocks(vec![CodeBlock {
            start: 0,
            end: 1,
            content: "x".to_owned(),
        }]);
        state.active = Some(0);
        state.selected = Some(0);
        state.set_blocks(Vec::new());
        assert!(state.active.is_none());
        assert!(state.selected.is_none());
    }

    fn paint(app: &mut App) -> ratatui::buffer::Buffer {
        let backend = ratatui::backend::TestBackend::new(24, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new("WillDeep: \nlet x = 1;"),
                    Rect::new(0, 0, 24, 8),
                );
                draw(frame, app);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn the_badge_is_painted_at_the_top_right_of_the_block() {
        let mut app = app_with_block();
        app.code_copy.hosted = true;
        app.code_copy.active = Some(0);
        // 让块的第一行正好落在正文矩形第一行，方便断言。
        app.transcript_render_offset = 1;
        let backend = paint(&mut app);
        let (rect, _) = app.code_copy.badge.expect("badge was painted");
        assert_eq!(backend[(rect.x, rect.y)].bg, Color::Cyan);
        assert_eq!(backend[(rect.x + 1, rect.y)].symbol(), "⧉");
        let screen = (0..8)
            .map(|y| {
                (0..24)
                    .map(|x| backend[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(screen.contains("⧉ Copy"), "{screen}");
    }

    #[test]
    fn a_terminal_without_the_muse_term_marker_gets_no_badge() {
        let mut app = app_with_block();
        app.code_copy.hosted = false;
        app.code_copy.active = Some(0);
        let backend = paint(&mut app);
        assert!(app.code_copy.badge.is_none());
        let screen = (0..8)
            .map(|y| {
                (0..24)
                    .map(|x| backend[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!screen.contains("⧉"), "{screen}");
    }
}
