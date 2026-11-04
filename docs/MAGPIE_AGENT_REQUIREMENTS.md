# Magpie 社区 Agent 插件接入需求

> Last updated: 2026-11-04 | 状态：**已实现，本地验收完成**
> 实施基线：CLI 0.141.0-rc3（`b83372be`）；Desktop 1.426.0-rc1（Xedit `4cd9e147`）
> 交付版本：CLI **0.142.0-rc1**；Desktop **1.427.0-rc1**（Build 941）；插件 **0.1.0-rc2**；未发布、未社区收录
> 适用范围：willdeep-rs、Xedit / WillDeep Desktop、两个 Magpie Agent 插件
> 关联文档：[设计](MAGPIE_AGENT_DESIGN.md)、[执行台账](MAGPIE_AGENT_PLAN.md)、[验收报告](verification/0.142.0-rc1/magpie-agents/README.md)、[现有网关接入](MAGPIE.md)
> 一句话：通过社区插件给 Magpie 增加两个独立条目，以单独的 JSON 配置提供客户端连接契约。

## 0. 需求来源与已确认范围

用户先提出“怎么让 magpie 里可以有 willdeep 和 willdeep-cli”，明确产品是 https://usemagpie.ai，随后选择“社区插件路线”。2026-11-04 用户确认“按设计实现”，首版范围、数据格式、会话语义及 D1–D5 均已确认，具体决策见设计第 3 节。

此文档是跨端契约的 canonical。功能已按本契约实现并完成 macOS 本地组合验收，验收边界见[验收报告](verification/0.142.0-rc1/magpie-agents/README.md)；“按设计实现”不授权 npm 发布、社区提交或修改用户实际配置。

## 1. 现状与证据

以下代码锚点记录实施前的职责与缺口，不作为生产实现或验收完成的声明。

| 代码锚点 | 现状 | 本次缺口 |
| --- | --- | --- |
| `crates/willdeep-cli/src/config.rs:845` | CLI 读取 TOML；`:871` 按显式 Profile、默认 Profile、唯一 Profile 选择 | 没有独立 JSON 集成来源 |
| `crates/willdeep-cli/src/harness.rs:376` | Runtime 每次任务重新加载配置 | 仍须处理显式会话模型和常驻进程的环境优先级 |
| `crates/willdeep-cli/src/local_provider_discovery.rs:120` | 已能发现本机 Magpie 并读取模型目录 | 方向是客户端发现网关，没有注册 Magpie Agent |
| `../Xedit/Xedit/AgentLocalModelGatewayDiscovery.swift:117` | Desktop 已支持 Magpie 探测和模型读取 | 没有外部集成文件读取器 |
| `../Xedit/Xedit/AgentSettingsStore.swift:773`、`:848` | Desktop Provider 库存在 UserDefaults；凭据分离保存在钥匙串 | 不能作为通用 JSON 配置直接交给 Magpie 编辑 |
| `../Xedit/Xedit/AgentWindowRootView.swift:80` | 当前深链处理 OAuth、会话和文件 | 没有 Provider import；社区插件也没有任意深链调用钩子 |
| `docs/XEDIT_INTEROP_STATUS.md:64` | 双端尚未共享 Provider 凭据 | 新集成不得偷偷合并双端配置或真实凭据 |

外部协议以核查的 Magpie `759b4d1d7da8b983860539e3bd8ce1eaf70a06a4` 为基线：

