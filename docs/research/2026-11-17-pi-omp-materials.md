# Pi 与 OMP：文章及对照图资料记录

> 整理日期：2026-11-17。资料来源：用户在本次讨论提供的中文文章及两张对照图。
> 本文件先忠实记录材料，**不把材料陈述自动视为已经独立核实的产品事实**。截图标注的最后更新时间为 2026-11-16，不代表本次已复现该日上游状态。
> 本轮属于既有项目的文档调研，不修改功能代码、版本号或既有发布记录。

## 1. 材料的核心判断

文章将 Pi 和 OMP（Oh My Pi）描述为同源的两条路线：OMP 从 Mario Zechner 的 Pi 分叉，保留终端交互、TypeScript 扩展、会话管理及多模型支持，然后增加更多原生开发能力。

- **Pi：精简内核、由用户组合工作流。** 基础工具主要为 read、write、edit、bash；通过 TypeScript 扩展注册工具、拦截事件、修改上下文和定制界面。子 Agent、计划、权限等工作流可由社区扩展提供。文章特别指出当前 Pi 已内置 MCP 和 Codemode。
- **OMP：保留扩展、强化原生集成。** 将代码分析、调试、子 Agent、搜索、浏览器、桌面操作等放入运行环境，减少安装和组合工作。文章称有 31 个内置工具、约 8 万行 Rust 底层代码，部分能力须额外配置。
- **真正的分歧是默认产品边界。** 两者都可扩展、都有 SDK/RPC；差别在于哪些能力默认由产品维护，哪些交给使用者搭建。
- **成本分配不同。** Pi 把更多配置及扩展组合成本交给使用者；OMP 承担更多集成、权限、兼容及模块维护成本。内置工具多不等于实际任务成功率高。

## 2. 文章分维度记录

| 维度 | 文章中的 Pi | 文章中的 OMP | 后续比较时的注意事项 |
|---|---|---|---|
| 架构 | 小内核、TypeScript 扩展、可改上下文与界面；内置 MCP/Codemode | 沿用 Pi 架构并扩充原生运行环境；TypeScript 加 Rust 底层 | 比较模块责任与默认体验，不能只比较语言或代码行数 |
| 代码理解 | 文件工具与 Shell 为基础，可接外部能力 | 原生 LSP、AST 搜索/编辑、DAP 调试 | 区分文本搜索、语义导航、结构化修改和运行期观察 |
| 编辑可靠性 | 文章未进一步描述其冲突保障 | Hashline 内容锚点，文件变化后拒绝不匹配编辑 | 比较防止陈旧覆盖的实际机制，不要求工具必须同名 |
| 多 Agent | 社区 pi-subagents：并行、串联、后台任务 | 内置 task、隔离工作区、结构化结果、Agent Hub | 原生入口、扩展方案和运行中用户介入分别核查 |
| 联网 | 如 pi-web-access 等扩展 | web_search、browser、computer | HTTP 正文读取、网页自动化、真实桌面控制是三种能力 |
| 扩展分发 | Pi Package，npm/Git，官方包目录 | 独立插件管理和 Marketplace，npm/Git/本地，兼容 Claude Code Marketplace 目录格式 | 包可发现、可安装、API 可用、行为相同是不同兼容层 |
| Pi 扩展兼容 | 自身平台 | 部分 Pi Package 声明与旧导入兼容 | 文章明确不保证所有 Pi 插件可直接迁移 |
| 模型 | 多 Provider、部分订阅授权、/model、models.json、自定义 Provider | 60 多 Provider，任务角色路由、失败回退、凭据轮换、项目路径模型限制 | Provider 数量不等于协议数量、模型数量或实测效果 |
| 角色路由 | 用户自行组织 | default、smol、slow、plan 等 | 角色与成本/能力档位可以正交，不宜直接照搬名称 |
| 会话及压缩 | 会话树、分支、自动压缩；扩展可接管上下文 | 会话树、输出裁剪、多策略压缩、可选后台预压缩 | 对比保真、延迟、总调用成本和恢复正确性 |
| Snapcompact | 未提及 | 将部分历史渲染为紧凑图片，交给视觉模型 | 图片也消耗上下文，效果须同任务实测 |
| 长期记忆 | 可通过扩展组织 | 可选跨会话持久记忆，保存、检索、更新 | 规则文件、聊天历史和自动检索记忆不能混算 |
| 生态 | 截至 11 月 16 日，官方 npm 包目录 5,366 项，含扩展/Skills/模板/主题 | 无可直接对比的统一收录量；可用自身及部分外部生态 | 缺少统计口径和可复现快照，不据此给出生态倍数 |
| 适合人群 | 深度定制、SDK 构建者 | 希望安装即具备完整开发工具的日常使用者 | 这是文章的使用建议，不是统一质量排名 |

