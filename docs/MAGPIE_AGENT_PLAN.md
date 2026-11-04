# Magpie 社区 Agent 插件执行台账

> Last updated: 2026-11-04 | 状态：**已实现，T0–T6 完成，T7 未授权**
> 实施基线：CLI 0.141.0-rc3（`b83372be`）；Desktop 1.426.0-rc1（Xedit `4cd9e147`）
> 交付版本：CLI **0.142.0-rc1**；Desktop **1.427.0-rc1**（Build 941）；插件 **0.1.0-rc2**；未发布、未社区收录
> 适用范围：CLI、Desktop、两个 Magpie Agent 插件
> 关联文档：[需求](MAGPIE_AGENT_REQUIREMENTS.md)、[设计](MAGPIE_AGENT_DESIGN.md)、[验收报告](verification/0.142.0-rc1/magpie-agents/README.md)
> 一句话：共享契约冻结，两端消费端与双插件已实现并通过 macOS 本地组合验收，未发布。

## 0. 已确认方向与当前阶段

2026-11-04 用户确认“按设计实现”。社区插件路线、数据格式、加载策略和首版边界 D1–D5 均已确认，参见设计第 3 节，不再重复询问路线或实现细节。当前为既有项目迭代：T0–T6 完成，T7 发布与社区提交未授权；交付为本地可安装源码，未发布、未社区收录。

设计文档保存在 willdeep-rs 作为跨端 canonical；Desktop 实施在 Xedit 文档补引用，不复制一份独立契约。实际验收结果与未覆盖项见[验收报告](verification/0.142.0-rc1/magpie-agents/README.md)。

## 1. 任务总览

| ID | 任务与文件范围 | 依赖 | 状态 | 验收 |
| --- | --- | --- | --- | --- |
| T0 | 用户评审设计 D1–D5 | — | 已完成（2026-11-04） | 用户确认“按设计实现”，D1–D5 移入已确认决策 |
| T1 | JSON Schema、有效/无效 fixture、预期状态 manifest；`docs/schemas/`、`docs/fixtures/` | T0 | 已冻结（已读取核对） | schema 与原始 fixture 固定；CLI、Swift、JS 对同一批数据的消费一致性仍由后续测试验收 |
| T2 | CLI 消费端；新窄模块、有效 Provider 目录、统一解析、Runtime/TUI/Web 新会话入口、来源/账本归类 | T1 | 已完成 | 隔离链 21/21；显式覆盖、固定会话、旧无固定选择会话恢复、daemon 先启动、断开后的错误语义均通过 |
| T3 | Desktop 消费端；新窄模块、私有支持标记、临时 Provider 目录、选择器/新会话解析和 i18n | T1 | 已完成 | 原生测试 19/19、契约测试 14 项/27 fixture；用户库/钥匙串未写；真实 App 点击组合验收未做 |
| T4 | 两个 Agent 包；`integrations/magpie/agent-willdeep*/`；描述符、connect、图标和单测 | T1 | 已完成 | JS 19/19、归档内原生测试各 2/2、真实宿主 12/12 |
| T5 | 隔离网关 + 两客户端 + 两插件闭环验证；Ruby 驱动和 JSON/Markdown 报告 | T2、T3、T4 | 已完成 | 最终二进制：隔离链 21/21、真实 Magpie 链 22/22；原配置不变与错误路径通过 |
| T6 | 两仓版本/CHANGELOG/产品及架构文档、安装说明、最低版本、收录材料 | T5 | 已完成 | CLI 0.142.0-rc1 / Desktop 1.427.0-rc1；版本、文档声明与实测相符 |
| T7 | 获授权后发布 npm/提交社区 registry | T6、发布授权 | 未授权 | 未执行；不把提交等同收录 |

T2/T3/T4 是同一交付的组成部分，已完成组合验收；T5/T6 的实际结果与未覆盖项见[验收报告](verification/0.142.0-rc1/magpie-agents/README.md)。T7 仍须单独授权。

## 2. 工作区与变更保护

