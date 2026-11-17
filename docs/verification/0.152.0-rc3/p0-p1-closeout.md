# P0/P1、TUI 与工具展示整合验收

日期：2026-11-17。版本：`0.152.0-rc3`。项目阶段：既有 Rust Runtime 项目迭代。

状态：本版完整工程、固定六类回归、浏览器三端九例、四套真实 PTY 和自定义智能体入口均已实际通过。所有最终应用入口使用同一只读 rc3 副本，源码和二进制摘要核对一致。机器可读结果与最终静态检查见 [JSON](p0-p1-closeout.json)。本报告记录提交前验收快照，Git 提交与推送结果另行交付。

## 范围与结果

范围包含原始三项 P0、六项 P1，以及已授权的并行 TUI、队列、验收素材、新输入与显式续跑、历史结束行、成功编辑差异、当前命令展开、Thought 点击和自定义智能体启动诊断。接口、配置与限制见 [Runtime 工具指南](../../P1_RUNTIME_TOOLS.md)、[P0 决策](../../decisions/2026-11-17-p0-coding-tools.md)、[P1 决策](../../decisions/2026-11-17-p1-runtime-tools.md)和[工具展示决策](../../decisions/2026-11-17-tui-tool-visuals.md)。

| 范围 | 本版实际结果 | 证据 |
|---|---|---|
| 格式、全工作区测试、严格 Clippy、显式构建 | 四阶段退出 0；3374 通过、0 失败、24 忽略 | [工程摘要](engineering.json) |
| P0/P1 六类固定回归 | 6 类、21 项执行通过；源码稳定，无 pending | [可移植摘要](fixed-runtime-regression-final1.json) |
| 浏览器 CLI/TUI/Web 入口 | 各成功、审批取消、执行取消，共 9/9；包含在上述 21 项内 | 同一摘要，已核原报告与清理证据 |
| TUI 阶段、投递、阅读、选区与草稿 | 两布局 18/18 | [JSON](tui-interactions-final1/integration.json) · [Markdown](tui-interactions-final1/integration.md) |
| 输入选区与隔离 OSC 52 | 两布局与 SSH 共 14/14 | [JSON](tui-prompt-selection-final1/integration.json) · [Markdown](tui-prompt-selection-final1/integration.md) |
| 两条排队消息时间线 | 两布局 10/10 | [JSON](tui-queue-timeline-final1/queue-timeline.json) · [Markdown](tui-queue-timeline-final1/queue-timeline.md) |
| 工具展示、历史结束行与真实 Thought 点击 | 两布局各 12 项及二进制稳定性，共 25/25 | [JSON](tui-tool-visuals-final1/tool-visuals.json) · [Markdown](tui-tool-visuals-final1/tool-visuals.md) |
| 自定义智能体启动与派工 | 8/8；本地和 Runtime 均观测到专属模型及完整自定义指令 | [JSON](custom-subagent-final1/custom-subagent-startup.json) · [Markdown](custom-subagent-final1/custom-subagent-startup.md) |

四套 PTY 合计 67 项；加自定义智能体为 75 项。这些是检查项数，不与工程测试或浏览器九例合并计数。五套入口本次各执行一次，没有通过自动重试或削弱断言取得通过。

## 工程与固定快照

按顺序完成 `cargo fmt --all -- --check`、`cargo test --offline --locked --workspace --no-fail-fast`、`cargo clippy --offline --locked --workspace --all-targets -- -D warnings` 和显式 CLI 构建，均退出 0。18 个顶层 target 共 3374 通过、0 失败、24 忽略；另有一条 `detached_jobs::tests::spawn_and_exit_helper` 子进程单项摘要，未重复计入。各 target 数量、忽略范围、原日志路径与 SHA256 见工程摘要。

完整工程后的显式构建冻结到新目录，权限设为 `0555`：