## 3. 两张对照图的逐项转录

以下保留图片中的标签和勾选含义；“是/否”只表示图片原文。第一张覆盖基础属性至语音，第二张覆盖 IDE 至开发状态。

| 图中项目 | OMP | Pi |
|---|---|---|
| Tagline | Oh My Pi (omp), a coding agent with the IDE wired in | A minimal terminal coding harness |
| Website | https://omp.sh/ | https://pi.dev/ |
| Language | typescript | typescript |
| Release Date | December 31, 2025 | December 2, 2025 |
| Last Updated | November 16, 2026 | November 16, 2026 |
| Provider Type | Independent | Independent |
| MCP Support | 是 | 是 |
| Multi-Agent Support | 是 | 否 |
| Git Integration | deep | basic |
| Multi-File Editing | 是 | 是 |
| Auto Test Running | 是 | 是 |
| Sandbox / Permissions | 是 | 否 |
| Image / Multimodal Input | 是 | 是 |
| Voice Mode | 是 | 否 |
| IDE Integration | Zed (via ACP) | None, terminal only |
| Hooks / Extensions | 是 | 是 |
| Web Search | 是 | 否 |
| Local Model Support | 是 | 是 |
| Context Window | Model-dependent | Model-dependent |
| Supported Models | 60+ providers，约 1000 models，9 routing roles；列举 Anthropic、OpenAI、Gemini、Vertex、xAI、DeepSeek、Mistral、Groq、Cerebras、Fireworks、Together、Bedrock、Azure、OpenRouter；本地列举 Ollama、LM Studio、llama.cpp、vLLM、LiteLLM | 20+ providers；列举 Anthropic、OpenAI、Google、Mistral、OpenRouter、Groq、Ollama、xAI、Azure、Bedrock |
| Supported Subscriptions | Claude Code、ChatGPT、SuperGrok、GitHub Copilot、Kilo Pass、OpenCode Go / Zen、MiniMax Token Plan | Claude Code、ChatGPT、GitHub Copilot、SuperGrok、X Premium |
| Pricing Model | Free, BYOK | BYOK |
| Free Tier | 是 | 是 |
| Price Range | Free (BYOK) | Free (BYOK) |
| Source Code Availability | Open Source (MIT) | Open Source (MIT) |
| Source Code | https://github.com/can1357/oh-my-pi | https://github.com/earendil-works/pi |
| Development State | ACTIVE | ACTIVE |

## 4. 不能从图中直接推出的结论

1. **“否”可能只指没有内置入口。** 文中已说明 Pi 可用扩展提供子 Agent、联网和权限工作流；截图没有给出 native / extension / external 的统一口径。
2. **Sandbox 与 Permissions 被合并。** 工具执行前审批和 OS 强制围栏不是同一能力；还需检查支持平台、默认开关、文件/网络边界及失败时是否拒绝运行。
3. **多文件编辑不等于语义重构。** Agent 逐文件替换也可算多文件编辑，不能据此推出支持语言服务器引用更新。
4. **自动跑测试不等于独立验证。** 需区分模型决定运行命令、宿主执行既定 verifier、是否有退出码和对应代码快照证据。
5. **Web Search 不代表 browser/computer。** 搜索、抓取、浏览器登录交互、DOM 操作、桌面截图/系统输入/无障碍树分别核查。
6. **BYOK 免费指客户端许可，不代表推理免费。** API、订阅、搜索服务和远端基础设施可能另行计费；“支持订阅”也不能代替鉴权流程、权限和服务条款核对。
7. **TypeScript 标签没有覆盖整个运行时。** 文章另提 OMP 的 Rust 模块，两种表述并不必然冲突，但不能称其全部实现均为 TypeScript。
8. **数量、日期与仓库地址需要原始来源。** 31 工具、约 8 万行 Rust、60+/20+ Provider、约 1000 模型、9 角色、5,366 包和首发日期均缺少材料内的可复现统计依据；文章的 Pi 作者来源与图中仓库归属也需核对迁移关系。
9. **上下文大小取决于完整请求。** 除模型声明窗口，还受工具 Schema、附件、推理、输出预留、代理限制和压缩策略影响。
10. **可安装不代表可兼容。** Pi Package、TypeScript 扩展运行时、Skills、MCP、Claude Marketplace 是不同层次的契约。

## 5. 后续事实核查的来源入口

以下 URL 来自用户材料或与其作者来源有关的核查候选；列入本节不代表已经确认它们目前的归属、内容或可访问性。

- OMP 官网：https://omp.sh/
- OMP 图中源码：https://github.com/can1357/oh-my-pi
- Pi 官网：https://pi.dev/
- Pi 图中源码：https://github.com/earendil-works/pi
- Pi 作者历史仓库候选：https://github.com/badlogic/pi-mono

