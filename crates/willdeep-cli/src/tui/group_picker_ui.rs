//! 群列表面板：`/groups` 开，方向键或鼠标选中，`Enter` 进入那个群的会话。
//!
//! 交互与 `/history` 一样：输入即过滤，↑/↓ 选，`Enter` 进入，`Esc` 退出。不一样的是数据来源：
//! 群记录在本机的 `federation.json`，不走 Runtime 查询；而且群会话落在 `[federation] workspace`，
//! 与当前工作区不必相同，所以进入时允许跨工作区切换。还没有会话的群留在列表里看得见，
//! 但进不去——群会话是第一条入站消息到来时才建的。

use super::*;

/// 一行要画的东西。联系人名称在打开时就解好，面板不再去读联系人表。
pub(super) struct GroupPickerRow {
    pub(super) id: uuid::Uuid,
    pub(super) name: String,
    /// 这个群落在本机的哪条会话上。没有就是还没人说过话。
    pub(super) session_id: Option<uuid::Uuid>,
    pub(super) member_names: Vec<String>,
    pub(super) paused: bool,
    pub(super) auto_turns: u32,
}

#[derive(Default)]
pub(super) struct GroupPickerState {
    pub(super) editor: PromptEditor,
    pub(super) rows: Vec<GroupPickerRow>,
    /// 命中当前关键词的下标，按匹配度排序。
    pub(super) filtered: Vec<usize>,
    pub(super) selected: usize,
    /// 当前打开的会话，用来在列表里标出「当前」。
    pub(super) current_session: uuid::Uuid,
}

pub(super) enum GroupPickerAction {
    None,
    Close,
    /// 进入这个群的会话。
    Open(uuid::Uuid),
    /// 这个群还没有会话，不能进入。携带群名，好把原因说给用户。
    Unavailable(String),
}

impl App {
    pub(super) fn open_group_picker(
        &mut self,
        rows: Vec<GroupPickerRow>,
        current_session: uuid::Uuid,
    ) {
        self.palette = None;
        self.search = None;
        self.session_picker = None;
        self.workspace_picker = None;
        self.model_picker = None;
        let mut picker = GroupPickerState {
            rows,
            current_session,
            ..Default::default()
        };
        picker.filtered = (0..picker.rows.len()).collect();
        // 光标落在当前群上：列表一打开，人先看见自己在哪儿。
        picker.selected = picker
            .filtered
            .iter()
            .position(|index| picker.rows[*index].session_id == Some(picker.current_session))
            .unwrap_or(0);
        self.group_picker = Some(picker);
    }

