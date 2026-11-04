# Product Overview

> 最后更新：2026-11-05 | 当前开发版本：v0.143.0-rc4（工作树基线，不代表已公开发布）

本文件只保留产品定位、能力概要和文档入口，并控制在 20,000 个 Unicode 字符以内，以便项目参考资料可直接注入 Agent 提示。能力细节按需读取模块台账；操作参数、协议和验收结果以所链接的专题文档为准。

<a id="项目简介"></a>
## 项目简介

WillDeep CLI 是跨平台 AI Coding Agent 客户端。用户通过自有 Provider、API Base、凭据与模型，在受限工作区内完成对话、工具执行、协作派工和结果验证。CLI 自动化、Ratatui TUI 与内嵌 Web 共用 Runtime、Session/Turn、审批和持久事件，不各自维护一套执行事实。

当前工作树在完整子会话 P0 上补齐 P1：除 `fresh` + 共享只读外，子会话可从父会话已完成的 Turn 精确继承历史，或绑定独立可写 Git worktree；支持在父 Provider 配置内做受限模型覆盖，并按目标模型重算上下文窗口。可写 worktree 子会话可派出 Worker，父会话与子会话共享家族级并发/排队和预算，用量账本按家族根去重；只读子会话仍禁止 Worker、写入与继续派生，所有子会话都不能创建下一层子会话。worktree 在取得执行槽后创建，父目录未提交修改不会被静默带入；任务结束保留分支和工作树，不自动合并、提交、推送或删除。P1 已实现并通过本机回归与验收（Rust/Web、隔离 E2E、宽窄 PTY 与浏览器），本次提交尚未发布。详情见[完整子会话](docs/CHILD_SESSIONS.md)、[分期方案](docs/decisions/2026-11-04-child-sessions.md)与[验收记录](docs/verification/0.143.0-rc1.md)。

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
