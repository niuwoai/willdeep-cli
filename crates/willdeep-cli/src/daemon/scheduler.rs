//! 定时任务的触发端（`willdeep-scheduler` 内置插件的宿主能力）。
//!
//! 插件的 MCP 进程只活一轮，没法自己计时；计时归常驻的 daemon。每
//! [`TICK`] 看一次 `$WILLDEEP_HOME/schedules.json`，到期的任务各开一个**全新**
//! Runtime 会话、把任务的 prompt 当第一句发出去（与 Xedit 的 Automations 一致）。
//!
//! Runtime 原生任务独立于入口插件。旧任务一次性迁移，原插件未启用的任务
//! 保持暂停；每次运行持久化配置快照与状态，由管理 API 维护配置和历史。

use std::time::Duration;

use anyhow::Result;

use super::ServerState;

/// 内置定时任务插件的 id，见 `builtin_plugins::PACKAGES`。
pub(super) const SCHEDULER_PLUGIN_ID: &str = "willdeep-scheduler";

/// 调度粒度。任务最短间隔是 1 分钟，30 秒的检查足够准时。
pub(super) const TICK: Duration = Duration::from_secs(30);

/// daemon 启动后常驻：每 [`TICK`] 触发一次到期任务。IO 都是同步的小文件读写，
/// 放到阻塞线程池里做，不占事件循环。
pub(super) fn spawn(state: std::sync::Arc<ServerState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let state = state.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let now = now();
                let offset = willdeep_core::schedule::local_utc_offset(now);
                if let Err(error) = fire_due(&state, now, offset) {
                    let _ = state
                        .events
                        .append("schedule.error", format!("error={error:#}"));
                }
            })
            .await;
        }
    });
}

pub(super) fn fire_due(state: &ServerState, now: u64, utc_offset: i64) -> Result<usize> {
    super::schedule_engine::tick(state, now, utc_offset)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}