- CLI 实施使用指定隔离 worktree `/Users/rocky/Sites/willdeep-rs/.claude/worktrees/magpie-community-agents`，基线 `b83372be` / `0.141.0-rc3`。原仓主目录只读原始三份需求、设计与台账，不在原仓根目录开发。
- 本轮文档同步只写该 worktree 的 `docs/MAGPIE_AGENT_REQUIREMENTS.md`、`docs/MAGPIE_AGENT_DESIGN.md`、`docs/MAGPIE_AGENT_PLAN.md` 和 `docs/MAGPIE.md`，保留原始详细约束并回写确认状态；不改冻结 schema/fixtures、版本、CHANGELOG 或其他架构文档。
- Desktop 实施基线为 `4cd9e147` / `1.426.0-rc1`，遵循 Xedit 根 `AGENTS.md` / `CLAUDE.md` 及各自隔离 worktree 的约束，不直接改共享 `develop`。本轮文档同步不声明其当前主目录或实施完成状态。
- 根据并发工作约束，编译和代码实现使用独立 worktree。核对基线所需能力，必要时明确指出尚未集成的依赖；不能把其他会话的未提交文件一并带走。
- 仅按明确路径暂存；不 stash/reset，不切换其他会话占用的分支。
- 本次不提交、合并、推送、打 tag 或发布；不读取真实配置、凭据、钥匙串，不调用收费模型，不安装或重启用户程序。后续发布须单独授权，并遵循各产品对应 release skill。

## 3. 必须验证的真实场景

### 3.1 安装与身份

1. 使用临时 Home 和隔离 Magpie，安装两个本地 Agent 包。
2. 只有 Desktop 私有标记、没有 CLI 二进制/私有文件时，不误显示 CLI 为已检测。
3. CLI 和 Desktop 各自单独连接与断开，另一端状态不变。
4. 验证包描述符最低版本提示；旧客户端不能被报告为已经消费新格式。
5. CLI 临时 Profile 固定为 `magpie-cli`，Desktop 临时 Provider UUID 固定为 `70D7B194-6038-4B32-8195-84F25E1648A3`；用户同 ID 条目冲突时报错，不覆盖。

### 3.2 CLI 与 Runtime

1. 先启动临时 Runtime，再通过隔离 Magpie 写入 CLI 集成文件；创建新会话，模拟上游收到选中模型 A。
2. 切换外部模型为 B，已有 A 会话继续 A；后续无显式选择的新会话使用 B。
3. 普通 `--profile`、`--model`/环境覆盖、自定义 `--config`、显式 Worker Profile 保留原语义；不能混用端点和凭据。显式其他 Provider、模型或连接覆盖不受无关集成文件 `invalid` 阻断，审批与沙箱设置保持原语义。
4. 分别验收关闭条目、切回非 Magpie 原生模型两种断开方式；即使留下仅有 `model` 的对象，新会话仍回原默认，不应用该模型。明确引用 `magpie-cli` 的会话下一轮返回不可用错误，不转发其他供应商。
5. 进行中的任务不断流、不被取消；断开不等于取消任务。
6. 网关自定义端口仍按远程 Magpie 推理记账，不计为本地免费调用。
7. 单测和集成测试不得读实际用户目录，自定义 `WILLDEEP_HOME` 不读取标准 Home 的连接。
8. 已固定 Provider/模型的会话恢复保留绑定；旧无固定 Provider/模型的会话恢复跳过集成默认值，沿原用户配置解析。覆盖本地执行、TUI、已运行的隔离 Runtime/Web 的实际调用入口，不能只验证配置解析器。

### 3.3 Desktop

1. 先启动隔离配置的 App，再连接插件；刷新选择器/回到前台后显示真实有效连接。
2. 新会话使用 A，外部改 B 后旧会话仍 A，新会话使用 B；旧无固定 Provider/模型的会话恢复跳过集成默认值。
3. 临时条目在会话恢复、模型目录和实际调用中可解析；不能只在 UI 插入一行。
4. 连接和断开前后比较测试 UserDefaults 库及测试钥匙串，确认没有集成引起的永久写入；不得读取真实用户配置或钥匙串。
5. 断开后原默认仍可用；原集成会话下一轮明确提示重连或改选，不转发其他供应商。显式其他 Provider、模型或连接覆盖不受无关集成 `invalid` 阻断。
6. App 首次启动标记和取消/错误状态不抢焦点，不触发模型付费探活；保留审批、沙箱与显式 Worker 设置。

### 3.4 数据和错误

已实际读取核对的冻结资料：[JSON Schema](schemas/magpie-agent.v1.schema.json)、[共享 fixture](fixtures/magpie-agent-v1/cases.json)。fixture 的 `contents` 必须原样传给消费者，保留重复键与损坏 JSON；`null` 表示文件缺失，不得先解析再序列化。

共享 fixture 和补充原生测试共同覆盖：缺失、`{}`、关闭、仅含合法 `model` 的未连接对象、有效双端样例、错误 ID、未知版本/字段、重复键、错误类型、blank/超长/带 C0/C1 控制符模型、非法数值、非回环 URL、userinfo/query/fragment、错误公开标记及真实 Key 形状。所有已提供字段须先校验；关闭状态也不得绕过非法字段错误。

