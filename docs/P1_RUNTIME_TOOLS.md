# Runtime 工具：结构查询、语义预览、浏览器与独立搜索

> 开发版本：0.152.0-rc3。本文记录共享 Runtime 已实现的接口、配置和限制。设计来源见 [P1 决策](decisions/2026-11-17-p1-runtime-tools.md)，历史命令、失败修复及未覆盖范围见 [rc1 本地验收](verification/0.151.0-rc1/p1-runtime-tools.md)与 [rc2 整合经过](verification/0.151.0-rc2/p0-p1-closeout.md)，当前整合检查见 [0.152.0-rc3 验收](verification/0.152.0-rc3/p0-p1-closeout.md)与 [JSON](verification/0.152.0-rc3/p0-p1-closeout.json)；本版本尚未安装或发布。

CLI、TUI 和 Web 中的模型使用同一个工具注册表，沿用 Runtime 审批、取消与权限。工具配置存在、依赖路径存在和真实连接可用是三个不同状态。

## 查看状态

`tool_status` 接受空对象以查看全部可选集成，或传入 `tool`，值为 `lsp`、`ast_grep`、`browser`、`web_search`、`mcp`。它不安装依赖、不启动进程、不执行 DNS 或服务连通性检查，也不运行 Hooks、刷新动态工具目录或读取 OAuth 凭据文件；浏览器的在线状态另由 `browser` 的 `action=status` 检查。

五类集成统一返回 `permission.state` 与固定 `reason`，区分策略允许、仍需运行期检查、待申请审批及禁止。配置可用不代表已获动作审批，命令 OS 围栏不自动覆盖宿主 HTTP 或既有 MCP 传输。

MCP 状态只保存配置摘要及已有请求的观察，不保存或回显端点、命令参数、服务名和原始错误；以会话内 `server_index` 标识。尚未尝试连接、已初始化、进程退出、最近请求失败和请求要求授权分别报告。认证状态在没有明确拒绝证据时保持 `unknown`，已有成功请求也不能证明当前凭据仍有效。只读委派与 doctor 使用同一配置投影，不因此连接 MCP。

```json
{"tool":"browser"}
```

只读会话可以查看此配置状态，但不会因此获得启动语言服务、AST 进程或浏览器的权限。`doctor` 使用同一配置状态投影，其可分享报告只保留固定状态和恢复建议，不复制端点、可执行路径、产物路径或原始错误。诊断不会证明 Runtime 的实际执行权限。

## 语义重命名预览

`lsp` 使用固定的本地 `rust-analyzer` 或 `typescript-language-server`，支持 Rust、TypeScript 和 TSX。位置为从零起算的 UTF-16 行列。依赖和查询限制见 [P0 协议](decisions/2026-11-17-p0-coding-tools.md)。

Rust 服务须支持 `experimental/serverStatus`，初始化等待健康且静止的项目状态；请求期间状态修订变化则丢弃结果。首次 VFS 加载所需的 Cargo 元数据重载允许执行，项目构建脚本、过程宏与自动编译检查继续关闭。初始化响应本身不证明项目已准备好；缺少就绪通知、错误、取消与超时均不能产生通过诊断。`status` 仅检查配置，不启动服务或沙箱探测进程。

`operation=rename_preview` 由语言服务计算跨文件编辑，再返回既有 `file_changes` 预览。必须查看差异，再用该回执的 `preview_id` 调用 `file_changes` 的 `operation=apply`。预览不直接写入，语言服务的 `workspace/applyEdit` 始终拒绝。

宿主先发现目标，在最终请求前打开并同步所有目标，保留精确 URI、文本与 SHA-256 快照。版本化 `documentChanges` 必须匹配同步版本；固定 TypeScript 服务原生返回的 `changes` 单独按请求前同步快照绑定，不伪造服务端版本，也不放宽版本化响应的缺失、null 或陈旧版本。Rust 不接受该无版本兼容路径。两种响应均整批校验：未知目标、未同步 URI、资源操作、注解、混合形状、越界、重叠、工作区外路径或快照变化都会拒绝。实际写入仍遵守 [多文件预览与恢复边界](decisions/2026-11-17-p0-coding-tools.md)。

## AST 结构查询

`ast_grep` 提供 `status` 和 `query`，使用预先安装的 `ast-grep`，不使用名称有歧义的 `sg`。可执行文件必须来自工作区外的绝对 PATH 目录；工具不会下载依赖。

```json
{"operation":"query","path":"src","language":"rust","pattern":"println!($$$ARGS)","limit":20}
```

