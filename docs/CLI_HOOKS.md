# willdeep-cli 的 Hooks 体系

> 状态：可用，契约版本与宿主 `0.155.0-rc1` 一致。
> 配置与操作说明见 [生命周期挂钩](HOOKS.md)；本页说明事件在宿主里的位置、投递边界和 TypeScript 用法。

Hook 是操作者在配置里写的 shell 命令。宿主在五个固定位置把一个 JSON 对象写入命令的 stdin，并等待它退出。它不是插件页面可以注册的回调，也没有随宿主启动的 TypeScript 扩展进程。

## 事件在一次运行中的位置

```text
执行租约
  -> before_agent          可拒绝；拒绝后不进入模型
  -> 模型请求与工具循环
       -> pre_tool         可拒绝；拒绝后不执行该工具
       -> 审批（仅需要审批的路径）
            -> approval_resolved
       -> 工具执行
       -> post_tool
  -> agent_end             观察真实终态；失败不改变终态
```

`before_agent` 与 `agent_end` 包围的是一次 Agent 运行，不是会话创建或销毁，也不是每一次模型请求。执行租约没有取得时，运行尚未开始，这两个事件都不投递。

## 五条事件

| 事件 | 宿主位置 | 能改变什么 | stdin 的业务字段 |
|---|---|---|---|
| `before_agent` | `Agent::run_checkpointed_with_summary` | 拒绝整次运行 | 无 `tool`、`detail`、`outcome` |
| `agent_end` | 同一次运行返回后 | 不能 | `outcome` |
| `pre_tool` | `ToolExecutor::execute` | 拒绝这一次工具调用 | `tool`、`detail` |
| `post_tool` | 工具分发返回后 | 不能 | `tool`、`detail`、`outcome` |
| `approval_resolved` | 审批器返回后，或判官最终放行后 | 不能 | `tool`、`detail`、`outcome`、`extra.source`、`extra.mode` |

`session_id` 与 `workspace` 由宿主绑定。调用没有会话时，`session_id` 是 JSON null，不是缺席字段。所有事件都序列化 `tool`、`detail`、`outcome`、`exit_code`；当前五条投递路径没有进程退出码，`exit_code` 为 null。空的 `extra` 不出现；超限裁剪后一定有 `extra.truncated="true"`。

### 终态与结果

`agent_end.outcome`：

| 值 | 含义 |
|---|---|
| `finished` | 模型给出终稿，且没有激活目标 |
| `goal_complete` | 激活目标被模型显式完成 |
| `budget_limited` | 预算用尽并交出部分结果 |
| `max_turns` | 轮次用尽 |
| `incomplete` | 提供者中断或过滤了输出 |
| `unverified` | 工作区改动没有通过当前快照验证 |
| `error` | 运行返回错误 |
| `blocked` | `before_agent` 已拒绝 |

`post_tool.outcome` 只有 `ok` 或 `error`，不包含工具返回正文。

`approval_resolved` 有两个来源：

- `extra.source="approver"`：`allow_once`、`always_allow`、`deny`。这里的 `deny` 也可能是无人交互、通道关闭或取消，不能当成人工拒绝。
- `extra.source="judge"`：只有判官最终放行，outcome 为 `allow`，`tool` 为 `run_command`。判官拒绝后转交审批器时，只投递审批器的最终决定。

`extra.mode` 是投递时的审批模式：`read-only`、`strict`、`smart`、`workspace-write`、`full-access`。

## 投递规则

同一事件按配置顺序执行，阻塞式 Hook 第一次拒绝即停止后续 Hook。`before_agent` 与 `pre_tool` 可以 `blocking = true`；其余三个事件配置为阻塞时，宿主启动失败。

退出码 0 放行。阻塞式 Hook 的非零退出拦截，stderr 最多 2 KiB 进入拒绝理由。超时默认 10 秒，阻塞式 Hook 默认 `on_error = "deny"`；`ignore` 必须显式写出。多个 Hook 的等待时间累加，并且不计入模型运行计时。

stdin 是一个 UTF-8 JSON 对象，编码后不超过 16 KiB。超限时先裁剪字段并设置截断标记，再输出仍可解析的 JSON。被裁短的 `detail` 不能当作完整命令或 diff。`detail` 在出进程前使用审批日志同一套凭据脱敏。

只读委派不执行操作者配置的 Hook。`tool_status` 不进入生命周期 Hook。静态放行、白名单命中、FullAccess 免审、审批前硬拒绝和 `ask_user` 不产生 `approval_resolved`。

取消、进程退出或崩溃可能使事件未投递。宿主不保存待补发队列；Hook 失败也不会改写已经形成的审批决定、工具结果或运行终态。

## 配置与信任

```toml
[[hooks]]
name = "change-ticket"
event = "pre_tool"
command = "/usr/local/bin/check-change-ticket"
blocking = true
timeout_seconds = 10
on_error = "deny"
```

命令使用宿主平台 shell 执行，不进入模型命令的 OS 写入围栏。因此能修改这份配置的人，就是能在宿主机器上执行命令的人。配置文件权限是这条能力的边界。

Hook 不能改参数、替换工具输出、增加模型上下文，也不能获得新的工具权限。需要这些能力时，应使用已批准插件的 MCP 工具与页面桥，而不是把 Hook 扩大成进程内扩展。

## TypeScript SDK

[`sdk/hooks`](../sdk/hooks) 是这五条事件的类型与解析器，包名为 `@willdeep/hooks`。它导出：

- `HOOK_EVENTS`、`BLOCKING_HOOK_EVENTS`、`OBSERVE_ONLY_HOOK_EVENTS`：可发现的事件清单。
- `parseHookEvent`：把宿主写入 stdin 的文本解析成对应事件；形状不符抛出 `HookInputError`。
- `canBlock`：判断事件是否接受阻塞配置。
- `validateHookConfig`：检查启动前就会被拒绝的配置组合，不执行命令。

最小用法：

```ts
import { parseHookEvent } from "@willdeep/hooks";

const event = parseHookEvent(await new Response(process.stdin).text());
if (event.event === "before_agent") {
  process.stderr.write("环境未就绪\n");
  process.exitCode = 2;
}
```

对应配置是 `command = "node /path/audit-hook.js"`。SDK 不扫描目录、不接收插件 manifest，也不会在宿主启动时自动加载。

## 与另外两条扩展路径的边界

| 路径 | 谁配置 | 运行位置 | 能做什么 |
|---|---|---|---|
| `[[hooks]]` | 操作者 | 宿主 shell 子进程 | 观察五个事件；两个事件可拒绝 |
| 插件 MCP 命令 | 已批准插件清单 | 插件隔离进程 | 提供工具和终端命令 |
| 插件页面桥 | 已批准插件页面 | opaque-origin iframe | 按权限调用宿主能力 |

插件页面和 MCP 进程都不能通过 manifest 注册这五个 Hook。允许插件贡献 Hook 之前，必须先绑定内容审批、继承权限和进程隔离；manifest 中的字符串不能直接当作受信任 shell。

## 现在没有的能力

- 没有会话开始/结束、每一轮模型请求、写文件前的独立事件。
- 没有 Hook 改写参数或贡献上下文的协议。
- 没有 TypeScript 扩展的热加载、会话状态和远程分发。
- `approval_resolved` 不保证崩溃后补发，也不证明工具随后执行成功。

这些缺项的取舍见 [参照 Pi 的扩展能力演进](PI_EXTENSION_EVOLUTION.md)。
