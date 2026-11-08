# WillDeep 文档

项目介绍与快速上手见仓库根目录的 [README](../README.md)。本目录是完整文档，按"先会用、再懂原理"的顺序组织。

## 上手

| 文档 | 内容 |
|---|---|
| [安装与构建](INSTALL.md) | 从源码构建、Shell 补全、首次运行、开发者验证 |
| [配置指南](CONFIGURATION.md) | TOML 配置、Provider 与 API 两个维度、环境变量、界面语言 |
| [认证与凭据](AUTHENTICATION.md) | API Key 解析链、some.im 登录、Runtime Token、权限强制 |

## 三种使用方式

| 文档 | 内容 |
|---|---|
| [TUI 使用指南](TUI_GUIDE.md) | 快捷键、鼠标（含 SSH / tmux）、斜杠命令、工具活动显示 |
| [Web 端使用指南](WEB_GUIDE.md) | 启动参数、界面功能、断线重连、安全模型、JSON API、前端开发 |
| [CLI 参考](CLI_REFERENCE.md) | 完整命令树、`run` 自动化入口、退出码 |

## 产品模块台账

模块台账说明“当前有哪些能力、边界在哪里”，操作参数和协议仍以专题文档为准。

| 模块 | 内容 |
|---|---|
| [Web](product/web.md) | 聊天、项目与会话导航、Composer、配置工作台、活动面板与重连 |
| [CLI 与 TUI](product/cli-tui.md) | 自动化入口、终端输入与显示、附件、历史、快捷键、启动与诊断 |
| [Runtime 与会话](product/runtime-sessions.md) | Daemon、工作区、Session/Turn、队列、计划、定时任务、事件与恢复 |
| [智能体与协作](product/agents-collaboration.md) | 完整子会话、Worker、工作流、智能体库、会话协作与 Worktree |
| [工具与模型](product/tools-models.md) | Provider、模型档位、压缩、工具、Skills、规则与 MCP |
| [插件与外部集成](product/integrations.md) | 插件、公网入口、分享、联邦、手机、通知、交接、SDK 与 CI |
| [安全与验证](product/security-verification.md) | 审批、只读上限、沙箱、未知副作用、Diff、验证与审计 |
| [评测与 RSI](product/evaluation-rsi.md) | RSI、模型/可靠性评测、用量与反馈账本、派工指标 |
| [拆分前历史快照](product/history/overview-through-0.142.0-rc1.md) | 版本流水、旧状态与完整待办；仅代表归档时语境 |

## 能力专题

| 文档 | 内容 |
|---|---|
| [附件与 Runtime 定时任务](decisions/2026-10-12-attachments-runtime-schedules.md) | 附件管理、定时任务所有权与历史维护设计 |
| [0.98.0-rc1 验收](verification/0.98.0-rc1/README.md) | 实际预算、超时、Web 与终端图片协议验证 |

