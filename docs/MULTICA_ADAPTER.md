# Multica 适配器

让 [Multica](https://github.com/multica-ai/multica) 把 willdeep 当成一个它已经会调度的
agent 来调用。Multica 不开放新协议，自定义 runtime 只能套用一个既有协议族，所以
[scripts/multica_claude_adapter.rb](../scripts/multica_claude_adapter.rb) 冒充 `claude` 二进制，
讲 daemon 期待的 stream-json，内里调 `willdeep run`。

适配器是本机的事，不进 willdeep 的发布物。它不注册 provider、不改 daemon、也不把
Multica 的凭据写进 willdeep 的配置。

## 它翻译什么

daemon 拉起一次任务时拼出的参数形如：

```
claude -p --output-format stream-json --input-format stream-json --verbose \
  --strict-mcp-config --permission-mode bypassPermissions \
  --disallowedTools AskUserQuestion [--model M] [--resume SESSION]
```

提示词是 stdin 上的一行 JSON：

```json
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"..."}]}}
```

适配器把它翻成：

```
willdeep run --output ndjson [--session SESSION] [--model M] --input -
```

willdeep 的事件再翻回 daemon 认得的四类事件：`system`（只在 willdeep 确实报告会话 ID 时携带 `session_id`）、
`assistant`（`text`、`thinking`、`tool_use` 块与按模型归属的 `usage`）、`user`（`tool_result`）、
`result`（终态，`is_error` 判成败）。文本、思考和工具事件逐行转发；会话 ID 在终态确认后另补一条 `system`，防止失效的续跑 ID 被当作已建立的会话。

## 注册

先确认 `willdeep` 在 `PATH` 上，且 `~/.willdeep/config.toml` 里 `agent.approval`
至少是 `workspace-write`。Multica 传的是 `bypassPermissions`，它期望进程自己跑完；
willdeep 一旦停下来等审批，这次任务就会以退出码 4 失败。

包装脚本（daemon 的命令字段不是 shell，管道和变量展开都要放进脚本里）：

```bash
#!/bin/sh
export WILLDEEP_BIN="${WILLDEEP_BIN:-willdeep}"
exec ruby /path/to/willdeep-rs/scripts/multica_claude_adapter.rb "$@"
```

在 Multica 里：Runtimes → 这台机器 → Add custom runtime → 协议族选 `claude`，
命令填这个脚本。daemon 启动时必须先探测到至少一个内置工具，只有这个自定义
runtime 时 daemon 起不来。

`multica runtime profile set-path <profile-id> --path /absolute/path` 用来给 daemon
一个它自己的 `PATH` 里找不到的绝对路径。

## 边界

这些参数 daemon 会传，但 willdeep 没有对应能力，适配器丢弃并在 stderr 记一行：

- `--mcp-config`：MCP 服务器不会接入这次任务。
- `--effort`：思考等级不生效。
- `--append-system-prompt`：追加的系统提示词不生效。
- `--disallowedTools`、`--allowedTools`：工具限制不生效。
- `--resume` 的值不是 willdeep 会话 UUID 时当作新会话，不报错；合法但已失效的 UUID 若尚未开始工具、执行失败且没有返回会话 ID，终态不回显旧 ID，让 Multica daemon 用新会话重试。

权限模式只接受 `bypassPermissions`、`acceptEdits`、`dontAsk`。`default`、`plan`
需要适配器在 stdin 上应答 `control_request`，这一版不做，直接以退出码 2 拒绝。

用量按 willdeep 的口径换算：它的 `input_tokens` 已含缓存命中，而 daemon 把
`input_tokens` 与 `cache_read_input_tokens` 相加，所以适配器先扣掉缓存部分。
willdeep 不报缓存写入量，`cache_creation_input_tokens` 恒为 0。数字互相矛盾
（缓存命中大于输入）时整条用量丢掉，不报一个负数。`usage` 从 NDJSON 事件取实际请求模型；
`subagent_usage` 也保留 Worker 自己的模型。旧版 willdeep 的用量事件没有 `model` 时无法可靠归属，会跳过该条，不拿 Multica 请求的模型代替实际模型。

willdeep 成功的 `tool_completed` 没有工具输出，只给 daemon 一个不带 `content` 的工具结果；
失败事件可能含敏感参数和输出，适配器只报告固定错误状态，不转发原文；
willdeep 的 stderr 也不直通 Multica daemon。若续跑失败前已经出现工具调用，适配器
保留旧会话指针并让任务失败，不让 daemon 用新会话重跑可能产生副作用的提示。
Multica 对适配器强制取消或超时时，独立监护进程会终止 willdeep 进程组和其中的工具进程。
进程自己脱离该进程组的后台作业不在此保证范围内。

`stop_reason` 为 `max_turns`、`budget_limited`、`incomplete`、`unverified` 或
`partial` 时，即使退出码是 0 也报 `is_error: true`。否则看板会把一次没收敛的
任务当成做完了。没有 `completed` 或 `partial` 终态行时，即便子进程返回 0，适配器也以退出码 5 和错误结果结束。

## 验证

```bash
ruby scripts/test/multica_claude_adapter_test.rb
```

测试用桩二进制喂入固定事件，不启动真实 willdeep，也不调用模型。检测到本机
`~/Sites/multica/server/go.mod` 时，它还会编译小型 Go 探针，经 Multica 自己的
`agent.New("claude")`/`Execute` 消费这些事件；其他位置可用 `MULTICA_ROOT` 指向
Multica 仓库。没有本地仓库时这两项测试明确跳过。取消测试分别发送 SIGTERM 和
SIGKILL，并验证 willdeep 工具子进程没有继续执行。真实 provider 跑通仍需另行验收。