冻结数值边界：文件最多 `65536` 字节，完整模型最多 `1024` UTF-8 字节且保留前缀；`context_window` 可空或 `4000..1048576`，`max_output_tokens` 可空或 `1..1048576`，两者均有值时 `max_output_tokens <= context_window`。`schema_version` 和元数据数字须为整数 JSON token，拒绝小数、指数写法（包括 `1.0`、`1e0`）。未知元数据为 `null`，不得编造。

URL 只接受小写 `http://` / `https://`；主机仅 `localhost`、规范四段 `127/8` IPv4 或 `[::1]`；IPv4 各段 `0..255` 且无前导零，拒绝简写和替代进制；可选端口 `1..65535`，路径精确为 `/v1`。拒绝 user-info、query、fragment、尾部斜杠、其他路径及主机别名/大小写变体；不能靠 URL 归一化接纳非法原文。localhost 实际请求固定回环，实际请求禁用代理与重定向。

I/O 覆盖：拒绝目录/符号链接、无权限、超限、截断、人为注入的不完整文件、同句柄校验以避免 check/open 竞态、有界重读后成功/失败、断开后不继续使用缓存。只有可重试 I/O 或截断在异步路径延后 `100ms` 重读一次；非法字段、版本或客户端错误立即报错。真宿主的正常模型更新单独验证其批量写入行为，不把模拟的不完整文件当作宿主必然行为。网络覆盖代理和重定向不生效，以及 localhost 回环固定。

## 4. 测试与报告

- Rust：相关原生单测、CLI 配置与 Runtime 集成测试；实施后按实际测试名固定命令。
- Swift：原生单元测试，包括 fixture、临时 Provider 解析、会话固定模型、测试 defaults/钥匙串隔离。
- JS：`connect` 和描述符原生单测；使用 yarn 运行，验证宿主无 Node API 也能加载。
- API/E2E：Ruby 驱动隔离进程，确定性本地 fake upstream，输出 JSON + Markdown。
- Magpie 真宿主验证：不仅调用 JS 函数，还检查 Agent 注册、模型选择、文件写入及恢复。
- Desktop 原生界面使用适用的原生运行验证；CLI/TUI 使用 PTY；仅当改动涉及 Web 可见行为时启动浏览器预览。不能用浏览器测试代替 Swift/CLI。

组合验收报告建议位于 `docs/verification/<实际版本>/magpie-agents/`，记录各二进制版本、源码 revision、场景、通过/失败/跳过和付费调用数。未运行的场景不得记为通过；本轮文档同步不创建验收报告，不把文档校验当作 Rust/Swift/JS、真实调用或端到端测试已通过。

## 5. 分发与社区收录

本地两个包先验收后再确定可用命名空间、版本、维护者和最低客户端版本。npm 包名、社区命名空间、registry 收录权限分别核验。

准备材料包括：两个条目的 ID/名称/图标、中英文摘要、安装和断开说明、源仓库、最低版本、已验收 Magpie 版本、包内容清单和测试证据。源码可先随本仓交付，进入社区仓库时按其包结构迁移并保留 canonical 契约链接。

主 Magpie 仓库当前不接受普通外部 PR；社区插件路线使用独立社区仓库流程。任何对外发布、提交社区申请或外部消息都在本地成品可审查后单独取得授权。

## 6. 当前状态和交接

- 已完成：官方插件能力核查、双端现状核查、社区路线确认、需求/设计/台账；2026-11-04 用户确认“按设计实现”，T0 完成。
- 已冻结：T1 的 schema 和原始 fixture；固定 Profile/UUID、字节上限、元数据范围、整数 token、严格 URL 与 localhost pin 均已回写，并由 Rust/Swift/JS 三端测试消费。
- 已完成：T2 CLI 消费端、T3 Desktop 消费端、T4 双插件，以及 T5 组合验收和 T6 版本文档同步。CLI 最终二进制 `6eb920c5…9e1f0c`，隔离链 21/21、真实 Magpie 链 22/22、真实宿主 12/12、Desktop 原生测试 19/19；完整证据见[验收报告](verification/0.142.0-rc1/magpie-agents/README.md)。
- 未覆盖：Linux/Windows 仅 Windows GNU 目标交叉编译；Desktop 未跑全仓完整测试、未做真实 App 点击组合验收；未签名、公证或安装。
- 未授权：T7 公开发布与社区提交；本次没有提交、合并、推送、发布或安装、重启用户程序的授权。
