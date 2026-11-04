# 安全与验证产品模块

> 范围：审批、只读硬上限、OS 围栏、公共数据投影、未知副作用、Diff/验证证据、检查点、Hook 与审计。开发版本见[产品概览](../../PRODUCT_OVERVIEW.md)。具体规则、配置和协议以专题文档为准。

## 能力概要与用户入口

WillDeep 将“能否执行”“执行后发生了什么”“结果是否验证”分成不同证据。审批与工作区策略决定动作是否可开始；OS 围栏限制进程可触达范围；Runtime 持久记录工具、快照、验证和人工裁决；TUI/Web/CLI 只在各自授权范围内展示或操作。

用户入口包括 TUI `/permissions` 与 Shift+Tab、Web 授权面板、Runtime 审批/问答入口、TUI `/diff`、CLI Diff/审计命令、`willdeep doctor` 及 `willdeep audit export`。

## 已实现范围

### 审批、工作区与命令边界

- 会话审批分 `strict`、`smart`、`workspace-write`（默认）与 `full-access` 四档。strict 逐次确认；smart 允许工作区写并对 Shell 采用静态分类与 AI 判官；workspace-write 还以 OS 围栏保护工作区内命令；full-access 仅免去可确定范围动作的常规审批，明确破坏性形态、越界路径、未知表达式和人工问题仍需人处理。
- Runtime 的 `read_only` 是工作区策略而非第五个会话档位：Shell、文件写入、Worktree、MCP 及写入型 Worker 在审批前直接拒绝，客户端切档或请求参数不能越过。完整子会话的共享只读模式还会持久施加独立只读上限，父会话放宽权限不传递；worktree 子会话最多获得工作区可写，不继承父会话完全访问，且所有子会话都不能创建下一层子会话。
- 命令审批采用确定性静态分类，再对适合自动判定的上下文调用 Safety Judge；判官只可减少打扰，拒绝、畸形、超时或不可用均回落人工，不扩大权限。危险命令、凭据内容和敏感路径不送给判官。
- Always Allow 绑定窄作用域，不能把一次精确授权扩成参数拼接、命令替换或任意 MCP 权限。MCP、副作用网络请求及跨工作区操作仍按各自规则处理；非交互场景无法取得批准时明确拒绝并返回稳定失败结果。
- 审批与提问按会话/任务归属排队，跨会话不能代答。Web、TUI、手机和后台任务处理的是同一 Runtime 对象，但来源、版本与目标归属仍重新校验；“看见摘要”不等于获得批准。

### OS 围栏、网络与诊断

- macOS Seatbelt 与 Linux bubblewrap 在可用时提供工作区、临时目录和工具链缓存的写入围栏；read-only 同时禁止网络，workspace-write 默认允许网络但仍走审批策略。`willdeep doctor` 报告后端、开关及退化状态，配置要求沙箱却无后端时拒绝启动受保护命令。
- 围栏是审批后的第二道边界，不是容器、虚拟机或恶意代码的完整隔离。full-access 的语义和平台缺口由[沙箱专题](../SANDBOX.md)维护；不能因诊断显示“可用”就宣称所有平台行为等价。
- Web Fetch 每次跳转重验公网目标，跨域按规则重新审批，并限制环路、次数、超时和响应大小。公共 SSE、手机中继和 Web 活动摘要仅投影有界状态；Prompt、工具参数/输出、内部路径、报告正文和凭据不进入公共 DTO。
- 失败诊断与公共投影分开：本机 `task.diagnostics`、TUI Inbox 或回环 Web 详情在重新校验会话/工作区/任务归属后读取脱敏摘要；公开错误只保留安全分类。

### 未知副作用与恢复

