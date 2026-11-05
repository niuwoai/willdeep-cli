# 完整子会话 P2：控制权、通知与任务依赖

> 日期：2026-11-05
> 版本：0.144.0-rc1
> 状态：已实现；实际执行记录、首次失败与重跑结果见[0.144.0-rc1 验收台账](../verification/0.144.0-rc1.md)。
> 基线：`origin/develop` 的 0.143.0-rc7，包含完整子会话 P0/P1。

## 目标与范围

继续既有完整子会话方案的 P2，覆盖用户接管、解除父子关系、细化结果通知和任务依赖。保留 Session、assignment、Turn、Worker 的现有身份与执行事实，不建立第二套执行器。Web、CLI/TUI 与主 Agent 工具复用 Runtime 协调器。

沿用现有桌面/终端布局、组件、主题与本地化资源。此轮无新平台、数据库或跨宿主运行时；Android/Xedit 不自动获得新的控制入口。

## 控制权与历史

- 用户接管后，会话仍可打开，当前执行不被暗中取消；原父会话不能继续派工、重试或通过旧 assignment 停止该会话。用户在子会话的普通聊天与停止入口仍有效。
- 接管使尚未交付的旧任务结果停止自动送入父模型，不删除任务结果、报告、聊天或审计关系；父界面可以查询历史。
- 解除父子关系同时取消父控制与后续自动通知，从普通会话树中独立展示；保留原父级、创建者、worktree 来源与历史委派记录。
- 控制权变化不移除持久只读上限、不扩大 Workspace 权限、不重置家族预算/用量、不自动合并、删除 worktree 或创建模型 Turn。
- 接管与解除关系只由用户入口发起，不作为模型工具开放。请求须有并发版本校验；响应丢失后相同请求能够安全重放，过期或无关关系请求明确拒绝。
- worktree 会话独立显示后仍通过原授权工作区与持久审计关系校验访问，不因移除树形父级失去访问，也不得扩大路径授权。

## 通知策略

沿用当前自动回报和关闭通知，增加仅在任务列表保留结果、不自动唤醒或注入父模型的策略。订阅范围仍为具体 assignment，普通后续聊天不自动加入旧委派。

用户可以修改未消费结果的通知偏好；已持久消费的历史不撤回、不重新标为未消费。任何变更均保留结果身份，停止抑制仍由用户明确继续父会话解除，通知变更不解除抑制。修改或关闭通知时，已经入队但尚未执行的空结果唤醒不得调用模型；专用跳过路径结束对应 Task/Turn，并使没有其他活动轮次的父会话回到 Idle，保留真实停止抑制，允许其他有效结果以后唤醒。

通知快照仅供候选选择。最终资格复核、构建待提交上下文和父历史 checkpoint 写入必须在同一个协调控制锁边界内同步完成，使用 `control → Core Session` 锁序，不持锁等待模型、Git 或异步操作。先成功撤销时，候选通知不能进入持久历史或模型内存；先完成历史提交时，该历史保留，不能宣称已撤回。消费仍以真实持久历史为证，不提前写入虚构消费回执。普通收尾没有追加新上下文时不额外保存 checkpoint，但仍运行最终宿主资格检查，不能让已撤销的空唤醒绕过校验。检查点保存成功后先更新可恢复快照和流状态，再确认 Worker 报告，后置确认失败不能回写旧历史。保存本身可能在文件替换后报告同步失败；这类结果不确定时，错误收尾、流刷新、Drop 和计划更新不推测性重写历史，直到显式成功保存新的完整边界。

## 任务依赖

- 新任务可依赖同一父会话已存在的 assignment，形成有向无环依赖图，支持串行、扇出和汇合。
- 等待依赖时保留持久任务身份并计入等待队列，不占执行槽、不创建 worktree、不调用模型。
- 全部前置任务成功后才可执行；任一前置任务失败、取消、部分完成、中断或阻塞，后续任务形成明确阻塞结果，不自动尝试绕过或替换依赖。
- 拒绝缺失、越权、重复、自依赖、循环及超过上限的依赖。接管或解除关系后，不允许新依赖隐式恢复父控制；已有等待任务需收敛为可解释状态。
- 重试产生新 assignment，旧结果不可变；原失败 assignment 的成功重试不会偷偷把旧依赖改写为成功。需要改变依赖时必须显式创建新任务。
- 恢复根据持久依赖和 Turn/Task 权威状态重新调度，沿用固定请求/Turn 身份，不重复执行模型。

## 容量调度与持久化

调度通道传递的是重新检查持久队列的提示，不是独立 Turn。周期扫描与重启按每个会话最早的未暂缓 queued Turn 的 `queue_sequence` 生成候选顺序，不能让 HashMap 迭代顺序反复把旧任务排到新任务之后；同一会话只生成一个候选，已有活动 Turn 和归档会话仍排除。周期协调可能重复通知受容量限制的同一会话；消费者按当前通道长度的快照有界收集并合并重复 Session，保留不同会话的首次入队顺序。新到达的通知继续进入后续批次，不能用永久已调度集合吞掉任务完成后的新轮次。

