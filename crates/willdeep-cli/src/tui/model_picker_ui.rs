use super::*;

const TWO_PANE_MIN_WIDTH: u16 = 76;
const POPUP_MAX_WIDTH: u16 = 104;
const POPUP_MAX_HEIGHT: u16 = 24;
const PROVIDER_PANE_WIDTH: u16 = 28;

pub(super) fn render(f: &mut ratatui::Frame<'_>, app: &mut App) {
    app.model_picker_hits.clear();
    app.model_provider_hits.clear();
    app.model_provider_rect = Rect::default();
    app.model_list_rect = Rect::default();
    let Some(picker) = app.model_picker.as_mut() else {
        return;
    };
    let popup = centered_rect(
        f.area().width.min(POPUP_MAX_WIDTH),
        f.area().height.min(POPUP_MAX_HEIGHT),
        f.area(),
    );
    app.model_picker_rect = popup;
    picker.wide = popup.width >= TWO_PANE_MIN_WIDTH;
    let language = app.language;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(language.text("选择模型", "Select model", "モデルを選択"))
        .border_style(Style::default().fg(Color::LightCyan));
    let inner = block.inner(popup);
    f.render_widget(Clear, popup);
    f.render_widget(block, popup);
    if inner.height < 3 || inner.width < 4 {
        return;
    }
    let header = Rect { height: 1, ..inner };
    let footer = Rect {
        y: inner.bottom() - 1,
        height: 1,
        ..inner
    };
    let body = Rect {
        y: inner.y + 1,
        height: inner.height - 2,
        ..inner
    };
    let active = picker
        .active_profile
        .as_deref()
        .unwrap_or(&picker.provider_label);
    let browsed = picker.profile.as_deref().unwrap_or(active);
    let heading = if picker.wide {
        format!(
            "{}: {active} / {}",
            language.text("当前", "Current", "現在"),
            picker.current_model
        )
    } else {
        format!(
            "Provider › {browsed} › {}",
            language.text(
                "模型 · 点击返回",
                "Models · click to go back",
                "モデル · 戻る"
            )
        )
    };
    f.render_widget(
        Paragraph::new(heading).style(Style::default().fg(Color::LightCyan)),
        header,
    );
    let help = if inner.width >= TWO_PANE_MIN_WIDTH {
        language.text(
            "↑↓ 选择 · Tab 切换栏 · Enter 确认 · F5 刷新 · Esc 取消",
            "↑↓ move · Tab switch panes · Enter choose · F5 refresh · Esc cancel",
            "↑↓ 選択 · Tab 列切替 · Enter 決定 · F5 更新 · Esc 終了",
        )
    } else {
        language.text(
            "Tab 切换 · ↑↓ · Enter · Esc 返回",
            "Tab switch · ↑↓ · Enter · Esc back",
            "Tab 切替 · ↑↓ · Enter · Esc 戻る",
        )
    };
    f.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::Gray)),
        footer,
    );
    if picker.wide {
        let panes =
            Layout::horizontal([Constraint::Length(PROVIDER_PANE_WIDTH), Constraint::Min(1)])
                .split(body);
        app.model_provider_rect = panes[0];
        app.model_list_rect = panes[1];
    } else if picker.choosing_provider {
        app.model_provider_rect = body;
    } else {
        app.model_list_rect = body;
    }
    if app.model_provider_rect.width > 0 {
        render_providers(
            f,
            picker,
            language,
            app.model_provider_rect,
            &mut app.model_provider_hits,
        );
    }
    if app.model_list_rect.width > 0 {
        render_models(
            f,
            picker,
            language,
            app.model_list_rect,
            &mut app.model_picker_hits,
        );
    }
    let pane = if picker.choosing_provider {
        app.model_provider_rect
    } else {
        app.model_list_rect
    };
    if pane.width > 4 && pane.height > 2 {
        let (_, col) = picker
            .focused_editor()
            .cursor_visual(pane.width.saturating_sub(4) as usize);
        f.set_cursor_position((pane.x + 3 + (col as u16).min(pane.width - 4), pane.y + 1));
    }
}

fn pane_block(title: String, focused: bool) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            Color::LightCyan
        } else {
            Color::Gray
        }))
}

