# 完整子会话

> 当前版本：0.144.0-rc1
>
> 状态：P0/P1/P2 已实现。P2 的具体约定见[控制权、通知与依赖设计](decisions/2026-11-05-child-sessions-p2.md)，实际执行结果与平台限制见[本版验收记录](verification/0.144.0-rc1.md)。

完整子会话适合把一项调查、阅读或分析交给另一条完整对话。它有独立的 Session、root Agent、聊天历史与执行轮次；你可以打开它、补充信息、回答问题，并在之后继续使用。父会话收到的是具体委派的结果，不是“这条会话以后所有工作的完成通知”。

## 支持的模式与边界

子会话的上下文有两种模式：

- `fresh`：从空聊天开始，只接收标题与有界任务简报。不复制父会话的聊天、Goal、计划、队列、审批对象或后台句柄。
- `fork`：从父会话一个**已完成 Turn** 的不可变消息边界继承历史，再开始新任务。只复制该边界前的消息与内联附件，校验工具调用已全部闭合、压缩代次一致；Goal、计划、压缩检查点、执行检查点、通知已读与运行游标不会复制。父会话正在执行时也可以 fork 已完成的旧边界，不等待父 Turn 结束；边界之后的内容不会泄露。

工作区有两种模式：

- `shared_read_only`：与父会话共享目录，子会话持久只读。可用读取、检索与报告工具，不能写文件、运行 Shell、使用 Git 进程工具、通过 MCP/插件产生副作用、派 Worker 或继续创建子会话。打开后继续聊天、切换审批偏好或重启 Daemon 都不会移除该上限。
- `worktree`：在任务取得执行槽后创建独立 Git worktree，子会话可写、可运行命令、可派 Worker。默认从已提交 HEAD 建立；`snapshot` 基线会冻结父目录当前的已跟踪修改与未跟踪文件并记录补丁摘要。父目录未提交修改不会被静默带入。子会话权限最多到工作区可写，不继承父会话的完全访问；任务结束保留 worktree 与分支，不自动合并、提交、推送或删除。

可选的模型覆盖只接受父会话 Provider 配置中可用的模型，并在创建前校验工具能力、图片兼容性与真实上下文窗口。子会话按目标模型记录窗口，不继承父模型在 Profile 里的窗口数值。

共享只读目录仍可能被父会话或其他进程修改，读到的文件不构成一致快照。需要固定基线、写代码、构建或执行命令验证的任务，应使用 worktree 模式或留在有授权的主会话处理。

| 对象 | 适用工作 | 用户入口 | 结果归属 |
|---|---|---|---|
| 完整子会话 | 能独立推进、需要保留完整对话的调查或编码任务 | 普通会话列表，可打开、插话和继续 | 每次委派有独立 assignment 与 Turn |
| 专业 Worker | 当前会话内有边界的专业派工 | Agent/Run、职责、工具时间线与报告 | 原有 Worker 调度与报告链 |
| 手动 fork | 从既有完成边界继续另一条对话 | 原有分叉入口 | `started_by` 记录历史来源，不自动订阅委派结果 |

活动树形关系、任务提出者/创建者和历史来源是独立概念。`parent_session_id` 保留原父来源，脱离状态决定是否继续树形展示。例如子会话发现的新工作，未来可以作为父会话下的同级任务提出，不必递归派生成孙会话。当前仍不开放子再派生，也暂不提供兄弟任务提案入口。

## 创建与打开

在父对话中说明目标、背景和交付要求，例如“请开一个只读子会话，调查配置加载顺序，给出文件引用与尚未核实的问题”或“请在独立工作树里实现并验证这个改动”。父 Agent 使用 `start_session`，立即获得持久任务身份后可继续自己的工作；不要把返回 `queued` 理解成子会话已经读完或已经完成。需要继承已完成历史时传 `context=fork` 与 `through_turn_id`；需要写代码时传 `workspace_mode=worktree`。

创建后，Web/TUI 会话列表按父关系展示完整子会话；选择该会话即可打开聊天。创建不会自动切换当前对话或清除草稿。父条目不在当前搜索结果或分页中时，子会话仍应能独立访问。也可以按返回的 child Session ID 从命令行打开：

```bash
willdeep --resume <child-session-id>
```

