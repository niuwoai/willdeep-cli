# 智能体整体规划：三端现状证据

日期：2026-10-13（Asia/Shanghai）。状态：本地源码静态核对；没有构建、运行新测试、请求生产数据、修改服务或发布版本。

架构主文：[专业智能体定义、云端分发与 RSI 迭代](2026-10-13-agent-registry-rsi-architecture.md)。

## 1. 核对边界

| 仓库 | HEAD / 分支 | 本次读取时状态 |
|---|---|---|
| willdeep-rs | `bd436f0` / `codex/h2-upgrade-and-bench-archive` | 有其他工作产生的已修改与未跟踪文件；workspace 版本为 `0.98.0-rc4` |
| Xedit | `4b8bb644` / `main` | `git status --short` 没有输出；Xcode marketing version 为 `1.417.0-rc8` |
| muchtoken | `81ac1255` / `main` | 有本地配置/文档/脚本改动；服务源码版本常量为 `0.247.0-rc3` |

以上是工作区快照，不是线上版本或已安装版本。源码能证明机制和接口定义存在；不能由此证明生产数据、部署状态、付费评测成绩或端到端效果。

some.im 服务范围根据以下实现定位到 MuchToken：`go.mod` 的 module 为 `someapi`；`main.go` 挂载 `/api/v1` 账号 API 和技能路由；Rust/Xedit 登录代码使用 some.im 相同 client-login 路径。本轮没有把旧 `some-backend` Rails 项目或 `someai` 目录当作新资产服务的实施基线。

## 2. Rust：已有能力、限制与复用位置

| 已核实事实 | 源码锚点 | 对规划的影响 |
|---|---|---|
| 五个公开职责为 generalist/implementer/tester/reviewer/ops_runner | [types.rs](../../crates/willdeep-core/src/subagent/types.rs) 的 PUBLIC_SUBAGENT_TRADES | 专业定义与基础职责分开 |
| Profile 包含 Provider/model、工具、capability prompt、预算、写范围、工作树、Shell 策略 | [types.rs](../../crates/willdeep-core/src/subagent/types.rs) 的 SubagentProfile；[profiles.rs](../../crates/willdeep-core/src/subagent/profiles.rs) | 复用 Profile，新增定义加载投影 |
| 用户配置主要是 model/provider_profile、预算与 worktree；组装从 builtin_profiles 开始 | [config.rs](../../crates/willdeep-cli/src/config.rs) 的 SubagentProfileSettings；[worker_profiles.rs](../../crates/willdeep-cli/src/harness/worker_profiles.rs) | 还不能等同于任意用户定义注册表 |
| task.skill 为单个可选技能；简报读取 SkillCatalog 并注入正文，缺失会写 unavailable 标记 | [types.rs](../../crates/willdeep-core/src/subagent/types.rs)、[brief.rs](../../crates/willdeep-core/src/subagent/brief.rs) | 定义依赖需要多技能、顺序、hash、必需性 |
| SkillCatalog 扫描项目/用户目录，按 identifier/name 读取；allow_only 空列表保留原集合 | [skills.rs](../../crates/willdeep-core/src/skills.rs) | 必需依赖与显式零技能语义需要新契约，不能复用含糊空列表 |
| 子 Agent 工具过滤 parent-only 工具；无已批准写目标会剔除写工具 | [runner.rs](../../crates/willdeep-core/src/subagent/runner.rs) | 受控权限边界继续由执行器强制 |
| 子 Agent 使用 SessionStore/checkpoint 保留实例历史和工作区归属 | [checkpoint.rs](../../crates/willdeep-core/src/subagent/checkpoint.rs) | checkpoint 可复用，但不是跨实例长期记忆 |
| 当前后台并发常量仍为 5；队列/默认 8/Agent-Run 分离方案是待实施规划 | [catalog.rs](../../crates/willdeep-core/src/subagent/catalog.rs)、[调度规划](2026-10-12-single-level-multi-agent.md) | 资产架构不冒充调度已交付，不新增平行调度器 |
| TaskPacket 与 compose_brief 主要是文本路径；相关文件用 read_to_string 注入 | [types.rs](../../crates/willdeep-core/src/subagent/types.rs)、[brief.rs](../../crates/willdeep-core/src/subagent/brief.rs) | 未核实完整的 Worker 图片输入传递，需要专项补齐 |
| 网关能力解析包含协议、image_input 和 verified | [capabilities.rs](../../crates/willdeep-core/src/provider/capabilities.rs) | 可复用模态/协议判断，能力声明仍需实际输入验证 |
| 登录成功主要返回 Provider 元组、API Key 与模型 | [onboarding.rs](../../crates/willdeep-cli/src/onboarding.rs) 的 some_im_login | 新增账号资产服务会话，不把 Provider 名称当登录身份 |

