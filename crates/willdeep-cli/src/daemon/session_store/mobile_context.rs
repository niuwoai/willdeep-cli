use super::*;

impl RuntimeSessionStore {
    /// 校验由控制面完成；完整候选先持久化，失败不改变内存中的会话。
    pub(in crate::daemon) fn configure_mobile(
        &self,
        id: uuid::Uuid,
        profile: Option<String>,
        model: Option<String>,
        mode: Option<super::super::WorkspaceAccess>,
        skills: Option<Vec<String>>,
        plugins: Option<Vec<String>>,
    ) -> Result<RuntimeSession> {
        // 模型与供应商供下一轮使用；审批句柄另行更新正在跑的一轮。
        let mut sessions = self.lock()?;
        let mut candidate = sessions
            .get(&id)
            .context("Runtime Session not found")?
            .clone();
        anyhow::ensure!(
            candidate.status != RuntimeSessionStatus::Archived,
            "Archived session cannot be configured"
        );
        let persist_core = candidate.active_turn_id.is_none()
            && !matches!(
                candidate.status,
                RuntimeSessionStatus::Queued
                    | RuntimeSessionStatus::Running
                    | RuntimeSessionStatus::WaitingApproval
                    | RuntimeSessionStatus::WaitingAnswer
            );
        let update_provider = profile.is_some();
        let update_model = model.is_some();
        if let Some(profile) = profile {
            candidate.profile = Some(profile);
        }
        if let Some(model) = model {
            candidate.model = Some(model);
        }
        if let Some(mode) = mode {
            candidate.approval_mode = Some(mode);
        }
        if let Some(skills) = skills {
            candidate.mobile_skills = skills;
        }
        if let Some(plugins) = plugins {
            candidate.mobile_plugins = plugins;
        }
        candidate.updated_at = now();
        let mut next = sessions.clone();
        next.insert(id, candidate.clone());
        persist_sessions(&self.path, &next)?;
        // A running worker owns the Core execution fingerprint. Preserve the
        // existing next-turn-only behavior for mobile changes during a turn.
        if persist_core
            && (update_provider || update_model)
            && let Err(error) = self.core.update(id, |core| {
                if update_provider {
                    core.profile = candidate.profile.clone();
                }
                if update_model {
                    core.model = candidate.model.clone();
                }
            })
        {
            persist_sessions(&self.path, &sessions)
                .context("restore Runtime configuration after Core Session save failure")?;
            return Err(error.into());
        }
        *sessions = next;
        Ok(candidate)
    }
}
