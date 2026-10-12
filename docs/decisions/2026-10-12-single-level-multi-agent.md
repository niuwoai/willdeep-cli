# 一层多智能体补齐方案

日期：2026-10-12。
状态：0.99.0-rc1 功能已实现，核心、客户端与独立 Runtime 验收通过；发布和安装状态单独核验。见最终验收矩阵。

## 目标与范围

支持主 Agent 派发多个子 Agent；默认同时运行 8 个子 Agent，超出后排队。支持继续同一个 Agent、状态查看与预算统计。子 Agent 不再派生子 Agent，保留现有工具与权限边界。

并发上限只计算子 Agent，不包含主 Agent、Shell 后台任务或 monitor。默认 8 是每个父会话的执行上限，不代表供应商允许 8 个模型请求同时成功。跨会话的模型请求限流仍需独立管理。

## 实施前的代码调查（2026-10-12）

| 路径 | 已有能力 | 本次缺口 |
|---|---|---|
| `crates/willdeep-cli/src/config.rs` | 工种配置、模型档位、单 Worker 轮次/token/超时限制 | 缺少团队调度配置 |
| `crates/willdeep-cli/src/harness.rs` | 创建 Catalog，挂载父会话、状态目录、用量账本和 Goal | 需注入调度策略并恢复团队执行状态 |
| `crates/willdeep-core/src/subagent/catalog.rs` | 派工、重试、工作区/写集合校验、派工记录与报告持久化 | 后台并发常量为 5；超限拒绝，没有调度队列 |
| `crates/willdeep-core/src/background.rs` | 后台执行、取消、重试、消息收件箱、报告交付去重 | 没有 Queued 状态；重试直接 launch；快照展示最多 50 条 |
| `crates/willdeep-core/src/subagent/checkpoint.rs` | 按 Agent UUID 保存独立聊天历史与执行 checkpoint | 可复用历史，但缺少明确的继续执行请求与轮次身份 |
| `crates/willdeep-core/src/subagent/catalog/control.rs` | 对本父会话子 Agent 发消息、等待、停止 | 当前只向运行中的子 Agent 投递消息；等待逻辑只识别 Running |
| `crates/willdeep-cli/src/daemon/agent_control.rs` | Runtime 命令持久化、控制 watcher、子任务归属校验 | 依赖父任务仍活跃；父会话空闲时不能直接继续子 Agent |
| `crates/willdeep-cli/src/daemon/agent_store.rs` | Agent 树、状态、轮次、工具、模型、累积用量 | 需区分 Agent 累计统计与本次 Run 统计 |
| `crates/willdeep-runtime-protocol/src/lib.rs` | `AgentStatus::Queued`、Agent 查询/等待/控制协议 | Runtime 的 Queued 尚未贯通核心队列；需补 Run 与团队概览 |
| `crates/willdeep-core/src/usage_ledger/` | 父 session_id、子 agent_id 的用量归属 | 增加 run_id、团队统计和预算范围 |

### 必须同时修正的边界

1. Catalog 的并发检查从 `snapshots()` 统计 Running；该接口只返回最近 50 条，不能承担准入控制。现有 `running_subagents()` 遍历完整记录，可用于观察，但“计数后启动”仍非原子准入。
2. 内存重试经 `background.retry_agent()` 直接调用 launcher，不重新经过 Catalog 的并发检查。新建、重试、恢复、继续必须经过同一个调度器。
3. 核心后台状态缺少 Queued；若只在 future 内等待 semaphore，前端会把排队任务误标成 Running，超时和等待逻辑也会错误。
4. `resume_agent` 当前用于前台中断恢复；已有完成报告会直接交回，不再次运行。不能把它简单改名为“继续”。
5. 子 Agent checkpoint 已保存历史，不需要实现第二套会话存储。继续时应追加新请求，而不是再次追加原始任务提示词。
6. Runtime 当前允许对 Completed 子 Agent 发起 Retry，但必须有活跃父任务承接命令。父会话空闲时需要新的执行入口。
7. AgentStore 在再次 Started 时重置轮次和报告，保留累积用量。若直接拿累积用量和单次预算相减，剩余预算口径会错误。

## 调度设计

### 配置

新增 `[agent_team]` 配置段，与 `[subagents.<工种>]` 分开，避免把调度配置误当成工种。

```toml
[agent_team]
max_concurrent = 8
max_queued = 64
# 可选：整个父会话团队的模型用量预算；不设置时不新增团队 token 限额。
# token_budget = 1000000
```

