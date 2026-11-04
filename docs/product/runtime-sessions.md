# Runtime 与会话产品模块

> 范围：Daemon、工作区、Session/Turn、队列、计划、定时任务、持久事件与恢复。开发版本见[产品概览](../../PRODUCT_OVERVIEW.md)；对象与参数唯一契约仍由 Runtime 专题维护。

## 能力概要与用户入口

CLI、TUI 与 Web 共用持久 Runtime Session/Turn、主 Harness 和 Runtime Client。Daemon 独立管理执行、人工等待、队列及事件，客户端退出可以分离观察而不停止已受理任务。入口包括 `willdeep daemon`、`willdeep session`、`willdeep schedule`、TUI `/runtime`、`/session`、`/schedule` 及 Web 会话/定时任务页面。

## 已实现范围

### 常驻服务与统一控制

- 跨平台 Daemon 提供 start/status/stop/logs/upgrade，使用带心跳的单实例租约协调启动。只读查询不自动启动、重启或清状态；实际提交和修改按原规则启动。TUI 不可用时的单次启动尝试见[终端模块](cli-tui.md)。
- 本机优先使用权限 0600 的 Unix Socket 或拒绝远端的 Windows Named Pipe，兼容受 Token 保护的回环 TCP。能力协商、公开对象、操作名、成功/错误信封由 `willdeep-runtime-protocol` 定义。
- 公开控制统一走 `POST /v1/api` 和共享 `willdeep-runtime-client`，覆盖 Runtime、Workspace、Session、Agent、Turn、Task、Approval、Question、Tool、Artifact、Event、Diff 与 Worktree。修改进入 Pending→Completed 持久幂等日志；不确定崩溃窗口拒绝自动重放。
- 进程内 Harness 的完整 Task 提交、长等待 Interaction、Agent 命令和 drain/shutdown 走 `/v1/internal` 私有传输，除 Token 外需要内部标记，公开 SDK 不提供这些方法。当前 TUI/CLI 不再回退旧资源路由；旧 Runtime 需受控升级。
- Daemon 直接持有 Harness Future，不为每个 Turn 启 CLI 子进程。各入口共用 Provider、视觉降级、审批、Skills、MCP、工具、Worker 配置、会话写入与结果回流的 Harness Factory。
- Upgrade 先以 draining 拒绝新工作，保留活动任务与持久排队 Turn，按实际 Task 状态等待排空；交接等待完整终态提交。关闭信号覆盖晚订阅者，控制连接有 5 秒收尾窗，超时释放活租约，清理失败明确报告。
- `/mobile` 与 Runtime 托管 `/webapp` 服务由 daemon 管理，退出 CLI 不关闭；Runtime 停止关闭监听，重启按已保存开关恢复。显式前台 Web 与首启临时配置服务另有生命周期，不能与托管开关混称。

### 工作区、会话与轮次

- 持久工作区注册表支持注册、更新、列表、激活与保守移除，保存规范化根路径、访问策略、默认 Provider、Skills/MCP 允许列表；切换默认项不影响旧任务。权限由服务端注册表覆盖客户端输入，见[安全模块](security-verification.md)。
- Session 元数据可迁移，旧 schema 升级前保存私有原始备份。新会话以首 Turn 生成有界标题，疑似凭据不复制进标题；显式命名与 Fork 标题保留人工所有权。Core Session 是聊天唯一事实来源。
- Session/Turn 保留稳定 Root Agent、幂等请求身份、严格串行队列、运行/排队取消和终态事件；结果提交后清除队列私密正文。运行与结果保存持有会话执行权，拒绝同会话竞争执行。
- 支持 Rename、完整消息快照 Fork、Archive/Unarchive、精确确认 Delete、安全 JSON Export、标题/消息 Search。活跃/排队会话禁止破坏性管理；Fork 不复制 Turn/Task/Interaction、游标及已读，Export 不包含队列私密正文与凭据。
- 分叉基于已完成 Turn 的持久消息边界，可覆盖 Provider/模型；手工压缩持久记录代次与消息计数，Runtime Fork 仅接受当前压缩代次的精确边界。历史来源与完整子会话树形 parent 分开，见[协作模块](agents-collaboration.md)。
- Provider、模型和配置按 Session 恢复；Skills/MCP 每轮按当前工作区策略重绑，撤权立即生效。`/goal` 保存持续目标并启动首轮，`/goal off` 关闭，各客户端共用目标信封与续推判定；运行中排队的新目标到执行时才生效。
- 完整子会话 P1 复用上述 Session/Turn 与主 Harness；fork 的上下文来源、worktree 绑定、模型窗口、assignment、协调账本和父唤醒另见[完整子会话指南](../CHILD_SESSIONS.md)，不在此另写一套对象协议。

## 持久队列与人工等待

Runtime 是**已提交消息**的权威来源，客户端输入框草稿仍归本机。Android、Web 与 Runtime TUI 共享可编辑队列；已接受消息不因刷新或 TUI 退出消失。忙碌时文本及附件持久入队，空闲时也可显式入队并暂缓，直到选择立即发送。

