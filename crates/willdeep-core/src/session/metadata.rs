use super::{Session, SessionError, TitleSource};
use std::path::PathBuf;

/// In-memory baseline only. It never becomes part of the persisted schema.
#[derive(Clone, Debug)]
pub(super) struct Metadata {
    title: (String, TitleSource),
    pinned_at: Option<u64>,
    model: Option<String>,
    profile: Option<String>,
    config: Option<PathBuf>,
    goal: Option<String>,
    response_style: Option<super::ResponseStyle>,
    /// `None` 是「这份快照没有执行基线」，不是某个执行状态的指纹。只读加载留空；
    /// 要改执行状态再写回的调用方必须在改动前用 `SessionStore::load_baselined`
    /// 算好。不能用全零占位——那是一个真实指纹可能取到的值，撞上会让合并
    /// 把「没算过」当成「没改过」。
    execution: Option<[u8; 32]>,
}

impl Metadata {
    pub(super) fn refresh_execution(&mut self, saved: &Session) -> Result<(), SessionError> {
        self.execution = Some(super::execution_state::fingerprint(saved)?);
        Ok(())
    }

    /// 只取标量字段。执行指纹是把整段对话序列化进 sha256，长会话上一次就要
    /// 几十毫秒，而读会话的大多数调用方（列表、归属校验、轮询）根本不写回，
    /// 所以加载时不算它，留给真正要合并写回的路径按需补。
    pub(super) fn capture_scalars(session: &Session) -> Self {
        Self {
            title: (session.title.clone(), session.title_source),
            pinned_at: session.pinned_at,
            model: session.model.clone(),
            profile: session.profile.clone(),
            config: session.config.clone(),
            goal: session.goal.clone(),
            response_style: session.response_style,
            execution: None,
        }
    }

    #[cfg(test)]
    pub(super) fn has_execution_fingerprint(&self) -> bool {
        self.execution.is_some()
    }

    pub(super) fn capture(session: &Session) -> Result<Self, SessionError> {
        let mut metadata = Self::capture_scalars(session);
        metadata.execution = Some(super::execution_state::fingerprint(session)?);
        Ok(metadata)
    }

    pub(super) fn merge(
        &self,
        proposed: &mut Session,
        latest: &Session,
    ) -> Result<(), SessionError> {
        // Validate all conflicts before changing the caller's snapshot.
        let mut merged = Self::capture(proposed)?;
        let current = Self::capture(latest)?;
        // 基线没有指纹即「这份快照没动过执行状态」：保留磁盘上的那一份。
        let retain_latest_execution = self
            .execution
            .is_none_or(|baseline| merged.execution == Some(baseline));
        if !retain_latest_execution
            && current.execution != self.execution
            && current.execution != merged.execution
        {
            return Err(SessionError::ConcurrentUpdate("execution"));
        }
        macro_rules! reconcile {
            ($field:ident) => {
                if merged.$field == self.$field {
                    merged.$field = current.$field;
                } else if current.$field != self.$field && current.$field != merged.$field {
                    return Err(SessionError::ConcurrentUpdate(stringify!($field)));
                }
            };
        }
        if merged.title.1 != TitleSource::User && current.title.1 == TitleSource::User {
            merged.title = current.title;
        } else {
            reconcile!(title);
        }
        reconcile!(pinned_at);
        reconcile!(model);
        reconcile!(profile);
        reconcile!(config);
        reconcile!(goal);
        reconcile!(response_style);
        if retain_latest_execution {
            super::execution_state::copy(latest, proposed);
        }
        (proposed.title, proposed.title_source) = merged.title;
        proposed.pinned_at = merged.pinned_at;
        proposed.model = merged.model;
        proposed.profile = merged.profile;
        proposed.config = merged.config;
        proposed.goal = merged.goal;
        proposed.response_style = merged.response_style;

        // 视图状态与路由归属不进执行指纹（见 `execution_state::fingerprint`），
        // 所以它们不会把两个写者判成冲突；但也不能让后写的那一方用自己的旧值
        // 把对方盖掉——守护进程跑完一轮写回来时，手上那份游标还停在它加载会话
        // 的那一刻，直接写回去就会把客户端刚记下的位置抹平，下次重开又从头重放。
        //
        // 三项各按自己的语义合：游标是单调书签取较大者，已读标记只增不减取并集，
        // 托管标志一旦置位就保持。
        proposed.runtime_event_cursor = proposed
            .runtime_event_cursor
            .max(latest.runtime_event_cursor);
        proposed
            .attention_read
            .extend(latest.attention_read.iter().cloned());
        proposed.runtime_managed |= latest.runtime_managed;
        proposed
            .agent_reports_received
            .extend(&latest.agent_reports_received);
        proposed
            .agent_control_requests
            .extend(&latest.agent_control_requests);
        Ok(())
    }
}