会话聊天中的普通后续输入只属于该会话，不自动成为旧 assignment 的新任务，也不重复向旧父任务回报。要再次委派并接收结果，使用下面的续做入口。

Web 父聊天中可展开子会话任务区，选择上下文（全新/继承已完成历史）、工作区（共享只读/独立工作树）、工作树基线（HEAD/冻结未提交修改）和可选模型，查看状态与报告，再打开完整子聊天。父聊天正在运行时，打开动作通过新页面保留原聊天流与草稿；子聊天提供返回父会话入口。任务简报和结果仅通过本机私有接口提供，子会话不允许创建公共分享，已有公共分享也不能读取或写入子会话。

CLI 的完整子会话入口位于 `session child`：

```bash
willdeep session child start --parent <parent-session-id> --title "调查配置加载顺序" "只读检查配置入口，给出文件引用与未核实项。"
```

```bash
willdeep session child start --parent <parent-session-id> --context fork --through-turn <completed-turn-id> --workspace-mode worktree --worktree-base snapshot --child-model <model> --title "在独立工作树继续" "基于继承的历史实现并验证改动。"
```

```bash
willdeep session child list --parent <parent-session-id>
```

```bash
willdeep session child get --parent <parent-session-id> <assignment-id>
```

```bash
willdeep session child continue --parent <parent-session-id> <child-session-id> "继续核对环境变量的优先级。"
```

```bash
willdeep session child retry --parent <parent-session-id> --expected-result-revision 1 <assignment-id>
```

```bash
willdeep session child stop --parent <parent-session-id> <assignment-id>
```

创建、续做、重试和停止接受 `--request-id <uuid>`。响应未确认时，保留同一请求 ID 与原参数重试，避免生成重复任务。

TUI 中使用以下命令；`open` 和 `parent` 仅切换聊天，不提交新的模型轮次：

```text
/session children start 调查配置加载顺序 | 只读检查配置入口，给出文件引用与未核实项。
/session children start --fork <completed-turn-id> --worktree --snapshot --model <model> 独立工作树实现 | 基于继承的历史实现并验证改动。
/session children list
/session children show <assignment-id>
/session children open <child-session-id>
/session children parent
/session children continue <child-session-id> 继续核对环境变量的优先级。
/session children retry <assignment-id> <result-revision>
/session children stop <assignment-id>
```

## 查询、续做、重试与停止

| 操作 | 使用方式 | 语义 |
|---|---|---|
| 查询具体任务 | 父对话使用 `get_session_task`；API `session.child.task.get` | 返回 assignment 实际状态、结果及通知信息 |
| 查询任务列表 | API `session.child.task.list` | 按父会话分页，返回 `tasks`、`next_cursor`；默认一页 50 |
| 续做 | 父对话使用 `continue_session`；API `session.child.continue` | 保留 child ID 和历史，创建新 assignment/Turn；目标忙碌时拒绝 |
| 显式重试 | 父对话使用 `retry_session_task`；API `session.child.retry` | 对 partial/failed/interrupted/blocked 提供原 assignment 与 `expected_result_revision`；新任务关联旧任务，旧结果保留 |
| 停止具体委派 | API `session.child.task.stop`，或打开子会话停止目标轮次 | 校验父关系和 assignment/Turn；不停止无关会话 |

已有 CLI 观察和 Turn 停止入口也适用于子会话：

```text
willdeep daemon session <child-session-id>
willdeep daemon turns <child-session-id>
willdeep daemon turn <child-turn-id>
willdeep daemon stop-turn <child-turn-id>
```

`preparing`、`queued`、`running` 和等待审批/回答都不是完成。`completed`、`partial`、`failed`、`cancelled`、`interrupted`、`blocked` 描述本次 assignment 的终态，不代表整条 Session 永久结束。取消后的再次执行使用明确续做，不自动重试；重试前应读取当前结果 revision，过期版本和忙碌目标会被拒绝。

## 回报、停止父会话与恢复

默认 `notify=on_task_terminal`，本次 assignment 进入终态后向父会话投递结果；`notify=none` 不自动回报，仍可查询。结果状态来自 Turn/最终 Task，模型报告只是待核验的工作内容；失败或没有最终正文时也应如实显示，不能编造成功摘要。摘要最多 2,000 个 Unicode 字符，完整性由结果字段标明。