首期建议 `max_concurrent` 校验为 1～64，`max_queued` 为 0～1024；0 表示不接收等待任务，容量满时明确拒绝。以上队列容量与校验边界为实施建议，只有默认并发 8 是用户明确指定值。

配置创建会话调度器时生效；不对运行中的会话偷偷缩减槽位。状态接口返回该会话实际使用的配置。

### 唯一执行入口

在 core 增加独立 `subagent/scheduler.rs`，管理会话级槽位、FIFO 队列、取消和终态释放。Catalog 的新建、重试、恢复、继续均提交执行记录，禁止绕过调度器直接运行。

- 原子登记执行记录、占用槽位或进入队列，不能依赖展示列表计数。
- 同一 Agent 同时最多一个未结束的 Run；重复请求通过 request_id 幂等返回原 Run。
- 排队时只保存派工/权限快照，不调用模型，不运行 verifier，不认领写文件，不创建 worktree。
- 出队后创建或验证原 worktree、取得文件锁；启动失败必须释放槽位。首期文件冲突给出明确 Blocked 原因，由父 Agent 决定改派或继续，避免占槽位无限等待。
- Run 超时从开始执行时计算；排队等待另记，不消耗执行超时。
- 取消队列任务不运行任何工具；运行中取消沿用现有收尾路径，释放槽位一次。
- 父任务进入收尾时必须检查 Queued 和 Running，不能只看 Running 就宣布完成。排队和运行任务未结束前，执行宿主保持存活。
- `await_agents` 和 Runtime wait 把 Queued 计入未完成状态；返回当前 Run 的报告与仍在等待的任务，不重复交付历史报告。

## Agent 与 Run 分离

Agent UUID 表示长期身份；每次新建执行、继续或显式重试分配独立 Run UUID。持久记录包含 parent_session_id、agent_id、run_id、request_id、触发原因、队列顺序、权限快照、排队/开始/结束时间、状态、报告、用量与预算。

状态流转：Queued → Running → Completed / Partial / Failed / Cancelled / Interrupted / Blocked。Blocked 若需再次执行，通过显式请求创建新 Run。旧版本记录没有 Run ID 时作为 legacy 记录展示，不编造早期执行历史。

继续、恢复与重试语义分开：

| 操作 | 行为 |
|---|---|
| instruct | 给正在运行的 Run 补充消息，下一个模型调用边界处理 |
| continue | 向同一 Agent 历史追加新任务，创建新 Run，重新排队 |
| recover | 在 checkpoint 所允许的安全边界恢复中断执行，不能盲目重跑结果不确定的工具 |
| retry | 显式再次尝试失败任务；保留历史与约束，不伪装成新任务，不默认自动恢复成功任务 |

继续保持原 Agent UUID、父会话、工种、工作区与已批准写集合。请求增加写范围必须走现有权限校验。已合并、归档或消失的 worktree 不能静默重建到不同代码状态；明确返回不可继续原因。

### 父会话空闲时继续

新增 `agent.continue` Runtime 操作与模型工具 `continue_agent`，TUI 增加 `/agent continue <ID> <任务>`。

- 父 Harness 仍运行时，由其 watcher 接收请求，Catalog 使用既有团队调度器。
- 父会话空闲时，Runtime 为该父会话安排一次专用控制执行，恢复 Catalog 后提交指定子 Agent Run，不要求用户先向主 Agent 发一条无关消息。
- 沿用 Runtime 现有会话串行调度和 session ownership；不得创建两个父 Harness 同时驱动同一个团队。
- 用户从不同客户端发送同一 request_id 时只产生一个 Run。
- 完成报告按 Run ID 交付一次，状态/历史仍可重复查询。

## 状态查看

复用 `agent.list/get/wait` 与现有 TUI 树，新增 `agent.team` 概览和 `agent.runs` 分页历史查询，避免在主 Agent 上下文塞入所有历史报告。

- 团队概览：实际并发上限、运行数、排队数、累计 Agent 数、当前预算范围与用量。
- 子 Agent 列表：任务名、工种、模型、Queued/Running/终态、当前 Run、队列位置、排队时长、执行时长、当前工具、用量。
- 详情：当前报告、验证结论、本次与累计用量、Run 历史；继续/补充指令/停止/重试入口根据状态显示。
- 状态来自 Runtime 持久记录与事件，TUI/Web 不各自推测生命周期。历史分页不会影响准入和完成门禁。

## 预算统计口径

复用现有 usage ledger，按 Run、Agent 累计、父会话团队三级汇总。展示输入、缓存、输出 token、模型调用数、执行时长与对应限额。token 字段分别显示实际总量与预算计量口径，不把缓存命中量重复计入。

