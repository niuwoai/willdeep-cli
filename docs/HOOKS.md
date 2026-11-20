# 生命周期挂钩（Hooks）

> 状态：**可用**。当前覆盖 `before_agent` / `agent_end` / `pre_tool` / `post_tool` / `approval_resolved`。新增 Agent 运行边界的设计依据见 [Pi 扩展演进](PI_EXTENSION_EVOLUTION.md)；审批结果契约见[审批结果决策](decisions/2026-11-17-approval-resolved-hooks.md)。

审计要的是"谁在什么时候做了什么"，合规要的是"没批准就不许干"。这两件事都不能
交给一条可以丢的通知。

## 和通知 webhook 的区别

| | `[notifications]` webhook | `[[hooks]]` |
|---|---|---|
| 时机 | 事后 | 事中 |
| 位置 | 关键路径**外**，detached | 关键路径**上** |
| 端点挂了 | 最多浪费一次超时，轮次照跑 | 阻塞式 hook 会拦下动作 |
| 用途 | 提醒人来看一眼 | 审计留痕、门禁拦截 |

webhook 那条设计成"永远不会让一个轮次失败"是对的——没人希望因为通知服务器
宕机就干不了活。但也正因如此，**它会丢事件**：把审计需求接到 webhook 上，
丢掉的恰好是最需要留痕的那次。

## 契约

每条 hook 是一条 shell 命令：

1. 事件 JSON 从 **stdin** 喂进去（一行 UTF-8，上限 16 KiB）。超出预算时先裁剪字段并标明截断，再序列化，仍保持合法 JSON。
2. **退出码 0 = 放行。** 阻塞式 hook 的**非零退出 = 拦截**，stderr 前 2 KB
   作为拒绝理由回给模型和用户。
3. 超时（默认 10 秒）按 `on_error` 处置。

非阻塞 hook 的退出码不影响流程。非零退出、启动失败或超时会向 stderr 输出结构化警告，包含脱敏后的 Hook 名、事件、失败类别和可用的退出码，不记录命令、事件载荷或原始 stderr。

### 事件载荷

```json
{
  "event": "pre_tool",
  "session_id": "0b4e…",
  "workspace": "/Users/me/project",
  "tool": "run_command",
  "detail": "cargo test -p willdeep-core",
  "outcome": "ok"
}
```

字段刻意扁平：hook 多半是三行 shell 加一个 `jq`，嵌套结构会让最常见的用法难写。

`detail` **已经按审批日志同一套规则脱敏**（`redact_credentials`）。凭据一旦交出
进程就收不回来了，这件事不指望每个 hook 作者自己记得。

## 触发点

| 事件 | 时机 | 能拦吗 |
|---|---|---|
| `before_agent` | 一次 Agent 运行的模型处理之前 | **能** |
| `agent_end` | 一次 Agent 运行返回时 | 不能 |
| `pre_tool` | 工具即将执行 | **能** |
| `post_tool` | 工具执行完毕 | 不能——事情已经发生了 |
| `approval_resolved` | 审批器返回决定，或 AI 判官最终放行 | 不能 |

审批器结果的 `tool` 是审批请求动作名；判官放行命令时为 `run_command`。`detail` 复用凭据脱敏，`session_id` / `workspace` 绑定当前 Registry。`extra.source=approver` 时，`outcome` 为 `allow_once`、`always_allow` 或 `deny`；`extra.source=judge` 时为 `allow`；`extra.mode` 记录投递时的审批模式。判官拒绝或不可用后转审批器，只投递后者实际返回的最终决定。超长载荷带 `extra.truncated="true"`，裁剪后的字段不能当作完整命令或 Diff。

`approver` 不等于人工来源：无人交互、通道关闭或取消也可能返回 `deny`。人工拒绝与取消的细分仍看 Runtime 反馈账本。`always_allow` 仅表示审批器返回该决定，不证明后续授权持久化成功；审批结果也不证明工具已经执行或成功。

静态放行、白名单命中、FullAccess 免审、审批前硬拒绝及 `ask_user` 问答不产生这类事件。事件在当前调用链内有界等待；失败不能改变审批决定，取消或进程退出仍可能中断投递，不提供持久队列或崩溃补发。