    pub(super) fn refresh_group_picker_matches(&mut self) {
        let Some(picker) = self.group_picker.as_mut() else {
            return;
        };
        let query = picker.editor.text().trim().to_lowercase();
        // 群名、成员名、ID 都参与匹配：人记得住的是群名或成员，排查时手上往往只有一个 ID。
        let mut ranked = picker
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                let haystack = format!("{} {} {}", row.name, row.member_names.join(" "), row.id)
                    .to_lowercase();
                fuzzy_score(&query, &haystack).map(|score| (score, index))
            })
            .collect::<Vec<_>>();
        ranked.sort_by_key(|(score, index)| (*score, *index));
        picker.filtered = ranked.into_iter().map(|(_, index)| index).collect();
        picker.selected = 0;
    }

    pub(super) fn handle_group_picker_key(&mut self, key: KeyEvent) -> GroupPickerAction {
        let Some(picker) = self.group_picker.as_mut() else {
            return GroupPickerAction::None;
        };
        if key.code == KeyCode::Esc {
            return GroupPickerAction::Close;
        }
        let mut query_changed = false;
        match key.code {
            KeyCode::Up | KeyCode::BackTab if !picker.filtered.is_empty() => {
                picker.selected = picker
                    .selected
                    .checked_sub(1)
                    .unwrap_or(picker.filtered.len() - 1);
            }
            KeyCode::Down | KeyCode::Tab if !picker.filtered.is_empty() => {
                picker.selected = (picker.selected + 1) % picker.filtered.len();
            }
            KeyCode::PageUp if !picker.filtered.is_empty() => {
                picker.selected = picker.selected.saturating_sub(10);
            }
            KeyCode::PageDown if !picker.filtered.is_empty() => {
                picker.selected = (picker.selected + 10).min(picker.filtered.len() - 1);
            }
            KeyCode::Enter if !picker.filtered.is_empty() => {
                let index = picker.filtered[picker.selected.min(picker.filtered.len() - 1)];
                return group_picker_choice(&picker.rows[index]);
            }
            KeyCode::Left => picker.editor.left(),
            KeyCode::Right => picker.editor.right(),
            KeyCode::Home => picker.editor.home(),
            KeyCode::End => picker.editor.end(),
            KeyCode::Backspace => {
                picker.editor.backspace();
                query_changed = true;
            }
            KeyCode::Delete => {
                picker.editor.delete();
                query_changed = true;
            }
            KeyCode::Char(character)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::ALT,
                ) =>
            {
                picker.editor.insert(&character.to_string());
                query_changed = true;
            }
            _ => {}
        }
        if query_changed {
            self.refresh_group_picker_matches();
        }
        GroupPickerAction::None
    }

    pub(super) fn group_picker_scroll(&mut self, delta: isize) {
        let Some(picker) = self.group_picker.as_mut() else {
            return;
        };
        if picker.filtered.is_empty() {
            return;
        }
        if delta < 0 {
            picker.selected = picker.selected.saturating_sub(1);
        } else {
            picker.selected = (picker.selected + 1).min(picker.filtered.len() - 1);
        }
    }

    /// 点中某一行就是选中它；点在输入行上只挪光标。
    pub(super) fn activate_group_picker_at(&mut self, x: u16, y: u16) -> GroupPickerAction {
        if !self.group_picker_rect.contains((x, y).into()) {
            return GroupPickerAction::None;
        }
        if y == self.group_picker_rect.y.saturating_add(1) {
            if let Some(picker) = self.group_picker.as_mut() {
                picker.editor.set_cursor_visual(
                    0,
                    x.saturating_sub(self.group_picker_rect.x + 3) as usize,
                    self.group_picker_rect.width.saturating_sub(4).max(1) as usize,
                );
            }
            return GroupPickerAction::None;
        }
        let Some(position) = self
            .group_picker_hits
            .iter()
            .find_map(|(row, position)| (*row == y).then_some(*position))
        else {
            return GroupPickerAction::None;
        };
        let Some(picker) = self.group_picker.as_mut() else {
            return GroupPickerAction::None;
        };
        picker.selected = position.min(picker.filtered.len().saturating_sub(1));
        let Some(index) = picker.filtered.get(picker.selected).copied() else {
            return GroupPickerAction::None;
        };
        picker
            .rows
            .get(index)
            .map(group_picker_choice)
            .unwrap_or(GroupPickerAction::None)
    }
}

/// 选中一行的结果：有会话才能进入，没有就把原因交给调用方告诉用户。
fn group_picker_choice(row: &GroupPickerRow) -> GroupPickerAction {
    match row.session_id {
        Some(session_id) => GroupPickerAction::Open(session_id),
        None => GroupPickerAction::Unavailable(row.name.clone()),
    }
}

/// 把本机的群记录变成面板的行。成员按 UUID 对到联系人名，对不上的留短 ID。
pub(super) fn group_picker_rows(home: &std::path::Path) -> Vec<GroupPickerRow> {
    let Ok(store) = crate::federation::load(home) else {
        return Vec::new();
    };
    store
        .groups
        .into_iter()
        .map(|group| {
            let member_names = group
                .members
                .iter()
                .map(|member| {
                    store
                        .contacts
                        .iter()
                        .find(|contact| contact.id == *member)
                        .map(|contact| contact.name.clone())
                        .unwrap_or_else(|| crate::identity::short_id(*member))
                })
                .collect();
            GroupPickerRow {
                id: group.id,
                name: group.name,
                session_id: group.session_id,
                member_names,
                paused: group.paused,
                auto_turns: group.auto_turns,
            }
        })
        .collect()
}