## 3. Xedit：已有用户定义能力比 Rust 更完整

| 已核实事实 | 源码锚点 | 对规划的影响 |
|---|---|---|
| AgentProfile 统一子代理/dispatch，分为 soul/runtime/capability | [AgentProfile.swift](../../../Xedit/Xedit/AgentProfile.swift) | 在已有模型上扩展来源、revision 与能力契约 |
| soul 包含 purpose、代码-owned basePromptSection、用户 promptSection、skillIdentifiers | [AgentProfile.swift](../../../Xedit/Xedit/AgentProfile.swift) | 自定义指令和多个技能绑定已存在，不能写成从零开发 |
| AgentProfileStore v2 保存内置 overrides、managed 用户定义和 CLI discovered 档案；保留 v1 迁移数据 | [AgentProfileStore.swift](../../../Xedit/Xedit/AgentProfileStore.swift) | 加版本迁移与云端 origin；不要覆盖用户 overrides |
| 设置中的 addProfile 创建 managed UUID 的 willDeep 子代理 | [AgentProfileSettingsView.swift](../../../Xedit/Xedit/AgentProfileSettingsView.swift) | 复用现有设置入口 |
| asSubagentProfile 仅对内置 origin 开启 requiresWriteAccess | [AgentProfile.swift](../../../Xedit/Xedit/AgentProfile.swift) 的 asSubagentProfile | 自定义写入应以受控基础职责/本次授权实现，不伪装 builtIn |
| effectiveToolNames 对宿主集合求交；特殊权限还按工种 ID 判断 | [AgentSubagentProfile.swift](../../../Xedit/Xedit/AgentSubagentProfile.swift) | definition_id 与 base_role 必须拆开，不能直接塞新 ID |
| runner 使用 capabilityPromptSection 与预载技能；缺失/禁用技能当前静默跳过 | [AgentSubagents.swift](../../../Xedit/Xedit/AgentSubagents.swift) 的 systemPrompt/skillPromptSection | 云端必需技能需要兼容检查和明确失败语义 |
| runner 的第一条 user message 接收 openingImages；原生路径有 verifier 参数/结果处理 | [AgentSubagents.swift](../../../Xedit/Xedit/AgentSubagents.swift) 的 run | Mac 有图片与验证基础，逐入口验证后才能用于实验 |
| 外部 CLI 运行时，soul/capability 不具备原生子 loop 同样的生效能力 | [AgentProfile.swift](../../../Xedit/Xedit/AgentProfile.swift)、[统一设计](../../../Xedit/docs/AGENT_PROFILE_UNIFICATION_DESIGN.md) | 首期保证原生执行；外部 executor 明确能力限制 |
| some.im 集成有共享会话、UserDefaults 设置与 Keychain 凭据 | [SomeIMIntegrationStore.swift](../../../Xedit/Xedit/SomeIMIntegrationStore.swift)、[SomeIMIntegration.swift](../../../Xedit/Xedit/SomeIMIntegration.swift) | 复用账号身份，绑定不要进入公共包 |
| 记忆存储主要按 global/project 管理，没有本轮核实的 definition 命名空间 | [AgentMemoryStore.swift](../../../Xedit/Xedit/AgentMemoryStore.swift) | 后期扩展专属记忆，避免直接同步全部全局记忆 |
| 反馈包含 run/turn、promptBundle、参数 hash、human/model 判定来源；普通结束 unverified | [AgentFeedbackLedger.swift](../../../Xedit/Xedit/AgentFeedbackLedger.swift)、[AgentRuntimeFeedback.swift](../../../Xedit/Xedit/AgentRuntimeFeedback.swift) | 不能把原生结束或模型自评当独立验证 |

