# @willdeep/hooks

willdeep-cli 生命周期 Hook 的 TypeScript 契约。它回答“宿主会投递哪些事件、字段是什么、程序能产生什么效果”，不负责注册、加载或执行扩展。

Hook 仍是操作者在 `[[hooks]]` 中配置的 shell 命令。宿主把一个 UTF-8 JSON 对象写入 stdin；退出码 0 放行，阻塞式 Hook 的非零退出拦截，stderr 前 2 KiB 是拒绝理由。本包不提供 `register()`，因为宿主没有 TypeScript 扩展进程。

## 安装

包尚未发布到 npm。开发时直接引用仓库：

```json
{
  "dependencies": {
    "@willdeep/hooks": "github:niuwoai/willdeep-cli#path:sdk/hooks"
  }
}
```

## 先看有哪些事件

```ts
import {
  BLOCKING_HOOK_EVENTS,
  HOOK_EVENTS,
  OBSERVE_ONLY_HOOK_EVENTS,
  canBlock,
} from "@willdeep/hooks";

console.log(HOOK_EVENTS);
// ["before_agent", "agent_end", "pre_tool", "post_tool", "approval_resolved"]

console.log(BLOCKING_HOOK_EVENTS); // ["before_agent", "pre_tool"]
console.log(OBSERVE_ONLY_HOOK_EVENTS);
console.log(canBlock("post_tool")); // false
```

| 事件 | 时机 | 能否拦截 | 关键字段 |
|---|---|---|---|
| `before_agent` | 一次 Agent 运行进入模型处理前 | 能 | `session_id`、`workspace` |
| `agent_end` | 该次运行返回时 | 不能 | `outcome` |
| `pre_tool` | 工具执行前的唯一收口 | 能 | `tool`、`detail` |
| `post_tool` | 工具执行后 | 不能 | `tool`、`detail`、`outcome` |
| `approval_resolved` | 审批器返回决定，或判官最终放行 | 不能 | `tool`、`detail`、`outcome`、`extra.source`、`extra.mode` |

没有 `session_start`、`turn_end`、`pre_write`，也不能改写工具参数或向模型追加上下文。

## 写一个 Hook

```ts
import { parseHookEvent } from "@willdeep/hooks";

const event = parseHookEvent(await new Response(process.stdin).text());
if (event.event === "pre_tool" && event.tool === "run_command") {
  process.stderr.write("生产目录拒绝执行命令\n");
  process.exitCode = 2;
}
```

`parseHookEvent` 只接受宿主会发出的形状。事件名、必填字段和 outcome 不合法时抛出 `HookInputError`；它不会补齐被截断的 JSON，也不会把缺席字段猜成空字符串。

配置写入前可以用 `validateHookConfig` 检查宿主启动时会拒绝的组合，例如在 `post_tool` 上设置 `blocking: true`。它不执行命令，也不证明命令文件存在。

完整的触发条件、脱敏、超时、失败语义和不会投递的路径见仓库 [Hooks 体系](../../docs/HOOKS.md)。
