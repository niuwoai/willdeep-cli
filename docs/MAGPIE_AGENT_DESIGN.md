# Magpie 社区 Agent 插件接入设计

> Last updated: 2026-11-04 | 状态：**已实现，本地验收完成**
> 实施基线：CLI 0.141.0-rc3（`b83372be`）；Desktop 1.426.0-rc1（Xedit `4cd9e147`）；Magpie 源码 `759b4d1d`
> 交付版本：CLI **0.142.0-rc1**；Desktop **1.427.0-rc1**（Build 941）；插件 **0.1.0-rc2**；未发布、未社区收录
> 适用范围：willdeep-rs、Xedit / WillDeep Desktop、两个独立 Magpie Agent 插件
> 关联文档：[需求与线格式](MAGPIE_AGENT_REQUIREMENTS.md)、[执行台账](MAGPIE_AGENT_PLAN.md)、[验收报告](verification/0.142.0-rc1/magpie-agents/README.md)、[双端边界](XEDIT_INTEROP_STATUS.md)
> 一句话：Magpie 只写自己的 JSON 文件，两端把它解析成临时 Provider，并只对新会话提供默认选择。

## 0. 需求来源

用户已选择社区插件路线，并于 2026-11-04 确认“按设计实现”，D1–D5 已移入第 3 节已确认决策。当前缺口是插件可编辑的配置格式与客户端实际存储不同；模型传输协议和本地发现能力已经存在。这是既有项目迭代，不是新建产品。

本设计只增加一个窄的客户端集成入口，不重做 Provider 库、凭据后端或 Magpie 自身的插件宿主。实现授权不包含发布、社区提交、读取真实配置/凭据或安装、重启用户程序。

## 1. 现状盘点（代码锚点）

以下代码锚点记录实施前的职责与缺口，不作为生产实现或验收完成的声明。

| 位置 | 当前职责 | 实施时的约束 |
| --- | --- | --- |
| `crates/willdeep-cli/src/config.rs:841` | `LoadedConfig` 保存解析后的用户 `ConfigFile` | 保留磁盘配置与有效 Provider 来源的区别 |
| `crates/willdeep-cli/src/config.rs:871` | 显式 Profile → 默认 Profile → 唯一 Profile | 新集成只能插入隐式的新会话默认选择，不压过显式意图 |
| `crates/willdeep-cli/src/provider_settings.rs:93` | 统一解析地址、Key、模型等覆盖 | 集成必须作为整套连接，不能与旧环境凭据拼接 |
| `crates/willdeep-cli/src/harness.rs:383` | 每次 Runtime 任务重新读配置 | daemon 无须为了外部文件变更而升级或重启 |
| `crates/willdeep-cli/src/daemon/session_store.rs:1482` | 会话固定模型进入任务参数 | 不能把更新外部默认值当作切换现有会话 |
| `crates/willdeep-core/src/provider/mod.rs:134` | 识别 Magpie 远程网关，避免记为本地模型 | 新 CLI 标记和非默认端口必须覆盖此分类 |
| `../Xedit/Xedit/AgentSettingsStore.swift:773`、`:848` | Desktop 加载 Provider 库、分离保存钥匙串 | 临时集成不得被保存为永久用户条目 |
| `../Xedit/Xedit/AgentLocalModelGatewayDiscovery.swift:23` | 已有发现结果转换为 Provider entry | 复用构造能力，但不复用“自动保存到用户库”流程 |
| `../Xedit/Xedit/AgentNetworkSupport.swift:49` | Desktop 请求身份 | 本机按独立公开标记区分双端用量 |
| `crates/willdeep-core/src/lib.rs:108` | CLI 请求身份 | 现有两端 UA 都以 WillDeep 开头；不把 UA 匹配当唯一归因机制 |