- [Agent plugins 文档](https://github.com/yetone/magpie/blob/759b4d1d7da8b983860539e3bd8ce1eaf70a06a4/site/public/docs/plugins.html#L813-L865)：一个 Agent 描述声明一个配置文件、模型键和可选 `connect`。
- [格式校验](https://github.com/yetone/magpie/blob/759b4d1d7da8b983860539e3bd8ce1eaf70a06a4/internal/agentplug/agentplug.go#L283-L304)：只支持 JSON、YAML、env，尚不支持 TOML。
- [实际注册链](https://github.com/yetone/magpie/blob/759b4d1d7da8b983860539e3bd8ce1eaf70a06a4/internal/agent/agents.go#L195-L200)：合并内置与插件 Agent。
- 旧 `docs/integrating.md` 关于“插件不能添加条目”的说明与当前代码冲突，不作为本方案的能力依据。

## 2. 用户场景与首版边界

已确认首版交付：

1. 用户安装支持该契约的新客户端，再安装、启用对应 Magpie 插件。
2. Magpie 的 Agents 页分别出现 **WillDeep**（ID `willdeep`）和 **WillDeep CLI**（ID `willdeep-cli`）。
3. 用户在条目中连接并选模型。后续没有显式选择其他 Provider/模型的新会话使用该连接。
4. 两端显示实际有效的 Magpie Provider、模型及外部管理来源。
5. 切换 Magpie 模型只改变后续新会话默认值；进行中的任务、已有会话固定模型、显式 Worker Profile 保持既有语义。
6. 断开后，新会话重新采用原有默认配置；已有会话若固定引用已断开的集成 Provider，下一次执行明确提示重新连接或另选 Provider。

首版支持本机回环网关和 OpenAI Chat Completions。CLI 在 macOS、Linux、Windows 验收；Desktop 在 macOS 验收。新反馈沿用两端现有中文、英文、日文资源。Magpie 插件的静态提示及市场摘要按其宿主支持方式提供。

首版验收使用标准用户目录。CLI 自定义 `WILLDEEP_HOME` 的读取器必须保持目录隔离，但发行插件默认描述符只指向标准路径；自定义目录用户需要匹配路径的本地描述符，不能声称两个不同进程的环境会自动同步。见设计中的路径约定。

## 3. 参与方和所有权

| 参与方 | 职责 | 写入边界 |
| --- | --- | --- |
| Magpie Agent 插件 | 声明 Agent、校验输入、返回要写入的字段 | 各自唯一的集成 JSON；实际写入由 Magpie 宿主执行 |
| CLI 消费端 | 读取、校验、构造临时 Provider，接入 TUI/无头/Runtime/Web 的统一解析链 | 正常会话状态；不把集成字段写进 TOML |
| Desktop 消费端 | 读取、校验、提供临时 Provider 和新会话默认模型 | 正常会话状态；不改原 Provider 库及钥匙串 |
| 用户 | 在 Magpie 选择连接、模型，在客户端显式选择会话配置 | 原有配置仍归用户所有 |

两个集成文件独立，插件 ID 独立；共享格式不代表共享连接或控制状态。CLI 临时 Profile ID 固定为 `magpie-cli`，Desktop 临时 Provider UUID 固定为 `70D7B194-6038-4B32-8195-84F25E1648A3`。两者已在共享 fixture 中冻结，均不得覆盖用户已有的同 ID 条目。

## 4. JSON v1 契约

格式标识：`willdeep.magpie-agent.v1`。CLI 默认文件 `~/.willdeep/integrations/magpie/cli/connection.json`，Desktop 默认文件 `~/.willdeep/integrations/magpie/desktop/connection.json`。

已冻结的机器可读资料：[JSON Schema](schemas/magpie-agent.v1.schema.json)、[共享 fixture 与预期状态](fixtures/magpie-agent-v1/cases.json)。fixture 的 `contents` 是原始 JSON 字符串，测试必须原样送入消费者；不能先解析再序列化而丢失重复键或损坏输入。`contents: null` 表示文件缺失。Schema 不替代整数 token、重复键、UTF-8 字节数、预期客户端、URL 数值和 `output <= context` 等消费端检查。

连接状态示例（其中 `client_marker` 是固定公开客户端归因标记，不是供应商凭据）：

```json
{
  "schema_version": 1,
  "client_id": "willdeep-cli",
  "enabled": true,
  "base_url": "http://127.0.0.1:3425/v1",
  "client_marker": "magpie-willdeep-cli",
  "model": "provider/model",
  "context_window": null,
  "max_output_tokens": null
}
```

| 字段 | 类型与约束 |
| --- | --- |
| `schema_version` | 整数 JSON token，必须为 `1`；拒绝小数或指数写法 |
| `client_id` | CLI 固定 `willdeep-cli`；Desktop 固定 `willdeep`；必须匹配当前消费端 |
| `enabled` | 布尔值；只有 `true` 才申请使用集成 |
| `base_url` | 仅小写 `http://` 或 `https://`；主机仅 `localhost`、规范四段 `127/8` IPv4 或 `[::1]`；可选端口为 `1..65535`，路径精确为 `/v1` |
| `client_marker` | CLI 固定 `magpie-willdeep-cli`；Desktop 固定 `magpie-willdeep`；必须与 `client_id` 精确配对 |
| `model` | 非 blank 的完整模型 ID，保留供应商前缀及原始值；禁止 C0/C1 控制字符，最多 `1024` UTF-8 字节 |
| `context_window` | 可省略或为 `null`；有值时为整数 JSON token，范围 `4000..1048576` |
| `max_output_tokens` | 可省略或为 `null`；有值时为整数 JSON token，范围 `1..1048576`；两项元数据均有值时必须 `max_output_tokens <= context_window` |

解析要求：单文件最多 `65536` 字节（64 KiB）；拒绝重复 JSON 键、未知字段、错误类型、非法字段值、超限整数、跨客户端 ID 和未支持版本。整数 token 不接受 `1.0`、`1e0` 等写法，即使其数值恰为整数。所有已提供字段先完成校验，再判定状态；`enabled: false` 不能绕过未知字段或其他非法字段检查。契约扩展必须协调两端和两个插件，不以静默丢字段处理。

URL 使用原始输入严格匹配，不靠 URL 解析器的归一化接纳非法输入。四段 IPv4 的首段为 `127`，每段为 `0..255` 的规范十进制表示，不接受简写、前导零、十六进制等替代写法。拒绝 user-info、query、fragment、其他路径（含 `/v1/`）、任意主机别名及大小写变体。`localhost` 的实际请求必须固定到回环地址，不能经 DNS 解析连接到其他目标。

状态约定：

- 文件不存在、空对象 `{}` 或明确 `enabled: false` 且已提供字段均合法：`disconnected`，不发探测或模型请求。
- 仅有一个合法非 blank `model` 字段、没有任何连接字段的对象：`disconnected`。这是 Magpie 恢复托管字段后切回原生模型可能留下的合法结果；该字段不成为外部原生模型配置，不应用该模型，客户端重新采用自己的原默认值。
- `schema_version: 1`、`enabled: true`，且 `client_id`、`client_marker`、`base_url`、`model` 等完整校验通过：`connected`。
- 除上述未连接形态外的非空不完整连接、读取失败、损坏 JSON、错误版本或非法字段：`invalid`，不能作为另一个 Provider 的连接材料使用。
- 对集成默认选择发生错误时停止该次新会话执行并提示；用户显式选择其他有效 Provider、模型或连接覆盖的任务不受无关集成错误阻断。
- JSON 内没有待执行命令、文件写入目标、审批档位、Worker 改写或真实远程凭据字段。

## 5. 选择优先级和生效契约

1. 用户明确给出的其他 Provider/模型选择和进程级连接覆盖优先，沿用各客户端原有规则；无关集成文件的 `invalid` 不阻断这些显式选择。
2. 有效集成只作为“没有显式选择”的新会话默认值，优先于落盘默认值，但不覆盖落盘内容。
3. 若 CLI 有 `--profile`、自定义 `--config` 或通用 Provider/地址/Key/模型覆盖，则不自动采用集成。用户显式选择集成 Profile 时，必须作为完整连接解析，禁止混用其他 Profile 的地址或凭据。
4. 同一会话的有效 Provider ID、模型在创建/首次提交时确定。恢复或继续该会话不重新套用 Magpie 的新默认模型。旧会话缺少固定 Provider/模型时，沿其原有用户配置解析并跳过集成默认值；只有用户明确选择集成时才绑定它。
5. 文件中模型 A 改为 B 后，客户端在下一次新会话创建前读取；旧 A 会话继续 A，新会话采用 B。不依赖重启 daemon，不依赖给 daemon 注入新环境。
6. 断开集成不取消正在执行的任务。新的执行如果明确引用断开的集成 Provider，必须提示不可用，不能静默发给原默认供应商。
7. 工具审批、沙箱、路由策略和已显式绑定的 Worker Profile 不由此协议更改。

## 6. 本机和安全要求

- 仅允许第 4 节严格定义的本机回环目标；实际请求绕过代理并禁止重定向。`localhost` 在请求时固定到回环，不接受任意主机别名或可重绑定的解析结果。
- 插件先校验 `gateway.v1` 和 `gateway.key`，只有后者恰好为本 Agent 的公开标记才返回配置；远程/共享网关使用真实 Key 的场景首版拒绝，不能把该 Key 写进文件。
- 客户端读取边界再次执行全部校验；安全不变量不依赖插件 UI。
- 不读取、搬运或发布用户真实供应商凭据，不自动读取桌面钥匙串为 CLI 授权。
- 不读取项目内同名文件作为集成来源；路径只来自客户端配置根和固定后缀。
- 文件读取拒绝符号链接及非普通文件，并使用有界读取；安全打开后校验同一文件句柄，兼顾原子替换，避免 check/open 竞态。可重试 I/O 或 JSON 截断仅在异步路径延后 `100ms` 重读一次；非法字段、版本或客户端错误立即返回 `invalid`，不得以重试绕过校验。
- 该文件仅含公开标记；不因 Magpie 创建文件的默认 mode 与 CLI 私密 TOML 的 `0600` 不同而要求改变主配置权限。
- 不把连接标记/原始 JSON 放进模型上下文、会话内容或诊断正文。错误输出使用字段名和错误分类；地址无 user-info。

## 7. 状态反馈

| 状态 | 用户可见反馈 |
| --- | --- |
| 客户端尚不支持该契约 | 插件安装要求中显示最低客户端版本；不声明已接通 |
| 尚未安装/首次启动 | 不借共享 `.willdeep` 目录假装检测到两个产品 |
| 未连接/断开 | 原有新会话默认值继续可用，集成 Provider 不可新选 |
| 已连接 | 显示 Magpie、有效模型，以及由 Magpie 管理的来源 |
| 模型变化 | 提示对新会话生效，既有任务不切换 |
| JSON 损坏/无权限/不完整写入 | 仅可重试 I/O 或截断异步重读一次；仍无效则明确显示错误，非法字段立即报错；不回退为隐蔽的其他供应商调用 |
| 网关未启动/模型不可用 | 复用现有网络/Provider 错误；不自动安装、重启网关或付费探活 |
| 显式集成会话断开后续聊 | 提示重连或选择另一个 Provider |

## 8. 可验收标准

- [x] 两个插件可从本地目录分别安装，Agent ID、名称、图标、模型选择独立。
- [x] 缺少 `willdeep` 二进制不会因 Desktop 的共享目录而误判 CLI 已安装。
- [x] Desktop 使用自己的已启动标记/目录检测，首次启动要求写进说明；不依赖可选的 `willdeep-app` 启动器。
- [x] 两端对同一批共享有效/无效 fixture 得出一致结论，原始重复键与损坏输入未经重新序列化。
- [x] 连接、A→B 切模型、断开恢复覆盖 CLI 本地执行、TUI、已运行的隔离 Runtime/Web；Desktop 由原生测试与模拟上游覆盖，未做真实 App 点击验收。
- [x] 明确其他 Profile、已有会话、活动任务、显式 Worker Profile 和两端原配置的行为符合第 5 节，旧无固定选择会话恢复跳过集成默认值。
- [x] 原 TOML、UserDefaults Provider 库和钥匙串均没有因本集成被改写。
- [x] 自定义网关端口仍归为远程 Magpie 推理，不记为免费本地模型。
- [x] 禁止远端 URL、真实网关 Key、重定向、代理、错误客户端 ID 等输入；验证 localhost 请求固定回环，错误信息脱敏。
- [x] API/E2E 验收输出 JSON 和 Markdown，使用临时 HOME、配置目录和模拟上游，不读取真实账号或产生付费调用。
- [x] 社区收录与公开发布完成前，交付状态明确为本地可安装；不写“已上架”。

第 5 项中 Desktop 的真实 App 前台点击组合验收、以及 Linux/Windows 原生执行未完成，已如实记录在验收报告。

## 9. 非目标与实施状态

首版不做远程/共享 Magpie、现有活动会话的外部强制切模、真实凭据共享、MCP/Skills Library 接管、官网首页展示或上游内置适配器。自定义根目录的开箱即用插件路径配置留在后续兼容范围，首版提供明确的本地描述符修改说明并测试目录隔离。

D1–D5 已于 2026-11-04 确认，不再重复询问路线或实现细节。T0–T6 已完成：schema/fixture 冻结，CLI 与 Desktop 消费端、双插件及 macOS 本地组合验收均已交付；实际结果与未覆盖项见[验收报告](verification/0.142.0-rc1/magpie-agents/README.md)。交付版本为 CLI `0.142.0-rc1`、Desktop `1.427.0-rc1`、插件 `0.1.0-rc2`；T7 公开发布与社区提交仍须另行授权，当前未发布、未收录。
