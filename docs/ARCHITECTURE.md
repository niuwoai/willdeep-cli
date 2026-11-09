# CLI Harness 架构

> 最后更新：2026-11-09 | 当前版本：v0.147.0-rc14

## 分发

npm 包 `@niuwoai/willdeep` 不携带 Rust 二进制。`npm install` 读取 `npm/willdeep/install-manifest.json` 中与包版本绑定的 GitHub Release，下载当前平台的一个发行包，校验 SHA256 并确认包内只有预期的可执行文件，然后写入 npm 包的 `bin/`。命令入口 `bin/willdeep.js` 只转发参数和标准流。macOS Universal 同时覆盖 x64 与 arm64；Linux 区分 amd64/arm64。Windows 暂不发布发行包，CI 仍保留 Windows 编译与测试以维持代码可移植性。Homebrew 继续直接安装 macOS Universal 发行包，两条路径都消费同一个 GitHub Release。

macOS 发行构建固定使用 `macos-26-intel`，以同一 macOS 主版本的 Intel runner 避开默认 arm64 池容量不足；分别构建两个 Apple Darwin 目标后用 `lipo` 合并。发行归档与运行平台支持范围不变。

## 边界

当前实现以同一个 Agent Runtime 支撑一次性 CLI、TUI 与内嵌 Web 模式。子 Agent、后台任务与常驻 Runtime Daemon 均已接入，任务在 Daemon 进程内执行并可跨客户端断连恢复（见 [Runtime Daemon 与工作区](RUNTIME_DAEMON.md)）；Computer Use 与 Browser Use 仍在后续范围内。

## 调用链

```text
CLI 参数、环境变量与 TOML Profile
        │
        ▼
ProviderConfig ── Provider 身份（鉴权/专属头）
        │         API Dialect（线格式）
        ▼
统一 Message / ToolCall / Completion
        │
        ▼
Agent Loop ── 工具注册表 ── 工作区边界与审批
        ├── Skills Catalog（SKILL.md）
        ├── MCP stdio / Streamable HTTP（动态工具；远程服务可 OAuth）
        ├── Session Store（版本化 JSON）
        └── 工具结果回传 Provider，直到最终文本
```

## 核心设计

### Web 失败与可操作会话投影

Web 失败详情统一从 Runtime Turn/Task 投影为有界、脱敏的 `failure_detail`，实时 SSE、终态重连、重复请求和会话历史使用同一逻辑。Provider 流内错误提取 code/type/message 后先过滤凭据和内部内容；浏览器只展示投影字段与已授权的诊断引用。待处理列表按父项目归属和注册工作区权限筛选 Session，聚合人工等待及未被新轮次覆盖的失败，计数直接取列表长度。

Web 来源 Turn 中的 `start_session` 先写入现有子会话账本的可选 `proposals` 字段，不创建 Session、Turn 或执行槽。来源由 Runtime 已持久化的 `origin_client` 判定，模型不能指定来源或确认权限。私有 Web 的 `proposal.start` 操作将保存的参数提交到原有 admission；稳定的派生 request ID 复用 assignment 幂等记录，因此跨刷新、重复点击和进程恢复均不会重复创建。`proposal.dismiss` 与开始共用账本锁，已开始任务不能被关闭提议操作取消。旧账本缺少该字段时按空集合读取；未新增数据库迁移。执行选项、权限、依赖和模型仍在确认开始时复核。

确认策略同时保存在 Turn 的可选 `child_start_confirmation` 字段。自动回报唤醒按持久化 wake 批次成员、已确认提议及来源 Turn 继承策略；入队和领取都在既有 `ledger → sessions → turns` 锁序内重核，兼容缺少字段的旧记录、来源 Turn 已删除及后来加入 Web 结果的批次。Harness 使用同一持久化策略追加提议契约，不改写 origin 或用量归属；模型参数不能覆盖该策略。

确认后准备失败时，assignment 保留安全错误且暂停自动准备，提议继续可见；显式重试复用同一 assignment 和 Turn 请求身份。关闭前先恢复已落盘 Turn 的回执，防止崩溃窗口把实际已开始的任务当成未开始而取消。旧 Runtime 缺少提议操作时，结构化能力错误经上下文包装仍映射 HTTP 501，浏览器可按原兼容路径隐藏不支持的入口。

### 当前计划卡的原位更新

Core 对话投影与 TUI 实时展示共同维持一张当前计划卡。`Plan::has_same_steps` 按步骤顺序、ID 和标题判断结构是否相同；普通进度变化保留详情，替换或结构变化清除无关旧详情。持久计划覆盖实时或历史投影时复用原卡片位置，TUI 同时保留展开状态，原始消息历史保持不变。

### TUI 提交与模型确认的首帧反馈

Enter 的本地 UI 状态更新与 Runtime 执行结果分开呈现：普通提交先把用户消息加入聊天区并清空输入；模型确认先捕获目标 Provider/model，再关闭弹窗并追加切换中反馈。首个落盘或控制请求之前完成一次 `BeginSynchronizedUpdate → draw → EndSynchronizedUpdate/flush`，避免内存状态已更新但终端仍显示旧画面。完成后再显示成功或失败，不将收到选择误报为切换完成。

本次保留事件循环中的串行操作和现有模型切换事务顺序，没有引入后台结果归并或新的 Runtime 协议；后续按键仍等待当前控制操作完成。运行中选择继续使用已有 pending model/provider，在轮次结束后生效；忙时提交的失败恢复与回显认领复用现有队列机制。验收使用独立 HOME、真实 PTY 与 Runtime 控制请求阻塞门，检查完整终端帧而非累计 ANSI 文本，见[本版记录](verification/0.145.0-rc4/README.md)。

### 首次目录审批引导

首次交互打开新目录时，客户端解释既有 Strict / Smart / FullAccess 的适用场景，默认 Smart，FullAccess 仍需明确确认；工作区访问档继续在原权限面板提供。选择写入既有 `WorkspaceAccess`，当前会话仍使用原 `session.update_approval_mode` 与 `SharedApprovalMode` 执行，不增加 Core 权限维度、信任账本或 Web 配对体系。新目录自动登记及公开注册参数缺省均为 Smart，用户既有只读策略保持硬上限。

`RuntimeWorkspace.approval_mode_configured` 只标识首次引导是否完成，不参与工具权限计算；自动登记为 false，显式登记/选择后为 true，旧记录和旧服务响应缺少该字段时按 true 读取，避免覆盖既有选择。`workspace.set_approval_mode` 在 Store 锁内只更新 access 与引导标记，和管理员改档串行检查 ReadOnly；工作区名称、Provider、Skills 与 MCP 限定保持不变。恢复会话保留已有会话覆盖值；非交互入口不增加阻塞式选择。产品边界见[审批引导决策](decisions/2026-11-06-workspace-trust.md)。

### Git 验收快照与嵌套工作树