父会话正在运行时在安全边界接收，空闲且允许续推时通过持久唤醒批次恢复处理。通知可能重放，Runtime 按稳定身份去重；客户端已读或收到事件不等于父模型已经处理。父会话普通轮次结束不取消已受理子任务。

主动停止父会话只停止父目标轮次，并持久抑制父自动续推。子会话继续运行，结果保留待处理；只有你明确继续父会话才解除抑制，子结果和协作消息不会自动解除。要停止子任务，需另行停止对应 assignment/Turn。父会话归档或删除不级联删除子会话，也不自动唤醒父会话。

Daemon 重启按协调账本中的固定请求、assignment、Turn、通知与批次身份对账；不能证明安全的在途执行显示中断并等待明确继续。已有结果和聊天保留，不因断线或无报告宣称成功。子任务容量同时受家族与宿主限制，执行槽 4、等待队列 16，等待审批/回答仍占执行槽；分页页数不改变容量。

## 用户接管与脱离父关系

用户接管后，原父会话保留任务历史与打开聊天入口，但不能继续委派、重试或停止该关系的任务，也不再自动接收它的新结果。已经存在的 child Turn 继续执行，接管本身不发停止、不代替你处理审批；尚未派发的依赖等待任务收敛为阻塞。

脱离在接管基础上让会话独立显示；原父来源、fork 来源、聊天、assignment 和 worktree 都保留。只读上限、原项目授权及家族预算不变。内部 `parent_session_id` 仍保存来源与额度归属，`delegation_detached=true` 解除当前树形关系；客户端不能再按 `started_by` 把它重新缩进。

如果 worktree 任务还在等待依赖，接管或脱离不会替它执行原委派。用户在 TUI 中明确打开该会话时，宿主会按原持久绑定与授权准备真实 Git worktree，再切换聊天；该操作不创建模型轮次、不恢复父控制，原阻塞任务仍保留。共享只读会话不会因此创建工作树或获得写权限。

Web 在任务卡或子聊天来源处提供明确确认的接管/脱离入口。控制请求携带 `expected_control_revision` 和稳定 request ID；旧页面与并发请求发生冲突时，应先重新读取关系。模型工具不能发起这两项用户控制操作。

CLI 先读取当前关系版本，再按该版本确认操作；`--yes` 只确认这次控制权变化，不扩大工具权限：

```bash
willdeep session child link --parent <parent-session-id> <child-session-id>
```

```bash
willdeep session child takeover --parent <parent-session-id> --expected-control-revision <revision> --yes <child-session-id>
```

```bash
willdeep session child detach --parent <parent-session-id> --expected-control-revision <revision> --yes <child-session-id>
```

TUI 对应 `/session children link <child-id>`、`takeover <child-id> --expected-control-revision <revision> --yes` 和 `detach <child-id> --expected-control-revision <revision> --yes`。未确认时只显示说明，不变更控制权、不发起模型轮次。

## 通知订阅与前置任务

每次 assignment 可选择三档 `subscription`：

| 策略 | 结果可查询 | 自动送入父模型 | 空闲自动续推 |
|---|---|---|---|
| `none` | 是 | 否 | 否 |
| `inbox` | 是，保留待处理提示 | 否 | 否 |
| `auto_wake` | 是 | 在安全边界投递 | 服从停止抑制与限流 |

未指定时兼容旧 `notify`：`on_task_terminal` 对应 `auto_wake`，`none` 对应 `none`。用户修改订阅时携带 `expected_subscription_revision`，策略作用于该任务尚未消费的结果；可补投已有结果，但不会更换通知身份、重写报告或撤回已经消费的历史。切换订阅不解除父会话的停止抑制，接管/脱离后也不能重新启用原父控制。

CLI 的订阅值使用 `none | inbox | auto-wake`；API 与 TUI 使用 `auto_wake`。先读取任务中的 `subscription_revision`，再修改该次委派的策略：

```bash
willdeep session child subscription set --parent <parent-session-id> --expected-subscription-revision <revision> --subscription auto-wake <assignment-id>
```

