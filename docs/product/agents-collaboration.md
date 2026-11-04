# 智能体与协作产品模块

> 范围：完整子会话、专业 Worker、声明式工作流、智能体库、独立会话协作、后台任务与 Worktree 派工。开发版本见[产品概览](../../PRODUCT_OVERVIEW.md)。

## 能力概要与用户入口

| 对象 | 工作范围 | 主要入口 | 当前边界 |
|---|---|---|---|
| 完整子会话 | 独立 Session/root Agent/历史/Turn，可持续对话 | `session child`、TUI `/session children`、Web 父聊天任务区与会话树 | P1 fresh/fork、共享只读或独立 worktree；已实现但尚未发布 |
| 专业 Worker | 当前会话内的有界任务、执行 Run 与报告 | `spawn_agent`、Agent/Run 查询与详情 | 单层团队，职责/工具/写集合/预算受限 |
| 手动 Fork | 从完成的历史边界另开一条对话 | 会话分叉入口 | 记录历史来源，不自动订阅委派结果 |
| 独立会话协作 | 同宿主同项目会话间协调消息 | TUI `/peers`、Web/手机收件箱 | 已读、模型消费和执行结果分开记录 |

这些对象不能互换：打开另一会话不等于派工，发送协作消息也不是新的用户授权。共同 Session/Turn 与恢复契约见[Runtime 模块](runtime-sessions.md)。

## 已实现范围

### 完整子会话 P0/P1

父会话可委派调查并立即继续自身工作；用户能打开子对话、插话、查询、续做、显式重试及停止具体委派。每次 assignment 绑定具体 Turn，任务终态不表示整条 Session 永久结束，普通后续聊天也不自动成为旧 assignment 的新任务。

P1 支持 `context=fresh|fork` 与 `workspace_mode=shared_read_only|worktree`。fork 只复制已完成 Turn 的不可变消息前缀、闭合工具调用与内联附件，清空 Goal、计划、压缩/执行检查点、通知已读与运行游标；shared 模式提供有界简报并持久施加独立只读上限，不开放 Shell、文件写入、Git 进程工具、MCP/插件副作用、Worker 转派及子再派生。worktree 模式在取得执行槽后创建独立 Git 检出，默认从 HEAD 建立，`snapshot` 基线冻结父目录已跟踪修改与未跟踪文件；权限最多工作区可写，可派 Worker，父子共享家族并发/队列与按记录 ID 去重的用量，不自动合并、提交、推送或删除。模型可在父 Provider 配置内覆盖，窗口按目标模型记录；所有子会话都不能创建下一层子会话。

协调账本持久保存关系、创建意图、稳定请求身份、assignment/Turn、结果 outbox 和父唤醒批次。结果以 Turn/Task 为准，模型报告待核验；重放按稳定身份去重，事件已收/客户端已读不等于父模型已处理。停止父会话不取消已受理子任务，但持久抑制父自动续推、保留待处理结果；明确继续父会话才解除抑制。

P0/P1 已通过既有本机隔离 Rust/Web、Runtime/API/CLI、宽窄 PTY 与浏览器验收，本次提交尚未发布；真实模型表现和其他原生平台未获本轮证明。P2、子再派生和兄弟任务提案入口仍属后续阶段。任务提出者/创建者与树形 parent 独立。参数、家族容量及恢复竞态只在[完整子会话指南](../CHILD_SESSIONS.md)与[分期方案](../decisions/2026-11-04-child-sessions.md)维护。

### 专业 Worker、预算与恢复

- `spawn_agent` 前台或后台派工，`continue_agent` 保留 Agent 历史/权限并创建独立 Run。Runtime 提供空闲续做、队列恢复和分页历史；前台恢复列表超过 4096 条仍有界。
- 五个公开职责为 `generalist`、`implementer`、`tester`、`reviewer`、`ops_runner`；模型档为 `standard/advanced/expert`，与职责正交。旧 `reader/judge/deep` 等 ID 留作兼容，不能把 Deep 升档当第六公开职责。专家档仍需升级票据和每 Harness 预算，默认模型/映射见[模型档位](../MODEL_TIERS.md)。
- Worker 与工作流叶节点共用父会话 Scheduler，默认并发 8、排队 64，由 `[agent_team]` 调整。后台派工与重试共用准入；独立写文件可并行，同路径写集合互斥，不因自动只读派工获得写权限。
- Task Packet 明确目标、事实、约束、读取上下文、精确 `write_files` 与验证命令。写入型最多声明 16 个文件，strict 整集合审批，smart/workspace-write 继承已授权写范围，只读仍禁止；越界拒绝并说明扩权路径。Verifier 实际执行命令，退出码裁决，失败回灌重试，耗尽如实失败。
- Profile 保留上下文窗口、最大轮次、Token 总预算、执行超时与连续失败熔断；本机智能体预算在档位选择后应用。团队统计包含主/子 Agent、压缩和辅助请求，可选团队预算阻止超额新请求；在途并发可能超额，用量未知或不完整时停止新请求。
- Agent 树记录实际模型、状态、工具、时长、Token、Worktree 与跨 Turn/重试/重启累计。缺少开始时间不以存续时间冒充任务耗时。受保护详情可查看有界报告（最多 64KiB），公开候选不下发 Prompt 原文或报告正文。
- 主 Agent 可发有界补充指令、停止本会话后台 Worker，下一 Provider 请求边界生效；终态后台 Worker 可原模型或指定新模型重试，运行中不热切。取消当前会话持有 Job 在 full-access 可免审，跨会话/无主 Job 仍确认。
- 私有目录保存派工、历史与工具输出，恢复沿用原 ID、Task Packet、写集合和工作树；已有前台完成报告直接交回，不重复工具或 verifier。后台取消/失败/重试沿用 UUID 与持久命令审计；重启不能证明完成的执行收敛为 Interrupted，Worktree 原地保留。
- TUI/Web 的公开手动只读入口仍接受兼容 `reader/judge`，不开放 Shell 或写目标；父级、Task、Workspace 与 Child ID 由 Runtime 推导，不能借外部参数指定写入范围。

