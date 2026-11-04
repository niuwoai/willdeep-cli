# Magpie 本地网关

WillDeep 可通过 Magpie 的 OpenAI Chat Completions 或 Anthropic Messages 接口运行。Magpie 作为单独的 Provider Profile，不替换已有供应商。下文保留现有手动网关接入方法；Magpie Agents 页的社区插件接入状态见下一节。

## 社区 Agent 插件（已实现，本地验收完成）

2026-11-04 用户确认“按设计实现”：通过两个独立社区 Agent 插件提供 **WillDeep**（`willdeep`）和 **WillDeep CLI**（`willdeep-cli`）。首版使用本机回环网关、OpenAI Chat Completions 和固定公开客户端标记；真实 Gateway Key、远程/共享网关不在首版范围。

- [需求与 JSON v1 契约](MAGPIE_AGENT_REQUIREMENTS.md)：字段、严格回环 URL、状态、选择优先级及验收标准。
- [设计与已确认决策 D1–D5](MAGPIE_AGENT_DESIGN.md)：两端独立临时 Provider、文件所有权、实际请求和会话恢复约束。
- [执行台账](MAGPIE_AGENT_PLAN.md)：T0–T6 完成；[schema](schemas/magpie-agent.v1.schema.json) / [原始 fixture](fixtures/magpie-agent-v1/cases.json) 冻结；T7 发布与社区提交未授权。
- [验收报告](verification/0.142.0-rc1/magpie-agents/README.md)：隔离链 21/21、真实 Magpie 链 22/22、插件宿主 12/12、桌面原生测试 19/19，含未覆盖项。

连接仅为没有显式 Provider/模型选择的新会话提供默认值；已有会话固定自己的 Provider 和模型，外部模型 A→B 后旧会话仍用 A，新会话用 B。旧无固定选择会话恢复跳过集成默认值；断开后明确引用集成的会话报错，不静默转到其他供应商。显式其他 Provider、模型或连接覆盖不受无关集成配置错误阻断。集成不写原 TOML、Desktop Provider 库或钥匙串，不改变审批、沙箱和显式 Worker 设置。

本版本交付 CLI **0.142.0-rc1** / Desktop **1.427.0-rc1**（Build 941）与两个 `0.1.0-rc2` 本地可安装插件。组合验收在 macOS 完成，Linux/Windows 仅做 Windows GNU 目标交叉编译；未发布、未社区收录，正式发布和社区提交另行授权。

## 临时前台连接

以下标准变量写法适用于没有显式 Provider 地址和 Key 的临时会话。已有 Profile 的地址和 Key 优先；日常使用已有配置时，请采用下一节的独立 Magpie Profile。Magpie 已在本机默认端口运行时，模型名从其 `/v1/models` 返回值中选择，替换下面的 `provider/model`：

```bash
OPENAI_BASE_URL=http://127.0.0.1:3425/v1 \
OPENAI_API_KEY=magpie-willdeep \
WILLDEEP_MODEL=provider/model \
willdeep --provider openai-compatible --api chat-completions
```

Anthropic Messages 使用不带 `/v1` 的地址，并显式选择协议，避免已有默认 Profile 的供应商或 API 类型覆盖本次意图：

```bash
ANTHROPIC_BASE_URL=http://127.0.0.1:3425 \
ANTHROPIC_API_KEY=magpie-willdeep \
WILLDEEP_MODEL=provider/model \
willdeep --provider anthropic --api anthropic-messages
```

这里的 `magpie-willdeep` 是本机网关的客户端标识，不是真实上游密钥。连接远程、共享或容器中的网关时，必须使用 Magpie 为该客户端签发的 Gateway Key。

## 前台、子 Agent 与 Runtime 共用 Profile

在 `~/.willdeep/config.toml` 中保留已有配置，加入：

```toml
[providers.magpie]
provider = "openai-compatible"
api = "chat-completions"
api_base = "http://127.0.0.1:3425/v1"
api_key_env = "MAGPIE_API_KEY"
model = "provider/model"

[subagents.scout]
provider_profile = "magpie"
```

```bash
export MAGPIE_API_KEY=magpie-willdeep
willdeep --profile magpie
willdeep --profile magpie run "检查项目并汇报"
```

未单独绑定 Profile 的子 Agent 跟随父会话；显式绑定的子 Agent、Worker 档位按它自己的 Profile 解析，不接受父进程的通用地址、Key 和模型覆盖。需要不同模型时，可新建另一个 Magpie Profile 并绑定给相应子 Agent。

地址也可来自环境变量：将 `api_base` 替换为 `api_base_env = "MAGPIE_BASE_URL"`，并设置该变量。两个字段不能同时定义。显式 `api_base_env` 或 `api_key_env` 缺失、空白时会报错，不会静默使用别的供应商。

Runtime 是长期运行的独立进程，只能读取它启动时继承的环境；之后在另一个终端 `export` 不会改变它。把相同变量提供给 Runtime 的启动环境，或在无活动任务、无待审批项时重新启动。不要把 Key 放进任务参数或事件存储。Web、定时任务和手机发起的后台执行沿用 Runtime 所选 Profile。

## 优先级

- 前台主模型及 Runtime 主模型：显式参数 → `WILLDEEP_API_BASE` / `WILLDEEP_API_KEY` / `WILLDEEP_MODEL` → 所选 Profile。
- Profile 的地址：`api_base` 或 `api_base_env` → 与供应商匹配的 `OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL` → 内置供应商默认地址。
- Profile 的 Key：`api_key` 或 `api_key_env` → 与供应商匹配的 Key 变量。
- 标准地址变量为空白时视为未设置；显式配置仍优先。不会把 Anthropic 地址用于明确的 OpenAI Profile，或反过来。没有供应商配置时，Anthropic 地址配合对应 Key 选择 Messages，OpenAI 地址选择 OpenAI-compatible。

临时 `--api-base` / `WILLDEEP_API_BASE` 等进程覆盖仍不序列化进 Runtime 任务；无头 `run` 会沿原规则在本进程执行。需要持久 Runtime 执行时使用 Profile。

## 隔离验证

常规自动测试覆盖变量优先级、配置隔离、错误输入，以及真实 CLI 的前台 → 子 Agent、Runtime → 子 Agent 两条链路。

使用自行构建的官方 Magpie CLI，还可运行真实网关验证：

```bash
MAGPIE_TEST_BIN=/path/to/magpie \
WILLDEEP_TEST_BIN=/path/to/willdeep \
ruby scripts/test/magpie_integration.rb /private/tmp/willdeep-magpie-qa
```

脚本只使用临时配置目录、隔离的子进程 HOME、本地模拟模型上游及测试 Key，关闭账号发现和遥测。验证模型目录、两个 API、流式输出、只读工具调用、显式子 Agent 和 Runtime；输出 JSON、Markdown 报告并关闭测试进程。不读取真实模型凭据，不发起付费请求，也不安装或升级用户的程序。

官方协议参考：[Magpie 集成文档](https://github.com/yetone/magpie/blob/main/docs/integrating.md)。