本轮 [willdeep 能力差距与补齐建议](2026-11-17-willdeep-cli-gap-analysis.md)使用本仓源码、注册路径和既有测试作为实现证据；官方原文的访问结果和材料差异见下节。对未安装、未执行、未获得匹配快照的项目保留“未验证”。

## 6. 官方来源核查与材料修正

核查日期为 2026-11-17。以下属于官方文档与源码说明核查，**没有安装运行 Pi/OMP，没有复现性能、可靠性、费用或任务成功率**。固定提交与会变动的官网页面分开引用；第 2、3 节仍保留用户材料原貌。

### 6.1 固定上游快照

| 项目 | 本轮查询 main 得到的提交 | 提交时间（UTC） | 说明 |
|---|---|---|---|
| Pi | `a9878e136f5576c2ffd00c6ff054c9cd07a4b277` | 2026-11-16 15:36:08 | [提交 API](https://api.github.com/repos/earendil-works/pi/commits/main)；下文文件链接固定到此 SHA |
| OMP | `6f03614d761366972e80e84e25d1418ed678d2c7` | 2026-11-16 15:18:25 | [提交 API](https://api.github.com/repos/can1357/oh-my-pi/commits/main)；下文文件链接固定到此 SHA |

提交时间不是目录统计时间、官网更新时间或截图生成时间。Pi 历史地址 `badlogic/pi-mono` 本轮返回 HTTP 301，目标为 `earendil-works/pi`；这只确认当前重定向关系，不推断迁移日期、原因或管理权变化。

### 6.2 已确认与应收窄的表述

| 主题 | 本轮核查结果 | 对材料的处理 |
|---|---|---|
| Pi MCP | 固定文档确认 stdio、Streamable HTTP，明确不支持 legacy SSE；项目级 `.pi/mcp.json` 受 project trust 控制 | 当前 Pi 已内置 MCP；旧“拒绝 MCP”印象失效。见 [P1] |
| Pi Codemode | 默认 MCP 工具暴露方式为 codemode；QuickJS 脚本可组合和并行调用工具，自身没有直接 Node API、文件系统或网络权限；已执行工具的副作用不随脚本失败回滚 | 是受限脚本编排入口，不能把脚本沙箱当成所有工具的 OS 隔离。见 [P1][P2] |
| Pi 子 Agent 与权限 | README 将子 Agent、计划模式留给扩展；默认没有逐调用审批，但有 project trust（默认 ask）及隔离运行说明 | 图中“多 Agent 否”只能解释为原生默认入口；“权限否”不能解释为完全没有信任机制。见 [P3][P4] |
| Pi 包目录 | 官网本轮显示 `1–50 / 5374`；包可分发扩展、Skills、模板、主题，来源有 npm/Git/本地 | 官方目录收录量不等于官方维护包数。没有 11 月 16 日 `5366` 的历史快照，不能用当前数替代或反驳历史数。见 [P5][P6] |
| Pi Provider 数 | 官网本轮写 `15+ providers, hundreds of models` | 与图中 `20+` 没有匹配统计口径和历史快照，保持未核实，不自行改成确定排名。见 [P6] |
| OMP 工具与规模 | 固定 README 自述 `31 built-in tools`、`60+ providers`、14 个 LSP 操作、28 个 DAP 操作、约 8 万行 Rust、约 1000 模型 | 找到官方声明；本轮没有独立清点、运行或逐 Provider 认证。部分工具默认关闭或需额外依赖。见 [O1] |
| OMP 代码操作 | 官方列出 LSP、`ast_grep`、`ast_edit`、DAP；Hashline 使用内容哈希锚点并检查陈旧锚点 | 可以确认产品机制，不能推导永不误改，也未复现材料中的速度、成功率或 Token 收益。见 [O1][O2] |
| OMP 子任务隔离 | task 支持隔离选项，但默认 `none`，worktree 依赖 Git；审批模式与工具能力策略另有配置 | “支持隔离”不等于所有任务默认隔离，worktree 也不是 OS 安全沙箱。见 [O3] |
| OMP 浏览器与桌面 | 浏览器可通过 Chromium/Puppeteer、CDP、relay 等连接；Computer Use 是独立主机桌面能力，默认关闭，需 helper 和系统授权 | 两者单独计数；浏览器开关启用不表示运行依赖、连接或权限已经满足。见 [O1][O2] |
| OMP 搜索与语音 | 搜索需要可用搜索 Provider；TTS 明确存在且默认关闭，支持语音合成输出 | 搜索不是无配置保证；已确认 TTS，不把它扩写为完整语音输入/转写/双向 Voice Mode。见 [O1][O2] |
| OMP 模型角色 | 固定 README 列 `default/smol/slow/plan/commit/vision/task/advisor/tiny` 九种；官网 agents-and-roles 页面另含 designer | 这是模型路由用途，不是九类子 Agent。官网与固定 README 存在口径漂移，不能给出无条件固定数量。见 [O1][O4] |
| OMP 后台压缩 | `compaction.asyncEnabled` 文档默认 true：预先生成候选，达到阈值再提交；历史前缀或分支变化会使候选失效 | 与前台压缩、已完成摘要缓存分开比较。见 [O5] |
| OMP Snapcompact | 在本地把整理后的历史渲染为图片供视觉模型使用，可能规范化或截断；是多种压缩方法之一 | 不称无损压缩，也不称视觉推理免费；本轮未验证实际收益。见 [O5][O6] |
| OMP 长期记忆 | 官方明确默认关闭，`memory.backend` 默认 off；可选 local/hindsight/mnemopi/sharpshooter；autolearn 另有开关，Hindsight 依赖服务 | “支持持久记忆”成立，“默认就会跨会话自动学习”不成立。见 [O7] |
| 消费订阅 | OMP 文档列 Claude Pro/Max OAuth、ChatGPT Plus/Pro（openai-codex）、GitHub Copilot、SuperGrok 或 X Premium+ 等入口 | 图中的 Claude Code 图标不等于集成该应用。这里只核对部分订阅入口，图中全部方案、两项目登录和续期流程未实测。见 [O8] |

Pi 的本地 `grep/find/ls`、MCP `tool_search` 不是原生网页搜索；本轮没有发现可直接确认的原生语音入口，但不据此否定扩展能力。图片的首发日期、完整订阅名单、Zed/ACP 的运行效果、所有平台权限和费用仍未完成逐项动态核验。

### 6.3 官方来源索引

- **[P1] Pi MCP（固定）**：[mcp.md](https://github.com/earendil-works/pi/blob/a9878e136f5576c2ffd00c6ff054c9cd07a4b277/packages/coding-agent/docs/mcp.md)。路线变更背景另见 [You said no MCP（2026-09-29）](https://earendil.com/posts/you-said-no-mcp/)。
- **[P2] Pi Codemode（固定）**：[codemode.md](https://github.com/earendil-works/pi/blob/a9878e136f5576c2ffd00c6ff054c9cd07a4b277/packages/coding-agent/docs/codemode.md)。
- **[P3] Pi 产品边界（固定）**：[README](https://github.com/earendil-works/pi/blob/a9878e136f5576c2ffd00c6ff054c9cd07a4b277/packages/coding-agent/README.md)。
- **[P4] Pi 信任与隔离（固定）**：[security.md](https://github.com/earendil-works/pi/blob/a9878e136f5576c2ffd00c6ff054c9cd07a4b277/packages/coding-agent/docs/security.md)。
- **[P5] Pi 包分发（固定）**：[packages.md](https://github.com/earendil-works/pi/blob/a9878e136f5576c2ffd00c6ff054c9cd07a4b277/packages/coding-agent/docs/packages.md)。
- **[P6] Pi 官网及目录（可变）**：[pi.dev](https://pi.dev/)、[packages](https://pi.dev/packages)。
- **[O1] OMP 能力总览（固定）**：[README](https://github.com/can1357/oh-my-pi/blob/6f03614d761366972e80e84e25d1418ed678d2c7/README.md)。
- **[O2] OMP 工具与默认开关（可变）**：[tools](https://omp.sh/docs/tools)。
- **[O3] OMP task（固定）**：[task.md](https://github.com/can1357/oh-my-pi/blob/6f03614d761366972e80e84e25d1418ed678d2c7/docs/tools/task.md)。
- **[O4] OMP 角色（可变）**：[roles](https://omp.sh/docs/roles)、[agents-and-roles](https://omp.sh/docs/agents-and-roles)。
- **[O5] OMP 压缩（固定）**：[compaction.md](https://github.com/can1357/oh-my-pi/blob/6f03614d761366972e80e84e25d1418ed678d2c7/docs/compaction.md)。
- **[O6] OMP Snapcompact（固定）**：[README](https://github.com/can1357/oh-my-pi/blob/6f03614d761366972e80e84e25d1418ed678d2c7/packages/snapcompact/README.md)。
- **[O7] OMP 记忆（固定）**：[memory.md](https://github.com/can1357/oh-my-pi/blob/6f03614d761366972e80e84e25d1418ed678d2c7/docs/memory.md)。
- **[O8] OMP Provider 与订阅（可变）**：[providers](https://omp.sh/docs/providers)。

比较结论应按“默认内置、可配置启用、扩展接入、未核实”分别记录。上述资料足以建立能力问题清单，仍不足以得出同模型同任务的质量胜负。
