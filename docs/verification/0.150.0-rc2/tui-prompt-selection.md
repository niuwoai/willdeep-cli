# TUI 输入草稿选择复制验证

日期：2026-11-17。源码与验收二进制版本：`0.150.0-rc2`。本记录覆盖输入区选择复制、弹层优先级、SSH 剪贴板路由及相关回归；未安装或发布。

## 验证结果

| 检查 | 结果 |
| --- | --- |
| 输入选区与剪贴板路由专项 Rust 单元测试 | 19 / 19 通过 |
| 编辑器与外部编辑器回归 | 10 / 10 通过 |
| 弹层关闭与输入定位回归 | 11 / 11 通过 |
| 简洁布局回归 | 9 / 9 通过 |
| 代码块复制回归，排除已计入专项的两项 | 11 / 11 通过 |
| Rust 合计，已去重 | 60 / 60 通过 |
| 真实 PTY：本地与模拟 SSH，各含完整及简洁布局 | 14 / 14 通过 |
| 普通 CLI 二进制构建 | 通过 |
| 本轮 Rust 文件定向格式检查 | 通过 |
| Ruby 夹具语法、版本元数据与差异空白检查 | 通过 |

选区测试覆盖原文空白与显式换行、Tab/CRLF、软折行、中文宽字符、组合 emoji、反向拖选、滚动、窗口缩放、编辑/粘贴清除选区和 Ctrl/Cmd+C 的复制决策。完整与简洁布局共用输入选择逻辑。

弹层测试先建立输入选区，再打开帮助、审批、提问或权限面板，确认 Esc 与 Ctrl/Cmd+C 不被输入选区消费；异步审批还实际接收 Esc，断言结果为拒绝且草稿完整。复制路由测试注入终端与本地写入器：SSH 下禁止调用本地剪贴板写入器，保留原始 Unicode、空白和换行；本地成功、终端兜底及失败路径分别验证。

真实 PTY 的四个场景分别检查：跨行草稿显示选区高亮；Esc 清除高亮且保留草稿；再次选择后 `y/q` 进入草稿，未变成聊天选区命令。两个模拟 SSH 场景另发送 Ctrl+C，截获 OSC 52 并解码 Base64，严格核对选区为 `"草稿😀 Alpha\n\n  "`，同时确认状态行显示 `Sent … prompt chars`、程序仍在运行且原草稿仍在。总计 14 项检查。

终端输出由 `@xterm/headless` 解码；夹具将其 Unicode 6 默认表中的测试 emoji 宽度校准为现代终端与 Rust 使用的两格。旧二进制 `0.149.0-rc10` 在未启用 SSH 复制的基线夹具中已完整显示草稿，但 `cross-line highlight` 超时，复现原有缺陷。

新增 Tab/CRLF 测试曾出现一次 `18 passed; 1 failed`：测试把 Tab 当作零列，落点停在末尾 `x` 之前。核对 Ratatui 和 unicode-width 的字符串宽度规则后，改为正确的四列落点，保留原文逐字断言及选区范围断言；重新运行专项为 `19 passed; 0 failed`。

## 复现命令

```bash
cargo test --offline --locked -p willdeep --bin willdeep prompt_selection
```

```bash
cargo test --offline --locked -p willdeep --bin willdeep editor::tests
```

```bash
cargo test --offline --locked -p willdeep --bin willdeep tui::overlay_dismiss::tests
```

```bash
cargo test --offline --locked -p willdeep --bin willdeep tui::quiet_mode::tests
```

```bash
cargo test --offline --locked -p willdeep --bin willdeep tui::code_copy::tests -- --skip prompt_selection
```

```bash
cargo build --offline --locked -p willdeep --bin willdeep
```

```bash
ruby scripts/verify_tui_prompt_selection.rb target/debug/willdeep build/verification/0.150.0-rc2/prompt-selection --ssh-copy
```

```bash
ruby scripts/check_release_version.rb 0.150.0-rc2
```

`--ssh-copy` 仅用于从本轮源码重建、已包含 SSH 路由修复的二进制。验证旧二进制时省略该参数，只运行不发送复制键的本地场景；旧输入复制实现可能仍访问系统剪贴板。

## 隔离与覆盖边界

PTY 使用临时 HOME、WILLDEEP_HOME 和工作区，关闭松手自动复制。只有模拟 SSH 场景发送 Ctrl+C；SSH 环境确保走终端路由，OSC 52 被 PTY 截获且不转发给真实终端。全部场景均未提交提示词、未调用模型、未写入真实系统剪贴板。本地剪贴板的实际写入未执行，路由与原文由注入式 Rust 单元测试验证。

OSC 52 不提供剪贴板接收回执，因此检查的是序列已发送及正文正确，不宣称真实终端或系统剪贴板已经接收。真实 SSH 链路、tmux/GNU screen 兼容性与跨平台 CI 不在这次定向验收范围内。

构建与测试只有既有的 `dead_code` 警告。本轮未运行完整工作区测试，也不将其他会话的检查归入本验收结果。

PTY 的 JSON、Markdown、ANSI 和单元格快照生成在 `build/verification/0.150.0-rc2/prompt-selection/`；该目录已被仓库忽略。可提交的机器可读结果见 [JSON 验收记录](tui-prompt-selection.json)。