| 文档 | 内容 |
|---|---|
| [some.im 集成](SOMEIM_INTEGRATION.md) | 登录、自动识别、请求头、视觉回退、web_search |
| [Magpie 本地网关](MAGPIE.md) | 标准地址变量、共享 Profile、Runtime 环境与隔离联调 |
| [Magpie 社区 Agent](MAGPIE_AGENT_PLAN.md) | 独立 JSON 连接、临时 `magpie-cli` Provider、双端插件与验收边界 |
| [Runtime Daemon 与工作区](RUNTIME_DAEMON.md) | 常驻控制面、工作区策略、Session/Turn、事件流、Diff |
| [子 Agent 与后台任务](SUBAGENTS.md) | 八种 Profile、模型绑定、Worktree 审查与合并 |
| [完整子会话](CHILD_SESSIONS.md) | fresh/fork、只读/worktree、接管/脱离、结果订阅、任务依赖及验收边界 |
| [0.143.0-rc4 Windows Clippy 修复验收](verification/0.143.0-rc4.md) | 非 Unix 分支 `bail!` 后不可达语句的根因、修复与 Windows 目标交叉 clippy |
| [0.143.0-rc3 Windows Magpie 传输测试修复验收](verification/0.143.0-rc3.md) | Windows 子进程环境块与回环连接超时的根因、修复与诊断改进 |
| [0.143.0-rc2 Windows 栈溢出修复验收](verification/0.143.0-rc2.md) | daemon/Web 默认 CLI 构造的栈开销根因、修复与阈值回归 |
| [0.143.0-rc1 子会话 P1 验收](verification/0.143.0-rc1.md) | fork、worktree、模型覆盖、家族 Worker 与本机回归的实际结果及覆盖边界 |
| [0.143.0-rc1 子会话 P0 阶段验收](verification/0.143.0-rc1-p0.md) | P0 的 Rust/Web、隔离 API/E2E、宽窄 PTY 与浏览器结果；后续已并入 0.143 最终回归 |
| [Agent Runtime Kernel（任务书）](AGENT_RUNTIME_KERNEL.md) | 统一事件信封、三档中断、lease 与外部事件的移植计划 |
| [小上下文 Skill Worker](SKILL_WORKERS.md) | 工种分档、Task Packet、Verifier 闭环、文件集锁 |
| [检查点回退](CHECKPOINT_REWIND.md) | `/rewind` 回到第 N 步：每轮开始前的工作树快照存哪、回退时发生什么 |
| [双端共享模型目录](SHARED_MODEL_CATALOG.md) | Xedit / rs 共用 Provider、模型事实、候选池与凭据引用契约 |
| [审批与自动化](APPROVALS.md) | 三档审批模式、Always Allow 边界、CI 用法、`ask_user` |
| [本机用量账本](USAGE_LEDGER.md) | 每次模型调用一行的 `usage/YYYY-MM.jsonl`、字段来源、`willdeep usage backfill`（canonical 在 Xedit 仓库） |
| [本机反馈账本](FEEDBACK_LEDGER.md) | RSI 强反馈信号 `feedback/YYYY-MM.jsonl`：下一句建议的采用与改写、工具失败、Worker 轮次耗尽 |
| [协调 / 长任务 / 插件 / RSI 路线图](ROADMAP_ORCHESTRATION_RSI.md) | 主 Agent 协调与 goal 缺口自查、Xedit 定时任务与专家团插件化、反馈数据分阶段计划 |
| [智能体 RSI P4a 与 P4b](AGENT_RSI_P4AB.md) | 已实现的候选隔离、执行证据、免费配对验收和显式授权上传；真实评测尚待批准 |
| [受控原生 Worker 评测](AGENT_RSI_NATIVE_TRIAL.md) | 单独提案、本机预算代理、零网络首请求预览、真实读取证明与独立宿主评分材料 |
| [首轮实际价格与评分登记](decisions/2026-10-15-agent-rsi-live-preregistration.md) | 账号实际账单价、$5 硬上限、冻结宿主评分、单次授权、执行证据及保留集准入 |
| [RSI 后续分阶段方案](decisions/2026-10-15-agent-rsi-iteration-plan.md) | 三端现状、原规划剩余项及评测、晋升、灰度与回滚边界 |
| [首轮真实评测确认清单](decisions/2026-10-15-agent-rsi-paid-evaluation-proposal.md) | 模型、24 道原创审查题、双臂重复、价格快照和 5 美元总预算提案；待确认 |
| [Skills 与 MCP](SKILLS_AND_MCP.md) | Skill 发现规则、MCP 配置、项目上下文文件 |
| [内置插件](BUILTIN_PLUGINS.md) | 定时任务（`willdeep-scheduler`）与专家圆桌（`willdeep-roundtable`）：安装、工具、无人值守审批、与 Xedit 的差异 |
| [插件系统](PLUGINS.md) | 与 macOS 版共享插件包、三种页面运行时、页面桥与沙箱边界 |
| [手机中继](MOBILE.md) | `/mobile` 二维码配对与凭据安全 |

## 排查

| 文档 | 内容 |
|---|---|
| [故障排查](TROUBLESHOOTING.md) | doctor、常见报错、SSH 鼠标与剪贴板、退出码对照 |
| [RSI 提交与近期数据审查](RSI_REVIEW_2026_10_02.md) | 评测门禁、反馈指标和近期数据的证据与复核 |
| [RSI 审查独立复核](RSI_REVIEW_CLAUDE_2026_10_02.md) | 逐项确认 R1–R8、新增 N1–N5，以及各项的修复进度 |