- 团队总量包括主 Agent 与子 Agent；辅助模型调用单列，并在可归属时计入同一团队。
- 不把 Goal 已聚合的 Worker 用量与 ledger 再相加，避免重复计费统计。
- Provider 未返回用量时标记不完整；已有数值表示已知部分，缺失值不冒充零。
- 首期 token 统计与限制是交付范围；费用估算只有存在可信模型价格配置时显示，否则标记未知。
- 团队 token 预算可选；单 Run 预算沿用工种设置，Agent 累计用量不在 continue/retry 时清零，继续也不重置团队预算。
- 预算不足时不启动新的排队 Run，也不发起新的模型调用；已在途请求可能导致最终用量超出限额，界面与文档必须如实显示，不能承诺严格费用硬上限。
- 预算计量状态持久化；Runtime 重启后不重新赠送预算。队列因预算暂停需明确原因，不能永久显示为普通排队。

## 重启与兼容

排队记录必须在返回成功前落盘，持久保存 FIFO 顺序和精确请求内容。Runtime 重启后尚未启动的 Run 可恢复到队列；已经运行的 Run 标记 Interrupted，根据 checkpoint 的已知/不确定工具边界处理，不盲目自动重跑。

现有 Runtime pending command 在重启时会被拒绝并清除消息，不能作为 durable Run 队列直接复用。命令负责请求回执，Run 记录负责执行事实，两者以 request_id 对齐。

现有工种配置、旧 UUID、状态查询、usage 命令保留。新增协议字段带默认值，未知历史保持未知。Core 保留 `PARENT_ONLY_TOOLS` 过滤，子 Agent 无派工能力。

## 实施顺序与验收

1. 配置和统一调度：默认 8、FIFO、有界队列、取消、准入原子性、重试也受限；补 Queued 事件和 Runtime 映射。
2. Agent/Run 持久身份和 continue：追加历史、空闲父会话唤起、请求幂等、原权限/worktree 校验、按 Run 交付。
3. 状态/预算 API 与 TUI 展示：当前 Run 和累计统计分开、团队预算持久化、缺失用量明确显示。
4. Web 状态消费及交互：复用同一 Runtime 接口，新增所有可见文案走现有 i18n。

核心自动化验收使用 mock Provider 与可控屏障，不消耗真实 API：

- 提交 12 个任务，最多 8 个 Running、其余 Queued；完成一个只启动一个等待任务。
- 同时新建、重试和继续时仍不超过限额；累计超过 50 条历史不影响准入。
- 排队时模型、工具、worktree 与文件锁操作计数为零；取消排队任务后不执行。
- 继续已完成 Agent 保留 UUID 和历史，新 Run 收到新任务、旧工具结果不丢；重复 request_id 不多执行。
- 父会话空闲可继续，客户端退出不丢 Runtime 队列；同 Agent 并发 continue 不产生两个执行实例。
- 重启保持队列顺序、Run ID 和预算；不确定工具执行不盲目重放。
- Queued 子任务存在时主 Goal 不能完成，await 不提前返回“无任务”。
- 各 Run 用量之和与 Agent 累计、团队已知用量一致，继续/重试不清零；预算不足不启动排队任务。
- Provider 缺失 usage 显示不完整；stop/完成竞争不泄漏槽位、不重复报告。
- 文件锁、独立工作树、工具权限与现有恢复测试继续通过；TUI/Web 实际查看窄屏、多任务和错误状态。

候选版本：0.99.0-rc1。开发分支：feature/single-level-agent-team，基于 develop db336d5；原工作区无关改动未动。

## 最终实现

统一内存 FIFO 与持久 Run 准入，续做、重试、新建共用容量限制；queued 出队后创建工作树，取消释放位置。同 Agent 的事件携带 Run 身份与序号，旧事件不覆盖新执行。父会话提交报告回执后才确认交付，重启恢复排队记录和未交付报告，Running 标为 Interrupted 而不盲目重放。

CLI/TUI 提供 continue-agent、agent-team、agent-runs 与 /agent continue、team、runs；Web 展示实际政策、当前/累计用量和显式分页报告。自动统计不包含报告。累计预算覆盖主、子、辅助与压缩调用；正好耗尽预算的最终答复保留，后续请求和工具执行被阻止。并发在途请求仍可能超额。

完整证据见 [验收矩阵](../verification/0.99.0-rc1/acceptance.md)、[原生 Runtime](../verification/0.99.0-rc1/runtime-team-smoke.md) 和 [Web 生命周期](../verification/0.99.0-rc1/runtime-web-smoke.md)。
