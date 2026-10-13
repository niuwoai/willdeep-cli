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
        *sessions = next;
        Ok(candidate)
    }
}
