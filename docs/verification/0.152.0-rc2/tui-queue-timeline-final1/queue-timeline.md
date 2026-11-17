# TUI 队列时间线验收

willdeep 0.152.0-rc2

通过：true；检查：10。

| 布局 | 检查 | 结果 |
|---|---|---|
| compact | queued inputs do not start early | PASS |
| compact | first dequeued user follows the complete preceding reply | PASS |
| compact | all dequeued messages and replies follow actual turn order | PASS |
| compact | queue acknowledgement is gone after delivery and draft remains | PASS |
| compact | Runtime persists the same conversation order | PASS |
| full | queued inputs do not start early | PASS |
| full | first dequeued user follows the complete preceding reply | PASS |
| full | all dequeued messages and replies follow actual turn order | PASS |
| full | queue acknowledgement is gone after delivery and draft remains | PASS |
| full | Runtime persists the same conversation order | PASS |

隔离：Private HOME/WILLDEEP_HOME/workspace, loopback synthetic provider, captured OSC 52 only.

仅覆盖本机合成 Provider 和 Runtime TUI，不代表真实模型、跨平台或 /local 的端到端验证。
