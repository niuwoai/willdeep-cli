use super::*;

const MAX_MODEL_NAME_BYTES: usize = 256;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ModelCommand {
    List,
    Switch(String),
}

#[derive(Default)]
pub(super) struct ModelPickerState {
    pub(super) profile: Option<String>,
    pub(super) active_profile: Option<String>,
    pub(super) provider_label: String,
    pub(super) profiles: Vec<String>,
    pub(super) choosing_provider: bool,
    pub(super) request_id: uuid::Uuid,
    pub(super) editor: PromptEditor,
    pub(super) models: Vec<String>,
    pub(super) filtered: Vec<usize>,
    pub(super) selected: usize,
    pub(super) current_model: String,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
}

pub(super) enum ModelPickerAction {
    None,
    Close,
    Select(String),
    Provider(Option<String>),
}

impl App {
    pub(super) fn open_model_picker(&mut self, current_model: String) {
        self.palette = None;
        self.search = None;
        self.session_picker = None;
        self.model_picker = Some(ModelPickerState {
            current_model,
            loading: true,
            ..Default::default()
        });
    }

    pub(super) fn set_model_picker_result(
        &mut self,
        result: std::result::Result<Vec<String>, String>,
    ) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.loading = false;
        match result {
            Ok(models) => {
                picker.models = models;
                picker.error = None;
            }
            Err(error) => {
                picker.models.clear();
                picker.error = Some(error);
            }
        }
        self.refresh_model_picker_matches();
    }

    pub(super) fn refresh_model_picker_matches(&mut self) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        let query = picker.editor.text().trim().to_lowercase();
        let mut ranked = picker
            .models
            .iter()
            .enumerate()
            .filter_map(|(index, model)| {
                fuzzy_score(&query, &model.to_lowercase()).map(|score| (score, index))
            })
            .collect::<Vec<_>>();
        ranked.sort_by_key(|(score, index)| (*score, *index));
        picker.filtered = ranked.into_iter().map(|(_, index)| index).collect();
        picker.selected = 0;
    }

    pub(super) fn handle_model_picker_key(&mut self, key: KeyEvent) -> ModelPickerAction {
        let Some(picker) = self.model_picker.as_mut() else {
            return ModelPickerAction::None;
        };
        if key.code == KeyCode::Esc {
            return ModelPickerAction::Close;
        }
        if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return ModelPickerAction::Provider(None);
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
                let model_index = picker.filtered[picker.selected.min(picker.filtered.len() - 1)];
                return if picker.choosing_provider {
                    ModelPickerAction::Provider(Some(picker.models[model_index].clone()))
                } else {
                    ModelPickerAction::Select(picker.models[model_index].clone())
                };
            }
            KeyCode::Enter
                if !picker.choosing_provider && !picker.editor.is_empty() && !picker.loading =>
            {
                return ModelPickerAction::Select(picker.editor.text().trim().to_owned());
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
            self.refresh_model_picker_matches();
        }
        ModelPickerAction::None
    }

    pub(super) fn model_picker_scroll(&mut self, delta: isize) {
        let Some(picker) = self.model_picker.as_mut() else {
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

    pub(super) fn activate_model_picker_at(&mut self, x: u16, y: u16) -> Option<ModelPickerAction> {
        if !self.model_picker_rect.contains((x, y).into()) {
            return None;
        }
        if y == self.model_picker_rect.y.saturating_add(1) {
            return Some(ModelPickerAction::Provider(None));
        }
        if y == self.model_picker_rect.y.saturating_add(2) {
            if let Some(picker) = self.model_picker.as_mut() {
                picker.editor.set_cursor_visual(
                    0,
                    x.saturating_sub(self.model_picker_rect.x + 3) as usize,
                    self.model_picker_rect.width.saturating_sub(4).max(1) as usize,
                );
            }
            return None;
        }
        let position = self
            .model_picker_hits
            .iter()
            .find_map(|(row, position)| (*row == y).then_some(*position))?;
        let picker = self.model_picker.as_mut()?;
        picker.selected = position.min(picker.filtered.len().saturating_sub(1));
        let model_index = *picker.filtered.get(picker.selected)?;
        picker.models.get(model_index).cloned().map(|value| {
            if picker.choosing_provider {
                ModelPickerAction::Provider(Some(value))
            } else {
                ModelPickerAction::Select(value)
            }
        })
    }
}

pub(super) fn render_model_picker(f: &mut ratatui::Frame<'_>, app: &mut App) {
    app.model_picker_rect = Rect::default();
    app.model_picker_hits.clear();
    let Some(picker) = &app.model_picker else {
        return;
    };
    let width = f.area().width.min(100);
    let desired_rows = if picker.loading || picker.error.is_some() || picker.filtered.is_empty() {
        8
    } else {
        picker.filtered.len().min(18) as u16 + 4
    };
    let height = f.area().height.min(desired_rows.max(7));
    let popup = centered_rect(width, height, f.area());
    app.model_picker_rect = popup;
    let visible = popup.height.saturating_sub(4).max(1) as usize;
    let start = picker.selected.saturating_sub(visible - 1);
    let provider = picker.active_profile.as_deref().unwrap_or_else(|| {
        if picker.provider_label.is_empty() {
            app.language
                .text("启动配置", "Startup configuration", "起動設定")
        } else {
            &picker.provider_label
        }
    });
    let browsing = picker.profile.as_deref().unwrap_or(provider);
    let mut lines = vec![
        Line::styled(
            format!(
                "Provider: {provider} · {}: {browsing} · Ctrl+P {}",
                app.language.text("浏览", "Browse", "参照"),
                app.language
                    .text("切换 / 点击此行", "switch / click here", "切替 / クリック")
            ),
            Style::default().fg(Color::LightCyan),
        ),
        Line::styled(
            format!("› {}", picker.editor.text()),
            Style::default().fg(Color::Yellow),
        ),
    ];
    if picker.loading {
        lines.push(Line::styled(
            app.language.text(
                "正在从 /v1/models 获取模型…",
                "Loading models from /v1/models…",
                "/v1/models からモデルを取得中…",
            ),
            Style::default().fg(Color::LightCyan),
        ));
    } else if let Some(error) = &picker.error {
        lines.push(Line::styled(
            format!(
                "{}: {error}",
                app.language.text(
                    "获取模型失败；仍可使用 /model <模型名>",
                    "Model lookup failed; /model <name> still works",
                    "モデル取得失敗。/model <名前> は使用可能です",
                )
            ),
            Style::default().fg(Color::LightRed),
        ));
    } else if picker.filtered.is_empty() {
        lines.push(Line::styled(
            app.language.text(
                "没有匹配的模型",
                "No models match",
                "一致するモデルがありません",
            ),
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        for (position, model_index) in picker.filtered.iter().enumerate().skip(start).take(visible)
        {
            let model = &picker.models[*model_index];
            let selected = position == picker.selected;
            let current = !picker.choosing_provider
                && picker.profile == picker.active_profile
                && model == &picker.current_model;
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
                format!(
                    "{} {}{}",
                    if selected { "▶" } else { " " },
                    model,
                    if current {
                        app.language.text(" · 当前", " · current", " · 現在")
                    } else {
                        ""
                    }
                ),
                style,
            ));
            app.model_picker_hits
                .push((popup.y + 3 + (position - start) as u16, position));
        }
    }
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(format!(
                    "{} · Ctrl+P Provider · {}/{} · ↑/↓/PgUp/PgDn · Enter · Esc",
                    if picker.choosing_provider {
                        app.language
                            .text("选择 Provider", "Select Provider", "Provider 選択")
                    } else {
                        app.language
                            .text("选择模型", "Select model", "モデルを選択")
                    },
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
    if popup.width > 3 {
        let cursor = UnicodeWidthStr::width(picker.editor.text())
            .min(popup.width.saturating_sub(4) as usize) as u16;
        f.set_cursor_position((popup.x + 3 + cursor, popup.y + 2));
    }
}

pub(super) fn parse(prompt: &str) -> Option<ModelCommand> {
    let value = prompt.trim();
    if value == "/model" {
        return Some(ModelCommand::List);
    }
    value
        .strip_prefix("/model ")
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(|model| ModelCommand::Switch(model.to_owned()))
}

pub(super) fn request_model_list(app: &mut App, runtime: &TuiRuntime, current_model: String) {
    app.open_model_picker(current_model);
    if let Err(error) = model_provider::initialize(app, runtime) {
        app.set_model_picker_result(Err(error.to_string()));
    }
}

/// 本轮在跑时只记下来，本轮结束再切；否则当场切。
///
/// 不能在轮次中途真的切：`agent.set_model` 会让进程内（`/local`）这一轮的下一次
/// 调模型就换掉，而这一轮的会话文件正由运行中的轮次写着，这里再 `store.save`
/// 会互相覆盖。延到轮次结束，行为就和「下一轮生效」的承诺一致。
pub(super) async fn switch_or_defer_model(
    model: &str,
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    runtime: &mut TuiRuntime,
    agent: &Arc<Agent>,
) -> Result<String> {
    app.pending_provider = None;
    if app.running {
        return defer_model_switch(app, model);
    }
    switch_model(model, app, session, store, runtime, agent).await
}

/// 记下待切换的模型。只校验名字、不碰 Agent / Runtime / 会话文件；同一轮里
/// 再敲一次 `/model` 以最后一次为准。
pub(super) fn defer_model_switch(app: &mut App, model: &str) -> Result<String> {
    let model = model.trim();
    if model.is_empty() || model.len() > MAX_MODEL_NAME_BYTES || model.chars().any(char::is_control)
    {
        bail!("model must contain 1 to {MAX_MODEL_NAME_BYTES} bytes");
    }
    app.pending_model = Some(model.to_owned());
    Ok(app
        .language
        .text(
            "本轮结束后切换到 {model}，下一轮起生效",
            "Switches to {model} when this turn ends; applies from the next turn",
            "このターン終了後に {model} へ切り替え、次のターンから適用",
        )
        .replace("{model}", model))
}

pub(super) async fn switch_model(
    model: &str,
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    runtime: &mut TuiRuntime,
    agent: &Arc<Agent>,
) -> Result<String> {
    let model = model.trim();
    if model.is_empty() || model.len() > MAX_MODEL_NAME_BYTES || model.chars().any(char::is_control)
    {
        bail!("model must contain 1 to {MAX_MODEL_NAME_BYTES} bytes");
    }
    // 还没发过消息的新会话不在磁盘上，Runtime 领养它会报 not found——和
    // submit_turn 一样先落盘。
    super::runtime_ui::persist_missing_core_session(session, store)?;
    let previous_model = runtime.provider_config.model.clone();
    agent
        .set_model(model)
        .with_context(|| format!("configure local Agent model `{model}`"))?;
    let remote_result = async {
        crate::daemon::ensure_runtime_session(
            &runtime.home,
            session.id,
            &session.workspace,
            session.profile.clone(),
            Some(model.to_owned()),
        )
        .await?;
        crate::daemon::update_remote_session_model(&runtime.home, session.id, model.to_owned())
            .await
    }
    .await;
    if let Err(error) = remote_result {
        let _ = agent.set_model(&previous_model);
        return Err(error).with_context(|| format!("update Runtime Session model to `{model}`"));
    }
    session.model = Some(model.to_owned());
    runtime.runtime_submit.model = Some(model.to_owned());
    runtime.provider_config.model = model.to_owned();
    store.save(session)?;
    Ok(format!(
        "{}: {model}",
        app.language
            .text("模型已切换", "Model switched", "モデルを切り替えました")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_list_and_direct_switch_without_stealing_similar_commands() {
        assert_eq!(parse(" /model "), Some(ModelCommand::List));
        assert_eq!(
            parse("/model  qwen3-coder "),
            Some(ModelCommand::Switch("qwen3-coder".to_owned()))
        );
        assert_eq!(parse("/models"), None);
        assert_eq!(parse("/model-name"), None);
    }
}