创建或续做可提供 `depends_on`，引用同一父会话已经存在的 assignment，最多 16 项。支持串行、扇出与多前置汇合，依赖集创建后固定。只有全部前置任务成功，当前任务才会启动；前置失败、取消、中断、部分完成、阻塞或控制权被撤销时，后继明确阻塞，不自动改用重试产生的新 assignment。

```bash
willdeep session child start --parent <parent-session-id> --title "汇总调查结果" --subscription inbox --depends-on <first-assignment-id> --depends-on <second-assignment-id> "等待两项调查成功后汇总证据。"
```

等待依赖时状态保持兼容的 `preparing`，并返回 `waiting_on_dependencies=true`；界面显示“等待前置任务”。此时无 child Turn，不调用模型、不创建 worktree，计入等待队列但不占执行槽；可以直接停止等待中的 assignment。重启恢复沿用固定请求与任务身份。

## Runtime API

先从 `GET /v1/capabilities` 检查 `session_children`、`session_children_fork`、`session_children_worktree`、`session_children_model`、`session_children_workers` 与具体 `operations`。P2 操作分别检查 `session_children_control`、`session_children_subscriptions`、`session_children_dependencies`；能力查询失败时保留依赖与通知草稿，确认能力后再提交。调用走带本机 Runtime Token 的 `POST /v1/api`；不要把 Token 写进脚本、日志或示例文件。旧 Runtime 不支持时按普通会话访问历史，不应自行拼接 `session.create` 与首条消息模拟委派。

创建示例采用统一信封，以下 UUID 是占位值；实际调用需使用已存在的父 Session ID 和本次请求的新 UUID。`context=fork` 时必须提供 `through_turn_id`；`worktree_base` 只对 `workspace_mode=worktree` 有意义；`model` 可省略：

```json
{
  "protocol_version": "1.0",
  "request_id": "00000000-0000-4000-8000-000000000001",
  "operation": "session.child.start",
  "params": {
    "parent_session_id": "00000000-0000-4000-8000-000000000000",
    "title": "调查配置加载顺序",
    "prompt": "只读检查配置入口，给出文件引用、加载顺序和未核实项。",
    "context": "fresh",
    "through_turn_id": null,
    "workspace_mode": "shared_read_only",
    "worktree_base": "head",
    "model": null,
    "notify": "on_task_terminal"
  }
}
```

响应丢失后使用原 `request_id` 和完全相同参数重放；同键异参会被拒绝。创建/续做的 prompt 上限为 16,000 个 Unicode 字符，超限应缩减简报，不能依赖静默截断。`created_from_turn_id` 为可选来源审计，模型调用由宿主绑定父会话、Turn 和请求身份，不接受模型伪造这些字段。fork 的 `through_turn_id` 必须属于父会话且已完成；worktree 要求父会话工作区是 Git 仓库根目录，非 Git 或子目录会在创建前被拒绝。完整操作参数见 [控制 API](RUNTIME_CONTROL_API.md)，唯一 DTO 定义见 [协议模块](../crates/willdeep-runtime-protocol/src/child_session.rs)。

## 验收与当前边界

本轮验收使用临时 `WILLDEEP_HOME`、合成 Git 项目与 mock Provider，无远端付费模型调用，不沿用个人配置发起真实请求。Rust、严格 Clippy 与构建、Web 测试与构建、隔离 Runtime/API/CLI、宽窄真实 PTY 及浏览器的实际结果见 [0.144.0-rc1 验收记录](verification/0.144.0-rc1.md)，P0/P1 历史验收见 [0.143.0-rc1](verification/0.143.0-rc1.md)。首次失败、重跑和未覆盖项目分别记录，不将尚未通过的检查计为通过。

正常使用子会话会产生所配置模型的调用与用量；只读、容量限制或 mock 验收不表示真实使用免费，也不承诺严格费用封顶。worktree 子会话的 Worker 与父会话共享家族并发、排队和预算，新增子会话不会重置额度；只读子会话仍然禁止 Worker。所有子会话都不能创建下一层子会话。真实模型表现、付费调用、Android/Xedit 新控制入口及 Linux/Windows 原生执行不在本轮已验证范围。

完整方案、恢复竞态与后续阶段见 [设计决策](decisions/2026-11-04-child-sessions.md)。
