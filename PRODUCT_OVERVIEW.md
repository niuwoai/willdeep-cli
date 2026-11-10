# Product Overview

> 最后更新：2026-11-10 | 当前版本：v0.147.0-rc22

0.147.0-rc19 修两处评测里反复出现的写错。`read_file` 每行是右对齐的六列行号再加两个空格，模型把整行行号、或只把那两个空格抄进 `edit_file` 的 `old_string` 后，各行都能找到、拼起来却匹配不到；现在每一行都是这种行号、或多行都多出这两个空格，而且去掉之后在文件里只出现一次时，按去掉前缀的原文替换。停掉子 Agent 时，进行中的模型调用没有用量是因为上游没来得及返回，不再把整队的下一次请求判成预算无法核验；正常完成但没有用量的调用仍照旧拒绝。

0.147.0-rc16 让每条消息记下产出它的模型。此前聊天区的模型名取的是会话当前的模型：切了模型，旧消息全被改标成新的；而跨 Provider 切换还漏了更新，于是切到 Qwen 之后每条消息仍标着 glm-5。现在模型名在请求发出、消息落进记录时写上，显示只认这条记录；旧会话没有记录的消息只留时间，不再拿当前模型去猜。本次改动尚未发布。

0.147.0-rc15 修复评测留下的空转 daemon。每个任务的 `willdeep run` 都会给自己的临时 home 拉起一个脱离进程组的 Runtime Daemon，调用方的清理杀不到它，它又每几秒重写一次租约，攥着目录让删除报 `Directory not empty`，于是目录和进程一起留下，每晚一轮就积一批。现在 daemon 在连续 15 分钟既没有客户端连接、也没有排队或执行中的轮次、也没有开启的服务时自己退出；评测收尾先逐个 home 停掉 daemon，停不掉再按它自己写下的 pid 发信号，然后才删目录，删不掉只记一笔，不再把已经跑完的一轮判成评测失败。本次改动尚未发布。

0.147.0-rc14 修复终端图片列的生命周期：willdeep 此前从不发结束帧，新进程启动也只发 `hello`、不报初始列表，于是退出客户端再进来时，右侧图片列还挂着上一轮草稿的缩略图（草稿本身其实已经清空）。现在退出时发 `bye`，启动时紧跟 `hello` 报一次当前列表，终端据此重画或收起这一列；删除图标对草稿里已经没有的 id 也回 `remove`，残留缩略图点一下就能去掉。本次改动尚未发布。

0.147.0-rc13 修复 TUI 在 MuseTerm 里完全收不到键盘输入：rc12 把标准输入换成管道时，crossterm 的事件源已经注册在真实终端上，之后的按键被转发线程读走，事件循环再也等不到。现在标准输入换成一个私有 pty，两端都是终端，且在任何读取之前装好，事件源与读取始终指向同一个描述符。输入过滤同时改为只扣住可能成为帧头前缀的尾巴，单独 Esc 50 毫秒后放行，箭头键等完整序列立即通过。本次改动尚未安装或发布。

0.147.0-rc12 修复 daemon 空闲时持续占 CPU：子会话对账每 250 毫秒跑一轮，此前每轮都把整份任务账本读进内存解析。账本里没有任务、槽位和唤醒批次时这一轮直接返回。本次改动尚未安装或发布。

0.147.0-rc11 修复终端图片列在 Windows 上的编译：输入过滤改用 Unix 管道，Windows 上 `install` 返回不支持，不写 `hello`，终端也就不发送图片列字节。rc10 的发行验证因此在 Windows 编译失败，未发布安装包，保留原 tag 并递增 rc 重发。功能与 rc10 相同。

0.147.0-rc10 收口这一轮 TUI 改动。MuseTerm 里运行时，粘贴的图片出现在终端右侧的缩略图列里，点删除图标会拿掉对应的草稿附件，两边用私有序列 OSC 7771 对话，字节在进入按键解析之前被拿掉。用户消息发出后、模型开口前，消息正下方显示一行 `.` `..` `...` 每秒轮换的等待动画，思考行、正文、工具或计划一出现就撤掉，压缩上下文时让位给压缩状态行，不写入会话记录。另修折行与三处输入回显：中文标点和 markdown 行内符号也作为断点，整句不留空格的文案不再从字中间硬切；继续一条已归档的 Runtime 会话时，发送前自动解除归档，不再报 `Runtime Session is archived`（服务端拒绝向归档会话入队的限制不变，只读打开历史也不改状态）；粘贴三行以内且不超过 2000 字直接进输入框，更长的才收成附件；用户消息在聊天区逐条标出「附件 N」，文本附件的正文直接可见，点标签可查看（文本开预览、图片进图片查看器），重开会话后同样显示。本次改动尚未安装或发布。

