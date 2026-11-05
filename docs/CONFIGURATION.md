# 配置指南

## 配置文件位置

默认配置文件为 `~/.willdeep/config.toml`。两种改变方式：

- 设置 `WILLDEEP_HOME` 改变整个配置目录；
- 用 `--config /path/to/config.toml` 指定单个文件。

`--config` 必须放在子命令**之前**：

```bash
willdeep --config ./config.toml config check
```

首次运行且没有配置时会自动进入交互式设置，也可随时执行 `willdeep --onboarding`。生成的配置在 Unix 上自动设为 `0600`。

## 完整示例

完整模板见仓库根目录的 [config.example.toml](../config.example.toml)。

```toml
version = 1
default_provider = "some-im"

[agent]
max_turns = 200
# Optional per-run wall clock limit; omit for unlimited runs.
# time_budget_seconds = 1200
# Explicit project-native checks are accepted only as these exact commands.
# verification_commands = ["python tests/runtests.py queries", "ruby tools/checks.rb"]
# run_command supports a confined workspace-relative cwd. Root configured
# checks must still run at the root; a subdirectory result cannot replace them.
# For repair tasks, optionally require an implementation change and preserve
# existing tests. Configure this contract only for tasks needing these rules.
# [agent.completion_contract]
# require_changed_globs = ["src/**"]
# preserve_existing_globs = ["tests/**"]
approval = "smart"
language = "zh-CN"      # zh-CN | en | ja
small_model_routing = true
auto_dispatch_read_only = true
max_deep_calls_per_harness = 1
auto_title = true

[local_model]
enabled = false
base_url = "http://127.0.0.1:11434/v1"
summary_model = "gemma4:e4b-it-qat"
prefer_for_titles = true
prefer_for_worker_routing = true

[providers.some-im]
provider = "some-im"
api = "chat-completions"
api_base = "https://some.im/v1"
api_key_env = "SOMEIM_API_KEY"
model = "glm-5"
vision_model = "qwen3-vl-plus"
context_window = 131072

[providers.openai]
provider = "openai-compatible"
api = "responses"
api_base = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
model = "gpt-5"

[providers.anthropic]
provider = "anthropic"
api = "anthropic-messages"
api_base = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-sonnet-4-5"
max_output_tokens = 16384
```

选择 Provider：

```bash
willdeep --profile anthropic --workspace . "检查当前项目"
```

如果只配置了一个 Provider，可以省略 `default_provider`，WillDeep 会自动选择唯一项。

## `[agent]` 段