## 协议与架构

| 文档 | 内容 |
|---|---|
| [架构](ARCHITECTURE.md) | 调用链、边界与模块划分 |
| [专业智能体定义 v1](AGENT_DEFINITION_V1.md) | Rust / Mac / some.im 的定义、固定版本、签名、私有同步与规范化契约 |
| [智能体库使用](AGENT_LIBRARY.md) | CLI、TUI、Web 的自定义、官方安装、账号同步与本地模型绑定 |
| [Provider 协议契约](PROVIDER_PROTOCOLS.md) | 三种线格式的请求/响应契约 |
| [Runtime 控制 API](RUNTIME_CONTROL_API.md) | 统一控制协议的完整契约 |
| [Runtime 会话协议](RUNTIME_SESSION_PROTOCOL.md) | Session / Turn 状态机与持久化 |
| [完整子会话补齐方案](decisions/2026-11-04-child-sessions.md) | 设计提案：子会话与 Worker 边界、上下文交接、完成回报与分期验收 |
| [进程内 Runtime Harness](IN_PROCESS_RUNTIME_HARNESS.md) | Daemon 进程内执行模型 |

## 路线与调研

| 文档 | 内容 |
|---|---|
| [CLI / TUI / Runtime 路线图](CLI_TUI_RUNTIME_ROADMAP.md) | 分阶段实施记录与验收 |
| [完整子会话补齐方案](decisions/2026-11-04-child-sessions.md) | P0/P1 历史分期、身份边界与故障恢复契约 |
| [完整子会话 P2](decisions/2026-11-05-child-sessions-p2.md) | 用户控制权、脱离投影、三档通知与不可变任务依赖 |
| [Goal Teams 引用与落地映射](GOAL_TEAMS.md) | 执行期角色 / 版本锚定里程碑 / 归档规范（canonical 在 Xedit 仓库） |
| [长程自主执行引用与落地映射](LONG_HORIZON_AUTONOMY.md) | 续推契约 / 预算与软停 / 计划工具化 / 重启可续（canonical 在 Xedit 仓库） |
| [Xedit 工具能力对照](XEDIT_TOOL_PARITY.md) | 工具覆盖度与 Computer Use 路线 |
| [Xedit 联动现状与路径](XEDIT_INTEROP_STATUS.md) | 文件层/契约层/运行时层三段式现状、漂移清单、建议路径（2026-08-21） |
| [竞争力分析](COMPETITIVE_LANDSCAPE.md) | vs pi / Claude Code / Codex / dsh，护城河与短板排序（2026-08-21） |
| [Herdr 研究与集成方案](HERDR_RESEARCH_AND_INTEGRATION.md) | 终端复用器集成的取舍与边界 |
| [需求单：rc29 收尾](decisions/2026-09-21-input-suggestion-followups.md) | 发布 rc29、Web 端下一句预测、README 演示 GIF、预测质量实弹、英文 README（待实现，给新会话开工用） |

## 文档约定

- 根目录 [README](../README.md) 面向“这是什么、值不值得用”，保持简短；
- 根目录 [PRODUCT_OVERVIEW.md](../PRODUCT_OVERVIEW.md) 保留产品定位、能力概要与模块索引，最多 20,000 个 Unicode 字符；预算内可作为 Agent 项目参考资料直接加载；
- `docs/product/` 是当前产品能力台账，说明模块范围、状态与限制；操作参数、协议、架构和验收仍由本目录对应专题维护，不在模块页复制另一套指南；
- 拆分前概览只作为历史快照，旧“当前”、勾选状态、版本号和实验授权不代表最新事实；
- 涉及安全边界的描述必须与实现一致。宁可写“当前不支持”，也不写模糊的承诺。
- README 首屏的 GIF 由 [`media/readme-demo.tape`](media/readme-demo.tape) 经 `ruby scripts/record_readme_demo.rb` 生成（真跑模型、临时演示仓库、≤ 3 MB）。改了 TUI 手感就重录。