队列支持编辑、删除、清空及立即发送；修改校验原文并保留附件/宿主上下文。管理 Runtime 队列不会暂停宿主调度，已经开始的消息拒绝过期编辑。显式本地执行保留本地插话及面板暂停语义，详见[CLI 与 TUI](cli-tui.md)。

人工请求归属会话，其他会话不可代答，切换时忽略旧会话快照；已记录的发起端与跨端审批规则见[安全模块](security-verification.md)。新普通输入不充当旧问题答案。`human_wait_yield_seconds` 默认 300（0 关闭）可将等待让给独立消息：无工具保守分类，依赖或不确定时保留队列，用户也可明确标记独立任务；原确认不产生默认结果，旧答案在安全边界恢复，独立历史与原上下文分别保存。

### 计划与定时任务

- `update_plan` 为普通会话与目标模式提供步骤 ID、`depends_on`、可启动项和等待原因，允许独立工作并行；共享文件、版本、Git 与部署资源须串行安排。步骤状态持久化，恢复先对账已有任务，状态本身不是完成证据。
- 完整步骤的后台命令可用 `plan_step_id` 或 `job_id` 关联，按实际退出状态独立更新并推送；失败/未知不放行依赖，中间命令不冒充整个步骤完成。客户端中途接入可补建计划卡，图标及动画遵循减少动态效果偏好。
- Runtime 原生定时任务在 CLI/Web 关闭后继续调度；共用配置和历史，支持显式时区、单次计划、预算、超时、去重、恢复、归档和历史保留。旧任务保留 ID 与启用意图，插件未启用时导入为暂停。
- 会话定时任务自 v0.100.0-rc1 已实现：聊天创建绑定当前会话，到点续跑，原会话忙时跳过；旧独立任务继续独立会话。运行结束、目标达成、验证通过分别记录，定时循环不证明质量提升。

### 事件、观察与恢复

- 事件以单调序号 NDJSON 持久化，按 `attach --after` 游标补读；Token SSE 先补历史再转实时，慢客户端从日志恢复。日志读写共用互斥边界，不读未完成末行。drain/shutdown 关闭观察连接，客户端沿游标重附着，不阻塞优雅退出。
- TUI 按 Session 事件游标更新内存预览，Runtime 保存活动会话检查点，收尾/正常退出合并客户端游标。预览事件可由专线程按最多 16ms 分组落盘再广播，关键事件同步写；连续会话文本检查点另有约 250ms/16KiB 合并策略。不同层的刷新窗口不能互相替代。
- 新会话从 Runtime 当前事件头接入，历史与用量按会话隔离；断线只补事件，不再次提交 Prompt。Web 恢复即使遇到 Turn 已结束也读取最终消息；重试等待作为结构状态传递并在开始重试/终止/重启时清除。
- Tool Activity、Workspace Change Artifact 与 Attention 状态由持久对象计算，支持按会话/轮次/任务/Agent 查询并向会话、工作区聚合，不从聊天文本反推；公开投影只含受允许元数据。
- 界面初始化不占团队执行锁或恢复 Worker；取得执行权后的首轮只恢复一次。Worker 私有恢复、Worktree 保留与工作流对账归[协作模块](agents-collaboration.md)。

## 当前限制

重启后仅对“无工具活动、历史边界完全匹配”的活动 Turn 自动重放；用户消息复用，副作用证据或歧义保留并停止自动恢复。遗留 Running/Waiting 状态收敛为 Interrupted，Pending Interaction 取消并记恢复事件。**不承诺跨重启恢复任意活动进程或副作用。** 强杀/断电可能丢失未刷新的少量流文本，工具副作用记录不采用该延迟策略。

统一公开 API 已有实现，不能再整体标成待做；完整执行资源恢复、任务级自然语言验收与其余写入口的一致性仍有边界。相应早期路由回退和“API 待完成”记录保存在[历史快照](history/overview-through-0.142.0-rc1.md)。

## 详细指南与证据

- [Runtime Daemon](../RUNTIME_DAEMON.md)、[控制 API](../RUNTIME_CONTROL_API.md)、[会话协议](../RUNTIME_SESSION_PROTOCOL.md)、[进程内 Harness](../IN_PROCESS_RUNTIME_HARNESS.md)。
- [附件与 Runtime 定时任务设计](../decisions/2026-10-12-attachments-runtime-schedules.md)、[定时任务预算/超时验收](../verification/0.98.0-rc1/runtime-management.md)、[Runtime Web/手机生命周期验收](../verification/0.99.0-rc1/runtime-web-smoke.md)。
- [检查点回退](../CHECKPOINT_REWIND.md)、[流式验收](../verification/0.114.0-rc2/README.md)、[交接回归](../verification/0.114.0-rc3/README.md)。历史验收是当时结果，不是本次文档整理重新执行的检查。
