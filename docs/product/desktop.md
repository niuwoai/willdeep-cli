# WillDeep Rust 桌面版规划

> 2026-11-24 · macOS、Windows、Linux 共用桌面实现 · 当前交付优先验证 macOS 安装包

把 WillDeep 做成一个按项目工作的 Agent 工作台。打开目录后能继续历史会话、发起任务、处理审批、审阅改动。关闭桌面窗口后，本机 Runtime 继续执行。再次打开，可以回到同一个会话看结果。

## 主窗口

| 区域 | 默认显示 | 交互 |
| --- | --- | --- |
| 左侧项目栏 | 新会话、搜索、待处理；按工作区列出最近会话 | 保留 Xedit 的项目分组、固定、归档。运行中、待审批、待回答、失败用文字与图标提示，点击待处理项定位到对应会话 |
| 中间会话区 | 顶部显示项目、分支与运行位置。正文显示消息、执行进度和完成结果 | 工具记录默认合并，按需展开。审批和问题出现在对应消息位置，滚动回看时保留阅读位置 |
| 底部输入区 | 工作区、模型、权限、附件与发送或停止 | 沿用 `/` 命令、`@` 引用和 `$` 技能。模型与权限提交后回显实际生效值，切会话保留各自草稿 |
| 右侧检查面板 | 默认收起。打开后先显示变更文件和 Diff | 从会话中的文件或结果跳到对应 Diff。窄窗口切为独立面板。后续加入计划、Worker 与产物 |

