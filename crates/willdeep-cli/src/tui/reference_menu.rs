use super::*;

impl App {
    pub(super) fn reference_matches(&self) -> Vec<crate::composer_references::Reference> {
        let Some((_, query)) = self.input.marker_query('@') else {
            return Vec::new();
        };
        let query = query.to_lowercase();
        let entries = self.references.as_deref().unwrap_or_default();
        ["mcp", "agent", "file"]
            .into_iter()
            .flat_map(|kind| {
                let query = query.as_str();
                entries
                    .iter()
                    .filter(move |entry| {
                        entry.kind == kind
                            && (entry.name.to_lowercase().contains(query)
                                || entry.value.to_lowercase().contains(query))
                    })
                    .take(if kind == "agent" {
                        willdeep_core::subagent::PUBLIC_SUBAGENT_IDS.len()
                    } else {
                        3
                    })
                    .cloned()
            })
            .collect()
    }

    pub(super) fn handle_reference_key(&mut self, key: KeyEvent) -> bool {
        if self.reference_dismissed || self.input.marker_query('@').is_none() {
            return false;
        }
        let matches = self.reference_matches();
        match key.code {
            KeyCode::Esc => {
                self.reference_dismissed = true;
                true
            }
            KeyCode::Up | KeyCode::Down if !matches.is_empty() => {
                self.reference_selected = if key.code == KeyCode::Up {
                    (self.reference_selected + matches.len() - 1) % matches.len()
                } else {
                    (self.reference_selected + 1) % matches.len()
                };
                true
            }
            KeyCode::Tab | KeyCode::Enter
                if !key
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            {
                if let Some(entry) =
                    matches.get(self.reference_selected.min(matches.len().saturating_sub(1)))
                {
                    let (start, _) = self.input.marker_query('@').expect("reference query");
                    self.input
                        .replace_before_cursor(start, &format!("{} ", entry.value));
                    self.reference_dismissed = true;
                }
                true
            }
            _ => false,
        }
    }
}

pub(super) fn draw(frame: &mut ratatui::Frame, app: &mut App, prompt: Rect) {
    app.reference_rect = Rect::default();
    app.reference_hits.clear();
    if app.reference_dismissed || app.input.marker_query('@').is_none() {
        return;
    }
    let matches = app.reference_matches();
    let popup = Rect {
        x: prompt.x,
        y: prompt.y.saturating_sub(matches.len() as u16 + 2),
        width: prompt.width.min(90),
        height: (matches.len() as u16 + 2).max(3).min(prompt.y),
    };
    if popup.height < 3 {
        return;
    }
    app.reference_rect = popup;
    let mut lines = Vec::new();
    for (index, entry) in matches.iter().enumerate() {
        let category = match entry.kind {
            "mcp" => "MCP",
            "agent" => app.language.text("智能体", "Agent", "エージェント"),
            _ => app.language.text("文件", "File", "ファイル"),
        };
        lines.push(Line::styled(
            format!(
                "{} {category} · {}",
                if index == app.reference_selected {
                    "▶"
                } else {
                    " "
                },
                entry.name
            ),
            Style::default().fg(if index == app.reference_selected {
                Color::LightCyan
            } else {
                Color::White
            }),
        ));
    }
    if lines.is_empty() {
        lines.push(Line::from(app.language.text(
            "没有匹配项",
            "No matches",
            "一致なし",
        )));
    }
    let visible_rows = popup.height.saturating_sub(2) as usize;
    let offset = app
        .reference_selected
        .saturating_sub(visible_rows.saturating_sub(1));
    app.reference_hits = (offset..matches.len())
        .take(visible_rows)
        .enumerate()
        .map(|(row, index)| (popup.y + 1 + row as u16, index))
        .collect();
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).scroll((offset as u16, 0)).block(
            Block::default()
                .title(app.language.text(
                    "引用 · ↑/↓ 选择 · Enter/Tab 插入 · Esc 关闭",
                    "References · ↑/↓ select · Enter/Tab insert · Esc close",
                    "参照 · ↑/↓ 選択 · Enter/Tab 挿入 · Esc 閉じる",
                ))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.references = Some(
            [
                ("mcp", "search", "@mcp:search"),
                ("agent", "generalist", "@agent:generalist"),
                ("file", "src/中文 file.rs", "@file:\"src/中文 file.rs\""),
            ]
            .into_iter()
            .map(
                |(kind, name, value)| crate::composer_references::Reference {
                    kind,
                    name: name.into(),
                    value: value.into(),
                },
            )
            .collect(),
        );
        app
    }

    #[test]
    fn reference_keys_select_in_order_and_preserve_suffix() {
        let mut app = app();
        app.input.insert("看 @ 后文");
        for _ in 0..3 {
            app.input.left();
        }
        assert_eq!(
            app.reference_matches()
                .iter()
                .map(|e| e.kind)
                .collect::<Vec<_>>(),
            ["mcp", "agent", "file"]
        );
        for _ in 0..2 {
            assert!(app.handle_reference_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
        }
        assert!(app.handle_reference_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert_eq!(app.input.text(), "看 @file:\"src/中文 file.rs\"  后文");
        assert!(app.transcript.is_empty());
    }

    #[test]
    fn references_filter_unicode_and_escape_dismisses() {
        let mut app = app();
        app.input.insert("@src/中文");
        assert_eq!(app.reference_matches().len(), 1);
        assert!(app.handle_reference_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(!app.handle_reference_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert_eq!(app.input.text(), "@src/中文");
    }
}
