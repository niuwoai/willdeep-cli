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
    pub(super) provider_filtered: Vec<usize>,
    pub(super) provider_selected: usize,
    pub(super) provider_editor: PromptEditor,
    /// Focuses the Provider pane, or shows that page on narrow terminals.
    pub(super) choosing_provider: bool,
    pub(super) wide: bool,
    pub(super) request_id: uuid::Uuid,
    pub(super) load_task: Option<ModelLookupTask>,
    pub(super) cache: std::collections::BTreeMap<Option<String>, Vec<String>>,
    pub(super) editor: PromptEditor,
    pub(super) models: Vec<String>,
    pub(super) filtered: Vec<usize>,
    pub(super) selected: usize,
    pub(super) current_model: String,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
}

pub(super) struct ModelLookupTask(pub(super) tokio::task::JoinHandle<()>);

impl Drop for ModelLookupTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) enum ModelPickerAction {
    None,
    Close,
    Select(String),
    Provider(Option<String>),
    Preview(String),
    Refresh,
}

impl ModelPickerState {
    pub(super) fn focused_editor(&self) -> &PromptEditor {
        if self.choosing_provider {
            &self.provider_editor
        } else {
            &self.editor
        }
    }

    fn focused_editor_mut(&mut self) -> &mut PromptEditor {
        if self.choosing_provider {
            &mut self.provider_editor
        } else {
            &mut self.editor
        }
    }

    fn provider_action(&self, enter: bool) -> ModelPickerAction {
        let Some(index) = self.provider_filtered.get(self.provider_selected) else {
            return ModelPickerAction::None;
        };
        let profile = self.profiles[*index].clone();
        if enter {
            ModelPickerAction::Provider(Some(profile))
        } else {
            ModelPickerAction::Preview(profile)
        }
    }

    fn navigate(&mut self, delta: isize, wrap: bool) -> ModelPickerAction {
        let (selected, len) = if self.choosing_provider {
            (&mut self.provider_selected, self.provider_filtered.len())
        } else {
            (&mut self.selected, self.filtered.len())
        };
        if len == 0 {
            return ModelPickerAction::None;
        }
        *selected = if wrap {
            (*selected as isize + delta).rem_euclid(len as isize) as usize
        } else {
            selected.saturating_add_signed(delta).min(len - 1)
        };
        if self.choosing_provider {
            self.provider_action(false)
        } else {
            ModelPickerAction::None
        }
    }
}

impl App {
    pub(super) fn open_model_picker(&mut self, current_model: String) {
        self.palette = None;
        self.search = None;
        self.session_picker = None;
        self.model_picker = Some(ModelPickerState {
            current_model,
            loading: true,
            wide: true,
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
        let previous = picker
            .filtered
            .get(picker.selected)
            .and_then(|i| picker.models.get(*i))
            .cloned();
        picker.loading = false;
        match result {
            Ok(models) => {
                picker.cache.insert(picker.profile.clone(), models.clone());
                picker.models = models;
                picker.error = None;
            }
            Err(error) => picker.error = Some(error),
        }
        let wanted = previous.or_else(|| {
            (picker.profile == picker.active_profile).then(|| picker.current_model.clone())
        });
        self.refresh_model_picker_matches();
        let picker = self.model_picker.as_mut().unwrap();
        if let Some(wanted) = wanted {
            picker.selected = picker
                .filtered
                .iter()
                .position(|i| picker.models[*i] == wanted)
                .unwrap_or(0);
        }
    }

    pub(super) fn refresh_model_picker_matches(&mut self) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.filtered = model_picker_matches(&picker.models, picker.editor.text());
        picker.selected = 0;
    }

    pub(super) fn refresh_provider_matches(&mut self) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.provider_filtered =
            model_picker_matches(&picker.profiles, picker.provider_editor.text());
        picker.provider_selected = picker
            .provider_filtered
            .iter()
            .position(|i| Some(&picker.profiles[*i]) == picker.profile.as_ref())
            .unwrap_or(0);
    }

