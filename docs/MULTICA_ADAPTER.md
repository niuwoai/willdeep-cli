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

willdeep 的事件再翻回 daemon 认得的四行：`system`（带 `session_id`，续跑全靠它）、
`assistant`（`text`、`thinking`、`tool_use` 块与 `usage`）、`user`（`tool_result`）、
`result`（终态，`is_error` 判成败）。

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
- `--resume` 的值不是 willdeep 会话 UUID 时当作新会话，不报错。

权限模式只接受 `bypassPermissions`、`acceptEdits`、`dontAsk`。`default`、`plan`
需要适配器在 stdin 上应答 `control_request`，这一版不做，直接以退出码 2 拒绝。

用量按 willdeep 的口径换算：它的 `input_tokens` 已含缓存命中，而 daemon 把
`input_tokens` 与 `cache_read_input_tokens` 相加，所以适配器先扣掉缓存部分。
willdeep 不报缓存写入量，`cache_creation_input_tokens` 恒为 0。数字互相矛盾
（缓存命中大于输入）时整条用量丢掉，不报一个负数。

`stop_reason` 为 `max_turns`、`budget_limited`、`incomplete`、`unverified` 或
`partial` 时，即使退出码是 0 也报 `is_error: true`。否则看板会把一次没收敛的
任务当成做完了。

## 验证

```bash
ruby scripts/test/multica_claude_adapter_test.rb
```

测试不启动 willdeep，用一个桩二进制喂入事件。真实的一次跑通需要本机装好
willdeep 并配好 provider，不在这里覆盖。