fn row_style(selected: bool, focused: bool, current: bool) -> Style {
    if selected && focused {
        Style::default()
            .fg(Color::Black)
            .bg(Color::LightCyan)
            .add_modifier(Modifier::BOLD)
    } else if selected {
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD)
    } else if current {
        Style::default()
            .fg(Color::LightGreen)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

fn render_providers(
    f: &mut ratatui::Frame<'_>,
    picker: &ModelPickerState,
    language: Language,
    pane: Rect,
    hits: &mut Vec<(Rect, usize)>,
) {
    let block = pane_block(
        format!(" Provider · {} ", picker.profiles.len()),
        picker.choosing_provider,
    );
    let inner = block.inner(pane);
    f.render_widget(block, pane);
    if inner.is_empty() {
        return;
    }
    f.render_widget(
        Paragraph::new(format!("› {}", picker.provider_editor.text())),
        Rect { height: 1, ..inner },
    );
    let visible = inner.height.saturating_sub(1) as usize;
    let start = picker
        .provider_selected
        .saturating_sub(visible.saturating_sub(1));
    for (position, index) in picker
        .provider_filtered
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
    {
        let name = &picker.profiles[*index];
        let selected = position == picker.provider_selected;
        let current = Some(name) == picker.active_profile.as_ref();
        let row = Rect {
            y: inner.y + 1 + (position - start) as u16,
            height: 1,
            ..inner
        };
        let label = format!(
            "{} {name}{}",
            if selected { "▶" } else { " " },
            if current { " ●" } else { "" }
        );
        f.render_widget(
            Paragraph::new(label).style(row_style(selected, picker.choosing_provider, current)),
            row,
        );
        hits.push((row, position));
    }
    if picker.provider_filtered.is_empty() && visible > 0 {
        f.render_widget(
            Paragraph::new(language.text(
                "没有匹配的 Provider",
                "No Providers match",
                "一致する Provider なし",
            )),
            Rect {
                y: inner.y + 1,
                height: 1,
                ..inner
            },
        );
    }
}

fn render_models(
    f: &mut ratatui::Frame<'_>,
    picker: &ModelPickerState,
    language: Language,
    pane: Rect,
    hits: &mut Vec<(Rect, usize)>,
) {
    let browsed = picker.profile.as_deref().unwrap_or(&picker.provider_label);
    let title = format!(
        " {} · {browsed} · {} ",
        language.text("模型", "Models", "モデル"),
        picker.filtered.len()
    );
    let block = pane_block(title, !picker.choosing_provider);
    let inner = block.inner(pane);
    f.render_widget(block, pane);
    if inner.is_empty() {
        return;
    }
    f.render_widget(
        Paragraph::new(format!("› {}", picker.editor.text())),
        Rect { height: 1, ..inner },
    );
    let mut list = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..inner
    };
    if list.height > 0 && (picker.loading || picker.error.is_some()) {
        let status = match &picker.error {
            Some(error) => format!(
                "{}: {error}",
                language.text(
                    "获取失败 · F5 重试",
                    "Lookup failed · F5 retry",
                    "取得失敗 · F5 再試行"
                )
            ),
            None => language
                .text("正在刷新模型…", "Refreshing models…", "モデルを更新中…")
                .to_owned(),
        };
        f.render_widget(
            Paragraph::new(status).style(Style::default().fg(Color::LightCyan)),
            Rect { height: 1, ..list },
        );
        list.y += 1;
        list.height -= 1;
    }
    let visible = list.height as usize;
    let start = picker.selected.saturating_sub(visible.saturating_sub(1));
    for (position, index) in picker.filtered.iter().enumerate().skip(start).take(visible) {
        let model = &picker.models[*index];
        let selected = position == picker.selected;
        let current = picker.profile == picker.active_profile && model == &picker.current_model;
        let row = Rect {
            y: list.y + (position - start) as u16,
            height: 1,
            ..list
        };
        let label = format!(
            "{} {model}{}",
            if selected { "▶" } else { " " },
            if current {
                language.text(" · 当前", " · current", " · 現在")
            } else {
                ""
            }
        );
        f.render_widget(
            Paragraph::new(label).style(row_style(selected, !picker.choosing_provider, current)),
            row,
        );
        hits.push((row, position));
    }
    if picker.filtered.is_empty() && list.height > 0 && !picker.loading {
        f.render_widget(
            Paragraph::new(language.text(
                "没有匹配的模型；可直接输入模型名",
                "No matches; type a model name",
                "一致なし。モデル名を入力できます",
            )),
            Rect { height: 1, ..list },
        );
    }
}