语言为 `rust`、`typescript`、`tsx`。宿主枚举普通源文件、保存 SHA-256 快照，通过标准输入交给固定 CLI；显式隔离配置发现，不接受自定义解析器、任意参数、改写或 Shell 命令。查询沿用命令审批与有效 OS 围栏。响应提供 UTF-8 字节范围、扫描文件数、总命中数、结果是否截断及快照。

结果显示上限与协议硬上限不同：前者可明确返回 `truncated=true`，后者超限则失败。进程结束后重新枚举和复核文本，选中文件集合变化会丢弃结果。隐藏、构建及依赖目录排除在扫描范围外；结构查询不能替代语言服务的类型与符号解析。

## 独立搜索

搜索设置独立于聊天 Provider；密钥仅可通过环境变量引用，不能写入本配置。

```toml
[search]
backend = "searxng"
api_base = "https://search.example.org"
api_key_env = "WILLDEEP_SEARCH_KEY"
allow_private_endpoint = false
```

匿名 SearXNG 可省略 `api_key_env`。环境引用缺失、为空或无法组成合法 HTTP 认证头时，配置解析失败，不借用聊天密钥。`backend="auto"` 保留 SomeIm 搜索兼容；`disabled` 禁用。显式后端失败不切换供应商。

SearXNG 请求为 `GET <api_base>/search?q=…&format=json`，保留配置的路径前缀；已以 `/search` 结尾的地址不重复追加。配置密钥时发送 Bearer 认证。`web_search` 参数仍为 `query` 和可选 `count`，默认 8 条，最多 20 条。空结果、认证失败、拒绝访问、限流、HTTP 错误、畸形响应和响应超限分别返回明确状态或错误。

审批先于 DNS 与网络请求。连接禁代理、禁重定向，使用核验后固定的地址，并保留原主机进行 TLS 校验；公网端点要求 HTTPS。显式本地服务可配置 `allow_private_endpoint=true`，这一开关仅适用于搜索，不改变 `web_fetch` 的公网限制。

响应总量上限 1 MiB，请求和读取共用 30 秒期限；取消会丢弃请求，不自动重试。结果含后端、端点来源、结果来源和截断标记。`web_search_status` 只检查配置，不验证认证是否有效或服务是否在线。SomeIm 兼容调用保持原 POST 协议，不携带聊天会话与工作区头。

## 受控浏览器

浏览器默认关闭，明确配置 Node 24 及 Chrome 的绝对路径。工具不安装浏览器，也不附着用户日常 profile。产物目录必须预先存在。

```toml
[browser]
enabled = true
node_path = "/absolute/path/to/node"
chrome_path = "/absolute/path/to/chrome"
allowed_origins = ["http://127.0.0.1:19851"]
artifact_dir = "/absolute/path/to/browser-artifacts"
timeout_seconds = 30
```

来源必须是精确、规范的 HTTP(S) origin，不包含路径、结尾斜杠、查询、片段或凭据。示例回环地址仅用于本地合成验收，生产配置应替换为需要访问的来源。

固定 `action` 为 `status`、`open`、`snapshot`、`fill`、`click`、`screenshot`、`assert`、`close`。没有任意 JavaScript、上传、下载或用户 profile 参数。`open` 接受 `url`；快照返回宿主生成的元素 `ref`，输入、点击与文本断言使用该引用。页面代次变化或旧引用失效时应重新快照，不能猜引用。

```json
{"action":"open","url":"http://127.0.0.1:19851"}
```

```json
{"action":"assert","assertion":"text_contains","ref":"<snapshot-ref>","expected":"Saved"}
```

导航、输入、点击按具体动作进入审批；等待审批后重新核对权限和页面代次。固定 DOM 操作在独立执行环境中解析节点，页面修改自身的 DOM 方法不能替代宿主的点击命中检查。`status` 实际探测 Chrome 与页面 CDP，挂起或断连不能继续报告 ready。

截图使用宿主生成路径，返回 PNG 的哈希、MIME 和尺寸；截图成功不代表业务断言通过。Unix 通过持有的目录句柄创建与清理文件，目录身份变化拒绝提交；写入和同步在阻塞线程中执行，取消后清理尚未提交的命名产物。其他平台目前拒绝截图落盘，不使用可被替换父目录绕过的路径写入降级。

启动时，端口发现、WebSocket 连接和 CDP 初始化共享从 `timeout_seconds` 派生的单调时间预算；总超时预留四分之一且最多 1 秒用于返回诊断。Chrome 标准错误持续排空，仅保存至多 8 KiB 启动前缀并转成固定原因标签；错误区分提前退出与超时，不回显原始日志、路径、环境值或远程地址。完成启动后清除捕获内容，继续排空管道。

