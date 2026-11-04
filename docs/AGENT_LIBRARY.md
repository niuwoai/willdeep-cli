# 专业智能体库

> 最后更新：2026-11-04 | 当前版本：v0.142.0-rc1

交换格式仍为 v1，见 [智能体契约](AGENT_DEFINITION_V1.md)。本轮增加对话管理与本机安装事务，不改交换包格式。

## 使用

Web 的“导入 Hermes / Multica”支持 Hermes profile 的 `config.yaml` + `SOUL.md`，以及 Multica Agent API 返回的单个 JSON 对象。填写独立的 `user/` ID，预览名称、指令，再选择本机 Provider 与模型后保存。来源 Provider 名称不等于本机 profile ID，必须显式选择。

此转换兼容身份、指令和模型引用；Hermes 还支持上下文窗口与轮次设置。未转换字段在预览列出；工具、技能、MCP、环境变量、访问控制和外部 Runtime 需要在宿主另外配置，凭据值不会进入转换结果。默认以只读 reviewer 职责创建。导入只回填文本能力，不会根据模型名字断言图片或视频能力。原生 WillDeep JSON 包仍走原有导入入口。

`willdeep agents create user/team-reviewer 审查员 代码审查 --role reviewer --instructions-file instructions.md` 创建本地定义。`willdeep agents import agent.json` 支持完整交换包或定义；`check` 验证而不安装；`export ID agent.json` 导出完整版本包。Web 一级入口“智能体库”提供 JSON 编辑、导入导出及全部安装选择操作，TUI `/agents` 提供列表、目录、同步、fork、固定及回滚。

编辑使用新版本。官方定义须 fork 为 `user/`，不能在官方名称下改指令。内嵌技能正文的 SHA-256 必须匹配，不会被本地同名技能替换；必需正文缺失拒绝导入，可选缺失显示降级。用户图片定义可保留及交换，但当前 Rust 子 Worker 暂不支持图片输入，导入后停用并显示不兼容原因。官方候选在切换安装选择前验证执行兼容性；不兼容时拒绝安装并保留原 hash、启用状态、固定状态与本地模型绑定。

CLI create 默认仅请求读取能力；实现者若需编辑，应显式重复传入 `--tool workspace.read --tool workspace.edit --tool process.verify`。请求仍受基础职责和本次精确写集合限制。Web 的完整定义编辑器也可配置这些能力。

`willdeep agents bind ID --provider-profile NAME --model MODEL` 为本机安装绑定已有 Provider 和模型；`unbind ID` 恢复沿用基础职责模型。Web“本地模型绑定”从现有模型配置列出 Provider，并调用 `/v1/models` 对应的发现入口列出模型，保留当前值；绑定字段只存在 Installation，不进入定义 hash、导出包或私有同步。明确的本地绑定优先于常规模型档位路由，职责、工具及写集合边界保持不变。

`willdeep agents catalog` 获取官方目录；`install official/ID HASH` 验证并安装指定 hash，禁止自动使用 latest。Web 先查看候选与当前定义，再安装。`pin ID` 固定当前版本；`unpin ID` 允许升级；`history ID` 显示本地历史；`rollback ID HASH` 切换并固定旧版本；`disable ID` 停用。

`willdeep agents login` 使用 some.im 现有浏览器登录（客户端登录凭据从环境变量读取），只保存独立 customer session，不更改模型 Provider。旧的纯推理 API Key 无法访问私有定义。登录后 `share user/ID` 显式加入当前账号范围，`sync` 上传并下载；另一客户端登录相同账号、相同租户后主动同步即可使用同一版本。`logout` 退出服务账号，保留隔离缓存以便后续重登，当前列表与新任务不会加载私有定义。

已加入私有同步的定义可通过 import 或 Web 编辑更新。客户端保存旧远程 etag，PUT 使用 If-Match；新建使用 If-None-Match。请求幂等标识由账号、方法、资源、revision 与远程基线稳定生成。网络结果不确定时再次同步使用相同请求标识。冲突列在同步报告 `conflicts`，不会覆盖本地；可先 export/fork 保留本地修改，并在新版本中人工合并后再次同步。此版本暂不提供自动合并。

