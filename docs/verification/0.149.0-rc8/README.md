# TUI 用户消息排版验收

日期：2026-11-16。版本：0.149.0-rc8。

## 结果

用户消息只显示发送时间，不显示模型；助手保留该条回复的实际模型。用户消息块整体靠右，使用低对比蓝灰背景，块内文字左对齐，与助手输出之间留一行空白。长行、附件、折叠、搜索、选区、等待与压缩提示继续按实际显示坐标工作。

审查发现的两项回归已先用失败测试复现，再修复并复跑：

- 带 20 个行首空格与 60 个连续字符的用户长行，在 80 列聊天区被内边距挤出末尾字符；现在按正文宽度完整折行，20/40/80 列的真实渲染缓冲均保留全部字符。
- 跨行复制和引用带入消息块缩进与补齐空格；现在以逐行正文列范围排除布局空白，保留用户原有缩进、行尾空格与空行，并覆盖缓存复用、反向拖选、等待、压缩和宽度变化。

## 检查范围

| 检查 | 结果 | 范围与限制 |
| --- | --- | --- |
| `CARGO_INCREMENTAL=0 cargo test -p willdeep tui:: --no-fail-fast` | PASS | 530 通过、0 失败、2 项原有手动性能测试忽略；只覆盖 `tui::` 过滤范围，未执行整个工作区测试 |
| 实际 TUI + Runtime，Ruby PTY 隔离夹具 | PASS | 120×35 简洁、120×35 完整、60×35 窄屏；2 次本机固定模型回复，无真实凭据或外部模型请求 |
| `cargo fmt --all -- --check` | PASS | Rust 格式 |
| `ruby scripts/check_release_version.rb 0.149.0-rc8` | PASS | Cargo、锁文件、两个 runtime 版本钉、Web/npm、安装清单、产品/架构头与 CHANGELOG |
| `ruby -c scripts/verify_tui_user_messages.rb` | PASS | Ruby 验收脚本语法 |
| `git diff --check` | PASS | 差异空白检查 |
| `CARGO_INCREMENTAL=0 cargo clippy -p willdeep --all-targets -- -D warnings` | FAIL | 3 处基线已有告警，详见下文 |

严格 Clippy 的实际错误摘要：

```text
error: method `wait_until_idle` is never used
  crates/willdeep-cli/src/daemon/task_manager.rs:879
error: method `handle_question_text` is never used
  crates/willdeep-cli/src/tui/app_state.rs:1855
error: call to `std::mem::drop` with a value that does not implement `Drop`
  crates/willdeep-cli/src/tui/human_gate_ui.rs:130
error: could not compile `willdeep` (bin "willdeep") due to 3 previous errors
```

已在本次收尾修改前的 HEAD 中核对到这三个原有位置；本轮未改动这些函数。严格静态检查仍为失败，不计为通过。

## 实际终端预览

- [简洁模式，120 列](user-messages/compact.svg)
- [完整模式，120 列](user-messages/full.svg)
- [窄屏，60 列]( FeaturedLinkPlaceholder )
- [集成验收报告](user-messages/integration.md)
- [集成验收 JSON](user-messages/integration.json)
- [汇总检查 JSON](checks.json)

SVG 由实际 ANSI 单元格还原并已逐张检查；字体和终端调色板可能与用户终端不同，不是系统截图。还原器按记录的缩放时点调整终端尺寸，等待布局稳定后取图；背景先于文字绘制，避免覆盖中文宽字符。原始输出及缩放记录保留在 `user-messages/terminal.ansi`、`user-messages/terminal-sizes.json`。

## 交付边界

本次使用 worktree 内的 debug 二进制完成实际交互验收，未替换已安装的 CLI，未安装、发布、推送或创建发布 tag。本次收尾修复和 rc8 版本记录未由本会话提交。