0.147.0-rc9 调整 TUI 聊天区行距：同一轮里用户消息与后续 Thought、正文不再空行，「本轮结束」分隔线与下一轮用户消息之间留一行。聊天配置快照补入每个 `[subagents.*]` 的 `provider_profile` 与 `model`，模型查看配置时能读到工种绑定；写入范围仍只有三个压缩字段。本次改动尚未安装或发布。

0.147.0-rc8 改进 TUI 启动会话选择框的“最后使用”时间：超过 90 分钟按小时显示，超过 24 小时按天显示，并同步中、英、日文案。本次改动尚未安装或发布。

0.147.0-rc7 为 Runtime 事件日志加上稀疏字节偏移索引：读取不再从文件头逐行解析整份日志，启动也不再把它全部读进内存；日志涨到 168 MB / 90 万条后，事件流、健康检查与控制面不再被全量扫描拖住。同时停止发布 Windows x64 发行包（CI 仍保留 Windows 编译与测试），macOS Universal、Linux 与 npm/Homebrew 分发不变。功能与 rc6 相同。

0.147.0-rc6 修正手机网关测试夹具的持久账本布局，使其与生产 `DaemonPaths` 一致；生产功能及确认策略沿用 rc5。rc5 完整发行验证因该夹具失败而阻止安装包发布，原 tag 保留，修复后递增 rc 重发。

0.147.0-rc5 整合 Web 聊天交互与摘要复用修复：失败原因通过统一安全投影展示，实时流与历史恢复共用；聊天明确请求的子会话生成完整提议，点击“开始”后执行，空会话不再常驻手填卡；“需要处理”按会话列出原因，点击计数定位列表、点击标题进入会话。长历史连续工具轮次仅以稳定规则和已覆盖历史校验摘要缓存，计划、目标和执行预算等实时状态在压缩后回填并提前预留容量，防止重复全量摘要。设计见 [Web 聊天交互](docs/decisions/2026-11-07-web-chat-actions.md)；独立分支的[Web 验收](docs/verification/0.147.0-rc4/README.md)与[摘要复用核查](docs/verification/0.147.0-rc4/context-compression.md)保留实际版本标记，组合源码的验证与发布结果单独记录。

0.147.0-rc4 为摘要复用独立分支的验证版本，未单独发布；修复已纳入 rc5。

0.147.0-rc3 修复审批被提前收起的问题：独立消息通过检查并领取成功后才暂时收起原请求，依赖不明、分类失败或领取失败时保持可见，没有独立任务运行时恢复遗留隐藏状态。TUI/Web 忙时反馈已观察到的进展或等待原因，并区分持久入队与模型已读取；等待人工处理时普通 Enter 保留草稿并恢复入口，Cmd/Ctrl+Enter 可明确入队。最终组合源码已通过 171 项定向测试、1 项 Runtime 集成、严格 Clippy、Web 构建及隔离浏览器验收，前两组提交另各自通过构建检查。后续完整工作区补跑出现超时失败，已分别记录隔离复跑、夹具修正和跨平台验证边界；详见[本轮整合验收](docs/verification/0.147.0-rc3/integration-review.md)。[rc3 历史验收记录](docs/verification/0.147.0-rc3/README.md) 保留独立证据边界，发布与安装状态另行核验。

0.147.0-rc2 修复 TUI 与 Web 会话中的计划卡重复：实时回执与历史回放均原位更新当前卡片，结构变化时清理旧详情，保留卡片位置与展开状态。源码增量仅涉及 Core 对话投影及 TUI 计划展示；思考等级能力来自前置 rc1。[历史验收记录](docs/verification/0.147.0-rc2/README.md) 覆盖两者；后续独立构建检查及最终组合源码回归见[本轮整合验收](docs/verification/0.147.0-rc3/integration-review.md)。

0.147.0-rc1 为每个 Provider Profile 增加可自定义的思考等级、默认等级与协议参数映射；TUI 可用 `/reasoning` 选择会话等级，Web 在会话工具栏提供选择器，并在配置工作区编辑映射。等级在任务领取时快照，正在执行的任务不受切换影响。空会话恢复、运行中切换、跨 Provider 派生、显式等级名称和配置往返边界一并纳入；Web 重新打开选择器时刷新当前选择。接口和持久字段见 [Runtime 控制 API](docs/RUNTIME_CONTROL_API.md) 与 [会话协议](docs/RUNTIME_SESSION_PROTOCOL.md)。独立构建检查及最终组合源码回归见[本轮整合验收](docs/verification/0.147.0-rc3/integration-review.md)。