超时或取消销毁所属桥和 Chrome 进程树，清理不依赖调用方 Tokio Runtime 继续运行。切到只读后，父 Registry 仍可用严格无附加字段的 `close` 释放自己已有的浏览器；这不开放其他动作、Worker 或委派会话。缺失回执的副作用不自动重试。

浏览器当前要求有效 OS 围栏为关闭状态；不能保留强制 OS 围栏时明确拒绝启动。CDP 请求拦截、页面 CSP 与 Chrome 启动限制控制导航和页面请求，不构成 Chrome 全进程网络隔离。浏览器暂不向 Worker 开放。

## 项目模型策略

项目可在 `.willdeep/model-policy.json` 声明允许的真实调用组合。文件不含密钥；`provider` 为 `openai-compatible`、`some-im` 或 `anthropic`，模型 ID 精确匹配。每条规则必须且只能选择 `origin` 或 `base_url`：前者允许该来源上的内置 Provider 路由，后者要求精确配置的 API base，不是任意 URL 前缀。

```json
{
  "version": 1,
  "allow": [
    {
      "provider": "openai-compatible",
      "model": "project-model",
      "base_url": "https://api.example.org/v1"
    }
  ]
}
```

首版限制 64 KiB、256 条规则，不支持通配符、未知字段、带凭据的端点或符号链接策略文件。缺文件的旧项目兼容原行为；空 `allow` 拒绝发送。策略存在但非法、不可读，或启用后被删除，均拒绝请求，不降级为无限制。

同一进程按规范项目路径共享首次生效的可信允许集。每次实际发送重新读取文件，与已生效集合取交集；Agent 修改文件、重建 Provider 或恢复旧文件都不能扩大当前进程的权限。需要扩大允许范围时，宿主应先审查策略，再重启所属 Runtime；持久文件本身仍属于宿主信任边界。

门禁检查最终 wire model，约束聊天、流式、重试、目录与能力探测、Worker、压缩、辅助模型和回退。完整子会话、恢复和 worktree 切换保留持久项目归属，不能用临时执行目录替换。启用策略时，无法预知模型的本机自动发现会在读取配置或联网前跳过；无策略项目保留原行为。图片生成、上传及下载端点分别授权，下载非 2xx 响应不能创建媒体文件或报成功。拒绝返回 `PolicyDenied`，不自动重试，也不伪造 usage；审计仅保留固定原因码和 `not_sent`。启用策略的 HTTP 连接禁代理、禁重定向，并在固定 DNS 地址后再次授权。这只约束接入的 Provider HTTP 出口，不能约束任意外部进程、插件自行联网或代替 OS 网络隔离。各调用入口的最终覆盖以本轮验收报告为准。

## Goal 完成证据

清单项的 `evidence` 仅说明记录与验收标准的关系。模型必须引用真实执行返回的 `evidence_ref`；自由文本、未知 ID、失败退出、运行中的任务、其他会话或其他 Goal 的记录不能证明完成。

宿主记录会话、工作区、Goal、执行尝试、来源、退出状态及起止代码快照。勾选和最终完成时均重新核验；执行期间或其后快照范围内的代码改变、后续同项执行失败或记录缺失，都会使旧证据失效。后台启动回执不等于测试成功。生产 CLI 复用 Git Diff 快照，覆盖 HEAD、暂存/未暂存及未跟踪内容，不证明 Git 忽略文件、外部依赖或外部服务保持不变；非 Git 工作区缺少此快照时不能生成有效成功证据。该快照是内容指纹，不将其宣称为 SHA-256 或对外部写者的 CAS。

人工观察仅由宿主 API 记录并标记来源，模型工具不能冒认人工。恢复后的旧文字和旧已勾选项不自动升级为有效记录；重启丢失进程内证据后，需要重新验证。失效清单可逐项重新取得引用，其他旧引用仍保持未完成，最终完成会核验整单。新 Goal 按独立身份计预算，同名文本不沿用已结束目标的消耗；同一身份的并发状态保留较大计数。

已知的收集、列举、帮助与 dry-run 参数不能签发通过证据。记录证明宿主实际运行了该命令、观察到相应退出状态和快照；它不能证明任意项目脚本内部确实执行了足够的断言，也不能判定测试是否覆盖全部需求。执行证据、预算和完成合同分别约束自身职责，彼此不替代。
