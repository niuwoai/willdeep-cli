//! 点击弹层外部 == 按 `Esc`。
//!
//! 终端里没有「焦点环」这种东西，弹层开着的时候用户唯一的退路是记得按 `Esc`。
//! 鼠标点到输入框上，视觉上像是已经离开了弹层，实际弹层还盖在上面，下一次
//! 敲键盘全被它吃掉——这一步就是把「点外面」接到和 `Esc` 同一个出口上，
//! 顺带把焦点交给点中的那块面板：点输入框的人就是想在那里打字。
//!
//! 审批与提问弹层**不在**可关闭之列：那里的 `Esc` 分别等于「拒绝」和「取消」，
//! 手滑点一下屏幕就替一条命令签了字，代价和关掉一个详情面板完全不是一个量级。
//! Diff 审阅同理留在外面——它有 Commit Preview、搜索、撤销确认等嵌套层，
//! `Esc` 在那里是「退一层」而不是「关掉」，用一次外部点击去模拟只会更糊涂。

use super::*;

/// 可以被「点外面」关掉的弹层。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DismissibleOverlay {
    Media,
    MobileQr,
    AttentionDetail,
    TaskDetail,
    WorktreeReview,
    AgentDetail,
    RoutingSettings,
    ModelPicker,
    WorkspacePicker,
    SessionPicker,
    Palette,
    Search,
    Help,
}

/// 最上层弹层的处置方式。`Blocking` 是「开着但不许这么关」，
/// 它必须参与排序，否则审批框盖在详情面板上时，点一下会把下面那层关掉。
enum OverlayLayer {
    Dismissible(DismissibleOverlay, Rect),
    Blocking,
}

impl App {
    pub(super) fn has_open_overlay(&self) -> bool {
        self.topmost_overlay().is_some()
            || self.prompt_queue.is_some()
            || self.attachment_panel.is_some()
    }