实际领取子会话执行槽时，还必须在同一个控制锁、会话和 Turn 状态快照内比较可执行候选的持久排队顺序。较早候选遇满槽后，后到提示不能抢走随后释放的槽。暂缓、已有活动轮次、等待人类、归档或已失效的唤醒不充当阻塞队首；有效唤醒只因公平性等待时保持 queued，不走撤销路径。普通无父会话不参与子槽排队，仍可独立启动。

领取前的 Core 加载、集成设置同步或恢复消息边界校验失败时，当前 Turn 必须持久化为 failed，记录 Session、Turn 与失败原因，再释放预留槽。失败任务不保留全局队首优先级，不需要重启或人工取消才能继续其他父会话的任务。协调器据真实失败终态生成结果并阻塞依赖后继，不创建虚构 Task 或成功报告，不覆盖损坏的 Core 文件；状态持久化自身失败仍向上传播。

账本事务仍在独占锁内读取最新状态并执行操作；无数据变化时不重写文件，首次创建、真实状态变更和 schema 迁移仍须原子落盘。周期协调跳过错过的 tick，避免恢复后集中追补旧扫描。此约束用于防止容量队列中的重复提示和空事务同步磁盘拖延无关父轮次，不改变四执行槽、十六等待或停止语义。

## P2 接口约定

下列命名供本轮各实现面共同使用，最终参数以协议源码为准：

- `ChildSessionControlState`：`delegated | user_owned | detached`。Link 新增 `control_state`、`control_revision`（旧关系默认为 delegated / 1）、`control_updated_at`。查询 Task 投影携带当前 `control_state` / `control_revision`，不复制成独立控制事实。
- 原 Session `parent_session_id` 保留来源、能力和家族归属；Core/Runtime Session 增加默认 false 的 `delegation_detached`，仅解除活动树形投影。会话列表、导航和来源栏用该标志区分当前关系与来源审计；原 fork 来源 `started_by` 保留，不再使已脱离会话缩进。
- `session.child.link.get`：`{ parent_session_id, child_session_id }`，返回 Link。
- `session.child.takeover` / `session.child.detach`：`{ parent_session_id, child_session_id, expected_control_revision }`，使用信封 request_id，返回 Link。两者阻止未来父控制与新依赖启动，但不取消已经存在的受理 child Turn；尚无 Turn 的依赖等待任务转为 blocked。已接管可继续脱离；不提供恢复父控制。
- `ChildSessionSubscription`：`none | inbox | auto_wake`。start/continue 新增可选 `subscription`，缺失时沿用旧 notify 推导；Task 投影返回有效 `subscription` 和 `subscription_revision`（默认 1）。旧 `notify` 枚举保持 `none | on_task_terminal`，不增加旧客户端不能识别的枚举值。
- `session.child.task.subscription.set`：`{ parent_session_id, assignment_id, expected_subscription_revision, subscription }`，返回 Task。策略作用于尚未消费的该 assignment 结果，不更换 notification_id、不撤回已消费内容、不解除父停止抑制。仅用户控制入口开放该修改操作。
- start/continue 新增 `depends_on: UUID[]`，默认空，最多 16 项；依赖创建后不可修改。retry 沿用原依赖，仍引用原 assignment，不自动改绑。引用只接受已经存在且同父、仍受委派控制的 assignment，因此新建路径不能构成环；同时校验自引用、重复与历史图环。
- Task 新增 `depends_on` 和默认 false 的 `waiting_on_dependencies`。等待期间保留兼容状态 `preparing`，无 child Turn；P2 客户端显示为等待前置任务。该任务占等待容量，所有依赖成功后复用固定 Turn 请求身份入队；前置非成功终态或控制关系撤销时 blocked。
- 新能力 `session_children_control`、`session_children_subscriptions`、`session_children_dependencies`。新客户端仅向对应能力宿主发送 P2 字段；旧枚举和无 P2 字段请求保持原行为。私有 Web 的 `GET /api/sessions/{parent}/children/capabilities` 返回 Runtime 能力；任务列表和 Link 查询也附带能力投影，供旧页面按缺失字段安全降级。
- 私有协调账本升级为 schema 2，新 Runtime 兼容读 schema 1 并保留迁移备份；旧 Runtime 应拒绝 schema 2，避免降级抹掉接管/依赖状态。

## 验收要求

Rust 原生单测覆盖旧账本兼容、并发控制、接管/脱离与创建/停止/结果投递竞争、空唤醒、依赖成功与失败、容量释放、重启、授权及家族归属。Web 交互测试覆盖加载、失败、旧宿主、草稿、控制按钮和依赖展示。Runtime/API/CLI 验收优先 Ruby，生成 JSON 与 Markdown 报告；使用临时 WILLDEEP_HOME、合成工作区与回环确定性模型。TUI 使用真实 PTY，Web 通过浏览器预览检查实际页面、交互、窄屏和错误日志。

全部执行结果与未覆盖范围写入版本验收文档。实现与验证不等于提交、推送或公开发布。
