# TUI 文字标识与无用户角色前缀的排版验收

日期：2026-11-16。版本：0.149.0-rc9。

## 结果

用户消息块隐藏生成的 `You:` 前缀，保留正文、发送时间、低对比蓝灰背景与整体右对齐。原始 transcript 保留角色标记；用户自己输入的 `You:`、助手前缀、Thought 文本和 Markdown 均按正文原样显示，不重新解释。

等待动画与压缩提示按用户消息块的逐行元数据定位；长消息换行、折叠阈值和收起定位共用不含角色前缀的正文排版。既有复制、引用、附件、搜索、缓存和流式间距回归继续通过。

## 检查范围

| 检查 | 结果 | 范围与限制 |
| --- | --- | --- |
| `CARGO_INCREMENTAL=0 cargo test -p willdeep tui:: --no-fail-fast` | PASS | 532 通过、0 失败、2 项原有手动性能测试忽略；仅覆盖 `tui::`，未执行全工作区测试 |
| `CARGO_INCREMENTAL=0 cargo build -p willdeep` | PASS | debug CLI 构建 |
| 实际 TUI + Runtime，Ruby PTY 隔离夹具 | PASS | 120×35 简洁、120×35 完整、60×35 窄屏；仅 2 次本机固定模型回复 |
| `cargo fmt --all -- --check` | PASS | Rust 格式 |
| `ruby scripts/check_release_version.rb 0.149.0-rc9` | PASS | Cargo/锁文件、两个 runtime 依赖版本钉、Web/npm、安装清单、产品与架构版本头、CHANGELOG |
| `ruby -c scripts/verify_tui_user_messages.rb` | PASS | Ruby 验收脚本语法 |
| `git diff --check` | PASS | 差异空白 |
| `CARGO_INCREMENTAL=0 cargo clippy -p willdeep --all-targets -- -D warnings` | FAIL | 3 处基线已有告警，exit 101 |

首轮 TUI 回归为 531 通过、1 失败、2 忽略：内存终端画面测试用连续中文定位消息，但 TestBackend 会把宽字符续格空白拼入测试文本。改用同一条用户消息中的 ASCII 正文词定位，仍严格检查等待动画紧贴消息下一行；完整复跑后得到上述 532 通过结果。

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

三个位置均为此前已核对的基线问题，本轮未改动对应函数。严格静态检查仍为失败。

## 图片与原始证据

已保存供教程或官网选用的 2 倍尺寸 PNG，并逐张检查；[素材说明与尺寸索引](../../assets/tui-user-messages/0.149.0-rc9/README.md)包含建议替代文本、来源和复现命令。

- [简洁模式 PNG](../../assets/tui-user-messages/0.149.0-rc9/compact.png) · [SVG 原稿](user-messages/compact.svg)
- [完整模式 PNG](../../assets/tui-user-messages/0.149.0-rc9/full.png) · [SVG 原稿](user-messages/full.svg)
- [窄屏 PNG](../../assets/tui-user-messages/0.149.0-rc9/narrow.png) · [SVG 原稿](user-messages/narrow.svg)
- [集成验收 JSON](user-messages/integration.json) · [Markdown](user-messages/integration.md)
- [汇总检查 JSON](checks.json)
- [原始 ANSI](user-messages/terminal.ansi) · [终端尺寸记录](user-messages/terminal-sizes.json)

画面来自实际 TUI 与 Runtime 的隔离演示对话，按 ANSI 单元格还原为 SVG，再转换成 PNG；不是操作系统窗口截图，字体和调色板可能与用户终端不同。测试模型标识 `fixture-chat-model` 仅代表本机固定回复。未使用真实会话内容、真实凭据或外部模型请求。

## 交付边界

素材保存在仓库内，不依赖 `/tmp` 临时图片。本轮未修改官网页面，未替换已安装 CLI，未安装、发布、推送、创建 tag 或由本会话提交。
