use super::*;
use willdeep_core::types::{MessageSource, Role};

const HISTORY_LIMIT: usize = 100;
const VISIBLE_ROWS: usize = 6;

pub(super) struct PromptHistory {
    entries: Vec<String>,
    selected: usize,
}

impl App {
    pub(super) fn handle_prompt_history_key(
        &mut self,
        key: KeyEvent,
        messages: &[Message],
    ) -> bool {
        if self.focus != FocusPane::Prompt {
            self.prompt_history = None;
            return false;
        }
        if self.prompt_history.is_none() {
            if key.code != KeyCode::Up || !key.modifiers.is_empty() || !self.input.text().is_empty()
            {
                return false;
            }
            let mut seen = BTreeSet::new();
            let mut entries: Vec<String> = messages
                .iter()
                .rev()
                .filter(|message| {
                    message.role == Role::User
                        && message.source != Some(MessageSource::HostInstruction)
                })
                .map(|message| willdeep_core::conversation::visible_user_text(&message.content))
                .filter(|text| !text.trim().is_empty() && seen.insert((*text).to_owned()))
                .take(HISTORY_LIMIT)
                .map(str::to_owned)
                .collect();
            entries.reverse();
            let selected = entries.len().saturating_sub(1);
            self.prompt_history = Some(PromptHistory { entries, selected });
            return true;
        }
        if !key.modifiers.is_empty() {
            self.prompt_history = None;
            return false;
        }
        let history = self.prompt_history.as_mut().expect("open history");
        match key.code {
            KeyCode::Up => history.selected = history.selected.saturating_sub(1),
            KeyCode::Down => {
                history.selected =
                    (history.selected + 1).min(history.entries.len().saturating_sub(1))
            }
            KeyCode::Enter | KeyCode::Tab => {
                let text = history.entries.get(history.selected).cloned();
                self.prompt_history = None;
                if let Some(text) = text {
                    self.edit_input(|input| {
                        input.take();
                        input.insert(&text);
                    });
                }
            }
            KeyCode::Esc => self.prompt_history = None,
            _ => {
                self.prompt_history = None;
                return false;
            }
        }
        true
    }
}

pub(super) fn draw(frame: &mut ratatui::Frame, app: &App, prompt: Rect) {
    let Some(history) = &app.prompt_history else {
        return;
    };
    if app.focus != FocusPane::Prompt || !app.input.text().is_empty() {
        return;
    }
    let height = (history.entries.len().clamp(1, VISIBLE_ROWS) as u16 + 2).min(prompt.y);
    if height < 3 {
        return;
    }
    let popup = Rect::new(prompt.x, prompt.y - height, prompt.width, height);
    let rows = height.saturating_sub(2) as usize;
    let offset = history.selected.saturating_sub(rows.saturating_sub(1));
    let mut lines: Vec<Line> = history
        .entries
        .iter()
        .enumerate()
        .skip(offset)
        .take(rows)
        .map(|(index, text)| {
            Line::styled(
                format!(
                    "{} {}",
                    if index == history.selected {
                        "▶"
                    } else {
                        " "
                    },
                    text.split_whitespace().collect::<Vec<_>>().join(" ")
                ),
                Style::default().fg(if index == history.selected {
                    Color::LightCyan
                } else {
                    Color::White
                }),
            )
        })
        .collect();
    if lines.is_empty() {
        lines.push(Line::from(app.language.text(
            "当前会话没有历史输入",
            "No prompt history in this Session",
            "このセッションには入力履歴がありません",
        )));
    }
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(app.language.text(
                    "历史输入 · ↑/↓ 选择 · Enter/Tab 回填 · Esc 关闭",
                    "Prompt history · ↑/↓ select · Enter/Tab insert · Esc close",
                    "入力履歴 · ↑/↓ 選択 · Enter/Tab 挿入 · Esc 閉じる",
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        popup,
    );
}
