# TUI 工具视觉真实 PTY 验收

版本：willdeep 0.152.0-rc2。结果：FAIL；通过 17 项检查。

| 布局 | 检查 | 结果 |
|---|---|---|
| full | requested layout is painted through the default Runtime TUI | PASS |
| full | historical ending touches AI/time and hides your turn | PASS |
| full | running command 1 expands past the 96-character summary | PASS |
| full | completed command 1 collapses while next Provider waits | PASS |
| full | running command 2 expands past the 96-character summary | PASS |
| full | completed command 2 collapses while next Provider waits | PASS |
| full | clicking the last folded thought reveals only its own body | PASS |
| full | create_file commits a green numbered diff with filename/counts | PASS |
| full | edit_file reads a real baseline then commits red/green/context rows | FAIL |
| compact | requested layout is painted through the default Runtime TUI | PASS |
| compact | historical ending touches AI/time and hides your turn | PASS |
| compact | running command 1 expands past the 96-character summary | PASS |
| compact | completed command 1 collapses while next Provider waits | PASS |
| compact | running command 2 expands past the 96-character summary | PASS |
| compact | completed command 2 collapses while next Provider waits | PASS |
| compact | clicking the last folded thought reveals only its own body | PASS |
| compact | create_file commits a green numbered diff with filename/counts | PASS |
| compact | edit_file reads a real baseline then commits red/green/context rows | FAIL |
| binary | binary SHA256 unchanged | PASS |

Binary：/Users/rocky/Sites/willdeep-rs/.claude/worktrees/multica-claude-adapter/target/p0-p1-rc152-rc2-20261117-final1/willdeep

SHA256 前：0a74c1a765fed6503dead6e52df4405ff70cb5e264c087ca40a46b72adca66a5

SHA256 后：0a74c1a765fed6503dead6e52df4405ff70cb5e264c087ca40a46b72adca66a5

取证：full/compact 目录保留各状态的原始 ANSI、屏幕 JSON（含逐格颜色）、私有 Provider 请求和工具回执。

隔离：Private HOME/WILLDEEP_HOME/XDG/temp/workspace; allowlisted child environment; synthetic loopback Provider; file-lock synchronization; OSC52 captured in owned PTY.

边界：No final finish after writes: the last Provider request remains gated until private Runtime shutdown. Does not verify real models, public-event redaction, Web/mobile, manual expansion, parallel calls or cross-platform behavior.

写工具后不发送最终 finish，不把自动验证或整轮完成作为本脚本通过条件。历史结束行来自写操作前实际完成的纯文本 Runtime 轮次。

私有夹具：失败保留于 /var/folders/np/rjc0y5w52x324x21pv6g33440000gp/T/wd-tui-tool-visuals-20261117-85065-grgg7a。Runtime 清理详情见 JSON 的 cleanup。

- full: RuntimeError: missing numbered diff row for visual-edit.txt: 1/1 EDIT_CONTEXT_BEFORE
- compact: RuntimeError: missing numbered diff row for visual-edit.txt: 1/1 EDIT_CONTEXT_BEFORE