本次修正先前讨论的范围：对 Rust 的自定义缺口判断不能直接套到 Mac；Xedit 已经实现用户 soul 和技能绑定。仍缺本次建议的三端统一可分发定义、不可变 revision 和独立专业迭代体系。

## 4. some.im / MuchToken：可复用基础与新服务缺口

| 已核实事实 | 源码锚点 | 对规划的影响 |
|---|---|---|
| 主服务将 authapi 和 skillsmarket 挂载到 /api/v1 | [main.go](../../../muchtoken/main.go) | 资产管理接口归主站管理面 |
| 官方技能目录、详情和 zip 下载；embedded 目录与 DB MCP 动态 skill 合并 | [handler.go](../../../muchtoken/internal/skillsmarket/handler.go)、[catalog.go](../../../muchtoken/internal/skillsmarket/catalog.go) | 复用资源下载，但新建智能体版本领域 |
| 技能记录有内容 hash、字节数、来源、风险说明；星级由 GitHub stars 推导 | [catalog.go](../../../muchtoken/internal/skillsmarket/catalog.go) | hash 可用于内容校验，star rating 不能当质量评测 |
| MCP publish_skill 可上传多文件并按 slug upsert/delete-reinsert 当前资源 | [mcp_skills.go](../../../muchtoken/internal/authapi/mcp_skills.go)、[mcp.go](../../../muchtoken/internal/authapi/mcp.go) | 当前发布路径不是私有不可变 revision 存储 |
| 158/162 迁移提供技能目录/动态资源；216 清理 GitHub DB 遮蔽，保留 embedded 来源 | [158](../../../muchtoken/internal/migrate/sql/158_agent_skill_catalog.up.sql)、[162](../../../muchtoken/internal/migrate/sql/162_agent_skill_mcp_publish.up.sql)、[216](../../../muchtoken/internal/migrate/sql/216_skill_catalog_embedded_source_of_truth.up.sql) | 新服务不能把当前来源语义混成另一份权威目录 |
| client-login 有邮箱/browser/status/session exchange；会话含 UserID、Realm、RealmContextID | [router.go](../../../muchtoken/internal/authapi/router.go)、[session.go](../../../muchtoken/internal/authapi/session.go)、[client_login_session_exchange.go](../../../muchtoken/internal/authapi/client_login_session_exchange.go) | 新服务复用明确用户会话与生命周期 |
| API Key 身份解析使用 created_by，缺失时有 firstTenantMemberUserID fallback | [customer_api_key_auth.go](../../../muchtoken/internal/authapi/customer_api_key_auth.go) | 私有定义不得复用模糊所有者归属 |
| authWrap 处理席位路径白名单，customerTenant 再核对 realm/成员关系 | [session.go](../../../muchtoken/internal/authapi/session.go)、[ctx.go](../../../muchtoken/internal/authapi/ctx.go) | 新接口需明确 realm、tenant、owner 和席位许可 |
| 新增产品遥测受 event/枚举白名单约束，210 表禁止 prompt/路径/命令/自由 JSON | [client_telemetry.go](../../../muchtoken/internal/authapi/client_telemetry.go)、[210](../../../muchtoken/internal/migrate/sql/210_client_telemetry_events.up.sql) | RSI 材料单独授权和存储，不扩大遥测入口 |
| 模型能力接口返回 modalities、supports_tools、context_window 等；trace 有请求级观测 | [model_capabilities.go](../../../muchtoken/internal/gateway/model_capabilities.go)、[gwtrace.go](../../../muchtoken/internal/gwtrace/gwtrace.go) | 复用模型/路由与资源事实，不当答案质量标签 |
| API 基础响应已有 X-App-Version、X-Request-Id | [response.go](../../../muchtoken/internal/authapi/response.go) | 新接口延续版本可观测性 |