- 工具已调用但结果未持久化时，恢复状态保留原参数和“效果未知”事实，不推断失败，也不沿用广泛自动授权重放同名同参数副作用。需要一次性明确确认后，仍重新经过当前审批、工作区与沙箱限制。
- Runtime 修改操作使用稳定 Request ID 和 Pending→Completed 幂等日志；明确成功可复用，不确定崩溃窗口拒绝自动重放。工作流、Worker、后台 Shell 与完整子会话分别保留自己的身份和恢复账本，不能用另一对象的终态替代。
- Daemon 重启把无法证明仍运行的 Child Agent、Tool、后台 Shell、Task/Turn 收敛为可解释的 Interrupted/Failed/Rejected 状态。只对无工具活动且历史边界完全匹配的 Turn 自动重放；不承诺恢复任意进程、网络请求或外部副作用。

### Diff、验证与回退证据

- Runtime Diff 快照包含 HEAD、暂存/未暂存/未跟踪范围、二进制标记与内容指纹。文件内容、审查、归因、验证、Commit Preview 和撤销都绑定精确 Workspace/Snapshot ID；变化后拒绝陈旧结果。
- 潜在写工具调用窗口前后采集工作区指纹，形成 `Session → Turn → Task → Agent → Tool → Paths` 的归因证据。共享工作区并发只能提供 Tool Window 置信度；强隔离依赖专属 Worktree，不能把时间相邻当成绝对作者证明。
- 识别的测试命令以退出码、摘要和精确 Diff 快照记录验证。退出码 0 但验证期间工作区变化记为 `invalidated`：它既不是失败，也不能作为通过证据。超时、未启动和未验证分别保留，不补成零或成功。
- 安全撤销先校验快照并二次确认；未跟踪或 HEAD 不存在的内容移入 Recovery 而非直接删除。`/rewind` 在每轮前的私有影子仓库快照上恢复对话和可选文件，覆盖前再保存当前内容；审批、Diff 归属和审计事件作为已发生证据不会被回退删除。
- 计划卡、模型报告、任务终态和界面绿色状态都不是独立验收。自然语言验收、测试命令、最终快照和用户裁决必须按各自证据类型保存。

### Hook 与审计

- 生命周期 Hook 运行在关键路径，可按配置记录或拦截 `pre_tool`/`post_tool` 等触发点；失败默认策略和宿主级信任边界见[Hooks](../HOOKS.md)。Hook 不等于旁路通知，也不自动进入 OS 沙箱。
- `willdeep audit export` 只读本机状态，汇总审批来源、人工裁决、Hook、验证、改动归属、审查与 Recovery，输出 Markdown/JSON；不启动 Runtime、不联系 Provider，也不导出 Prompt、模型正文、工具载荷或凭据。
- 审计归入方式会标明精确 session ID 或旧记录的时间窗匹配；验证证据去重，未验证与失败分开。报告能说明已有记录，不能证明未记录的外部动作从未发生。

## 当前限制

- Hook 的 `approval_resolved` 事件类型已有定义但尚未接线；不能把审批日志存在误写成该 Hook 已触发。
- Windows 与 macOS/Linux 的 OS 围栏后端能力不等价；doctor 结果和专题说明是平台判断依据。
- 任务级自然语言验收、全部写入口的一致性及共享工作区并发强归因仍有未完成边界；Computer Use Helper/AX 短效控制租约尚未开放。
- 手机没有 macOS 同型“补丁提案”对象；跨端查看 Diff 不等于跨端提案审阅流程已经补齐。

## 详细指南与证据

[审批与自动化](../APPROVALS.md)、[OS 围栏](../SANDBOX.md)、[Hooks](../HOOKS.md)、[审计导出](../AUDIT_EXPORT.md)、[检查点回退](../CHECKPOINT_REWIND.md)、[Runtime 控制 API](../RUNTIME_CONTROL_API.md)和[会话协议](../RUNTIME_SESSION_PROTOCOL.md)是详细事实来源。Worker/Worktree 见[智能体与协作](agents-collaboration.md)，公网与手机边界见[集成](integrations.md)，历史矛盾和旧待办保存在[历史快照](history/overview-through-0.142.0-rc1.md)。
