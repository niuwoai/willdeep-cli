# P0/P1、TUI 与工具展示阶段验收

日期：2026-11-17。版本：`0.152.0-rc2`。项目阶段：既有 Rust Runtime 项目迭代。

状态：工程、六类回归、浏览器九例、三套 PTY 和自定义智能体入口已通过；工具展示 PTY 在后续编辑 diff 检查中发现空白显示缺陷，因此本阶段未签发全部通过。修复和最终组合由 [0.152.0-rc3 验收](../0.152.0-rc3/p0-p1-closeout.md)接续。机器可读记录见 [JSON](p0-p1-closeout.json)。

## 范围与结果

原始三项 P0、六项 P1、并行 TUI、队列时间线、普通新输入与显式续跑、历史结束行、成功写入 diff、当前命令展开，以及 Thought 点击修复。配置与边界见 [Runtime 工具指南](../../P1_RUNTIME_TOOLS.md)和[工具展示决策](../../decisions/2026-11-17-tui-tool-visuals.md)。

| 范围 | 本阶段实际结果 | 证据 |
|---|---|---|
| 格式、全工作区测试、严格 Clippy、显式构建 | 四阶段通过；3371 通过、0 失败、24 忽略 | [工程摘要](engineering.json) |
| P0/P1 六类固定回归 | 6 类、21 项执行通过，源码稳定 | [安全摘要](fixed-runtime-regression-final1.json) |
| 浏览器 CLI/TUI/Web 入口 | 各成功、审批取消、执行取消，共 9/9 | 同一安全摘要；已核原报告 |
| TUI 阶段、队列、阅读、选区与草稿 | 两布局共 18/18 | [JSON](tui-interactions-final1/integration.json) · [Markdown](tui-interactions-final1/integration.md) |
| 输入选区与隔离 OSC 52 | 两布局与 SSH 共 14/14 | [JSON](tui-prompt-selection-final1/integration.json) · [Markdown](tui-prompt-selection-final1/integration.md) |
| 两条排队消息时间线 | 两布局共 10/10 | [JSON](tui-queue-timeline-final1/queue-timeline.json) · [Markdown](tui-queue-timeline-final1/queue-timeline.md) |
| 自定义智能体 | 同一固定应用 8/8，专属模型与完整指令均观测到 | [JSON](custom-subagent-final1/custom-subagent-startup.json) · [Markdown](custom-subagent-final1/custom-subagent-startup.md) |
| 工具展示首轮 | 11 项检查通过；两布局在命令二收起检查超时 | [失败 JSON](tui-tool-visuals-final1/tool-visuals.json) |
| 工具展示第二轮 | 17 项通过、2 项失败；编辑 diff 空白损坏，后续检查未执行 | [失败 JSON](tui-tool-visuals-final2/tool-visuals.json) · [诊断](tui-tool-visuals-final2/diagnosis.txt) |

## 工程与专项说明

18 个顶层 target 共 3371 通过、0 失败、24 忽略。原始日志有 19 条结果、3372 通过，多出的单项是 `detached_jobs::tests::spawn_and_exit_helper` 子进程输出，未重复计入顶层总数。格式检查、全目标 Clippy `-D warnings`、显式 CLI 构建均退出 0，日志摘要见 [engineering.json](engineering.json)。

Runtime 新建会话固定默认 Provider/model 时，显式配置优先，否则读取自身 home 下的配置；缺省文件不存在使用内置默认，显式文件缺失仍报错。四项新增回归已在完整工程中全部通过。缺省加载路径不写回显式配置字段，不改变默认集成 Provider 资格判定。手机控制面继续执行真实 `session.create`，切会话与 Web 命令测试绑定临时配置，不修改真实用户配置或并行测试全局环境。

Thought 相关 19 项原生测试通过、0 失败、0 忽略。六项新增回归覆盖完整与简洁布局、归档、滚动、正文伪目标、窄屏标题续行及重绘前移除或重排；点击坐标从实际画面标题独立取得，原始思考不删除。工具展示第二轮已实际点击最后一条折叠思考，两布局均只显示其自身正文并能再次收起；整套工具展示仍因后续 diff 失败而判失败。

自定义智能体的未知配置段继续拒绝启动，诊断增加实际路径、完整段名和中英日修复入口；定义与 Provider/model/窗口绑定沿用智能体库，不放宽白名单。[专门验收](custom-subagent-closeout.md)的原生 95 项及实际日志已核对，最终固定应用又完成 8 项真实入口检查。旧报告保留三次夹具失败与清理修正；旧安装版启动恢复单独记录，不作为 rc2 证据。

Web 52 文件、430 测试通过，lint 0 error、1 条既有 Hook warning，构建通过；此后 Web 源码未改，版本元数据由本阶段一致性检查接续。既有真实 Rust LSP 3/3、TypeScript LSP 4/4、真实 AST 1/1（Rust/TS/TSX）、Goal detached 组合 2/2、Windows GNU Core 全目标严格 Clippy 通过，相关源码未改，完整工作区原生回归接续通过。Ruby/Node 详细契约检查保留在 [rc1 阶段记录](../0.152.0-rc1/p0-p1-closeout.md)。