pub(super) fn render_group_picker(f: &mut ratatui::Frame<'_>, app: &mut App) {
    app.group_picker_rect = Rect::default();
    app.group_picker_hits.clear();
    let Some(picker) = &app.group_picker else {
        return;
    };
    let width = f.area().width.min(100);
    let desired_rows = if picker.filtered.is_empty() {
        7
    } else {
        picker.filtered.len().min(18) as u16 + 3
    };
    let height = f.area().height.min(desired_rows.max(7));
    let popup = centered_rect(width, height, f.area());
    app.group_picker_rect = popup;
    let visible = popup.height.saturating_sub(3).max(1) as usize;
    let start = picker.selected.saturating_sub(visible - 1);
    let mut lines = vec![Line::styled(
        format!("› {}", picker.editor.text()),
        Style::default().fg(Color::Yellow),
    )];
    if picker.filtered.is_empty() {
        lines.push(Line::styled(
            app.language.text(
                "没有匹配的群聊",
                "No groups match",
                "一致するグループはありません",
            ),
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        for (position, index) in picker.filtered.iter().enumerate().skip(start).take(visible) {
            let row = &picker.rows[*index];
            let selected = position == picker.selected;
            let current = row.session_id == Some(picker.current_session);
            let style = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD)
            } else if current {
                Style::default()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::styled(
                group_picker_line(row, current, app.language, selected),
                style,
            ));
            app.group_picker_hits
                .push((popup.y + 2 + (position - start) as u16, position));
        }
    }
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(format!(
                    "{} · {}/{} · ↑/↓/PgUp/PgDn · Enter · Esc",
                    app.language.text("群聊", "Groups", "グループ"),
                    if picker.filtered.is_empty() {
                        0
                    } else {
                        picker.selected + 1
                    },
                    picker.filtered.len()
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::LightCyan)),
        ),
        popup,
    );
    if popup.width > 4 && popup.height > 2 {
        // The filter is a clipped single line, so calculate the insertion column without
        // wrapping and clamp only after locating it (including wide characters).
        let (_, column) = picker.editor.cursor_visual(usize::MAX);
        let cursor = column.min(popup.width.saturating_sub(5) as usize) as u16;
        f.set_cursor_position((popup.x + 3 + cursor, popup.y + 1));
    }
}