`delete ID` 删除本地安装选择；当前账号私有定义同时用原 etag 删除云端，服务保存 tombstone。另一端未修改的安装在同步时移除；有修改或 pin 的本地定义保留并报告冲突。历史内容包保留，显式固定/回滚不会改写包正文。

## 对话内管理（0.141.0-rc1）

父智能体可在正常对话中读取定义并管理本机安装；基础职责仍是权限上限，定制基础职责时创建独立 `user/` 定义。

| 工具 | operation | params 要点 |
| --- | --- | --- |
| `agent_library_read` | `list` / `get` / `history` | get/history 用 `id`；get 返回定义和当前安装修订 |
| `agent_library_manage` | `create` | `definition`，可选本机 `binding`；目标 ID 必须不存在 |
| 同上 | `update` | `definition`、`expected_revision`；正文版本严格递增 |
| 同上 | `fork` | 源 `id`、新 `new_id`、源安装的 `expected_revision` |
| 同上 | `bind` | `id`、`binding`、`expected_revision`；空 binding 恢复继承 |
| 同上 | `enable` | `id`、布尔 `value`、`expected_revision` |

这些工具只注入父会话，不提供给 Worker。写操作走既有授权和审计；工作区只读策略拒绝写。它们不会自动分享、同步上传定义，也不读取或修改 Provider 密钥。运行时为聊天写操作补充稳定 request ID，并使用宿主捕获的 scope，模型不能自行选择账号或存储目录。

### 两类修订与原子收据

- `definition_revision` 是规范化包内容的 SHA；本机绑定和启停不会改变它。
- `installation_revision` 是安装状态的递增整数，正文选择、绑定、启停等变更都会推进；写请求的 `expected_revision` 必须填刚读取的此值，不能填正文 hash。
- 创建只接受不存在的目标。更新已有安装缺少前置修订会明确拒绝；官方正文需先 fork，显式回滚使用已有 rollback 操作和历史 SHA，不能伪装成普通保存。
- CLI、Web、对话管理共用存储事务。创建检查、scope 校验、CAS、request ID 收据与安装选择在同一锁内提交；同 request ID 同参数返回 `replayed: true`，同 ID 异参返回冲突。账号或库作用域变化后先重新读取，不跨账号重放旧请求。

读结果带账户 `scope` 和目标安装库身份（list 的 `scope: local/private`，get/写回执的 `target_scope`）；原始 Runtime/Web 客户端在管理写请求中带回 `expected_scope` 与读取到的 `target_scope`，宿主会在事务内校验当前身份。CLI 适配器在提交前捕获 scope 和安装代数。聊天宿主固定会话账户，并记录 list/get 观测到的目标库和修订，自动补齐、校验 `target_scope`；未读过的旧目标须先读取，fork 固定源安装，create 无需旧目标。相同修订号的 private 安装后来遮蔽 local 时，旧写入会拒绝并要求重读。相同 request ID 使用首次注入的目标，历史重放不倒退当前观测。写回执区分 `saved`、`definition_revision`、`installation_revision`、`replayed`。聊天回执额外返回 `applied` / `catalog_refreshed`；刷新失败时为 `saved_not_applied`，定义已经保存，可用同参数及同 request ID 重试刷新，不能当成保存失败另建版本。

重复 import 完全相同的包时保留原 pin、启停、绑定及来源设置；更改绑定须显式 bind。普通 update 仍要求正文版本严格递增，不能用相同内容绕过 pin 或版本约束。

旧的平面安装记录兼容只读加载，缺失代数读取为 1；首次成功写入才在锁内迁移为 `willdeep.agent-installations.v2`，包含 `installations`、clock 和 receipts。本机状态格式升级不改变智能体交换包 v1、签名和 hash 计算。

### 同轮新派工与在途快照

聊天保存成功并刷新后，当前父会话原子替换专业目录，保留基础职责；之后发起的新派工可立即使用新 ID/正文/绑定。已在途、排队及续做/恢复的 Worker 保留派工时原快照，不因新正文、停用或新绑定改写运行。此保证针对当前宿主后续派工，不表示所有已经构建的其他会话目录都会自动热刷新。

## 权限与实际执行

