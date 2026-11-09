//! daemon 空闲自退。
//!
//! 评测和一次性的 `willdeep run` 会给各自的临时 home 拉起一个 Runtime Daemon，而它
//! 是脱离调用方进程组启动的：调用方的清理杀不到它，它又每几秒重写一次租约，
//! 攥着那个临时目录不让人删。没有人会回来对一个临时 home 执行 `daemon stop`，
//! 所以它得自己认出「没人用了」并退出。
//!
//! 判定要同时满足才算空闲，少一条都不退：
//!
//! - 没有排队、执行中或在等人回应的轮次，也没有还在跑的 Agent；
//! - 没有开启的服务：Web 页、手机中继、公网入口；
//! - 控制端点上没有任何连接。事件流是长连接，TUI 挂着它就一直算有人在用。
//!
//! 连接数取自传输层而不是应用层：请求一到来 axum 就先消费掉 body 再跑处理函数，
//! 在处理函数里数「正在处理的请求」会把正在问这句话的那一个也算进去，永远数不到零。
//! 轮询的间隔远长过一次请求的处理时间，所以一次查询不会把自己算成「有人在用」。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::Result;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

use super::session_store::{RuntimeSessionStatus, RuntimeTurnStatus};
use super::{RuntimeAgentStatus, ServerState};

/// 连接计数挂在传输层。TCP 与本地套接字各走各的监听，但空闲与否看的是总和：
/// 任何一条连接都说明有客户端还在。
pub(super) type ConnectionCounter = Arc<AtomicUsize>;

pub(super) fn connection_counter() -> ConnectionCounter {
    Arc::new(AtomicUsize::new(0))
}

/// 把连接计数器放进请求扩展。单独一层是因为它要读路由状态，而计数那一层
/// 不能再读：axum 0.8 的 `from_fn` 要么取状态、要么取请求，不能同时取。
pub(super) async fn attach_connection_counter(
    axum::extract::State(counter): axum::extract::State<ConnectionCounter>,
    mut request: Request,
    next: Next,
) -> Response {
    request.extensions_mut().insert(counter);
    next.run(request).await
}

/// 请求进来加一、出去减一。放在 body 被消费之前的中间件里，所以它数的是连接上
/// 还没结束的请求，不是处理函数。
///
/// 计数器走请求扩展而不是中间件的状态参数：这一层挂在 `with_state` 之后，
/// axum 0.8 的 `from_fn` 只从请求里取参数。没有挂上计数器的请求（测试里直接调
/// 处理函数）不计。
pub(super) async fn track_connection(request: Request, next: Next) -> Response {
    let counter = request.extensions().get::<ConnectionCounter>().cloned();
    if let Some(counter) = &counter {
        counter.fetch_add(1, Ordering::SeqCst);
    }
    let response = next.run(request).await;
    if let Some(counter) = &counter {
        counter.fetch_sub(1, Ordering::SeqCst);
    }
    response
}

/// 启动后常驻。第一次发现空闲只记下时刻，连续空闲超过 [`super::IDLE_EXIT_AFTER`]
/// 才退出；中间任何一次不空闲都把时刻清掉，重新计。
pub(super) fn spawn(state: Arc<ServerState>) {
    let shutdown = state.shutdown.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(super::IDLE_CHECK_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // 第一次 tick 立即到期。启动本身不算空闲，先跳过它。
        interval.tick().await;
        let mut idle_since: Option<Instant> = None;
        loop {
            interval.tick().await;
            if *shutdown.borrow() {
                return;
            }
            let quiet = match runtime_is_idle(&state).await {
                Ok(quiet) => quiet,
                Err(error) => {
                    // 查不出就按还在用处理：宁可多留一会儿，也不在读失败时把一个
                    // 正有人用的 Runtime 退掉。
                    eprintln!("Runtime idle check failed; keeping the Daemon: {error:#}");
                    false
                }
            };
            if !quiet {
                idle_since = None;
                continue;
            }
            let since = idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() < super::IDLE_EXIT_AFTER {
                continue;
            }
            // 退出前再确认一次。窗口的最后一刻可能正好有人连上来。
            match runtime_is_idle(&state).await {
                Ok(true) => {
                    let idle_seconds = since.elapsed().as_secs();
                    let _ = state.events.append(
                        "daemon.idle",
                        format!("pid={} idle_seconds={idle_seconds}", std::process::id()),
                    );
                    eprintln!(
                        "WillDeep Runtime Daemon has been idle for {idle_seconds}s with no client; stopping"
                    );
                    let _ = shutdown.send(true);
                    return;
                }
                Ok(false) => idle_since = None,
                Err(error) => {
                    eprintln!("Runtime idle recheck failed; keeping the Daemon: {error:#}");
                    idle_since = None;
                }
            }
        }
    });
}