`daemon::diff_review::fingerprints` 负责验收基线与 Diff capture 共用的内容指纹。Git 报告的未跟踪目录按嵌套仓库处理，递归绑定 HEAD、暂存/未暂存 binary diff、未跟踪文件字节与 gitlink；不会跳过 `.claude/worktrees`。子模块和嵌套仓库共用 32 层上限，未出生 HEAD 明确支持，损坏 gitfile、缺失嵌套元数据与读取错误继续失败。工作区文件符号链接仅记录链接目标；Git 元数据链接保留 Git 本身支持的解析方式。

Harness 在模型请求前采集基线，失败保留路径与错误链，先脱敏再按 Unicode 字符截断至 2,048 字符。普通非 Git 工作区仍可用，但采集失败不能伪装为不支持验收或指纹未变化。四项整合来源和本轮验证见 [rc3 验收](verification/0.145.0-rc3/README.md)。

### 准备链路与持久化诊断

`WILLDEEP_TRACE_PREPARATION=1` 在 stderr 输出 `willdeep.preparation.v1` JSON：固定 component/stage 名、阶段耗时 `elapsed_ms` 和组件累计 `total_ms`，默认关闭，不携带任务正文、工作区、配置或凭据。Runtime 调度与文件回退内容快照、Harness、会话认领、首条用户消息落盘、Core 验证基线和首次模型轮次前的上下文准备分别计时；嵌套组件的累计耗时不可直接相加。诊断写入失败后停用本次 tracing，不让 stderr 故障在析构路径影响任务。

子会话账本仍由独立控制锁串行保护。`persist` 在锁内比较当前持久字节与拟提交内容；内容相同且本实例已经为该内容成功完成文件/目录同步，才跳过后续同步，不递增 revision 或替换文件。新实例及同步失败后的重试须先重新建立持久性，避免将页缓存中的相同字节当作成功提交。内容变化仍原子写入并同步文件/父目录，显式结果、唤醒和调度屏障保持原位置。任务状态与错误未变时不修改 `updated_at`，避免轮询本身制造变化。TUI 首答在首个正文增量到达时记录，并用提交时事件头屏蔽旧输出；该值包含准备与传输，不能与服务端 prefill 混用。

### 配置优先级

默认读取 `~/.willdeep/config.toml`，`WILLDEEP_HOME` 可替换配置目录，`--config` 可指定任意 TOML 文件。命令行与 `WILLDEEP_*` 环境变量覆盖 Profile；Profile 覆盖 Provider 专属环境变量和内置默认值。

配置 Schema 使用 `version = 1`，未知字段直接报错，避免拼写错误被静默忽略。内联密钥（Provider 的 `api_key`、通知渠道的 `secret` / `password` / `api_key`）在 Unix 上要求配置文件权限为 `0600`；也可以改用对应的 `*_env` 字段引用环境变量。`willdeep config show` 对内联密钥统一脱敏。

### 聊天配置与压缩快照

默认父 Harness 在全局配置能力允许时挂载 `ChatConfigurationHost`，Core 通过 `ConfigurationHost` trait 暴露 `configuration_read`（schema/snapshot/plan）与 `configuration_write`（apply）。该能力独立于用户插件目录、普通 MCP 服务和 Web 网关；Worker、委派子会话及受限 MCP 任务不会继承它。写操作沿用全局写审批和共享审批句柄，`workspace-write` 不会将其当成工作区内编辑放行；只读保持硬拒绝，未知结果的写操作进入现有副作用恢复门禁。配置写重放先检查计划身份、参数和当前权限，再请求一次性审批；失效或只读计划不弹审批。

CLI 适配器绑定实际配置绝对路径，现有符号链接解析到目标。首次调用才展开内嵌配置资产并用已有 MCP stdio 客户端启动私有 Ruby 服务，显式注入 `WD_CONFIG_FILE`、`WD_HOME`、`WD_CONFIG_BACKUP_DIR`；不安装/启用/批准插件。首版只编辑三个 `[agent] compressor_*` 字段，结构化查看返回这些字段、Provider 的名称、模型、窗口，以及每个 `[subagents.*]` 的 `provider_profile` 与 `model`；原文、注释、未知节和密钥预览不进入模型上下文。

`plan` 复用 Ruby schema/渲染，私有传输将候选文档交给 Rust `ConfigFile` 解析、`config::validate` 与 `build_compression_settings` 校验，随后仅返回结构化变更和宿主 plan_id。每个宿主保留最新成功计划，绑定本实例、目标、edits 及文件 SHA256 revision；`apply` 只收 plan_id，在审批后注入保存参数。共享 Ruby 保存路径在文件锁内重新检查 revision、备份、原子替换及写后校验。不同目标共用备份目录时，再以目录级锁覆盖备份名称分配和写入，避免同名配置覆盖彼此备份。回执丢失不自动重放；已收到的成功回执可幂等返回。文件锁协调使用同一配置服务协议的保存，普通外部编辑器不参与该协议。

Runtime 在新 Turn 开始执行时读取配置并构建独立 Agent；已运行 Turn 不被热替换。Core `CompressionSettings` 以一把锁整体保存候选和窗口，压缩入口复制快照贯穿分块、合并、候选回退，网络等待期间不持锁。TUI 手动 `/compress` 先从当前提交来源加载配置并构建设置，再 claim/refresh 会话、整体应用并派发；配置加载或构建错误只显示安全本地化提示与目标路径，不展开可能含凭据源行的底层异常；失败在界面反馈后继续事件循环，执行权持有至结果消费。设置成功刷新后也供后续 `/local` 压缩使用，即使之后摘要请求失败或取消仍保留新设置；其余 `/local` 配置仍为启动快照。配置语义验证不等于端点连通性验证，预览不发送模型请求。