- 应用：`target/p0-p1-rc152-rc3-20261117-final1/willdeep`。
- 应用版本：`willdeep 0.152.0-rc3`。
- 二进制 SHA256：`df3dbbc59a3da8bc67ff0931e666ccf90dbacc4afdfdc71e4dc0a20c7b4129eb`。
- 源码 SHA256：`b1a7b55af7ae8ac297c001e1ced1f531b98b1777f19342b2098aab397e076d58`。
- 六类回归 Core 测试产物 SHA256：`ecd801dc370f09e883ba4a31a758993fbcd808eef86436f2475b7f6ededb78b1`。

源码摘要覆盖 tracked 与相关 untracked 的 crates、scripts、web/src、Cargo.toml 和 Cargo.lock；不含生成报告文档。六类运行前后 `source_stable=true`，全部真实入口完成后再次核对，源码与只读应用均未变化。workspace、四个锁文件包、两个 Runtime 依赖版本钉、Web、npm、安装清单及当前文档统一为 rc3。源码长度口径为上述目录下 `.rs/.rb/.mjs/.js/.ts/.tsx`，共 1102 文件，无超过 3000 行文件；超过 2000 行的文件已考虑拆分，通过新测试或模块承接本次新增，不做无关重构。

## 重点行为与清理

- **成功编辑 diff**：只在真实写入成功后展示相对路径、增删数、旧新行号、上下文及红绿行。真实回执和磁盘均核对，预览不写盘且不显示成功卡。两布局实际编辑帧均保留上下文标记；原生回归另覆盖缩进、连续空格、尾随空白、脱敏和控制字符转义。
- **当前命令**：只自动展开最近一条活动前台命令，结束后收起。验收读取真实逐调用成功回执和磁盘完成标记，接受既有无失败标记的连续工具折叠汇总，并确认历史命令尾部不再显示。
- **Thought 点击与历史结束行**：真实点击最后一条折叠 Thought 只展开其正文，再次点击收起；历史结束线紧贴 AI/时间，历史隐藏「轮到你」。两布局均通过。
- **问答与续跑**：普通新输入独立验证，显式 `--continue-task` 恢复原任务要求。未竟会话中身份问答的实际默认 Runtime 检查为 1 轮、0 工具，旧 Partial 与 required 保留；本版 `headless_runtime` 63 项全过。不据单例推断所有任务的工具量或速度改善。
- **浏览器**：九个场景身份唯一，同次运行、同版本、同 SHA。成功终态均 completed，取消均 cancelled；每例 `processes_after=[]`、`profiles_after=[]`，三端执行取消的导航 socket 均关闭。私有 preview 已停止，所属 Runtime PID 查询确认已退出。
- **PTY 与自定义智能体**：工具展示清理 12/12 通过，其余夹具的私有 Runtime 均停止，队列及自定义智能体临时夹具按脚本清理；保留所需屏幕、请求和回执证据。OSC 52 仅在私有 PTY 捕获，未操作真实剪贴板。未再次读写真实用户配置。

四个与本次修复直接相关的原生用例已从本版完整工程原始日志核实通过：`credentials_layout_preserves_whitespace_even_when_values_are_redacted`、`committed_diff_preserves_context_indentation_spacing_and_redaction`、`literal_diff_escapes_controls_after_redacting_without_flattening`、`redaction_keeps_the_message_layout`。旧通用脱敏 API 语义保留，TUI 与手机复用保留排版的共享入口。

## 历史失败与非阻断观察

[rc2 阶段报告](../0.152.0-rc2/p0-p1-closeout.md)保留该版 3371/0/24、六类 21 项和三端九例、三套 PTY 42 项及自定义智能体 8 项，不能替代本版结果。

