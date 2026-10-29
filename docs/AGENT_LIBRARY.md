# 专业智能体库

版本：0.131.0-rc1。交换格式见 [智能体契约](AGENT_DEFINITION_V1.md)。

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

## 权限与实际执行

父智能体的 spawn_agent schema 中列出可用定义 ID 与简短适用说明；派工后才注入完整专业指令和锁定技能。Web“试运行”把选定定义和任务填入对话，用户发送后由正常父智能体派工与审批链执行。

定义只请求 workspace.read / workspace.edit / process.verify / repository.inspect，运行时与基础职责工具取交集。写入仍要求原有精确文件集合及审批；预算、Shell 策略、Provider 凭据和沙箱保持宿主控制。更新安装影响后续构建的派工目录，不改变运行中的实例；派工持久记录包含实际完整定义、revision（由规范化正文计算）及技能正文。恢复使用该快照重新与当前宿主基础职责取交集。

## Runtime 与存储

Web 与 TUI 共用鉴权 Runtime `agent_library.*` 操作；能力协商列出管理操作。Web 只允许回环监听下管理。API 保持既有版本响应头。

库位于 `$WILLDEEP_HOME/agents`，版本以 hash 存储，安装选择单独保存。私有目录由 origin / user / tenant 组合 hash 分隔。Session 文件使用 0600，写入使用临时文件和原子 rename，库修改持有文件锁。同步报告与导出不返回 session token、模型 Key 或宿主授权记录。

首次官方下载只信任用户指定的 HTTPS origin（开发测试可使用 loopback HTTP）；所有 redirect 均拒绝。签名密钥来自该服务 origin 并缓存。JSON 不接受重复键、未知字段、数字与缺失的 nullable 必填字段；大小上限 256 KiB，官方包 hash、ID、签名都必须匹配。三端共用包含中文、emoji、引号、反斜杠的 golden 样本。

服务轮换签名密钥时使用独立 key ID；同 origin 缓存合并新旧 ID，保留旧版包的离线验签能力。同一 key ID 对应不同公钥会拒绝并保留原缓存；切换 origin 后缓存独立替换，不混合其他服务的可信密钥。

## 本机执行配置

Web 智能体库的模型绑定面板可设置 `context_window`（1024–2000000 Token）与 `max_turns`（1–1000）。留空继承职责/Worker 默认值。数值保存在配置目录 `agents/installations.json` 的 `binding` 中，不进入签名定义包或云端分享内容。窗口应按所选模型真实能力填写，不代表增加模型容量。

CLI 示例：`willdeep agents bind user/reviewer --provider-profile my-provider --model my-model --context-window 32768 --max-turns 12`。绑定操作替换整组配置，省略字段恢复默认。新派工及恢复派工会应用明确的本机预算，已有运行中的任务不被即时修改。

Hermes 导入支持 `model.context_length` 和 `agent.max_turns`，在确认面板中可修改。非整数、零值和超界值会拒绝导入；未映射的其他配置仍只显示字段名称。