契约与生效时机见[配置指南](CONFIGURATION.md#在聊天中修改压缩配置)，隔离测试与限制见[聊天配置验收](verification/0.143.0-rc9/chat-config/README.md)。完整子会话 P2 的控制、订阅、依赖和检查点保护继续保留；测试夹具回收仅作用于测试私有进程。组合边界见[整合决策](decisions/2026-11-05-p2-configuration-native-integration.md)，本版组合测试状态见[验收记录](verification/0.145.0-rc1.md)。

### Magpie 社区 Agent 集成

Magpie 通过独立 Agent 插件管理 `$WILLDEEP_HOME/integrations/magpie/cli/connection.json`；Desktop 使用自己的 `~/.willdeep/integrations/magpie/desktop/connection.json`。两端共享 v1 契约与测试样例，独立读取、校验和控制状态。配置只允许回环地址、完整模型 ID 和固定公开归因标记。

CLI 的磁盘 `ConfigFile` 与临时有效 Provider 目录分离；`magpie-cli` 不写入 TOML。隐式新会话解析当前连接并固定 Provider、模型与集成来源；来源区分临时集成与用户原有的同名档案，避免断开后把会话转给同名档案。旧会话恢复不重新采用外部默认值。Runtime 每次执行读取连接，无须重启；断开后显式引用集成的会话返回不可用错误。实际请求禁止代理与重定向，`localhost` 固定到回环地址。原工具审批、沙箱与显式 Worker Profile 继续由现有执行链控制。

详见 [需求契约](MAGPIE_AGENT_REQUIREMENTS.md)、[跨端设计](MAGPIE_AGENT_DESIGN.md) 与 [实施及验收台账](MAGPIE_AGENT_PLAN.md)。

### Provider 身份与 API Dialect 分离

Provider 身份回答“如何鉴权、是否附带 some.im 上下文”；API Dialect 回答“如何编码消息和 Tool Call”。因此同一个 some.im Provider 可以承载 Chat Completions、Responses 或 Anthropic Messages。

Web 模型选择先通过 `/api/settings/model-routing` 读取 Provider 档案，再以档案 id 请求 `/api/schedules/models?profile=<id>` 发现模型目录。模型切换同时提交模型名与 `provider_id`，会话详情也返回当前 Provider 身份；前端据此区分不同 Provider 下的同名模型，并把搜索限制在当前档案目录。

### 会话级思考等级

Provider Profile 可定义 `ReasoningProfile`：可自定义的逻辑等级名映射到字符串、数字或布尔值，并指定默认等级和请求参数路径。未覆盖路径时由 API Dialect 提供协议默认值。TUI `/reasoning` 与 Web 选择器保存 Runtime Session 的等级覆盖；`null` 表示跟随 Provider 默认，不写入可导出的 Core Session。恢复会话直接读取已存在的 Runtime 元数据，不依赖是否已经提交首个任务。

Runtime 在领取任务时将生效等级复制到 `SubmitTask`，当前执行任务不会因后续选择改变。TUI 运行中切换先校验并保存 Runtime 选择，避免任务队列抢先领取旧等级；本地 Provider 刷新与正在执行的任务快照分离。Web 思考等级专用保存使用 `reasoning_only` 补丁，只更新映射并保留连接字段。`/thinking` 仍只控制 TUI 的思维内容显示。

### 统一领域模型

Agent Loop 只理解：

- `Message { role, content, tool_call_id, tool_calls }`；
- `ToolCall { id, name, arguments }`；
- `Completion { content, tool_calls, finish_reason, usage }`。

三种 Provider Adapter 负责协议翻译，工具与 Agent Loop 不包含 Provider 分支。

### Swift 资产的复用方式

本阶段没有复制 Swift 源码，而是迁移其行为契约：

- 工具名称和参数尽量一致；
- `read_file` 使用行号、offset、limit、max_bytes；
- 新文件只能由 `create_file` 创建；
- `edit_file` 使用精确字符串替换并要求唯一匹配；
- 原生 Anthropic 的系统消息、tool_use/tool_result 和 Token 上限规则；
- Responses 的 typed input、function_call/function_call_output；
- some.im 专属上下文头不得包含路径。

### 安全边界

- 工具仅接受工作区相对路径；
- 拒绝绝对路径和 `..`；
- 已有文件 canonicalize 后再次检查工作区前缀，阻止符号链接逃逸；
- 新文件检查最近存在的父目录，阻止通过符号链接写出工作区；
- 审批档位 `strict` / `smart` / `workspace-write`（默认）/ `full-access` 与 Xedit 逐档对齐，判定集中在 `willdeep-core::tools::approval`；档位是 `SharedApprovalMode` 原子句柄，TUI（`/permissions`、Shift+Tab）、进程内 Agent 与 Runtime 任务共享同一份，切档对正在跑的一轮立即生效。Runtime 侧由 `daemon::approval_modes` 登记活跃任务句柄，会话档位经 `session.update_approval_mode` 持久化在 Runtime 会话上，`read_only` 工作区策略是硬上限；
- 根 Agent 与子 Agent 均有 `AgentInstructionInbox`，主循环每次调用模型前排空；用户插话以 `OperatorInput` 注入，父 Agent 指令保持宿主来源。Runtime 控制面 `turn.steer` 可向在途任务的内存收件箱投递，正常收尾时未消费输入通过 `steer_undelivered` 事件交回；该接口没有进程崩溃后的持久回收保证，且前台工具/审批等待期间不等于模型已读取。当前 Runtime TUI/Web 默认提交仍走持久 Turn 队列，宿主即时反馈真实任务状态和投递状态，不将入队显示成模型已读；本地执行保持原插话机制；
- `strict` 对写入和命令逐次审批；`smart` 放行已通过工作区边界校验的创建和编辑；
- Shell 命令走两级审批：`willdeep-core::safety` 的静态分类器（按 Shell 语义分段，只读/受限即放行，破坏性形状直接交用户且**不送 AI**），中间地带交 `willdeep-core::judge` 的 AI 判官——判官只能免审，不能扩权，NO / 不可用一律回落用户；命令在出网前本地脱敏，裁决只认唯一一个完整 `<verdict>` 标签；
- 非交互审批失败时拒绝，不自动升级；
- 会话分享是第三种对外面：Web（9847）只在回环开放创建/撤销 token（`shares.json`，0600，一条会话一个 token，重建即吊销旧链接），公开访问走 `[intake]` 监听（默认 9857）的 `/share/<token>` 与 `/api/share/<token>`。链接前缀取 `[intake] public_url`，未配置时按实际监听地址拼，管理页与分享面板共用同一条拼接规则。分享页只投影 `user`/`assistant` 正文，不列工作区、不读配置、不挂任何管理端点；`chat` 档的访客发言作为普通 Runtime 轮次按会话当前审批档位执行，另有 8 KiB 正文上限和每 token 每分钟 20 条的限流。这与插件宿主、Mobile Relay 一致：对外面只拿到一处刻意收窄的投影，管理面永远留在本机；
- 公网 Intake 监听（默认 9857）只注册站点、表单、webhook、分享与健康检查路由；Web 管理接口 `/api/intake` 的配置读取和写入都限制为回环访问，远程 Web 监听不会返回表单/Webhook 配置、工作区路径、失败投递或会话绑定。浏览器表单页签发绑定 slug、有效期和随机 nonce 的 HMAC-SHA256 短时令牌，签名密钥只驻留在 Intake 进程内；POST 原子消费 nonce，拒绝过期和重放。令牌用于防盲跨站提交，不证明提交者身份，也不能阻止机器人获取新表单。
- 私有 Web 的失败 SSE 与历史 DTO 包含 Session/Turn/Task 引用、失败域和经过脱敏限长的具体原因；服务端读取诊断前验证工作区 allowlist 与 Task/Session/Turn 归属，只允许失败或中断任务。展开诊断沿用相同归属检查，不返回原始工具参数或输出；手机中继与公共 SSE 不携带原始诊断；
- 远程单用户 Web 通过带认证的 HTTPS 反向代理访问。Web 仅监听节点回环地址；跨节点部署可由前置代理通过专用 SSH 本地隧道接入，隧道监听也仅限回环。入口认证配置只在服务器之间安全复用，不复制登录会话，不允许从公网直接绕过前置认证访问后端。
- HTTP 错误体限长并进行基础凭据脱敏。

### 会话、Skills 与 MCP

会话按 UUID 写入 `$WILLDEEP_HOME/sessions`，使用临时文件加同目录 rename 保证单文件原子替换。恢复历史时会丢弃旧 system message 并注入当前工作区和 Skill 清单，避免陈旧边界跨会话延续。

Runtime 侧每个轮次在 `runtime/turns.json` 里记着自己在会话消息里的起止下标与压缩代数，这是「回到第 N 步」的对话边界。文件边界是**工作区检查点**（`daemon::workspace_checkpoint`）：轮次开始前、模型动手之前，把工作树写成 `$WILLDEEP_HOME/runtime/checkpoints/<工作区哈希>/` 这个私有裸仓库里的一个独立 commit，对象库经 `alternates` 借用工作区自己的 `.git/objects`，引用在 `refs/willdeep/<会话>/<轮次>`，用户仓库的 HEAD、索引与 `git log --all` 不受影响；非 git 工作区没有检查点。控制面 `session.rewind` 先按第 N+1 步的检查点恢复文件（被覆盖的原件进 `runtime/recovery/rewind-<会话>-*`，回退前的整棵树另拍 `before-rewind` 快照），再原地截断会话、摘掉被丢的轮次、清空执行检查点；文件恢复失败时对话不动。回退不删审批、Diff 归属与审计事件。详见 `docs/CHECKPOINT_REWIND.md`。

Skill Catalog 只扫描配置根目录的直接子目录，读取入口固定为 `SKILL.md`；附属资源 canonicalize 后必须仍位于 Skill 根目录，单次读取最多 48,000 字符。

MCP server 由 TOML 声明：stdio 服务给 command、args、env，Streamable HTTP 服务给 url、静态 headers 与鉴权方式（`bearer_token_env` 或 `oauth`），两者二选一，规则在 `willdeep-core::mcp::McpServerConfig::validate` 里只定一份，配置校验与连接共用。stdio 建长连接；HTTP 每条 JSON-RPC 一个 POST，JSON 与 SSE 响应都认，维护 `Mcp-Session-Id` 与协商到的协议版本头，会话过期重握手一次，退出时 DELETE。OAuth 由 `mcp::oauth` 实现（授权码 + PKCE、回环回调、RFC 9728 / 8414 / 7591 / 8707），token 存 `$WILLDEEP_HOME/mcp-oauth/`，连接时自动带上并刷新。两种传输都执行 initialize/initialized/tools/list，并以 `mcp__<server>__<tool>` 建立内部索引；远程服务连不上只警告跳过，本地 stdio 配置错误直接失败。Provider 侧只常驻 `list_mcp_tools` / `call_mcp_tool` 两个固定工具，匹配 Schema 按需加载，避免全量目录占满小上下文。MCP 调用在 `strict`、`smart`、`workspace-write` 三种模式下都进入审批链（后两种只免审当前工作区内的 `create_file`、`edit_file`）；`full-access` 下免审并记审计。MCP stderr 继承到宿主终端，stdout 仅作为 JSON-RPC 通道。

### Prompt 与附件

TUI 使用独立的多行 Prompt Editor，光标以 UTF-8 边界存储，并按 Unicode 显示宽度计算换行、上下移动、点击定位和内部滚动。终端启用 Bracketed Paste，长文本保留为消息附件而不是撑满编辑框。

图片从本机系统剪贴板读取 RGBA，限制为最多 64 MB 原始像素，编码为 PNG/Base64 后进入版本兼容的 `Message.attachments`。发送前附件可从草稿删除；发送后随会话 JSON 持久化。Provider Adapter 分别转换为 Chat Completions `image_url`、Responses `input_image` 和 Anthropic `image/source`。some.im 已知纯文本主模型由 Agent 通过同一 API Base/API Key 的 `qwen3-vl-plus`（或 `vision_model` 覆盖值）生成描述，再移除发往主模型的图片负载。「是否纯文本」优先取网关能力表（`GET /v1/model-capabilities`，见 `willdeep_core::provider::capabilities`）：只信经上游探测验证的协议声明或目录明确列出的 `image`，给不出结论时回落 `model_accepts_images` 名单。

网关能力表在会话启动时拉取（限时 5 秒，按「端点 + Key」摘要缓存于 `<WILLDEEP_HOME>/cache/model-capabilities/`，新鲜 10 分钟、接口不存在负缓存 24 小时、失败时退用 7 天内旧缓存），还用于：配置未写 `context_window` 时取网关报告的窗口；未显式指定 `api` 且网关没有线路能以缺省协议服务该模型时，会话模型改用网关能服务的协议（派生的子 Agent / 压缩器配置不受影响）。

助手输出的媒体采用另一条只读链路：Markdown 图片先在聊天文本中变成卡片，用户通过 `Ctrl+L` 明确选择后才下载。远端目标复用 Web Tool 的公网地址校验，并叠加重定向、超时、8 MiB 响应、尺寸和解码内存上限；本地目标 canonicalize 后必须仍位于 Workspace。Ratatui 图片状态在后台线程完成缩放/编码，启动探测 Kitty、iTerm2、Sixel，原生编码失败时使用保留的解码缓存重建 Unicode halfblocks，避免阻塞或清空 TUI。

### 上下文与网络工具

Provider Profile 可声明 `context_window`。自动压缩只作用于请求投影，会话保留完整 durable 历史。可用容量先扣除工具定义、窗口 1/8 的输出预留，以及本轮计划、目标、轮次/Token/时间预算等实时 system 状态的完整估算量。

| 阶段 | 边界 | 行为 |
| --- | --- | --- |
| 工具分页 | 可用容量的 1/8，最低 64 tokens | 超长工具正文归档并生成可检索的首尾摘录，重复宿主指令只保留最后一次；未知副作用批次保持保护 |
| 摘要触发 | 可用容量的 75% | 先尝试已有摘要的投影；仍达水位才增量摘要未覆盖的旧历史，通常保留最近六条并对齐完整工具批次 |
| 超窗收敛 | 可用容量 | 近期尾部仍过大时继续摘要已压缩的结果，再归档完整旧工具批次；无法保留用户约束并降到容量以内时返回错误，不报告假成功 |

`SummaryCache.through` 锚定 durable 下标，分页投影单独映射，工具摘录变化或重复宿主指令去重不改写缓存来源。稳定系统提示、项目规则、验收契约和实例身份参与 system 指纹，已覆盖非 system 历史参与前缀指纹。真实规则或前缀变化时失效；正常尾部追加继续复用或增量合并。运行成功后摘要通过 `AgentOutcome` 交由 Runtime harness 随会话保存，后续 run 恢复后重新校验。

从 `0.147.0-rc4` 起，durable 消息、检查点和运行结果始终保留完整 system；仅在请求投影中以稳定规则快照替换 system，压缩后回填本轮实时计划、目标和执行预算。动态状态不进入摘要原料或缓存指纹，非托管摘要器的目标说明也不再携带实时状态。下一轮重建稳定规则再校验缓存，避免轮次计数和剩余毫秒数导致每轮全量重摘。容量预留发生在分页和水位判定前，压缩事件的估算包含该预留量，最终请求仍包含实时状态。旧版本含动态 system 指纹的摘要可安全失效一次，无持久协议迁移。

用户执行 `/compress` 时则显式生成摘要、保留最近六条消息并原子保存精简后的会话。TUI 展示最近一次 Provider Usage、缓存命中率、耗时和压缩阶段，不把运行状态伪装成模型私有推理文本。

`web_search` 调用同一 some.im Origin 的 `/api/v1/customer/web-search`。`web_fetch` 只接受 HTTP(S) 公网目标，在请求前及每次重定向时解析 DNS，并拒绝私网、回环、链路本地和文档保留地址。GET 自行处理最多 8 次跳转：同 hostname 自动跟随，跨 hostname 在 `strict` 模式下重新审批，HTTPS→HTTP 降级拒绝；最终响应限制为 3 MiB 和最多 100,000 字符。POST 由 `method` 与 `body` 参数触发，请求体上限 1 MiB，一律不跟随重定向，在所有审批模式下逐次审批，且可按注册域名记成 Always Allow 规则。两项网络工具的 GET 与搜索只在 `strict` 模式下逐次审批。

### 实例身份（联邦 P0）

每台 willdeep 有一个终身不变的实例 UUID 和一个可改的名字，存在 `$WILLDEEP_HOME/identity.json`（0600，原子替换；首次使用时从 64 个内置名字里挑一个）。改名走 TUI/Web 的 `/name` 或 `willdeep identity rename`，UUID 不变——名字是标签，身份是 UUID。名字限定 1–24 个字符且显示宽度不超过 24 列，可用中文、日文等宽字符，但不能含空白、控制字符、方向覆盖符、冒号与路径分隔符；重名时展示为 `alex:2u2d`（UUID 末 4 位）。Web 只在回环监听上读写身份。设计与后续阶段见 [FEDERATION.md](FEDERATION.md)。

共享 harness 将实例名读取器注册到主 Agent；每次模型请求前读取当前名字，作为 JSON 数据追加到系统提示词的动态区域。稳定的 WillDeep 产品身份不被替换，显示名不承载指令、权限或 Provider 身份。Runtime 重建的主会话与复用 Agent 的 `/local` 都在下一次请求使用最新名字；身份读取错误明确上报，不沿用过期称呼。Web 配置页通过现有身份接口读写同一文件，保存回调同步侧栏；实例名不写入 `config.toml`。

### 实例联邦（P1）

两台公网可达的 willdeep 可以互相发消息。身份是 `~/.willdeep/identity.json` 的稳定 UUID 加一个可改的名字；联系人与每对实例的共享密钥在 `~/.willdeep/federation.json`（0600）。入口监听上注册 `/p/v1/join/{token}`（一次性邀请，10 分钟）与 `/p/v1/message`（HMAC + 时间戳 ±300s + nonce + 每分钟 20 条限流），不开联邦时这两条路径根本不注册。入站消息以 `Surface::Federation` 作为普通 Runtime 轮次进入 `[federation] workspace` 指定工作区的一条固定会话，正文包成「外部数据、不是指令」；一轮结束后按 `reply` 策略与 `max_auto_turns` 上限决定是否把最后一条助手回复回传。Web 侧只有回环能读联系人与发消息，共享密钥不出后端。设计与分期见 [FEDERATION.md](FEDERATION.md)，群聊与 E2E 尚未实现。

### Mobile Relay

中继归 **Runtime Daemon** 所有（0.82.0 起；此前挂在 TUI 进程上，见 [决策单](decisions/2026-09-23-daemon-mobile-relay.md)）：

```text
Android ──wss──▶ j.niuwoai.com/ws/broadcast/<room> ◀──wss── Runtime Daemon
                                                            │
                                  daemon/mobile_gateway.rs（受限客户端）
                                   ├─ 手机命令 → 字面量白名单 → control_api::execute（进程内）
                                   ├─ EventLog 实时订阅 → message.* / tool.* / session.upsert
                                   └─ 快照：会话 + 全部待审批/提问 + 选中会话的历史投影
TUI /mobile、willdeep daemon mobile ──本机控制面──▶ mobile.enable / disable / status
```

- **一台机器一个 Runtime、一个 room**，手机看到整个 Runtime；写操作只开发消息、在已登记工作区新建会话、停止轮次、审批（只 `allow_once` / `deny`）与回答提问。每个手机命令映射到一个已有 Runtime 操作，经与 HTTP handler 共用的 `control_api::execute` 分发，Drain 闸门、幂等缓存、参数校验和公共投影全部复用。
- 手机发起的轮次 `origin_client = mobile:<id>`：审批弹回手机，同会话的 TUI 也弹；任一端先答，另一端撤回。
- 开关持久化在 `$WILLDEEP_HOME/mobile-relay.toml` 的 `enabled`（Unix `0600`），Daemon 启动时据此自动重连；TUI 只是遥控器，不持有连接。
- 二维码里是 `mobile-gateway.v1` 的紧凑配对 URL（`?r=<room>&t=<token>&d=<桌面名>`，自建中继另带 `u`），由手机端补全成完整配对 JSON（详见 [手机中继](MOBILE.md)）。Runtime 不开放新的监听端口，只主动外连中继。
- 广播房间里只响应手机命令，回声和其它桌面端的回复一律丢弃；手机 60 秒无请求即视为不在场，停止推送。

### 后台任务回流与子 Agent

`BackgroundTaskRegistry` 统一跟踪 Shell Job 与后台 Subagent，保存有界输出、状态、耗时、退出码和取消通道。启动 Shell 的入口保持 crate 私有，且只能在 `run_command` 完成既有逐次审批后注册。任务终态会同时发布事件并形成 `<background-task-notification>` 或 `<subagent-report>`（格式由 `background_notice` 按后台任务合同 v1 渲染，脱离作业同一个渲染器）；TUI 把事件排队，主 Agent 空闲时立即续跑，繁忙时在当前 turn 结束后续跑。非交互 CLI 等待所有关联任务完成并逐个处理通知。

`SubagentCatalog` 公开 `generalist`、`implementer`、`tester`、`reviewer`、`ops_runner` 五种职责，与 `standard` / `advanced` / `expert` 模型档位正交；reader、judge、deep、scout、editor 等旧 ID 保留历史兼容。职责限定工具、写范围、窗口与轮数，模型由既有 Provider/档位和本机专业绑定解析。子 Agent 不装载 Subagent Catalog 或父会话管理宿主，不能递归派生或修改专业目录。

窗口纪律由客户端执行，具体职责窗口、工具输出上限与模型档位见 [子 Agent](SUBAGENTS.md)。普通 Worker 与工作流 agent 叶节点共享父会话 Scheduler，默认并发 8、排队 64，可由 `[agent_team]` 调整；工作流 driver、switch、foreach、repeat 不占 Worker 槽。

`spawn_agent` 的可选 `task` 参数是 Task Packet：目标、已知事实、约束、相关文件与 verifier 命令。Runtime 按窗口档位把相关文件内联进 Worker 首条消息，并在每次尝试后**由 Runtime 自己执行** verifier 命令来判定成败——退出码是唯一裁决，模型的自述不是。失败输出经纯 Rust 的确定性消化（失败聚焦段 + 尾部，断言原文逐字保留）后回灌下一次尝试；尝试打满即判运行失败并要求升档。命令型 Worker 共用三层门禁：静态安全命令直接放行；非破坏、非凭据敏感的模糊命令交 AI 判官；拒绝或不可用时精确返回父 Agent。父 Agent 仅能通过 `ops_runner + target_command` 请求人类对同一命令一次性确认，子 Agent 本身没有审批 UI，危险与凭据命令也绝不进入 AI。

写通道是一条通路的两种规模：写入型工种的目标文件先由父 ToolRegistry canonicalize 并作为**一个集合**整体审批，子 ToolRegistry 再以 canonical 绝对路径做集合成员匹配，符号链接或路径别名不能扩大授权范围；editor 是集合大小为 1 的特例。同一 Catalog 内运行中 Worker 的文件集互斥，冲突时后来者被拒绝并点名冲突文件，锁在运行结束、超时、取消或 panic 时随 `Drop` 释放。详见 [小上下文 Skill Worker](SKILL_WORKERS.md)。

### 完整子会话（0.144.0-rc1 P0/P1/P2）

完整子会话是独立 Runtime Session/root Agent，不使用 Worker Agent/Run 充当对话身份。`parent_session_id` 保存委派来源与家族归属，活动树形展示还需检查 `delegation_detached`；`started_by` 继续仅表示历史分叉来源，不能重新挂回已脱离会话。任务提出者/创建者与活动树形 parent 是独立概念。当前仍只允许有权父会话派生一层，不提供子再派生或兄弟任务提案入口。定义与分期见 [完整方案](decisions/2026-11-04-child-sessions.md)，使用见 [完整子会话](CHILD_SESSIONS.md)。

协议由 `willdeep-runtime-protocol::child_session` 唯一维护，capability 为 `session_children`（另声明 `session_children_fork/worktree/model/workers`）；`session.child.start/continue/retry` 与 `session.child.task.get/list/stop` 共用 Daemon 协调器，CLI/TUI/Web 和模型宿主不分别拼接创建与首消息。每次委派有独立 `assignment_id`、固定请求身份及对应 child Turn，续做/显式重试创建新 assignment/Turn，保留原会话历史与旧结果。Turn/Task 是执行状态的权威来源，Idle 仅表示当前没有执行，不代表整条子会话永久完成。

`$WILLDEEP_HOME/runtime/session-children.json` 是私有协调账本，记录关系、preparing/accepted 创建意图、assignment/Turn 关联、上下文来源、worktree 绑定、结果 outbox、通知身份与 `wake_batch_id`。协调锁内完成幂等、参数指纹与容量检查；受理点与父停止代数相互核验，崩溃后按固定身份补齐准备对象和已受理 Turn，不能凭多次文件 rename 宣称跨文件事务。Core `SessionStore` 仍是消息与附件的唯一事实来源，账本不复制聊天；结果正文引用或有界报告不另建消息历史。

上下文支持 `fresh` 与 `fork`。fork 只读取父会话已完成 Turn 的不可变消息前缀，校验压缩代次、消息边界与工具调用闭合，复制内联附件并清空 Goal、计划、队列、执行检查点、压缩检查点和运行游标；不使用手动 fork 的 `ensure_manageable` 闸门，也不等待父会话空闲。工作区支持 `shared_read_only` 与 `worktree`。worktree 在子 Turn 取得执行槽后创建，默认从已提交 HEAD 建立；`snapshot` 基线通过受控 patch 与未跟踪文件复制冻结父目录修改，并记录补丁摘要、未跟踪文件数和基线 commit。子会话 workspace 固定为该 worktree，权限最多为 `WorkspaceWrite`，不继承父会话 `FullAccess`；任务结束保留 worktree 和分支，不自动合并、提交、推送或删除。

可写 worktree 子会话可通过主 Harness 的 Worker 工具派工；父会话与其子会话共享一个进程内家族调度器和冻结的 `agent_team` 策略，Worker 排队/并发不因新 Session 重置。本机用量账本新增 `family_session_id`，主回合、Worker、压缩与辅助调用按记录 ID 去重后归入家族预算；每个子会话仍保留自己的 AgentRun 记录与所有权锁。只读子会话继续禁止 Worker、Shell、写入、MCP/插件副作用和继续派生；所有子会话都不安装完整子会话创建/续做工具，也不获得 peer 控制。

模型可由创建请求在父 Session 的 Provider 配置内覆盖。控制层异步校验模型列表、工具能力、图片兼容性与真实上下文窗口，将窗口快照持久化到子 Core/Runtime Session；Harness 优先使用该快照，绝不继承父模型在 Profile 中的窗口数值。

终态由对应 Turn 的最终 Task 投影为结果，先持久化 outbox，再通知父会话；报告是待核验内容，不是完成证据。父执行中在安全边界注入，Idle 时按固定 `wake_batch_id` 与 Turn request ID 续推；批次开始后冻结成员，确认消费依赖持久历史/回执，事件发布或客户端已读不能代替消费。重放按通知身份去重，保证至少一次投递并防止重复创建唤醒 Turn，不承诺模型或外部副作用 exactly-once。

父普通 Turn 收尾或 stop 不取消已受理的 child assignment；主动 stop 持久抑制父自动唤醒，结果保留待处理，只有用户明确继续父会话才解除抑制。宿主委派与子结果使用独立来源，不冒充用户输入。子任务停止按 parent/assignment/Turn 验证，归档/删除父会话不级联删除子数据。准入同时限制家族与宿主的子任务容量，P0/P1 执行槽/队列为 4/16，等待审批/回答仍占执行槽，列表分页不参与计数。

P2 在同一协调器上新增控制权与通知版本、稳定控制请求回执及不可变 assignment 依赖。账本 schema 2 保存用户接管/脱离的权威 Link；Core/Runtime 的 `delegation_detached` 只投影树形关系，父来源、能力、家族用量归属继续保存。执行中的旧 Session 保存必须单调保留脱离标记，控制锁内不等待 Core 执行租约。订阅 `none/inbox/auto_wake` 分离客户端待处理与模型自动续推，pending/唤醒 claim 按当前控制权和策略逐项过滤；宿主同步提交 hook 将最终资格复核与父历史 checkpoint 写入放在同一控制锁边界，成功后才替换模型内存，不持锁跨 await 或模型请求。撤销后的空唤醒通过专用正常收尾恢复父 Idle，保留真实停止抑制；终态执行结果保持原身份。依赖等待计入队列但不占槽，无 Turn、worktree 或模型调用，全部前置成功后按固定请求身份启动，失败/控制撤销形成 blocked。周期扫描与重启按会话最早未暂缓 queued Turn 的 `queue_sequence` 排序；实际子槽准入在同一控制锁、会话和 Turn 状态快照内再次选择最早可领用队首，避免较晚提示抢占新释放的槽。已有有效预留可恢复，暂缓、活动/等待人类、归档和无效唤醒不阻塞其他候选；有效唤醒只因公平性等待时不取消。领取前的 Core 加载、集成设置同步或恢复消息边界校验失败，先持久化带 Session/Turn/原因的 failed 终态，再释放预留槽，避免损坏队首扩大为跨父饥饿；协调器据真实终态生成失败结果并阻塞后继，不伪造 Task/成功报告、不覆盖损坏 Core，状态落盘失败仍传播。普通无父会话绕过子槽排队。周期调度跳过错过的 tick，消费者按通道长度快照合并同会话重复提示，保留不同会话的首次入队顺序及后续新提示。协调账本按最近成功落盘的完整内容比较，空事务和重复显式屏障不再次写入，首次创建、真实变更和迁移仍原子提交；普通收尾没有新增协作上下文时不额外保存 checkpoint，但仍复核空唤醒资格。保存成功后先更新 Recorder 的可恢复快照和流状态，再确认 Worker 报告；保存结果不确定时禁止错误收尾、Drop、流刷新和计划更新推测性回写，直到完整 record 成功，避免历史通知被旧快照覆盖。旧 schema 1 升级前备份，旧 Runtime 拒绝 schema 2，防止降级丢失控制约束。详见 [P2 决策](decisions/2026-11-05-child-sessions-p2.md)。

P0/P1 的历史本机验收见 [记录](verification/0.143.0-rc1.md)。P2 实现完成，专项与真实 Runtime 验收通过；原生前置超时与取消稳定性仍有未解决项，实际通过、失败和覆盖边界见 [P2 验收](verification/0.144.0-rc1.md)，不表示公开发布。

### 声明式工作流（0.141.0-rc1）

工作流定义与运行对象由 `willdeep-runtime-protocol::workflow` 唯一维护。`willdeep-core::workflow` 只负责有限 DSL、Schema/引用校验、状态转换和持久存储，不依赖 Provider、Agent、CLI 或事件实现。`agent` 是唯一调用模型的节点；`switch` 选择一个分支，`foreach` 为每项展开独立子图，`repeat` 按显式上限执行。每个作用域内部是 DAG；`phase` 仅分组展示，不产生调度屏障。有限 Schema 禁止 `$ref` 联网和未知关键字；模板引用单独使用 `input/item/previous/node:<id>#JSON-Pointer`，不能越过可见上游。完整字段、硬限与例子见 [工作流](WORKFLOWS.md)。

CLI `harness/workflows.rs` 通过独立 `WorkflowHost` 注入父 Agent。会话持有执行权时直接激活同一 BuiltHarness 的 driver，父模型结束后继续推进；无头等待、目标完成和唤醒也计入尚无 Worker 的活动图。外部 `daemon/workflows` 只完成持久准入与会话控制 Turn 排队，幂等锁内不等待模型或审批；同一会话复用既有执行器和锁。driver 的叶节点调用 `Agent::dispatch_workflow_node_with_catalog`，与普通 `spawn_agent` 共用准入，保留审批、基础职责、写集合、expert 票据、团队预算、文件租约及单层团队边界。宿主在第一次派工前固定整条运行的目录与档位模型快照，重试/恢复沿用源快照；冻结目录仍共享父会话的 Scheduler、审批句柄和文件租约。

`WorkflowStore` 按 `$WILLDEEP_HOME/workflows/<parent_session_id>/` 隔离定义与运行，一把文件锁保护运行状态与派工意图的原子替换。定义 ID 对应不可变正文，变更内容需新 ID；每个 run 固定 spec/input/limits。先持久化节点路径、展开位置、attempt/request ID，再向 AgentRun 派工；重启按 request ID 对账并复用已确认结果。无法确认的副作用保留 `interrupted`，等待明确恢复，不承诺跨进程 exactly-once。retry/resume 创建带 `source_run_id` 的新运行，旧成功节点带 `reused_from`，不重跑也不重复计费。

AgentRun 使用归属区分普通父派工与工作流节点。工作流收集叶报告，普通 `await_agents` 和完成通知只处理父派工；节点的普通续做/重试/发指令/停止入口拒绝修改，控制通过所属运行完成。父会话每个工作流运行接收聚合报告，先保存会话再 ack。终态图仍对账叶收据，补齐图已落盘而叶尚未确认的崩溃窗口。执行宿主释放时持久停止已接管的活动图并取消全量所属 Worker，保留外部刚受理而尚未接管的运行供后续控制轮次执行；未激活的只读宿主不改变其他执行者的运行。恢复排队叶前由工作流宿主核验图状态与节点意图，已停止的图不得重启 Worker。状态更新复用 EventLog 的持久游标与订阅；`conversation::project` 从真实控制回执投影卡片，Web/TUI 以持久运行状态重连，不从模型正文推导完成。

### 对话内专业智能体管理

`AgentDefinitionHost` 是独立父会话宿主接口，与 `WorkflowHost` 一起注入 ToolRegistry，不占插件的 LazyToolSource。父 Agent 使用稳定的 `agent_library_read` / `agent_library_manage`；CLI `harness/agent_library` 复用 `agent_registry/management`，Web/CLI 写入也走同一安装事务。只读工具不写入；管理工具受现有审批与审计约束，只读策略拒绝修改，Worker 不获得这些宿主工具。

定义正文 SHA（`definition_revision`）与本机安装代数（`installation_revision`）分离；更新、绑定、启停等写入以安装代数做 CAS。宿主捕获账号与库 scope，聊天宿主同时记录 list/get 观测到的 local/private 安装身份及修订，强制为后续写入补齐复合 CAS 的 target_scope；未观测旧目标先读，fork 固定源安装，同 request 重放保留首次目标。request ID 及参数指纹形成持久收据；scope 检查、创建不存在检查、CAS 与安装提交在同一锁内完成。同 ID 异参请求、过期修订和账号切换明确拒绝。官方正文先 fork 为 `user/`，本机绑定/启停不修改签名正文。旧安装状态兼容读取，首次成功写入迁移为带修订与收据的本机格式；可交换智能体包维持 v1。

专业目录使用共享且原子替换的快照，只替换专业定义部分、保留基础职责。对话保存成功后返回前刷新目录，本轮后续新派工即可读取新 ID；刷新失败返回 `saved_not_applied`，同时清空旧专业目录、保留基础职责，防止继续使用过期定义；可用同 request ID 重取收据并重试刷新。已经派出的 Worker 与续做/恢复使用原完整定义、技能、Provider/model 及执行限制快照，凭据在宿主重新解析，当前权限和预算重新核验。此刷新不重写历史或稳定工具前缀，也不主动分享或上传。安装字段见 [智能体库](AGENT_LIBRARY.md)，实际测试与隔离 Runtime 验收结果见 [验收报告](verification/0.141.0-rc1.md)。

### 出站通知

`[notifications]` 是事后旁路：任务完成、或 Agent 停下来等人处理时，把一句话发到外部。它与 `[[hooks]]` 是两件事——hook 在关键路径上、能拦动作；通知可以丢，失败只记一条错误，不影响轮次结果。一次事件先算一份 `willdeep.webhook.v1` payload，再按每条渠道自己的 `events` 过滤，给每条渠道起一个 detached 任务；HTTP 与邮件共用同一个请求超时，退出前 `flush` 最多等 6 秒。渠道有三种：Webhook（可选 HMAC 签名）、飞书自定义机器人（可选签名校验）、邮件（SMTP / Resend 两种可插拔后端）；密钥只写环境变量名，配置校验不检查变量是否存在——缺失只跳过该渠道。详见 [出站通知渠道](NOTIFICATIONS.md)。

### 用户交互与持久授权

Core 的 `Approver` 同时承载结构化 `ApprovalDecision`（AllowOnce / Deny / AlwaysAllow）和 `UserQuestion`。因此 `ask_user`、审批状态机及其工具结果属于 Harness，不依赖 SwiftUI 或 Ratatui；不同宿主只实现交互适配器。`ask_user` 支持 options、multi_select 和自由答案，答案使用 `<user_answer>` 边界并转义标记字符。

Always Allow 使用可撤销的窄签名。当前 Shell 仅保存无组合操作符的完整规范化命令，MCP 保存精确工具名；没有安全签名的调用不会向前端提供 AlwaysAllow。规则保存在 `$WILLDEEP_HOME/always-allow.json`，可用 CLI 列出或清空。后续 Swift App 复用 Rust Core 时，可替换规则存储适配器而保持相同决策语义。

### 插件宿主

插件是 Harness 之外的一层：它不进 Agent Loop，也拿不到工具注册表。一个插件贡献的是
**目的地**——一级入口、配套侧栏、中央页面与工具栏绑成同一个不可分割的导航状态；
真实业务能力走插件自带的 MCP 服务，宿主只回答"谁能跑、跑什么、拿得到什么上下文"。
反方向只有一条窄路：已启用插件的 MCP 工具经持久化目录（`plugin-mcp-tools.json`）
挂进聊天的 `list_mcp_tools` / `call_mcp_tool`，用到时才拉起插件，走与配置 MCP 工具
同一套审批。

插件 MCP 进程跨两个进程被用到：页面与**插件 MCP 网关**在 `willdeep web` 进程（网关是
单独的 `127.0.0.1` 监听，地址与 token 持久化在 `mcp-gateway.json`，给 Claude Code
等外部客户端用），聊天在 daemon / CLI 进程。聊天调用插件工具优先经网关，好让同一个
插件只有一个进程；没开 Web 时才由聊天进程自己拉起。网关只转发给本进程拉起的插件
公布的 HTTP 入口（`mcp-http.json` 的 `parentPID` 须是 `willdeep web` 自己的 pid，
macOS 数据目录里只认不带 `host` 的旧文件），否则经 stdio 中转，与 WillDeep macOS
同时开着也不会串到对方的插件进程。插件进程可以反向请求宿主出图、
问模型（`io.willdeep/host-requests`），凭据不出宿主。契约见
[decisions/2026-09-26-plugin-mcp-gateway.md](decisions/2026-09-26-plugin-mcp-gateway.md)。

包住在 `~/.willdeep/plugins/<id>/<version>/`，与 macOS 版共享；启用状态与权限审批
存 `~/.willdeep/plugin-registry.web.json`，**不共享**。分界线是"内容 vs 判断"：
包内容是同一份东西，而两个宿主的沙箱边界不同，一侧的审批不能替另一侧点头。这与
Always Allow 只共享精确命令、不共享命令族是同一条原则。

Web 宿主把页面放进 `sandbox="allow-scripts"` 的 iframe（opaque origin），CSP
`connect-src 'none'` 断掉它的一切网络出口，于是页面唯一能让宿主做事的通道是
postMessage 到父窗口，而父窗口只接受清单里声明过的命令 ID。分层与安全不变量见
[插件系统](PLUGINS.md)。

页面能力（桥 2.5.0：文件、命令、代发网络、结构化存储、技能清单、生图、
把文本递给主 Agent、宿主事件）实现在 `willdeep-cli` 的 `plugin_capabilities.rs`，
与既有的 `plugin_web.rs` 分开：后者是宿主的快照/命令/页面服务，前者是页面能让宿主
**动手**的那一面，权限与边界一律在这一层现场核。三条边界值得在架构层记下来：
`fs.*` 与 `process.run` 用的是与聊天端**同一个**工作区白名单对象（不是启动时的
副本，否则界面里加的工作区插件永远看不见）；`net.fetch` 的门是清单里的
`networkDomains` 而不是权限位；插件页面跑命令另有一层比主 Agent 更严的硬地板，
因为插件页上的确认框给不了用户判断所需的上下文。

### 本机用量账本

每次模型调用在 `$WILLDEEP_HOME/usage/YYYY-MM.jsonl` 记一行（`willdeep.usage-ledger.v1`，
canonical 规范在 Xedit `docs/USAGE_LEDGER_DESIGN.md`）。写入口在 Core 的
`usage_ledger`：有界通道 + 单个写线程，`O_APPEND` 单次写整行，IO 失败只警告。主回合、
子 Agent 与压缩由 `Agent` 在收到 usage 的同一处记账，并经 `EventSink::emit_sequenced`
拿到 daemon `events.ndjson` 的序号；辅助请求由 harness 用 `LedgeredProvider` 在构造处包好。
Mac 只读这份账本，rs 不读 Mac 的任何数据。细节与字段映射见 [本机用量账本](USAGE_LEDGER.md)。

## 尚未满足的长期架构要求

阶段 0 文档要求的 ACP 双轨验证、crate 级复用清单、协议 Schema 与跨客户端会话均尚未完成。当前实现是原生 Harness 起点，不能视为完整阶段 0 或阶段 1 产品。