在 `agent_end`、`post_tool` 或 `approval_resolved` 上写 `blocking = true` 会被配置检查直接拒绝启动。
一个拦不住任何东西却看起来像门禁的配置，比没有配置更危险。

### Agent 运行边界

`before_agent` / `agent_end` 沿用 `session_id` 与 `workspace`，`tool`、`detail` 为空，不发送用户提示词、模型输出或原始错误。结束事件的 `outcome` 为实际停止原因：`finished`、`goal_complete`、`budget_limited`、`max_turns`、`incomplete` 或 `unverified`；返回错误时为 `error`，前置 Hook 拒绝时为 `blocked`。前置拒绝不会调用模型。

```toml
[[hooks]]
name = "environment-check"
event = "before_agent"
command = "/usr/local/bin/check-agent-environment"
blocking = true

[[hooks]]
name = "run-audit"
event = "agent_end"
command = "jq -c . >> /var/log/willdeep-runs.jsonl"
```

这些事件覆盖一次 Agent 运行，而不是每次模型请求或会话创建/销毁。结束 Hook 自己失败不会改变运行结果；结束事件也不代表后台任务全部完成。只读委派不执行这类 Hook。Hook 使用自身 `timeout_seconds`，在模型运行计时边界之外等待；多个 Hook 的等待时间会累加。调用方取消整个 future、进程退出或崩溃时，不保证投递结束事件，也不提供补发。执行租约获取失败时运行未开始，不产生这两个事件。

## 失败时默认拦，不默认放

阻塞式 hook 超时或起不来时，默认 `on_error = "deny"`。

理由是这类 hook 的用途是合规门禁，而**一个坏掉就自动放行的门禁等于没有门禁**
——它恰好会在出事的时候失效。代价是 hook 写坏会把 agent 卡住，所以拒绝理由里
必须点名是哪条 hook、为什么：

```
<hook-denied hook="change-ticket">
缺少变更单编号
</hook-denied>
```

明确不想要这个语义的，写 `on_error = "ignore"`。

## 配置

```toml
[[hooks]]
name = "audit"
event = "pre_tool"
command = "jq -c . >> /var/log/willdeep-audit.jsonl"

[[hooks]]
name = "change-ticket"
event = "pre_tool"
command = "/usr/local/bin/check-change-ticket"
blocking = true
timeout_seconds = 10
on_error = "deny"
```

同一事件上的多条 hook 按配置顺序执行，**第一条拦截即短路**：动作已经不会发生
了，后面的 hook 再跑也只是在为一件不存在的事收集审计记录。

配置写错（事件名拼错、`post_tool` 上开 `blocking`、命令为空、`on_error` 取值
非法）会**直接让启动失败**，不静默跳过。一条被悄悄忽略的门禁 hook 会让用户
以为门禁在生效，而它一次都不会触发。

## hook 自己不进沙箱

[OS 级写入围栏](SANDBOX.md)罩的是模型请求执行的命令。hook 是**操作者自己配的
代码**，信任级别与操作者的 shell 相同：审计 hook 往 `/var/log` 写、门禁 hook 去
问公司的策略服务，都是它的本职。把它关进工作区围栏只会让这两件事都干不成。

反过来说：**能写 `[[hooks]]` 的人，等于能在这台机器上执行任意命令。** 配置文件
的权限就是这条能力的边界。

## 现在还没有的

- **尚无会话与逐轮事件。** `before_agent` / `agent_end` 对应一次运行，不是会话创建/销毁，也不是每次模型请求；没有 `session_start` / `turn_end` / `pre_write`。
- **审批结果不保证持久送达。** 取消或崩溃可中断 Hook；它不替代 Runtime 人工处置反馈账本和工具执行证据。
- **hook 改不了参数。** 它只能放行或拦截，不能重写模型请求的命令——能改的话
  就得回答"改完之后审批过的还算不算数"，那是另一个设计。

## 相关文档

- [审批与自动化](APPROVALS.md) — 进程内的三道闸门
- [OS 级写入围栏](SANDBOX.md) — 内核裁决的第四道
- [配置指南](CONFIGURATION.md) — 完整 TOML 结构
- [rc3 审批 Hook 验收](verification/0.150.0-rc3/clippy-approval-hooks.md) — 实际命令、真实 stdin 与文件效果、失败历史和未验证项