/// 现在这一刻有没有任何理由继续留着。
async fn runtime_is_idle(state: &ServerState) -> Result<bool> {
    if connections_open(&state.connections) {
        return Ok(false);
    }
    if service_enabled(state).await {
        return Ok(false);
    }
    // 不放到阻塞线程上：账本都是小文件，而 `spawn_blocking` 会把 `ServerState`
    // 的克隆活得比这次检查更久，测试里目录可能先被删掉。
    Ok(!work_pending(&Arc::new(state.clone()))?)
}

fn connections_open(counter: &ConnectionCounter) -> bool {
    counter.load(Ordering::SeqCst) > 0
}

/// 用户显式开过的服务。它们空着也是在等人，不算没人用。
async fn service_enabled(state: &ServerState) -> bool {
    if state.web_app.status().enabled {
        return true;
    }
    if state.mobile.status().enabled {
        return true;
    }
    state.intake.status().await.is_some()
}

/// 还没做完的活：排队或执行中的轮次、等人回应的轮次、还在跑的 Agent。
/// 这些都存在各自的账本里，不依赖有没有客户端连着。
fn work_pending(state: &Arc<ServerState>) -> Result<bool> {
    let sessions = state.sessions.list()?;
    for session in &sessions {
        if matches!(
            session.status,
            RuntimeSessionStatus::Queued
                | RuntimeSessionStatus::Running
                | RuntimeSessionStatus::WaitingApproval
                | RuntimeSessionStatus::WaitingAnswer
        ) {
            return Ok(true);
        }
        for turn in state.sessions.list_turns(session.id)? {
            if matches!(
                turn.status,
                RuntimeTurnStatus::Queued
                    | RuntimeTurnStatus::Running
                    | RuntimeTurnStatus::WaitingApproval
                    | RuntimeTurnStatus::WaitingAnswer
            ) {
                return Ok(true);
            }
        }
    }
    for agent in state.agents.list()? {
        if matches!(
            agent.status,
            RuntimeAgentStatus::Queued
                | RuntimeAgentStatus::Running
                | RuntimeAgentStatus::WaitingApproval
                | RuntimeAgentStatus::WaitingAnswer
                | RuntimeAgentStatus::Blocked
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 一个刚启动、什么都没有的 Runtime 是空闲的；有连接或有活就不是。
#[cfg(test)]
mod tests {
    use super::super::tests::{test_runtime_session_store, test_server_state};
    use super::*;

    fn fresh() -> (std::path::PathBuf, ServerState) {
        // 不用系统临时目录：那里可能留着别的测试写的 `webapp-service.json`，
        // `WebAppService` 会把它读成「Web 页开着」，空闲判定就被带偏。
        // 构建目录下的 tmp 是本次编译专有的，里面不会有别人的状态文件。
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("willdeep-idle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let state = test_server_state(&root, test_runtime_session_store(&root));
        (root, state)
    }

    #[tokio::test]
    async fn a_fresh_runtime_is_idle() {
        let (root, state) = fresh();
        assert!(runtime_is_idle(&state).await.unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn an_open_connection_keeps_the_runtime() {
        let (root, state) = fresh();
        state.connections.fetch_add(1, Ordering::SeqCst);
        assert!(!runtime_is_idle(&state).await.unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_queued_turn_keeps_the_runtime() {
        let (root, state) = fresh();
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let session = state
            .sessions
            .create(super::super::session_store::CreateRuntimeSession {
                id: None,
                workspace,
                profile: None,
                model: None,
                config: None,
                title: None,
            })
            .unwrap();
        state
            .sessions
            .enqueue_turn(
                session.id,
                super::super::session_store::CreateRuntimeTurn {
                    team_control: None,
                    request_id: uuid::Uuid::new_v4(),
                    prompt: "继续".to_owned(),
                    attachments: Vec::new(),
                    origin_client: None,
                    defer_until_released: false,
                },
            )
            .unwrap();
        assert!(work_pending(&Arc::new(state)).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_finished_turn_does_not_keep_the_runtime() {
        let (root, state) = fresh();
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let _session = state
            .sessions
            .create(super::super::session_store::CreateRuntimeSession {
                id: None,
                workspace,
                profile: None,
                model: None,
                config: None,
                title: None,
            })
            .unwrap();
        assert!(!work_pending(&Arc::new(state)).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_connection_counter_tracks_a_request() {
        let counter = connection_counter();
        assert!(!connections_open(&counter));
        counter.fetch_add(1, Ordering::SeqCst);
        assert!(connections_open(&counter));
        counter.fetch_sub(1, Ordering::SeqCst);
        assert!(!connections_open(&counter));
    }
}