/// 一行的排版：群名在最前，成员其次，状态收尾。ID 不进这一行——面板里没人再需要它。
pub(super) fn group_picker_line(
    row: &GroupPickerRow,
    current: bool,
    language: Language,
    selected: bool,
) -> String {
    let marker = if selected { "▶" } else { " " };
    let current = if current {
        format!(" [{}]", language.text("当前", "current", "現在"))
    } else {
        String::new()
    };
    let members = if row.member_names.is_empty() {
        language
            .text("没有成员", "no members", "メンバーなし")
            .to_owned()
    } else {
        row.member_names.join(", ")
    };
    let state = if row.session_id.is_none() {
        language
            .text("还没有会话", "no conversation yet", "会話はまだありません")
            .to_owned()
    } else if row.paused {
        language.text("已暂停", "paused", "一時停止").to_owned()
    } else {
        format!(
            "{} {}",
            language.text("自动回复", "auto replies", "自動返信"),
            row.auto_turns
        )
    };
    format!("{marker} #{}{current} · {members} · {state}", row.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, session: Option<uuid::Uuid>, members: &[&str]) -> GroupPickerRow {
        GroupPickerRow {
            id: uuid::Uuid::new_v4(),
            name: name.to_owned(),
            session_id: session,
            member_names: members.iter().map(|name| (*name).to_owned()).collect(),
            paused: false,
            auto_turns: 2,
        }
    }

    #[test]
    fn typing_filters_and_enter_opens_only_a_group_that_has_a_session() {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        let current = uuid::Uuid::new_v4();
        let quiet = uuid::Uuid::new_v4();
        app.open_group_picker(
            vec![
                row("室内", Some(current), &["alex", "mira"]),
                row("外出", Some(quiet), &["lin"]),
                row("还没说过话", None, &["alex"]),
            ],
            current,
        );
        assert_eq!(app.group_picker.as_ref().unwrap().selected, 0);

        app.handle_group_picker_key(KeyEvent::new(KeyCode::Char('外'), KeyModifiers::NONE));
        let picker = app.group_picker.as_ref().unwrap();
        assert_eq!(picker.filtered.len(), 1);
        assert_eq!(picker.rows[picker.filtered[0]].name, "外出");

        let GroupPickerAction::Open(session) =
            app.handle_group_picker_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("a group with a session should open it");
        };
        assert_eq!(session, quiet);

        app.handle_group_picker_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        for _ in 0..2 {
            app.handle_group_picker_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        let GroupPickerAction::Unavailable(name) =
            app.handle_group_picker_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!("a group without a session cannot be entered");
        };
        assert_eq!(name, "还没说过话");
    }

    #[test]
    fn the_line_names_members_and_marks_the_current_group() {
        let line = group_picker_line(
            &row("室内", Some(uuid::Uuid::new_v4()), &["alex", "mira"]),
            true,
            Language::ZhCn,
            true,
        );
        assert!(line.contains("#室内"));
        assert!(line.contains("[当前]"));
        assert!(line.contains("alex, mira"));
        assert!(line.contains("自动回复 2"));

        let mut silent = row("空", None, &[]);
        silent.paused = true;
        let line = group_picker_line(&silent, false, Language::En, false);
        assert!(line.contains("no conversation yet"));
        assert!(
            !line.contains("paused"),
            "no session matters more than paused"
        );
    }

    #[test]
    fn rendered_cursor_follows_home_left_wide_text_and_clips_long_queries() {
        let mut app = App::new(Vec::new(), Language::En);
        app.open_group_picker(Vec::new(), uuid::Uuid::nil());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 12)).unwrap();
        app.group_picker.as_mut().unwrap().editor.insert("中文ab");
        for (key, column) in [
            (KeyCode::Left, 5),
            (KeyCode::Home, 0),
            (KeyCode::Right, 2),
            (KeyCode::End, 6),
        ] {
            app.handle_group_picker_key(KeyEvent::new(key, KeyModifiers::NONE));
            terminal
                .draw(|frame| render_group_picker(frame, &mut app))
                .unwrap();
            let cursor = terminal.get_cursor_position().unwrap();
            assert_eq!(cursor.x, app.group_picker_rect.x + 3 + column, "{key:?}");
            assert_eq!(cursor.y, app.group_picker_rect.y + 1);
        }
        app.group_picker
            .as_mut()
            .unwrap()
            .editor
            .insert(&"中".repeat(50));
        terminal
            .draw(|frame| render_group_picker(frame, &mut app))
            .unwrap();
        assert_eq!(
            terminal.get_cursor_position().unwrap().x,
            app.group_picker_rect.right() - 2
        );
        app.handle_group_picker_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        terminal
            .draw(|frame| render_group_picker(frame, &mut app))
            .unwrap();
        assert_eq!(
            terminal.get_cursor_position().unwrap().x,
            app.group_picker_rect.x + 3
        );
    }

    #[test]
    fn escape_closes_and_clicking_a_row_selects_it() {
        let mut app = App::new(Vec::new(), Language::En);
        let session = uuid::Uuid::new_v4();
        app.open_group_picker(
            vec![row("room", Some(session), &["alex"])],
            uuid::Uuid::nil(),
        );
        assert!(matches!(
            app.handle_group_picker_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            GroupPickerAction::Close
        ));

        app.group_picker_rect = Rect::new(10, 4, 60, 10);
        app.group_picker_hits.push((6, 0));
        let GroupPickerAction::Open(opened) = app.activate_group_picker_at(12, 6) else {
            panic!("clicking a row should open its session");
        };
        assert_eq!(opened, session);
        assert!(matches!(
            app.activate_group_picker_at(12, 5),
            GroupPickerAction::None
        ));
    }
}
