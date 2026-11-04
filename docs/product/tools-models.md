# 工具与模型产品模块

> 范围：Provider、模型与档位、配置、上下文压缩、推理回传、内置工具、Skills、规则和 MCP 客户端。开发版本见[产品概览](../../PRODUCT_OVERVIEW.md)。

## 能力概要与用户入口

会话通过用户配置的 API Base、凭据与模型 ID 运行多轮 Tool Call Harness。支持 some.im 与 BYOK，以及 Chat Completions、Responses、Anthropic Messages 三种协议。TOML Provider Profile、`willdeep config`、TUI `/model`/`/routing` 与 Web 内置配置工作台提供配置入口；具体字段、默认值与凭据解析归[配置指南](../CONFIGURATION.md)。

## 已实现范围

### Provider、模型与预算

- 前台、Runtime 与 Worker 共用 Profile 解析，支持标准地址环境变量与 `api_base_env`，配置使用安全凭据引用；Session 保留所选 Provider/模型与配置。只改 Worker 模型时继承父连接，显式绑定 Provider 时保留自身档案，运行中的切换边界见[Worker 专题](../SUBAGENTS.md)。
- Provider 模型目录可搜索、刷新或手动输入。some.im/tokenhub 模型能力表参与视觉判定、上下文缺省及协议回落，取不到声明则沿用原规则；`qwen3-vl-plus` 为 some.im 纯文本模型提供图片描述降级。
- 本机发现仅探测已运行的 Ollama/Magpie 回环服务，尊重已保存的本地端口和地址变量；绕过代理、不带云端凭据、不启动服务或下载模型、不覆盖默认 Provider。`WILLDEEP_LOCAL_DISCOVERY=0` 可关闭。Magpie Profile 独立于原供应商，本地网关后的远程推理不计作本地模型。
- Root 与 Worker 配置、上下文、最大轮次、自动派工和专家预算经 `/routing` 与 Web 共用持久设置，保存检测版本冲突。职责与模型档正交，五职责 × 三档；S/M/L 部署基线与 `standard/advanced/expert` Worker 档也分别建模，旧 `deep` 名字不再代表独立公开职责。约束与映射见[模型档位](../MODEL_TIERS.md)。
- 本机智能体窗口/最大轮次支持 Web 模型绑定和 `agents bind`，明确预算在档位选择后应用，本机安装设置不进入可分享定义包；Worker 最大轮次与主会话统一允许至 1000，保存前校验。
- 完整子会话可在父 Provider 配置内覆盖模型；创建前校验可用目录、工具能力、图片兼容性与真实上下文窗口，窗口快照写入子会话，不继承父模型数值。worktree 子会话可派 Worker，父子共享家族并发/队列；用量账本携带家族根并按记录 ID 去重，主回合、Worker、压缩和辅助调用不因新 Session 重置或双计，未知/不完整计量仍 fail closed。
- `config init/check/show` 安全创建、严格校验与脱敏展示，旧字段以 legacy 提示；无配置时可由已存在环境凭据推断 Provider/缺省模型，首启无需重复引导。保存能解析不表示远端认证或账户余额已经验证。

### 上下文、压缩与推理

- 状态提供上下文用量、Token、缓存命中与耗时。自动摘要与 `/compress` 手动压缩保存会话及压缩检查点；压缩容量校验通过才报告成功，原始用户约束、近期工具配对和未知效果批次需保留。
- `[agent] compressor_profile` 显式指定摘要器的整套 Provider 档案，主模型仍用会话档案。`compressor_context_window` 封顶分块原料，不写先取压缩档案窗口，再取会话窗口；不存在档案/非法窗口启动即报错，不静默忽略。摘要器输出上限与输入窗口是不同配置问题。
- `[local_model]` 仅为标题和低置信度路由等短辅助任务复用模型，各职能可独立开关、失败回退；**当前压缩链不读取此段**，废弃的 `prefer_for_context_summaries` 被忽略，本机压缩必须显式选择压缩档案。旧概览“local_model 兼做压缩”的说法仅作历史保留。
- 压缩的已报告模型用量持久保存并进入预算、结果和检查点；失败消耗、空摘要与备用调用都计入，恢复不重复累计，达到预算阻止后续模型请求。未知/缺失结果不被压缩成成功，副作用约束见[安全模块](security-verification.md)。
- Responses thinking 保存并回传原始 `reasoning_text`，覆盖流式、非流式、中断和工具续轮；summary 不替代原始推理。压缩后保留必要空标记，不伪造原思维链；未出现推理的会话不发送此项，旧会话丢失的推理不凭空恢复。
- 相同宿主提示保留最后一次，用户输入不去重；思考计入上下文。系统提示词将契约、全局规则和技能目录与动态工作区/预算分界，支持稳定前缀缓存。
- 网络在成功响应头前最多重试五次，沿用总期限、取消和预算；已过期请求不再发送。响应正文/流中断不自动重放，超时收尾有界，资源释放失败明确报错。
- 轮次后下一句预测是标题摘要档位的独立小请求，仅取最近两条用户原话和助手尾部，不持久化、不进入 Runtime 协议；`input_suggestions=false` 关闭。显示与采用不自动提交，见[终端](cli-tui.md)和[Web](web.md)。

