# willdeep-cli 对照 Pi / OMP：能力差距与补齐建议

> 日期：2026-11-17。性质：产品与技术调研、待决策建议，不是已批准的开发排期。
> willdeep 基线：本工作区 `HEAD ba894a59` 加核查时已有的未提交改动，`Cargo.toml` 为 `0.149.0-rc10`；不代表该版本已经合并、安装或发布。
> 材料记录：[Pi / OMP 文章及两张图片、官方来源核查](2026-11-17-pi-omp-materials.md)。历史分析：[2026-09-20 竞争力快照](../COMPETITIVE_LANDSCAPE.md)。
> 核查方式：本仓文档、工具注册与执行路径、已有测试，以及上游官方资料。本轮不安装竞品、不执行真实模型任务、不重跑运行时测试；建议和验收条件均不表示对应功能已经实现。
> 后续实施：用户已批准补齐 P0，协议和实现边界另见[P0 编码工具决策](../decisions/2026-11-17-p0-coding-tools.md)，实际命令、结果与未验证项见[P0 验收](../verification/0.150.0-rc2/p0-coding-tools.md)。2026-11-17 进一步授权修复三处 Clippy 阻塞并推进其余 P0/P1；各项按独立验收范围落地。本页仍保留调研时的基线，不将后续实现倒写为当时已经具备。

## 1. 结论与产品路线

willdeep 已具备大量 OMP 路线中的基础集成：原生 Worker、完整子会话、工作流、隔离工作树、模型档位、MCP、审批与平台写入围栏、上下文压缩、验证、审计、常驻 Runtime 及多端接入。这些不应列为从零补齐项；本轮也没有证据支持宣称已经全面优于 Pi 或 OMP。

应继续围绕 **常驻 Runtime、多端同会话、主权与本地模型、可审计执行** 补齐日常编程闭环。借鉴 OMP 的原生集成，同时让能力按需启用，不为凑工具数量增加维护面。

最应先解决的是：**安全修改文件、理解代码语义、验证真实交付**。其次补普通 BYOK 的搜索体验，以及主权策略和目标验收证据的执行约束。DAP、记忆、ACP 和后台预压缩按场景分期；桌面控制、语音和图片压缩暂后。

特别需要纠正两个乐观判断：现有 `edit_file` 的唯一匹配和原子写入不等于读时版本校验；已有 verifier 不等于每一条目标验收都已绑定可验证的执行证据。

## 2. 口径与适用范围

- **已实现**：发现配置或注册、执行链与既有测试/契约证据；本轮未执行这些测试。
- **部分**：已有相关机制，但缺少对照材料所指的完整操作或用户体验。
- **可外接**：可以通过 MCP、插件或 Shell 集成；不计作开箱可用的原生能力。
- **未发现原生实现**：在核查的注册、执行路径和对应模块中未找到；不排除用户自行安装外部工具。
- **未核实**：缺少匹配版本、原始来源或动态验证；不按“没有”或“支持”处理。

比较范围包括 CLI/TUI 实际使用的 Core、Daemon、内嵌 Web 和公开 Runtime 客户端。macOS Xedit 是独立宿主，其能力不能直接计入 Rust CLI；网页或桌面辅助程序也不能成为第二套执行状态来源。