### 声明式工作流

工作流由 `agent/switch/foreach/repeat` 节点构成，依赖 DAG 决定顺序与并行，阶段仅用于显示。driver 与控制节点不占 Worker 槽，模型叶复用同一基础职责、审批、精确写集合、专家票据、预算和文件租约；多开图不扩大团队额度。

运行在派工前保存 intent、attempt/request ID，重启先对账 AgentRun，复用已确认成功结果；未知副作用标为中断，明确恢复后才继续。重试/恢复建立关联的新运行，保留旧历史，复用不计作新模型调用。整组停止及节点控制通过所属工作流，不能由普通 Worker 控制接口绕过。完整图、硬限和实际验收见[工作流](../WORKFLOWS.md)。

### 智能体定义与会话协作

- CLI `agents`、TUI `/agents`、Web 智能体库管理自定义定义、签名官方固定版本、导入导出、fork/pin、历史及回滚。Web 支持精选目录、搜索、来源筛选与安装状态；Hermes config.yaml/SOUL.md 与 Multica JSON 可转换，未映射字段明列，外部 Runtime、技能与预算不伪装成已接通。
- some.im 服务账号与模型 Provider 分开；私有定义需显式加入账号同步并主动同步，缓存按 origin/user/tenant 隔离。不兼容定义可见但停用。冲突用本机 revision 与云端 ETag 绑定用户选择，保留两支历史；账号/版本变化中止操作，云端删除可明确接受或恢复。
- 对话 `agent_library_read/manage` 支持读取、创建、更新、fork、绑定、启停；按宿主 scope、request 收据及 `installation_revision` 原子校验。官方正文先 fork；保存并刷新后本轮后续新派工用新快照，在途/续做/恢复 Worker 保留原定义与绑定，当前权限和预算重新核验。本机安装元数据不进入 v1 交换包。
- 项目范围的持久协调收件箱支持来源、关联回复、独立已读及模型消费回执，TUI/Web/Android 共用；忙时安全边界消费，用户主动停止后只提醒。模型来信不冒充人类授权，分工声明不等于文件锁。跨宿主联邦另见[集成模块](integrations.md)。

### 后台 Shell、监视器与 Worktree

后台 Shell Job 结束/失败可唤醒主 Harness；Supervisor 通过匿名帧管道收命令，命令不进进程参数/资源索引，Unix 在取消、超时或父端断开时终止进程组。明确 ≥30 秒 sleep 开头的非验收命令可自动后台，保留审批、顺序、工作目录、超时和总预算；动态或短等待不自动转换。

`monitor` 合批 stdout，在命令运行时按有界事件通知模型，结束、超时、刷屏或取消后报告终态，完整日志落盘；与命令共用审批，唤醒有频率限制。具体时间/数量上限见[后台任务专题](../SUBAGENTS.md)。

写入型 Worker 的专属 Git Worktree 与完整子会话 worktree 不同：前者由父会话内的 Worker 派工产生，后者是完整 Session 的执行空间；两者都保留 Diff 归因和结果。两阶段 Review/Merge 绑定 Child/Root 精确快照，变化、同文件冲突、未跟踪内容、未解决冲突和超大补丁阻断；审计区分 Active/Reviewable/Merged/Clean/Quarantined/Missing/Unknown，保守清理只将符合条件的终态目录整体移入 Recovery，保留内容、Git 关联与分支。

## 当前限制与进一步阅读

完整子会话只读/工作树边界、单层 Worker 团队和工作流是不同限制，不可互相扩权；恢复记录不保证任意进程或未知副作用自动重放。定义管理不等于 RSI 自动晋升，见[评测与 RSI](evaluation-rsi.md)。

详细事实来源：[完整子会话](../CHILD_SESSIONS.md)、[专业 Worker 与后台任务](../SUBAGENTS.md)、[Skill Worker](../SKILL_WORKERS.md)、[智能体库](../AGENT_LIBRARY.md)、[工作流](../WORKFLOWS.md)、[会话协作实施与最终验收](../SESSION_COLLABORATION_PLAN.md)。历史证据：[0.99 团队验收](../verification/0.99.0-rc1/acceptance.md)、[0.141 工作流验收](../verification/0.141.0-rc1.md)、[0.143 子会话 P0 阶段验收](../verification/0.143.0-rc1-p0.md)、[0.143 子会话 P1 验收](../verification/0.143.0-rc1.md)。
