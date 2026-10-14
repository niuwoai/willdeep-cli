# 智能体交换契约 v1

本契约覆盖 P0—P3；不包含 RSI 自动修改、长期记忆上传、团队共享或自动扩权。三个客户端共用 JSON，定义包不包含 Provider Key、本地绝对路径、Shell 安装命令或授权规则。

## 定义

`schema` 固定为 `willdeep.agent-definition.v1`。字段全部必填：

| 字段 | 类型与约束 |
|---|---|
| definition_id | 1—96 字符，`[a-z0-9][a-z0-9._/-]*`，不含 `..`；官方为 official/，用户为 user/ |
| version | 语义版本，允许 rcN；同 ID+版本内容不可变 |
| name | locale → 展示名，最多 16 项，locale 为 1—32 位 ASCII 字母、数字或连字符；至少 en 或 zh-Hans，单项非空且最多 128 个 Unicode scalar |
| purpose | 适用任务，1—1024 字符 |
| base_role | generalist / implementer / tester / reviewer / ops_runner |
| instructions | 专业指令，1—16000 字符 |
| input_modalities | 非空且不重复，text / image；不兼容宿主必须报错 |
| tools | 不重复能力 ID；workspace.read / workspace.edit / process.verify / repository.inspect |
| skills | 最多 8 项；id、content、sha256、required，content 为 UTF-8 技能正文，单项最多 24000 字符 |
| forked_from | null 或原始 revision 的 SHA-256 |

v1 不接受未知字段和重复 JSON 对象键，不接受数字。总包上限 256 KiB。技能 sha256 对 UTF-8 原文取 hash；不使用本地同名技能替换版本包正文。必需正文为空视为缺依赖；可选正文为空允许但需显示降级。

## 内容身份与签名

交换包为 `{ "definition": <定义>, "sha256": <64 位小写 hex>, "signature": null | { "key_id": <标识>, "value": <128 位小写 hex> } }`。

revision_id 等于 sha256。只对 definition 规范化字节取 SHA-256，避免把 hash 包含在自身输入里。规范化规则：

- 对象键按 UTF-8 字节升序；数组保持原顺序。
- 不输出空白；null/true/false 使用 JSON 字面量。
- 字符串先按 UTF-16 code unit 编码；可打印 ASCII 原样输出，但引号和反斜杠转义；其余字符输出小写 `\uXXXX`，非 BMP 输出两个 surrogate。
- 没有数字、重复键或未知字段；UTF-8 必须有效；不进行 Unicode normalization。

官方包使用 Ed25519，对 ASCII `willdeep.agent-package.v1\n` 加 64 字符 sha256 签名。包不携带可自我授信的公钥；公钥由用户信任的服务 HTTPS origin `/api/v1/public/agents/keys` 提供并缓存在本机。自定义本地/私有包 signature 为 null，不能标成官方。

只有固定、可信 service origin 能发布官方包；拒绝非 HTTPS，开发测试只允许 loopback HTTP；禁用跨 origin redirect。服务签名私钥由环境配置 `SOMEAPI_AGENT_SIGNING_KEY` 提供（Ed25519 32-byte seed hex），缺少时官方签名下载明确不可用，不生成临时线上密钥。

当前服务 `key_id` 为 `someim-agents-v1-` 加公钥原始字节 SHA-256 的前 16 位 hex；轮换生成独立 ID，固定定义 hash 不变。固定包下载由当前 key 重新签名，keys/package 响应 no-store。客户端只合并同 origin 的独立 key ID，拒绝同 ID 不同公钥，不混合不同 origin 信任；已信任旧键可用于离线固定版本。目录撤回与停止旧键签发不承诺远程强制删除缓存。

## 服务 API

路径统一带 `/api/v1`；服务端保留其既有 data envelope，以下业务 data 格式固定。