Pi 现已内置 MCP/Codemode；其扩展子 Agent 不能按截图一律记为“不支持”。OMP 的工具、Provider、角色数量按官方声明记录，未独立清点；其 TTS、worktree 和审批也不能分别扩写为完整 Voice Mode、OS 隔离和全平台沙箱。具体修正见[资料核查](2026-11-17-pi-omp-materials.md#6-官方来源核查与材料修正)。

## 3. 与材料对应的能力概览

| 维度 | willdeep 当前判断 | 差距或应保留的边界 |
|---|---|---|
| 架构与入口 | CLI、TUI、Web 共用常驻 Runtime；Rust SDK/控制协议 | SDK 不等于 ACP；无需改变既有产品定位 |
| 多 Agent | 原生五职责 Worker、完整子会话、声明式工作流、worktree | 不需要重建 task；保持用户接管、停止、订阅与家族配额统一 |
| 文本与多文件编辑 | 搜索、读取、创建、旧文本匹配、默认唯一性检查、原子写入 | 未绑定读时文件版本；逐文件修改不等于统一多文件提交或语义重构 |
| LSP / AST / DAP | 在核查的原生工具注册和执行链中未发现完整实现 | Shell/MCP 可外接；仍缺正式产品入口、依赖管理与统一证据 |
| Git | 状态、Diff、历史、blame、worktree、检查点回退及审计 | 图中 deep/basic 无统一标准，不据此给自身打分 |
| 测试与验证 | Shell、后台任务、宿主 verifier、验证快照与验收门 | Goal criterion 的 evidence 仍是自由文本，未强制引用真实执行记录 |
| MCP | stdio、Streamable HTTP、OAuth 客户端；对外只读 MCP 面 | 部分插件反向宿主请求已存在；不能据此称完整支持所有标准服务端主动请求 |
| 审批与沙箱 | 四档审批、只读硬上限、macOS/Linux 写入围栏及网络通断 | worktree 不等于沙箱；现有围栏不是宿主读取白名单，也无域名级网络策略和 Windows 等价后端 |
| Hooks / Skills | pre_tool/post_tool 接线、规则加载、Skills 发现与安装 | approval_resolved 已定义但未接线；SKILL.md 兼容不代表 Pi TS 扩展兼容 |
| 插件与 Codemode | 自有插件包、宿主桥、MCP 网关、声明式工作流 | 包要求 `.codex-plugin/plugin.json`；未发现 Pi TS/Claude Marketplace 通用运行兼容；工作流不执行任意 JS |
| 网页搜索 / 正文读取 | 原生 web_fetch；web_search 通过 some.im 接通 | 普通 BYOK 缺独立搜索适配；HTTP 抓取不等于浏览器交互 |
| 浏览器 / Computer Use | 仍属后续范围，可通过外部工具集成 | 先补正式网页验收，再独立评估桌面执行面；Web 聊天页不算 Agent 浏览器 |
| Provider 与本地模型 | 多 Profile、三种线协议、本机 Ollama/Magpie 发现 | 协议数不等于 Provider 数；发现不负责启动服务或下载模型 |
| 模型角色与档位 | 五职责 × standard/advanced/expert 三档；用户可明确绑定 | 保持正交设计；未绑定更强模型的升档不能宣传为实际换了更强模型 |
| 模型切换与回退 | 会话内切换、传输层有界重试；压缩/辅助用途有候选链 | 未发现主模型通用备用链和多凭据冷却轮换；不可将不同机制混称自动容灾 |
| 主权策略 | 有 Profile 和共享模型目录策略结构 | 目录尚未接管运行路由；未形成覆盖全部模型调用的项目级出网硬限制 |
| 上下文 | 手动/自动增量压缩、摘要指纹缓存、检查点与用量预算 | 主请求前仍等待压缩；缺后台候选接管及摘要用途独立输出额度 |
| 会话分支 | 整体 fork、按已完成轮次 fork、rewind、来源血缘 | 不是任意消息节点的持久树；手动压缩代际限制精确历史 fork |
| 长期记忆 | 规则、持久会话、摘要、历史搜索、同父会话黑板 | 不等于跨独立任务的自动提取、相关性检索、注入和维护闭环 |
| 图像与多模态 | 文本/图片附件、三种协议图片编码、some.im 视觉描述降级 | 不能据此承诺音频/视频/PDF 原生输入或多模态输出 |
| 语音 / IDE | 尚无已确认的原生通用语音链路；ACP 仍待办 | 系统听写不算 Voice Mode；Runtime SDK 不直接等于 ACP 已支持 |
| 订阅 / 费用 | some.im 账户及 BYOK；Apache-2.0；有用量和时间预算 | 第三方消费订阅登录另核；Token 上限不是金额封顶，外部推理独立计费 |

## 4. 不应重复建设的能力

- **协作骨架**：复用完整子会话、专业 Worker、现有工作流和家族配额。新工具先确定执行身份，避免再建一套 task 和任务账本。
- **模型分工**：沿用职责 × 档位与用户绑定。OMP 路由角色可以启发场景划分，不能替代本项目的主权、连接、预算和权限契约。
- **安全与审计**：新工具接入 Runtime 的审批、规则、用量、事件、取消与审计。现有围栏的覆盖范围应如实说明，不把“有沙箱”当作所有执行面均隔离。
- **编辑机制**：继续复用路径约束、匹配器和原子写入；新增版本前置条件及变更协议。是否使用 Hashline 名称不是验收标准。
- **生态入口**：先完善现有 Skills、MCP、插件的发现、安装、配置、状态与示例；外部包兼容需要逐层契约验证。
- **评测机制**：扩展既有任务集、独立 verifier 和 JSON/Markdown 报告，不重建另一套质量看板。

## 5. 建议的补齐顺序

优先级表示建议顺序，不是工期估算或已经批准的承诺。每项形成独立可验收交付，测试场景与实现一起推进。

| 优先级 | 项目 | 最小交付范围 | 为什么做 |
|---|---|---|---|
| P0 | 编辑版本校验与多文件变更协议 | read 返回可引用版本；edit 接受并校验前置版本；统一变更预检、路径/写集/版本校验、差异展示、部分失败与恢复结果 | 当前没有读时版本绑定；原子写入只防半写，不能防陈旧覆盖或丢更新 |
| P0 | 核心场景回归基线 | 在既有评测中补陈旧编辑、跨文件引用、网页交互、压缩后约束、多任务恢复与错误完成案例 | 给后续工具增益提供基线，不拿工具数量当质量 |
| P0 | LSP 查询型语义能力 | 先覆盖 Rust/TypeScript 的 diagnostics、definition、references、symbols；按工作区管理语言服务 | 减少文本猜测，是语义修改与 IDE 的共用基础；查询接口不意味着服务进程天然只读 |
| P1 | 语义重命名与 AST 查询 | LSP WorkspaceEdit 转统一变更协议，预览、版本检查、审批、应用和恢复；AST 先做结构化搜索 | 依赖前面的编辑协议；不让语言服务器绕过写入边界 |
| P1 | 交互式浏览器验收 | 先以受控 MCP/适配器提供导航、快照、点击、输入、截图、断言与产物；有正式配置和状态入口 | 补上前端交互验收断点，不必首轮自行内嵌浏览器引擎 |
| P1 | 普通 BYOK 的搜索适配 | 搜索后端独立于聊天 Provider，可配置服务或正式 MCP 集成；返回来源、错误和用量边界 | 现有 web_search 只接 some.im，普通 BYOK 开箱体验不完整 |
| P1 | 项目级模型调用策略 | 将允许 Provider/模型/网络范围接到根会话、Worker、压缩、视觉、标题和回退的真实调用前 | 主权是既有产品定位；文字规则和目录 Schema 尚不能形成硬约束 |
| P1 | 验收证据与审计接线 | Goal criterion 引用真实 Tool/Job/verification、归属、状态和快照；人工验收单列；补 approval_resolved 投递 | 避免非空 evidence 被误当作已核实证据；补齐已有能力的执行链 |
| P1（随集成交付） | 工具默认体验 | 统一必需依赖、启用状态、配置检查、权限原因与失败恢复；先覆盖 LSP/浏览器/搜索及已有 MCP | 后端存在不等于用户能顺利配置和使用 |
| P2 | DAP 调试闭环 | 先一种语言：启动/附加、断点、栈、变量、继续/停止；求值单独审批 | 对复杂故障有用，但环境和执行成本高于语义查询 |
| P2 | 可选项目记忆 | 来源、项目隔离、人工确认写入、预算内检索注入、纠错/过期/删除；先不做无提示自动学习 | 现有规则、摘要和黑板已解决部分需求，先验证跨会话复用收益 |
| P2 | ACP / IDE 适配 | 复用 Runtime 会话、流事件、取消、审批与工作区；先验 Zed 一条链路 | 扩展入口，不另起 Agent Loop 和权限系统 |
| P2 | 后台预压缩 | 不可变前缀候选、采用前指纹校验、取消/去重/过期丢弃、同家族预算；摘要独立输出额度 | 已有缓存解决重复摘要，下一步才是降低新摘要阻塞 |
| 条件触发 | 主模型备用链、凭据轮换、第三方订阅 | 按真实需求与可用接口逐项接入；限制允许链、冷却、历史兼容、计费和安全重放 | 不能用传输重试代替容灾，也不宜先造凭据池再找需求 |
| 条件触发 | 金额预算与更强隔离 | 若提供费用封顶，补价格快照、调用前额度预留、未知用量处理；若要不可信代码隔离，另定义读取/网络/平台契约 | Token 预算和写入围栏不能承诺它们当前没有的保证 |
| 暂后 | 全桌面、完整语音、Snapcompact、任意消息树、全面 Pi 包兼容 | 独立试点并评估收益、依赖、成本与维护面 | 本轮没有实测证据证明优先于编程核心闭环 |

### 5.1 编辑版本与多文件变更的验收

- `read_file` 的版本标识与读取内容绑定；编辑明确声明基于哪个版本，陈旧版本返回冲突并保留外部修改。
- 区分“整文件版本相同”与“目标片段仍能唯一定位”两种策略；允许局部锚点重定位时说明其保证，不能把任何文件变化都等同于必须拒绝。
- 宿主内并行写入有统一协调；进程间锁只约束合作方。提交前复查不能单独封闭任意外部写者的竞态窗口，方案必须说明保证边界，不能把原子 rename 当成通用 CAS。
- 多文件操作先统一预检路径、版本和权限，明确支持的创建/修改/删除/重命名种类；审批所见差异与实际应用内容绑定。
- 不承诺文件系统级跨文件原子事务。部分落盘、恢复失败及未确认结果必须可见，不能报告整体成功。
- 夹具注入陈旧读取、重复片段、并发变更、多文件中途失败和越界路径，独立检查最终内容、冲突及恢复报告。

### 5.2 LSP 与语义编辑的验收

- 语言服务绑定工作区，具备启动、取消、退出和资源管理；缺依赖时给出可操作原因。
- 查询返回文件、范围、严重级别和快照，限制结果体积；诊断可随着文档版本失效。
- 初版只开放查询接口，不开放 rename 或 `workspace/applyEdit`。服务进程仍可能运行构建脚本、过程宏或插件；选定服务器后明确执行开关及读写/网络需求，按实际能力接入 Runtime 审批和围栏，并验证所选配置。
- 后续 WorkspaceEdit 转统一变更协议；验证所有路径、版本和写权限，不让服务端直接写磁盘。
- 重命名夹具同时包含必须更新的字符串键引用（如属于目标符号的属性访问）与同内容的无关普通字符串，按语义验证引用集合、允许写集、编译与测试。

### 5.3 浏览器与搜索的验收

- 把“通过 MCP 可接”变成有依赖版本、配置检查、错误反馈和文档的正式集成，再决定是否原生内嵌。
- 浏览器使用隔离 profile，不读取用户日常浏览器凭据；下载和截图进入授权产物目录。
- 导航、重定向、下载、上传及网络可达范围有明确策略，本地开发服务器按授权开放；有副作用的提交复用审批。
- 用本地受控网页证明表单、路由、错误态、截图和断言真实执行；截图不能替代业务断言。CLI/TUI/Web 可取消并查看真实终态。
- 搜索提供商独立于聊天 Provider；覆盖未配置、认证失败、限流、空结果、来源返回和审批，不能把没有搜索后端的 BYOK 配置展示成可用。

### 5.4 主权、验收证据及其他边界

- 项目模型策略覆盖主模型及所有辅助调用，按每次真实目标连接校验；候选和回退也受同一规则约束，策略拒绝要可审计。
- Goal 的 `evidence_ref` 应关联真实执行、退出码、归属与代码快照；代码变更后证据能失效。人工观察可以记录，但应标明人工来源，不能伪装工具执行。
- 记忆区分规则、事实、偏好和实验推断，保存来源并允许纠错撤销；不把历史全文或协作黑板直接升格为可信全局指令。
- ACP 是客户端接入协议，LSP 是语言服务协议；Runtime SDK 也不能直接算 ACP 支持。
- 回退重新核对主权、能力、上下文、预算和计费；已开始生成或可能产生副作用时不能盲目重放。凭据轮换与消费订阅登录分别设计。
- 自动围栏缺少可用后端时与显式要求围栏的行为应准确显示；当前写入围栏不能称作敏感目录读取保护。

## 6. 如何评估是否值得继续

本仓已有[模型行为评测](../MODEL_EVAL.md)、[可靠性评测](../AGENT_RELIABILITY_EVALUATION.md)与[评测模块台账](../product/evaluation-rsi.md)。复用 `scripts/model_eval.rb`、`scripts/agent_reliability_eval.rb` 和外部 verifier，缺的是新增场景及对应数据，不能写成“现无质量评测”。

先补离线夹具、预期结果与失败边界，验证缺陷原样失败、正确实现通过，再验工具适配链。真实模型对照须单独冻结任务、模型与预算；历史实验额度不延续为本轮授权。

对照固定二进制/提交/脏工作区标记、模型与推理档位、Provider、工具/插件版本、任务夹具、超时、调用数、计费预算、重试口径和验收器。能力不兼容的模型不强行合并平均值。

| 场景 | 要回答的问题 | 独立验收 |
|---|---|---|
| 陈旧编辑与并发 | 是否拒绝陈旧变更、保留外部修改 | 注入修改时点、最终内容、冲突和恢复结果 |
| 跨文件符号定位 | LSP 是否降低误命中和遗漏 | 固定目标与引用集合，包含同名无关符号 |
| 语义重命名 | 是否完整更新目标，保留无关内容 | 字符串键引用与无关字符串、允许写集、编译和测试 |
| 前端交互 | 是否发现并修复真实流程故障 | 独立浏览器操作、状态断言和产物 |
| BYOK 搜索 | 是否真正脱离 some.im 可配置使用 | 独立后端夹具、真实请求、结果来源和失败分支 |
| 项目模型策略 | 是否覆盖辅助和备用调用 | 根/Worker/压缩/视觉/标题的允许与拒绝记录 |
| 目标验收 | 是否拒绝无记录、失败记录或过期记录作为完成证据 | evidence 引用校验、快照变更、人工来源区分 |
| 调试 | 是否通过运行期事实找到原因 | 固定故障、真实调试事件和回归测试 |
| 长上下文与恢复 | 约束是否在压缩、取消和重启后保留 | 原始约束、工具配对、无重复副作用及最终结果 |
| 多任务集成 | 是否等待、审阅并正确集成委派结果 | 子任务真实终态、最终写集和宿主 verifier |

报告同时记录成功率、误报完成、受保护文件变更、Token（含压缩/辅助/Worker）、账单成本、墙钟、工具失败和人工介入。缺失用量保留未知，失败和超时留在报告。数量与阈值在试验前登记，不预设未经验证的通用分数。Pi/OMP 对照须另行安装匹配版本并披露配置差异；本轮未执行。

## 7. 本轮边界与建议起点

本轮只落资料、差距分析和文档入口。没有实施新工具、修改功能代码或应用版本，没有把已有测试当成本轮通过结果，也没有提交、推送或发布。

建议下一份实施方案先围绕 **编辑版本校验与多文件变更协议** 定义契约、并发边界和失败夹具，再接 LSP 查询与 WorkspaceEdit。浏览器可先验证 MCP 能否提供正式产品体验。项目策略和验收证据作为现有 Runtime 的可靠性补齐单独推进；这些均为待决策建议。

### 7.1 后续实施状态（0.150.0-rc3）

本节记录调研后的已授权实施，不改变上文原始基线。P0 已实现版本化编辑、多文件创建/替换、查询型 LSP 和固定任务基线，详见[rc2 验收](../verification/0.150.0-rc2/p0-coding-tools.md)。真实 Rust 语言服务、Windows 原生运行与真实模型成绩仍未取得证据。

rc3 修复三处 CLI Clippy 阻塞，并补提问粘贴、隐藏权限选择器和后方审批的输入路由及多行答案绘制。P1 已推进「验收证据与审计接线」中的审批结果 Hook：复用真实审批器/判官决定、会话归属、脱敏和合法有界 JSON；不等同于 Goal criterion 已绑定执行证据。范围见[审批结果决策](../decisions/2026-11-17-approval-resolved-hooks.md)，实际命令与限制见[rc3 验收](../verification/0.150.0-rc3/clippy-approval-hooks.md)。

语义重命名/AST、正式浏览器集成、独立 BYOK 搜索、项目级全部模型调用策略、Goal 结构化执行证据仍未交付。各项继续按第 5 节的依赖与验收边界推进，不把本轮审批 Hook 修复计为整个 P1 完成。

### 7.2 后续 P1 实施（0.151.0-rc1）

在后续授权下，剩余 P1 已接入共享 Runtime，实际验证按本地回归、真实工具和未覆盖范围分层记录。语义重命名复用两阶段同步与一次性文件预览；固定 TypeScript 服务的原生无版本映射按请求前快照绑定，不冒充服务端版本。AST 提供结构查询而非改写；受控浏览器采用内置 Node/CDP 桥、独立 profile、具体动作审批、隔离 DOM 操作和宿主截图；搜索可独立配置 SearXNG，也保留 SomeIm 兼容。

项目模型策略约束已接入的实际 Provider HTTP 出口，持久项目归属覆盖恢复、worktree、Worker 与辅助调用，运行期允许集只可收窄；不能约束任意外部进程。Goal 已从文字说明推进到真实执行引用与快照校验，失败、取消、超时和代码变化使旧成功失效，重启缺记录需重验。配置状态和 doctor 共用离线投影。

本轮实现边界见[Runtime 工具指南](../P1_RUNTIME_TOOLS.md)，命令、首次失败、修复和最终覆盖见[P1 验收](../verification/0.151.0-rc1/p1-runtime-tools.md)。真实 AST CLI 缺依赖、真实 Rust 语言服务、Windows 原生与真实模型成绩仍没有本轮证据。浏览器的页面来源限制不构成全进程网络隔离，当前要求有效 OS 围栏 Off；非 Unix 暂拒截图落盘。DAP、Computer Use/AX、语音等原 P2 范围未纳入本轮，未提交、安装或发布。

## 8. 证据与核查记录

本节引用源码、已有契约和已有测试位置；**没有执行这些测试**。性能改善、远端认证和竞品胜负仍需动态证据。

### 8.1 基线与评测

| 核查项 | 直接证据 | 可得结论 |
|---|---|---|
| 版本与许可 | [Cargo.toml](../../Cargo.toml#L10) | 工作区开发版 0.149.0-rc10、Apache-2.0；不证明发行状态 |
| Runtime 边界 | [架构](../ARCHITECTURE.md#L11)、[产品概览](../../PRODUCT_OVERVIEW.md#L115) | 多客户端共享 Runtime；Browser/Computer Use 列为后续 |
| 工程评测入口 | [CI](../../.github/workflows/ci.yml#L57)、[verifier](../../scripts/lib/model_eval/verifier.rb#L88) | 有脚本测试、离线夹具检查及外部验收，未在本轮跑真实模型 |
| 统计边界 | [任务归档](../../bench/model-eval/README.md#L18)、[report](../../scripts/lib/model_eval/report.rb#L113)、[可靠性场景](../../scripts/agent_reliability_eval.rb#L195) | 可复用成功/失败/误报等指标；压缩/中断旧报告可能缺用量，总费用不能按零补齐 |

### 8.2 工具、编辑与集成

| 核查项 | 直接证据 | 实际边界 |
|---|---|---|
| 原生工具入口 | [definitions/dispatch](../../crates/willdeep-core/src/tools.rs#L742)、[宿主工具](../../crates/willdeep-core/src/tools/host_tools.rs) | 核查注册与分派，未发现完整原生 LSP/AST/DAP/browser/computer/语音入口；外接另计 |
| edit 参数与执行 | [schema](../../crates/willdeep-core/src/tools.rs#L938)、[edit_file](../../crates/willdeep-core/src/tools.rs#L2019)、[原子写入](../../crates/willdeep-core/src/tools.rs#L2352) | path/old_string/new_string/replace_all；无 expected_hash/revision 或强制先读；重新读文件后匹配并写入，仍有并发丢更新窗口 |
| 局部匹配 | [edit_match](../../crates/willdeep-core/src/tools/edit_match.rs#L7)、[测试](../../crates/willdeep-core/src/tools/tests.rs#L240) | 旧文本不存在或多命中默认拒绝，可处理唯一匹配的行号前缀；测试允许未先 read 即 edit |
| 不可混算的版本保护 | [SessionStore 基线测试](../../crates/willdeep-core/src/session/read_baseline_tests.rs#L7)、[完成合同](../../crates/willdeep-core/src/tools/patch_contract.rs#L53) | 前者保护会话保存，后者做完成检查；均非源码编辑的读时 CAS |
| 搜索 Provider 绑定 | [Harness](../../crates/willdeep-cli/src/harness.rs#L1032)、[web_search](../../crates/willdeep-core/src/tools.rs#L1523)、[web_fetch](../../crates/willdeep-core/src/tools.rs#L1562) | 仅 SomeIm 构造 WebToolConfig；search 缺配置报错，fetch 不以该配置为前提 |
| Skills 与 MCP | [Skills 发现](../../crates/willdeep-core/src/skills.rs#L65)、[MCP 注册表](../../crates/willdeep-core/src/mcp.rs#L1)、[HTTP](../../crates/willdeep-core/src/mcp/http.rs)、[OAuth](../../crates/willdeep-core/src/mcp/oauth.rs) | 发现 SKILL.md；MCP 有 stdio/HTTP/OAuth 执行链；部分插件可反向请求宿主，不代表全部标准主动请求均实现 |
| 插件格式与编排 | [load_package](../../crates/willdeep-core/src/plugin/package.rs#L303)、[workflow_control](../../crates/willdeep-core/src/tools/host_tools.rs#L245) | 要求 Codex 包元数据，Willdeep 宿主贡献可选；支持 agent/switch/foreach/repeat，不执行任意 JavaScript |
| 浏览器与 IDE 待办 | [工具对照](../XEDIT_TOOL_PARITY.md#L25)、[集成限制](../product/integrations.md#L56) | 本仓尚不能因 Xedit 路线、Web 页面或 Rust SDK 而算原生浏览器/ACP 已完成 |

### 8.3 协作、目标与安全

| 核查项 | 直接证据 | 实际边界 |
|---|---|---|
| 职责与档位 | [Worker 类型](../../crates/willdeep-core/src/subagent/types.rs#L69)、[档位](../../crates/willdeep-core/src/worker_tier.rs#L104) | 五职责与三档独立，不等于 OMP 九个路由角色 |
| 完整子会话与家族资源 | [创建](../../crates/willdeep-cli/src/daemon/child_sessions/mod.rs#L387)、[调度器](../../crates/willdeep-core/src/subagent/scheduler.rs#L9)、[文件租约](../../crates/willdeep-core/src/subagent/runner.rs#L429) | 已有 fresh/fork、只读/worktree、家族调度及 Worker 文件协调；租约不约束任意外部进程 |
| 目标预算 | [GoalBudget](../../crates/willdeep-core/src/goal.rs#L36) | wall-clock、续推次数、Token；没有金额上限字段 |
| 目标证据 | [GoalCriterion](../../crates/willdeep-core/src/goal.rs#L128)、[验证快照](../../crates/willdeep-core/src/tools/verification.rs#L388) | criterion 以 done 与非空 evidence 判断；可复用已有验证快照失效机制补结构化引用，不应否定现有 verifier |
| 审批与平台围栏 | [审批档位](../../crates/willdeep-core/src/tools/approval.rs#L79)、[网络策略](../../crates/willdeep-core/src/sandbox.rs#L74)、[Seatbelt](../../crates/willdeep-core/src/sandbox.rs#L166)、[bubblewrap](../../crates/willdeep-core/src/sandbox.rs#L229) | 网络只有 Allow/Deny；Seatbelt 以 allow default 限制写，Linux 根只读挂载仍可读；不是读取白名单或域名粒度隔离 |
| 围栏开关与覆盖 | [Harness 选择](../../crates/willdeep-cli/src/harness.rs#L2138)、[覆盖说明](../SANDBOX.md#L160) | 非只读默认可通网；自动模式缺后端可能无围栏，显式要求不可用则拒绝；Hook/MCP 等不能一概算受同一围栏约束 |
| Hook 接线 | [工具前后调用](../../crates/willdeep-core/src/tools.rs#L992)、[Hook 类型](../../crates/willdeep-core/src/hooks.rs#L48)、[已知限制](../HOOKS.md#L110) | pre_tool/post_tool 已接；approval_resolved 定义/配置存在但生产审批未投递；自配 Hook 使用宿主权限 |

### 8.4 模型、上下文与记忆

| 核查项 | 直接证据 | 实际边界 |
|---|---|---|
| Provider 与切换 | [协议/工厂](../../crates/willdeep-core/src/provider/mod.rs#L35)、[解析](../../crates/willdeep-cli/src/provider_settings.rs#L177)、[TUI 切换](../../crates/willdeep-cli/src/tui/model_provider.rs#L319)、[共享句柄](../../crates/willdeep-core/src/provider/main_model.rs#L40) | 三种协议与真实切换；不证明所有厂商及历史字段均无损兼容 |
| 重试不等于备用链 | [传输重试](../../crates/willdeep-core/src/provider/common.rs#L142)、[主模型错误路径](../../crates/willdeep-core/src/agent.rs#L1506)、[单凭据解析](../../crates/willdeep-cli/src/provider_settings.rs#L151) | 响应前重试有界；普通主模型错误返回；配置引用不等于凭据池 |
| 主权策略结构与落地 | [目录策略](../../crates/willdeep-runtime-protocol/src/model_catalog.rs#L268)、[当前限制](../product/tools-models.md#L43) | Schema 已有网络范围/候选模型，尚未接管实际路由和凭据来源 |
| 登录边界 | [some.im 配对](../../crates/willdeep-cli/src/onboarding.rs#L176)、[Profile](../../crates/willdeep-cli/src/config.rs#L831) | some.im 账户/API Key 与 MCP OAuth 分开；不能推出第三方消费订阅 OAuth 已实现 |
| 压缩与缓存 | [请求期压缩](../../crates/willdeep-core/src/agent/context.rs#L216)、[持久字段](../../crates/willdeep-core/src/context_summary.rs#L17)、[恢复校验](../../crates/willdeep-core/src/agent/context.rs#L842)、[主请求等待](../../crates/willdeep-core/src/agent.rs#L1348) | 有增量摘要和前缀/规则指纹复用，主请求仍等待；缓存不是后台预压缩或长期记忆 |
| 压缩额度 | [压缩 Profile](../../crates/willdeep-cli/src/harness.rs#L1844)、[输出限制](../product/tools-models.md#L44) | 可绑定独立摘要器；缺摘要用途独立 max_output_tokens |
| 会话分支 | [fork_through](../../crates/willdeep-cli/src/daemon/session_store.rs#L717)、[线性消息](../../crates/willdeep-core/src/session.rs#L119) | 完成轮次与有效代际限定精确 fork，不是任意消息树 |
| 图像 | [附件类型](../../crates/willdeep-core/src/types.rs#L51)、[视觉降级](../../crates/willdeep-cli/src/harness.rs#L1039) | 原生 Text/Image 和 some.im 图片描述降级；不证明音视频/PDF 原生链路 |
| 记忆各环节 | [规则加载](../../crates/willdeep-core/src/project_rules.rs#L32)、[黑板](../../crates/willdeep-core/src/board.rs#L124)、[会话搜索](../../crates/willdeep-cli/src/daemon/session_store.rs#L1092)、[注入](../../crates/willdeep-cli/src/harness.rs#L1374) | 显式规则、父会话笔记和历史搜索已存在；自动跨任务记忆闭环仍缺 |

### 8.5 核查覆盖与限制

五个调研主题中，代码工具、协作/安全、模型/上下文、上游官方来源完成只读核查；扩展集成分项因上下文过长失败，随后由主会话对工具注册、Skills、MCP、插件加载、搜索接线和集成台账做定向补查。没有把失败分项视为已完成的全面审计。

两张原图与资料转录经过独立复核；LSP 进程可能执行代码、语义重命名可能更新字符串键引用两项建议已修正。部分旧专题文档仍有默认网络策略、Hook 接线等历史描述，本报告按当前执行链收窄结论，不在本轮顺手改写全部历史文档。