    /// Repaint the visible Esc label as a mouse affordance, using terminal-cell
    /// widths. A clipped title gets a compact control on its right border.
    pub(super) fn render_overlay_close_hint(
        &mut self,
        frame: &mut ratatui::Frame<'_>,
        overlay: DismissibleOverlay,
        popup: Rect,
        title: &str,
    ) {
        let Some(index) = title.rfind("Esc") else {
            return;
        };
        let width = popup.width.saturating_sub(2);
        if width < 5 || popup.height == 0 {
            return;
        }
        let offset = UnicodeWidthStr::width(&title[..index]) as u16;
        let label = &title[index..];
        let label_width = UnicodeWidthStr::width(label) as u16;
        let (text, x, width) = if offset.saturating_add(label_width) <= width {
            (label, popup.x + 1 + offset, label_width)
        } else {
            // Repaint the title row before shortening it: starting an ASCII
            // button in the trailing cell of a CJK glyph leaves a broken glyph.
            let style = frame.buffer_mut()[(popup.x, popup.y)].style();
            frame.render_widget(
                Paragraph::new("─".repeat(width as usize)).style(style),
                Rect::new(popup.x + 1, popup.y, width, 1),
            );
            frame.render_widget(
                Paragraph::new(&title[..index]).style(style),
                Rect::new(popup.x + 1, popup.y, width.saturating_sub(6), 1),
            );
            ("[Esc]", popup.right() - 6, 5)
        };
        let rect = Rect::new(x, popup.y, width, 1);
        frame.render_widget(
            Paragraph::new(text).style(
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::UNDERLINED | Modifier::BOLD),
            ),
            rect,
        );
        self.overlay_close_hits.push((overlay, rect));
    }

    pub(super) fn dismiss_overlay_on_close_click(&mut self, column: u16, row: u16) -> bool {
        let Some(OverlayLayer::Dismissible(overlay, popup)) = self.topmost_overlay() else {
            return false;
        };
        let point = (column, row).into();
        if !popup.contains(point)
            || !self
                .overlay_close_hits
                .iter()
                .any(|(kind, rect)| *kind == overlay && rect.contains(point))
        {
            return false;
        }
        self.dismiss_overlay(overlay);
        self.overlay_close_hits.clear();
        true
    }

    /// 最上层的弹层，顺序与键盘 `Esc` 的优先级严格一致（见 `tui.rs` 的按键分发）。
    fn topmost_overlay(&self) -> Option<OverlayLayer> {
        let layers = [
            (self.mobile_qr.is_some()).then_some(OverlayLayer::Dismissible(
                DismissibleOverlay::MobileQr,
                self.mobile_qr_rect,
            )),
            (self.question.is_some()).then_some(OverlayLayer::Blocking),
            (self.approval.is_some()).then_some(OverlayLayer::Blocking),
            (self.media.is_open()).then_some(OverlayLayer::Dismissible(
                DismissibleOverlay::Media,
                self.media.rect,
            )),
            (self.attention_detail.is_some()).then_some(OverlayLayer::Dismissible(
                DismissibleOverlay::AttentionDetail,
                self.attention_detail_rect,
            )),
            (self.task_detail.is_some()).then_some(OverlayLayer::Dismissible(
                DismissibleOverlay::TaskDetail,
                self.task_detail_rect,
            )),
            (self.worktree_review.is_some()).then_some(OverlayLayer::Dismissible(
                DismissibleOverlay::WorktreeReview,
                self.worktree_review_rect,
            )),
            (self.agent_detail.is_some()).then_some(OverlayLayer::Dismissible(
                DismissibleOverlay::AgentDetail,
                self.agent_detail_rect,
            )),
            (self.diff_review.is_some()).then_some(OverlayLayer::Blocking),
            // 回退面板自己处理 Esc（确认阶段的 Esc 是「返回列表」，不是关掉）。
            (self.rewind_picker.is_some()).then_some(OverlayLayer::Blocking),
            (self.routing_settings.is_some()).then_some(OverlayLayer::Dismissible(
                DismissibleOverlay::RoutingSettings,
                self.routing_settings_rect,
            )),
            (self.model_picker.is_some()).then_some(OverlayLayer::Dismissible(
                DismissibleOverlay::ModelPicker,
                self.model_picker_rect,
            )),
            (self.workspace_picker.is_some()).then_some(OverlayLayer::Dismissible(
                DismissibleOverlay::WorkspacePicker,
                self.workspace_picker_rect,
            )),
            (self.session_picker.is_some()).then_some(OverlayLayer::Dismissible(
                DismissibleOverlay::SessionPicker,
                self.session_picker_rect,
            )),
            (self.palette.is_some()).then_some(OverlayLayer::Dismissible(
                DismissibleOverlay::Palette,
                self.palette_rect,
            )),
            (self.search.is_some()).then_some(OverlayLayer::Dismissible(
                DismissibleOverlay::Search,
                self.search_rect,
            )),
            (self.help_visible).then_some(OverlayLayer::Dismissible(
                DismissibleOverlay::Help,
                self.help_rect,
            )),
        ];
        layers.into_iter().flatten().next()
    }

    /// 落在最上层弹层外面、且该弹层可以这么关时，返回它。
    pub(super) fn overlay_dismissed_by_click(
        &self,
        column: u16,
        row: u16,
    ) -> Option<DismissibleOverlay> {
        let OverlayLayer::Dismissible(overlay, rect) = self.topmost_overlay()? else {
            return None;
        };
        // 这一帧还没画出来（刚打开、或者终端小到画不下）就没有边界可言，
        // 此时认定「点在里面」，不去关一个用户还没看见的弹层。
        if rect.width == 0 || rect.height == 0 {
            return None;
        }
        (!rect.contains((column, row).into())).then_some(overlay)
    }

    /// 关掉弹层之后，焦点交给点中的那块面板——点输入框的人就是想在那里打字，
    /// 让他再点一次纯属添堵。只转移焦点、只挪光标，**不**触发点中位置的动作：
    /// 一次点击不该既关掉弹层又顺手点开侧栏里的另一条 Inbox。
    fn focus_pane_after_dismiss(&mut self, column: u16, row: u16) {
        let point = (column, row).into();
        // 命中顺序与 `handle_mouse` 一致：窄屏时侧栏是压在正文上的浮层。
        if self.sidebar_rect.contains(point) {
            self.focus = FocusPane::Sidebar;
        } else if self.transcript_rect.contains(point) {
            self.focus = FocusPane::Chat;
        } else if self.activity_rect.contains(point) {
            self.focus = FocusPane::Activity;
        } else if self.prompt_rect.contains(point) {
            self.focus = FocusPane::Prompt;
            let line = row.saturating_sub(self.prompt_rect.y + 1) as usize + self.prompt_scroll;
            let column = column.saturating_sub(self.prompt_rect.x + 1) as usize;
            self.input.set_cursor_visual(
                line,
                column,
                self.prompt_rect.width.saturating_sub(2) as usize,
            );
        }
    }

    /// 走 `Esc` 那条路把弹层关掉，再把焦点落到点中的面板上。返回 `true` 表示
    /// 这次点击已被消化，调用方不该再让它触发下面那层的动作。
    pub(super) fn dismiss_overlay_on_outside_click(&mut self, column: u16, row: u16) -> bool {
        let Some(overlay) = self.overlay_dismissed_by_click(column, row) else {
            return false;
        };
        self.dismiss_overlay(overlay);
        self.focus_pane_after_dismiss(column, row);
        true
    }

    fn dismiss_overlay(&mut self, overlay: DismissibleOverlay) {
        match overlay {
            DismissibleOverlay::Media => self.media.close(),
            DismissibleOverlay::MobileQr => self.mobile_qr = None,
            DismissibleOverlay::AttentionDetail => self.attention_detail = None,
            DismissibleOverlay::TaskDetail => self.task_detail = None,
            DismissibleOverlay::WorktreeReview => self.worktree_review = None,
            DismissibleOverlay::AgentDetail => {
                self.agent_detail = None;
                self.agent_detail_scroll = 0;
            }
            // 路由设置里还有一层「改模型名」的行内编辑器，`Esc` 先收编辑器再关面板，
            // 这里复用同一个处理函数，免得两条路径的语义各走各的。
            DismissibleOverlay::RoutingSettings => {
                if let RoutingSettingsAction::Close = self
                    .handle_routing_settings_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
                {
                    self.routing_settings = None;
                }
            }
            DismissibleOverlay::ModelPicker => self.model_picker = None,
            DismissibleOverlay::WorkspacePicker => self.workspace_picker = None,
            DismissibleOverlay::SessionPicker => self.session_picker = None,
            DismissibleOverlay::Palette => self.palette = None,
            DismissibleOverlay::Search => self.search = None,
            DismissibleOverlay::Help => self.help_visible = false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一屏常见布局：右侧栏、正文、活动条、底部输入框。
    fn app_with_layout() -> App {
        let mut app = App::new(Vec::new(), Language::En);
        app.sidebar_rect = Rect::new(80, 0, 20, 30);
        app.transcript_rect = Rect::new(0, 0, 80, 18);
        app.activity_rect = Rect::new(0, 18, 80, 2);
        app.prompt_rect = Rect::new(0, 20, 80, 8);
        app
    }

    fn app_with_history_panel() -> App {
        let mut app = app_with_layout();
        app.focus = FocusPane::Sidebar;
        app.open_session_picker(uuid::Uuid::nil(), SessionPickerRequest::default());
        app.session_picker_rect = Rect::new(20, 4, 60, 12);
        app
    }

    #[test]
    fn clicking_the_composer_closes_the_panel_and_hands_it_the_focus() {
        let mut app = app_with_history_panel();

        assert!(app.dismiss_overlay_on_outside_click(10, 22));
        assert!(app.session_picker.is_none());
        assert_eq!(app.focus, FocusPane::Prompt);
    }

    #[test]
    fn a_click_inside_the_panel_still_belongs_to_the_panel() {
        let mut app = app_with_history_panel();

        assert!(!app.dismiss_overlay_on_outside_click(30, 6));
        assert!(app.session_picker.is_some());
        assert_eq!(app.focus, FocusPane::Sidebar);
    }

    #[test]
    fn visible_inbox_close_label_is_clickable_without_stopping_task_or_touching_draft() {
        for language in [Language::ZhCn, Language::En, Language::Ja] {
            for width in [100, 24] {
                let mut app = App::new(Vec::new(), language);
                app.begin_turn(true, "running task".into());
                app.input.insert("unsent message");
                app.attention_detail = Some(AttentionItem {
                    id: "running-task".into(),
                    source: AttentionSource::BackgroundShell,
                    title: "Running task".into(),
                    detail: "Turn: 51".into(),
                    status: RuntimeStatus::Working,
                    elapsed_millis: Some(1000),
                });
                let mut terminal =
                    Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
                terminal
                    .draw(|frame| render_attention_detail(frame, &mut app))
                    .unwrap();
                let (kind, hit) = app.overlay_close_hits[0];
                assert_eq!(kind, DismissibleOverlay::AttentionDetail);
                assert_eq!(hit.height, 1);
                assert!(
                    terminal.backend().buffer()[(hit.x, hit.y)]
                        .modifier
                        .contains(Modifier::UNDERLINED)
                );
                assert!(!app.dismiss_overlay_on_close_click(hit.x.saturating_sub(1), hit.y));
                assert!(!app.dismiss_overlay_on_close_click(hit.x, hit.y + 1));
                assert!(app.dismiss_overlay_on_close_click(hit.right() - 1, hit.y));
                assert!(app.attention_detail.is_none());
                assert!(app.running && app.runtime_turn);
                assert_eq!(app.input.text(), "unsent message");
            }
        }
    }

    #[test]
    fn close_label_of_an_underlying_panel_cannot_dismiss_the_top_panel() {
        let mut app = app_with_layout();
        app.help_visible = true;
        app.help_rect = Rect::new(0, 0, 80, 28);
        app.overlay_close_hits
            .push((DismissibleOverlay::Help, Rect::new(10, 0, 10, 1)));
        app.mobile_qr = Some("QR".into());
        app.mobile_qr_rect = Rect::new(0, 0, 30, 10);
        assert!(!app.dismiss_overlay_on_close_click(11, 0));
        assert!(app.help_visible && app.mobile_qr.is_some());
    }

    #[test]
    fn a_panel_that_has_not_been_drawn_yet_has_no_boundary_to_be_outside_of() {
        let mut app = app_with_layout();
        app.open_session_picker(uuid::Uuid::nil(), SessionPickerRequest::default());

        assert!(!app.dismiss_overlay_on_outside_click(10, 22));
        assert!(app.session_picker.is_some());
    }

    #[test]
    fn only_the_topmost_overlay_goes_away() {
        let mut app = app_with_layout();
        app.search = Some(SearchState::default());
        app.search_rect = Rect::new(0, 0, 40, 3);
        app.mobile_qr = Some("▀▄▀".to_owned());
        app.mobile_qr_rect = Rect::new(30, 5, 20, 10);

        assert!(app.dismiss_overlay_on_outside_click(10, 22));
        assert!(app.mobile_qr.is_none());
        assert!(app.search.is_some(), "下面那层不该被同一次点击一起带走");
    }

    /// 审批弹层的 `Esc` 等于「拒绝」，一次手滑不该替命令签字；
    /// 它盖着的时候，下面那层也不许被点掉。
    #[tokio::test]
    async fn an_open_approval_shields_itself_and_whatever_sits_under_it() {
        let mut app = app_with_layout();
        let (sender, receiver) = oneshot::channel();
        app.approval = Some(("Run deploy".to_owned(), false, sender));
        app.approval_rect = Rect::new(20, 6, 40, 9);
        app.help_visible = true;
        app.help_rect = Rect::new(10, 2, 60, 20);
        app.overlay_close_hits
            .push((DismissibleOverlay::Help, Rect::new(12, 2, 10, 1)));

        assert!(!app.dismiss_overlay_on_close_click(13, 2));
        assert!(!app.dismiss_overlay_on_outside_click(10, 22));
        assert!(app.approval.is_some());
        assert!(app.help_visible);
        drop(app);
        assert!(receiver.await.is_err(), "不该替用户做出任何审批决定");
    }

    /// 提问弹层同理：`Esc` 在那里是「取消」。
    #[test]
    fn an_open_question_is_not_dismissed_by_a_stray_click() {
        let mut app = app_with_layout();
        let (sender, _receiver) = oneshot::channel();
        app.question = Some(AskDialog {
            request: UserQuestion {
                question: "Choose".to_owned(),
                options: vec!["A".to_owned(), "B".to_owned()],
                multi_select: false,
            },
            selected: 0,
            checked: vec![false, false],
            answer: PromptEditor::default(),
            sender,
        });
        app.question_rect = Rect::new(20, 6, 40, 10);

        assert!(!app.dismiss_overlay_on_outside_click(10, 22));
        assert!(app.question.is_some());
    }

    /// Diff 审阅有 Commit Preview / 搜索 / 撤销确认等嵌套层，`Esc` 在那里是
    /// 「退一层」；用一次外部点击去模拟只会更糊涂，所以它同样挡住下面那层。
    #[test]
    fn diff_review_keeps_its_own_escape_semantics() {
        let mut app = app_with_layout();
        app.help_visible = true;
        app.help_rect = Rect::new(10, 2, 60, 20);
        app.diff_review = Some(DiffReviewState {
            snapshot: crate::daemon::diff_review::DiffSnapshot {
                id: "snapshot".to_owned(),
                workspace: std::path::PathBuf::from("/tmp/willdeep"),
                head: None,
                files: Vec::new(),
                additions: 0,
                deletions: 0,
                has_conflicts: false,
            },
            selected: 0,
            content: None,
            scroll: 0,
            area: crate::daemon::diff_review::DiffArea::Combined,
            view: DiffViewMode::Unified,
            search: None,
            search_matches: Vec::new(),
            search_selected: 0,
            reviews: BTreeMap::new(),
            confirm_revert: false,
            verifications: Vec::new(),
            attributions: Vec::new(),
            commit_preview: None,
            preview_draft: None,
        });

        assert!(!app.dismiss_overlay_on_outside_click(10, 22));
        assert!(app.help_visible);
    }

    /// 边界不是凭空写在测试里的：真画一遍面板，看画出来的那个框认不认外部点击。
    #[test]
    fn the_boundary_comes_from_the_frame_that_was_actually_drawn() {
        let mut app = app_with_layout();
        app.open_session_picker(uuid::Uuid::nil(), SessionPickerRequest::default());
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_session_picker(frame, &mut app))
            .unwrap();

        let popup = app.session_picker_rect;
        assert!(popup.width > 0 && popup.height > 0, "面板得先有个框");
        assert!(popup.y > 0, "居中的面板上面该留出边距");
        assert!(app.dismiss_overlay_on_outside_click(popup.x, popup.y - 1));
        assert!(app.session_picker.is_none());
    }

    #[test]
    fn clicking_the_sidebar_only_moves_the_focus_there() {
        let mut app = app_with_layout();
        app.help_visible = true;
        app.help_rect = Rect::new(10, 2, 60, 20);

        assert!(app.dismiss_overlay_on_outside_click(85, 4));
        assert!(!app.help_visible);
        assert_eq!(app.focus, FocusPane::Sidebar);
    }
}