### 工具、Skills、规则与 MCP

- 文件搜索、读取、创建、精确编辑；`rg` 优先且有跨平台扫描兜底。Git 状态、Diff、受限历史、逐行归因、Worktree 与 Shell 执行受工作区及审批保护，命令不能借参数或替换扩权。
- 网页搜索和正文读取受审批；Fetch 对每次跳转重新校验公网目标，同域跟随、跨域重新审批，限制环路、次数、超时及流式 3MiB 响应。网络边界归[安全模块](security-verification.md)。
- Codex 兼容 Skills 发现、目录与按需正文读取，`$skill-name` 显式触发，额外技能根可配置；配置工作台可管理技能并从 GitHub 安装。工具 Schema 不全量常驻每轮上下文。
- 全局及项目/目录 `AGENTS.md`、`CLAUDE.md` 加载，执行前刷新；Shell/MCP/派工的目录规则发现不受搜索忽略影响，读取失败或超长明确报错。`[rules].files` 可启用、排序多文件/目录，缺省沿用全局规则；首次规则目录不存在时写入内置精简规则，已存在则不覆盖。
- 产品概览是参考资料，适用项目指令仍来自规则文件。根概览不超过 20,000 Unicode 字符才直接注入；超限只注入固定超限提示，不会自动读取摘要、索引或子页。详见[实际加载实现](../../crates/willdeep-core/src/project_rules.rs)。
- MCP 客户端发现、注册与调用支持 stdio、Streamable HTTP、静态 Bearer 与 OAuth 2.1（`willdeep mcp login`）；`list_mcp_tools` 按需检索，`call_mcp_tool` 精确调用。外部副作用仍需审批；插件 MCP 与对外只读 MCP 服务归[集成模块](integrations.md)。

## 当前限制与提案

- 共享 `model-catalog.v1` 已有严格 Rust 解析/校验及跨端设计，通过 `credential_ref` 分开目录与凭据；**尚未替换现有 Provider 路由和凭据来源**，不能当成已切换的运行配置体系。
- 压缩仍沿用 Provider `max_output_tokens`，没有独立输出上限；思考占同一额度，过小会截断摘要并导致无正文错误。缺省 16384 并非模型真实上限，已有显式档案不会随缺省修复，见[输出上限说明](../CONFIGURATION.md)。原问题、示例和报错完整留在历史归档。
- MCP 服务端主动请求与 GET 事件流未接；LSP 诊断、自动记忆及流式真实 reasoning 摘要仍是待办。运行阶段显示不能冒充模型思考内容。
- 旧“Provider 原生 token streaming 待做”与已有流式实现/验收冲突，仅保留历史状态，不再用作当前整体未实现结论。Hooks、插件与平台沙箱已有单独能力，不继续合并写成缺口，见[集成](integrations.md)和[安全](security-verification.md)。

## 详细指南与证据

[配置指南](../CONFIGURATION.md)、[Provider 协议](../PROVIDER_PROTOCOLS.md)、[some.im 集成](../SOMEIM_INTEGRATION.md)、[Magpie](../MAGPIE.md)、[模型档位](../MODEL_TIERS.md)、[共享模型目录](../SHARED_MODEL_CATALOG.md)、[Skills 与 MCP](../SKILLS_AND_MCP.md)是对应契约入口。压缩与流式的既有验证见[可靠性评测](../AGENT_RELIABILITY_EVALUATION.md)和[流式验收](../verification/0.114.0-rc2/README.md)，原始逐版记录见[历史快照](history/overview-through-0.142.0-rc1.md)。