| 方法与路径 | data |
|---|---|
| GET /public/agents/index | `{items:[{definition_id,version,sha256,name,purpose,base_role}]}` |
| GET /public/agents/keys | `{keys:{key_id:public_key_hex}}` |
| GET /public/agents/package?definition_id=...&revision=... | 包；revision 必须为固定 hash，禁止静默返回 latest |
| GET /customer/agent-identity | `{user_id,tenant_id}`；确认独立 customer session 及仍有效的明确成员关系 |
| GET /customer/agents | `{items:[{definition_id,sha256,version,deleted,etag,package}],cursor}` |
| GET /customer/agents/history?definition_id=... | 当前账号定义的不可变历史版本 |
| GET /customer/agents/package?definition_id=...&revision=... | 当前账号的固定历史包；与列表共用 ACL |
| PUT /customer/agents?definition_id=... | 上传完整包（unsigned）；首次 If-None-Match:*，后续 If-Match:etag；Idempotency-Key 必填 |
| DELETE /customer/agents?definition_id=... | tombstone；If-Match 与 Idempotency-Key 必填 |
| GET /admin/agents | 官方历史包、当前目录指针与 signing_configured；仅真实且有效的运营管理员会话 |
| POST /admin/agents | unsigned official 包；默认只读预览，`?confirm=true` 加 If-Match / If-None-Match:* 才发布不可变版本并更新指针 |
| POST /admin/agents/select?definition_id=...&revision=...&active=... | 已归档固定版本的回滚/目录撤回预览；`confirm=true` 加并发前提才改变指针并记录审计 |

首期以有上限的账号全量同步实现，cursor 为服务结果版本标识。用户主动同步，按明确 customer session + user + tenant 隔离；无明确用户归属的 API Key 不能访问私有资产。读取和下载共用 ACL。版本包不可变，删除保留 tombstone；恢复要求基于 tombstone etag 写入。409/412 冲突不覆盖任何本地修改。

私有响应 `Cache-Control:no-store`，统一 X-App-Version 与 request ID。服务端错误不返回正文/凭据。Idempotency-Key 按账号、方法和资源隔离，重用 key 的请求内容不同则 409。

官方正文与目录指针分开。发布后同一 ID+version 不可改写，回滚只改变指针，撤回目录不删除历史固定包；已经安装的客户端不会自动换版本。管理员发布不接收签名私钥，签名只在服务端配置环境中产生，发布和指针变更在同一事务中记录 actor 审计。

## 安装与执行

本地库放在 `$WILLDEEP_HOME/agents`，revision 包按 hash 保存，安装选择单独保存。Mac 导入导出同一包，不直接与 Rust 同时写 UserDefaults。原子写入，当前安装保持在候选验证成功前的版本；pin/rollback 只切换选择，不修改版本正文。

来源：builtin / local / official / private；来源来自安装上下文与验签，不信任用户可编辑的名称或 definition_id。官方指令修改必须 fork 到 user/ ID。服务账户退出后停止私有同步，缓存按账号隔离。

定义 ID 与权限职责分开。有效权限为宿主 ∩ base_role ∩ tools 请求 ∩ 本次授权。实现者仍需既有精确写集合；不能通过自定义 ID、签名或修改 JSON 增权。模型继续使用本地 Provider 绑定。工具缺失、模型不支持模态、required skill 缺失应明确不可运行。

本期两端 Worker 对超出基础职责的工具请求取交集，不授予额外能力；有效工具集合为空或包含尚不支持的图片输入时不可运行，但用户定义仍可保存、导出和同步并显示不兼容状态。官方更新先检查兼容性，失败保留已有正常安装选择。模型绑定、pin 和启用状态属于本机安装；同步正文不会覆盖这些选择，初次加入私有同步保留本机模型绑定。

派工记录固定 revision/hash、实际专业指令和技能版本。更新安装不改变正在运行实例的快照。只把定义简短索引放到父 Agent 工具目录，选定后再注入正文。