    pub(super) fn handle_model_picker_key(&mut self, key: KeyEvent) -> ModelPickerAction {
        let Some(picker) = self.model_picker.as_mut() else {
            return ModelPickerAction::None;
        };
        if key.code == KeyCode::Esc {
            if !picker.wide && !picker.choosing_provider && !picker.profiles.is_empty() {
                picker.choosing_provider = true;
                return ModelPickerAction::None;
            }
            return ModelPickerAction::Close;
        }
        if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return ModelPickerAction::Provider(None);
        }
        if key.code == KeyCode::F(5) {
            return ModelPickerAction::Refresh;
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                picker.choosing_provider = !picker.choosing_provider;
                return ModelPickerAction::None;
            }
            KeyCode::Up => return picker.navigate(-1, true),
            KeyCode::Down => return picker.navigate(1, true),
            KeyCode::PageUp => return picker.navigate(-10, false),
            KeyCode::PageDown => return picker.navigate(10, false),
            KeyCode::Right if picker.choosing_provider && picker.provider_editor.is_empty() => {
                return picker.provider_action(true);
            }
            KeyCode::Left if !picker.choosing_provider && picker.editor.is_empty() => {
                picker.choosing_provider = true;
                return ModelPickerAction::None;
            }
            KeyCode::Enter if picker.choosing_provider => return picker.provider_action(true),
            KeyCode::Enter if !picker.filtered.is_empty() => {
                return ModelPickerAction::Select(
                    picker.models[picker.filtered[picker.selected]].clone(),
                );
            }
            KeyCode::Enter if !picker.editor.is_empty() && !picker.loading => {
                return ModelPickerAction::Select(picker.editor.text().trim().to_owned());
            }
            _ => {}
        }
        let editor = picker.focused_editor_mut();
        let changed = match key.code {
            KeyCode::Left => {
                editor.left();
                false
            }
            KeyCode::Right => {
                editor.right();
                false
            }
            KeyCode::Home => {
                editor.home();
                false
            }
            KeyCode::End => {
                editor.end();
                false
            }
            KeyCode::Backspace => {
                editor.backspace();
                true
            }
            KeyCode::Delete => {
                editor.delete();
                true
            }
            KeyCode::Char(c)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::ALT,
                ) =>
            {
                editor.insert(&c.to_string());
                true
            }
            _ => false,
        };
        if changed {
            if self.model_picker.as_ref().unwrap().choosing_provider {
                self.refresh_provider_matches();
                return self.model_picker.as_ref().unwrap().provider_action(false);
            }
            self.refresh_model_picker_matches();
        }
        ModelPickerAction::None
    }

    pub(super) fn model_picker_scroll_at(
        &mut self,
        delta: isize,
        x: u16,
        y: u16,
    ) -> ModelPickerAction {
        let point = (x, y).into();
        let Some(picker) = self.model_picker.as_mut() else {
            return ModelPickerAction::None;
        };
        if self.model_provider_rect.contains(point) {
            picker.choosing_provider = true;
        } else if self.model_list_rect.contains(point) {
            picker.choosing_provider = false;
        } else {
            return ModelPickerAction::None;
        }
        picker.navigate(delta, false)
    }

    pub(super) fn activate_model_picker_at(&mut self, x: u16, y: u16) -> Option<ModelPickerAction> {
        let point = (x, y).into();
        if !self.model_picker_rect.contains(point) {
            return None;
        }
        let picker = self.model_picker.as_mut()?;
        if !picker.wide && y == self.model_picker_rect.y + 1 {
            picker.choosing_provider = !picker.choosing_provider;
            return Some(ModelPickerAction::None);
        }
        if let Some((_, position)) = self
            .model_provider_hits
            .iter()
            .find(|(rect, _)| rect.contains(point))
        {
            picker.provider_selected = *position;
            picker.choosing_provider = true;
            return Some(picker.provider_action(!picker.wide));
        }
        if let Some((_, position)) = self
            .model_picker_hits
            .iter()
            .find(|(rect, _)| rect.contains(point))
        {
            picker.selected = *position;
            picker.choosing_provider = false;
            return Some(ModelPickerAction::Select(
                picker.models[picker.filtered[*position]].clone(),
            ));
        }
        let provider = self.model_provider_rect.contains(point);
        let model = self.model_list_rect.contains(point);
        if provider || model {
            picker.choosing_provider = provider;
            let pane = if provider {
                self.model_provider_rect
            } else {
                self.model_list_rect
            };
            if y == pane.y + 1 {
                picker.focused_editor_mut().set_cursor_visual(
                    0,
                    x.saturating_sub(pane.x + 3) as usize,
                    pane.width.saturating_sub(4).max(1) as usize,
                );
            }
        }
        None
    }
}

fn model_picker_matches(values: &[String], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let mut ranked = values
        .iter()
        .enumerate()
        .filter_map(|(i, value)| fuzzy_score(&query, &value.to_lowercase()).map(|score| (score, i)))
        .collect::<Vec<_>>();
    ranked.sort_by_key(|(score, i)| (*score, *i));
    ranked.into_iter().map(|(_, i)| i).collect()
}

pub(super) fn render_model_picker(f: &mut ratatui::Frame<'_>, app: &mut App) {
    super::model_picker_ui::render(f, app);
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
    model_provider::discover_local(runtime);
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
    app.display_model = Some(model.to_owned());
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
