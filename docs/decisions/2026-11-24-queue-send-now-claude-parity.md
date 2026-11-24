# 队列 `send_now` 对齐 Claude Code（#208）

日期：2026-11-24  
状态：已采纳（阶段 A）

## 决策

队列「立即发送 / 优先」（`QueueAction::SendNow`）**默认不**调用 `turn.stop`，对齐 Claude Code「少打断」：

| 会话状态 | 行为 |
|----------|------|
| **空闲** | 置顶 + 清 `held`，`schedule_session`，下一条由调度认领 |
| **Running** + 纯文本队列项（无附件） | 置顶后 `turn.steer` 注入当前任务；成功则从队列 **Remove** 该条；记事件 `turn.queue_steered` |
| **Running** + 附件 / 不可编辑正文 | 仅置顶，等当前轮结束再认领 |
| **WaitingApproval / WaitingAnswer** | 不 steer、不 stop；可 `RunIndependently` 或仅改顺序（与既有门控一致） |

硬中断当前活动轮仍用 **`turn.stop`**（TUI **`Ctrl+X`**），不与「优先发送」绑定。

## 实现要点

- `control_api/turn_queue.rs`：移除 `SendNow` → `turn.stop` 路径；新增 `deliver_send_now_without_stopping_active_turn`。
- `session_store/turn_queue.rs`：`steer_text_for_pending_queue_message`。
- `control_api.rs`：抽出 `deliver_session_steer` 供 steer API 与队列共用。

## 非目标（阶段 B 及以后）

- 同一轮合并多条队列、附件 steer、与 `/btw` 混用 send_now。

## 相关

- GitHub [willdeep-cli#208](https://github.com/niuwoai/willdeep-cli/issues/208)
- [`RUNTIME_CONTROL_API.md`](../RUNTIME_CONTROL_API.md) 待发队列