`spawn_agent.profile` 使用稳定字符串 Schema，父智能体通过 `agent_library_read.list/get` 获取可用 ID、适用说明及当前修订；不在运行开始时把专业 ID 固定为枚举。派工后才注入完整专业指令和锁定技能。Web“试运行”把选定定义和任务填入对话，用户发送后由正常父智能体派工与审批链执行。

定义只请求 workspace.read / workspace.edit / process.verify / repository.inspect，运行时与基础职责工具取交集。写入仍要求原有精确文件集合及审批；预算、Shell 策略、Provider 凭据和沙箱保持宿主控制。保存成功并刷新目录后影响当前父会话后续新派工，不改变在途实例；持久派工记录包含完整定义、正文 revision、技能与不含凭据的 Provider/model 和执行限制快照。续做/恢复从原快照还原，即使最新版定义被停用或删除，也不静默改用新正文或新绑定；当前宿主权限、基础职责和预算仍重新核验。没有可靠执行绑定快照的旧专业派工明确阻止恢复，要求重新派工。

## Runtime 与存储

Web 与 TUI 共用鉴权 Runtime `agent_library.*` 操作；能力协商列出管理操作。Web 只允许回环监听下管理。API 保持既有版本响应头。

库位于 `$WILLDEEP_HOME/agents`，版本以 hash 存储，安装选择单独保存。私有目录由 origin / user / tenant 组合 hash 分隔。Session 文件使用 0600，写入使用临时文件和原子 rename，库修改持有文件锁。同步报告与导出不返回 session token、模型 Key 或宿主授权记录。

首次官方下载只信任用户指定的 HTTPS origin（开发测试可使用 loopback HTTP）；所有 redirect 均拒绝。签名密钥来自该服务 origin 并缓存。JSON 不接受重复键、未知字段、数字与缺失的 nullable 必填字段；大小上限 256 KiB，官方包 hash、ID、签名都必须匹配。三端共用包含中文、emoji、引号、反斜杠的 golden 样本。

服务轮换签名密钥时使用独立 key ID；同 origin 缓存合并新旧 ID，保留旧版包的离线验签能力。同一 key ID 对应不同公钥会拒绝并保留原缓存；切换 origin 后缓存独立替换，不混合其他服务的可信密钥。

## 本机执行配置

Web 智能体库的模型绑定面板可设置 `context_window`（1024–2000000 Token）与 `max_turns`（1–1000）。留空继承职责/Worker 默认值。数值保存在配置目录 `agents/installations.json` 对应安装项的 `binding` 中（迁移后的文件以 `installations` 收纳安装项），不进入签名定义包或云端分享内容。窗口应按所选模型真实能力填写，不代表增加模型容量。

CLI 示例：`willdeep agents bind user/reviewer --provider-profile my-provider --model my-model --context-window 32768 --max-turns 12`。绑定操作替换整组配置，省略字段恢复默认。后续新派工应用新绑定；已有任务的在途、续做与恢复沿用原派工绑定及预算快照，当前权限和团队预算重新核验。

Hermes 导入支持 `model.context_length` 和 `agent.max_turns`，在确认面板中可修改。非整数、零值和超界值会拒绝导入；未映射的其他配置仍只显示字段名称。

## 本轮实现与验收

本轮实现对话宿主、共享安装事务、稳定 profile 读取与目录快照更新，覆盖 CLI/Web/对话写入及旧安装记录读取。Rust workspace 测试与 Web 测试已执行通过；相关测试位于 `agent_registry/management_tests.rs`、`harness/agent_library/tests.rs`、core `agent_definition/store/`、`subagent/catalog/snapshot_tests.rs`、`tools/host_tools/` 和 Web `AgentLibrary.manage.test.tsx`。

原生测试覆盖创建竞争、正文/绑定/启停 CAS、同 request 异参、账号切换、local/private 同名同修订遮蔽、官方先 fork、保存成功但刷新失败的安全重试。Ruby `scripts/verify_workflow_runtime.rb` 使用隔离 WILLDEEP_HOME、临时工作区和本机假模型，实际完成“对话创建→同轮新派工→更新/绑定→新旧 Worker 使用各自快照”，并验证工作流崩溃后显式恢复仍使用旧定义与模型，普通新派工使用新目录。报告输出 JSON + Markdown；完整命令、结果及平台边界见 [验收报告](verification/0.141.0-rc1.md)。