新会话打开后焦点直接落入输入框，主区留给选项目、继续最近会话和当前任务。Xedit 的账号概览、Token 热力图及月历移到独立「用量」入口。三端使用系统窗口、菜单、快捷键和目录选择，界面采用 WillDeep 自有字号、间距、图标和状态样式。Codex 提供项目分组、会话内 Diff 和 CLI 连续性的交互参考，Claude Code 提供可收起检查面板、逐文件审阅与权限选择的参考。[Codex 桌面版](https://openai.com/index/introducing-the-codex-app/) · [Claude Code 桌面文档](https://code.claude.com/docs/en/desktop)

「本机运行」指 Runtime 和工具在本机执行。模型选择器单独显示本地模型或远端 Provider，让用户在发送前看清执行位置和模型请求去向。

## 技术方案

**建议在本仓新增 Tauri 2 桌面入口，三端与浏览器共用现有 React 工作台和 Rust Web API。** 第一阶段在三端原生窗口加载同一份页面，沿用会话树、历史投影、流式输出、输入联想、审批和中英日资源。通过桌面宿主增加目录选择、系统菜单和快捷键，逐文件 Diff 通过共用 API 增补。Tauri 使用各平台系统 WebView，关键交互在各平台实际窗口验证。[现有 Web 能力](web.md) · [Tauri 进程模型](https://v2.tauri.app/concept/process-model/)

| 层 | 职责 |
| --- | --- |
| React 工作台 | 布局、可见状态、每个会话的草稿和阅读位置。中英日文案全部来自 i18n 资源 |
| Tauri Rust 宿主 | 原生目录对话框、Runtime 发现、显式启动、能力协商。通过受限 IPC 暴露桌面专属能力 |
| Rust Web 服务 | 复用现有页面和 API 适配。桌面发行模式为本地页面和 API 增加访问校验，Runtime 凭据留在 Rust 进程内 |
| Rust Daemon | 会话、轮次、任务、队列、工具执行、审批和事件的权威来源。共用 Rust SDK 和协议，macOS、Linux 优先 Unix Socket，Windows 优先本地命名管道 |

已有兼容 Daemon 时直接连接。首次安装且服务未运行时，桌面通过同版随包 Runtime 和现有启动流程启动服务，并共享当前 Rust CLI 的 Runtime 数据目录。窗口关闭、应用退出与停止任务分别处理。连接先做能力协商，运行中的服务更新走既有排空升级。断开后从最后已应用事件序号恢复，并按会话、轮次和任务校验归属。发送重试复用原请求 ID；未收到服务端送达结果前显示对应等待状态。桌面关于页显示随包版本，诊断页另显示当前连接的 Runtime 版本和协议版本。随包前后端同版，接入现有发行版本校验，沿用 Runtime 的 `x-willdeep-version` 响应头。[Runtime Daemon](../RUNTIME_DAEMON.md) · [控制 API](../RUNTIME_CONTROL_API.md) · [Rust SDK](../../crates/willdeep-runtime-client/README.md)

**优先验证历史与实时事件的衔接。** 桌面沿用 Web 的历史详情、工作区校验和重连逻辑，在真实窗口验证断线续传。现有 Web 显示 Diff 摘要，逐文件内容走 Runtime 已有的快照与内容接口，区分任务归属变更和工作区已有改动。桌面发行模式的本地访问校验需在打包前完成。[Web 历史详情源码](../../crates/willdeep-cli/src/web.rs#L1463) · [Web 适配接口与访问边界](../WEB_GUIDE.md) · [控制 API](../RUNTIME_CONTROL_API.md)

## 三平台交付

三端共用一套产品功能和验收用例，各自构建安装包并在目标系统验证。当前 Runtime 发包流程有 macOS 通用包和 Linux AMD64、ARM64 包。Windows 源码仍有本地传输和后台进程支持，当前 CI 与发包流程已停用 Windows。Windows 原生构建、后台任务生命周期和权限行为是第一阶段的验证项，进程枚举超时需在原生测试环境复核。[现有发包流程](../../.github/workflows/release.yml) · [CI 平台矩阵](../../.github/workflows/ci.yml#L26) · [本地传输实现](../../crates/willdeep-cli/src/daemon/local_transport.rs)

桌面构建与 CLI 构建分别运行，桌面安装包附带本仓同版本 Runtime。当前交付先做 macOS 本地内测，安装包构建与复测命令见[桌面开发文档](../../desktop/README.md)。

| 平台 | 首批安装包建议 | 平台验收重点 |
| --- | --- | --- |
| macOS | 通用 DMG | 签名与公证，WKWebView，退出 App 后继续任务 |
| Windows | x64 安装程序 | WebView2、本地命名管道、后台进程启动与停止、路径和权限语义 |
| Linux | Ubuntu x64 的 DEB | WebKitGTK 及系统依赖、目录选择、bubblewrap 权限行为 |

平台能力以运行时实际探测为准。已有命令沙箱文档覆盖 macOS Seatbelt 与 Linux bubblewrap，Windows 的执行权限与隔离状态单独验收并如实展示。安装包各自通过后进入内测，正式三端发布以共同的任务链路验收为准。[Runtime 沙箱](../SANDBOX.md) · [Tauri 安装包](https://v2.tauri.app/distribute/)

## Xedit 迁移与扩展

| 范围 | 取舍 |
| --- | --- |
| 项目与会话 | 首版共用 Rust Runtime 的项目和会话。后续接入已有 Xedit 历史桥，标注来源、先只读预览，用户选择继续时复制成 Rust 会话，原记录留在 Xedit |
| 插件与技能 | 首版提供能从 Runtime 查询的已启用技能。第二阶段做插件中心，展示来源、版本、权限、批准和启用，接入目录、ZIP、Git 安装。有独立插件页面时单独隔离，并沿用 Rust 插件声明和宿主能力校验 |
| Worker、计划与调度 | 第二阶段先在会话内显示 Worker、产物、计划和状态，再把调度历史链接回会话。Worker 统计跟随任务记录，专家圆桌与长流程插件按 Rust 能力分批接回 |
| 用量 | 按现有账本做活动热力图、月历和日明细。桌面来源目前会落入 `unknown`，需先补来源标记再开放桌面和 CLI 筛选。历史来源、用量、价格缺失时显示未知，费用为零时才显示零 |

旧版源码对照采用 Xedit 的 `AgentSessionsSidebar.swift`、`AgentModeComposerView.swift`、`AgentPluginCenterPage.swift`、`AgentStatusIndicators.swift`、`AgentUsageStats.swift` 和 `WillDeepRuntimeClient.swift`。旧客户端的一条桥接路径已有 Unix Socket 只读观察，另有专门的工作流集成。新桌面会话的发送、审批与重连按 Rust 控制协议实施。五张截图和对应的设计取舍见[设计参考索引](desktop-references/README.md)。[Xedit 联动记录](../XEDIT_INTEROP_STATUS.md) · [用量账本](../USAGE_LEDGER.md)

## 实施顺序与验收

| 阶段 | 可验收结果 |
| --- | --- |
| 技术验证 | 三端真实窗口运行现有 Web 工作台，接入本机 Daemon。打开项目、加载历史、发送一轮，退出重开后恢复会话。优先验证 Windows 原生 Runtime、事件恢复、中文输入法和长会话滚动 |
| 首个内测包 | 三平台各自安装和验收上手引导、项目会话、模型与权限、文本及图片附件、发送与停止、队列状态、审批与回答、只读 Diff。在同一 Daemon 上通过桌面接续 CLI/TUI 会话 |
| 第二阶段 | 插件中心增强、Xedit 历史导入、Worker 与用量。在此基础上扩展 Worktree、调度及更多 Linux 发行版与架构 |

首版发布前以隔离测试项目验收四条路径。

1. 两个工作区并行运行，切换时草稿、事件、工具结果和审批各归各位。
2. 在流式输出、排队与待审批状态关闭再打开桌面，仍能准确显示服务端状态。重试一次发送只产生一轮；Daemon 重启后按 Runtime 返回的已恢复或已中断状态展示。
3. 在桌面批准或拒绝一次操作，切回 CLI/TUI 看到同一结果。只读工作区策略仍由 Runtime 生效。
4. 以有未提交改动的 Git 仓库跑一轮编辑，打开 Diff 能区分本轮变更与原有改动；非 Git 目录仍可正常聊天和运行。