1. [rc2 工具展示首轮](../0.152.0-rc2/tui-tool-visuals-final1/tool-visuals.json)两布局在第二条命令完成检查超时：真实执行成功，但脚本误要求独立成功行，未识别连续工具折叠汇总。修脚本后仍保留逐调用回执、磁盘完成、尾部隐藏和 Provider 请求约束。
2. [第二轮](../0.152.0-rc2/tui-tool-visuals-final2/tool-visuals.json)17 项通过、2 项失败；编辑回执和磁盘正确，显示脱敏却压缩 diff 空白。rc3 修复产品显示，没有放宽上下文或颜色断言，最终 25 项通过。两次失败、终端帧和失败夹具仍保留。
3. rc1 工程中的显式续跑参数、真实配置污染、Thought 测试夹具、rc2 锁版本不一致以及更早导航/复制/恢复问题，经过与原始结果继续保留在阶段报告，不回写为通过。
4. 本次 interactions 的 full/compact 私有日志各有 1 条 `Runtime Session not found`；prompt 的 full/compact/compact-ssh/full-ssh 分别为 1/2/2/3 条。既有断言全部通过，私有 Daemon 均有停止记录且无 pid/socket 元数据残留；没有修改断言或重跑。此处保留为非阻断诊断观察，未确认根因，不声称日志零错误。

既有 Web 52 文件、430 测试和构建、真实 Rust LSP 3 项、TypeScript LSP 4 项、AST 1 项、Goal detached 组合 2 项及 Windows GNU Core 交叉 Clippy，按 [rc2 说明](../0.152.0-rc2/p0-p1-closeout.md#工程与专项说明)继承各自执行时的范围，不冒充本版全量重跑。rc3 重新执行的范围以上表和工程摘要为准。

## 保证边界

- SHA256 是内容版本；逐文件原子安装不提供跨文件事务或任意外部写者的文件系统 CAS。
- diff 来自真实成功写入的绑定快照，不增加模型、Git 或 Shell 调用；每文件 8 KiB、合计 64 KiB、最多 32 文件，预算耗尽明确截断。预览、拒绝、失败和回滚不展示成功卡。
- 本机认证 SSE 与公共脱敏事件保留既有边界；公共 Web/手机继续剥除完整命令和源码差异，显示状态不改变模型上下文或执行证据。
- 普通新问题完成不代表旧任务完成；显式续跑、replay、host、活动 Goal、真实失败、完成契约和未知副作用保留门禁。
- 工具展示 PTY 写操作后把最后一次 Provider 响应保持 gated，直到私有 Runtime 关闭；不验证写后整轮 finish 或自动验证。历史结束行来自写操作前真实完成的纯文本轮次。
- 浏览器九例验证 CLI、私有 PTY 与 Web HTTP/SSE 入口，不等于 React 页面九例操作、Chrome 全进程网络隔离或部分写入截图时的取消。
- 合成 Provider 不验证真实模型质量、Token、费用或整体提速。重复空参数收敛、规则刷新编排、失败分类、上游原始协议取证及真实网关提速测量尚未实施，见[效率诊断](../../research/2026-11-17-tool-call-efficiency.md)。未调用付费模型或搜索。
- Windows GNU 交叉检查不代表 Windows 原生执行、CI 或发行。P2、DAP、Computer Use/AX、语音、ACP、后台预压缩和 `/btw` 不在本次范围内。

## Git 与交付

提交检查保留原始 ANSI 录屏及一份内嵌原屏的历史失败报告中的终端空白，使用 `.gitattributes` 精确限定例外；普通 Markdown 报告仅移除多余 EOF 空行。没有改动原始录屏、断言或源码。最终静态检查覆盖 482 个文件、193 份 JSON 和 500 个本地链接，未发现高置信凭据内容；详细结果见 JSON。

分支为 `feat/p0-p1-runtime-tools`，原实现提交 `662e69c8`，develop 整合 `2744e575`。本报告与 JSON 固定为提交前验收快照，后续整合提交包含并行 TUI、队列、素材、诊断与本版修复和证据，按已有授权统一提交推送。

未安装、发布或合入默认分支，未创建发布 tag。发布 tag 只允许指向已合入默认分支的提交，安装清单中的 tag 字符串不代表实际 Git tag。