对 `internal/` 与 `cmd/` 的 Go/SQL 源码检索未发现本次拟定的 agent registry、agent_definition、agent_revision 或 agent-rsi 领域实现。结论限于所检索的本地源码，不宣称检查了生产数据库或其他服务。

## 5. RSI：已有结构与仍需补齐的证据

| 已核实事实 | 源码锚点 | 后续工作 |
|---|---|---|
| main/input_suggestion/worker 的稳定提示词有 bundle hash | [prompt_bundle.rs](../../crates/willdeep-core/src/prompt_bundle.rs) | 补依赖/用户规则/适配器的有效运行快照 |
| worker:<工种> 支持 capability_prompt 等分段变体 | [prompt_sections.rs](../../crates/willdeep-core/src/prompt_sections.rs) | 从 built-in ID 扩展到专业定义 revision |
| proposer 只看原段/计数/假设，结构门保护审批、安全与 verifier | [prompt_optimizer.rs](../../crates/willdeep-core/src/prompt_optimizer.rs)、[prompt_sections.rs](../../crates/willdeep-core/src/prompt_sections.rs) | 保留优化器边界，新增实验与发布审核 |
| gate 检查出处、任务集、validation/regression/holdout 和资源/未知 | [gate.rb](../../scripts/lib/prompt_rsi/gate.rb)、[prompt_rsi_eval.rb](../../scripts/prompt_rsi_eval.rb) | 复用完整性门禁，加入定义实际执行覆盖与跨端条件 |
| model_eval 进入 willdeep run --local；--profile 是 Provider profile | [model_eval.rb](../../scripts/model_eval.rb) | 不把该开关当作专业 Agent 选择，新增明确执行入口 |
| 本地 feedback 默认计数/hash；正文需显式 store_text；Worker 带 agent_id/worker_profile | [feedback.rs](../../crates/willdeep-core/src/feedback.rs) | 加 definition/revision 归属，上传单独 opt-in |

历史设计与审查作为规则背景，当前能力以本次源码为准：

- [Prompt RSI 设计](../PROMPT_RSI_DESIGN.md)、[本地操作手册](../PROMPT_RSI_OPERATIONS.md)。
- [2026-10-08 实现说明](../RSI_IMPLEMENTATION_2026_10_08.md)、[Mac 同步说明](../../../Xedit/docs/RSI_MAC_SYNC_2026_10_08.md)。
- [运行参数契约](../RSI_RUNTIME_PARAMETERS_V1.md)。

没有在本轮生成 baseline/candidate 结果、质量提升结论或云端版本发布记录。

## 6. 外部产品参照

延续本会话已经读取的官方资料，参考其可复用定义、技能与权限组织方式；本轮以三端现有代码规划为主，没有重新调研功能变更。

- [OpenAI / Codex 子智能体](https://learn.chatgpt.com/docs/agent-configuration/subagents)：个人/项目 TOML 定义、model/developer_instructions、sandbox/MCP/skills 配置；运行时权限仍受宿主策略影响。
- [Claude Code 自定义子智能体](https://code.claude.com/docs/en/sub-agents)：Markdown frontmatter、模型/工具/技能/MCP/记忆等。
- [ChatGPT 自定义 GPT 工作流与插件](https://learn.chatgpt.com/docs/migrate-custom-gpts)：指令、知识、技能与工具的复用组织。

这些配置机制与记忆机制不能单独证明自动 RSI 质量提升。