0.146.0-rc1 增加首次目录的审批模式引导：默认智能审核；希望亲自值守时可选严格模式，愿意给予更大自主执行权限时明确选择完全访问。继续复用现有工作区/会话审批值和权限面板，不引入独立信任状态；原工作区访问档、只读上限及审批语义保持。设计与验收要求见[首次目录审批引导](docs/decisions/2026-11-06-workspace-trust.md)。本功能尚未安装或发布。

0.145.0-rc4 修复终端 Enter 后反馈被 Runtime 请求阻塞的问题：普通消息先回显，模型确认先关闭选择弹窗并显示切换中，再展示真实执行结果。操作保持原有串行顺序；当前轮次中的模型变更仍在结束后生效。验证与部署状态见[即时反馈验收](docs/verification/0.145.0-rc4/README.md)。

0.145.0-rc3 补入嵌套 Git 仓库与 linked worktree 的验收快照修复：递归采集实际内容并保留安全错误诊断。完整子会话 P2、原生 Runtime 测试进程回收、聊天内置配置下一轮生效均已核对保留；本轮来源对照与验证见 [rc3 整合验收](docs/verification/0.145.0-rc3/README.md)。

0.145.0-rc2 在完整子会话 P2 工作树整合聊天压缩配置、原生 Runtime 测试进程回收，以及 0.143.0-rc8 的首答计时与无变化账本写入修复。rc2 修复 Windows 上两个测试假设（审批描述按规范路径比较、迁移重试前释放账本句柄），并把 Windows 的内置配置服务启动期限放宽到 30 秒（其他平台保持 10 秒）。rc1 因这两处测试假设未通过 Windows 验证、未发布；rc2 复测又暴露首次配置编辑的冷启动超时，一并修复。配置入口只编辑 `[agent]` 的三个 `compressor_*` 字段，绑定当前配置文件，经现有审批保存；下一次新 Runtime Turn 读取新配置，当前 Turn 保持原快照，TUI 手动 `/compress` 刷新压缩设置。范围与时机见[配置指南](docs/CONFIGURATION.md#在聊天中修改压缩配置)。

`--web` 启动已改为惰性插件宿主：内置插件安装、包发现与 MCP 网关移出端口绑定路径，`/health` 秒级可达，首个插件请求才完成准备；headless 一次性调用不再预装只服务交互面板的包。最终源码的静态九项、Windows 交叉、Runtime 八组、客户端与浏览器复测通过；`headless_runtime` 完整目标在高负载下 5 项超时用同一二进制隔离重跑全部通过，64 Worker 宿主取消用例在负载下仍可能超原期限。来源与证据边界见[本版整合验收](docs/verification/0.145.0-rc1.md)。

0.143.0-rc8 的首答显示包含准备与传输耗时，子会话账本只在内容真实变化时写入；历史证据见[首答延迟验收](docs/verification/0.143.0-rc8/README.md)。原 macOS universal 架构检查、Windows CI 测试并行度限制与 Web 插件固定入口修复继续保留。

本文件只保留产品定位、能力概要和文档入口，并控制在 20,000 个 Unicode 字符以内，以便项目参考资料可直接注入 Agent 提示。能力细节按需读取模块台账；操作参数、协议和验收结果以所链接的专题文档为准。

<a id="项目简介"></a>
## 项目简介

WillDeep CLI 是跨平台 AI Coding Agent 客户端。用户通过自有 Provider、API Base、凭据与模型，在受限工作区内完成对话、工具执行、协作派工和结果验证。CLI 自动化、Ratatui TUI 与内嵌 Web 共用 Runtime、Session/Turn、审批和持久事件，不各自维护一套执行事实。

完整子会话 P0/P1 支持 `fresh/fork`、共享只读或独立可写 Git worktree、受限模型覆盖及家族 Worker 配额。P2 支持用户接管、脱离父关系、三档结果订阅和 assignment 依赖；接管不取消已有执行，脱离保留历史、能力上限和家族用量归属；依赖等待不占执行槽，全部前置成功后启动，失败明确阻塞。所有子会话仍禁止递归派生，worktree 不自动合并、提交、推送或删除。功能与验收边界见[完整子会话](docs/CHILD_SESSIONS.md)、[P2 决策](docs/decisions/2026-11-05-child-sessions-p2.md)；P2 来源检查与限制见[原阶段验收记录](docs/verification/0.144.0-rc1.md)，组合版结果另记于[本版整合验收](docs/verification/0.145.0-rc1.md)，P0/P1 历史结果见[阶段记录](docs/verification/0.143.0-rc1.md)。

0.142.0-rc1 接入 Magpie 社区 Agent 插件的独立 JSON 配置源：WillDeep 与 WillDeep CLI 各自连接，CLI 通过临时 `magpie-cli` Provider 解析，无显式选择的新会话采用当前连接，已有会话保留自己的 Provider 与模型。原 TOML、桌面 Provider 库与钥匙串不作为插件写入目标。契约、实现状态与验收台账见 [Magpie 集成](docs/MAGPIE_AGENT_PLAN.md)；本地实现与组合验收已完成，尚未公开发布或社区收录，Linux/Windows 原生执行与 Desktop 全量测试未覆盖，详见 [验收报告](docs/verification/0.142.0-rc1/magpie-agents/README.md)。

<a id="核心功能"></a>
## 核心功能

- **多端交互**：CLI、TUI 与 Web 支持持久对话、附件、历史、模型/权限选择、活动状态和断线恢复；客户端草稿与 Runtime 已提交队列分开管理。
- **常驻 Runtime**：Daemon 管理多工作区、Session/Turn、计划、定时任务、人工等待、事件游标和恢复；公开客户端复用统一控制 API。
- **协作执行**：完整子会话、专业 Worker、声明式工作流、智能体库、会话协作与 Worktree 派工各有独立身份、权限和恢复边界。
- **工具与模型**：支持多 Provider、三种线协议、模型档位、上下文压缩、文件/Git/Shell/Web 工具、Skills、规则和 MCP 客户端。
- **外部集成**：插件、公网入口与分享、实例联邦、手机中继、出站通知、Mac 交接、Magpie 社区 Agent、终端适配及 Runtime SDK/CI 入口复用宿主安全边界。
- **安全与证据**：四档审批、只读工作区硬上限、OS 写入/网络围栏、未知副作用保护、Diff 快照、验证证据、检查点回退与审计导出共同约束执行。
- **评测与改进**：RSI 实验、模型评测、用量/反馈账本和派工指标保留样本、预算与证据限制；工程链路通过不等于质量提升或候选晋升。

## 产品模块台账

| 模块 | 当前能力与边界 |
|---|---|
| [Web](docs/product/web.md) | 浏览器聊天、项目/会话导航、Composer、配置工作台、活动面板与重连 |
| [CLI 与 TUI](docs/product/cli-tui.md) | 自动化入口、终端输入与显示、附件、历史、快捷键、启动和诊断 |
| [Runtime 与会话](docs/product/runtime-sessions.md) | Daemon、Workspace、Session/Turn、队列、计划、定时任务、事件与恢复 |
| [智能体与协作](docs/product/agents-collaboration.md) | 完整子会话、Worker、工作流、智能体库、会话协作和 Worktree 派工 |
| [工具与模型](docs/product/tools-models.md) | Provider、模型档位、配置、压缩、工具、Skills、规则和 MCP |
| [插件与外部集成](docs/product/integrations.md) | 插件、Magpie 社区 Agent、公网入口、分享、联邦、手机、通知、交接、SDK 与 CI |
| [安全与验证](docs/product/security-verification.md) | 审批、只读上限、沙箱、公共投影、未知副作用、Diff、验证与审计 |
| [评测与 RSI](docs/product/evaluation-rsi.md) | RSI、可靠性/模型评测、用量与反馈账本、派工指标及证据限制 |
| [历史快照](docs/product/history/overview-through-0.142.0-rc1.md) | 拆分前概览全文；旧“当前”、版本流水、待办和勾选状态只代表当时语境 |

产品路线与逐项验收状态继续见 [CLI/TUI/Runtime 路线图](docs/CLI_TUI_RUNTIME_ROADMAP.md)；技术栈、模块边界、数据流和调用链见[架构文档](docs/ARCHITECTURE.md)。

<a id="技术栈"></a>
<a id="项目结构"></a>
<a id="已知问题与后续"></a>
旧版“技术栈”“项目结构”“已知问题与后续”已分别迁入架构入口、对应模块的当前限制及[历史快照](docs/product/history/overview-through-0.142.0-rc1.md)，不再在根概览重复维护。

<a id="运行"></a>
## 运行与进一步阅读

- 安装、源码构建与平台产物：[安装与构建](docs/INSTALL.md)。
- Provider、凭据、模型和工作区配置：[首次使用](docs/FIRST_USE_SETUP.md)、[配置指南](docs/CONFIGURATION.md)、[认证与凭据](docs/AUTHENTICATION.md)。
- 交互入口：[TUI 使用指南](docs/TUI_GUIDE.md)、[Web 使用指南](docs/WEB_GUIDE.md)。
- 自动化命令、输出格式和退出码：[CLI 参考](docs/CLI_REFERENCE.md)。
- Runtime 部署与控制：[Runtime Daemon](docs/RUNTIME_DAEMON.md)、[控制 API](docs/RUNTIME_CONTROL_API.md)、[会话协议](docs/RUNTIME_SESSION_PROTOCOL.md)。
- 全部文档按用途分类见[文档目录](docs/README.md)，逐版变化见[变更记录](CHANGELOG.md)。
