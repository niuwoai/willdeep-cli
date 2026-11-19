# TUI 轮次收尾投影真实 PTY 验收

版本：willdeep 0.154.0-rc5。结果：PASS；通过 7 项检查。

| 布局 | 检查 | 结果 |
|---|---|---|
| full | committed edit diff is visible while the turn is still running | PASS |
| full | the final result folds the intermediate diff and keeps the tool receipt | PASS |
| full | clicking the retained tool row reopens the committed diff | PASS |
| compact | committed edit diff is visible while the turn is still running | PASS |
| compact | the final result folds the intermediate diff and keeps the tool receipt | PASS |
| compact | clicking the retained tool row reopens the committed diff | PASS |
| binary | binary SHA256 unchanged | PASS |

Binary：/Users/rocky/Sites/willdeep-rs/.claude/worktrees/rule-scan-unreadable-20261119/.claude/target/debug/willdeep

SHA256 前：b1cd13cb9b9189cc60819e197044c4531232b3bb447c6326ae56860e800d5e0d

SHA256 后：b1cd13cb9b9189cc60819e197044c4531232b3bb447c6326ae56860e800d5e0d

取证：full/compact 目录保留运行中、最终结果后和点击重开三个状态的原始 ANSI、屏幕 JSON（含逐格颜色）与私有 Provider 请求。

隔离：Private HOME/WILLDEEP_HOME/XDG/temp/workspace; allowlisted child environment; synthetic loopback Provider; OSC52 captured in owned PTY.

边界：Synthetic loopback Provider only; no real model or user Runtime. Session-switch, failed-send and manual-expansion persistence are covered by Rust native regressions, not this PTY.

私有夹具：成功后已清理。Runtime 清理详情见 JSON 的 cleanup。

无失败项。