| 键 | 说明 |
|---|---|
| `max_turns` | 模型/工具轮次上限，1–1000，不写时 200；`--max-turns` 优先。触顶交出部分结果，不判失败 |
| `token_budget` | 主 Agent 一轮的 token 预算（输入 + 输出累计），1000–10000000；用尽时交出部分结果与交接信息，不判失败。不写不限 |
| `sandbox` | OS 级围栏（macOS Seatbelt / Linux bubblewrap）。不写：有后端就开；`true`：没后端时命令拒绝启动；`false`：关。见 [OS 级围栏](SANDBOX.md) |
| `sandbox_writable_roots` | 围栏开着时额外放行的写入根，支持 `~/…`，必须是绝对路径 |
| `sandbox_toolchain_caches` | 是否默认放行 `~/.cargo/registry`、`~/.npm`、`~/.cache` 等工具链缓存，不写按放行 |
| `sandbox_network` | 网络围栏：不写按档位（`read-only` 与 `workspace-write` 断，`strict` / `smart` 通）；`"deny"` 全断（需要联网的命令由模型用 `network: true` 重试、由你放行），`"allow"` 全通 |
| `approval` | `strict` / `smart` / `workspace-write`（默认）/ `full-access`；TUI 中 `/permissions default <档位>` 可写回，见 [审批与自动化](APPROVALS.md) |
| `language` | 界面语言 `zh-CN` / `en` / `ja` |
| `small_model_routing` | Runtime 小模型优先路由；默认 `true` |
| `auto_dispatch_read_only` | 自动把高置信度定位、阅读、日志、Git 追溯派给窄工种；默认 `true` |
| `max_deep_calls_per_harness` | 每个 Harness 允许的 1M Deep 升级次数，`0..16`，默认 `1` |
| `auto_title` | 自动整理会话标题；默认 `true`。关掉后标题停在第一条提示词的确定性派生，见 [会话标题](TUI_GUIDE.md#会话标题怎么来的) |
| `title_model` | 标题摘要模型；默认取会话模型。请求只发一问一答各 800 字，与对话长度无关 |
| `input_suggestions` | 轮次结束后在空输入框里灰字预测你的下一句，`Tab` 采用；默认 `true`。用与标题摘要相同的模型候选，开关独立于 `auto_title`，见 [下一句预测](TUI_GUIDE.md#轮次结束后的下一句预测) |
| `compressor_model` | 上下文压缩模型。some.im 会话默认走网关托管压缩（固定指令由服务端注入），其它 Provider 默认取会话模型 |
| `compressor_profile` | 压缩专用的 Provider 档案名（`[providers.<id>]`）。设了之后压缩整套换到该档案的端点、协议、密钥、模型与窗口，主模型仍留在会话 Provider 上——云端主模型配本机压缩器靠的就是这一项。`compressor_model` 仍可覆盖档案里的模型名 |
| `compressor_context_window` | 摘要器一次能吃下的原料上限所用的窗口。不写取压缩档案的 `context_window`，再没有就沿用会话窗口。本地模型窗口远小于云端会话时必须写 |

压缩失败会依次落到下一个候选，最后兜底会话模型；`compressor_profile` 指向不存在的档案、窗口超出 `4000..=1048576` 都会在启动时校验报错。压缩链本身**不**读 `[local_model]`：那一段的模型做坏了摘要不可逆，所以本地压缩必须显式配一个档案。

## 在聊天中修改压缩配置

默认父聊天提供两个内置工具 `configuration_read` 与 `configuration_write`。可以直接说“查看当前压缩配置”，或“把压缩档案改成 summary、模型改成 my-summary、窗口设为 32768，先预览再保存”。不需要安装、启用或审批 `willdeep-config` 插件。

首版聊天允许编辑的范围是 `[agent]` 中的 `compressor_model`、`compressor_profile`、`compressor_context_window`，支持设置与取消设置。Provider 档案的创建、端点和凭据维护仍通过配置工作台或手动配置完成；聊天只能查看档案名、模型和窗口，无法读取密钥。

| 步骤 | 工具及参数 | 行为 |
|---|---|---|
| 查看 | `configuration_read`，`operation: "schema"` 或 `"snapshot"` | 返回受限 schema / 结构化字段，不返回原始 TOML、注释、未知节或密钥预览 |
| 预览 | `configuration_read`，`operation: "plan"`，`params: {"edits": [...]}` | 校验整份候选及压缩 Provider 构建，返回变更和 `plan_id`，不保存 |
| 保存 | `configuration_write`，`operation: "apply"`，`params: {"plan_id": "..."}` | 按现有权限审批，再备份与原子替换。不能另传路径、edits 或 confirm |

`edits` 元素形如 `{"op":"set","section":"agent","key":"compressor_context_window","value":32768}`；取消设置用 `"op":"unset"` 并省略 value。档案必须存在且能构建，窗口必须在 `4000..=1048576` 内。没有有效预览、跨会话计划、旧计划或非法参数都不能保存。每个宿主实例只保留最新的成功预览；新一轮需重新预览。

目标由宿主绑定为本次实际读取的配置文件，遵守 `--config` 和 `WILLDEEP_HOME`；相对路径按加载时工作目录解析，现有符号链接固定到已读取的目标文件。模型无法用参数改写目标。配置服务在同一文件锁中检查预览 revision 并执行保存；其它配置服务保存导致版本变化时，必须重新预览。普通外部编辑器不参与该锁协议，应避免与正在保存的操作同时写文件。

只读模式允许查看与预览，禁止保存。strict、smart、workspace-write 的全局配置写入均需现有审批；full-access 沿用其已有授权。Worker、委派子会话及限制 MCP 服务的任务不获得全局配置工具。API 返回的 `confirm` 不能代替宿主审批；提交回执丢失时不自动重放，应先查看文件再重新预览。备份写入 `$WILLDEEP_HOME/plugin-data/willdeep-config/backups`。

生效时机：

- **普通 CLI / TUI / Web Runtime 聊天**：下一次新 Turn 在开始执行时重新读取配置；已经执行中的 Turn 保留原设置，队列中的 Turn 以实际开始执行时的配置为准。
- **TUI 手动 `/compress`**：空闲时重新加载 TUI 当前配置来源，先构建完整候选和窗口，再取得会话执行权。非法配置会显示错误并继续交互，修正后可重试；运行中不接受该命令。
- **TUI `/local`**：仍复用启动 Agent；`/compress` 成功加载配置、构建设置并取得会话执行权后，即应用新的压缩设置，之后即使摘要请求失败或取消也保留该设置。后续 `/local` 压缩会复用它，其它参数保持启动快照。需要完整刷新时使用默认 Runtime 路径或重启客户端。

聊天配置服务按需启动，需要本机已有 Ruby；缺失时仅该配置能力不可用，不阻止普通聊天启动。私有 stdio 服务与 Web 配置工作台复用发布资产，不借用个人 Web/Daemon 端口。配置写入不意味着网络端点可达：保存前验证配置语义和 Provider 构建，不发送模型请求。

现有配置编辑器使用受限 TOML 解析器，不支持三引号多行字符串等完整 TOML 语法；遇到此类文件会安全拒绝预览，不生成可提交计划。请先手动将配置改为编辑器支持的写法，例如单行字符串或环境变量引用，再重试。多个配置共用备份目录时，目录锁保证备份名称分配和写入不互相覆盖；锁文件不属于备份列表。

## `[local_model]` 段

本段对齐 macOS Swift App 的“本地模型辅助”语义，复用一个轻量文本模型做短任务，避免标题和路由各自常驻一套模型。上下文压缩不在其中：它的候选链与冷却策略独立，本机压缩请改用 `[agent] compressor_profile`。

| 键 | 说明 |
|---|---|
| `enabled` | 本地辅助模型总开关；默认 `false` |
| `base_url` | OpenAI-compatible API Base；支持域名、局域网 IP 与回环地址，默认 `http://127.0.0.1:11434/v1` |
| `summary_model` | 辅助任务（标题与下一句预测、Worker 路由）复用的模型 ID；默认 `gemma4:e4b-it-qat` |
| `prefer_for_titles` | 会话标题优先本地生成；默认 `true` |
| `prefer_for_context_summaries` | **已废弃、值被忽略。** 上下文压缩不再支持本地模型：摘要替换的是整段旧历史，弱模型压坏一次就不可逆。旧配置写了这一行仍然能正常启动 |
| `prefer_for_worker_routing` | 低置信度 Worker 路由优先咨询本地模型；默认 `true` |

辅助端点可以不设置 API Key/Token，此时不会发送空的 `Authorization`。它可以部署在当前机器、家庭局域网或用户控制的域名后面；如果使用明文 HTTP，传输内容不会加密，应只放在可信网络。标题会按候选链自动回退（压缩的候选链只有远端模型，与本段无关）；Worker 路由仅在关键词规则置信度低于 86 时调用模型，并且只允许更换已知 Worker Profile，不会改变 Tier、自动派工或安全约束。关闭总开关后行为与旧版本一致。

## `[providers.*]` 段

| 键 | 说明 |
|---|---|
| `provider` | 身份：`some-im` / `openai-compatible` / `anthropic` |
| `api` | 线格式：`chat-completions` / `responses` / `anthropic-messages` |
| `api_base` | API Base URL |
| `api_base_env` | 地址环境变量名，与 `api_base` 互斥；缺失时明确报错 |
| `api_key_env` | 存放 Key 的环境变量名（**推荐**） |
| `api_key` | 内联明文 Key（触发 `0600` 权限强制） |
| `model` | 模型 ID |
| `vision_model` | some.im 纯文本模型的视觉回退模型 |
| `context_window` | 上下文窗口，用于状态栏占比显示；被 `[agent] compressor_profile` 选中时，还决定压缩摘要器能吃下的原料上限 |
| `max_output_tokens` | 单次回复的输出上限，三种协议都会带上。上下文压缩、会话标题和跟主模型的 Worker 都沿用它，思考 token 占同一额度。写小了，长会话的压缩会因为摘要被截断而失败，报「provider response did not include a completion」。不写时缺省 16384，不是模型的真实上限 |

`api_key` 与 `api_key_env` 不能同时定义，也不允许为空字符串。

## Provider 与 API 是两个维度

这是最容易混淆的一点：**`--provider` 决定身份、鉴权和专属请求头；`--api` 决定请求/响应的线格式。**

```text
--provider auto | openai-compatible | some-im | anthropic
--api      auto | chat-completions  | responses | anthropic-messages
```

`auto` 的规则：

- host 精确等于 `some.im`、`api.some.im`、`api.niuwoai.com` → 识别为 some.im；
- host 精确等于 `api.anthropic.com` → 识别为 Anthropic Messages；
- 其他 API Base → OpenAI-compatible Chat Completions；
- Responses 必须显式指定 `--api responses`，`auto` 永远不会选它。

两者可以自由组合。例如 some.im 中转提供 Anthropic Messages 线格式时：

```bash
willdeep --provider some-im --api anthropic-messages ...
```

此时仍使用 some.im 的 Bearer 鉴权和上下文头，而消息体使用 Anthropic 格式。

三种线格式的具体契约见 [Provider 协议契约](PROVIDER_PROTOCOLS.md)。

## 三种 Provider 的最小配置

### some.im

```bash
export SOMEIM_API_KEY="<your-key>"
willdeep --provider some-im --model glm-5 --workspace . "检查当前仓库"
```

详见 [some.im 集成](SOMEIM_INTEGRATION.md)。

### OpenAI-compatible Chat Completions

```bash
export WILLDEEP_API_BASE="https://provider.example/v1"
export WILLDEEP_API_KEY="<your-key>"
export WILLDEEP_MODEL="model-id"

willdeep --api chat-completions --workspace . "解释这个项目"
```

### OpenAI Responses

```bash
willdeep \
  --api-base https://api.openai.com/v1 \
  --api responses \
  --model gpt-5 \
  --workspace . \
  "检查 Git 状态并总结风险"
```

### Anthropic Messages

```bash
export ANTHROPIC_API_KEY="<your-key>"

willdeep \
  --provider anthropic \
  --api anthropic-messages \
  --model claude-sonnet-4-5 \
  --workspace . \
  "阅读 README 并提出改进建议"
```

原生 Anthropic 使用 `x-api-key` 和 `anthropic-version: 2023-06-01`。API Base 可写成 `https://api.anthropic.com` 或带尾部 `/v1` 的形式。

## 配置优先级

```text
命令行参数 / WILLDEEP_* 环境变量
  → 当前 Provider Profile
    → Provider 专属环境变量
      → 内置安全默认值
```

API Key 的完整解析链和权限强制规则见 [认证与凭据](AUTHENTICATION.md)。

## 零配置启动

没有 `config.toml` 也能跑：环境里有认得出的钥匙就直接开工，不再强制先走首次设置。

| 环境里有 | 推断出的 Provider | API Base | 缺省模型 |
|---|---|---|---|
| `SOMEIM_API_KEY` | some.im | `https://some.im/v1` | `glm-5` |
| `ANTHROPIC_API_KEY` | Anthropic | `https://api.anthropic.com` | `claude-sonnet-4-5` |
| `OPENAI_API_KEY` + `OPENAI_BASE_URL`（或 `WILLDEEP_API_BASE`） | OpenAI-compatible | 使用提供的地址 | 无，须给 `WILLDEEP_MODEL` |

命令行的 `--provider` / `--api-base` / `--api-key` / `--model` 优先于推断。TUI 开屏会提示凭据来自环境、`willdeep config init` 可固化；`willdeep doctor` 的 `provider` 项在这种情况下报通过。

旧写法不再静默映射：`agent.approval = "ask"`（等同 `strict`）会在 TUI 开屏、`willdeep config check` 的 `legacy` 行和 `doctor` 的 `config` 告警里点名，推荐改成 `smart`。

## 环境变量

| 环境变量 | 用途 |
|---|---|
| `WILLDEEP_API_BASE` | API Base |
| `OPENAI_BASE_URL` | OpenAI-compatible 地址后备；显式 Profile 地址优先 |
| `ANTHROPIC_BASE_URL` | Anthropic 地址后备，保留 Messages 协议；显式 Profile 地址优先 |
| `WILLDEEP_API_KEY` | 通用 API Key |
| `WILLDEEP_MODEL` | 模型 ID |
| `WILLDEEP_CONFIG` | 显式 TOML 配置文件路径 |
| `WILLDEEP_HOME` | 配置与运行时目录，默认 `~/.willdeep` |
| `WILLDEEP_WORKSPACE_CHECKPOINTS` | 设为 `0` / `false` / `off` 关掉每轮开始前的工作区检查点（`/rewind` 就只能回对话），默认开；见 `docs/CHECKPOINT_REWIND.md` |
| `WILLDEEP_LANGUAGE` | 界面语言 `zh-CN` / `en` / `ja` |
| `WILLDEEP_CLIENT_LOGIN_SECRET` | some.im 浏览器登录的客户端密钥，构建时注入 |
| `SOMEIM_API_KEY` | some.im Key 回退 |
| `ANTHROPIC_API_KEY` | Anthropic Key 回退 |
| `OPENAI_API_KEY` | OpenAI-compatible Key 回退 |

## 界面语言

支持简体中文、英语和日语。三种设置方式，优先级从高到低：

1. `--language` 或 `WILLDEEP_LANGUAGE`；
2. `[agent]` 的 `language`；
3. 自动探测（Web 端读浏览器语言，默认 `zh-CN`）。

Web 左栏可直接切换语言，选择保存在当前浏览器的 localStorage 中。

## 图形化模型路由设置

- TUI 输入 `/routing`；
- Web 左栏点击“模型与路由”。

两者编辑同一个 `config.toml`，覆盖 Root Provider/模型、`[agent]` 路由开关与 Deep
预算，以及两根正交的轴：

- **五个公开职责**（`[subagents.*]`）：`generalist` / `implementer` / `tester` /
  `reviewer` / `ops_runner` 的 Provider、模型和上下文窗口——「这个职责平时用什么」。
- **三个模型档位**（`[worker_tiers.*]`）：基础 / 进阶 / 专家的 Provider、模型和
  上下文预算——「派工时说要贵一档，贵成什么样」。专家档标着「需票据」。

空的 Provider/模型表示采用推荐默认：职责在 some.im 上落到基础档 `someim-32b`，
档位落到网关默认表（`someim-32b` / `deepseek-v4-flash` / `gpt-5.6-sol`），其余情况
继承 Root/Provider。旧专门工种配置不会被设置页删除，继续服务自动路由和已保存流程；
显式值始终优先。

想让**某次派工**更贵是传 `worker_tier`，给某个职责绑贵模型是让它**每次**都贵——
别用第二种去达成第一种。

保存使用配置内容指纹检测并发修改，并通过 `0600` 临时文件原子替换；不会重写无关
字段或整份文件的注释。若页面打开后又手改了文件，保存会失败，重新打开设置即可。
正在运行的 Harness 不热切配置，新值从下一次 Harness/子 Agent 创建开始生效。

## 其他配置段

- `[subagents.*]` — 子 Agent 的模型绑定、上下文窗口、工具输出上限（`tool_output_limit`）、验证重试次数（`max_attempts`）、Token 预算、超时与熔断，见 [子 Agent 与后台任务](SUBAGENTS.md) 与 [小上下文 Skill Worker](SKILL_WORKERS.md)；
- `[mcp_servers.*]` — MCP 服务：stdio（`command`）或 Streamable HTTP（`url`，鉴权走 `bearer_token_env` 或 `[mcp_servers.*.oauth]`），见 [Skills 与 MCP](SKILLS_AND_MCP.md)；
- `[skills]` 的 `roots` — 额外的 Skill 搜索根目录，见 [Skills 与 MCP](SKILLS_AND_MCP.md)；
- `[feedback]` — 本机反馈账本：`enabled`（默认 `true`）、`store_text`（默认 `false`，只记 hash 与长度）与 `retain_months`（默认 12，0 表示不清理），见 [本机反馈账本](FEEDBACK_LEDGER.md)；
- `[notifications]` — 出站通知：旧的 `webhook_*` 仍然有效，`[[notifications.channels]]` 可追加飞书、邮件（SMTP / Resend）等多条渠道，见 [出站通知渠道](NOTIFICATIONS.md)；
- `[intake]` — 公网入口：`enabled`、`listen`、`public_url` 三个标量可在配置工作台编辑，站点、表单与 webhook 的数组表在 Web「公网入口」页维护，见 [公网入口](PUBLIC_INTAKE.md)。

## 配置命令

```bash
willdeep config init     # 用私有权限创建示例文件，不覆盖已有配置
willdeep config check    # 解析并严格校验
willdeep config show     # 打印校验后配置，内联 api_key 替换为 [REDACTED]
```

生产配置推荐只使用 `api_key_env`。
