# 队列 `send_now` 与 #208

日期：2026-11-24  
状态：**已撤回**阶段 A（不打断 + steer）；恢复「优先发送时 stop 活动轮」

## 当前决策（rc13）

`QueueAction::SendNow` 将目标置顶；若会话有活动轮且**不在**审批/回答门控，调用 `turn.stop` 取消该轮，再 `schedule_session` 由调度认领优先消息。与 develop 上单测 `queue_remove_clear_and_send_now_preserve_other_sessions_and_order` 一致。

## 已撤回：阶段 A（rc12）

曾尝试对齐 Claude Code「少打断」：`send_now` 不 `turn.stop`，Running + 纯文本走 `turn.steer`。CI 与产品预期要求活动轮进入 `Cancelled`，且与另一会话对 #208 的表述冲突后于 rc13 恢复 stop 路径。

## 相关

- GitHub [willdeep-cli#208](https://github.com/niuwoai/willdeep-cli/issues/208)
- PR #219
- [`RUNTIME_CONTROL_API.md`](../RUNTIME_CONTROL_API.md) 待发队列