Magpie 插件能力依据为[新版 Agent plugins 文档](https://github.com/yetone/magpie/blob/759b4d1d7da8b983860539e3bd8ce1eaf70a06a4/site/public/docs/plugins.html#L813-L865)。插件在 moejs 内运行，没有 Node APIs、任意文件系统访问、终端启动或应用导入回调；配置写入和恢复由宿主实施。

## 2. 结论：两个只读消费端，两个独立 JSON 文件

### 2.1 设计原则

1. 共享线格式、fixture 和行为，不共享开关、用户凭据或运行时。
2. Magpie 对自己的文件拥有写权；客户端只读消费，不与宿主争写。
3. 外部配置是临时有效来源；原有默认配置和 Provider 库保留。
4. 每次选择明确其 Provider 来源；错误不触发隐式的其他供应商调用。
5. 第一版作用于新会话默认值；已有会话、正在执行任务及审批边界保持原有语义。

### 2.2 方案取舍

| 方案 | 取舍 |
| --- | --- |
| 新增独立 JSON 集成入口 + Agent 插件 | 已确认。插件格式受支持，客户端自己控制加载/校验/会话语义，可独立分发 |
| 直接让插件编辑 TOML | 当前宿主不支持；不能靠改扩展名或写一个无人读取的文件实现 |
| 外部直接改 Desktop UserDefaults/钥匙串 | 缓存、嵌套 JSON、签名权限和恢复复杂，耦合内部存储 |
| 给 Desktop 加 Provider 导入 URL | 适合内置 Import 适配器，但社区 Agent 描述没有该钩子，且只导入不能实现完整切模与恢复 |
| 内置 Go 适配器 | CLI 可少改客户端，但依赖主仓维护者接纳和发版，不是这次已选路线 |

## 3. 已确认决策（2026-11-04）

用户确认用语为“按设计实现”；以下决策已确认，不再重复询问路线或实现细节。确认覆盖实现，不包含正式发布和社区提交。

- 采用社区 Agent 插件路线，分别提供 WillDeep 和 WillDeep CLI。
- 两端已有 Magpie 网关能力可以复用；两端消费端和两个插件按同一冻结契约实施，并已完成 macOS 本地组合验收。
- T0–T6 完成；T7 发布与社区提交未授权，当前交付为本地可安装源码。

| 编号 | 已确认决定 | 影响 |
| --- | --- | --- |
| D1 | 本机回环 Magpie；真实远端 Gateway Key 不落文件，首版不接共享网关 | 缩小凭据与远程部署范围 |
| D2 | 接通时作为无显式选择的新会话默认值；已有会话/活动任务不自动切换 | 决定运行中生效范围 |
| D3 | 两端独立临时配置；断开回到原有默认；原 TOML、用户库和钥匙串保留 | 决定所有权和恢复策略 |
| D4 | 首版交付本地可安装双插件及收录材料；正式发布和社区提交在验收后另行授权 | 区分实现、发布与收录 |
| D5 | 标准用户目录开箱即用；自定义 CLI 根保持隔离，使用匹配的本地描述符 | 避免对环境自动继承作错误承诺 |

## 4. 总体架构

```text
Magpie Agents 页
    ├─ willdeep-cli 插件 ─ Magpie JSON writer ─ cli/connection.json
    │                                           ↓
    │                               CLI 有效配置解析 / Runtime
    │                                           ↓
    │                                  新会话 Provider + 模型
    └─ willdeep 插件 ───── Magpie JSON writer ─ desktop/connection.json
                                                ↓
                                   Desktop 临时 Provider 目录
                                                ↓
                                       新会话 Provider + 模型
```

原 TOML 和 UserDefaults Provider 库仍是用户配置源。JSON 消费端不得调用这些配置源的保存方法来“同步”集成。

### 4.1 文件布局与目录隔离

| 内容 | 路径/归属 |
| --- | --- |
| CLI 集成 | `$WILLDEEP_HOME/integrations/magpie/cli/connection.json`；未设置时为 `~/.willdeep/...` |
| Desktop 集成 | 用户 Home 下 `~/.willdeep/integrations/magpie/desktop/connection.json` |
| Desktop 支持标记 | 同一 Desktop 私有目录内的 `client.json`，仅客户端创建，记录支持契约的客户端身份和版本 |
| 协议说明 | 本仓 `docs/MAGPIE_AGENT_REQUIREMENTS.md`，canonical |
| 已冻结机器可读 schema | [docs/schemas/magpie-agent.v1.schema.json](schemas/magpie-agent.v1.schema.json) |
| 已冻结跨端 fixture | [docs/fixtures/magpie-agent-v1/cases.json](fixtures/magpie-agent-v1/cases.json)，含原始输入及预期状态 |
| 两个插件源码 | `integrations/magpie/agent-willdeep/`、`integrations/magpie/agent-willdeep-cli/` |

发行插件的默认 `config` 是各自标准 `~/.willdeep/...` 路径。Magpie 只支持 `~` / `$VAR` 展开，不支持 shell 默认值表达式；不会推断 CLI 另一进程的 `WILLDEEP_HOME`。自定义根目录需匹配的本地描述符；客户端不得为了找文件越过配置根去探测普通 Home 的连接。`--config` 默认不激活外部默认值。

Desktop 启动时仅创建自己的私有支持标记目录，不创建或修改 Magpie 托管的连接文件。插件用此目录检测支持该契约的 Desktop；不使用共享 `.willdeep` 根目录，也不依赖用户可选安装的 `willdeep-app`。这是“至少启动过支持版本”的检测，卸载后遗留目录的行为与普通配置型 Agent 相同，不能宣称实时验证了 App bundle。

CLI 插件声明 `bin: willdeep`，并只引用自己的配置文件。Desktop 不创建 CLI 私有目录，所以不会因共享根目录而把未使用的 CLI 误判为已安装。

### 4.2 协议所有权

同名格式按 `client_id` 区分，客户端只读取自己对应的文件。保留 `magpie` 这个已有发现 Profile，新的 CLI 临时 Profile ID 固定为 `magpie-cli`；若该 ID 已被用户 TOML 占用，报告冲突而不是覆盖。

Desktop 使用稳定的集成 UUID `70D7B194-6038-4B32-8195-84F25E1648A3` 常量构造 `AgentProviderEntry`，并在有效目录中带上来源元信息。若用户库已存在该 UUID，拒绝冲突。UUID 只为适配现有会话/选择器键，不写回用户库；该 ID 已与 CLI 的 `magpie-cli` 一起在共享 fixture 中冻结，不能每次生成随机值。

## 5. 数据模型与读取状态

完整字段、验证及示例见需求第 4 节。已冻结的 schema 和 fixture 不替代消费端校验；fixture 的 `contents` 必须作为原始 JSON 输入保留，不能先解析再序列化而丢失重复键或损坏输入。`contents: null` 表示文件缺失。

客户端实现统一的概念状态；判定断开之前先验证所有已提供字段，不能用 `enabled: false` 绕过重复键、未知字段、错误类型/版本、跨客户端或其他非法字段检查：

- `disconnected`：文件不存在、空对象、合法关闭，或只包含合法非 blank `model` 的原生模型恢复结果。最后一种结果来自宿主的恢复后设模路径，不能误判为损坏连接，也不能应用该模型。
- `connected(config)`：`schema_version: 1`、`enabled: true`，且 `client_id`、`client_marker`、`base_url`、`model` 等完整通过验证。CLI 身份为 `willdeep-cli` / `magpie-willdeep-cli`，Desktop 为 `willdeep` / `magpie-willdeep`。
- `invalid(reason)`：不完整、损坏、不支持的版本、权限或字段错误。

读取时只解析数据，不发起模型调用。候选连接必须通过回环目标验证；实际发出请求前使用无代理、无重定向客户端。

### 5.1 文件更新与异常重读

核查的 Magpie 源码先收集模型及 `connect` 字段，再通过一次 `SetJSON` 批量写入，并用 `atomic(a, path)` 包装适配器（[`plugged.go:180`](https://github.com/yetone/magpie/blob/759b4d1d7da8b983860539e3bd8ce1eaf70a06a4/internal/agent/plugged.go#L180-L186)、[`:315`](https://github.com/yetone/magpie/blob/759b4d1d7da8b983860539e3bd8ce1eaf70a06a4/internal/agent/plugged.go#L315-L334)、[`:470`](https://github.com/yetone/magpie/blob/759b4d1d7da8b983860539e3bd8ce1eaf70a06a4/internal/agent/plugged.go#L470)）。不将宿主正常行为描述为逐键落盘，也不把字段收集顺序当成消费者可见的提交顺序。

两端读取文件的有界快照，拒绝符号链接与非普通文件。安全打开与元数据校验必须针对同一文件句柄，避免 check/open 竞态，同时支持宿主原子替换文件；不能用先检查路径、随后普通重开的方式规避校验。

对可重试 I/O、读取竞态或 JSON 截断，在当前异步路径延后 `100ms` 重读一次；仍失败则返回 `invalid`。明确的未知版本、错误客户端、重复键、未知字段、错误类型或非法字段直接返回错误。重读是防御策略，不保证外部写者会在该时间内完成。不得阻塞 UI 主线程、无限轮询或把上次有效连接当作断开后的常驻后备。

大小上限为命名常量 `MAX_MAGPIE_CONNECTION_BYTES = 65536`；重读次数和延迟也使用命名常量。运行中的任务继续持有已构建 Provider；新执行必须重新读取/校验。

### 5.2 模型元数据

`model.id` 原样写入 `model`，保留供应商前缀；必须非 blank、无 C0/C1 控制字符且最多 `1024` UTF-8 字节。`model.context`、`model.output` 仅在合法时写入对应可空字段。未知值写为 `null`，不编造窗口或输出上限。

`context_window` 有值时范围为 `4000..1048576`，`max_output_tokens` 有值时范围为 `1..1048576`；两者都有值时必须 `max_output_tokens <= context_window`。两项均可省略或为 `null`；与 `schema_version` 一样，数字必须使用整数 JSON token，拒绝小数和指数写法（包括数值等于整数的 `1.0`、`1e0`）。消费端仍应用系统预算，不因外部目录给出更大值而扩大工具/Token 安全边界。

插件侧仅写契约字段，不复制 `models` 全目录或供应商凭据。模型目录、图像能力与推理等级的完整同步留待后续扩展版本。

### 5.3 严格 URL 与实际请求约束

原始 URL 只接受小写 `http://` 或 `https://`，主机精确为 `localhost`、规范四段 `127/8` IPv4 或 `[::1]`。IPv4 首段为 `127`，各段为 `0..255` 的规范十进制表示，拒绝简写、前导零、十六进制等形式。可选端口为 `1..65535`，路径精确为 `/v1`；拒绝 user-info、query、fragment、其他路径（包括尾部斜杠）、主机别名及大小写变体。不通过 URL 解析器的归一化把非法原始输入变成合法地址。

`localhost` 在实际请求时固定到回环地址，不依赖可能返回其他目标的 DNS 解析。所有集成请求均禁用代理和重定向；校验与实际连接边界必须一致，不能仅在插件 `connect` 中检查。

## 6. 双端配置解析与关键流程

### 6.1 CLI

新增窄模块（建议 `crates/willdeep-cli/src/magpie_integration.rs`）负责路径、解析与来源；`ConfigFile` 保留用户磁盘配置，另建有效 Provider 视图，不把临时 Provider 注入会被 `config show` / `config set` 持久化的字段。

统一解析入口向 TUI、无头执行、Runtime 和 Web 目录投影同一来源。实施前枚举直接读取 `loaded.file.providers` 的调用点，按用途区分“磁盘编辑”与“有效连接”，逐个接到相应视图。

隐式新会话选择：先识别 CLI/环境显式覆盖和自定义 `--config`；没有这些意图时，再读取当前集成状态。连接有效则选 `magpie-cli`，关闭则沿原选择器，错误则明确阻止本次隐式集成选择。显式选择其他 Provider、模型或连接覆盖时，无关集成文件的 `invalid` 不阻断该选择。

用户显式指定 `--profile magpie-cli` 时，只能解析该临时连接。其他 `WILLDEEP_*` 连接覆盖与该显式连接冲突时应报冲突，不能把外部 Key 与其他 Base URL 拼起来。用户如需要完全手工连接，仍使用原来的普通 Profile。

新会话在创建/首次提交时保存所选 Provider ID 与模型。后续任务重新读取集成端点，但继续使用会话自己的模型；原会话 Provider 为其他来源时，不受外部默认值影响。旧会话若缺少固定 Provider/模型，恢复路径跳过集成默认值，沿其原有用户配置解析；不能因本次新增默认来源而改用 Magpie。明确引用集成但它已断开时，返回可恢复错误。用户显式给集成会话指定模型可沿用该集成端点；地址、Key、Provider/API 等连接覆盖与显式集成冲突时才报告冲突。

Runtime 每任务读取文件，不能把注入环境变量、重启 daemon 或临时 CLI 覆盖作为集成的必要条件。Web/定时任务沿同一解析器，但有显式 Provider/模型的配置仍优先。

### 6.2 Desktop

新增窄模块（建议 `Xedit/AgentMagpieIntegration.swift`），复用 Provider entry 和现有模型网络调用，但在内存中维护“用户库 + 临时集成”的有效目录。

新会话创建/首次提交之前读取外部状态；模型选择器打开和 App 回到前台可刷新可见状态，磁盘 I/O 离开主线程。已有会话通过原 Provider/模型选择，不能因全局临时默认变化改写其 override；旧会话若缺少固定 Provider/模型，恢复时跳过集成默认值。显式选择其他 Provider、模型或连接覆盖的路径不受无关集成 `invalid` 阻断。

只在内存中向 Provider 选择器呈现集成条目，来源显示为 Magpie 管理，地址/鉴权字段不可在普通用户库编辑页保存。该条目不会进入 `saveProviderLibrary` 或钥匙串写入。

当前 Provider UUID 在所有解析路径必须能解析到临时条目，包括会话恢复、辅助请求和模型目录；若引用断开的临时条目，要返回同一种不可用错误，不复用旧活动库作为隐蔽后备。

### 6.3 两个 Agent 插件

每个包提供 `package.json`、`*.agent.js`、图标和原生 JS 单元测试；`package.json` 声明 `magpie.agent`，不依赖 Node 模块或安装后脚本。

描述符：唯一 ID/名称、固定 `config`、`format: json`、`model: model`、各自检测依据、图标、对新会话生效的提示。`connect` 使用宿主传入的 `gateway` 和 `model` 计算并返回所有其余契约键。

`connect` 在任何返回前检查地址、公开客户端标记和模型完整性；远程地址或真实 Gateway Key 直接拒绝。返回值只含本契约字段，不设置命令、审批、进程环境或主配置路径。元数据仅在已知且合法时输出数值，未知值为 `null`。

包名在实施查重及发布权限确认后确定；开发和验收先用本地目录安装。加入社区仓库时遵守其 `@magpie-community/agent-<name>` 命名和 `registry.json` 的 `kind: agent` 规则，不能提前声称获得该命名空间的发布权。

### 6.4 断开、恢复和用户修改

Magpie 恢复其托管字段，客户端观察关闭、空对象或缺失文件即停止把该来源作为新会话默认。宿主切回非 Magpie 原生模型时，会在恢复后单独写 `model`，可能留下仅有合法模型字段的对象（[`plugged.go:407`](https://github.com/yetone/magpie/blob/759b4d1d7da8b983860539e3bd8ce1eaf70a06a4/internal/agent/plugged.go#L407-L420)）；该状态同样表示断开，不直接应用这个无 Provider 归属的模型。原 TOML、用户 Provider 库和钥匙串从未被改过，所以不需要通过复制整份快照恢复它们。

Magpie 通用插件恢复当前没有逐键 CAS 用户修改保护。为此集成文件明确为 Magpie 托管文件，不承诺保留用户对这些托管键的手工改写；用户个性化设置应通过客户端原有配置与显式会话选择表达。文件里的非协议未知字段被消费端拒绝，不提供混合编辑区。

## 7. 不可让渡的安全不变量

1. 只有第 5.3 节定义的严格回环 URL 和固定公开归因标记可进入连接；两端读入和请求发出边界都校验，localhost 请求固定回环。
2. 真实供应商 Key、远端 Gateway Key、钥匙串内容不进入集成 JSON。远程集成需要未来的独立凭据方案。
3. 本地文件不是执行脚本；不包含 shell、审批策略或自动重启指令。
4. 不越过自定义配置根读取个人连接；测试只使用依赖注入的临时目录。
5. 请求不走代理、不跟重定向。连接身份来源与端点一起选择，不能靠同名 UA 混用。
6. 临时连接的目录/可见性不扩大文件工具、沙箱、Mobile Gateway 或 Web 管理权限；保留审批、沙箱及显式 Worker 设置。
7. 对集成 Provider 的显式引用失效时，错误优先于发送给其他供应商。

## 8. 失败路径与 UI

沿用两端现有视觉体系和 Provider 选择器，不新增独立工作台。中文、英文、日文文案通过现有本地化资源；Magpie 插件提示遵循宿主支持的字段。

状态：未连接、正在刷新、已连接、配置无效、网关不可用、会话引用已断开；具体行为见需求第 7 节。刷新状态不抢占输入焦点，不以轮询让应用保持忙碌。

不因插入新 Provider 自动升级审批档位、不取消任务、不清空上下文。Magpie 显示“配置已写入”不等于客户端实际推理成功；只有隔离端到端证据才可宣称已验证调用。

## 9. 兼容性、版本和架构文档

没有集成文件时，与旧版本行为一致；旧客户端不会读取新文件，插件说明标出经测试的最低客户端版本。协议变更要求双端 fixture 一起更新；不把一个客户端文件直接交给另一端使用。

CLI 自定义根目录仍隔离，默认描述符的路径限制明确列入安装文档。Desktop 和 CLI 的当前 UA 不在本机首版强制改名；各自公开归因标记区分请求。新增 `magpie-willdeep-cli` 时同步覆盖远程网关账本分类，尤其自定义端口。

实施属于功能新增：CLI 从 `0.141.0-rc3` 升到 `0.142.0-rc1`，Desktop 从 `1.426.0-rc1` 升到 `1.427.0-rc1`（Build 941）；插件单独版本化为 `0.1.0-rc2`。版本文件、CHANGELOG、产品概览和架构/互操作文档已同步。

实施交付须同步各仓版本、CHANGELOG、产品概览和架构/互操作文档。CLI 架构更新到既有 `docs/ARCHITECTURE.md`；Xedit 按项目习惯在本设计及关联文档说明，不新建根架构文件。本轮文档同步仅更新需求、设计、执行台账及 Magpie 入口，其他版本和架构文件由对应实施任务处理。

## 10. 分期与测试计划

| 阶段 | 交付 | 验证 |
| --- | --- | --- |
| P0 | schema + fixture + 两个消费端 + 两个本地可安装插件 | Rust/Swift/JS 单测；本地模拟网关闭环；原配置前后比较 |
| P1 | 安装说明、最低版本、包检查及社区提交材料 | 本地包完整性、图标、说明、中英文摘要、registry 草案 |
| 后续 | 获准后发布包/提交社区；远程凭据、自定义路径便捷配置和 Library 集成另行评估 | 发布目标、版本及社区实际状态单独报告 |

验收详见需求第 8 节及执行台账。API/E2E 使用 Ruby 产生 JSON、Markdown；Rust/Swift/JS 单元测试使用原生框架。真实隔离 Runtime 必须在插件连接之前已经启动，以证明读取不依赖后注入环境；Desktop 验收涵盖启动后连接和 App 前台刷新。

## 11. 非目标与风险

不修改 Magpie 核心、不争取内置条目、不在本阶段自动发布、不把官网静态支持列表当作插件安装结果。MCP/Skills Library、远端共享网关、已有活动会话外部强制切模不在首版。

| 风险 | 对策 |
| --- | --- |
| 临时 Provider 被误保存进主配置 | 磁盘配置和有效目录分离；测试 TOML、UserDefaults、测试钥匙串前后快照比较，不读取真实用户数据 |
| 异常不完整文件或更新竞态 | 同句柄校验与有界异步重读，错误可见；真宿主批量更新与故障注入分开验证 |
| 自定义路径与 Magpie 环境不同 | 明确默认路径；自定义根不回退；提供本地描述符说明 |
| 已有 Profile/UUID 冲突 | 明确报错，保留用户数据 |
| 已有会话误随全局模型变化 | 创建时固定 Provider/模型；恢复和切换用已有会话流程，旧无固定选择会话跳过集成默认 |
| 自定义端口被记为免费本地模型 | 新公开标记及集成来源纳入 Magpie 远程分类回归 |
| 上游插件契约继续变化 | 锁定已验收 Magpie 发行版本和源 revision，升级时重跑安装闭环 |

## 12. 实施边界与验收门槛

D1–D5 已于 2026-11-04 移入第 3 节已确认决策，当前状态为“已实现，本地验收完成”。schema 与原始 fixture 已冻结；CLI、Desktop 与双插件均已完成实现并通过各自的 macOS 本地验收，结果见[验收报告](verification/0.142.0-rc1/magpie-agents/README.md)。

组合验收必须覆盖实际调用、A→B 的旧/新会话隔离、旧无固定选择会话恢复、断开后显式集成会话报错，以及显式其他 Provider/模型/覆盖不受无关 `invalid` 阻断。测试使用临时 HOME、隔离配置和本地 fake upstream，不读取真实配置、凭据或钥匙串，不产生付费调用，不安装或重启用户程序。

后续验收、版本同步与分发材料已按执行台账完成本地部分；未执行或失败的场景如实记录在验收报告，包括 Linux/Windows 原生执行与 Desktop 真实 App 点击组合验收。正式发布和社区提交在本地成品可审查并验收后另行授权，不因“按设计实现”而自动获得发布权限。