## 固定应用与源码

- 版本：`willdeep 0.152.0-rc2`。
- 应用：`target/p0-p1-rc152-rc2-20261117-final1/willdeep`，权限 `0555`。
- 二进制 SHA256：`0a74c1a765fed6503dead6e52df4405ff70cb5e264c087ca40a46b72adca66a5`。
- 六类运行时源码 SHA256：`7fb99ecd5a6360417404915b0fae80e8310aaa458866649c1c320a34411df81f`；该次运行前后稳定。
- 四套 PTY 与自定义智能体入口均使用此副本，运行前后二进制未变。第二轮工具展示只修改验收脚本，仍使用原应用；其源码摘要不冒充原六类运行摘要。

六类分别执行 stale_edit 3、cross_file_references 3、browser_interaction 9、compression_constraints 2、multitask_recovery 2、false_completion 2，共 21 项。九例均为同次运行、同版本、同 SHA，取消或完成后所属浏览器进程和 profile 均为空。

## 失败与修复经过

1. rc1 首轮工作区 3353 通过、2 失败、24 忽略；补齐恢复场景显式 `--continue-task` 后保留原断言通过。后续为 3337 通过、26 失败、24 忽略，其中 25 项明确受到真实用户配置未知 subagent profile 影响，另 1 项 Web 地址测试在回环检查前加载配置。Runtime 默认配置来源和测试隔离修复后，本阶段完整工程通过。原始记录仍在 rc1。
2. Thought 初次修复复现折叠顺序错位与正文伪目标；可见性编译错误已修。补充窄屏及重排回归首轮 18 通过、1 失败，夹具误把用户消息移到末尾而触发既有历史折叠；改为移动目标思考后，19 项通过。
3. 一次工程链在测试前因 Cargo.toml 已切 rc2、锁文件仍是 rc1 而被 `--locked` 拒绝（exit 101）；该次没有执行测试、Clippy 或构建。锁文件同步后，四阶段重新完整执行并通过。
4. 工具展示首轮真实画面已把连续完成工具合成 `2 tool calls folded`，两条命令尾部均消失，逐调用回执均成功。脚本却要求两行独立 `✓ run_command`，因此两布局均超时。脚本改为识别无失败标记的折叠汇总，保留执行、回执、磁盘完成标记与尾部隐藏断言。
5. 第二轮两布局均通过命令展开/收起、历史结束行、Thought 点击和新建文件 diff；编辑上下文行未匹配。工具回执与磁盘正确，但显示层 `safe_text` 调用的脱敏函数使用 `split_whitespace` 与 `join(" ")`，压缩了上下文标记、缩进和连续空格。未放宽断言，rc3 修复显示保真后重新验收。
6. 两次工具展示失败报告、终端帧与私有夹具均保留；每轮所属清理 12/12 通过。更早浏览器导航、Runtime 终态/写盘、复制和队列来源经过见 [0.151 rc2](../0.151.0-rc2/p0-p1-closeout.md)、[0.151 rc3](../0.151.0-rc3/p0-p1-closeout.md)。

## 保证边界

- 文件 SHA256 是内容版本；逐文件原子安装不是跨文件事务，也不提供任意外部写者的文件系统 CAS。
- diff 使用真实成功写入的绑定快照，不增加模型、Git 或 Shell 调用；每文件 8 KiB、总计 64 KiB、最多 32 文件，预算耗尽明确截断。失败、拒绝、预览和回滚不展示成功卡。
- 命令展开属于显示状态；默认 TUI 复用认证本机 SSE，公共 Web/手机流继续剥除完整命令与源码 diff。
- 普通新输入独立验证不代表旧任务完成；显式续跑、replay、host、活动 Goal、完成契约、真实失败和未知副作用保留门禁。
- 页面来源控制不等于 Chrome 全进程网络隔离；九例覆盖 CLI、私有 PTY、Web HTTP/SSE，不包含 Web React 九例操作或部分写入截图时的取消。
- 合成 Provider 不测真实模型质量、Token、费用或整体提速；未调用付费模型或搜索。重复空参数收敛、规则刷新编排、失败分类和上游原始协议取证尚未实施，见[效率诊断](../../research/2026-11-17-tool-call-efficiency.md)。
- Windows GNU 交叉 Clippy 不代表 Windows 原生执行、CI 或发行。P2、DAP、Computer Use/AX、语音、ACP、后台预压缩和 `/btw` 不在范围内。

## Git 与交付

本阶段位于 `feat/p0-p1-runtime-tools`，原实现提交 `662e69c8`，develop 整合 `2744e575`；尚未提交推送后续整合改动。本报告是历史阶段证据，最终交付状态由 rc3 记录。未安装、发布或创建发布 tag；发布 tag 只能指向已合入默认分支的提交。
